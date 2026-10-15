//! Stateful RPC service facade shared by TCP connections and local callers.

use crate::{
    dispatch_result_with_flow_cache, job_status_json, optional_nonnegative_usize,
    optional_positive_usize, parse_json, parse_remote_job_artifacts, parse_string_map,
    remote_job_artifact_json, remote_job_artifact_metadata_json, remote_worker_json,
    required_artifact_name, required_job_id, required_string, required_value, string_map_json,
    Json, RpcJobManager, RpcJobStatus, RpcRequest, RpcResponse, MAX_REMOTE_JOB_ARTIFACT_PAGE_SIZE,
};
use eda_api::{Flow, FlowCancellation, SharedFlowCache};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) const TIMING_WAIVER_REGISTRY_CAPACITY: usize = 128;
pub(crate) const LAYOUT_DRC_WAIVER_REGISTRY_CAPACITY: usize = 128;

#[derive(Clone)]
pub(crate) struct TimingViolationRegistryState {
    pub(crate) flow: Flow,
    pub(crate) definitions: Vec<eda_api::TimingScenarioDefinition>,
    pub(crate) registry: eda_api::TimingViolationRegistry,
}

pub(crate) type SharedTimingViolationRegistries =
    Arc<Mutex<BTreeMap<String, TimingViolationRegistryState>>>;

#[derive(Clone, Default)]
pub(crate) struct LayoutDrcWaiverRegistryState {
    pub(crate) waivers: BTreeMap<String, eda_api::LayoutDrcWaiver>,
    pub(crate) audit: Vec<LayoutDrcWaiverAuditEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LayoutDrcWaiverAuditEvent {
    pub(crate) sequence: usize,
    pub(crate) action: LayoutDrcWaiverAuditAction,
    pub(crate) waiver_id: String,
    pub(crate) actor: Option<String>,
    pub(crate) waiver: Option<eda_api::LayoutDrcWaiver>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LayoutDrcWaiverAuditAction {
    Added,
    Removed,
}

impl LayoutDrcWaiverAuditAction {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
        }
    }
}

pub(crate) type SharedLayoutDrcWaiverRegistries =
    Arc<Mutex<BTreeMap<String, LayoutDrcWaiverRegistryState>>>;

/// Stateful RPC front end shared by all TCP connections.
///
/// Direct requests remain synchronous. `jobSubmit`, `jobStatus`, `jobCancel`,
/// and `jobWait` expose the persistent worker queue to remote agents.
#[derive(Clone)]
pub struct RpcService {
    pub(crate) manager: Arc<Mutex<RpcJobManager>>,
    pub(crate) flow_cache: SharedFlowCache,
    pub(crate) timing_waiver_registries: SharedTimingViolationRegistries,
    pub(crate) layout_drc_waiver_registries: SharedLayoutDrcWaiverRegistries,
}

impl RpcService {
    pub fn new() -> Self {
        Self::with_flow_cache_capacity_and_worker_count(128, 4)
            .expect("default RPC service configuration")
    }

    pub fn with_flow_cache_capacity(capacity: usize) -> Result<Self, String> {
        Self::with_flow_cache_capacity_and_worker_count(capacity, 4)
    }

    pub fn with_flow_cache_capacity_and_worker_count(
        capacity: usize,
        worker_count: usize,
    ) -> Result<Self, String> {
        Self::with_flow_cache_capacity_worker_count_and_artifact_store(capacity, worker_count, None)
    }

    pub fn with_flow_cache_capacity_worker_count_and_artifact_store(
        capacity: usize,
        worker_count: usize,
        artifact_store: Option<&Path>,
    ) -> Result<Self, String> {
        if worker_count == 0 {
            return Err("RPC service requires at least one job worker".into());
        }
        let flow_cache = SharedFlowCache::new(capacity)?;
        let dispatcher_cache = flow_cache.clone();
        let timing_waiver_registries = Arc::new(Mutex::new(BTreeMap::new()));
        let dispatcher_registries = timing_waiver_registries.clone();
        let layout_drc_waiver_registries = Arc::new(Mutex::new(BTreeMap::new()));
        let dispatcher_layout_registries = layout_drc_waiver_registries.clone();
        let manager = RpcJobManager::try_new_with_dispatcher_worker_count_and_artifact_store(
            Arc::new(move |request, cancellation| {
                dispatch_result_with_flow_cache(
                    &request,
                    &cancellation,
                    &dispatcher_cache,
                    &dispatcher_registries,
                    &dispatcher_layout_registries,
                )
            }),
            worker_count,
            artifact_store,
        )?;
        let service = Self {
            manager: Arc::new(Mutex::new(manager)),
            flow_cache,
            timing_waiver_registries,
            layout_drc_waiver_registries,
        };
        Ok(service)
    }

