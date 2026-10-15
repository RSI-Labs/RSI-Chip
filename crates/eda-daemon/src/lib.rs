//! Persistent JSON-RPC-style job service for long-running agent sessions.
//!
//! The protocol is deliberately line-delimited JSON so an agent can use a
//! socket without linking the Rust API. Jobs are also exposed as a native,
//! typed manager for in-process automation.

use eda_api::{capability_report, FlowCancellation, SharedFlowCache};
use std::collections::BTreeMap;
use std::time::Instant;

mod analysis;
mod implementation;
mod jobs;
mod layout;
mod physical;
mod physical_timing;
mod protocol;
mod server;
mod service;
mod timing;
mod verification;

pub const PROTOCOL_VERSION: usize = 1;
use analysis::{
    assertions_rpc, functional_coverage_rpc, spice_dc, spice_run, waveform_measure_rpc,
};
use implementation::{
    flow_cache_stats_json, rtl_cts, rtl_eco, rtl_equiv, rtl_equiv_views, rtl_flow, rtl_flow_cached,
    rtl_signal_quality, rtl_sta,
};
pub use jobs::{
    RemoteJobArtifact, RemoteJobArtifactMetadata, RemoteJobClaim, RemoteLeaseExpirationResult,
    RemoteWorkerSnapshot, RpcJobManager, RpcJobStats, RpcJobStatus,
};
use layout::{
    layout_any_angle_boolean_rpc, layout_any_angle_maze_route_rpc, layout_any_angle_route_rpc,
    layout_drc_rpc, layout_drc_rpc_with_registry, layout_drc_waiver_registry_add_waiver,
    layout_drc_waiver_registry_create, layout_drc_waiver_registry_delete,
    layout_drc_waiver_registry_get, layout_drc_waiver_registry_list,
    layout_drc_waiver_registry_remove_waiver, layout_rule_deck_rpc,
};
use physical::{rtl_antenna_repair, rtl_ir, rtl_pnr, rtl_power, rtl_sim};
use physical_timing::{
    pdk_signoff, rtl_fault, rtl_physical_timing, rtl_post_route_power_ir,
    rtl_post_route_power_mode_ir,
};
pub use protocol::{parse_json, Json, RpcRequest, RpcResponse};
pub use server::{serve_stream, serve_stream_with_service, Daemon};
pub use service::RpcService;
use service::{SharedLayoutDrcWaiverRegistries, SharedTimingViolationRegistries};
use timing::{
    rtl_incremental_mmmc_timing, rtl_incremental_mmmc_timing_violations, rtl_mmmc_timing,
    rtl_mmmc_timing_paths, rtl_mmmc_timing_violations, timing_scenario_diff, timing_waiver_audit,
    timing_waiver_registry_add_waivers, timing_waiver_registry_create,
    timing_waiver_registry_delete, timing_waiver_registry_get, timing_waiver_registry_list,
    timing_waiver_registry_remove_waiver, timing_waiver_registry_update_scenarios,
};
use verification::{rtl_bmc, rtl_cdc, rtl_dft, rtl_lvs, rtl_power_intent, rtl_safety};

const MAX_RPC_LINE_BYTES: usize = 8 * 1024 * 1024;
const MAX_REMOTE_JOB_ARTIFACTS_PER_JOB: usize = 64;
const MAX_REMOTE_JOB_ARTIFACT_BYTES_PER_JOB: usize = 8 * 1024 * 1024;
const MAX_REMOTE_JOB_ARTIFACT_PAGE_SIZE: usize = 256;
const MAX_REMOTE_JOB_ARTIFACT_REGISTRY_COUNT: usize = 512;
const MAX_REMOTE_JOB_ARTIFACT_REGISTRY_BYTES: usize = 64 * 1024 * 1024;

pub fn dispatch(request: &RpcRequest) -> RpcResponse {
    RpcResponse {
        id: request.id.clone(),
        result: dispatch_result(request),
    }
}

fn required_value(params: &Json, name: &str) -> Result<Json, String> {
    params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?
        .get(name)
        .cloned()
        .ok_or_else(|| format!("RPC parameter '{name}' is required"))
}

