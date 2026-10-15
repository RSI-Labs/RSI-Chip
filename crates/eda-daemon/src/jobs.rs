//! Typed local and remote asynchronous RPC job management.

use crate::parse_json;
use crate::{
    dispatch_result_cancellable, parse_remote_job_artifacts, Json, RpcRequest,
    MAX_REMOTE_JOB_ARTIFACT_REGISTRY_BYTES, MAX_REMOTE_JOB_ARTIFACT_REGISTRY_COUNT,
};
use eda_api::FlowCancellation;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{spawn, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum RpcJobStatus {
    Queued,
    Running,
    Completed(Json),
    Failed(String),
    Cancelled,
}

pub(crate) enum WorkerCommand {
    Submit(u64, RpcRequest, Option<FlowCancellation>),
    Shutdown,
}

#[derive(Clone)]
pub(crate) struct WorkerChannel {
    commands: Arc<Mutex<VecDeque<WorkerCommand>>>,
    condition: Arc<Condvar>,
}

impl WorkerChannel {
    fn new() -> Self {
        Self {
            commands: Arc::new(Mutex::new(VecDeque::new())),
            condition: Arc::new(Condvar::new()),
        }
    }

    pub(crate) fn send(&self, command: WorkerCommand) -> Result<(), String> {
        let mut commands = self
            .commands
            .lock()
            .map_err(|_| "RPC worker queue mutex poisoned".to_string())?;
        commands.push_back(command);
        self.condition.notify_one();
        Ok(())
    }

    fn receive(&self) -> Option<WorkerCommand> {
        let Ok(mut commands) = self.commands.lock() else {
            return None;
        };
        loop {
            if let Some(command) = commands.pop_front() {
                return Some(command);
            }
            let condition = Arc::clone(&self.condition);
            commands = match condition.wait(commands) {
                Ok(state) => state,
                Err(_) => return None,
            };
        }
    }
}

type SharedJobs = Arc<Mutex<BTreeMap<u64, RpcJobStatus>>>;
type SharedRunning = Arc<Mutex<BTreeMap<u64, FlowCancellation>>>;
type SharedPendingJobs = Arc<Mutex<BTreeMap<u64, RemoteJobRecord>>>;
type SharedRemoteClaims = Arc<Mutex<BTreeMap<u64, RemoteJobLease>>>;
type SharedRemoteWorkers = Arc<Mutex<BTreeMap<String, RemoteWorker>>>;
type SharedRemoteJobArtifacts = Arc<Mutex<BTreeMap<u64, BTreeMap<String, RemoteJobArtifact>>>>;

#[derive(Clone, Debug)]
pub(crate) struct RemoteWorker {
    worker_id: String,
    capacity: usize,
    lease_timeout: Duration,
    labels: BTreeMap<String, String>,
    active_jobs: BTreeSet<u64>,
    pub(crate) last_heartbeat: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteWorkerSnapshot {
    pub worker_id: String,
    pub capacity: usize,
    pub lease_timeout_seconds: u64,
    pub labels: BTreeMap<String, String>,
    pub active_job_ids: Vec<u64>,
    pub last_heartbeat_age_seconds: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct RemoteJobLease {
    worker_id: String,
    record: RemoteJobRecord,
    pub(crate) last_heartbeat: Instant,
}

#[derive(Clone, Debug)]
struct RemoteJobRecord {
    request: RpcRequest,
    attempts: usize,
    max_attempts: usize,
    priority: u32,
    required_worker_labels: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct RemoteJobClaim {
    pub job_id: u64,
    pub worker_id: String,
    pub request: RpcRequest,
    pub attempt: usize,
    pub max_attempts: usize,
    pub priority: u32,
    pub required_worker_labels: BTreeMap<String, String>,
}

/// Result of expiring remote leases and pruning idle fleet entries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteLeaseExpirationResult {
    pub expired_job_ids: Vec<u64>,
    pub removed_worker_ids: Vec<String>,
}

/// A result artifact returned by a remote worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteJobArtifact {
    pub job_id: u64,
    pub name: String,
    pub content_type: String,
    pub content_encoding: String,
    pub content: String,
    pub sha256: String,
    pub size_bytes: usize,
    pub last_access: Instant,
}

/// Artifact metadata safe to expose without transferring full contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteJobArtifactMetadata {
    pub job_id: u64,
    pub name: String,
    pub content_type: String,
    pub content_encoding: String,
    pub sha256: String,
    pub size_bytes: usize,
}

impl From<&RemoteJobArtifact> for RemoteJobArtifactMetadata {
    fn from(artifact: &RemoteJobArtifact) -> Self {
        Self {
            job_id: artifact.job_id,
            name: artifact.name.clone(),
            content_type: artifact.content_type.clone(),
            content_encoding: artifact.content_encoding.clone(),
            sha256: artifact.sha256.clone(),
            size_bytes: artifact.size_bytes,
        }
    }
}

/// Typed, persistent job queue backed by one or more local worker threads.
pub struct RpcJobManager {
    next_id: AtomicU64,
    pub(crate) jobs: SharedJobs,
    pub(crate) running: SharedRunning,
    pub(crate) sender: WorkerChannel,
    pending_remote_jobs: SharedPendingJobs,
    pub(crate) remote_claims: SharedRemoteClaims,
    pub(crate) remote_workers: SharedRemoteWorkers,
    remote_job_artifacts: SharedRemoteJobArtifacts,
    artifact_store: Option<PathBuf>,
    pub(crate) remote_artifact_capacity: usize,
    pub(crate) remote_artifact_byte_capacity: usize,
    remote_reassignments: AtomicU64,
    remote_retry_exhausted: AtomicU64,
    remote_worker_expirations: AtomicU64,
    remote_artifact_evictions: AtomicU64,
    workers: Option<Vec<JoinHandle<()>>>,
    worker_count: usize,
}

impl RpcJobManager {
    pub fn new() -> Self {
        Self::new_with_dispatcher(Arc::new(|request, cancellation| {
            dispatch_result_cancellable(&request, &cancellation)
        }))
    }

    fn new_with_dispatcher(dispatcher: RpcDispatcher) -> Self {
        Self::new_with_dispatcher_and_worker_count(dispatcher, 1)
    }

    pub(crate) fn new_with_dispatcher_and_worker_count(
        dispatcher: RpcDispatcher,
        worker_count: usize,
    ) -> Self {
        Self::try_new_with_dispatcher_worker_count_and_artifact_store(
            dispatcher,
            worker_count,
            None,
        )
        .expect("RPC job manager configuration")
    }

    pub(crate) fn try_new_with_dispatcher_worker_count_and_artifact_store(
        dispatcher: RpcDispatcher,
        worker_count: usize,
        artifact_store: Option<&Path>,
    ) -> Result<Self, String> {
        if worker_count == 0 {
            panic!("RPC job manager requires at least one worker");
        }
        if let Some(root) = artifact_store {
            fs::create_dir_all(root).map_err(|error| {
                format!(
                    "failed to create RPC artifact store {}: {error}",
                    root.display()
                )
            })?;
        }
        let receiver = WorkerChannel::new();
        let sender = receiver.clone();
        let jobs = Arc::new(Mutex::new(BTreeMap::new()));
        let pending_remote_jobs = Arc::new(Mutex::new(BTreeMap::new()));
        let worker_jobs = Arc::clone(&jobs);
        let running = Arc::new(Mutex::new(BTreeMap::new()));
        let remote_claims = Arc::new(Mutex::new(BTreeMap::new()));
        let remote_workers = Arc::new(Mutex::new(BTreeMap::new()));
        let remote_job_artifacts = Arc::new(Mutex::new(BTreeMap::new()));
        let worker_running = Arc::clone(&running);
        let worker_dispatcher = Arc::clone(&dispatcher);
        let workers = (0..worker_count
            .min(std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)))
            .map(|_| {
                let receiver = receiver.clone();
                let jobs = Arc::clone(&worker_jobs);
                let running = Arc::clone(&worker_running);
                let dispatcher = Arc::clone(&worker_dispatcher);
                spawn(move || worker_loop(receiver, jobs, running, dispatcher))
            })
            .collect::<Vec<_>>();
        let manager = Self {
            next_id: AtomicU64::new(1),
            jobs,
            running,
            sender,
            pending_remote_jobs,
            remote_claims,
            remote_workers,
            remote_job_artifacts,
            artifact_store: artifact_store.map(Path::to_path_buf),
            remote_artifact_capacity: MAX_REMOTE_JOB_ARTIFACT_REGISTRY_COUNT,
            remote_artifact_byte_capacity: MAX_REMOTE_JOB_ARTIFACT_REGISTRY_BYTES,
            remote_reassignments: AtomicU64::new(0),
            remote_retry_exhausted: AtomicU64::new(0),
            remote_worker_expirations: AtomicU64::new(0),
            remote_artifact_evictions: AtomicU64::new(0),
            workers: Some(workers),
            worker_count,
        };
        if let Some(root) = artifact_store {
            manager.load_remote_job_records(root)?;
        }
        Ok(manager)
    }

    pub fn submit(&self, request: RpcRequest) -> Result<u64, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.jobs
            .lock()
            .map_err(|_| "job state mutex poisoned".to_string())?
            .insert(id, RpcJobStatus::Queued);
        self.sender
            .send(WorkerCommand::Submit(id, request, None))
            .map_err(|_| format!("failed to queue RPC job {id}"))?;
        Ok(id)
    }

    /// Queue a job for an RPC-visible remote worker instead of local execution.
    pub fn submit_remote(
        &self,
        request: RpcRequest,
        max_attempts: usize,
        priority: u32,
        required_worker_labels: BTreeMap<String, String>,
    ) -> Result<u64, String> {
        if max_attempts == 0 {
            return Err("remote RPC job requires a positive attempt limit".into());
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        if let Some(root) = &self.artifact_store {
            write_remote_job_next_id(root, id.saturating_add(1))?;
        }
        self.jobs
            .lock()
            .map_err(|_| "job state mutex poisoned".to_string())?
            .insert(id, RpcJobStatus::Queued);
        self.pending_remote_jobs
            .lock()
            .map_err(|_| "remote job queue mutex poisoned".to_string())?
            .insert(
                id,
                RemoteJobRecord {
                    request,
                    attempts: 0,
                    max_attempts,
                    priority,
                    required_worker_labels,
                },
            );
        Ok(id)
    }

    pub fn poll(&self, id: u64) -> Result<Option<RpcJobStatus>, String> {
        Ok(self
            .jobs
            .lock()
            .map_err(|_| "job state mutex poisoned".to_string())?
            .get(&id)
            .cloned())
    }

    pub fn worker_count(&self) -> usize {
        self.worker_count
    }

    fn load_remote_job_records(&self, root: &Path) -> Result<(), String> {
        let mut maximum_job_id = 0_u64;
        let entries = fs::read_dir(root).map_err(|error| {
            format!(
                "failed to read RPC artifact store {}: {error}",
                root.display()
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "failed to scan RPC artifact store {}: {error}",
                    root.display()
                )
            })?;
            if !entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
            {
                continue;
            }
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            if !file_name.ends_with(".json") {
                continue;
            }
            let text = fs::read_to_string(entry.path()).map_err(|error| {
                format!(
                    "failed to read RPC job record {}: {error}",
                    entry.path().display()
                )
            })?;
            let record = parse_json(&text)?;
            let fields = record.as_object().ok_or_else(|| {
                format!(
                    "RPC job record {} must be an object",
                    entry.path().display()
                )
            })?;
            if fields.get("schemaVersion") != Some(&Json::from(1_u64)) {
                return Err(format!(
                    "unsupported RPC job record schema in {}",
                    entry.path().display()
                ));
            }
            let Some(job_id) = fields
                .get("jobId")
                .and_then(Json::as_number)
                .filter(|value| **value >= 0.0 && value.fract() == 0.0)
                .map(|value| *value as u64)
            else {
                return Err(format!(
                    "RPC job record {} has an invalid jobId",
                    entry.path().display()
                ));
            };
            if file_name != format!("{job_id}.json") {
                return Err(format!(
                    "RPC job record filename {file_name} does not match job {job_id}"
                ));
            }
            let status = remote_job_status_from_record(&record)?;
            let artifacts = parse_remote_job_artifacts(&record, job_id)?;
            let artifact_map = artifacts
                .into_iter()
                .map(|artifact| (artifact.name.clone(), artifact))
                .collect::<BTreeMap<_, _>>();
            {
                let mut remote_artifacts = self
                    .remote_job_artifacts
                    .lock()
                    .map_err(|_| "remote job artifact mutex poisoned".to_string())?;
                let mut jobs = self
                    .jobs
                    .lock()
                    .map_err(|_| "job state mutex poisoned".to_string())?;
                if jobs.contains_key(&job_id) {
                    return Err(format!("duplicate persisted RPC job record {job_id}"));
                }
                jobs.insert(job_id, status);
                remote_artifacts.insert(job_id, artifact_map);
            }
            maximum_job_id = maximum_job_id.max(job_id);
        }
        let watermark_path = remote_job_next_id_path(root);
        if watermark_path.exists() {
            let watermark = fs::read_to_string(&watermark_path)
                .map_err(|error| {
                    format!(
                        "failed to read RPC job ID watermark {}: {error}",
                        watermark_path.display()
                    )
                })?
                .trim()
                .parse::<u64>()
                .map_err(|_| {
                    format!(
                        "invalid RPC job ID watermark in {}",
                        watermark_path.display()
                    )
                })?;
            self.next_id.fetch_max(watermark, Ordering::SeqCst);
        }
        self.next_id
            .fetch_max(maximum_job_id.saturating_add(1), Ordering::SeqCst);
        let mut remote_artifacts = self
            .remote_job_artifacts
            .lock()
            .map_err(|_| "remote job artifact mutex poisoned".to_string())?;
        enforce_remote_artifact_limits_locked(
            &mut remote_artifacts,
            0,
            self.remote_artifact_capacity,
            self.remote_artifact_byte_capacity,
            &self.remote_artifact_evictions,
        )?;
        Ok(())
    }

    pub fn job_stats(&self) -> Result<RpcJobStats, String> {
        let (remote_artifact_count, remote_artifact_bytes) = {
            let artifacts = self
                .remote_job_artifacts
                .lock()
                .map_err(|_| "remote job artifact mutex poisoned".to_string())?;
            remote_artifact_registry_totals(&artifacts)
        };
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| "job state mutex poisoned".to_string())?;
        let running = self
            .running
            .lock()
            .map_err(|_| "running job state mutex poisoned".to_string())?;
        let mut stats = RpcJobStats {
            worker_count: self.worker_count,
            ..RpcJobStats::default()
        };
        stats.remote_artifact_count = remote_artifact_count;
        stats.remote_artifact_bytes = remote_artifact_bytes;
        stats.remote_artifact_capacity = self.remote_artifact_capacity;
        stats.remote_artifact_byte_capacity = self.remote_artifact_byte_capacity;
        stats.remote_artifact_evictions = self.remote_artifact_evictions.load(Ordering::SeqCst);
        for (id, status) in jobs.iter() {
            match status {
                RpcJobStatus::Queued => stats.queued += 1,
                RpcJobStatus::Running => stats.running += 1,
                RpcJobStatus::Completed(_) => stats.completed += 1,
                RpcJobStatus::Failed(_) => stats.failed += 1,
                RpcJobStatus::Cancelled => stats.cancelled += 1,
            }
            if matches!(status, RpcJobStatus::Queued | RpcJobStatus::Running) {
                stats.active_job_ids.push(*id);
            }
        }
        for id in running.keys() {
            if !stats.active_job_ids.contains(id) {
                stats.active_job_ids.push(*id);
            }
        }
        let remote_workers = self
            .remote_workers
            .lock()
            .map_err(|_| "remote worker mutex poisoned".to_string())?;
        stats.remote_worker_count = remote_workers.len();
        stats.remote_active_job_count = remote_workers
            .values()
            .map(|worker| worker.active_jobs.len())
            .sum();
        stats.remote_reassignments = self.remote_reassignments.load(Ordering::SeqCst);
        stats.remote_retry_exhausted = self.remote_retry_exhausted.load(Ordering::SeqCst);
        stats.remote_worker_expirations = self.remote_worker_expirations.load(Ordering::SeqCst);
        stats.active_job_ids.sort_unstable();
        stats.active_job_ids.dedup();
        Ok(stats)
    }

    pub fn register_remote_worker(
        &self,
        worker_id: &str,
        capacity: usize,
        lease_timeout: Duration,
        labels: BTreeMap<String, String>,
    ) -> Result<RemoteWorkerSnapshot, String> {
        if worker_id.trim().is_empty() {
            return Err("remote RPC worker ID cannot be empty".into());
        }
        if capacity == 0 {
            return Err("remote RPC worker capacity must be positive".into());
        }
        if lease_timeout.is_zero() {
            return Err("remote RPC worker lease timeout must be positive".into());
        }
        let mut workers = self
            .remote_workers
            .lock()
            .map_err(|_| "remote worker mutex poisoned".to_string())?;
        if let Some(existing) = workers.get(worker_id) {
            if !existing.active_jobs.is_empty() {
                return Err(format!(
                    "remote RPC worker '{worker_id}' cannot be re-registered while jobs are active"
                ));
            }
        }
        let worker = RemoteWorker {
            worker_id: worker_id.to_string(),
            capacity,
            lease_timeout,
            labels,
            active_jobs: BTreeSet::new(),
            last_heartbeat: Instant::now(),
        };
        let snapshot = remote_worker_snapshot(&worker);
        workers.insert(worker_id.to_string(), worker);
        Ok(snapshot)
    }

    pub fn remote_worker_heartbeat(&self, worker_id: &str) -> Result<RemoteWorkerSnapshot, String> {
        let mut workers = self
            .remote_workers
            .lock()
            .map_err(|_| "remote worker mutex poisoned".to_string())?;
        let worker = workers
            .get_mut(worker_id)
            .ok_or_else(|| format!("unknown remote RPC worker '{worker_id}'"))?;
        worker.last_heartbeat = Instant::now();
        let snapshot = remote_worker_snapshot(worker);
        drop(workers);
        let now = Instant::now();
        self.remote_claims
            .lock()
            .map_err(|_| "remote claim mutex poisoned".to_string())?
            .values_mut()
            .filter(|lease| lease.worker_id == worker_id)
            .for_each(|lease| lease.last_heartbeat = now);
        Ok(snapshot)
    }

    pub fn remote_workers(&self) -> Result<Vec<RemoteWorkerSnapshot>, String> {
        let workers = self
            .remote_workers
            .lock()
            .map_err(|_| "remote worker mutex poisoned".to_string())?;
        Ok(workers.values().map(remote_worker_snapshot).collect())
    }

    /// Requeue remote claims whose worker lease has timed out.
    ///
    /// A lease is renewed by worker registration, claim, or heartbeat. Expired
    /// ownership is removed before the request is returned to the remote queue,
    /// so a late completion from the previous owner cannot overwrite the job.
    /// Idle workers whose registration heartbeat has also expired are removed.
    pub fn expire_remote_leases(&self) -> Result<RemoteLeaseExpirationResult, String> {
        let mut claims = self
            .remote_claims
            .lock()
            .map_err(|_| "remote claim mutex poisoned".to_string())?;
        let mut expired = Vec::<(u64, RemoteJobLease)>::new();
        {
            let mut workers = self
                .remote_workers
                .lock()
                .map_err(|_| "remote worker mutex poisoned".to_string())?;
            for (job_id, lease) in claims.iter() {
                let timeout = workers
                    .get(&lease.worker_id)
                    .map_or(Duration::from_secs(60), |worker| worker.lease_timeout);
                if lease.last_heartbeat.elapsed() < timeout {
                    continue;
                }
                if let Some(worker) = workers.get_mut(&lease.worker_id) {
                    worker.active_jobs.remove(job_id);
                }
                expired.push((*job_id, lease.clone()));
            }
        }
        let expired_ids = expired.iter().map(|(id, _)| *id).collect::<Vec<_>>();

        if !expired.is_empty() {
            let mut running = self
                .running
                .lock()
                .map_err(|_| "running job state mutex poisoned".to_string())?;
            let mut pending = self
                .pending_remote_jobs
                .lock()
                .map_err(|_| "remote job queue mutex poisoned".to_string())?;
            let mut jobs = self
                .jobs
                .lock()
                .map_err(|_| "job state mutex poisoned".to_string())?;
            for (job_id, lease) in expired {
                claims.remove(&job_id);
                running.remove(&job_id);
                if lease.record.attempts >= lease.record.max_attempts {
                    jobs.insert(
                        job_id,
                        RpcJobStatus::Failed(format!(
                            "remote RPC job {job_id} exhausted {} attempt(s)",
                            lease.record.max_attempts
                        )),
                    );
                    self.remote_retry_exhausted.fetch_add(1, Ordering::SeqCst);
                } else {
                    pending.insert(job_id, lease.record);
                    jobs.insert(job_id, RpcJobStatus::Queued);
                    self.remote_reassignments.fetch_add(1, Ordering::SeqCst);
                }
            }
        }

        drop(claims);
        let mut removed_worker_ids = Vec::new();
        let mut workers = self
            .remote_workers
            .lock()
            .map_err(|_| "remote worker mutex poisoned".to_string())?;
        workers.retain(|worker_id, worker| {
            if worker.active_jobs.is_empty()
                && worker.last_heartbeat.elapsed() >= worker.lease_timeout
            {
                removed_worker_ids.push(worker_id.clone());
                false
            } else {
                true
            }
        });
        if !removed_worker_ids.is_empty() {
            self.remote_worker_expirations
                .fetch_add(removed_worker_ids.len() as u64, Ordering::SeqCst);
        }
        Ok(RemoteLeaseExpirationResult {
            expired_job_ids: expired_ids,
            removed_worker_ids,
        })
    }

    pub fn claim_remote_job(&self, worker_id: &str) -> Result<Option<RemoteJobClaim>, String> {
        self.expire_remote_leases()?;
        let worker_labels;
        {
            let workers = self
                .remote_workers
                .lock()
                .map_err(|_| "remote worker mutex poisoned".to_string())?;
            let Some(worker) = workers.get(worker_id) else {
                return Err(format!("unknown remote RPC worker '{worker_id}'"));
            };
            if worker.active_jobs.len() >= worker.capacity {
                return Ok(None);
            }
            worker_labels = worker.labels.clone();
        }

        let mut pending = self
            .pending_remote_jobs
            .lock()
            .map_err(|_| "remote job queue mutex poisoned".to_string())?;
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| "job state mutex poisoned".to_string())?;
        let mut selected_job = None::<(u64, u32)>;
        for (id, record) in pending.iter() {
            let queued = jobs
                .get(id)
                .is_some_and(|status| matches!(status, RpcJobStatus::Queued));
            let labels_match = record
                .required_worker_labels
                .iter()
                .all(|(name, value)| worker_labels.get(name) == Some(value));
            if queued
                && labels_match
                && selected_job.is_none_or(|(_, priority)| record.priority > priority)
            {
                selected_job = Some((*id, record.priority));
            }
        }
        let Some(job_id) = selected_job.map(|(job_id, _)| job_id) else {
            return Ok(None);
        };
        let mut record = pending
            .remove(&job_id)
            .ok_or_else(|| format!("remote RPC job {job_id} vanished from queue"))?;
        record.attempts += 1;
        let request = record.request.clone();
        drop(pending);
        drop(jobs);

        let cancellation = FlowCancellation::new();
        self.running
            .lock()
            .map_err(|_| "running job state mutex poisoned".to_string())?
            .insert(job_id, cancellation);
        self.remote_claims
            .lock()
            .map_err(|_| "remote claim mutex poisoned".to_string())?
            .insert(
                job_id,
                RemoteJobLease {
                    worker_id: worker_id.to_string(),
                    record: record.clone(),
                    last_heartbeat: Instant::now(),
                },
            );
        self.jobs
            .lock()
            .map_err(|_| "job state mutex poisoned".to_string())?
            .insert(job_id, RpcJobStatus::Running);
        let mut workers = self
            .remote_workers
            .lock()
            .map_err(|_| "remote worker mutex poisoned".to_string())?;
        let worker = workers
            .get_mut(worker_id)
            .ok_or_else(|| format!("unknown remote RPC worker '{worker_id}'"))?;
        worker.active_jobs.insert(job_id);
        worker.last_heartbeat = Instant::now();
        Ok(Some(RemoteJobClaim {
            job_id,
            worker_id: worker_id.to_string(),
            request,
            attempt: record.attempts,
            max_attempts: record.max_attempts,
            priority: record.priority,
            required_worker_labels: record.required_worker_labels,
        }))
    }

    pub fn remote_claim_status(&self, job_id: u64) -> Result<bool, String> {
        if !self
            .remote_claims
            .lock()
            .map_err(|_| "remote claim mutex poisoned".to_string())?
            .contains_key(&job_id)
        {
            return Err(format!("RPC job {job_id} is not remotely claimed"));
        }
        let running = self
            .running
            .lock()
            .map_err(|_| "running job state mutex poisoned".to_string())?;
        Ok(running
            .get(&job_id)
            .is_some_and(FlowCancellation::is_cancelled))
    }

    pub fn complete_remote_job(
        &self,
        worker_id: &str,
        job_id: u64,
        result: Result<Json, String>,
        artifacts: Vec<RemoteJobArtifact>,
    ) -> Result<RpcJobStatus, String> {
        let cancelled;
        let status;
        {
            let mut claims = self
                .remote_claims
                .lock()
                .map_err(|_| "remote claim mutex poisoned".to_string())?;
            let Some(lease) = claims.get(&job_id) else {
                return Err(format!("RPC job {job_id} is not remotely claimed"));
            };
            if lease.worker_id != worker_id {
                return Err(format!(
                    "remote RPC worker '{worker_id}' does not own job {job_id}"
                ));
            }
            cancelled = self
                .running
                .lock()
                .map_err(|_| "running job state mutex poisoned".to_string())?
                .remove(&job_id)
                .is_some_and(|token| token.is_cancelled());
            status = if cancelled {
                RpcJobStatus::Cancelled
            } else {
                match result {
                    Ok(value) => RpcJobStatus::Completed(value),
                    Err(error) => RpcJobStatus::Failed(error),
                }
            };
            if let Some(root) = &self.artifact_store {
                let record = remote_job_record_json(job_id, &status, &artifacts)?;
                write_remote_job_record(root, &record)?;
            }
            claims.remove(&job_id);
            drop(claims);
        }
        let mut remote_job_artifacts = self
            .remote_job_artifacts
            .lock()
            .map_err(|_| "remote job artifact mutex poisoned".to_string())?;
        let mut jobs = self
            .jobs
            .lock()
            .map_err(|_| "job state mutex poisoned".to_string())?;
        if cancelled {
            remote_job_artifacts.remove(&job_id);
        } else {
            let job_artifacts = remote_job_artifacts.entry(job_id).or_default();
            for artifact in artifacts {
                job_artifacts.insert(artifact.name.clone(), artifact);
            }
            enforce_remote_artifact_limits_locked(
                &mut remote_job_artifacts,
                job_id,
                self.remote_artifact_capacity,
                self.remote_artifact_byte_capacity,
                &self.remote_artifact_evictions,
            )?;
        }
        jobs.insert(job_id, status.clone());
        if let Some(worker) = self
            .remote_workers
            .lock()
            .map_err(|_| "remote worker mutex poisoned".to_string())?
            .get_mut(worker_id)
        {
            worker.active_jobs.remove(&job_id);
            worker.last_heartbeat = Instant::now();
        }
        Ok(status)
    }

    pub fn remote_job_artifacts(
        &self,
        job_id: u64,
    ) -> Result<Vec<RemoteJobArtifactMetadata>, String> {
        let artifacts = self
            .remote_job_artifacts
            .lock()
            .map_err(|_| "remote job artifact mutex poisoned".to_string())?;
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| "job state mutex poisoned".to_string())?;
        if !jobs.contains_key(&job_id) {
            return Err(format!("unknown RPC job {job_id}"));
        }
        Ok(artifacts
            .get(&job_id)
            .map(BTreeMap::values)
            .map_or_else(Vec::new, |values| {
                values.map(RemoteJobArtifactMetadata::from).collect()
            }))
    }

    pub fn remote_job_artifact(
        &self,
        job_id: u64,
        name: &str,
    ) -> Result<RemoteJobArtifact, String> {
        let mut artifacts = self
            .remote_job_artifacts
            .lock()
            .map_err(|_| "remote job artifact mutex poisoned".to_string())?;
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| "job state mutex poisoned".to_string())?;
        if !jobs.contains_key(&job_id) {
            return Err(format!("unknown RPC job {job_id}"));
        }
        let artifact = artifacts
            .get_mut(&job_id)
            .and_then(|artifacts| artifacts.get_mut(name))
            .ok_or_else(|| format!("RPC job {job_id} has no artifact '{name}'"))?;
        artifact.last_access = Instant::now();
        Ok(artifact.clone())
    }

    pub fn delete_remote_job_artifact(
        &self,
        job_id: u64,
        name: &str,
    ) -> Result<RemoteJobArtifactMetadata, String> {
        let mut artifacts = self
            .remote_job_artifacts
            .lock()
            .map_err(|_| "remote job artifact mutex poisoned".to_string())?;
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| "job state mutex poisoned".to_string())?;
        if !jobs.contains_key(&job_id) {
            return Err(format!("unknown RPC job {job_id}"));
        }
        let Some(job_artifacts) = artifacts.get_mut(&job_id) else {
            return Err(format!("RPC job {job_id} has no artifact '{name}'"));
        };
        let Some(artifact) = job_artifacts.get(name) else {
            return Err(format!("RPC job {job_id} has no artifact '{name}'"));
        };
        let metadata = RemoteJobArtifactMetadata::from(artifact);
        let remaining = job_artifacts
            .values()
            .filter(|artifact| artifact.name != name)
            .cloned()
            .collect::<Vec<_>>();
        if let Some(root) = &self.artifact_store {
            let status = jobs
                .get(&job_id)
                .cloned()
                .ok_or_else(|| format!("unknown RPC job {job_id}"))?;
            let record = remote_job_record_json(job_id, &status, &remaining)?;
            write_remote_job_record(root, &record)?;
        }
        job_artifacts.remove(name);
        Ok(metadata)
    }

    /// Cancel a queued job or request cooperative cancellation of a running job.
    pub fn cancel(&self, id: u64) -> Result<bool, String> {
        let running = self
            .running
            .lock()
            .map_err(|_| "running job state mutex poisoned".to_string())?;
        let mut jobs = self
            .jobs
            .lock()
            .map_err(|_| "job state mutex poisoned".to_string())?;
        if let Some(state) = jobs.get_mut(&id) {
            if matches!(state, RpcJobStatus::Queued) {
                *state = RpcJobStatus::Cancelled;
                if let Some(token) = running.get(&id) {
                    token.cancel();
                }
                return Ok(true);
            }
            if matches!(state, RpcJobStatus::Running) {
                if let Some(token) = running.get(&id) {
                    if token.is_cancelled() {
                        return Ok(true);
                    }
                    return Ok(token.cancel());
                }
                return Ok(false);
            }
            return Ok(false);
        }
        Ok(false)
    }

    pub fn run_to_completion(&self, request: RpcRequest) -> Result<RpcJobStatus, String> {
        let id = self.submit(request)?;
        for _ in 0..20_000 {
            if let Some(
                status @ (RpcJobStatus::Completed(_)
                | RpcJobStatus::Failed(_)
                | RpcJobStatus::Cancelled),
            ) = self.poll(id)?
            {
                return Ok(status);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Err(format!("RPC job {id} did not finish within 20 seconds"))
    }

    pub fn shutdown(mut self) -> Result<(), String> {
        self.shutdown_background()
    }

    pub fn shutdown_background(&mut self) -> Result<(), String> {
        if let Some(workers) = self.workers.take() {
            for _ in &workers {
                self.sender
                    .send(WorkerCommand::Shutdown)
                    .map_err(|_| "daemon worker already stopped".to_string())?;
            }
            for worker in workers {
                worker
                    .join()
                    .map_err(|_| "daemon worker panicked".to_string())?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RpcJobStats {
    pub worker_count: usize,
    pub remote_worker_count: usize,
    pub remote_active_job_count: usize,
    pub remote_artifact_count: usize,
    pub remote_artifact_bytes: usize,
    pub remote_artifact_capacity: usize,
    pub remote_artifact_byte_capacity: usize,
    pub remote_artifact_evictions: u64,
    pub remote_reassignments: u64,
    pub remote_retry_exhausted: u64,
    pub remote_worker_expirations: u64,
    pub queued: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub active_job_ids: Vec<u64>,
}

fn remote_worker_snapshot(worker: &RemoteWorker) -> RemoteWorkerSnapshot {
    RemoteWorkerSnapshot {
        worker_id: worker.worker_id.clone(),
        capacity: worker.capacity,
        lease_timeout_seconds: worker.lease_timeout.as_secs(),
        labels: worker.labels.clone(),
        active_job_ids: worker.active_jobs.iter().copied().collect(),
        last_heartbeat_age_seconds: worker.last_heartbeat.elapsed().as_secs(),
    }
}

fn remote_job_record_path(root: &Path, job_id: u64) -> PathBuf {
    root.join(format!("{job_id}.json"))
}

fn remote_job_next_id_path(root: &Path) -> PathBuf {
    root.join("next-id")
}

fn write_remote_job_next_id(root: &Path, next_id: u64) -> Result<(), String> {
    let temporary = root.join("next-id.tmp");
    let target = remote_job_next_id_path(root);
    let mut file = File::options()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| {
            format!(
                "failed to open temporary RPC job ID watermark {}: {error}",
                temporary.display()
            )
        })?;
    file.write_all(next_id.to_string().as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            format!(
                "failed to write temporary RPC job ID watermark {}: {error}",
                temporary.display()
            )
        })?;
    drop(file);
    fs::rename(&temporary, &target).map_err(|error| {
        format!(
            "failed to publish RPC job ID watermark {}: {error}",
            target.display()
        )
    })
}

fn remote_job_record_json(
    job_id: u64,
    status: &RpcJobStatus,
    artifacts: &[RemoteJobArtifact],
) -> Result<Json, String> {
    let (state, result, error) = match status {
        RpcJobStatus::Queued => ("queued", None, None),
        RpcJobStatus::Running => ("running", None, None),
        RpcJobStatus::Completed(value) => ("completed", Some(value.clone()), None),
        RpcJobStatus::Failed(message) => ("failed", None, Some(message.clone())),
        RpcJobStatus::Cancelled => ("cancelled", None, None),
    };
    let mut fields = vec![
        ("schemaVersion".to_string(), Json::from(1_u64)),
        ("jobId".to_string(), Json::from(job_id)),
        ("state".to_string(), Json::string(state)),
    ];
    if let Some(result) = result {
        fields.push(("result".to_string(), result));
    }
    if let Some(error) = error {
        fields.push(("error".to_string(), Json::string(error)));
    }
    fields.push((
        "artifacts".to_string(),
        Json::Array(
            artifacts
                .iter()
                .map(|artifact| {
                    Json::object([
                        ("name", Json::string(artifact.name.clone())),
                        ("contentType", Json::string(artifact.content_type.clone())),
                        (
                            "contentEncoding",
                            Json::string(artifact.content_encoding.clone()),
                        ),
                        ("content", Json::string(artifact.content.clone())),
                        ("sha256", Json::string(artifact.sha256.clone())),
                        ("sizeBytes", Json::from(artifact.size_bytes as u64)),
                    ])
                })
                .collect(),
        ),
    ));
    Ok(Json::Object(fields.into_iter().collect()))
}

pub(crate) fn write_remote_job_record(root: &Path, record: &Json) -> Result<(), String> {
    let job_id = record
        .as_object()
        .and_then(|fields| fields.get("jobId"))
        .and_then(Json::as_number)
        .copied()
        .ok_or_else(|| "RPC job record has no numeric jobId".to_string())? as u64;
    let temporary = root.join(format!("{job_id}.json.tmp"));
    let target = remote_job_record_path(root, job_id);
    let mut file = File::options()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| {
            format!(
                "failed to open temporary RPC job record {}: {error}",
                temporary.display()
            )
        })?;
    file.write_all(record.write().as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            format!(
                "failed to write temporary RPC job record {}: {error}",
                temporary.display()
            )
        })?;
    drop(file);
    fs::rename(&temporary, &target).map_err(|error| {
        format!(
            "failed to publish RPC job record {}: {error}",
            target.display()
        )
    })
}

fn remote_job_status_from_record(record: &Json) -> Result<RpcJobStatus, String> {
    let fields = record
        .as_object()
        .ok_or_else(|| "RPC job record must be an object".to_string())?;
    let state = fields
        .get("state")
        .and_then(Json::as_str)
        .ok_or_else(|| "RPC job record 'state' must be a string".to_string())?;
    match state {
        "completed" => Ok(RpcJobStatus::Completed(
            fields.get("result").cloned().unwrap_or(Json::Null),
        )),
        "failed" => Ok(RpcJobStatus::Failed(
            fields
                .get("error")
                .and_then(Json::as_str)
                .ok_or_else(|| "failed RPC job record requires a string error".to_string())?
                .to_string(),
        )),
        "cancelled" => Ok(RpcJobStatus::Cancelled),
        _ => Err(format!(
            "RPC job record has unsupported terminal state '{state}'"
        )),
    }
}

fn remote_artifact_registry_totals(
    artifacts: &BTreeMap<u64, BTreeMap<String, RemoteJobArtifact>>,
) -> (usize, usize) {
    artifacts.values().flat_map(BTreeMap::values).fold(
        (0_usize, 0_usize),
        |(count, bytes), artifact| {
            (
                count.saturating_add(1),
                bytes.saturating_add(artifact.size_bytes),
            )
        },
    )
}

fn enforce_remote_artifact_limits_locked(
    artifacts: &mut BTreeMap<u64, BTreeMap<String, RemoteJobArtifact>>,
    protected_job_id: u64,
    count_capacity: usize,
    byte_capacity: usize,
    evictions: &AtomicU64,
) -> Result<(), String> {
    loop {
        let (count, bytes) = remote_artifact_registry_totals(artifacts);
        if count <= count_capacity && bytes <= byte_capacity {
            return Ok(());
        }
        let candidates = artifacts
            .iter()
            .filter(|(job_id, _)| **job_id != protected_job_id)
            .flat_map(|(job_id, artifacts)| {
                artifacts
                    .iter()
                    .map(move |(name, artifact)| (*job_id, name.clone(), artifact.last_access))
            })
            .collect::<Vec<_>>();
        let Some((victim_job_id, victim_name, _)) = candidates
            .into_iter()
            .min_by_key(|(_, _, last_access)| *last_access)
        else {
            return Err(format!(
                "remote artifact registry cannot hold protected job {protected_job_id} within {count_capacity} artifact(s) and {byte_capacity} byte(s)"
            ));
        };
        if let Some(job_artifacts) = artifacts.get_mut(&victim_job_id) {
            job_artifacts.remove(&victim_name);
            if job_artifacts.is_empty() {
                artifacts.remove(&victim_job_id);
            }
        }
        evictions.fetch_add(1, Ordering::SeqCst);
    }
}

type RpcDispatcher =
    Arc<dyn Fn(RpcRequest, FlowCancellation) -> Result<Json, String> + Send + Sync>;

impl Default for RpcJobManager {
    fn default() -> Self {
        Self::new()
    }
}

fn worker_loop(
    receiver: WorkerChannel,
    jobs: SharedJobs,
    running: SharedRunning,
    dispatcher: RpcDispatcher,
) {
    while let Some(command) = receiver.receive() {
        match command {
            WorkerCommand::Shutdown => break,
            WorkerCommand::Submit(id, request, provided_cancellation) => {
                let cancellation = provided_cancellation.unwrap_or_default();
                {
                    let Ok(mut running_tokens) = running.lock() else {
                        continue;
                    };
                    running_tokens.insert(id, cancellation.clone());
                    let Ok(mut jobs) = jobs.lock() else {
                        running_tokens.remove(&id);
                        continue;
                    };
                    match jobs.get(&id) {
                        Some(RpcJobStatus::Cancelled) => {
                            running_tokens.remove(&id);
                            continue;
                        }
                        Some(_) => {
                            jobs.insert(id, RpcJobStatus::Running);
                        }
                        None => {
                            running_tokens.remove(&id);
                            continue;
                        }
                    }
                }
                let result = dispatcher(request, cancellation.clone());
                if let Ok(mut running_tokens) = running.lock() {
                    running_tokens.remove(&id);
                }
                let status = if cancellation.is_cancelled() {
                    RpcJobStatus::Cancelled
                } else {
                    match result {
                        Ok(value) => RpcJobStatus::Completed(value),
                        Err(error) => RpcJobStatus::Failed(error),
                    }
                };
                if let Ok(mut jobs) = jobs.lock() {
                    jobs.insert(id, status);
                }
            }
        }
    }
}