    pub fn dispatch(&self, request: &RpcRequest) -> RpcResponse {
        RpcResponse {
            id: request.id.clone(),
            result: self.dispatch_result(request),
        }
    }

    fn dispatch_result(&self, request: &RpcRequest) -> Result<Json, String> {
        match request.method.as_str() {
            "jobSubmit" => self.job_submit(&request.params),
            "jobSubmitRemote" => self.job_submit_remote(&request.params),
            "jobStatus" => self.job_status(&request.params),
            "jobCancel" => self.job_cancel(&request.params),
            "jobWait" => self.job_wait(&request.params),
            "jobWorkerStats" => self.job_worker_stats(),
            "jobArtifactList" => self.job_artifact_list(&request.params),
            "jobArtifactGet" => self.job_artifact_get(&request.params),
            "jobArtifactDelete" => self.job_artifact_delete(&request.params),
            "workerRegister" => self.worker_register(&request.params),
            "workerHeartbeat" => self.worker_heartbeat(&request.params),
            "workerList" => self.worker_list(),
            "workerExpireLeases" => self.worker_expire_leases(),
            "workerClaim" => self.worker_claim(&request.params),
            "workerClaimStatus" => self.worker_claim_status(&request.params),
            "workerComplete" => self.worker_complete(&request.params),
            "rtlFlowCache" => dispatch_result_with_flow_cache(
                request,
                &FlowCancellation::new(),
                &self.flow_cache,
                &self.timing_waiver_registries,
                &self.layout_drc_waiver_registries,
            ),
            "flowCacheClear" => {
                self.flow_cache.clear()?;
                Ok(Json::object([("cleared", Json::Bool(true))]))
            }
            _ => dispatch_result_with_flow_cache(
                request,
                &FlowCancellation::new(),
                &self.flow_cache,
                &self.timing_waiver_registries,
                &self.layout_drc_waiver_registries,
            ),
        }
    }