pub(crate) fn required_job_id(params: &Json) -> Result<u64, String> {
    match required_value(params, "jobId")? {
        Json::Number(value) if value >= 0.0 && value.fract() == 0.0 && value <= u64::MAX as f64 => {
            Ok(value as u64)
        }
        _ => Err("'jobId' must be a non-negative integer".into()),
    }
}

pub(crate) fn required_artifact_name(params: &Json) -> Result<String, String> {
    let name = required_string(params, "name")?;
    if name.trim().is_empty() {
        return Err("'name' cannot be empty".into());
    }
    Ok(name)
}

pub(crate) fn parse_string_map(
    params: &Json,
    name: &str,
) -> Result<BTreeMap<String, String>, String> {
    let Some(fields) = params.as_object() else {
        return Err("params must be an object".to_string());
    };
    let values = match fields.get(name) {
        None | Some(Json::Null) => return Ok(BTreeMap::new()),
        Some(Json::Object(values)) => values,
        Some(_) => return Err(format!("'{name}' must be an object of strings")),
    };
    let mut parsed = BTreeMap::new();
    for (key, value) in values {
        let Json::String(value) = value else {
            return Err(format!("'{name}' values must be strings"));
        };
        if key.trim().is_empty() {
            return Err(format!("'{name}' keys cannot be empty"));
        }
        if value.trim().is_empty() {
            return Err(format!("'{name}'['{key}'] cannot be empty"));
        }
        parsed.insert(key.clone(), value.clone());
    }
    Ok(parsed)
}

