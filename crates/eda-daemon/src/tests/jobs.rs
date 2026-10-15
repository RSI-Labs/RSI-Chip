use super::*;

#[test]
fn rpc_rejects_unknown_methods_and_bad_parameters() {
    let response = dispatch(&RpcRequest::new(1, "doesNotExist"));
    assert!(response.wire().contains("unknown RPC method"));
    let response = dispatch(&RpcRequest::new(2, "spiceDc"));
    assert!(response.wire().contains("source"));
}

#[test]
fn line_protocol_serves_multiple_requests() {
    let input = format!(
        "{}\n\n{}\n",
        RpcRequest::new(1, "ping").wire(),
        RpcRequest::new("two", "ping").wire()
    );
    let mut output = Cursor::new(Vec::new());
    let count = serve_stream(Cursor::new(input), &mut output).unwrap();
    assert_eq!(count, 2);
    let text = String::from_utf8(output.into_inner()).unwrap();
    assert_eq!(text.lines().count(), 2);
    assert!(text.contains("\"id\":1") && text.contains("\"id\":\"two\""));
}

#[test]
fn rpc_service_exposes_asynchronous_jobs() {
    let service = RpcService::new();
    let nested = parse_json(&RpcRequest::new("nested", "ping").wire()).unwrap();
    let submit = service.dispatch(
        &RpcRequest::new(1, "jobSubmit").with_params(Json::object([("request", nested)])),
    );
    let result = submit.result.unwrap();
    let job_id = result.as_object().unwrap()["jobId"].clone();
    assert_eq!(result.as_object().unwrap()["state"], Json::string("queued"));

    let wait = service.dispatch(&RpcRequest::new(2, "jobWait").with_params(Json::object([
        ("jobId", job_id),
        ("timeoutMs", Json::Number(2_000.0)),
    ])));
    let result = wait.result.unwrap();
    let fields = result.as_object().unwrap();
    assert_eq!(fields["state"], Json::string("completed"));
    assert_eq!(
        fields["result"],
        Json::object([
            ("protocol", Json::Number(1.0)),
            ("service", Json::string("eda")),
        ])
    );

    let status = service.dispatch(
        &RpcRequest::new(3, "jobStatus").with_params(Json::object([("jobId", Json::Number(1.0))])),
    );
    assert!(status.wire().contains("\"state\":\"completed\""));

    service
        .manager
        .lock()
        .unwrap()
        .jobs
        .lock()
        .unwrap()
        .insert(99, RpcJobStatus::Queued);
    let cancel = service.dispatch(
        &RpcRequest::new(4, "jobCancel").with_params(Json::object([("jobId", Json::Number(99.0))])),
    );
    assert!(cancel.wire().contains("\"cancelled\":true"));
    service.shutdown().unwrap();
}

#[test]
fn rpc_service_reports_local_worker_pool_stats() {
    let service = RpcService::with_flow_cache_capacity_and_worker_count(4, 2).unwrap();
    let initial = service
        .dispatch(&RpcRequest::new(1, "jobWorkerStats"))
        .result
        .unwrap();
    let initial_fields = initial.as_object().unwrap();
    assert_eq!(initial_fields["workerCount"], Json::from(2_u64));
    assert_eq!(initial_fields["activeJobIds"], Json::Array(Vec::new()));

    let first_nested = parse_json(&RpcRequest::new("first", "ping").wire()).unwrap();
    let first = service
        .dispatch(
            &RpcRequest::new(2, "jobSubmit").with_params(Json::object([("request", first_nested)])),
        )
        .result
        .unwrap();
    let first_id = first.as_object().unwrap()["jobId"].clone();
    let second_nested = parse_json(&RpcRequest::new("second", "ping").wire()).unwrap();
    let second = service
        .dispatch(
            &RpcRequest::new(3, "jobSubmit")
                .with_params(Json::object([("request", second_nested)])),
        )
        .result
        .unwrap();
    let second_id = second.as_object().unwrap()["jobId"].clone();

    for job_id in [first_id.clone(), second_id.clone()] {
        let waited = service
            .dispatch(&RpcRequest::new(4, "jobWait").with_params(Json::object([
                ("jobId", job_id),
                ("timeoutMs", Json::Number(2_000.0)),
            ])))
            .result
            .unwrap();
        assert_eq!(
            waited.as_object().unwrap()["state"],
            Json::string("completed")
        );
    }
    let final_stats = service
        .dispatch(&RpcRequest::new(5, "jobWorkerStats"))
        .result
        .unwrap();
    let fields = final_stats.as_object().unwrap();
    assert_eq!(fields["workerCount"], Json::from(2_u64));
    assert_eq!(fields["completed"], Json::from(2_u64));
    assert_eq!(fields["activeJobIds"], Json::Array(Vec::new()));
    service.shutdown().unwrap();
}

#[test]
fn rpc_service_dispatches_remote_worker_jobs() {
    let service = RpcService::with_flow_cache_capacity_and_worker_count(4, 2).unwrap();
    let initial_workers = service
        .dispatch(&RpcRequest::new(1, "workerList"))
        .result
        .unwrap();
    assert_eq!(
        initial_workers.as_object().unwrap()["count"],
        Json::from(0_u64)
    );

    let registered = service
        .dispatch(
            &RpcRequest::new(2, "workerRegister").with_params(Json::object([
                ("workerId", Json::string("node-1")),
                ("capacity", Json::from(1_u64)),
            ])),
        )
        .result
        .unwrap();
    let registered_fields = registered.as_object().unwrap();
    assert_eq!(registered_fields["workerId"], Json::string("node-1"));
    assert_eq!(registered_fields["capacity"], Json::from(1_u64));

    let nested = parse_json(&RpcRequest::new("remote", "ping").wire()).unwrap();
    let submitted = service
        .dispatch(
            &RpcRequest::new(3, "jobSubmitRemote").with_params(Json::object([("request", nested)])),
        )
        .result
        .unwrap();
    let submit_fields = submitted.as_object().unwrap();
    assert_eq!(submit_fields["scheduled"], Json::string("remote"));
    let job_id = submit_fields["jobId"].clone();

    std::thread::sleep(Duration::from_millis(10));
    let status = service
        .dispatch(
            &RpcRequest::new(4, "jobStatus").with_params(Json::object([("jobId", job_id.clone())])),
        )
        .result
        .unwrap();
    assert_eq!(status.as_object().unwrap()["state"], Json::string("queued"));

    let claim = service
        .dispatch(
            &RpcRequest::new(5, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("node-1"))])),
        )
        .result
        .unwrap();
    let claim_fields = claim.as_object().unwrap();
    assert_eq!(claim_fields["jobId"], job_id);
    assert_eq!(claim_fields["workerId"], Json::string("node-1"));
    let request = claim_fields["request"].as_object().unwrap();
    assert_eq!(request["method"], Json::string("ping"));

    let stats = service
        .dispatch(&RpcRequest::new(6, "jobWorkerStats"))
        .result
        .unwrap();
    let stats_fields = stats.as_object().unwrap();
    assert_eq!(stats_fields["remoteWorkerCount"], Json::from(1_u64));
    assert_eq!(stats_fields["remoteActiveJobCount"], Json::from(1_u64));
    assert_eq!(stats_fields["running"], Json::from(1_u64));

    let heartbeat = service
        .dispatch(
            &RpcRequest::new(7, "workerHeartbeat")
                .with_params(Json::object([("workerId", Json::string("node-1"))])),
        )
        .result
        .unwrap();
    assert_eq!(
        heartbeat.as_object().unwrap()["activeJobIds"],
        Json::Array(vec![job_id.clone()])
    );

    let completed = service
        .dispatch(
            &RpcRequest::new(8, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("node-1")),
                ("jobId", job_id.clone()),
                (
                    "result",
                    Json::object([
                        ("protocol", Json::Number(1.0)),
                        ("service", Json::string("eda")),
                    ]),
                ),
            ])),
        )
        .result
        .unwrap();
    assert_eq!(
        completed.as_object().unwrap()["state"],
        Json::string("completed")
    );
    let final_stats = service
        .dispatch(&RpcRequest::new(9, "jobWorkerStats"))
        .result
        .unwrap();
    let fields = final_stats.as_object().unwrap();
    assert_eq!(fields["remoteActiveJobCount"], Json::from(0_u64));
    assert_eq!(fields["activeJobIds"], Json::Array(Vec::new()));
    service.shutdown().unwrap();
}

