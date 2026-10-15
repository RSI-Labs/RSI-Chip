use super::*;

#[test]
fn service_line_protocol_supports_job_submit_and_wait() {
    let nested = parse_json(&RpcRequest::new("work", "ping").wire()).unwrap();
    let input = format!(
        "{}\n{}\n",
        RpcRequest::new(1, "jobSubmit")
            .with_params(Json::object([("request", nested)]))
            .wire(),
        RpcRequest::new(2, "jobWait")
            .with_params(Json::object([
                ("jobId", Json::Number(1.0)),
                ("timeoutMs", Json::Number(2_000.0)),
            ]))
            .wire()
    );
    let service = RpcService::new();
    let mut output = Cursor::new(Vec::new());
    let count = serve_stream_with_service(Cursor::new(input), &mut output, &service).unwrap();
    assert_eq!(count, 2);
    let text = String::from_utf8(output.into_inner()).unwrap();
    assert!(
        text.contains("\"state\":\"queued\"")
            && text.contains("\"state\":\"completed\"")
            && text.contains("\"service\":\"eda\""),
        "{text}"
    );
    service.shutdown().unwrap();
}

#[test]
fn job_manager_completes_and_fails_typed_requests() {
    let manager = RpcJobManager::new();
    let completed = manager
        .run_to_completion(RpcRequest::new(1, "ping"))
        .unwrap();
    assert!(matches!(completed, RpcJobStatus::Completed(_)));
    let failed = manager
        .run_to_completion(RpcRequest::new(2, "unknown"))
        .unwrap();
    assert!(matches!(failed, RpcJobStatus::Failed(_)));
    manager.shutdown().unwrap();
}

#[test]
fn job_manager_runs_multiple_workers_concurrently() {
    if std::thread::available_parallelism().is_ok_and(|count| count.get() < 2) {
        return;
    }
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let dispatcher_barrier = Arc::clone(&barrier);
    let manager = RpcJobManager::new_with_dispatcher_and_worker_count(
        Arc::new(move |_request, _cancellation| {
            dispatcher_barrier.wait();
            Ok(Json::string("concurrent"))
        }),
        2,
    );
    assert_eq!(manager.worker_count(), 2);
    let first = manager.submit(RpcRequest::new(1, "ping")).unwrap();
    let second = manager.submit(RpcRequest::new(2, "ping")).unwrap();
    let mut completed = 0;
    for _ in 0..2_000 {
        completed = [first, second]
            .into_iter()
            .filter(|id| matches!(manager.poll(*id).unwrap(), Some(RpcJobStatus::Completed(_))))
            .count();
        if completed == 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(completed, 2);
    let stats = manager.job_stats().unwrap();
    assert_eq!(stats.worker_count, 2);
    assert_eq!(stats.completed, 2);
    assert!(stats.active_job_ids.is_empty());
    manager.shutdown().unwrap();
}

#[test]
fn job_manager_cancel_before_worker_receives_submission() {
    let manager = RpcJobManager::new();
    manager.jobs.lock().unwrap().insert(1, RpcJobStatus::Queued);
    assert!(manager.cancel(1).unwrap());
    manager
        .sender
        .send(WorkerCommand::Submit(1, RpcRequest::new(1, "ping"), None))
        .unwrap();
    for _ in 0..1000 {
        if manager.poll(1).unwrap() == Some(RpcJobStatus::Cancelled) {
            manager.shutdown().unwrap();
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("cancelled job was executed: {:?}", manager.poll(1).unwrap());
}

#[test]
fn job_manager_requests_cooperative_cancellation_while_running() {
    let manager = RpcJobManager::new();
    let cancellation = FlowCancellation::new();
    manager
        .running
        .lock()
        .unwrap()
        .insert(1, cancellation.clone());
    manager
        .jobs
        .lock()
        .unwrap()
        .insert(1, RpcJobStatus::Running);

    assert!(manager.cancel(1).unwrap());
    assert!(cancellation.is_cancelled());
    let source = "module top(input a, output y); assign y = ~a; endmodule";
    let request =
        RpcRequest::new(1, "rtlFlow").with_params(Json::object([("source", Json::string(source))]));
    manager
        .sender
        .send(WorkerCommand::Submit(1, request, Some(cancellation)))
        .unwrap();
    for _ in 0..1000 {
        if manager.poll(1).unwrap() == Some(RpcJobStatus::Cancelled) {
            manager.shutdown().unwrap();
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!(
        "cooperatively cancelled job did not finish: {:?}",
        manager.poll(1).unwrap()
    );
}

#[test]
fn tcp_daemon_serves_a_client() {
    let daemon = Daemon::bind("127.0.0.1:0").unwrap();
    let address = daemon.local_address();
    let mut stream = TcpStream::connect(address).unwrap();
    let nested = parse_json(&RpcRequest::new("work", "ping").wire()).unwrap();
    stream
        .write_all(
            RpcRequest::new(11, "jobSubmit")
                .with_params(Json::object([("request", nested)]))
                .wire()
                .as_bytes(),
        )
        .unwrap();
    stream.write_all(b"\n").unwrap();
    stream
        .write_all(
            RpcRequest::new(12, "jobWait")
                .with_params(Json::object([
                    ("jobId", Json::Number(1.0)),
                    ("timeoutMs", Json::Number(2_000.0)),
                ]))
                .wire()
                .as_bytes(),
        )
        .unwrap();
    stream.write_all(b"\n").unwrap();
    stream.flush().unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(
        response.contains("\"ok\":true")
            && response.contains("\"state\":\"queued\"")
            && response.contains("\"state\":\"completed\""),
        "{response}"
    );
    daemon.stop().unwrap();
}
