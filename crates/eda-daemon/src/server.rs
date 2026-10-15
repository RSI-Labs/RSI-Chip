//! Line-delimited RPC stream serving and TCP daemon lifecycle.

use crate::{dispatch, RpcRequest, RpcService, MAX_RPC_LINE_BYTES};
use std::io::{BufRead, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{spawn, JoinHandle};

pub fn serve_stream(mut reader: impl BufRead, mut writer: impl Write) -> Result<usize, String> {
    let mut line = Vec::new();
    let mut requests = 0_usize;
    loop {
        line.clear();
        let bytes = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("failed to read RPC request: {error}"))?;
        if bytes == 0 {
            return Ok(requests);
        }
        if line.len() > MAX_RPC_LINE_BYTES {
            return Err(format!(
                "RPC request exceeds {} byte limit",
                MAX_RPC_LINE_BYTES
            ));
        }
        let text = String::from_utf8(
            line.strip_suffix(b"\n")
                .map(|value| value.strip_suffix(b"\r").unwrap_or(value))
                .unwrap_or(&line)
                .to_vec(),
        )
        .map_err(|_| "RPC request is not UTF-8".to_string())?;
        if text.trim().is_empty() {
            continue;
        }
        let request = RpcRequest::parse(&text)?;
        let response = dispatch(&request);
        writer
            .write_all(response.wire().as_bytes())
            .and_then(|_| writer.write_all(b"\n"))
            .map_err(|error| format!("failed to write RPC response: {error}"))?;
        writer
            .flush()
            .map_err(|error| format!("failed to flush RPC response: {error}"))?;
        requests += 1;
    }
}

pub fn serve_stream_with_service(
    mut reader: impl BufRead,
    mut writer: impl Write,
    service: &RpcService,
) -> Result<usize, String> {
    let mut line = Vec::new();
    let mut requests = 0_usize;
    loop {
        line.clear();
        let bytes = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("failed to read RPC request: {error}"))?;
        if bytes == 0 {
            return Ok(requests);
        }
        if line.len() > MAX_RPC_LINE_BYTES {
            return Err(format!(
                "RPC request exceeds {} byte limit",
                MAX_RPC_LINE_BYTES
            ));
        }
        let text = String::from_utf8(
            line.strip_suffix(b"\n")
                .map(|value| value.strip_suffix(b"\r").unwrap_or(value))
                .unwrap_or(&line)
                .to_vec(),
        )
        .map_err(|_| "RPC request is not UTF-8".to_string())?;
        if text.trim().is_empty() {
            continue;
        }
        let request = RpcRequest::parse(&text)?;
        let response = service.dispatch(&request);
        writer
            .write_all(response.wire().as_bytes())
            .and_then(|_| writer.write_all(b"\n"))
            .map_err(|error| format!("failed to write RPC response: {error}"))?;
        writer
            .flush()
            .map_err(|error| format!("failed to flush RPC response: {error}"))?;
        requests += 1;
    }
}

pub struct Daemon {
    address: String,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Daemon {
    pub fn bind(address: &str) -> Result<Self, String> {
        Self::bind_with_artifact_store(address, None)
    }

    pub fn bind_with_artifact_store(
        address: &str,
        artifact_store: Option<&Path>,
    ) -> Result<Self, String> {
        let listener = TcpListener::bind(address)
            .map_err(|error| format!("failed to bind EDA daemon to {address}: {error}"))?;
        let local_address = listener
            .local_addr()
            .map(|address| address.to_string())
            .map_err(|error| format!("failed to resolve EDA daemon address: {error}"))?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let service = RpcService::with_flow_cache_capacity_worker_count_and_artifact_store(
            128,
            4,
            artifact_store,
        )?;
        let worker_service = service.clone();
        let worker = spawn(move || {
            accept_loop(listener, worker_shutdown, worker_service.clone());
            let _ = worker_service.shutdown();
        });
        Ok(Self {
            address: local_address,
            shutdown,
            worker: Some(worker),
        })
    }

    pub fn local_address(&self) -> String {
        self.address.clone()
    }

    pub fn stop(mut self) -> Result<(), String> {
        self.shutdown.store(true, Ordering::SeqCst);
        drop(TcpStream::connect(&self.address));
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| "EDA daemon accept worker panicked".to_string())?;
        }
        Ok(())
    }

    pub fn wait(mut self) -> Result<(), String> {
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| "EDA daemon accept worker panicked".to_string())?;
        }
        Ok(())
    }
}

fn accept_loop(listener: TcpListener, shutdown: Arc<AtomicBool>, service: RpcService) {
    for stream in listener.incoming() {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else {
            continue;
        };
        let connection_service = service.clone();
        spawn(move || {
            if let Ok(clone) = stream.try_clone() {
                let reader = clone;
                let writer = stream;
                let _ = serve_stream_with_service(
                    std::io::BufReader::new(reader),
                    std::io::BufWriter::new(writer),
                    &connection_service,
                );
            }
        });
    }
}