#[test]
fn rpc_service_remote_worker_respects_capacity_and_cancellation() {
    let service = RpcService::with_flow_cache_capacity_and_worker_count(4, 2).unwrap();
    service
        .dispatch(
            &RpcRequest::new(1, "workerRegister").with_params(Json::object([
                ("workerId", Json::string("node-1")),
                ("capacity", Json::from(1_u64)),
            ])),
        )
        .result
        .unwrap();

    let submit = |rpc_id: i32, name: &str| {
        let nested = parse_json(&RpcRequest::new(name, "ping").wire()).unwrap();
        service
            .dispatch(
                &RpcRequest::new(rpc_id, "jobSubmitRemote")
                    .with_params(Json::object([("request", nested)])),
            )
            .result
            .unwrap()
            .as_object()
            .unwrap()["jobId"]
            .clone()
    };
    let first = submit(2, "first");
    let second = submit(3, "second");
    let first_claim = service
        .dispatch(
            &RpcRequest::new(4, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("node-1"))])),
        )
        .result
        .unwrap();
    assert_eq!(first_claim.as_object().unwrap()["jobId"], first);
    let no_second = service
        .dispatch(
            &RpcRequest::new(5, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("node-1"))])),
        )
        .result
        .unwrap();
    assert_eq!(no_second, Json::Null);

    let claim_status = service
        .dispatch(
            &RpcRequest::new(6, "workerClaimStatus")
                .with_params(Json::object([("jobId", first.clone())])),
        )
        .result
        .unwrap();
    assert_eq!(
        claim_status.as_object().unwrap()["cancelled"],
        Json::Bool(false)
    );
    let cancel = service
        .dispatch(
            &RpcRequest::new(7, "jobCancel").with_params(Json::object([("jobId", first.clone())])),
        )
        .result
        .unwrap();
    assert_eq!(cancel.as_object().unwrap()["cancelled"], Json::Bool(true));
    let cancelled_status = service
        .dispatch(
            &RpcRequest::new(8, "workerClaimStatus")
                .with_params(Json::object([("jobId", first.clone())])),
        )
        .result
        .unwrap();
    assert_eq!(
        cancelled_status.as_object().unwrap()["cancelled"],
        Json::Bool(true)
    );

    let wrong_owner = service
        .dispatch(
            &RpcRequest::new(9, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("not-owner")),
                ("jobId", first.clone()),
                ("result", Json::Null),
            ])),
        )
        .result;
    assert!(wrong_owner.unwrap_err().contains("does not own job"));
    let completed = service
        .dispatch(
            &RpcRequest::new(10, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("node-1")),
                ("jobId", first.clone()),
                ("result", Json::Null),
            ])),
        )
        .result
        .unwrap();
    assert_eq!(
        completed.as_object().unwrap()["state"],
        Json::string("cancelled")
    );

    let second_claim = service
        .dispatch(
            &RpcRequest::new(11, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("node-1"))])),
        )
        .result
        .unwrap();
    assert_eq!(second_claim.as_object().unwrap()["jobId"], second);
    let failed = service
        .dispatch(
            &RpcRequest::new(12, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("node-1")),
                ("jobId", second),
                ("error", Json::string("remote execution failed")),
            ])),
        )
        .result
        .unwrap();
    assert_eq!(failed.as_object().unwrap()["state"], Json::string("failed"));
    service.shutdown().unwrap();
}