fn string_map_json(values: &BTreeMap<String, String>) -> Json {
    Json::object(
        values
            .iter()
            .map(|(key, value)| (key.clone(), Json::string(value.clone()))),
    )
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_value(byte: u8) -> Option<u8> {
    BASE64_ALPHABET
        .iter()
        .position(|candidate| *candidate == byte)
        .map(|position| position as u8)
}

fn base64_encode(message: &[u8]) -> String {
    let mut encoded = String::with_capacity(message.len().div_ceil(3) * 4);
    for chunk in message.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        let word = ((first as u32) << 16) | ((second as u32) << 8) | third as u32;
        encoded.push(BASE64_ALPHABET[(word >> 18) as usize & 0x3f] as char);
        encoded.push(BASE64_ALPHABET[(word >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            encoded.push(BASE64_ALPHABET[(word >> 6) as usize & 0x3f] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(BASE64_ALPHABET[word as usize & 0x3f] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}

fn base64_decode(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(4) {
        return Err("artifact base64 content must use padded four-byte groups".into());
    }
    let padding = value.bytes().filter(|byte| *byte == b'=').count();
    if padding > 2 {
        return Err("artifact base64 content has invalid padding".into());
    }
    let mut decoded = Vec::with_capacity(value.len() / 4 * 3);
    for group in value.as_bytes().chunks_exact(4) {
        let mut symbols = [0_u8; 4];
        for (index, byte) in group.iter().enumerate() {
            if *byte == b'=' {
                if index < 2 || group[..index].contains(&b'=') {
                    return Err("artifact base64 content has misplaced padding".into());
                }
                symbols[index] = 0;
                continue;
            }
            symbols[index] = base64_value(*byte)
                .ok_or_else(|| "artifact content is not canonical base64".to_string())?;
        }
        let word = ((symbols[0] as u32) << 18)
            | ((symbols[1] as u32) << 12)
            | ((symbols[2] as u32) << 6)
            | symbols[3] as u32;
        decoded.push((word >> 16) as u8);
        if group[2] != b'=' {
            decoded.push((word >> 8) as u8);
        }
        if group[3] != b'=' {
            decoded.push(word as u8);
        }
    }
    if base64_encode(&decoded) != value {
        return Err("artifact content is not canonical base64".into());
    }
    Ok(decoded)
}

pub(crate) fn parse_remote_job_artifacts(
    params: &Json,
    job_id: u64,
) -> Result<Vec<RemoteJobArtifact>, String> {
    let Some(fields) = params.as_object() else {
        return Err("params must be an object".to_string());
    };
    let artifacts = match fields.get("artifacts") {
        None | Some(Json::Null) => return Ok(Vec::new()),
        Some(Json::Array(artifacts)) => artifacts,
        Some(_) => return Err("'artifacts' must be an array".into()),
    };
    if artifacts.len() > MAX_REMOTE_JOB_ARTIFACTS_PER_JOB {
        return Err(format!(
            "remote RPC job accepts at most {MAX_REMOTE_JOB_ARTIFACTS_PER_JOB} artifacts"
        ));
    }

    let mut parsed = BTreeMap::<String, RemoteJobArtifact>::new();
    let mut total_decoded_bytes = 0_usize;
    for artifact in artifacts {
        let Some(fields) = artifact.as_object() else {
            return Err("each artifact must be an object".into());
        };
        let Json::String(name) = fields.get("name").ok_or("artifact 'name' is required")? else {
            return Err("artifact 'name' must be a string".into());
        };
        if name.trim().is_empty() {
            return Err("artifact 'name' cannot be empty".into());
        }
        let Json::String(content) = fields
            .get("content")
            .ok_or("artifact 'content' is required")?
        else {
            return Err("artifact 'content' must be a string".into());
        };
        let content_type = match fields.get("contentType") {
            None | Some(Json::Null) => "text/plain".to_string(),
            Some(Json::String(value)) => {
                if value.trim().is_empty() {
                    return Err("artifact 'contentType' cannot be empty".into());
                }
                value.clone()
            }
            Some(_) => return Err("artifact 'contentType' must be a string".into()),
        };
        let content_encoding = match fields.get("contentEncoding") {
            None | Some(Json::Null) => "utf-8".to_string(),
            Some(Json::String(value)) => {
                if value != "utf-8" && value != "base64" {
                    return Err(
                        "artifact 'contentEncoding' must be either 'utf-8' or 'base64'".into(),
                    );
                }
                value.clone()
            }
            Some(_) => {
                return Err("artifact 'contentEncoding' must be either 'utf-8' or 'base64'".into())
            }
        };
        if parsed.contains_key(name) {
            return Err(format!("duplicate remote RPC artifact name '{name}'"));
        }
        let content_bytes = if content_encoding == "base64" {
            base64_decode(content)?
        } else {
            content.as_bytes().to_vec()
        };
        let size_bytes = content_bytes.len();
        total_decoded_bytes = total_decoded_bytes
            .checked_add(size_bytes)
            .ok_or_else(|| "remote RPC artifact byte quota overflowed".to_string())?;
        if total_decoded_bytes > MAX_REMOTE_JOB_ARTIFACT_BYTES_PER_JOB {
            return Err(format!(
                "remote RPC job accepts at most {MAX_REMOTE_JOB_ARTIFACT_BYTES_PER_JOB} decoded artifact bytes"
            ));
        }
        let sha256 = eda_api::pdk_liberty_sha256_bytes(&content_bytes);
        match fields.get("sha256") {
            None | Some(Json::Null) => {}
            Some(Json::String(expected)) => {
                if expected != &sha256 {
                    return Err(format!("remote RPC artifact '{name}' checksum mismatch"));
                }
            }
            Some(_) => return Err("artifact 'sha256' must be a string".into()),
        }
        match fields.get("sizeBytes") {
            None | Some(Json::Null) => {}
            Some(Json::Number(expected)) => {
                if *expected < 0.0
                    || expected.fract() != 0.0
                    || *expected > usize::MAX as f64
                    || *expected as usize != size_bytes
                {
                    return Err(format!("remote RPC artifact '{name}' size mismatch"));
                }
            }
            Some(_) => return Err("artifact 'sizeBytes' must be a non-negative integer".into()),
        }
        let artifact = RemoteJobArtifact {
            job_id,
            name: name.clone(),
            content_type,
            content_encoding,
            content: content.clone(),
            sha256,
            size_bytes,
            last_access: Instant::now(),
        };
        parsed.insert(name.clone(), artifact);
    }
    Ok(parsed.into_values().collect())
}

pub(crate) fn optional_positive_usize(
    params: &Json,
    name: &str,
    default: usize,
) -> Result<usize, String> {
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    match fields.get(name) {
        None | Some(Json::Null) => Ok(default),
        Some(Json::Number(value)) => {
            if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
                return Err(format!("'{name}' must be a positive integer"));
            }
            Ok(*value as usize)
        }
        Some(_) => Err(format!("'{name}' must be a positive integer")),
    }
}

pub(crate) fn optional_nonnegative_usize(
    params: &Json,
    name: &str,
    default: usize,
) -> Result<usize, String> {
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    match fields.get(name) {
        None | Some(Json::Null) => Ok(default),
        Some(Json::Number(value)) => {
            if *value < 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
                return Err(format!("'{name}' must be a non-negative integer"));
            }
            Ok(*value as usize)
        }
        Some(_) => Err(format!("'{name}' must be a non-negative integer")),
    }
}

pub(crate) fn job_status_json(id: u64, status: &RpcJobStatus) -> Result<Json, String> {
    let (state, result, error) = match status {
        RpcJobStatus::Queued => ("queued", None, None),
        RpcJobStatus::Running => ("running", None, None),
        RpcJobStatus::Completed(value) => ("completed", Some(value.clone()), None),
        RpcJobStatus::Failed(message) => ("failed", None, Some(message.clone())),
        RpcJobStatus::Cancelled => ("cancelled", None, None),
    };
    let mut fields = vec![
        ("jobId".to_string(), Json::number(id as f64)?),
        ("state".to_string(), Json::string(state)),
    ];
    if let Some(result) = result {
        fields.push(("result".to_string(), result));
    }
    if let Some(error) = error {
        fields.push(("error".to_string(), Json::string(error)));
    }
    Ok(Json::Object(fields.into_iter().collect()))
}

fn dispatch_result(request: &RpcRequest) -> Result<Json, String> {
    match request.method.as_str() {
        "ping" => Ok(Json::object([
            ("protocol", Json::number(PROTOCOL_VERSION as f64)?),
            ("service", Json::string("eda")),
        ])),
        "capabilities" => Ok(Json::object([(
            "report",
            Json::string(capability_report()),
        )])),
        "coverage" => functional_coverage_rpc(&request.params, &FlowCancellation::new()),
        "assertions" => assertions_rpc(&request.params, &FlowCancellation::new()),
        "waveformMeasure" => waveform_measure_rpc(&request.params, &FlowCancellation::new()),
        "layoutRuleDeck" => layout_rule_deck_rpc(&request.params, &FlowCancellation::new()),
        "layoutDrc" => layout_drc_rpc(&request.params, &FlowCancellation::new()),
        "layoutAnyAngleRoute" => {
            layout_any_angle_route_rpc(&request.params, &FlowCancellation::new())
        }
        "layoutAnyAngleMazeRoute" => {
            layout_any_angle_maze_route_rpc(&request.params, &FlowCancellation::new())
        }
        "layoutAnyAngleBoolean" => {
            layout_any_angle_boolean_rpc(&request.params, &FlowCancellation::new())
        }
        "spiceDc" => spice_dc(&request.params, &FlowCancellation::new()),
        "spiceRun" => spice_run(&request.params, &FlowCancellation::new()),
        "rtlFlow" => rtl_flow(&request.params, &FlowCancellation::new()),
        "rtlCts" => rtl_cts(&request.params, &FlowCancellation::new()),
        "rtlEco" => rtl_eco(&request.params, &FlowCancellation::new()),
        "rtlEquiv" => rtl_equiv(&request.params, &FlowCancellation::new()),
        "rtlEquivViews" => rtl_equiv_views(&request.params, &FlowCancellation::new()),
        "rtlSta" => rtl_sta(&request.params, &FlowCancellation::new()),
        "rtlMmmcTiming" => rtl_mmmc_timing(&request.params, &FlowCancellation::new()),
        "rtlMmmcTimingPaths" => rtl_mmmc_timing_paths(&request.params, &FlowCancellation::new()),
        "rtlMmmcTimingViolations" => {
            rtl_mmmc_timing_violations(&request.params, &FlowCancellation::new())
        }
        "rtlIncrementalMmmcTimingViolations" => {
            rtl_incremental_mmmc_timing_violations(&request.params, &FlowCancellation::new())
        }
        "rtlSignalQuality" => rtl_signal_quality(&request.params, &FlowCancellation::new()),
        "timingScenarioDiff" => timing_scenario_diff(&request.params, &FlowCancellation::new()),
        "timingWaiverAudit" => timing_waiver_audit(&request.params, &FlowCancellation::new()),
        "rtlIncrementalMmmcTiming" => {
            rtl_incremental_mmmc_timing(&request.params, &FlowCancellation::new())
        }
        "rtlPnr" => rtl_pnr(&request.params, &FlowCancellation::new()),
        "rtlAntennaRepair" => rtl_antenna_repair(&request.params, &FlowCancellation::new()),
        "rtlPower" => rtl_power(&request.params, &FlowCancellation::new()),
        "rtlIr" => rtl_ir(&request.params, &FlowCancellation::new()),
        "rtlBmc" => rtl_bmc(&request.params, &FlowCancellation::new()),
        "rtlDft" => rtl_dft(&request.params, &FlowCancellation::new()),
        "rtlCdc" => rtl_cdc(&request.params, &FlowCancellation::new()),
        "rtlFault" => rtl_fault(&request.params, &FlowCancellation::new()),
        "rtlSim" => rtl_sim(&request.params, &FlowCancellation::new()),
        "rtlLvs" => rtl_lvs(&request.params, &FlowCancellation::new()),
        "rtlPowerIntent" => rtl_power_intent(&request.params, &FlowCancellation::new()),
        "rtlSafety" => rtl_safety(&request.params, &FlowCancellation::new()),
        "rtlPhysicalTiming" => rtl_physical_timing(&request.params, &FlowCancellation::new()),
        "rtlPostRoutePowerIr" => rtl_post_route_power_ir(&request.params, &FlowCancellation::new()),
        "rtlPostRoutePowerModeIr" => {
            rtl_post_route_power_mode_ir(&request.params, &FlowCancellation::new())
        }
        "pdkSignoff" => pdk_signoff(&request.params, &FlowCancellation::new()),
        method => Err(format!("unknown RPC method '{method}'")),
    }
}

pub(crate) fn dispatch_result_with_flow_cache(
    request: &RpcRequest,
    cancellation: &FlowCancellation,
    flow_cache: &SharedFlowCache,
    timing_waiver_registries: &SharedTimingViolationRegistries,
    layout_drc_waiver_registries: &SharedLayoutDrcWaiverRegistries,
) -> Result<Json, String> {
    match request.method.as_str() {
        "rtlFlowCache" => rtl_flow_cached(&request.params, cancellation, flow_cache),
        "timingWaiverRegistryCreate" => timing_waiver_registry_create(
            &request.params,
            cancellation,
            flow_cache,
            timing_waiver_registries,
        ),
        "timingWaiverRegistryGet" => {
            timing_waiver_registry_get(&request.params, cancellation, timing_waiver_registries)
        }
        "timingWaiverRegistryList" => {
            timing_waiver_registry_list(cancellation, timing_waiver_registries)
        }
        "timingWaiverRegistryAddWaivers" => timing_waiver_registry_add_waivers(
            &request.params,
            cancellation,
            timing_waiver_registries,
        ),
        "timingWaiverRegistryRemoveWaiver" => timing_waiver_registry_remove_waiver(
            &request.params,
            cancellation,
            timing_waiver_registries,
        ),
        "timingWaiverRegistryUpdateScenarios" => timing_waiver_registry_update_scenarios(
            &request.params,
            cancellation,
            timing_waiver_registries,
        ),
        "timingWaiverRegistryDelete" => {
            timing_waiver_registry_delete(&request.params, cancellation, timing_waiver_registries)
        }
        "layoutDrcWaiverRegistryCreate" => layout_drc_waiver_registry_create(
            &request.params,
            cancellation,
            layout_drc_waiver_registries,
        ),
        "layoutDrcWaiverRegistryGet" => layout_drc_waiver_registry_get(
            &request.params,
            cancellation,
            layout_drc_waiver_registries,
        ),
        "layoutDrcWaiverRegistryList" => {
            layout_drc_waiver_registry_list(cancellation, layout_drc_waiver_registries)
        }
        "layoutDrcWaiverRegistryAddWaiver" => layout_drc_waiver_registry_add_waiver(
            &request.params,
            cancellation,
            layout_drc_waiver_registries,
        ),
        "layoutDrcWaiverRegistryRemoveWaiver" => layout_drc_waiver_registry_remove_waiver(
            &request.params,
            cancellation,
            layout_drc_waiver_registries,
        ),
        "layoutDrcWaiverRegistryDelete" => layout_drc_waiver_registry_delete(
            &request.params,
            cancellation,
            layout_drc_waiver_registries,
        ),
        "layoutDrc" => layout_drc_rpc_with_registry(
            &request.params,
            cancellation,
            layout_drc_waiver_registries,
        ),
        "flowCacheStats" => flow_cache_stats_json(flow_cache),
        "flowCacheClear" => {
            flow_cache.clear()?;
            Ok(Json::object([("cleared", Json::Bool(true))]))
        }
        _ => dispatch_result_cancellable(request, cancellation),
    }
}

pub(crate) fn dispatch_result_cancellable(
    request: &RpcRequest,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RPC job cancelled".into());
    }
    match request.method.as_str() {
        "coverage" => functional_coverage_rpc(&request.params, cancellation),
        "assertions" => assertions_rpc(&request.params, cancellation),
        "waveformMeasure" => waveform_measure_rpc(&request.params, cancellation),
        "layoutRuleDeck" => layout_rule_deck_rpc(&request.params, cancellation),
        "layoutDrc" => layout_drc_rpc(&request.params, cancellation),
        "layoutAnyAngleRoute" => layout_any_angle_route_rpc(&request.params, cancellation),
        "layoutAnyAngleMazeRoute" => layout_any_angle_maze_route_rpc(&request.params, cancellation),
        "layoutAnyAngleBoolean" => layout_any_angle_boolean_rpc(&request.params, cancellation),
        "rtlFlow" => rtl_flow(&request.params, cancellation),
        "rtlCts" => rtl_cts(&request.params, cancellation),
        "rtlEco" => rtl_eco(&request.params, cancellation),
        "rtlEquiv" => rtl_equiv(&request.params, cancellation),
        "rtlEquivViews" => rtl_equiv_views(&request.params, cancellation),
        "rtlSta" => rtl_sta(&request.params, cancellation),
        "rtlMmmcTiming" => rtl_mmmc_timing(&request.params, cancellation),
        "rtlMmmcTimingPaths" => rtl_mmmc_timing_paths(&request.params, cancellation),
        "rtlMmmcTimingViolations" => rtl_mmmc_timing_violations(&request.params, cancellation),
        "rtlIncrementalMmmcTimingViolations" => {
            rtl_incremental_mmmc_timing_violations(&request.params, cancellation)
        }
        "rtlSignalQuality" => rtl_signal_quality(&request.params, cancellation),
        "timingScenarioDiff" => timing_scenario_diff(&request.params, cancellation),
        "timingWaiverAudit" => timing_waiver_audit(&request.params, cancellation),
        "rtlIncrementalMmmcTiming" => rtl_incremental_mmmc_timing(&request.params, cancellation),
        "rtlPnr" => rtl_pnr(&request.params, cancellation),
        "rtlAntennaRepair" => rtl_antenna_repair(&request.params, cancellation),
        "rtlPower" => rtl_power(&request.params, cancellation),
        "rtlIr" => rtl_ir(&request.params, cancellation),
        "rtlBmc" => rtl_bmc(&request.params, cancellation),
        "rtlDft" => rtl_dft(&request.params, cancellation),
        "rtlCdc" => rtl_cdc(&request.params, cancellation),
        "rtlFault" => rtl_fault(&request.params, cancellation),
        "rtlSim" => rtl_sim(&request.params, cancellation),
        "rtlLvs" => rtl_lvs(&request.params, cancellation),
        "rtlPowerIntent" => rtl_power_intent(&request.params, cancellation),
        "rtlSafety" => rtl_safety(&request.params, cancellation),
        "rtlPhysicalTiming" => rtl_physical_timing(&request.params, cancellation),
        "rtlPostRoutePowerIr" => rtl_post_route_power_ir(&request.params, cancellation),
        "rtlPostRoutePowerModeIr" => rtl_post_route_power_mode_ir(&request.params, cancellation),
        "pdkSignoff" => pdk_signoff(&request.params, cancellation),
        "spiceRun" => spice_run(&request.params, cancellation),
        "spiceDc" => spice_dc(&request.params, cancellation),
        _ => dispatch_result(request),
    }
}

fn optional_json_array<'a>(
    fields: &'a BTreeMap<String, Json>,
    name: &str,
) -> Result<Vec<&'a Json>, String> {
    match fields.get(name) {
        None | Some(Json::Null) => Ok(Vec::new()),
        Some(Json::Array(values)) => Ok(values.iter().collect()),
        Some(_) => Err(format!("'{name}' must be an array")),
    }
}

pub(crate) fn required_object_value(
    fields: &BTreeMap<String, Json>,
    name: &str,
) -> Result<Json, String> {
    fields
        .get(name)
        .cloned()
        .ok_or_else(|| format!("layout object requires '{name}'"))
}

pub(crate) fn required_object_string(
    fields: &BTreeMap<String, Json>,
    name: &str,
) -> Result<String, String> {
    fields
        .get(name)
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("layout field '{name}' must be a string"))
}

