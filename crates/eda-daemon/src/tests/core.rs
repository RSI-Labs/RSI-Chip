use super::*;

#[test]
fn json_round_trips_nested_values_and_escapes() {
    let text = r#"{"a":[1,-2.5,true,null],"b":"line\nquote\" \\ü"}"#;
    let value = parse_json(text).unwrap();
    assert_eq!(value, parse_json(&value.write()).unwrap());
    assert_eq!(value.write(), text);
}

#[test]
fn json_rejects_malformed_and_duplicate_fields() {
    assert!(parse_json("{\"a\":1,\"a\":2}").is_err());
    assert!(parse_json("[1,]").is_err());
    assert!(parse_json("{\"a\":1} trailing").is_err());
    assert!(parse_json("\"\\ud800\"").is_err());
}

#[test]
fn rpc_ping_and_capabilities_round_trip() {
    let request = RpcRequest::new(7, "ping");
    let response = dispatch(&request);
    let value = parse_json(&response.wire()).unwrap();
    assert_eq!(value.as_object().unwrap()["ok"], Json::Bool(true));
    let report = dispatch(&RpcRequest::new("caps", "capabilities"));
    assert!(report.wire().contains("spice_dc_ac_transient"));
}

#[test]
fn rpc_flow_cache_reuses_and_reports_stats() {
    let service = RpcService::with_flow_cache_capacity(4).unwrap();
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let request = RpcRequest::new(41, "rtlFlowCache").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
    ]));
    let first = service.dispatch(&request).result.unwrap();
    let first_fields = first.as_object().unwrap();
    assert_eq!(first_fields["cacheHit"], Json::Bool(false));
    let first_verilog = first_fields["verilog"].as_str().unwrap().to_string();

    let second = service.dispatch(&request).result.unwrap();
    let second_fields = second.as_object().unwrap();
    assert_eq!(second_fields["cacheHit"], Json::Bool(true));
    assert_eq!(second_fields["verilog"].as_str().unwrap(), first_verilog);

    let stats = service
        .dispatch(&RpcRequest::new(42, "flowCacheStats"))
        .result
        .unwrap();
    assert_eq!(
        stats,
        Json::object([
            ("hits", Json::from(1_u64)),
            ("misses", Json::from(1_u64)),
            ("inserts", Json::from(1_u64)),
            ("evictions", Json::from(0_u64)),
        ])
    );

    service
        .dispatch(&RpcRequest::new(43, "flowCacheClear"))
        .result
        .unwrap();
    let recomputed = service.dispatch(&request).result.unwrap();
    assert_eq!(
        recomputed.as_object().unwrap()["cacheHit"],
        Json::Bool(false)
    );
    let nested = parse_json(&request.wire()).unwrap();
    let submitted = service
        .dispatch(
            &RpcRequest::new(45, "jobSubmit").with_params(Json::object([("request", nested)])),
        )
        .result
        .unwrap();
    let job_id = submitted.as_object().unwrap()["jobId"].clone();
    let waited = service
        .dispatch(&RpcRequest::new(46, "jobWait").with_params(Json::object([
            ("jobId", job_id),
            ("timeoutMs", Json::Number(2_000.0)),
        ])))
        .result
        .unwrap();
    let waited_fields = waited.as_object().unwrap();
    assert_eq!(waited_fields["state"], Json::string("completed"));
    assert_eq!(
        waited_fields["result"].as_object().unwrap()["cacheHit"],
        Json::Bool(true)
    );
    let cleared = service
        .dispatch(&RpcRequest::new(44, "flowCacheStats"))
        .result
        .unwrap();
    assert_eq!(
        cleared,
        Json::object([
            ("hits", Json::from(2_u64)),
            ("misses", Json::from(2_u64)),
            ("inserts", Json::from(2_u64)),
            ("evictions", Json::from(0_u64)),
        ])
    );
}