#[test]
fn rpc_service_remote_worker_artifacts_are_checksummed_and_managed() {
    let service = RpcService::with_flow_cache_capacity_and_worker_count(4, 2).unwrap();
    service
        .dispatch(
            &RpcRequest::new(1, "workerRegister").with_params(Json::object([
                ("workerId", Json::string("node-1")),
                ("capacity", Json::from(4_u64)),
                ("leaseTimeoutSeconds", Json::from(60_u64)),
            ])),
        )
        .result
        .unwrap();

    let nested = parse_json(&RpcRequest::new("artifact-job", "ping").wire()).unwrap();
    let submitted = service
        .dispatch(
            &RpcRequest::new(2, "jobSubmitRemote").with_params(Json::object([("request", nested)])),
        )
        .result
        .unwrap();
    let fields = submitted.as_object().unwrap();
    let job_id = fields["jobId"].clone();
    let claim = service
        .dispatch(
            &RpcRequest::new(3, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("node-1"))])),
        )
        .result
        .unwrap();
    assert_eq!(claim.as_object().unwrap()["jobId"], job_id);

    let duplicate = service
        .dispatch(
            &RpcRequest::new(4, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("node-1")),
                ("jobId", job_id.clone()),
                ("result", Json::Null),
                (
                    "artifacts",
                    Json::Array(vec![
                        Json::object([
                            ("name", Json::string("report")),
                            ("content", Json::string("first")),
                        ]),
                        Json::object([
                            ("name", Json::string("report")),
                            ("content", Json::string("second")),
                        ]),
                    ]),
                ),
            ])),
        )
        .result;
    assert!(duplicate
        .unwrap_err()
        .contains("duplicate remote RPC artifact name"));

    let stale_owner = service
        .dispatch(
            &RpcRequest::new(5, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("node-2")),
                ("jobId", job_id.clone()),
                ("result", Json::Null),
                (
                    "artifacts",
                    Json::Array(vec![Json::object([
                        ("name", Json::string("stale")),
                        ("content", Json::string("must not persist")),
                    ])]),
                ),
            ])),
        )
        .result;
    assert!(stale_owner.unwrap_err().contains("does not own job"));
    let before_completion = service
        .dispatch(
            &RpcRequest::new(6, "jobArtifactList")
                .with_params(Json::object([("jobId", job_id.clone())])),
        )
        .result
        .unwrap();
    assert_eq!(
        before_completion.as_object().unwrap()["count"],
        Json::from(0_u64)
    );

    let report = "signoff report\n";
    let waveform = r#"{"vcd":"stub"}"#;
    let completed = service
        .dispatch(
            &RpcRequest::new(7, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("node-1")),
                ("jobId", job_id.clone()),
                ("result", Json::Null),
                (
                    "artifacts",
                    Json::Array(vec![
                        Json::object([
                            ("name", Json::string("report.txt")),
                            ("content", Json::string(report)),
                        ]),
                        Json::object([
                            ("name", Json::string("waveform.json")),
                            ("contentType", Json::string("application/json")),
                            ("content", Json::string(waveform)),
                        ]),
                    ]),
                ),
            ])),
        )
        .result
        .unwrap();
    assert_eq!(
        completed.as_object().unwrap()["state"],
        Json::string("completed")
    );

    let listed = service
        .dispatch(
            &RpcRequest::new(8, "jobArtifactList")
                .with_params(Json::object([("jobId", job_id.clone())])),
        )
        .result
        .unwrap();
    let list_fields = listed.as_object().unwrap();
    assert_eq!(list_fields["count"], Json::from(2_u64));
    assert!(!listed.write().contains("signoff report"));
    let Json::Array(artifacts) = &list_fields["artifacts"] else {
        panic!("artifact list must return an array");
    };
    assert_eq!(
        artifacts[0].as_object().unwrap()["contentType"],
        Json::string("text/plain")
    );
    assert_eq!(
        artifacts[1].as_object().unwrap()["contentType"],
        Json::string("application/json")
    );

    let fetched = service
        .dispatch(
            &RpcRequest::new(9, "jobArtifactGet").with_params(Json::object([
                ("jobId", job_id.clone()),
                ("name", Json::string("report.txt")),
            ])),
        )
        .result
        .unwrap();
    let fetched_fields = fetched.as_object().unwrap();
    assert_eq!(fetched_fields["content"], Json::string(report));
    assert_eq!(
        fetched_fields["sha256"],
        Json::string(eda_api::pdk_liberty_sha256(report))
    );
    assert_eq!(fetched_fields["sizeBytes"], Json::from(report.len() as u64));

    let deleted = service
        .dispatch(
            &RpcRequest::new(10, "jobArtifactDelete").with_params(Json::object([
                ("jobId", job_id.clone()),
                ("name", Json::string("waveform.json")),
            ])),
        )
        .result
        .unwrap();
    assert_eq!(
        deleted.as_object().unwrap()["sha256"],
        Json::string(eda_api::pdk_liberty_sha256(waveform))
    );
    let missing = service
        .dispatch(
            &RpcRequest::new(11, "jobArtifactGet").with_params(Json::object([
                ("jobId", job_id.clone()),
                ("name", Json::string("waveform.json")),
            ])),
        )
        .result;
    assert!(missing.unwrap_err().contains("has no artifact"));

    let failed_nested = parse_json(&RpcRequest::new("diagnostic", "ping").wire()).unwrap();
    let failed_submitted = service
        .dispatch(
            &RpcRequest::new(12, "jobSubmitRemote")
                .with_params(Json::object([("request", failed_nested)])),
        )
        .result
        .unwrap();
    let failed_job_id = failed_submitted.as_object().unwrap()["jobId"].clone();
    service
        .dispatch(
            &RpcRequest::new(13, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("node-1"))])),
        )
        .result
        .unwrap();
    let failed = service
        .dispatch(
            &RpcRequest::new(14, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("node-1")),
                ("jobId", failed_job_id.clone()),
                ("error", Json::string("worker diagnostics")),
                (
                    "artifacts",
                    Json::Array(vec![Json::object([
                        ("name", Json::string("worker.log")),
                        ("content", Json::string("failure diagnostic")),
                    ])]),
                ),
            ])),
        )
        .result
        .unwrap();
    assert_eq!(failed.as_object().unwrap()["state"], Json::string("failed"));
    let diagnostic = service
        .dispatch(
            &RpcRequest::new(15, "jobArtifactGet").with_params(Json::object([
                ("jobId", failed_job_id),
                ("name", Json::string("worker.log")),
            ])),
        )
        .result
        .unwrap();
    assert_eq!(
        diagnostic.as_object().unwrap()["content"],
        Json::string("failure diagnostic")
    );

    let cancelled_nested = parse_json(&RpcRequest::new("cancelled", "ping").wire()).unwrap();
    let cancelled_submitted = service
        .dispatch(
            &RpcRequest::new(16, "jobSubmitRemote")
                .with_params(Json::object([("request", cancelled_nested)])),
        )
        .result
        .unwrap();
    let cancelled_job_id = cancelled_submitted.as_object().unwrap()["jobId"].clone();
    service
        .dispatch(
            &RpcRequest::new(17, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("node-1"))])),
        )
        .result
        .unwrap();
    service
        .dispatch(
            &RpcRequest::new(18, "jobCancel")
                .with_params(Json::object([("jobId", cancelled_job_id.clone())])),
        )
        .result
        .unwrap();
    let cancelled = service
        .dispatch(
            &RpcRequest::new(19, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("node-1")),
                ("jobId", cancelled_job_id.clone()),
                ("result", Json::Null),
                (
                    "artifacts",
                    Json::Array(vec![Json::object([
                        ("name", Json::string("partial.txt")),
                        ("content", Json::string("partial")),
                    ])]),
                ),
            ])),
        )
        .result
        .unwrap();
    assert_eq!(
        cancelled.as_object().unwrap()["state"],
        Json::string("cancelled")
    );
    let cancelled_artifacts = service
        .dispatch(
            &RpcRequest::new(20, "jobArtifactList")
                .with_params(Json::object([("jobId", cancelled_job_id)])),
        )
        .result
        .unwrap();
    assert_eq!(
        cancelled_artifacts.as_object().unwrap()["count"],
        Json::from(0_u64)
    );
    service.shutdown().unwrap();
}