pub(crate) fn required_object_i64(
    fields: &BTreeMap<String, Json>,
    name: &str,
) -> Result<i64, String> {
    let value = fields
        .get(name)
        .and_then(Json::as_number)
        .ok_or_else(|| format!("layout field '{name}' must be an integer"))?;
    if value.fract() != 0.0 || !(i64::MIN as f64..=i64::MAX as f64).contains(value) {
        return Err(format!("layout field '{name}' must be an i64 integer"));
    }
    Ok(*value as i64)
}

pub(crate) fn remote_worker_json(worker: &RemoteWorkerSnapshot) -> Result<Json, String> {
    Ok(Json::object([
        ("workerId", Json::string(worker.worker_id.clone())),
        ("capacity", Json::from(worker.capacity as u64)),
        (
            "leaseTimeoutSeconds",
            Json::from(worker.lease_timeout_seconds),
        ),
        ("labels", string_map_json(&worker.labels)),
        (
            "activeJobIds",
            Json::Array(
                worker
                    .active_job_ids
                    .iter()
                    .map(|id| Json::from(*id))
                    .collect(),
            ),
        ),
        (
            "activeJobCount",
            Json::from(worker.active_job_ids.len() as u64),
        ),
        (
            "lastHeartbeatAgeSeconds",
            Json::from(worker.last_heartbeat_age_seconds),
        ),
    ]))
}