    fn manager(&self) -> Result<std::sync::MutexGuard<'_, RpcJobManager>, String> {
        self.manager
            .lock()
            .map_err(|_| "RPC job manager mutex poisoned".to_string())
    }

    fn job_submit(&self, params: &Json) -> Result<Json, String> {
        let nested = required_value(params, "request")?;
        let request = RpcRequest::parse(&nested.write())?;
        let id = self.manager()?.submit(request)?;
        Ok(Json::object([
            ("jobId", Json::number(id as f64)?),
            ("state", Json::string("queued")),
        ]))
    }

    fn job_submit_remote(&self, params: &Json) -> Result<Json, String> {
        let nested = required_value(params, "request")?;
        let request = RpcRequest::parse(&nested.write())?;
        let fields = params
            .as_object()
            .ok_or_else(|| "params must be an object".to_string())?;
        let max_attempts = match fields.get("maxAttempts") {
            None | Some(Json::Null) => 3_usize,
            Some(Json::Number(value)) => {
                if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
                    return Err("'maxAttempts' must be a positive usize".into());
                }
                *value as usize
            }
            Some(_) => return Err("'maxAttempts' must be a positive usize".into()),
        };
        let priority = match fields.get("priority") {
            None | Some(Json::Null) => 0_u32,
            Some(Json::Number(value)) => {
                if *value < 0.0 || value.fract() != 0.0 || *value > u32::MAX as f64 {
                    return Err("'priority' must be a non-negative u32".into());
                }
                *value as u32
            }
            Some(_) => return Err("'priority' must be a non-negative u32".into()),
        };
        let required_worker_labels = parse_string_map(params, "requiredWorkerLabels")?;
        let id = self.manager()?.submit_remote(
            request,
            max_attempts,
            priority,
            required_worker_labels,
        )?;
        Ok(Json::object([
            ("jobId", Json::number(id as f64)?),
            ("state", Json::string("queued")),
            ("scheduled", Json::string("remote")),
            ("maxAttempts", Json::from(max_attempts as u64)),
            ("priority", Json::from(u64::from(priority))),
        ]))
    }

    fn job_status(&self, params: &Json) -> Result<Json, String> {
        let id = required_job_id(params)?;
        let status = self
            .manager()?
            .poll(id)?
            .ok_or_else(|| format!("unknown RPC job {id}"))?;
        job_status_json(id, &status)
    }

    fn job_cancel(&self, params: &Json) -> Result<Json, String> {
        let id = required_job_id(params)?;
        let cancelled = self.manager()?.cancel(id)?;
        Ok(Json::object([
            ("jobId", Json::number(id as f64)?),
            ("cancelled", Json::Bool(cancelled)),
        ]))
    }

    fn job_wait(&self, params: &Json) -> Result<Json, String> {
        let id = required_job_id(params)?;
        let fields = params
            .as_object()
            .ok_or_else(|| "params must be an object".to_string())?;
        let timeout_ms = match fields.get("timeoutMs") {
            None => 1_000_u64,
            Some(Json::Number(value)) => {
                if *value < 0.0 || value.fract() != 0.0 || *value > u64::MAX as f64 {
                    return Err("'timeoutMs' must be a non-negative integer".into());
                }
                *value as u64
            }
            Some(Json::Null) => 1_000,
            Some(_) => return Err("'timeoutMs' must be a non-negative integer".into()),
        };
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let status = self
                .manager()?
                .poll(id)?
                .ok_or_else(|| format!("unknown RPC job {id}"))?;
            if !matches!(status, RpcJobStatus::Queued | RpcJobStatus::Running) {
                return job_status_json(id, &status);
            }
            if std::time::Instant::now() >= deadline {
                return job_status_json(id, &status);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn job_worker_stats(&self) -> Result<Json, String> {
        let stats = self.manager()?.job_stats()?;
        Ok(Json::object([
            ("workerCount", Json::from(stats.worker_count as u64)),
            (
                "remoteWorkerCount",
                Json::from(stats.remote_worker_count as u64),
            ),
            (
                "remoteActiveJobCount",
                Json::from(stats.remote_active_job_count as u64),
            ),
            (
                "remoteArtifactCount",
                Json::from(stats.remote_artifact_count as u64),
            ),
            (
                "remoteArtifactBytes",
                Json::from(stats.remote_artifact_bytes as u64),
            ),
            (
                "remoteArtifactCapacity",
                Json::from(stats.remote_artifact_capacity as u64),
            ),
            (
                "remoteArtifactByteCapacity",
                Json::from(stats.remote_artifact_byte_capacity as u64),
            ),
            (
                "remoteArtifactEvictions",
                Json::from(stats.remote_artifact_evictions),
            ),
            (
                "remoteReassignments",
                Json::from(stats.remote_reassignments),
            ),
            (
                "remoteRetryExhausted",
                Json::from(stats.remote_retry_exhausted),
            ),
            (
                "remoteWorkerExpirations",
                Json::from(stats.remote_worker_expirations),
            ),
            ("queued", Json::from(stats.queued as u64)),
            ("running", Json::from(stats.running as u64)),
            ("completed", Json::from(stats.completed as u64)),
            ("failed", Json::from(stats.failed as u64)),
            ("cancelled", Json::from(stats.cancelled as u64)),
            (
                "activeJobIds",
                Json::Array(stats.active_job_ids.into_iter().map(Json::from).collect()),
            ),
        ]))
    }

    fn job_artifact_list(&self, params: &Json) -> Result<Json, String> {
        let job_id = required_job_id(params)?;
        let offset = optional_nonnegative_usize(params, "offset", 0)?;
        let limit = optional_positive_usize(params, "limit", MAX_REMOTE_JOB_ARTIFACT_PAGE_SIZE)?;
        if limit > MAX_REMOTE_JOB_ARTIFACT_PAGE_SIZE {
            return Err(format!(
                "'limit' cannot exceed {MAX_REMOTE_JOB_ARTIFACT_PAGE_SIZE}"
            ));
        }
        let artifacts = self.manager()?.remote_job_artifacts(job_id)?;
        let total = artifacts.len();
        let values = artifacts
            .iter()
            .skip(offset)
            .take(limit)
            .map(remote_job_artifact_metadata_json)
            .collect::<Vec<_>>();
        Ok(Json::object([
            ("jobId", Json::number(job_id as f64)?),
            ("count", Json::from(values.len() as u64)),
            ("total", Json::from(total as u64)),
            ("offset", Json::from(offset as u64)),
            ("limit", Json::from(limit as u64)),
            ("artifacts", Json::Array(values)),
        ]))
    }

    fn job_artifact_get(&self, params: &Json) -> Result<Json, String> {
        let job_id = required_job_id(params)?;
        let name = required_artifact_name(params)?;
        let artifact = self.manager()?.remote_job_artifact(job_id, &name)?;
        remote_job_artifact_json(&artifact)
    }

    fn job_artifact_delete(&self, params: &Json) -> Result<Json, String> {
        let job_id = required_job_id(params)?;
        let name = required_artifact_name(params)?;
        let artifact = self.manager()?.delete_remote_job_artifact(job_id, &name)?;
        Ok(Json::object([
            ("jobId", Json::number(job_id as f64)?),
            ("name", Json::string(artifact.name)),
            ("deleted", Json::Bool(true)),
            ("sha256", Json::string(artifact.sha256)),
            ("sizeBytes", Json::from(artifact.size_bytes as u64)),
        ]))
    }

    fn worker_register(&self, params: &Json) -> Result<Json, String> {
        let worker_id = required_string(params, "workerId")?;
        let fields = params
            .as_object()
            .ok_or_else(|| "params must be an object".to_string())?;
        let capacity = match fields.get("capacity") {
            None | Some(Json::Null) => 1_usize,
            Some(Json::Number(value)) => {
                if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
                    return Err("'capacity' must be a positive usize".into());
                }
                *value as usize
            }
            Some(_) => return Err("'capacity' must be a positive usize".into()),
        };
        let lease_timeout_seconds = match fields.get("leaseTimeoutSeconds") {
            None | Some(Json::Null) => 60_u64,
            Some(Json::Number(value)) => {
                if *value <= 0.0 || value.fract() != 0.0 || *value > u64::MAX as f64 {
                    return Err("'leaseTimeoutSeconds' must be a positive u64".into());
                }
                *value as u64
            }
            Some(_) => return Err("'leaseTimeoutSeconds' must be a positive u64".into()),
        };
        let snapshot = self.manager()?.register_remote_worker(
            &worker_id,
            capacity,
            Duration::from_secs(lease_timeout_seconds),
            parse_string_map(params, "labels")?,
        )?;
        remote_worker_json(&snapshot)
    }

    fn worker_heartbeat(&self, params: &Json) -> Result<Json, String> {
        let worker_id = required_string(params, "workerId")?;
        let snapshot = self.manager()?.remote_worker_heartbeat(&worker_id)?;
        remote_worker_json(&snapshot)
    }

    fn worker_list(&self) -> Result<Json, String> {
        let workers = self.manager()?.remote_workers()?;
        let values = workers
            .iter()
            .map(remote_worker_json)
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Json::object([
            ("count", Json::from(values.len() as u64)),
            ("workers", Json::Array(values)),
        ]))
    }

    fn worker_expire_leases(&self) -> Result<Json, String> {
        let expiration = self.manager()?.expire_remote_leases()?;
        Ok(Json::object([
            (
                "expiredCount",
                Json::from(expiration.expired_job_ids.len() as u64),
            ),
            (
                "expiredJobIds",
                Json::Array(
                    expiration
                        .expired_job_ids
                        .into_iter()
                        .map(Json::from)
                        .collect(),
                ),
            ),
            (
                "removedWorkerCount",
                Json::from(expiration.removed_worker_ids.len() as u64),
            ),
            (
                "removedWorkerIds",
                Json::Array(
                    expiration
                        .removed_worker_ids
                        .into_iter()
                        .map(Json::string)
                        .collect(),
                ),
            ),
        ]))
    }

    fn worker_claim(&self, params: &Json) -> Result<Json, String> {
        let worker_id = required_string(params, "workerId")?;
        let Some(claim) = self.manager()?.claim_remote_job(&worker_id)? else {
            return Ok(Json::Null);
        };
        Ok(Json::object([
            ("jobId", Json::number(claim.job_id as f64)?),
            ("workerId", Json::string(claim.worker_id)),
            ("request", parse_json(&claim.request.wire())?),
            ("attempt", Json::from(claim.attempt as u64)),
            ("maxAttempts", Json::from(claim.max_attempts as u64)),
            ("priority", Json::from(u64::from(claim.priority))),
            (
                "requiredWorkerLabels",
                string_map_json(&claim.required_worker_labels),
            ),
        ]))
    }

    fn worker_claim_status(&self, params: &Json) -> Result<Json, String> {
        let job_id = required_job_id(params)?;
        let cancelled = self.manager()?.remote_claim_status(job_id)?;
        Ok(Json::object([("cancelled", Json::Bool(cancelled))]))
    }

    fn worker_complete(&self, params: &Json) -> Result<Json, String> {
        let worker_id = required_string(params, "workerId")?;
        let job_id = required_job_id(params)?;
        let fields = params
            .as_object()
            .ok_or_else(|| "params must be an object".to_string())?;
        let result = if let Some(error) = fields.get("error") {
            let Json::String(error) = error else {
                return Err("'error' must be a string".into());
            };
            Err(error.clone())
        } else {
            Ok(fields.get("result").cloned().unwrap_or(Json::Null))
        };
        let artifacts = parse_remote_job_artifacts(params, job_id)?;
        let status = self
            .manager()?
            .complete_remote_job(&worker_id, job_id, result, artifacts)?;
        job_status_json(job_id, &status)
    }

    pub(crate) fn shutdown(&self) -> Result<(), String> {
        self.manager()?.shutdown_background()
    }
}

impl Default for RpcService {
    fn default() -> Self {
        Self::new()
    }
}