#[test]
fn rpc_service_remote_artifact_store_survives_restart_and_preserves_ids() {
    let unique = format!(
        "eda-rpc-artifact-store-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(&unique);
    let service =
        RpcService::with_flow_cache_capacity_worker_count_and_artifact_store(4, 2, Some(&root))
            .unwrap();
    service
        .dispatch(
            &RpcRequest::new(1, "workerRegister").with_params(Json::object([(
                "workerId",
                Json::string("persistent-node"),
            )])),
        )
        .result
        .unwrap();
    let nested = parse_json(&RpcRequest::new("persisted", "ping").wire()).unwrap();
    let submitted = service
        .dispatch(
            &RpcRequest::new(2, "jobSubmitRemote").with_params(Json::object([("request", nested)])),
        )
        .result
        .unwrap();
    let job_id = submitted.as_object().unwrap()["jobId"].clone();
    service
        .dispatch(
            &RpcRequest::new(3, "workerClaim").with_params(Json::object([(
                "workerId",
                Json::string("persistent-node"),
            )])),
        )
        .result
        .unwrap();
    service
        .dispatch(
            &RpcRequest::new(4, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("persistent-node")),
                ("jobId", job_id.clone()),
                (
                    "result",
                    Json::object([
                        ("protocol", Json::from(PROTOCOL_VERSION as u64)),
                        ("service", Json::string("eda")),
                    ]),
                ),
                (
                    "artifacts",
                    Json::Array(vec![Json::object([
                        ("name", Json::string("report.txt")),
                        ("content", Json::string("persistent report")),
                    ])]),
                ),
            ])),
        )
        .result
        .unwrap();
    service.shutdown().unwrap();

    let restarted =
        RpcService::with_flow_cache_capacity_worker_count_and_artifact_store(4, 2, Some(&root))
            .unwrap();
    let restored_status = restarted
        .dispatch(&RpcRequest::new(1, "jobStatus").with_params(Json::object([("jobId", job_id)])))
        .result
        .unwrap();
    assert_eq!(
        restored_status.as_object().unwrap()["state"],
        Json::string("completed")
    );
    let restored_artifact = restarted
        .dispatch(
            &RpcRequest::new(2, "jobArtifactGet").with_params(Json::object([
                ("jobId", Json::from(1_u64)),
                ("name", Json::string("report.txt")),
            ])),
        )
        .result
        .unwrap();
    let artifact_fields = restored_artifact.as_object().unwrap();
    assert_eq!(
        artifact_fields["content"],
        Json::string("persistent report")
    );
    assert_eq!(
        artifact_fields["sha256"],
        Json::string(eda_api::pdk_liberty_sha256("persistent report"))
    );

    restarted
        .dispatch(
            &RpcRequest::new(3, "workerRegister").with_params(Json::object([(
                "workerId",
                Json::string("persistent-node"),
            )])),
        )
        .result
        .unwrap();
    let second_nested = parse_json(&RpcRequest::new("uncompleted", "ping").wire()).unwrap();
    let second_submitted = restarted
        .dispatch(
            &RpcRequest::new(4, "jobSubmitRemote")
                .with_params(Json::object([("request", second_nested)])),
        )
        .result
        .unwrap();
    assert_eq!(
        second_submitted.as_object().unwrap()["jobId"],
        Json::from(2_u64)
    );
    restarted.shutdown().unwrap();

    let third =
        RpcService::with_flow_cache_capacity_worker_count_and_artifact_store(4, 2, Some(&root))
            .unwrap();
    third
        .dispatch(
            &RpcRequest::new(1, "workerRegister").with_params(Json::object([(
                "workerId",
                Json::string("persistent-node"),
            )])),
        )
        .result
        .unwrap();
    let third_nested = parse_json(&RpcRequest::new("watermark", "ping").wire()).unwrap();
    let third_submitted = third
        .dispatch(
            &RpcRequest::new(2, "jobSubmitRemote")
                .with_params(Json::object([("request", third_nested)])),
        )
        .result
        .unwrap();
    assert_eq!(
        third_submitted.as_object().unwrap()["jobId"],
        Json::from(3_u64)
    );
    let checksum_root = std::env::temp_dir().join(format!("{unique}-bad-checksum"));
    fs::create_dir_all(&checksum_root).unwrap();
    let mut checksum_record =
        parse_json(&fs::read_to_string(root.join("1.json")).unwrap()).unwrap();
    let Json::Object(fields) = &mut checksum_record else {
        panic!("persisted job record must be an object");
    };
    let Json::Array(artifacts) = fields.get_mut("artifacts").unwrap() else {
        panic!("persisted job artifacts must be an array");
    };
    let Json::Object(artifact) = &mut artifacts[0] else {
        panic!("persisted artifact must be an object");
    };
    artifact.insert("content".into(), Json::string("tampered"));
    write_remote_job_record(&checksum_root, &checksum_record).unwrap();
    let bad_checksum = RpcService::with_flow_cache_capacity_worker_count_and_artifact_store(
        4,
        2,
        Some(&checksum_root),
    );
    let bad_checksum_error = match bad_checksum {
        Ok(service) => {
            service.shutdown().unwrap();
            panic!("tampered artifact checksum unexpectedly loaded");
        }
        Err(error) => error,
    };
    assert!(bad_checksum_error.contains("checksum mismatch"));
    let deleted = third
        .dispatch(
            &RpcRequest::new(3, "jobArtifactDelete").with_params(Json::object([
                ("jobId", Json::from(1_u64)),
                ("name", Json::string("report.txt")),
            ])),
        )
        .result
        .unwrap();
    assert_eq!(deleted.as_object().unwrap()["deleted"], Json::Bool(true));
    third.shutdown().unwrap();

    let after_delete =
        RpcService::with_flow_cache_capacity_worker_count_and_artifact_store(4, 2, Some(&root))
            .unwrap();
    let empty = after_delete
        .dispatch(
            &RpcRequest::new(1, "jobArtifactList")
                .with_params(Json::object([("jobId", Json::from(1_u64))])),
        )
        .result
        .unwrap();
    assert_eq!(empty.as_object().unwrap()["count"], Json::from(0_u64));
    after_delete.shutdown().unwrap();

    let corrupt_root = std::env::temp_dir().join(format!("{unique}-corrupt"));
    fs::create_dir_all(&corrupt_root).unwrap();
    fs::write(corrupt_root.join("1.json"), "not-json").unwrap();
    let corrupt = RpcService::with_flow_cache_capacity_worker_count_and_artifact_store(
        4,
        2,
        Some(&corrupt_root),
    );
    let corrupt_error = match corrupt {
        Ok(service) => {
            service.shutdown().unwrap();
            panic!("corrupt artifact store unexpectedly loaded");
        }
        Err(error) => error,
    };
    assert!(!corrupt_error.is_empty());
    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(&checksum_root).unwrap();
    fs::remove_dir_all(&corrupt_root).unwrap();
}

#[test]
fn rpc_service_remote_worker_artifact_quotas_and_pagination() {
    let oversized = "x".repeat(MAX_REMOTE_JOB_ARTIFACT_BYTES_PER_JOB / 2 + 1);
    let quota_params = Json::object([
        ("jobId", Json::from(1_u64)),
        (
            "artifacts",
            Json::Array(vec![
                Json::object([
                    ("name", Json::string("first.bin")),
                    ("content", Json::string(oversized.clone())),
                ]),
                Json::object([
                    ("name", Json::string("second.bin")),
                    ("content", Json::string(oversized)),
                ]),
            ]),
        ),
    ]);
    assert!(parse_remote_job_artifacts(&quota_params, 1)
        .unwrap_err()
        .contains("decoded artifact bytes"));

    let service = RpcService::with_flow_cache_capacity_and_worker_count(4, 2).unwrap();
    service
        .dispatch(
            &RpcRequest::new(1, "workerRegister")
                .with_params(Json::object([("workerId", Json::string("quota-node"))])),
        )
        .result
        .unwrap();
    let nested = parse_json(&RpcRequest::new("quota", "ping").wire()).unwrap();
    let submitted = service
        .dispatch(
            &RpcRequest::new(2, "jobSubmitRemote").with_params(Json::object([("request", nested)])),
        )
        .result
        .unwrap();
    let job_id = submitted.as_object().unwrap()["jobId"].clone();
    service
        .dispatch(
            &RpcRequest::new(3, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("quota-node"))])),
        )
        .result
        .unwrap();

    let too_many = (0..=MAX_REMOTE_JOB_ARTIFACTS_PER_JOB)
        .map(|index| {
            Json::object([
                ("name", Json::string(format!("artifact-{index}"))),
                ("content", Json::string("x")),
            ])
        })
        .collect::<Vec<_>>();
    let rejected = service
        .dispatch(
            &RpcRequest::new(4, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("quota-node")),
                ("jobId", job_id.clone()),
                ("result", Json::Null),
                ("artifacts", Json::Array(too_many)),
            ])),
        )
        .result;
    assert!(rejected.unwrap_err().contains("at most 64 artifacts"));

    let three_artifacts = ["alpha", "beta", "gamma"]
        .into_iter()
        .map(|name| {
            Json::object([
                ("name", Json::string(format!("{name}.log"))),
                ("content", Json::string(name)),
            ])
        })
        .collect::<Vec<_>>();
    service
        .dispatch(
            &RpcRequest::new(5, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("quota-node")),
                ("jobId", job_id.clone()),
                ("result", Json::Null),
                ("artifacts", Json::Array(three_artifacts)),
            ])),
        )
        .result
        .unwrap();

    let default_list = service
        .dispatch(
            &RpcRequest::new(6, "jobArtifactList")
                .with_params(Json::object([("jobId", job_id.clone())])),
        )
        .result
        .unwrap();
    let default_fields = default_list.as_object().unwrap();
    assert_eq!(default_fields["total"], Json::from(3_u64));
    assert_eq!(default_fields["count"], Json::from(3_u64));
    assert_eq!(default_fields["offset"], Json::from(0_u64));
    assert_eq!(default_fields["limit"], Json::from(256_u64));

    let page = service
        .dispatch(
            &RpcRequest::new(7, "jobArtifactList").with_params(Json::object([
                ("jobId", job_id.clone()),
                ("offset", Json::from(1_u64)),
                ("limit", Json::from(1_u64)),
            ])),
        )
        .result
        .unwrap();
    let page_fields = page.as_object().unwrap();
    assert_eq!(page_fields["total"], Json::from(3_u64));
    assert_eq!(page_fields["count"], Json::from(1_u64));
    let Json::Array(page_artifacts) = &page_fields["artifacts"] else {
        panic!("artifact page must be an array");
    };
    assert_eq!(
        page_artifacts[0].as_object().unwrap()["name"],
        Json::string("beta.log")
    );

    let beyond_end = service
        .dispatch(
            &RpcRequest::new(8, "jobArtifactList").with_params(Json::object([
                ("jobId", job_id.clone()),
                ("offset", Json::from(3_u64)),
                ("limit", Json::from(2_u64)),
            ])),
        )
        .result
        .unwrap();
    assert_eq!(beyond_end.as_object().unwrap()["count"], Json::from(0_u64));

    assert!(service
        .dispatch(
            &RpcRequest::new(9, "jobArtifactList").with_params(Json::object([
                ("jobId", job_id),
                ("limit", Json::from(0_u64)),
            ])),
        )
        .result
        .unwrap_err()
        .contains("'limit' must be a positive integer"));
    assert!(service
        .dispatch(
            &RpcRequest::new(10, "jobArtifactList").with_params(Json::object([
                ("jobId", Json::from(1_u64)),
                ("limit", Json::from(257_u64)),
            ])),
        )
        .result
        .unwrap_err()
        .contains("'limit' cannot exceed 256"));
    service.shutdown().unwrap();
}