#[test]
fn rpc_service_persists_timing_waiver_registry_lifecycle() {
    let service = RpcService::new();
    let source = "module top(input a, output y); assign y = ~a; endmodule";
    let fast = r#"
scenario fast
create_clock -name clk -period 0.2 [get_ports a]
endscenario
"#;
    let faster = r#"
scenario fast
create_clock -name clk -period 0.1 [get_ports a]
endscenario
"#;
    let empty = service
        .dispatch(&RpcRequest::new(82, "timingWaiverRegistryList"))
        .result
        .unwrap();
    let empty_fields = empty.as_object().unwrap();
    assert_eq!(empty_fields["count"], Json::from(0_u64));
    assert_eq!(empty_fields["capacity"], Json::from(128_u64));

    let create = RpcRequest::new(83, "timingWaiverRegistryCreate").with_params(Json::object([
        ("registryId", Json::string("signoff")),
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        ("scenarios", Json::string(fast)),
    ]));
    let created = service.dispatch(&create).result.unwrap();
    let created_fields = created.as_object().unwrap();
    assert_eq!(created_fields["registryId"], Json::string("signoff"));
    assert_eq!(created_fields["scenarioCount"], Json::from(1_u64));
    assert_eq!(created_fields["activeViolationCount"], Json::from(1_u64));
    assert_eq!(
        created_fields["scenarioNames"],
        Json::Array(vec![Json::string("fast")])
    );
    assert_eq!(
        service.dispatch(&create).result.unwrap_err(),
        "timing waiver registry 'signoff' already exists"
    );

    let fetched = service
        .dispatch(
            &RpcRequest::new(84, "timingWaiverRegistryGet")
                .with_params(Json::object([("registryId", Json::string("signoff"))])),
        )
        .result
        .unwrap();
    let fetched_fields = fetched.as_object().unwrap();
    assert_eq!(fetched_fields["generation"], created_fields["generation"]);
    assert_eq!(fetched_fields["activeViolationCount"], Json::from(1_u64));

    let add = RpcRequest::new(85, "timingWaiverRegistryAddWaivers").with_params(Json::object([
        ("registryId", Json::string("signoff")),
        (
            "waivers",
            Json::string("waiver \"fast-only\" \"fast\" \"y\" \"-1.0\"\n"),
        ),
        ("actor", Json::string("timing-owner")),
    ]));
    let nested_add = parse_json(&add.wire()).unwrap();
    let submitted_add = service
        .dispatch(
            &RpcRequest::new(91, "jobSubmit").with_params(Json::object([("request", nested_add)])),
        )
        .result
        .unwrap();
    let add_job_id = submitted_add.as_object().unwrap()["jobId"].clone();
    let waited_add = service
        .dispatch(&RpcRequest::new(92, "jobWait").with_params(Json::object([
            ("jobId", add_job_id),
            ("timeoutMs", Json::Number(2_000.0)),
        ])))
        .result
        .unwrap();
    let waited_add_fields = waited_add.as_object().unwrap();
    assert_eq!(waited_add_fields["state"], Json::string("completed"));
    let waived = waited_add_fields["result"].clone();
    let waived_fields = waived.as_object().unwrap();
    let waived_scenarios = waived_fields["scenarios"].as_object().unwrap();
    let waived_fast = waived_scenarios["fast"].as_object().unwrap();
    assert_eq!(waived_fields["activeViolationCount"], Json::from(0_u64));
    assert_eq!(waived_fast["waivedViolationCount"], Json::from(1_u64));
    assert_eq!(
        waived_fields["activeWaiverIds"],
        Json::Array(vec![Json::string("fast-only")])
    );
    assert!(waived_fields["audit"]
        .as_str()
        .unwrap()
        .contains("\"timing-owner\""));

    let update =
        RpcRequest::new(86, "timingWaiverRegistryUpdateScenarios").with_params(Json::object([
            ("registryId", Json::string("signoff")),
            ("scenarios", Json::string(faster)),
        ]));
    let updated = service.dispatch(&update).result.unwrap();
    let updated_fields = updated.as_object().unwrap();
    let updated_scenarios = updated_fields["scenarios"].as_object().unwrap();
    let updated_fast = updated_scenarios["fast"].as_object().unwrap();
    let update_fields = updated_fields["update"].as_object().unwrap();
    let update_diffs = update_fields["scenarioDiffs"].as_object().unwrap();
    let update_fast = update_diffs["fast"].as_object().unwrap();
    assert_eq!(updated_fields["activeViolationCount"], Json::from(0_u64));
    assert_eq!(updated_fast["waivedViolationCount"], Json::from(1_u64));
    assert_eq!(update_fast["degradedCount"], Json::from(1_u64));
    assert!(updated_fields["audit"]
        .as_str()
        .unwrap()
        .contains("\"timing-owner\""));

    let remove =
        RpcRequest::new(87, "timingWaiverRegistryRemoveWaiver").with_params(Json::object([
            ("registryId", Json::string("signoff")),
            ("waiverId", Json::string("fast-only")),
            ("actor", Json::string("reviewer")),
        ]));
    let restored = service.dispatch(&remove).result.unwrap();
    let restored_fields = restored.as_object().unwrap();
    assert_eq!(restored_fields["activeViolationCount"], Json::from(1_u64));
    assert_eq!(restored_fields["activeWaiverIds"], Json::Array(Vec::new()));
    let audit = restored_fields["audit"].as_str().unwrap();
    assert!(audit.contains("\"timing-owner\"") && audit.contains("\"reviewer\""));

    let listed = service
        .dispatch(&RpcRequest::new(88, "timingWaiverRegistryList"))
        .result
        .unwrap();
    let listed_fields = listed.as_object().unwrap();
    assert_eq!(listed_fields["count"], Json::from(1_u64));
    let Json::Array(listed_registries) = &listed_fields["registries"] else {
        panic!("timing waiver registries should be an array");
    };
    let listed_registry = listed_registries[0].as_object().unwrap();
    assert_eq!(listed_registry["activeWaiverCount"], Json::from(0_u64));

    let delete = RpcRequest::new(89, "timingWaiverRegistryDelete")
        .with_params(Json::object([("registryId", Json::string("signoff"))]));
    let deleted = service.dispatch(&delete).result.unwrap();
    let deleted_fields = deleted.as_object().unwrap();
    assert_eq!(deleted_fields["deleted"], Json::Bool(true));
    assert_eq!(deleted_fields["remaining"], Json::from(0_u64));
    let missing = service.dispatch(
        &RpcRequest::new(90, "timingWaiverRegistryGet")
            .with_params(Json::object([("registryId", Json::string("signoff"))])),
    );
    assert_eq!(
        missing.result.unwrap_err(),
        "timing waiver registry 'signoff' does not exist"
    );
}