pub(crate) fn remote_job_artifact_metadata_json(artifact: &RemoteJobArtifactMetadata) -> Json {
    Json::object([
        ("jobId", Json::from(artifact.job_id)),
        ("name", Json::string(artifact.name.clone())),
        ("contentType", Json::string(artifact.content_type.clone())),
        (
            "contentEncoding",
            Json::string(artifact.content_encoding.clone()),
        ),
        ("sha256", Json::string(artifact.sha256.clone())),
        ("sizeBytes", Json::from(artifact.size_bytes as u64)),
    ])
}

pub(crate) fn remote_job_artifact_json(artifact: &RemoteJobArtifact) -> Result<Json, String> {
    Ok(Json::object([
        ("jobId", Json::number(artifact.job_id as f64)?),
        ("name", Json::string(artifact.name.clone())),
        ("contentType", Json::string(artifact.content_type.clone())),
        (
            "contentEncoding",
            Json::string(artifact.content_encoding.clone()),
        ),
        ("content", Json::string(artifact.content.clone())),
        ("sha256", Json::string(artifact.sha256.clone())),
        ("sizeBytes", Json::from(artifact.size_bytes as u64)),
    ]))
}

fn required_string(params: &Json, name: &str) -> Result<String, String> {
    params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?
        .get(name)
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("RPC parameter '{name}' must be a string"))
}

fn optional_top(params: &Json) -> Result<Option<&str>, String> {
    match params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?
        .get("top")
    {
        None | Some(Json::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| "'top' must be a string".to_string()),
    }
}

#[cfg(test)]
mod tests;