#[test]
fn rpc_service_remote_worker_binary_artifacts_use_base64_checksums() {
    let service = RpcService::with_flow_cache_capacity_and_worker_count(4, 2).unwrap();
    service
        .dispatch(
            &RpcRequest::new(1, "workerRegister")
                .with_params(Json::object([("workerId", Json::string("binary-node"))])),
        )
        .result
        .unwrap();
    let nested = parse_json(&RpcRequest::new("gds", "ping").wire()).unwrap();
    let submitted = service
        .dispatch(
            &RpcRequest::new(2, "jobSubmitRemote").with_params(Json::object([("request", nested)])),
        )
        .result
        .unwrap();
    let job_id = submitted.as_object().unwrap()["jobId"].clone();
    service
        .dispatch(
            &RpcRequest::new(3, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("binary-node"))])),
        )
        .result
        .unwrap();

    let unsupported_encoding = service
        .dispatch(
            &RpcRequest::new(4, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("binary-node")),
                ("jobId", job_id.clone()),
                ("result", Json::Null),
                (
                    "artifacts",
                    Json::Array(vec![Json::object([
                        ("name", Json::string("layout.gds")),
                        ("contentEncoding", Json::string("hex")),
                        ("content", Json::string("001f")),
                    ])]),
                ),
            ])),
        )
        .result;
    assert!(unsupported_encoding
        .unwrap_err()
        .contains("'contentEncoding' must be either 'utf-8' or 'base64'"));
    let invalid_base64 = service
        .dispatch(
            &RpcRequest::new(5, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("binary-node")),
                ("jobId", job_id.clone()),
                ("result", Json::Null),
                (
                    "artifacts",
                    Json::Array(vec![Json::object([
                        ("name", Json::string("layout.gds")),
                        ("contentEncoding", Json::string("base64")),
                        ("content", Json::string("A")),
                    ])]),
                ),
            ])),
        )
        .result;
    assert!(invalid_base64
        .unwrap_err()
        .contains("padded four-byte groups"));

    let bytes = [0_u8, 1, 2, 255, 100];
    let encoded = base64_encode(&bytes);
    assert_eq!(base64_decode("YWJj").unwrap(), b"abc".as_slice());
    assert!(base64_decode("YR==").is_err());
    assert!(base64_decode("YW Jj").is_err());
    let completed = service
        .dispatch(
            &RpcRequest::new(6, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("binary-node")),
                ("jobId", job_id.clone()),
                ("result", Json::Null),
                (
                    "artifacts",
                    Json::Array(vec![Json::object([
                        ("name", Json::string("layout.gds")),
                        ("contentType", Json::string("application/octet-stream")),
                        ("contentEncoding", Json::string("base64")),
                        ("content", Json::string(encoded.clone())),
                    ])]),
                ),
            ])),
        )
        .result
        .unwrap();
    assert_eq!(
        completed.as_object().unwrap()["state"],
        Json::string("completed")
    );

    let artifact = service
        .dispatch(
            &RpcRequest::new(7, "jobArtifactGet").with_params(Json::object([
                ("jobId", job_id),
                ("name", Json::string("layout.gds")),
            ])),
        )
        .result
        .unwrap();
    let fields = artifact.as_object().unwrap();
    assert_eq!(fields["content"], Json::string(encoded));
    assert_eq!(fields["contentEncoding"], Json::string("base64"));
    assert_eq!(fields["sizeBytes"], Json::from(bytes.len() as u64));
    assert_eq!(
        fields["sha256"],
        Json::string(eda_api::pdk_liberty_sha256_bytes(&bytes))
    );
    assert_eq!(
        base64_decode(fields["content"].as_str().unwrap()).unwrap(),
        bytes
    );
    service.shutdown().unwrap();
}