#[test]
fn rpc_waveform_measure_returns_clock_pulse_and_comparison() {
    let source = r#"
$timescale 1ns $end
$var wire 1 ! clk $end
$var wire 1 " sample $end
$enddefinitions $end
#0
0!
0"
#4
1!
1"
#9
0!
0"
#14
1!
0"
"#;
    let request = RpcRequest::new(8, "waveformMeasure").with_params(Json::object([
        ("vcd", Json::string(source)),
        ("signal", Json::string("clk")),
        ("compareWith", Json::string("sample")),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    let clock = fields["clock"].as_object().unwrap();
    assert_eq!(clock["riseCount"], Json::from(2_u64));
    assert_eq!(clock["fallCount"], Json::from(1_u64));
    assert_eq!(clock["periodCount"], Json::from(1_u64));
    assert_eq!(clock["averagePeriodNs"], Json::Number(10.0));
    let pulses = fields["pulses"].as_object().unwrap();
    assert_eq!(pulses["highCount"], Json::from(1_u64));
    assert_eq!(pulses["lowCount"], Json::from(1_u64));
    let comparison = fields["comparison"].as_object().unwrap();
    assert_eq!(comparison["sampleCount"], Json::from(4_u64));
    assert_eq!(comparison["matchingSamples"], Json::from(3_u64));
    assert_eq!(comparison["differingSamples"], Json::from(1_u64));
    assert_eq!(comparison["hammingDistance"], Json::from(1_u64));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}

#[test]
fn rpc_rtl_cts_returns_structured_trees() {
    let source = r#"
module top(input clk, rst, output reg [7:0] q);
always @(posedge clk or posedge rst) if (rst) q <= 8'd0; else q <= q + 8'd1;
endmodule
"#;
    let request = RpcRequest::new(47, "rtlCts").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
    ]));
    let result = dispatch(&request).result.unwrap();
    let fields = result.as_object().unwrap();
    assert_eq!(fields["timingDriven"], Json::Bool(false));
    assert_eq!(fields["buffersInserted"], Json::from(5_u64));
    let Json::Array(trees) = &fields["trees"] else {
        panic!("CTS result has no tree array");
    };
    assert_eq!(trees.len(), 1);
    let Json::Object(tree) = &trees[0] else {
        panic!("CTS tree is not an object");
    };
    assert_eq!(tree["sinkCount"], Json::from(8_u64));
    assert_eq!(tree["placementAware"], Json::Bool(true));
    assert!(fields["report"].as_str().unwrap().contains("CTS report"));

    let timing_request = RpcRequest::new(48, "rtlCts").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        ("timingDriven", Json::Bool(true)),
        ("targetSkewNs", Json::Number(0.2)),
    ]));
    let timing = dispatch(&timing_request).result.unwrap();
    let timing_fields = timing.as_object().unwrap();
    assert_eq!(timing_fields["timingDriven"], Json::Bool(true));
    assert_eq!(
        timing_fields["targetMet"],
        Json::Bool(timing_fields["bestEstimatedSkewNs"].as_number().unwrap() <= &0.2)
    );
    assert!(matches!(
        &timing_fields["searchedBranchFactors"],
        Json::Array(factors) if !factors.is_empty()
    ));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}