#[test]
fn rpc_service_remote_worker_artifact_registry_is_globally_bounded_lru() {
    fn complete_with_artifact(
        service: &RpcService,
        worker_id: &str,
        label: &str,
    ) -> Result<u64, String> {
        let nested = parse_json(&RpcRequest::new(label, "ping").wire()).unwrap();
        let submitted = service
            .dispatch(
                &RpcRequest::new(1, "jobSubmitRemote")
                    .with_params(Json::object([("request", nested)])),
            )
            .result?;
        let job_id = submitted.as_object().unwrap()["jobId"]
            .as_number()
            .copied()
            .unwrap() as u64;
        service
            .dispatch(
                &RpcRequest::new(2, "workerClaim")
                    .with_params(Json::object([("workerId", Json::string(worker_id))])),
            )
            .result?;
        service
            .dispatch(
                &RpcRequest::new(3, "workerComplete").with_params(Json::object([
                    ("workerId", Json::string(worker_id)),
                    ("jobId", Json::from(job_id)),
                    ("result", Json::Null),
                    (
                        "artifacts",
                        Json::Array(vec![Json::object([
                            ("name", Json::string(format!("{label}.log"))),
                            ("content", Json::string(label)),
                        ])]),
                    ),
                ])),
            )
            .result?;
        Ok(job_id)
    }

    let service = RpcService::with_flow_cache_capacity_and_worker_count(4, 2).unwrap();
    service
        .dispatch(
            &RpcRequest::new(1, "workerRegister")
                .with_params(Json::object([("workerId", Json::string("lru-node"))])),
        )
        .result
        .unwrap();
    {
        let mut manager = service.manager.lock().unwrap();
        manager.remote_artifact_capacity = 2;
    }

    let first = complete_with_artifact(&service, "lru-node", "first").unwrap();
    let second = complete_with_artifact(&service, "lru-node", "second").unwrap();
    let touched = service
        .dispatch(
            &RpcRequest::new(4, "jobArtifactGet").with_params(Json::object([
                ("jobId", Json::from(first)),
                ("name", Json::string("first.log")),
            ])),
        )
        .result
        .unwrap();
    assert_eq!(
        touched.as_object().unwrap()["name"],
        Json::string("first.log")
    );
    let third = complete_with_artifact(&service, "lru-node", "third").unwrap();

    let evicted = service
        .dispatch(
            &RpcRequest::new(5, "jobArtifactList")
                .with_params(Json::object([("jobId", Json::from(second))])),
        )
        .result
        .unwrap();
    assert_eq!(evicted.as_object().unwrap()["count"], Json::from(0_u64));
    for (job_id, name) in [(first, "first.log"), (third, "third.log")] {
        let retained = service
            .dispatch(
                &RpcRequest::new(6, "jobArtifactGet").with_params(Json::object([
                    ("jobId", Json::from(job_id)),
                    ("name", Json::string(name)),
                ])),
            )
            .result
            .unwrap();
        assert_eq!(retained.as_object().unwrap()["name"], Json::string(name));
    }
    let count_stats = service
        .dispatch(&RpcRequest::new(7, "jobWorkerStats"))
        .result
        .unwrap();
    let count_fields = count_stats.as_object().unwrap();
    assert_eq!(count_fields["remoteArtifactCount"], Json::from(2_u64));
    assert_eq!(count_fields["remoteArtifactCapacity"], Json::from(2_u64));
    assert_eq!(count_fields["remoteArtifactEvictions"], Json::from(1_u64));
    service.shutdown().unwrap();

    let byte_service = RpcService::with_flow_cache_capacity_and_worker_count(4, 2).unwrap();
    byte_service
        .dispatch(
            &RpcRequest::new(1, "workerRegister")
                .with_params(Json::object([("workerId", Json::string("byte-node"))])),
        )
        .result
        .unwrap();
    {
        let mut manager = byte_service.manager.lock().unwrap();
        manager.remote_artifact_byte_capacity = 4;
    }
    let wide = complete_with_artifact(&byte_service, "byte-node", "abc").unwrap();
    let _narrow = complete_with_artifact(&byte_service, "byte-node", "de").unwrap();
    let missing = byte_service
        .dispatch(
            &RpcRequest::new(2, "jobArtifactGet").with_params(Json::object([
                ("jobId", Json::from(wide)),
                ("name", Json::string("abc.log")),
            ])),
        )
        .result;
    assert!(missing.unwrap_err().contains("has no artifact"));
    let byte_stats = byte_service
        .dispatch(&RpcRequest::new(3, "jobWorkerStats"))
        .result
        .unwrap();
    let byte_fields = byte_stats.as_object().unwrap();
    assert_eq!(byte_fields["remoteArtifactBytes"], Json::from(2_u64));
    assert_eq!(byte_fields["remoteArtifactByteCapacity"], Json::from(4_u64));
    assert_eq!(byte_fields["remoteArtifactEvictions"], Json::from(1_u64));
    byte_service.shutdown().unwrap();
}

#[test]
fn rpc_service_remote_jobs_claim_highest_priority_first() {
    let service = RpcService::with_flow_cache_capacity_and_worker_count(4, 2).unwrap();
    service
        .dispatch(
            &RpcRequest::new(1, "workerRegister").with_params(Json::object([
                ("workerId", Json::string("priority-node")),
                ("capacity", Json::from(1_u64)),
            ])),
        )
        .result
        .unwrap();

    let submissions = [
        ("low", 1_u32),
        ("high", 100_u32),
        ("default", 0_u32),
        ("medium", 10_u32),
    ];
    let mut job_ids = Vec::new();
    for (index, (name, priority)) in submissions.iter().enumerate() {
        let nested = parse_json(&RpcRequest::new(*name, "ping").wire()).unwrap();
        let submitted = service
            .dispatch(
                &RpcRequest::new(index as i64 + 2, "jobSubmitRemote").with_params(Json::object([
                    ("request", nested),
                    ("priority", Json::from(u64::from(*priority))),
                ])),
            )
            .result
            .unwrap();
        let fields = submitted.as_object().unwrap();
        assert_eq!(fields["priority"], Json::from(u64::from(*priority)));
        job_ids.push(fields["jobId"].clone());
    }

    for (expected_job_id, expected_priority) in [
        (job_ids[1].clone(), 100_u32),
        (job_ids[3].clone(), 10_u32),
        (job_ids[0].clone(), 1_u32),
        (job_ids[2].clone(), 0_u32),
    ] {
        let claim = service
            .dispatch(
                &RpcRequest::new(20, "workerClaim")
                    .with_params(Json::object([("workerId", Json::string("priority-node"))])),
            )
            .result
            .unwrap();
        let fields = claim.as_object().unwrap();
        assert_eq!(fields["jobId"], expected_job_id);
        assert_eq!(fields["priority"], Json::from(u64::from(expected_priority)));
        service
            .dispatch(
                &RpcRequest::new(21, "workerComplete").with_params(Json::object([
                    ("workerId", Json::string("priority-node")),
                    ("jobId", expected_job_id),
                    ("result", Json::Null),
                ])),
            )
            .result
            .unwrap();
    }

    let invalid = service
        .dispatch(
            &RpcRequest::new(22, "jobSubmitRemote").with_params(Json::object([
                (
                    "request",
                    parse_json(&RpcRequest::new("bad", "ping").wire()).unwrap(),
                ),
                ("priority", Json::Number(-1.0)),
            ])),
        )
        .result;
    assert!(invalid
        .unwrap_err()
        .contains("'priority' must be a non-negative u32"));
    service.shutdown().unwrap();
}

#[test]
fn rpc_service_expires_idle_remote_workers_and_keeps_active_leases() {
    let service = RpcService::with_flow_cache_capacity_and_worker_count(4, 2).unwrap();
    for worker_id in ["fresh-node", "stale-node", "active-node"] {
        service
            .dispatch(
                &RpcRequest::new(worker_id, "workerRegister").with_params(Json::object([
                    ("workerId", Json::string(worker_id)),
                    ("leaseTimeoutSeconds", Json::from(60_u64)),
                ])),
            )
            .result
            .unwrap();
    }
    {
        let manager = service.manager.lock().unwrap();
        let mut workers = manager.remote_workers.lock().unwrap();
        workers.get_mut("stale-node").unwrap().last_heartbeat =
            Instant::now() - Duration::from_secs(61);
    }

    let expiration = service
        .dispatch(&RpcRequest::new(4, "workerExpireLeases"))
        .result
        .unwrap();
    let fields = expiration.as_object().unwrap();
    assert_eq!(fields["expiredCount"], Json::from(0_u64));
    assert_eq!(fields["removedWorkerCount"], Json::from(1_u64));
    assert_eq!(
        fields["removedWorkerIds"],
        Json::Array(vec![Json::string("stale-node")])
    );

    let workers = service
        .dispatch(&RpcRequest::new(5, "workerList"))
        .result
        .unwrap();
    assert!(workers.write().contains("fresh-node"));
    assert!(!workers.write().contains("stale-node"));

    let nested = parse_json(&RpcRequest::new("active", "ping").wire()).unwrap();
    let submitted = service
        .dispatch(
            &RpcRequest::new(6, "jobSubmitRemote").with_params(Json::object([("request", nested)])),
        )
        .result
        .unwrap();
    let job_id = submitted.as_object().unwrap()["jobId"].clone();
    service
        .dispatch(
            &RpcRequest::new(7, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("active-node"))])),
        )
        .result
        .unwrap();
    {
        let manager = service.manager.lock().unwrap();
        let mut workers = manager.remote_workers.lock().unwrap();
        workers.get_mut("active-node").unwrap().last_heartbeat =
            Instant::now() - Duration::from_secs(61);
    }
    let active_expiration = service
        .dispatch(&RpcRequest::new(8, "workerExpireLeases"))
        .result
        .unwrap();
    let active_fields = active_expiration.as_object().unwrap();
    assert_eq!(active_fields["expiredCount"], Json::from(0_u64));
    assert_eq!(active_fields["removedWorkerCount"], Json::from(0_u64));
    assert_eq!(active_fields["removedWorkerIds"], Json::Array(Vec::new()));

    let stats = service
        .dispatch(&RpcRequest::new(9, "jobWorkerStats"))
        .result
        .unwrap();
    assert_eq!(
        stats.as_object().unwrap()["remoteWorkerExpirations"],
        Json::from(1_u64)
    );
    assert_eq!(
        stats.as_object().unwrap()["remoteActiveJobCount"],
        Json::from(1_u64)
    );
    let _ = job_id;
    service.shutdown().unwrap();
}

#[test]
fn rpc_service_remote_worker_labels_route_claims() {
    let service = RpcService::with_flow_cache_capacity_and_worker_count(4, 2).unwrap();
    let invalid_labels = service
        .dispatch(
            &RpcRequest::new(1, "workerRegister").with_params(Json::object([
                ("workerId", Json::string("bad-node")),
                ("labels", Json::object([("pool", Json::from(7_i64))])),
            ])),
        )
        .result;
    assert!(invalid_labels
        .unwrap_err()
        .contains("'labels' values must be strings"));

    service
        .dispatch(
            &RpcRequest::new(2, "workerRegister").with_params(Json::object([
                ("workerId", Json::string("sim-node")),
                (
                    "labels",
                    Json::object([("pool", Json::string("simulation"))]),
                ),
            ])),
        )
        .result
        .unwrap();
    service
        .dispatch(
            &RpcRequest::new(3, "workerRegister").with_params(Json::object([
                ("workerId", Json::string("signoff-node")),
                ("capacity", Json::from(4_u64)),
                (
                    "labels",
                    Json::object([
                        ("pool", Json::string("signoff")),
                        ("license", Json::string("foundry")),
                    ]),
                ),
            ])),
        )
        .result
        .unwrap();

    let workers = service
        .dispatch(&RpcRequest::new(4, "workerList"))
        .result
        .unwrap();
    let worker_text = workers.write();
    assert!(worker_text.contains("\"pool\":\"simulation\""));
    assert!(worker_text.contains("\"pool\":\"signoff\""));
    assert!(worker_text.contains("\"license\":\"foundry\""));

    let nested = parse_json(&RpcRequest::new("routed", "ping").wire()).unwrap();
    let submitted = service
        .dispatch(
            &RpcRequest::new(5, "jobSubmitRemote").with_params(Json::object([
                ("request", nested),
                (
                    "requiredWorkerLabels",
                    Json::object([("pool", Json::string("signoff"))]),
                ),
            ])),
        )
        .result
        .unwrap();
    let job_id = submitted.as_object().unwrap()["jobId"].clone();
    let wrong_worker = service
        .dispatch(
            &RpcRequest::new(6, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("sim-node"))])),
        )
        .result
        .unwrap();
    assert_eq!(wrong_worker, Json::Null);

    let matching_worker = service
        .dispatch(
            &RpcRequest::new(7, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("signoff-node"))])),
        )
        .result
        .unwrap();
    let claim_fields = matching_worker.as_object().unwrap();
    assert_eq!(claim_fields["jobId"], job_id);
    assert_eq!(
        claim_fields["requiredWorkerLabels"],
        Json::object([("pool", Json::string("signoff"))])
    );

    let second_nested = parse_json(&RpcRequest::new("simulation", "ping").wire()).unwrap();
    service
        .dispatch(
            &RpcRequest::new(8, "jobSubmitRemote").with_params(Json::object([
                ("request", second_nested),
                (
                    "requiredWorkerLabels",
                    Json::object([("pool", Json::string("simulation"))]),
                ),
            ])),
        )
        .result
        .unwrap();
    let wrong_pool = service
        .dispatch(
            &RpcRequest::new(9, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("signoff-node"))])),
        )
        .result
        .unwrap();
    assert_eq!(wrong_pool, Json::Null);
    let matching_pool = service
        .dispatch(
            &RpcRequest::new(10, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("sim-node"))])),
        )
        .result
        .unwrap();
    assert_eq!(
        matching_pool.as_object().unwrap()["requiredWorkerLabels"],
        Json::object([("pool", Json::string("simulation"))])
    );

    let unmatched_nested = parse_json(&RpcRequest::new("unmatched", "ping").wire()).unwrap();
    service
        .dispatch(
            &RpcRequest::new(11, "jobSubmitRemote").with_params(Json::object([
                ("request", unmatched_nested),
                (
                    "requiredWorkerLabels",
                    Json::object([
                        ("pool", Json::string("signoff")),
                        ("license", Json::string("missing")),
                    ]),
                ),
            ])),
        )
        .result
        .unwrap();
    let unmatched = service
        .dispatch(
            &RpcRequest::new(12, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("signoff-node"))])),
        )
        .result
        .unwrap();
    assert_eq!(unmatched, Json::Null);
    service.shutdown().unwrap();
}