#[test]
fn rpc_rtl_eco_applies_structural_patch() {
    let source = "module top(input a,b,c, output y); wire n; assign n=a&b; assign y=n|c; endmodule";
    let request = RpcRequest::new(49, "rtlEco").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        (
            "eco",
            Json::string("eco functional_fix; replace_cell u1_and2 OR2;"),
        ),
    ]));
    let result = dispatch(&request).result.unwrap();
    let fields = result.as_object().unwrap();
    assert_eq!(fields["patchName"], Json::string("functional_fix"));
    assert_eq!(fields["operationsApplied"], Json::from(1_u64));
    let Json::Array(changes) = &fields["changes"] else {
        panic!("ECO changes array missing");
    };
    assert_eq!(changes.len(), 1);
    let Json::Object(change) = &changes[0] else {
        panic!("ECO change is not an object");
    };
    assert_eq!(change["operation"], Json::from(0_u64));
    assert!(change["description"].as_str().unwrap().contains("OR2"));
    assert!(fields["verilog"].as_str().unwrap().contains("module top"));
    assert!(fields["report"]
        .as_str()
        .unwrap()
        .contains("1 operation(s) applied"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}

#[test]
fn rpc_assertions_check_vcd_trace() {
    let vcd = r#"
$timescale 1ns $end
$var wire 1 ! clk $end
$var wire 1 " req $end
$var wire 1 # ack $end
$enddefinitions $end
#0
0!
0"
0#
#4
1!
1"
1#
"#;
    let request = RpcRequest::new(50, "assertions").with_params(Json::object([
        ("vcd", Json::string(vcd)),
        (
            "sva",
            Json::string("assert property (@(posedge clk) !(req && ack));"),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["count"], Json::from(1_u64));
    assert_eq!(fields["passed"], Json::from(0_u64));
    let Json::Array(results) = &fields["results"] else {
        panic!("assertion results array missing");
    };
    let Json::Object(result) = &results[0] else {
        panic!("assertion result is not an object");
    };
    assert_eq!(result["kind"], Json::string("assert"));
    assert_eq!(result["status"], Json::string("fail"));
    assert_eq!(result["passed"], Json::Bool(false));
    assert_eq!(result["clock"], Json::string("clk"));
    assert!(fields["report"].as_str().unwrap().contains("FAIL"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}