#[test]
fn rpc_service_reassigns_expired_remote_worker_leases() {
    let service = RpcService::with_flow_cache_capacity_and_worker_count(4, 2).unwrap();
    for (rpc_id, worker) in [(1, "node-1"), (2, "node-2")] {
        let registered = service
            .dispatch(
                &RpcRequest::new(rpc_id, "workerRegister").with_params(Json::object([
                    ("workerId", Json::string(worker)),
                    ("capacity", Json::from(1_u64)),
                    ("leaseTimeoutSeconds", Json::from(60_u64)),
                ])),
            )
            .result
            .unwrap();
        assert_eq!(
            registered.as_object().unwrap()["leaseTimeoutSeconds"],
            Json::from(60_u64)
        );
    }

    let nested = parse_json(&RpcRequest::new("remote", "ping").wire()).unwrap();
    let submitted = service
        .dispatch(
            &RpcRequest::new(3, "jobSubmitRemote").with_params(Json::object([("request", nested)])),
        )
        .result
        .unwrap();
    let job_id = submitted.as_object().unwrap()["jobId"].clone();
    let first_claim = service
        .dispatch(
            &RpcRequest::new(4, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("node-1"))])),
        )
        .result
        .unwrap();
    assert_eq!(first_claim.as_object().unwrap()["jobId"], job_id);

    {
        let manager = service.manager.lock().unwrap();
        let mut claims = manager.remote_claims.lock().unwrap();
        claims.get_mut(&1).unwrap().last_heartbeat = Instant::now() - Duration::from_secs(61);
    }
    let expired = service
        .dispatch(&RpcRequest::new(5, "workerExpireLeases"))
        .result
        .unwrap();
    let expired_fields = expired.as_object().unwrap();
    assert_eq!(expired_fields["expiredCount"], Json::from(1_u64));
    assert_eq!(
        expired_fields["expiredJobIds"],
        Json::Array(vec![job_id.clone()])
    );

    let stats = service
        .dispatch(&RpcRequest::new(6, "jobWorkerStats"))
        .result
        .unwrap();
    let stats_fields = stats.as_object().unwrap();
    assert_eq!(stats_fields["queued"], Json::from(1_u64));
    assert_eq!(stats_fields["running"], Json::from(0_u64));
    assert_eq!(stats_fields["remoteActiveJobCount"], Json::from(0_u64));
    assert_eq!(stats_fields["remoteReassignments"], Json::from(1_u64));

    let second_claim = service
        .dispatch(
            &RpcRequest::new(7, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("node-2"))])),
        )
        .result
        .unwrap();
    assert_eq!(second_claim.as_object().unwrap()["jobId"], job_id);
    let stale_owner = service
        .dispatch(
            &RpcRequest::new(8, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("node-1")),
                ("jobId", job_id.clone()),
                ("result", Json::Null),
            ])),
        )
        .result;
    assert!(stale_owner.unwrap_err().contains("does not own job"));

    {
        let manager = service.manager.lock().unwrap();
        let mut claims = manager.remote_claims.lock().unwrap();
        claims.get_mut(&1).unwrap().last_heartbeat = Instant::now() - Duration::from_secs(61);
    }
    let heartbeat = service
        .dispatch(
            &RpcRequest::new(9, "workerHeartbeat")
                .with_params(Json::object([("workerId", Json::string("node-2"))])),
        )
        .result
        .unwrap();
    assert_eq!(
        heartbeat.as_object().unwrap()["activeJobIds"],
        Json::Array(vec![job_id.clone()])
    );
    let still_active = service
        .dispatch(&RpcRequest::new(10, "workerExpireLeases"))
        .result
        .unwrap();
    assert_eq!(
        still_active.as_object().unwrap()["expiredCount"],
        Json::from(0_u64)
    );

    let completed = service
        .dispatch(
            &RpcRequest::new(11, "workerComplete").with_params(Json::object([
                ("workerId", Json::string("node-2")),
                ("jobId", job_id),
                (
                    "result",
                    Json::object([
                        ("protocol", Json::Number(1.0)),
                        ("service", Json::string("eda")),
                    ]),
                ),
            ])),
        )
        .result
        .unwrap();
    assert_eq!(
        completed.as_object().unwrap()["state"],
        Json::string("completed")
    );

    let second_nested = parse_json(&RpcRequest::new("remote-2", "ping").wire()).unwrap();
    let second_submitted = service
        .dispatch(
            &RpcRequest::new(12, "jobSubmitRemote")
                .with_params(Json::object([("request", second_nested)])),
        )
        .result
        .unwrap();
    let second_fields = second_submitted.as_object().unwrap();
    let second_job_id = second_fields["jobId"].as_number().copied().unwrap() as u64;
    let second_claim = service
        .dispatch(
            &RpcRequest::new(13, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("node-1"))])),
        )
        .result
        .unwrap();
    assert_eq!(
        second_claim.as_object().unwrap()["jobId"],
        second_fields["jobId"]
    );
    {
        let manager = service.manager.lock().unwrap();
        let mut claims = manager.remote_claims.lock().unwrap();
        claims.get_mut(&second_job_id).unwrap().last_heartbeat =
            Instant::now() - Duration::from_secs(61);
    }
    let automatically_reassigned = service
        .dispatch(
            &RpcRequest::new(14, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("node-2"))])),
        )
        .result
        .unwrap();
    assert_eq!(
        automatically_reassigned.as_object().unwrap()["jobId"],
        second_fields["jobId"]
    );
    let final_stats = service
        .dispatch(&RpcRequest::new(15, "jobWorkerStats"))
        .result
        .unwrap();
    assert_eq!(
        final_stats.as_object().unwrap()["remoteReassignments"],
        Json::from(2_u64)
    );
    service.shutdown().unwrap();
}

#[test]
fn rpc_service_remote_retry_budget_fails_closed() {
    let service = RpcService::with_flow_cache_capacity_and_worker_count(4, 2).unwrap();
    service
        .dispatch(
            &RpcRequest::new(1, "workerRegister").with_params(Json::object([
                ("workerId", Json::string("node-1")),
                ("capacity", Json::from(1_u64)),
                ("leaseTimeoutSeconds", Json::from(60_u64)),
            ])),
        )
        .result
        .unwrap();

    let nested = parse_json(&RpcRequest::new("retry", "ping").wire()).unwrap();
    let submitted = service
        .dispatch(
            &RpcRequest::new(2, "jobSubmitRemote").with_params(Json::object([
                ("request", nested),
                ("maxAttempts", Json::from(1_u64)),
            ])),
        )
        .result
        .unwrap();
    let fields = submitted.as_object().unwrap();
    assert_eq!(fields["maxAttempts"], Json::from(1_u64));
    let job_id = fields["jobId"].clone();
    let claim = service
        .dispatch(
            &RpcRequest::new(3, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("node-1"))])),
        )
        .result
        .unwrap();
    let claim_fields = claim.as_object().unwrap();
    assert_eq!(claim_fields["jobId"], job_id);
    assert_eq!(claim_fields["attempt"], Json::from(1_u64));
    assert_eq!(claim_fields["maxAttempts"], Json::from(1_u64));

    {
        let manager = service.manager.lock().unwrap();
        let mut claims = manager.remote_claims.lock().unwrap();
        claims.get_mut(&1).unwrap().last_heartbeat = Instant::now() - Duration::from_secs(61);
    }
    let expired = service
        .dispatch(&RpcRequest::new(4, "workerExpireLeases"))
        .result
        .unwrap();
    assert_eq!(
        expired.as_object().unwrap()["expiredJobIds"],
        Json::Array(vec![job_id.clone()])
    );
    let status = service
        .dispatch(
            &RpcRequest::new(5, "jobStatus").with_params(Json::object([("jobId", job_id.clone())])),
        )
        .result
        .unwrap();
    let status_fields = status.as_object().unwrap();
    assert_eq!(status_fields["state"], Json::string("failed"));
    assert_eq!(
        status_fields["error"],
        Json::string("remote RPC job 1 exhausted 1 attempt(s)")
    );

    let stats = service
        .dispatch(&RpcRequest::new(6, "jobWorkerStats"))
        .result
        .unwrap();
    let stats_fields = stats.as_object().unwrap();
    assert_eq!(stats_fields["remoteReassignments"], Json::from(0_u64));
    assert_eq!(stats_fields["remoteRetryExhausted"], Json::from(1_u64));
    assert_eq!(stats_fields["activeJobIds"], Json::Array(Vec::new()));
    let no_retry = service
        .dispatch(
            &RpcRequest::new(7, "workerClaim")
                .with_params(Json::object([("workerId", Json::string("node-1"))])),
        )
        .result
        .unwrap();
    assert_eq!(no_retry, Json::Null);

    let invalid = service
        .dispatch(
            &RpcRequest::new(8, "jobSubmitRemote").with_params(Json::object([
                (
                    "request",
                    parse_json(&RpcRequest::new("bad", "ping").wire()).unwrap(),
                ),
                ("maxAttempts", Json::from(0_u64)),
            ])),
        )
        .result;
    assert!(invalid
        .unwrap_err()
        .contains("'maxAttempts' must be a positive usize"));
    service.shutdown().unwrap();
}
