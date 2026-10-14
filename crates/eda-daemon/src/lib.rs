//! Persistent JSON-RPC-style job service for long-running agent sessions.
//!
//! The protocol is deliberately line-delimited JSON so an agent can use a
//! socket without linking the Rust API. Jobs are also exposed as a native,
//! typed manager for in-process automation.

use eda_api::{
    active_timing_waivers_from_audit, add_timing_violation_registry_waiver_with_actor,
    analyze_physical_timing_power_ir, analyze_physical_timing_power_ir_from_waveform,
    analyze_physical_timing_power_ir_power_modes_from_waveform_with_upf,
    analyze_physical_timing_power_ir_power_modes_with_upf, analyze_rtl_cdc_cancellable,
    analyze_rtl_faults_cancellable, analyze_rtl_ir_cancellable, analyze_rtl_power_cancellable,
    analyze_rtl_timing_cancellable, analyze_signal_quality,
    analyze_timing_scenarios_from_definitions, any_angle_boolean, apply_eco, assertion_report,
    capability_report, cdc_report, check_assertions, check_netlists_equivalent_cancellable,
    check_rtl_equivalence_cancellable, check_rtl_properties_cancellable,
    clock_tree_synthesis_with_placement, coverage_report, cts_report, default_power_vectors,
    diff_timing_scenarios as diff_api_timing_scenarios, eco_report, export_verilog, fault_report,
    functional_coverage, implement_rtl_pnr_cancellable,
    implement_rtl_pnr_with_drc_options_cancellable, insert_rtl_scan_cancellable, ir_report,
    multi_corner_hold_timing_path_report, multi_corner_hold_timing_paths,
    multi_corner_reset_timing_path_report, multi_corner_reset_timing_paths,
    multi_corner_setup_timing_path_report, multi_corner_setup_timing_paths,
    multi_corner_timing_report, parse_assertions, parse_covergroups, parse_eco, parse_pdk,
    parse_power_intent, parse_spice, parse_timing_scenarios, parse_timing_violation_waivers,
    parse_timing_waiver_audit, parse_waveform, physical_timing_flow_report,
    post_route_power_ir_report, post_route_power_scenarios_from_timing_scenarios, power_report,
    proof_report, prove_safety_cancellable, read_layout_rule_deck,
    remove_timing_violation_registry_waiver_with_actor, repair_rtl_antennas_cancellable,
    report_ams_result, report_pdk_signoff, route_any_angle_direct,
    route_any_angle_maze_cancellable, route_any_angle_maze_ripgup_cancellable,
    run_all_ams_analyses_cancellable, run_ams_analysis_cancellable, run_flow_cancellable,
    run_flow_with_library_cancellable, run_pdk_signoff_cancellable,
    run_physical_timing_flow_cancellable, signal_quality_report, simulate_rtl_cancellable,
    timing_driven_clock_tree_synthesis_with_placement, timing_driven_cts_report,
    timing_scenario_diff_report, timing_violation_registry, timing_violation_registry_report,
    timing_waiver_audit_hierarchy_report, timing_waiver_audit_hierarchy_summaries,
    timing_waiver_audit_report, update_timing_scenarios_incrementally,
    update_timing_violation_registry, upf_power_mode_ir_report, write_delay_file, write_parasitics,
    write_timing_waiver_audit, AmsResult, AnyAngleBooleanOperation, AnyAngleMazeRoutingOptions,
    CellLibrary, Flow, FlowCacheKey, FlowCancellation, IrOptions, MultilayerPowerMeshOptions,
    PatternOptions, PhysicalTimingFlowOptions, PowerOptions, ProofStatus, PropertyBmcStatus,
    QualityKind, QualityOptions, RtlSimulationVector, ScanOptions, ScopedQualityLimit,
    Severity as CdcSeverity, SharedFlowCache, TimingDrivenCtsOptions, TimingObjectKind,
    TimingScenario, TimingWaiverAuditAction,
};
use eda_core::logic::LogicWord;
use eda_core::netlist::Netlist;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::{BufRead, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{spawn, JoinHandle};
use std::time::Duration;

pub const PROTOCOL_VERSION: usize = 1;
const MAX_RPC_LINE_BYTES: usize = 8 * 1024 * 1024;

/// Minimal, dependency-free JSON value used by the RPC wire protocol.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl From<i64> for Json {
    fn from(value: i64) -> Self {
        Self::Number(value as f64)
    }
}

impl From<i32> for Json {
    fn from(value: i32) -> Self {
        Self::Number(f64::from(value))
    }
}

impl From<u64> for Json {
    fn from(value: u64) -> Self {
        Self::Number(value as f64)
    }
}

impl From<&str> for Json {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl Json {
    pub fn object(entries: impl IntoIterator<Item = (impl Into<String>, Json)>) -> Self {
        Self::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    pub fn string(value: impl Into<String>) -> Self {
        Self::String(value.into())
    }

    pub fn number(value: f64) -> Result<Self, String> {
        if value.is_finite() {
            Ok(Self::Number(value))
        } else {
            Err(format!("JSON cannot encode non-finite number {value}"))
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&BTreeMap<String, Json>> {
        match self {
            Self::Object(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<&f64> {
        match self {
            Self::Number(value) => Some(value),
            _ => None,
        }
    }

    pub fn write(&self) -> String {
        let mut out = String::new();
        self.write_into(&mut out);
        out
    }

    fn write_into(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Number(value) => {
                if value.is_finite() {
                    if *value == value.trunc() && value.abs() < 1e15 {
                        let _ = write!(out, "{}", *value as i64);
                    } else {
                        let _ = write!(out, "{value}");
                    }
                } else {
                    out.push_str("null");
                }
            }
            Self::String(value) => write_json_string(value, out),
            Self::Array(values) => {
                out.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        out.push(',');
                    }
                    value.write_into(out);
                }
                out.push(']');
            }
            Self::Object(values) => {
                out.push('{');
                for (index, (key, value)) in values.iter().enumerate() {
                    if index != 0 {
                        out.push(',');
                    }
                    write_json_string(key, out);
                    out.push(':');
                    value.write_into(out);
                }
                out.push('}');
            }
        }
    }
}

pub fn parse_json(text: &str) -> Result<Json, String> {
    let mut parser = JsonParser {
        bytes: text.as_bytes(),
        position: 0,
        depth: 0,
    };
    parser.skip_whitespace();
    let value = parser.value()?;
    parser.skip_whitespace();
    if parser.position != parser.bytes.len() {
        return Err(format!(
            "unexpected trailing JSON data at byte {}",
            parser.position
        ));
    }
    Ok(value)
}

fn write_json_string(value: &str, out: &mut String) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            character if character.is_control() => {
                let code = character as u32;
                let _ = write!(out, "\\u{code:04x}");
            }
            character => out.push(character),
        }
    }
    out.push('"');
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    position: usize,
    depth: usize,
}

impl<'a> JsonParser<'a> {
    fn value(&mut self) -> Result<Json, String> {
        if self.depth >= 64 {
            return Err("JSON nesting exceeds 64 levels".into());
        }
        match self.peek()? {
            b'n' => {
                self.literal("null")?;
                Ok(Json::Null)
            }
            b't' => {
                self.literal("true")?;
                Ok(Json::Bool(true))
            }
            b'f' => {
                self.literal("false")?;
                Ok(Json::Bool(false))
            }
            b'"' => Ok(Json::String(self.string()?)),
            b'[' => {
                self.depth += 1;
                self.take()?;
                self.skip_whitespace();
                let mut values = Vec::new();
                if self.peek()? == b']' {
                    self.take()?;
                    self.depth -= 1;
                    return Ok(Json::Array(values));
                }
                loop {
                    self.skip_whitespace();
                    values.push(self.value()?);
                    self.skip_whitespace();
                    match self.take()? {
                        b',' => continue,
                        b']' => break,
                        byte => {
                            return Err(format!(
                                "expected ',' or ']' in JSON array, got '{}'",
                                byte as char
                            ))
                        }
                    }
                }
                self.depth -= 1;
                Ok(Json::Array(values))
            }
            b'{' => {
                self.depth += 1;
                self.take()?;
                let mut values = BTreeMap::new();
                self.skip_whitespace();
                if self.peek()? == b'}' {
                    self.take()?;
                    self.depth -= 1;
                    return Ok(Json::Object(values));
                }
                loop {
                    self.skip_whitespace();
                    let key = self.string()?;
                    self.skip_whitespace();
                    if self.take()? != b':' {
                        return Err("expected ':' after JSON object key".into());
                    }
                    self.skip_whitespace();
                    let value = self.value()?;
                    if values.insert(key, value).is_some() {
                        return Err("duplicate JSON object key".into());
                    }
                    self.skip_whitespace();
                    match self.take()? {
                        b',' => continue,
                        b'}' => break,
                        byte => {
                            return Err(format!(
                                "expected ',' or '}}' in JSON object, got '{}'",
                                byte as char
                            ))
                        }
                    }
                }
                self.depth -= 1;
                Ok(Json::Object(values))
            }
            byte if byte == b'-' || byte.is_ascii_digit() => self.number(),
            byte => Err(format!("unexpected JSON value byte '{}'", byte as char)),
        }
    }

    fn literal(&mut self, expected: &str) -> Result<(), String> {
        if self.bytes[self.position..].starts_with(expected.as_bytes()) {
            self.position += expected.len();
            Ok(())
        } else {
            Err(format!("invalid JSON literal, expected '{expected}'"))
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.position;
        if self.peek()? == b'-' {
            self.position += 1;
        }
        let digits_start = self.position;
        while self.position < self.bytes.len() && self.bytes[self.position].is_ascii_digit() {
            self.position += 1;
        }
        if self.position == digits_start {
            return Err("JSON number requires digits".into());
        }
        if self.position < self.bytes.len() && self.bytes[self.position] == b'.' {
            self.position += 1;
            let fraction_start = self.position;
            while self.position < self.bytes.len() && self.bytes[self.position].is_ascii_digit() {
                self.position += 1;
            }
            if self.position == fraction_start {
                return Err("JSON fraction requires digits".into());
            }
        }
        if self.position < self.bytes.len()
            && (self.bytes[self.position] == b'e' || self.bytes[self.position] == b'E')
        {
            self.position += 1;
            if self.position < self.bytes.len()
                && (self.bytes[self.position] == b'+' || self.bytes[self.position] == b'-')
            {
                self.position += 1;
            }
            let exponent_start = self.position;
            while self.position < self.bytes.len() && self.bytes[self.position].is_ascii_digit() {
                self.position += 1;
            }
            if self.position == exponent_start {
                return Err("JSON exponent requires digits".into());
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.position])
            .map_err(|_| "invalid UTF-8 in JSON number".to_string())?;
        let value: f64 = text
            .parse()
            .map_err(|_| format!("invalid JSON number '{text}'"))?;
        if !value.is_finite() {
            return Err(format!("non-finite JSON number '{text}'"));
        }
        Ok(Json::Number(value))
    }

    fn string(&mut self) -> Result<String, String> {
        if self.take()? != b'"' {
            return Err("expected JSON string".into());
        }
        let mut out = String::new();
        loop {
            let byte = self.take()?;
            match byte {
                b'"' => return Ok(out),
                b'\\' => {
                    let escaped = self.take()?;
                    match escaped {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape()?),
                        byte => {
                            return Err(format!("invalid JSON escape '\\{}'", byte as char));
                        }
                    }
                }
                byte if byte < 0x20 => {
                    return Err(format!(
                        "unescaped control character {byte:#04x} in JSON string"
                    ));
                }
                byte if byte < 0x80 => out.push(byte as char),
                _ => {
                    let start = self.position - 1;
                    let mut end = self.position;
                    while end < self.bytes.len() && (self.bytes[end] & 0xc0) == 0x80 {
                        end += 1;
                    }
                    let text = std::str::from_utf8(&self.bytes[start..end])
                        .map_err(|_| "invalid UTF-8 in JSON string".to_string())?;
                    out.push_str(text);
                    self.position = end;
                }
            }
        }
    }

    fn unicode_escape(&mut self) -> Result<char, String> {
        let first = self.hex4()?;
        let code = if (0xd800..=0xdbff).contains(&first) {
            if self.take()? != b'\\' || self.take()? != b'u' {
                return Err("high surrogate must be followed by \\uDC00-\\uDFFF".into());
            }
            let second = self.hex4()?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return Err("invalid low surrogate in JSON string".into());
            }
            0x10000 + ((first - 0xd800) << 10) + second - 0xdc00
        } else if (0xdc00..=0xdfff).contains(&first) {
            return Err("unexpected low surrogate in JSON string".into());
        } else {
            first
        };
        char::from_u32(code).ok_or_else(|| format!("invalid Unicode scalar value {code:#x}"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut value = 0_u32;
        for _ in 0..4 {
            let byte = self.take()?;
            let digit = match byte {
                b'0'..=b'9' => u32::from(byte - b'0'),
                b'a'..=b'f' => u32::from(byte - b'a' + 10),
                b'A'..=b'F' => u32::from(byte - b'A' + 10),
                byte => return Err(format!("invalid JSON hex digit '{}'", byte as char)),
            };
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn skip_whitespace(&mut self) {
        while self.position < self.bytes.len()
            && matches!(self.bytes[self.position], b' ' | b'\t' | b'\n' | b'\r')
        {
            self.position += 1;
        }
    }

    fn peek(&self) -> Result<u8, String> {
        self.bytes
            .get(self.position)
            .copied()
            .ok_or_else(|| "unexpected end of JSON input".to_string())
    }

    fn take(&mut self) -> Result<u8, String> {
        let byte = self.peek()?;
        self.position += 1;
        Ok(byte)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RpcRequest {
    pub id: Json,
    pub method: String,
    pub params: Json,
}

impl RpcRequest {
    pub fn new(id: impl Into<Json>, method: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            method: method.into(),
            params: Json::Object(BTreeMap::new()),
        }
    }

    pub fn with_params(mut self, params: Json) -> Self {
        self.params = params;
        self
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let Json::Object(fields) = parse_json(text)? else {
            return Err("RPC request must be a JSON object".into());
        };
        let id = fields
            .get("id")
            .cloned()
            .ok_or_else(|| "RPC request requires 'id'".to_string())?;
        if id == Json::Null {
            return Err("RPC request id cannot be null".into());
        }
        let method = fields
            .get("method")
            .and_then(Json::as_str)
            .map(str::to_string)
            .ok_or_else(|| "RPC request requires string 'method'".to_string())?;
        let params = fields
            .get("params")
            .cloned()
            .unwrap_or_else(|| Json::Object(BTreeMap::new()));
        if !matches!(params, Json::Object(_)) {
            return Err("RPC request params must be an object".into());
        }
        Ok(Self { id, method, params })
    }

    pub fn wire(&self) -> String {
        Json::object([
            ("id", self.id.clone()),
            ("method", Json::string(self.method.clone())),
            ("params", self.params.clone()),
        ])
        .write()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RpcResponse {
    pub id: Json,
    pub result: Result<Json, String>,
}

impl RpcResponse {
    pub fn wire(&self) -> String {
        match &self.result {
            Ok(result) => Json::object([
                ("id", self.id.clone()),
                ("ok", Json::Bool(true)),
                ("result", result.clone()),
            ]),
            Err(error) => Json::object([
                ("id", self.id.clone()),
                ("ok", Json::Bool(false)),
                ("error", Json::string(error.clone())),
            ]),
        }
        .write()
    }
}

pub fn dispatch(request: &RpcRequest) -> RpcResponse {
    RpcResponse {
        id: request.id.clone(),
        result: dispatch_result(request),
    }
}

const TIMING_WAIVER_REGISTRY_CAPACITY: usize = 128;
const LAYOUT_DRC_WAIVER_REGISTRY_CAPACITY: usize = 128;

#[derive(Clone)]
struct TimingViolationRegistryState {
    flow: Flow,
    definitions: Vec<eda_api::TimingScenarioDefinition>,
    registry: eda_api::TimingViolationRegistry,
}

type SharedTimingViolationRegistries = Arc<Mutex<BTreeMap<String, TimingViolationRegistryState>>>;

#[derive(Clone, Default)]
struct LayoutDrcWaiverRegistryState {
    waivers: BTreeMap<String, eda_api::LayoutDrcWaiver>,
    audit: Vec<LayoutDrcWaiverAuditEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LayoutDrcWaiverAuditEvent {
    sequence: usize,
    action: LayoutDrcWaiverAuditAction,
    waiver_id: String,
    actor: Option<String>,
    waiver: Option<eda_api::LayoutDrcWaiver>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayoutDrcWaiverAuditAction {
    Added,
    Removed,
}

impl LayoutDrcWaiverAuditAction {
    fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
        }
    }
}

type SharedLayoutDrcWaiverRegistries = Arc<Mutex<BTreeMap<String, LayoutDrcWaiverRegistryState>>>;

/// Stateful RPC front end shared by all TCP connections.
///
/// Direct requests remain synchronous. `jobSubmit`, `jobStatus`, `jobCancel`,
/// and `jobWait` expose the persistent worker queue to remote agents.
#[derive(Clone)]
pub struct RpcService {
    manager: Arc<Mutex<RpcJobManager>>,
    flow_cache: SharedFlowCache,
    timing_waiver_registries: SharedTimingViolationRegistries,
    layout_drc_waiver_registries: SharedLayoutDrcWaiverRegistries,
}

impl RpcService {
    pub fn new() -> Self {
        Self::with_flow_cache_capacity(128).expect("default RPC flow cache capacity")
    }

    pub fn with_flow_cache_capacity(capacity: usize) -> Result<Self, String> {
        let flow_cache = SharedFlowCache::new(capacity)?;
        let dispatcher_cache = flow_cache.clone();
        let timing_waiver_registries = Arc::new(Mutex::new(BTreeMap::new()));
        let dispatcher_registries = timing_waiver_registries.clone();
        let layout_drc_waiver_registries = Arc::new(Mutex::new(BTreeMap::new()));
        let dispatcher_layout_registries = layout_drc_waiver_registries.clone();
        let manager = RpcJobManager::new_with_dispatcher(Arc::new(move |request, cancellation| {
            dispatch_result_with_flow_cache(
                &request,
                &cancellation,
                &dispatcher_cache,
                &dispatcher_registries,
                &dispatcher_layout_registries,
            )
        }));
        Ok(Self {
            manager: Arc::new(Mutex::new(manager)),
            flow_cache,
            timing_waiver_registries,
            layout_drc_waiver_registries,
        })
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
            "jobStatus" => self.job_status(&request.params),
            "jobCancel" => self.job_cancel(&request.params),
            "jobWait" => self.job_wait(&request.params),
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

    fn shutdown(&self) -> Result<(), String> {
        self.manager()?.shutdown_background()
    }
}

impl Default for RpcService {
    fn default() -> Self {
        Self::new()
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

fn required_job_id(params: &Json) -> Result<u64, String> {
    match required_value(params, "jobId")? {
        Json::Number(value) if value >= 0.0 && value.fract() == 0.0 && value <= u64::MAX as f64 => {
            Ok(value as u64)
        }
        _ => Err("'jobId' must be a non-negative integer".into()),
    }
}

fn optional_positive_usize(params: &Json, name: &str, default: usize) -> Result<usize, String> {
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

fn job_status_json(id: u64, status: &RpcJobStatus) -> Result<Json, String> {
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

fn dispatch_result_with_flow_cache(
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

fn dispatch_result_cancellable(
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

fn functional_coverage_rpc(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("functional coverage analysis cancelled".into());
    }
    let waveform_source = required_string(params, "vcd")?;
    let plan_source = required_string(params, "coverage")?;
    let waveform = parse_waveform(&waveform_source)?;
    let plan = parse_covergroups(&plan_source)?;
    if cancellation.is_cancelled() {
        return Err("functional coverage analysis cancelled".into());
    }
    let result = functional_coverage(&waveform, &plan)?;
    if cancellation.is_cancelled() {
        return Err("functional coverage analysis cancelled".into());
    }
    let bins = result
        .bins
        .iter()
        .map(|bin| {
            Ok(Json::object([
                ("group", Json::string(bin.group.clone())),
                ("point", Json::string(bin.point.clone())),
                ("bin", Json::string(bin.bin.clone())),
                ("hits", Json::from(bin.hits)),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let crosses = result
        .crosses
        .iter()
        .map(|cross| {
            Ok(Json::object([
                ("group", Json::string(cross.group.clone())),
                ("cross", Json::string(cross.cross.clone())),
                (
                    "combination",
                    Json::Array(
                        cross
                            .combination
                            .iter()
                            .map(|(point, bin)| {
                                Json::object([
                                    ("point", Json::string(point.clone())),
                                    ("bin", Json::string(bin.clone())),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("hits", Json::from(cross.hits)),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let toggles = result
        .toggles
        .iter()
        .map(|toggle| {
            Ok(Json::object([
                ("name", Json::string(toggle.name.clone())),
                ("transitions", Json::from(toggle.transitions)),
                ("samples", Json::number(toggle.samples as f64)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("samples", Json::number(result.samples as f64)?),
        ("bins", Json::Array(bins)),
        ("crosses", Json::Array(crosses)),
        ("toggles", Json::Array(toggles)),
        ("percentage", Json::number(result.percentage)?),
        ("report", Json::string(coverage_report(&result))),
    ]))
}

fn layout_rule_deck_rpc(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout rule deck validation cancelled".into());
    }
    let source = required_string(params, "deck")?;
    let document = read_layout_rule_deck(&source)?;
    let normalized = eda_api::write_layout_rule_deck(&document)?;
    if cancellation.is_cancelled() {
        return Err("layout rule deck validation cancelled".into());
    }
    let layer_count = normalized
        .lines()
        .filter(|line| line.starts_with("layer "))
        .count();
    let spacing_count = normalized
        .lines()
        .filter(|line| line.starts_with("spacing "))
        .count();
    let via_count = normalized
        .lines()
        .filter(|line| line.starts_with("via "))
        .count();
    Ok(Json::object([
        ("valid", Json::Bool(true)),
        ("layerCount", Json::from(layer_count as u64)),
        ("crossLayerSpacingCount", Json::from(spacing_count as u64)),
        ("viaRuleCount", Json::from(via_count as u64)),
        ("normalized", Json::string(normalized)),
    ]))
}

fn parse_layout_drc_waiver(value: &Json) -> Result<eda_api::LayoutDrcWaiver, String> {
    let waiver = value
        .as_object()
        .ok_or_else(|| "each layout DRC waiver must be an object".to_string())?;
    eda_api::LayoutDrcWaiver::new(
        required_object_string(waiver, "id")?,
        required_object_string(waiver, "code")?,
        optional_object_string(waiver, "layer")?,
        optional_object_string(waiver, "net")?,
        optional_object_string(waiver, "object")?,
        required_object_string(waiver, "owner")?,
        required_object_string(waiver, "reason")?,
    )
}

fn layout_drc_waiver_json(waiver: &eda_api::LayoutDrcWaiver) -> Json {
    Json::object([
        ("id", Json::string(waiver.id.clone())),
        ("code", Json::string(waiver.code.clone())),
        ("layer", optional_string_json(&waiver.layer)),
        ("net", optional_string_json(&waiver.net)),
        ("object", optional_string_json(&waiver.object)),
        ("owner", Json::string(waiver.owner.clone())),
        ("reason", Json::string(waiver.reason.clone())),
    ])
}

fn layout_drc_rpc(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout DRC cancelled".into());
    }
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let mut layout = eda_api::LayoutDocument::new();
    let optional_nonnegative_i64 =
        |fields: &BTreeMap<String, Json>, name: &'static str| -> Result<Option<i64>, String> {
            match fields.get(name) {
                None | Some(Json::Null) => Ok(None),
                Some(Json::Number(value)) => {
                    if *value < 0.0 || value.fract() != 0.0 || *value > i64::MAX as f64 {
                        return Err(format!("'{name}' must be a nonnegative i64"));
                    }
                    Ok(Some(*value as i64))
                }
                Some(_) => Err(format!("'{name}' must be a nonnegative i64")),
            }
        };
    for value in optional_json_array(fields, "layerRules")? {
        let rule = value
            .as_object()
            .ok_or_else(|| "each layer rule must be an object".to_string())?;
        let layer_rule = eda_api::LayoutLayerRule::new(
            required_object_string(rule, "layer")?,
            required_object_i64(rule, "minWidth")?,
            required_object_i64(rule, "minSpacing")?,
            required_object_i64(rule, "minArea")?,
        )?;
        let density_fields = [
            ("minDensityBp", rule.get("minDensityBp")),
            ("maxDensityBp", rule.get("maxDensityBp")),
        ];
        let density_present = density_fields
            .iter()
            .any(|(_, value)| matches!(value, Some(Json::Number(_))));
        let layer_rule = if density_present {
            let parse_density = |name: &str, value: Option<&Json>| -> Result<Option<u32>, String> {
                match value {
                    None | Some(Json::Null) => Ok(None),
                    Some(Json::Number(value)) => {
                        if *value < 0.0 || value.fract() != 0.0 || *value > u32::MAX as f64 {
                            return Err(format!("'{name}' must be a u32 basis-point value"));
                        }
                        Ok(Some(*value as u32))
                    }
                    Some(_) => Err(format!("'{name}' must be a u32 basis-point value")),
                }
            };
            layer_rule.with_density(
                parse_density("minDensityBp", rule.get("minDensityBp"))?,
                parse_density("maxDensityBp", rule.get("maxDensityBp"))?,
                required_object_i64(rule, "densityWindow")?,
            )?
        } else {
            layer_rule
        };
        let layer_rule = match rule.get("minNotch") {
            None | Some(Json::Null) => layer_rule,
            Some(Json::Number(value)) => {
                if *value < 0.0 || value.fract() != 0.0 || *value > i64::MAX as f64 {
                    return Err("'minNotch' must be a nonnegative i64".into());
                }
                layer_rule.with_notch(*value as i64)?
            }
            Some(_) => return Err("'minNotch' must be a nonnegative i64".into()),
        };
        let layer_rule = match rule.get("spacingTo") {
            None | Some(Json::Null) => layer_rule,
            Some(Json::Object(spacing_rules)) => {
                let mut layered_rule = layer_rule;
                for (other_layer, spacing) in spacing_rules {
                    let Json::Number(spacing) = spacing else {
                        return Err(format!(
                            "cross-layer spacing for '{other_layer}' must be a nonnegative i64"
                        ));
                    };
                    if *spacing < 0.0 || spacing.fract() != 0.0 || *spacing > i64::MAX as f64 {
                        return Err(format!(
                            "cross-layer spacing for '{other_layer}' must be a nonnegative i64"
                        ));
                    }
                    layered_rule =
                        layered_rule.with_spacing_to(other_layer.clone(), *spacing as i64)?;
                }
                layered_rule
            }
            Some(_) => return Err("'spacingTo' must be an object".into()),
        };
        layout.set_layer_rule(layer_rule)?;
    }
    for value in optional_json_array(fields, "viaRules")? {
        let rule = value
            .as_object()
            .ok_or_else(|| "each via rule must be an object".to_string())?;
        let via_rule = eda_api::LayoutViaRule::new(
            required_object_string(rule, "name")?,
            required_object_string(rule, "cutLayer")?,
            required_object_string(rule, "bottomLayer")?,
            required_object_string(rule, "topLayer")?,
            required_object_i64(rule, "cutWidth")?,
            required_object_i64(rule, "cutHeight")?,
            required_object_i64(rule, "minBottomEnclosure")?,
            required_object_i64(rule, "minTopEnclosure")?,
        )?;
        let via_rule = match rule.get("minCutSpacing") {
            None | Some(Json::Null) => via_rule,
            Some(Json::Number(value)) => {
                if *value < 0.0 || value.fract() != 0.0 || *value > i64::MAX as f64 {
                    return Err("'minCutSpacing' must be a nonnegative i64".into());
                }
                via_rule.with_cut_spacing(*value as i64)?
            }
            Some(_) => return Err("'minCutSpacing' must be a nonnegative i64".into()),
        };
        let extension_present = [
            "minBottomExtensionX",
            "minBottomExtensionY",
            "minTopExtensionX",
            "minTopExtensionY",
        ]
        .into_iter()
        .any(|name| rule.contains_key(name));
        let via_rule = if extension_present {
            via_rule.with_extensions(
                optional_nonnegative_i64(rule, "minBottomExtensionX")?.unwrap_or(0),
                optional_nonnegative_i64(rule, "minBottomExtensionY")?.unwrap_or(0),
                optional_nonnegative_i64(rule, "minTopExtensionX")?.unwrap_or(0),
                optional_nonnegative_i64(rule, "minTopExtensionY")?.unwrap_or(0),
            )?
        } else {
            via_rule
        };
        layout.set_via_rule(via_rule)?;
    }
    for value in optional_json_array(fields, "shapes")? {
        let shape = value
            .as_object()
            .ok_or_else(|| "each layout shape must be an object".to_string())?;
        let geometry = if let Some(rect) = shape.get("rect") {
            eda_api::LayoutGeometry::Rect(parse_rect(rect)?)
        } else if let Some(polygon) = shape.get("polygon") {
            eda_api::LayoutGeometry::Polygon(eda_api::LayoutPolygon::new(parse_points(polygon)?)?)
        } else if let Some(polygon) = shape.get("anyAnglePolygon") {
            eda_api::LayoutGeometry::AnyAngle(eda_api::AnyAnglePolygon::new(parse_points(
                polygon,
            )?)?)
        } else {
            return Err("each layout shape needs 'rect', 'polygon', or 'anyAnglePolygon'".into());
        };
        layout.add_shape(eda_api::LayoutShape {
            id: required_object_string(shape, "id")?,
            layer: required_object_string(shape, "layer")?,
            net: required_object_string(shape, "net")?,
            geometry,
        })?;
    }
    for value in optional_json_array(fields, "vias")? {
        let via = value
            .as_object()
            .ok_or_else(|| "each layout via must be an object".to_string())?;
        layout.add_via(eda_api::LayoutVia {
            id: required_object_string(via, "id")?,
            rule: required_object_string(via, "rule")?,
            net: required_object_string(via, "net")?,
            cut_layer: required_object_string(via, "cutLayer")?,
            bottom_layer: required_object_string(via, "bottomLayer")?,
            top_layer: required_object_string(via, "topLayer")?,
            cut: parse_rect(&required_object_value(via, "cut")?)?,
        })?;
    }
    let waivers = optional_json_array(fields, "waivers")?
        .into_iter()
        .map(parse_layout_drc_waiver)
        .collect::<Result<Vec<_>, String>>()?;
    if cancellation.is_cancelled() {
        return Err("layout DRC cancelled".into());
    }
    let raw_result = layout.check_layout_drc();
    let raw_violation_count = raw_result.violations.len();
    let raw_error_count = raw_result.error_count();
    let waiver_result = if waivers.is_empty() {
        None
    } else {
        Some(eda_api::apply_layout_drc_waivers(&raw_result, &waivers)?)
    };
    let result = waiver_result
        .as_ref()
        .map(|waived| waived.active.clone())
        .unwrap_or(raw_result);
    let waived = waiver_result
        .as_ref()
        .map(|result| result.waived.clone())
        .unwrap_or_default();
    let used_waiver_ids = waiver_result
        .as_ref()
        .map(|result| result.used_waiver_ids.clone())
        .unwrap_or_default();
    let waiver_report = waiver_result
        .as_ref()
        .map(eda_api::layout_drc_waiver_report)
        .unwrap_or_default();
    let violations = result
        .violations
        .iter()
        .map(|violation| {
            Ok(Json::object([
                ("code", Json::string(violation.code.clone())),
                ("severity", Json::string(drc_severity(&violation.severity))),
                ("message", Json::string(violation.message.clone())),
                ("layer", optional_string_json(&violation.layer)),
                ("net", optional_string_json(&violation.net)),
                ("object", optional_string_json(&violation.object)),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("clean", Json::Bool(result.is_clean())),
        ("errorCount", Json::from(result.error_count() as u64)),
        ("violationCount", Json::from(result.violations.len() as u64)),
        ("violations", Json::Array(violations)),
        (
            "preWaiverViolationCount",
            Json::from(raw_violation_count as u64),
        ),
        ("preWaiverErrorCount", Json::from(raw_error_count as u64)),
        ("waiverCount", Json::from(waivers.len() as u64)),
        ("waivedViolationCount", Json::from(waived.len() as u64)),
        (
            "waivedViolations",
            Json::Array(
                waived
                    .iter()
                    .map(|waived| {
                        Ok(Json::object([
                            ("waiverId", Json::string(waived.waiver_id.clone())),
                            ("owner", Json::string(waived.owner.clone())),
                            ("reason", Json::string(waived.reason.clone())),
                            (
                                "violation",
                                Json::object([
                                    ("code", Json::string(waived.violation.code.clone())),
                                    ("message", Json::string(waived.violation.message.clone())),
                                    ("layer", optional_string_json(&waived.violation.layer)),
                                    ("net", optional_string_json(&waived.violation.net)),
                                    ("object", optional_string_json(&waived.violation.object)),
                                ]),
                            ),
                        ]))
                    })
                    .collect::<Result<Vec<_>, String>>()?,
            ),
        ),
        (
            "usedWaiverIds",
            Json::Array(
                used_waiver_ids
                    .iter()
                    .map(|id| Json::string(id.clone()))
                    .collect(),
            ),
        ),
        ("waiverReport", Json::string(waiver_report)),
    ]))
}

fn layout_drc_waiver_audit_report(
    registry_id: &str,
    state: &LayoutDrcWaiverRegistryState,
) -> String {
    let mut out = format!(
        "Layout DRC waiver registry '{}': {} active, {} event(s)\n",
        registry_id,
        state.waivers.len(),
        state.audit.len()
    );
    for event in &state.audit {
        out.push_str(&format!(
            "  #{} {} '{}' by {}\n",
            event.sequence,
            event.action.label(),
            event.waiver_id,
            event.actor.as_deref().unwrap_or("<unknown>")
        ));
    }
    out
}

fn layout_drc_waiver_registry_response(
    registry_id: &str,
    state: &LayoutDrcWaiverRegistryState,
) -> Result<Json, String> {
    let audit = state
        .audit
        .iter()
        .map(|event| {
            Ok(Json::object([
                ("sequence", Json::from(event.sequence as u64)),
                ("action", Json::string(event.action.label().to_string())),
                ("waiverId", Json::string(event.waiver_id.clone())),
                (
                    "actor",
                    event.actor.clone().map_or(Json::Null, Json::string),
                ),
                (
                    "waiver",
                    event
                        .waiver
                        .as_ref()
                        .map_or(Json::Null, layout_drc_waiver_json),
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let waivers = state
        .waivers
        .values()
        .map(layout_drc_waiver_json)
        .collect::<Vec<_>>();
    Ok(Json::object([
        ("registryId", Json::string(registry_id)),
        ("activeWaiverCount", Json::from(waivers.len() as u64)),
        ("waivers", Json::Array(waivers)),
        ("auditEventCount", Json::from(state.audit.len() as u64)),
        ("audit", Json::Array(audit)),
        (
            "report",
            Json::string(layout_drc_waiver_audit_report(registry_id, state)),
        ),
    ]))
}

fn layout_drc_waiver_registry_create(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedLayoutDrcWaiverRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout DRC waiver registry creation cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    if registry_id.trim().is_empty() {
        return Err("layout DRC waiver registry ID cannot be empty".into());
    }
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let waivers = optional_json_array(fields, "waivers")?
        .into_iter()
        .map(parse_layout_drc_waiver)
        .collect::<Result<Vec<_>, String>>()?;
    let actor = optional_object_string(fields, "actor")?;
    let mut state = LayoutDrcWaiverRegistryState::default();
    for waiver in waivers {
        if state
            .waivers
            .insert(waiver.id.clone(), waiver.clone())
            .is_some()
        {
            return Err(format!(
                "duplicate initial layout DRC waiver '{}'",
                waiver.id
            ));
        }
        state.audit.push(LayoutDrcWaiverAuditEvent {
            sequence: state.audit.len() + 1,
            action: LayoutDrcWaiverAuditAction::Added,
            waiver_id: waiver.id.clone(),
            actor: actor.clone(),
            waiver: Some(waiver),
        });
    }
    let mut entries = registries
        .lock()
        .map_err(|_| "layout DRC waiver registry mutex poisoned".to_string())?;
    if entries.contains_key(&registry_id) {
        return Err(format!(
            "layout DRC waiver registry '{registry_id}' already exists"
        ));
    }
    if entries.len() >= LAYOUT_DRC_WAIVER_REGISTRY_CAPACITY {
        return Err(format!(
            "layout DRC waiver registry store is full ({LAYOUT_DRC_WAIVER_REGISTRY_CAPACITY} entries)"
        ));
    }
    entries.insert(registry_id.clone(), state);
    let state = entries
        .get(&registry_id)
        .ok_or_else(|| "internal error: layout DRC waiver registry vanished".to_string())?;
    layout_drc_waiver_registry_response(&registry_id, state)
}

fn layout_drc_waiver_registry_get(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedLayoutDrcWaiverRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout DRC waiver registry lookup cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let entries = registries
        .lock()
        .map_err(|_| "layout DRC waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get(&registry_id)
        .ok_or_else(|| format!("layout DRC waiver registry '{registry_id}' does not exist"))?;
    layout_drc_waiver_registry_response(&registry_id, state)
}

fn layout_drc_waiver_registry_list(
    cancellation: &FlowCancellation,
    registries: &SharedLayoutDrcWaiverRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout DRC waiver registry listing cancelled".into());
    }
    let entries = registries
        .lock()
        .map_err(|_| "layout DRC waiver registry mutex poisoned".to_string())?;
    let summaries = entries
        .iter()
        .map(|(id, state)| {
            Json::object([
                ("registryId", Json::string(id.clone())),
                ("activeWaiverCount", Json::from(state.waivers.len() as u64)),
                ("auditEventCount", Json::from(state.audit.len() as u64)),
            ])
        })
        .collect::<Vec<_>>();
    Ok(Json::object([
        ("count", Json::from(entries.len() as u64)),
        (
            "capacity",
            Json::from(LAYOUT_DRC_WAIVER_REGISTRY_CAPACITY as u64),
        ),
        ("registries", Json::Array(summaries)),
    ]))
}

fn layout_drc_waiver_registry_add_waiver(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedLayoutDrcWaiverRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout DRC waiver registry update cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let waiver = parse_layout_drc_waiver(&required_value(params, "waiver")?)?;
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let actor = optional_object_string(fields, "actor")?;
    let mut entries = registries
        .lock()
        .map_err(|_| "layout DRC waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get_mut(&registry_id)
        .ok_or_else(|| format!("layout DRC waiver registry '{registry_id}' does not exist"))?;
    if state.waivers.contains_key(&waiver.id) {
        return Err(format!("layout DRC waiver '{}' already exists", waiver.id));
    }
    state.audit.push(LayoutDrcWaiverAuditEvent {
        sequence: state.audit.len() + 1,
        action: LayoutDrcWaiverAuditAction::Added,
        waiver_id: waiver.id.clone(),
        actor,
        waiver: Some(waiver.clone()),
    });
    let waiver_id = waiver.id.clone();
    state.waivers.insert(waiver_id, waiver);
    let state = entries
        .get(&registry_id)
        .ok_or_else(|| "internal error: layout DRC waiver registry vanished".to_string())?;
    layout_drc_waiver_registry_response(&registry_id, state)
}

fn layout_drc_waiver_registry_remove_waiver(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedLayoutDrcWaiverRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout DRC waiver registry update cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let waiver_id = required_string(params, "waiverId")?;
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let actor = optional_object_string(fields, "actor")?;
    let mut entries = registries
        .lock()
        .map_err(|_| "layout DRC waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get_mut(&registry_id)
        .ok_or_else(|| format!("layout DRC waiver registry '{registry_id}' does not exist"))?;
    let Some(waiver) = state.waivers.remove(&waiver_id) else {
        return Err(format!("layout DRC waiver '{waiver_id}' does not exist"));
    };
    state.audit.push(LayoutDrcWaiverAuditEvent {
        sequence: state.audit.len() + 1,
        action: LayoutDrcWaiverAuditAction::Removed,
        waiver_id,
        actor,
        waiver: Some(waiver),
    });
    let state = entries
        .get(&registry_id)
        .ok_or_else(|| "internal error: layout DRC waiver registry vanished".to_string())?;
    layout_drc_waiver_registry_response(&registry_id, state)
}

fn layout_drc_waiver_registry_delete(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedLayoutDrcWaiverRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout DRC waiver registry deletion cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let mut entries = registries
        .lock()
        .map_err(|_| "layout DRC waiver registry mutex poisoned".to_string())?;
    entries
        .remove(&registry_id)
        .ok_or_else(|| format!("layout DRC waiver registry '{registry_id}' does not exist"))?;
    Ok(Json::object([
        ("registryId", Json::string(registry_id)),
        ("deleted", Json::Bool(true)),
        ("remaining", Json::from(entries.len() as u64)),
    ]))
}

fn layout_drc_rpc_with_registry(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedLayoutDrcWaiverRegistries,
) -> Result<Json, String> {
    let Some(fields) = params.as_object() else {
        return layout_drc_rpc(params, cancellation);
    };
    let Some(registry_id) = fields.get("waiverRegistryId") else {
        return layout_drc_rpc(params, cancellation);
    };
    let Json::String(registry_id) = registry_id else {
        return Err("'waiverRegistryId' must be a string".into());
    };
    if fields.contains_key("waivers") {
        return Err("'waivers' and 'waiverRegistryId' are mutually exclusive".into());
    }
    let entries = registries
        .lock()
        .map_err(|_| "layout DRC waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get(registry_id)
        .ok_or_else(|| format!("layout DRC waiver registry '{registry_id}' does not exist"))?;
    let waivers = Json::Array(state.waivers.values().map(layout_drc_waiver_json).collect());
    drop(entries);
    let mut params = params.clone();
    let Json::Object(fields) = &mut params else {
        return Err("params must be an object".into());
    };
    fields.insert("waivers".into(), waivers);
    layout_drc_rpc(&params, cancellation)
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

fn required_object_value(fields: &BTreeMap<String, Json>, name: &str) -> Result<Json, String> {
    fields
        .get(name)
        .cloned()
        .ok_or_else(|| format!("layout object requires '{name}'"))
}

fn required_object_string(fields: &BTreeMap<String, Json>, name: &str) -> Result<String, String> {
    fields
        .get(name)
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("layout field '{name}' must be a string"))
}

fn required_object_i64(fields: &BTreeMap<String, Json>, name: &str) -> Result<i64, String> {
    let value = fields
        .get(name)
        .and_then(Json::as_number)
        .ok_or_else(|| format!("layout field '{name}' must be an integer"))?;
    if value.fract() != 0.0 || !(i64::MIN as f64..=i64::MAX as f64).contains(value) {
        return Err(format!("layout field '{name}' must be an i64 integer"));
    }
    Ok(*value as i64)
}

fn parse_rect(value: &Json) -> Result<eda_api::LayoutRect, String> {
    let points = parse_points(value)?;
    if points.len() != 2 {
        return Err("layout rectangle must contain two [x,y] points".into());
    }
    eda_api::LayoutRect::new(points[0].x, points[0].y, points[1].x, points[1].y)
}

fn parse_points(value: &Json) -> Result<Vec<eda_api::LayoutPoint>, String> {
    let Json::Array(values) = value else {
        return Err("layout geometry must be an array".into());
    };
    values
        .iter()
        .map(|point| {
            let Json::Array(coordinates) = point else {
                return Err("layout point must be [x,y]".into());
            };
            if coordinates.len() != 2 {
                return Err("layout point must contain exactly x and y".into());
            }
            Ok(eda_api::LayoutPoint::new(
                required_json_i64(&coordinates[0])?,
                required_json_i64(&coordinates[1])?,
            ))
        })
        .collect()
}

fn required_json_i64(value: &Json) -> Result<i64, String> {
    let number = value
        .as_number()
        .ok_or_else(|| "layout coordinate must be an integer".to_string())?;
    if number.fract() != 0.0 || !(i64::MIN as f64..=i64::MAX as f64).contains(number) {
        return Err("layout coordinate is outside i64 range".into());
    }
    Ok(*number as i64)
}

fn optional_object_string(
    fields: &BTreeMap<String, Json>,
    name: &str,
) -> Result<Option<String>, String> {
    match fields.get(name) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("layout field '{name}' must be a string or null")),
    }
}

fn parse_any_angle_polygon(value: &Json) -> Result<eda_api::AnyAnglePolygon, String> {
    let fields = value
        .as_object()
        .ok_or_else(|| "any-angle blockage must be an object".to_string())?;
    let points = required_object_value(fields, "points")?;
    let points = parse_points(&points)?;
    eda_api::AnyAnglePolygon::new(points)
}

fn parse_any_angle_route_request(value: &Json) -> Result<eda_api::AnyAngleRouteRequest, String> {
    let fields = value
        .as_object()
        .ok_or_else(|| "any-angle route request must be an object".to_string())?;
    let net = required_object_i64(fields, "net")?;
    if net < 0 {
        return Err("any-angle route net must be a nonnegative usize".into());
    }
    let start_value = required_object_value(fields, "start")?;
    let end_value = required_object_value(fields, "end")?;
    let Json::Array(start) = start_value else {
        return Err("any-angle route start must be [x,y]".into());
    };
    let Json::Array(end) = end_value else {
        return Err("any-angle route end must be [x,y]".into());
    };
    if start.len() != 2 || end.len() != 2 {
        return Err("any-angle route endpoints must each contain exactly x and y".into());
    }
    eda_api::AnyAngleRouteRequest::new(
        net as usize,
        eda_api::LayoutPoint::new(required_json_i64(&start[0])?, required_json_i64(&start[1])?),
        eda_api::LayoutPoint::new(required_json_i64(&end[0])?, required_json_i64(&end[1])?),
    )
}

fn parse_any_angle_routing_options(
    params: &Json,
) -> Result<eda_api::AnyAngleRoutingOptions, String> {
    let Some(value) = params.as_object().and_then(|fields| fields.get("options")) else {
        return Ok(eda_api::AnyAngleRoutingOptions::default());
    };
    let fields = value
        .as_object()
        .ok_or_else(|| "any-angle routing options must be an object".to_string())?;
    let min_spacing = required_object_i64(fields, "minSpacing")?;
    if min_spacing < 0 {
        return Err("any-angle routing spacing cannot be negative".into());
    }
    Ok(eda_api::AnyAngleRoutingOptions { min_spacing })
}

fn parse_any_angle_maze_routing_options(
    params: &Json,
) -> Result<AnyAngleMazeRoutingOptions, String> {
    let Some(value) = params.as_object().and_then(|fields| fields.get("options")) else {
        return Ok(AnyAngleMazeRoutingOptions::default());
    };
    let fields = value
        .as_object()
        .ok_or_else(|| "any-angle maze routing options must be an object".to_string())?;
    let min_spacing = required_object_i64(fields, "minSpacing")?;
    if min_spacing < 0 {
        return Err("any-angle maze routing spacing cannot be negative".into());
    }
    let max_search_nodes = required_object_i64(fields, "maxSearchNodes")?;
    if max_search_nodes < 0 || u128::from(max_search_nodes.unsigned_abs()) > usize::MAX as u128 {
        return Err("'maxSearchNodes' must be a nonnegative usize".into());
    }
    Ok(AnyAngleMazeRoutingOptions {
        min_spacing,
        max_search_nodes: max_search_nodes as usize,
    })
}

fn layout_any_angle_route_rpc(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("any-angle routing cancelled".into());
    }
    let requests_value = required_value(params, "requests")?;
    let Json::Array(request_values) = requests_value else {
        return Err("'requests' must be an array".into());
    };
    if request_values.is_empty() {
        return Err("any-angle routing requires at least one request".into());
    }
    let requests = request_values
        .iter()
        .map(parse_any_angle_route_request)
        .collect::<Result<Vec<_>, String>>()?;
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let blockage_values = optional_json_array(fields, "blockages")?;
    let blockages = blockage_values
        .into_iter()
        .map(parse_any_angle_polygon)
        .collect::<Result<Vec<_>, String>>()?;
    let options = parse_any_angle_routing_options(params)?;
    if cancellation.is_cancelled() {
        return Err("any-angle routing cancelled".into());
    }
    let routing = route_any_angle_direct(&requests, &blockages, &options)?;
    if cancellation.is_cancelled() {
        return Err("any-angle routing cancelled".into());
    }

    let point_json = |point: eda_api::LayoutPoint| {
        Json::object([("x", Json::from(point.x)), ("y", Json::from(point.y))])
    };
    let routes = routing
        .routes
        .iter()
        .map(|route| {
            Ok(Json::object([
                ("net", Json::from(route.net as u64)),
                ("start", point_json(route.start)),
                ("end", point_json(route.end)),
                (
                    "lengthSquaredNumerator",
                    Json::string(route.length_squared.numerator.to_string()),
                ),
                (
                    "lengthSquaredDenominator",
                    Json::string(route.length_squared.denominator.to_string()),
                ),
                ("length", Json::number(route.length())?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("routeCount", Json::from(routing.routes.len() as u64)),
        ("blockageCount", Json::from(blockages.len() as u64)),
        ("minSpacing", Json::from(options.min_spacing as u64)),
        ("routes", Json::Array(routes)),
        ("totalLength", Json::number(routing.total_length())?),
    ]))
}

fn layout_any_angle_maze_route_rpc(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("any-angle maze routing cancelled".into());
    }
    let requests_value = required_value(params, "requests")?;
    let Json::Array(request_values) = requests_value else {
        return Err("'requests' must be an array".into());
    };
    if request_values.is_empty() {
        return Err("any-angle maze routing requires at least one request".into());
    }
    let requests = request_values
        .iter()
        .map(parse_any_angle_route_request)
        .collect::<Result<Vec<_>, String>>()?;
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let blockage_values = optional_json_array(fields, "blockages")?;
    let blockages = blockage_values
        .into_iter()
        .map(parse_any_angle_polygon)
        .collect::<Result<Vec<_>, String>>()?;
    let options = parse_any_angle_maze_routing_options(params)?;
    let ripgup = match fields.get("ripgup") {
        None | Some(Json::Null) => false,
        Some(Json::Bool(value)) => *value,
        Some(_) => return Err("'ripgup' must be a boolean".into()),
    };
    let max_passes = if ripgup {
        let value = required_object_i64(fields, "maxPasses")?;
        if value < 0 || u128::from(value.unsigned_abs()) > usize::MAX as u128 {
            return Err("'maxPasses' must be a nonnegative usize".into());
        }
        value as usize
    } else {
        0
    };
    if cancellation.is_cancelled() {
        return Err("any-angle maze routing cancelled".into());
    }
    let (routing, passes, initial_conflicts) = if ripgup {
        let result = route_any_angle_maze_ripgup_cancellable(
            &requests,
            &blockages,
            &options,
            max_passes,
            cancellation,
        )?;
        let conflicts = result
            .initial_conflicts
            .iter()
            .map(|conflict| {
                Ok(Json::object([
                    ("leftNet", Json::from(conflict.left_net as u64)),
                    ("rightNet", Json::from(conflict.right_net as u64)),
                    ("leftSegment", Json::from(conflict.left_segment as u64)),
                    ("rightSegment", Json::from(conflict.right_segment as u64)),
                ]))
            })
            .collect::<Result<Vec<_>, String>>()?;
        (result.routing, result.passes, Some(conflicts))
    } else {
        (
            route_any_angle_maze_cancellable(&requests, &blockages, &options, cancellation)?,
            0,
            None,
        )
    };
    if cancellation.is_cancelled() {
        return Err("any-angle maze routing cancelled".into());
    }

    let point_json = |point: eda_api::LayoutPoint| {
        Json::object([("x", Json::from(point.x)), ("y", Json::from(point.y))])
    };
    let routes = routing
        .routes
        .iter()
        .map(|route| {
            let lengths = route
                .segment_lengths_squared
                .iter()
                .map(|length| {
                    Ok(Json::object([
                        ("numerator", Json::string(length.numerator.to_string())),
                        ("denominator", Json::string(length.denominator.to_string())),
                        ("value", Json::number(length.sqrt())?),
                    ]))
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(Json::object([
                ("net", Json::from(route.net as u64)),
                (
                    "points",
                    Json::Array(
                        route
                            .points
                            .iter()
                            .map(|point| point_json(*point))
                            .collect(),
                    ),
                ),
                ("segmentCount", Json::from(route.segment_count() as u64)),
                ("segmentLengthsSquared", Json::Array(lengths)),
                ("length", Json::number(route.total_length())?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut response = vec![
        ("routeCount", Json::from(routing.routes.len() as u64)),
        ("blockageCount", Json::from(blockages.len() as u64)),
        ("minSpacing", Json::from(options.min_spacing as u64)),
        (
            "maxSearchNodes",
            Json::from(options.max_search_nodes as u64),
        ),
        ("routes", Json::Array(routes)),
        ("totalLength", Json::number(routing.total_length())?),
    ];
    if ripgup {
        response.push(("ripgup", Json::Bool(true)));
        response.push(("passes", Json::from(passes as u64)));
        response.push((
            "initialConflicts",
            Json::Array(initial_conflicts.unwrap_or_default()),
        ));
    }
    Ok(Json::Object(
        response
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    ))
}

fn any_angle_rational_point_json(point: eda_api::AnyAngleRationalPoint) -> Result<Json, String> {
    Ok(Json::object([
        ("xNumerator", Json::string(point.x_numerator.to_string())),
        (
            "xDenominator",
            Json::string(point.x_denominator.to_string()),
        ),
        ("yNumerator", Json::string(point.y_numerator.to_string())),
        (
            "yDenominator",
            Json::string(point.y_denominator.to_string()),
        ),
        ("x", Json::number(point.x())?),
        ("y", Json::number(point.y())?),
    ]))
}

fn layout_any_angle_boolean_rpc(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("any-angle Boolean operation cancelled".into());
    }
    let left_value = required_value(params, "left")?;
    let right_value = required_value(params, "right")?;
    let left = parse_any_angle_polygon(&left_value)?;
    let right = parse_any_angle_polygon(&right_value)?;
    let operation = match required_string(params, "operation")?.as_str() {
        "union" => AnyAngleBooleanOperation::Union,
        "intersection" => AnyAngleBooleanOperation::Intersection,
        "difference" => AnyAngleBooleanOperation::Difference,
        other => {
            return Err(format!(
                "any-angle Boolean operation must be union, intersection, or difference, found '{other}'"
            ))
        }
    };
    if cancellation.is_cancelled() {
        return Err("any-angle Boolean operation cancelled".into());
    }
    let result = any_angle_boolean(&left, &right, operation)?;
    if cancellation.is_cancelled() {
        return Err("any-angle Boolean operation cancelled".into());
    }
    let polygons = result
        .polygons
        .iter()
        .map(|polygon| {
            Ok(Json::object([
                (
                    "points",
                    Json::Array(
                        polygon
                            .points()
                            .iter()
                            .map(|point| any_angle_rational_point_json(*point))
                            .collect::<Result<Vec<_>, String>>()?,
                    ),
                ),
                ("area", Json::number(polygon.area())?),
                ("signedArea", Json::number(polygon.signed_area())?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        (
            "operation",
            Json::string(match operation {
                AnyAngleBooleanOperation::Union => "union",
                AnyAngleBooleanOperation::Intersection => "intersection",
                AnyAngleBooleanOperation::Difference => "difference",
            }),
        ),
        ("polygonCount", Json::from(result.polygons.len() as u64)),
        ("polygons", Json::Array(polygons)),
        ("totalArea", Json::number(result.total_area)?),
    ]))
}

fn optional_string_json(value: &Option<String>) -> Json {
    match value {
        Some(value) => Json::string(value.clone()),
        None => Json::Null,
    }
}

fn drc_severity(severity: &eda_api::DrcSeverity) -> &'static str {
    match severity {
        eda_api::DrcSeverity::Warning => "warning",
        eda_api::DrcSeverity::Error => "error",
    }
}

fn assertions_rpc(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("assertion analysis cancelled".into());
    }
    let waveform_source = required_string(params, "vcd")?;
    let assertion_source = required_string(params, "sva")?;
    let waveform = parse_waveform(&waveform_source)?;
    let assertions = parse_assertions(&assertion_source)?;
    if cancellation.is_cancelled() {
        return Err("assertion analysis cancelled".into());
    }
    let results = check_assertions(&waveform, &assertions)?;
    if cancellation.is_cancelled() {
        return Err("assertion analysis cancelled".into());
    }
    let values = results
        .iter()
        .map(|result| {
            Ok(Json::object([
                ("name", Json::string(result.name.clone())),
                ("kind", Json::string(assertion_kind(&result.kind))),
                ("status", Json::string(assertion_status(&result.status))),
                (
                    "sample",
                    match result.status {
                        eda_api::AssertionStatus::Fail { sample }
                        | eda_api::AssertionStatus::CoverHit { sample } => {
                            Json::from(sample as u64)
                        }
                        _ => Json::Null,
                    },
                ),
                ("samples", Json::from(result.samples as u64)),
                (
                    "clock",
                    result.clock.clone().map_or(Json::Null, Json::string),
                ),
                ("passed", Json::Bool(result.passed())),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("count", Json::from(results.len() as u64)),
        (
            "passed",
            Json::from(results.iter().filter(|result| result.passed()).count() as u64),
        ),
        ("results", Json::Array(values)),
        ("report", Json::string(assertion_report(&results))),
    ]))
}

fn assertion_kind(kind: &eda_api::AssertionKind) -> &'static str {
    match kind {
        eda_api::AssertionKind::Assert => "assert",
        eda_api::AssertionKind::Assume => "assume",
        eda_api::AssertionKind::Cover => "cover",
    }
}

fn assertion_status(status: &eda_api::AssertionStatus) -> &'static str {
    match status {
        eda_api::AssertionStatus::Pass => "pass",
        eda_api::AssertionStatus::Fail { .. } => "fail",
        eda_api::AssertionStatus::CoverHit { .. } => "coverHit",
        eda_api::AssertionStatus::CoverMiss => "coverMiss",
        eda_api::AssertionStatus::Unknown => "unknown",
    }
}

fn waveform_measure_rpc(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("waveform measurement cancelled".into());
    }
    let source = required_string(params, "vcd")?;
    let signal_name = required_string(params, "signal")?;
    let waveform = parse_waveform(&source)?;
    if cancellation.is_cancelled() {
        return Err("waveform measurement cancelled".into());
    }
    let clock = waveform.clock_metrics(&signal_name)?;
    let pulses = waveform.pulse_metrics(&signal_name)?;
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let comparison = match fields.get("compareWith") {
        None | Some(Json::Null) => None,
        Some(Json::String(other)) => Some(waveform.compare_signals(&signal_name, other)?),
        Some(_) => return Err("'compareWith' must be a signal name string".into()),
    };
    if cancellation.is_cancelled() {
        return Err("waveform measurement cancelled".into());
    }
    let mut result = vec![
        ("signal".to_string(), Json::string(signal_name.clone())),
        ("clock".to_string(), clock_metrics_json(&clock)?),
        ("pulses".to_string(), pulse_metrics_json(&pulses)?),
    ];
    if let Some(comparison) = comparison {
        result.push(("comparison".to_string(), comparison_json(&comparison)?));
    }
    Ok(Json::Object(result.into_iter().collect()))
}

fn clock_metrics_json(metrics: &eda_api::ClockMetrics) -> Result<Json, String> {
    Ok(Json::object([
        ("riseCount", Json::from(metrics.rise_count as u64)),
        ("fallCount", Json::from(metrics.fall_count as u64)),
        ("periodCount", Json::from(metrics.period_count as u64)),
        ("minPeriodNs", Json::number(metrics.min_period_ns)?),
        ("maxPeriodNs", Json::number(metrics.max_period_ns)?),
        ("averagePeriodNs", Json::number(metrics.average_period_ns)?),
        ("minHighNs", Json::number(metrics.min_high_ns)?),
        ("maxHighNs", Json::number(metrics.max_high_ns)?),
        ("averageHighNs", Json::number(metrics.average_high_ns)?),
        ("minLowNs", Json::number(metrics.min_low_ns)?),
        ("maxLowNs", Json::number(metrics.max_low_ns)?),
        ("averageLowNs", Json::number(metrics.average_low_ns)?),
        ("frequencyMhz", Json::number(metrics.frequency_mhz)?),
        (
            "dutyCyclePercent",
            Json::number(metrics.duty_cycle_percent)?,
        ),
    ]))
}

fn pulse_metrics_json(metrics: &eda_api::PulseMetrics) -> Result<Json, String> {
    Ok(Json::object([
        ("highCount", Json::from(metrics.high_count as u64)),
        ("lowCount", Json::from(metrics.low_count as u64)),
        ("minHighNs", Json::number(metrics.min_high_ns)?),
        ("maxHighNs", Json::number(metrics.max_high_ns)?),
        ("averageHighNs", Json::number(metrics.average_high_ns)?),
        ("minLowNs", Json::number(metrics.min_low_ns)?),
        ("maxLowNs", Json::number(metrics.max_low_ns)?),
        ("averageLowNs", Json::number(metrics.average_low_ns)?),
        (
            "dutyCyclePercent",
            Json::number(metrics.duty_cycle_percent)?,
        ),
    ]))
}

fn comparison_json(comparison: &eda_api::SignalComparison) -> Result<Json, String> {
    Ok(Json::object([
        ("sampleCount", Json::from(comparison.sample_count as u64)),
        (
            "matchingSamples",
            Json::from(comparison.matching_samples as u64),
        ),
        (
            "differingSamples",
            Json::from(comparison.differing_samples as u64),
        ),
        (
            "unknownSamples",
            Json::from(comparison.unknown_samples as u64),
        ),
        (
            "hammingDistance",
            Json::from(comparison.hamming_distance as u64),
        ),
    ]))
}

fn spice_dc(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("DC operating point RPC cancelled".into());
    }
    let source = required_string(params, "source")?;
    let circuit = parse_spice(&source)?;
    let op = eda_api::dc_operating_point_cancellable(&circuit, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("DC operating point RPC cancelled".into());
    }
    let nodes = op
        .node_voltages
        .iter()
        .map(|(node, value)| Ok((node.clone(), Json::number(*value)?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let branches = op
        .branch_currents
        .iter()
        .map(|(branch, value)| Ok((branch.clone(), Json::number(*value)?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    Ok(Json::object([
        ("title", Json::string(circuit.title.clone())),
        ("converged", Json::Bool(op.converged)),
        ("iterations", Json::number(op.iterations as f64)?),
        ("nodeVoltages", Json::Object(nodes)),
        ("branchCurrents", Json::Object(branches)),
    ]))
}

fn spice_run(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let circuit = parse_spice(&source)?;
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let all = matches!(fields.get("all"), Some(Json::Bool(true)));
    if all {
        let results = run_all_ams_analyses_cancellable(&circuit, cancellation)?;
        let reports = results
            .iter()
            .map(report_ams_result)
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(Json::object([
            ("kinds", Json::Array(results.iter().map(ams_kind).collect())),
            ("count", Json::number(results.len() as f64)?),
            ("reports", Json::string(reports)),
        ]));
    }
    let index = match fields.get("analysisIndex") {
        None => 0_usize,
        Some(Json::Number(value)) => {
            let index = *value;
            if index < 0.0 || index.fract() != 0.0 || index > usize::MAX as f64 {
                return Err("'analysisIndex' must be a non-negative integer".into());
            }
            index as usize
        }
        Some(Json::Null) => 0_usize,
        Some(_) => return Err("'analysisIndex' must be a non-negative integer".into()),
    };
    let analysis = circuit
        .analyses
        .get(index)
        .ok_or_else(|| format!("circuit has no analysis at index {index}"))?;
    let result = run_ams_analysis_cancellable(&circuit, analysis, cancellation)?;
    Ok(Json::object([
        ("kind", ams_kind(&result)),
        ("analysisIndex", Json::number(index as f64)?),
        ("report", Json::string(report_ams_result(&result))),
    ]))
}

fn ams_kind(result: &AmsResult) -> Json {
    let kind = match result {
        AmsResult::OperatingPoint(_) => "operatingPoint",
        AmsResult::DcSweep(_) => "dcSweep",
        AmsResult::Transient(_) => "transient",
        AmsResult::AcSweep(_) => "acSweep",
        AmsResult::NoiseSweep(_) => "noiseSweep",
        AmsResult::MonteCarlo(_) => "monteCarlo",
        AmsResult::Sensitivity(_) => "sensitivity",
        AmsResult::GlobalSensitivity(_) => "globalSensitivity",
        AmsResult::Derivative(_) => "derivative",
        AmsResult::Optimization(_) => "optimization",
        AmsResult::Pareto(_) => "pareto",
        AmsResult::PeriodicSteadyState(_) => "periodicSteadyState",
        AmsResult::EnvelopeFollow(_) => "envelopeFollow",
        AmsResult::Aging(_) => "aging",
    };
    Json::string(kind)
}

fn rtl_flow(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = match params
        .as_object()
        .ok_or("params must be an object")?
        .get("top")
    {
        None | Some(Json::Null) => None,
        Some(value) => Some(value.as_str().ok_or("'top' must be a string")?),
    };
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    flow_result_json(&flow)
}

fn rtl_flow_cached(
    params: &Json,
    cancellation: &FlowCancellation,
    flow_cache: &SharedFlowCache,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("cached RTL flow cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let library = CellLibrary::std();
    let key = FlowCacheKey::new(&source, top, &library);
    let cached = flow_cache.get_or_insert_with(&key, || {
        run_flow_with_library_cancellable(&source, top, &library, cancellation)
    })?;
    if cancellation.is_cancelled() {
        return Err("cached RTL flow cancelled".into());
    }
    let mut result = flow_result_json(&cached.flow)?;
    let Json::Object(fields) = &mut result else {
        return Err("internal error: RTL flow result is not an object".into());
    };
    fields.insert("cacheHit".into(), Json::Bool(cached.cache_hit));
    fields.insert("cacheId".into(), Json::string(key.cache_id()));
    Ok(result)
}

fn flow_cache_stats_json(flow_cache: &SharedFlowCache) -> Result<Json, String> {
    let stats = flow_cache.stats()?;
    Ok(Json::object([
        ("hits", Json::from(stats.hits)),
        ("misses", Json::from(stats.misses)),
        ("inserts", Json::from(stats.inserts)),
        ("evictions", Json::from(stats.evictions)),
    ]))
}

fn rtl_cts(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let timing_driven = match fields.get("timingDriven") {
        None | Some(Json::Null) => false,
        Some(Json::Bool(value)) => *value,
        Some(_) => return Err("'timingDriven' must be a boolean".into()),
    };
    let target_skew_ns = match fields.get("targetSkewNs") {
        None | Some(Json::Null) => 0.05,
        Some(Json::Number(value)) => {
            if !value.is_finite() || *value < 0.0 {
                return Err("'targetSkewNs' must be finite and nonnegative".into());
            }
            *value
        }
        Some(_) => return Err("'targetSkewNs' must be a number".into()),
    };
    if !timing_driven && fields.contains_key("targetSkewNs") {
        return Err("'targetSkewNs' requires 'timingDriven': true".into());
    }
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if timing_driven {
        let options = TimingDrivenCtsOptions {
            target_skew_ns,
            ..Default::default()
        };
        let result = timing_driven_clock_tree_synthesis_with_placement(
            &flow.netlist,
            &flow.placement,
            &options,
        )?;
        let mut trees = Vec::new();
        for tree in &result.cts.trees {
            trees.push(clock_tree_json(tree)?);
        }
        return Ok(Json::object([
            ("timingDriven", Json::Bool(true)),
            (
                "buffersInserted",
                Json::from(result.cts.buffers_inserted as u64),
            ),
            (
                "originalCells",
                Json::from(result.cts.original_cells as u64),
            ),
            ("originalNets", Json::from(result.cts.original_nets as u64)),
            (
                "selectedBranchFactor",
                Json::from(result.selected_branch_factor as u64),
            ),
            (
                "searchedBranchFactors",
                Json::Array(
                    result
                        .searched_branch_factors
                        .iter()
                        .map(|factor| Json::from(*factor as u64))
                        .collect(),
                ),
            ),
            (
                "bestEstimatedSkewNs",
                Json::number(result.best_estimated_skew_ns)?,
            ),
            ("targetMet", Json::Bool(result.target_met)),
            ("trees", Json::Array(trees)),
            ("report", Json::string(timing_driven_cts_report(&result))),
        ]));
    }
    let result =
        clock_tree_synthesis_with_placement(&flow.netlist, &flow.placement, &Default::default())?;
    let trees = result
        .trees
        .iter()
        .map(clock_tree_json)
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("timingDriven", Json::Bool(false)),
        (
            "buffersInserted",
            Json::from(result.buffers_inserted as u64),
        ),
        ("originalCells", Json::from(result.original_cells as u64)),
        ("originalNets", Json::from(result.original_nets as u64)),
        ("trees", Json::Array(trees)),
        ("report", Json::string(cts_report(&result))),
    ]))
}

fn clock_tree_json(tree: &eda_api::ClockTree) -> Result<Json, String> {
    Ok(Json::object([
        ("domain", Json::string(tree.domain.clone())),
        ("root", Json::from(tree.root as u64)),
        ("sinkCount", Json::from(tree.sinks.len() as u64)),
        ("bufferCells", Json::from(tree.buffer_cells.len() as u64)),
        ("levels", Json::from(tree.levels as u64)),
        ("branchFactor", Json::from(tree.branch_factor as u64)),
        ("maxFanout", Json::from(tree.max_fanout as u64)),
        (
            "insertionDelayNs",
            Json::number(tree.estimated_insertion_delay_ns)?,
        ),
        (
            "logicalSkewNs",
            Json::number(tree.estimated_logical_skew_ns)?,
        ),
        ("wireDelayNs", Json::number(tree.estimated_wire_delay_ns)?),
        (
            "physicalSkewNs",
            Json::number(tree.estimated_physical_skew_ns)?,
        ),
        ("skewNs", Json::number(tree.estimated_skew_ns)?),
        ("placementAware", Json::Bool(tree.placement_aware)),
    ]))
}

fn rtl_eco(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL ECO cancelled".into());
    }
    let source = required_string(params, "source")?;
    let patch_source = required_string(params, "eco")?;
    let top = optional_top(params)?;
    let patch = parse_eco(&patch_source)?;
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL ECO cancelled".into());
    }
    let result = apply_eco(&flow.netlist, &patch)?;
    let placement = eda_api::extend_physical_placement(&result.netlist, &flow.placement)?;
    let stats = result.netlist.stats();
    let changes = result
        .changes
        .iter()
        .map(|change| {
            Ok(Json::object([
                ("operation", Json::from(change.operation as u64)),
                ("description", Json::string(change.description.clone())),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let verilog = export_verilog(&result.netlist)?;
    if cancellation.is_cancelled() {
        return Err("RTL ECO cancelled".into());
    }
    Ok(Json::object([
        ("patchName", Json::string(result.patch_name.clone())),
        (
            "hierarchyScope",
            result
                .hierarchy_scope
                .clone()
                .map_or(Json::Null, Json::string),
        ),
        (
            "operationsApplied",
            Json::from(result.operations_applied as u64),
        ),
        (
            "stats",
            Json::object([
                ("nets", Json::number(stats.nets as f64)?),
                ("cells", Json::number(stats.cells as f64)?),
                ("comb", Json::number(stats.comb as f64)?),
                ("seq", Json::number(stats.seq as f64)?),
                ("consts", Json::number(stats.consts as f64)?),
                ("libCells", Json::number(stats.lib_cells as f64)?),
            ]),
        ),
        ("placementRows", Json::number(placement.rows as f64)?),
        ("changes", Json::Array(changes)),
        ("verilog", Json::string(verilog)),
        ("report", Json::string(eco_report(&result))),
    ]))
}

fn flow_result_json(flow: &Flow) -> Result<Json, String> {
    let drc_violations = flow
        .drc
        .violations
        .iter()
        .map(|violation| {
            Ok(Json::object([
                ("code", Json::string(violation.code.clone())),
                (
                    "severity",
                    Json::string(match violation.severity {
                        eda_pnr::DrcSeverity::Warning => "warning",
                        eda_pnr::DrcSeverity::Error => "error",
                    }),
                ),
                ("message", Json::string(violation.message.clone())),
                (
                    "cell",
                    violation
                        .cell
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "net",
                    violation
                        .net
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "row",
                    violation
                        .row
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "site",
                    violation
                        .site
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let wire_length = flow
        .routing
        .segments
        .iter()
        .map(|segment| segment.length())
        .sum::<usize>();
    Ok(Json::object([
        ("equivalent", Json::Bool(flow.equivalent)),
        ("netlist", Json::string(flow.netlist.name.clone())),
        (
            "stats",
            Json::object([
                ("nets", Json::number(flow.stats.nets as f64)?),
                ("cells", Json::number(flow.stats.cells as f64)?),
                ("comb", Json::number(flow.stats.comb as f64)?),
                ("seq", Json::number(flow.stats.seq as f64)?),
                ("consts", Json::number(flow.stats.consts as f64)?),
                ("libCells", Json::number(flow.stats.lib_cells as f64)?),
            ]),
        ),
        (
            "timing",
            Json::object([
                (
                    "criticalPathDelayNs",
                    Json::number(flow.timing.critical_path_delay)?,
                ),
                (
                    "reg2regCriticalNs",
                    Json::number(flow.timing.reg2reg_critical)?,
                ),
                (
                    "in2regCriticalNs",
                    Json::number(flow.timing.in2reg_critical)?,
                ),
                (
                    "reg2regHoldCriticalNs",
                    Json::number(flow.timing.reg2reg_hold_critical)?,
                ),
                (
                    "in2regHoldCriticalNs",
                    Json::number(flow.timing.in2reg_hold_critical)?,
                ),
                (
                    "disabledTimingArcs",
                    Json::number(flow.timing.disabled_timing_arcs as f64)?,
                ),
                (
                    "caseAnalysisNets",
                    Json::Object(
                        flow.timing
                            .case_analysis_nets
                            .iter()
                            .map(|(net, value)| (net.clone(), Json::Bool(*value)))
                            .collect(),
                    ),
                ),
            ]),
        ),
        (
            "physical",
            Json::object([
                ("placementRows", Json::number(flow.placement.rows as f64)?),
                (
                    "placedCells",
                    Json::number(flow.placement.cell_pos.len() as f64)?,
                ),
                (
                    "placementBlockages",
                    Json::number(flow.placement.blockages.len() as f64)?,
                ),
                (
                    "routingSegments",
                    Json::number(flow.routing.segments.len() as f64)?,
                ),
                ("vias", Json::number(flow.routing.vias as f64)?),
                (
                    "explicitViaLocations",
                    Json::number(flow.routing.via_locations.len() as f64)?,
                ),
                (
                    "tracksPerChannel",
                    Json::number(flow.routing.tracks_per_channel as f64)?,
                ),
                (
                    "congestedChannels",
                    Json::number(flow.routing.congested_channels.len() as f64)?,
                ),
                ("wireLength", Json::number(wire_length as f64)?),
                (
                    "drcErrors",
                    Json::number(
                        flow.drc
                            .violations
                            .iter()
                            .filter(|violation| violation.severity == eda_pnr::DrcSeverity::Error)
                            .count() as f64,
                    )?,
                ),
                (
                    "drcWarnings",
                    Json::number(
                        flow.drc
                            .violations
                            .iter()
                            .filter(|violation| violation.severity == eda_pnr::DrcSeverity::Warning)
                            .count() as f64,
                    )?,
                ),
                ("drcViolations", Json::Array(drc_violations)),
            ]),
        ),
        ("verilog", Json::string(flow.verilog.clone())),
        ("lef", Json::string(flow.lef.clone())),
        ("def", Json::string(flow.def.clone())),
        ("sdf", Json::string(flow.sdf.clone())),
    ]))
}

fn rtl_equiv(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let result = check_rtl_equivalence_cancellable(&source, top, cancellation)?;
    Ok(Json::object([
        ("equivalent", Json::Bool(result.equivalent)),
        (
            "counterexample",
            Json::Object(
                result
                    .counterexample
                    .into_iter()
                    .map(|(input, value)| (input, Json::string(value)))
                    .collect(),
            ),
        ),
        (
            "notes",
            Json::Array(result.notes.into_iter().map(Json::string).collect()),
        ),
        (
            "goldStats",
            Json::object([
                ("nets", Json::number(result.gold_stats.nets as f64)?),
                ("cells", Json::number(result.gold_stats.cells as f64)?),
                ("comb", Json::number(result.gold_stats.comb as f64)?),
                ("seq", Json::number(result.gold_stats.seq as f64)?),
                ("consts", Json::number(result.gold_stats.consts as f64)?),
            ]),
        ),
        (
            "mappedStats",
            Json::object([
                ("nets", Json::number(result.mapped_stats.nets as f64)?),
                ("cells", Json::number(result.mapped_stats.cells as f64)?),
                ("comb", Json::number(result.mapped_stats.comb as f64)?),
                ("seq", Json::number(result.mapped_stats.seq as f64)?),
                ("consts", Json::number(result.mapped_stats.consts as f64)?),
                (
                    "libCells",
                    Json::number(result.mapped_stats.lib_cells as f64)?,
                ),
            ]),
        ),
    ]))
}

fn rtl_equiv_views(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL view equivalence cancelled".into());
    }
    let source_a = required_string(params, "sourceA")?;
    let source_b = required_string(params, "sourceB")?;
    let top_a = optional_top_field(params, "topA")?;
    let top_b = optional_top_field(params, "topB")?;
    let flow_a = run_flow_cancellable(&source_a, top_a, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL view equivalence cancelled".into());
    }
    let flow_b = run_flow_cancellable(&source_b, top_b, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL view equivalence cancelled".into());
    }
    let result = check_netlists_equivalent_cancellable(&flow_a.gold, &flow_b.gold, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL view equivalence cancelled".into());
    }

    let flow_stats = |flow: &Flow| -> Result<Json, String> {
        Ok(Json::object([
            ("nets", Json::number(flow.gold.net_count() as f64)?),
            ("cells", Json::number(flow.gold.cell_count() as f64)?),
            ("libCells", Json::number(flow.stats.lib_cells as f64)?),
        ]))
    };
    Ok(Json::object([
        ("equivalent", Json::Bool(result.equivalent)),
        (
            "counterexample",
            Json::Object(
                result
                    .counterexample
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(input, value)| (input, Json::string(value.to_bin_str())))
                    .collect(),
            ),
        ),
        (
            "notes",
            Json::Array(result.notes.into_iter().map(Json::string).collect()),
        ),
        ("viewA", flow_stats(&flow_a)?),
        ("viewB", flow_stats(&flow_b)?),
    ]))
}

fn rtl_sta(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let result = analyze_rtl_timing_cancellable(&source, top, cancellation)?;
    Ok(Json::object([
        (
            "criticalPathDelayNs",
            Json::number(result.critical_path_delay_ns)?,
        ),
        (
            "reg2regCriticalNs",
            Json::number(result.reg2reg_critical_ns)?,
        ),
        ("in2regCriticalNs", Json::number(result.in2reg_critical_ns)?),
        (
            "reg2regHoldCriticalNs",
            Json::number(result.reg2reg_hold_critical_ns)?,
        ),
        (
            "in2regHoldCriticalNs",
            Json::number(result.in2reg_hold_critical_ns)?,
        ),
        (
            "disabledTimingArcs",
            Json::number(result.disabled_timing_arcs as f64)?,
        ),
        (
            "caseAnalysisNets",
            Json::Object(
                result
                    .case_analysis_nets
                    .into_iter()
                    .map(|(net, value)| (net, Json::Bool(value)))
                    .collect(),
            ),
        ),
        (
            "mappedStats",
            Json::object([
                ("nets", Json::number(result.mapped_stats.nets as f64)?),
                ("cells", Json::number(result.mapped_stats.cells as f64)?),
                ("comb", Json::number(result.mapped_stats.comb as f64)?),
                ("seq", Json::number(result.mapped_stats.seq as f64)?),
                ("consts", Json::number(result.mapped_stats.consts as f64)?),
                (
                    "libCells",
                    Json::number(result.mapped_stats.lib_cells as f64)?,
                ),
            ]),
        ),
    ]))
}

fn parse_quality_limit(value: &Json) -> Result<ScopedQualityLimit, String> {
    let Some(fields) = value.as_object() else {
        return Err("quality limit entries must be objects".into());
    };
    let Some(Json::String(kind)) = fields.get("kind") else {
        return Err("quality limit entry requires a string 'kind'".into());
    };
    let kind = match kind.as_str() {
        "port" => TimingObjectKind::Port,
        "pin" => TimingObjectKind::Pin,
        "net" => TimingObjectKind::Net,
        "cell" => TimingObjectKind::Cell,
        "clock" => TimingObjectKind::Clock,
        other => {
            return Err(format!(
                "quality limit kind must be port, pin, net, cell, or clock, found '{other}'"
            ))
        }
    };
    let Some(Json::String(name)) = fields.get("name") else {
        return Err("quality limit entry requires a string 'name'".into());
    };
    let Some(limit) = fields.get("limit").and_then(Json::as_number) else {
        return Err("quality limit entry requires a numeric 'limit'".into());
    };
    Ok(ScopedQualityLimit {
        object: eda_api::TimingObjectRef::new(kind, name.clone()),
        limit: *limit,
    })
}

fn parse_quality_limits(value: Option<&Json>) -> Result<Vec<ScopedQualityLimit>, String> {
    let Some(Json::Array(values)) = value else {
        return Ok(Vec::new());
    };
    values.iter().map(parse_quality_limit).collect()
}

fn parse_quality_options(params: &Json) -> Result<QualityOptions, String> {
    let Some(fields) = params
        .as_object()
        .and_then(|fields| fields.get("qualityOptions"))
        .and_then(Json::as_object)
    else {
        return Ok(QualityOptions::default());
    };
    let mut options = QualityOptions::default();
    if let Some(value) = fields.get("inputTransitionNs") {
        let Some(value) = value.as_number() else {
            return Err("quality option 'inputTransitionNs' must be a number".into());
        };
        options.input_transition_ns = *value;
    }
    for (json_name, target) in [
        ("maxTransitionNs", &mut options.max_transition_ns),
        ("maxCapacitancePf", &mut options.max_capacitance_pf),
    ] {
        match fields.get(json_name) {
            None | Some(Json::Null) => {}
            Some(value) => {
                let Some(value) = value.as_number() else {
                    return Err(format!(
                        "quality option '{json_name}' must be a number or null"
                    ));
                };
                *target = Some(*value);
            }
        }
    }
    match fields.get("maxFanout") {
        None | Some(Json::Null) => {}
        Some(Json::Number(value)) => {
            if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
                return Err("quality option 'maxFanout' must be a positive integer".into());
            }
            options.max_fanout = Some(*value as usize);
        }
        Some(_) => return Err("quality option 'maxFanout' must be a positive integer".into()),
    }
    options.scoped_max_transition_ns = parse_quality_limits(fields.get("scopedMaxTransitionNs"))?;
    options.scoped_max_capacitance_pf = parse_quality_limits(fields.get("scopedMaxCapacitancePf"))?;
    options.scoped_max_fanout = parse_quality_limits(fields.get("scopedMaxFanout"))?;
    options.input_transitions = parse_quality_limits(fields.get("inputTransitions"))?;
    options.load_capacitances = parse_quality_limits(fields.get("loadCapacitances"))?;
    if let Some(Json::Object(values)) = fields.get("extraCapacitancePf") {
        for (net, value) in values {
            let Some(value) = value.as_number() else {
                return Err(format!(
                    "extra capacitance for net '{net}' must be a number"
                ));
            };
            options.extra_capacitance_pf.insert(net.clone(), *value);
        }
    } else if fields.contains_key("extraCapacitancePf") {
        return Err("quality option 'extraCapacitancePf' must be an object".into());
    }
    Ok(options)
}

fn rtl_signal_quality(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL signal-quality analysis cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let options = parse_quality_options(params)?;
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL signal-quality analysis cancelled".into());
    }
    let result = analyze_signal_quality(&flow.netlist, &options)?;
    if cancellation.is_cancelled() {
        return Err("RTL signal-quality analysis cancelled".into());
    }

    let optional_number = |value: Option<f64>| -> Result<Json, String> {
        value
            .filter(|value| value.is_finite())
            .map_or(Ok(Json::Null), Json::number)
    };
    let violations = result
        .violations
        .iter()
        .map(|violation| {
            Ok(Json::object([
                (
                    "kind",
                    Json::string(match violation.kind {
                        QualityKind::MaxTransition => "maxTransition",
                        QualityKind::MaxCapacitance => "maxCapacitance",
                        QualityKind::MaxFanout => "maxFanout",
                    }),
                ),
                ("cell", Json::from(violation.cell as i64)),
                ("net", Json::from(violation.net as i64)),
                (
                    "netName",
                    Json::string(flow.netlist.net(violation.net).name.clone()),
                ),
                ("actual", Json::number(violation.actual)?),
                ("limit", Json::number(violation.limit)?),
                ("message", Json::string(violation.message.clone())),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("hasViolations", Json::Bool(result.has_violations())),
        ("violationCount", Json::from(result.violations.len() as u64)),
        (
            "transitionsChecked",
            Json::from(result.transitions_checked as u64),
        ),
        (
            "capacitancesChecked",
            Json::from(result.capacitances_checked as u64),
        ),
        ("fanoutsChecked", Json::from(result.fanouts_checked as u64)),
        (
            "worstTransitionSlackNs",
            optional_number(result.worst_transition_slack)?,
        ),
        (
            "worstCapacitanceSlackNs",
            optional_number(result.worst_capacitance_slack)?,
        ),
        ("violations", Json::Array(violations)),
        (
            "mappedStats",
            Json::object([
                ("cells", Json::number(flow.netlist.cell_count() as f64)?),
                ("nets", Json::number(flow.netlist.net_count() as f64)?),
                ("libCells", Json::number(flow.stats.lib_cells as f64)?),
            ]),
        ),
        ("report", Json::string(signal_quality_report(&result))),
    ]))
}

fn string_array(values: impl IntoIterator<Item = String>) -> Json {
    Json::Array(values.into_iter().map(Json::string).collect())
}

fn timing_pin_label(pin: &eda_api::TimingPin) -> String {
    format!("{}/{}", pin.cell, pin.pin)
}

fn timing_constraint_diff_json(diff: &eda_api::TimingConstraintDiff) -> Json {
    Json::object([
        (
            "changedClocks",
            string_array(diff.changed_clocks.iter().cloned()),
        ),
        (
            "changedInputPorts",
            string_array(diff.changed_input_ports.iter().cloned()),
        ),
        (
            "changedOutputPorts",
            string_array(diff.changed_output_ports.iter().cloned()),
        ),
        (
            "changedCellDerates",
            string_array(diff.changed_cell_derates.iter().cloned()),
        ),
        (
            "changedPinDerates",
            string_array(diff.changed_pin_derates.iter().map(timing_pin_label)),
        ),
        (
            "scopedTimingDeratesChanged",
            Json::Bool(diff.scoped_timing_derates_changed),
        ),
        (
            "changedPortDerates",
            string_array(diff.changed_port_derates.iter().cloned()),
        ),
        (
            "changedCellCheckDerates",
            string_array(diff.changed_cell_check_derates.iter().cloned()),
        ),
        (
            "changedPinCheckDerates",
            string_array(diff.changed_pin_check_derates.iter().map(timing_pin_label)),
        ),
        (
            "changedClockUncertainty",
            string_array(diff.changed_clock_uncertainty.iter().cloned()),
        ),
        (
            "changedHoldUncertainty",
            string_array(diff.changed_hold_uncertainty.iter().cloned()),
        ),
        (
            "timingExceptionsChanged",
            Json::Bool(diff.timing_exceptions_changed),
        ),
        ("clockGroupsChanged", Json::Bool(diff.clock_groups_changed)),
        ("delayLimitsChanged", Json::Bool(diff.delay_limits_changed)),
        (
            "disabledTimingChanged",
            Json::Bool(diff.disabled_timing_changed),
        ),
        (
            "caseAnalysisChanged",
            Json::Bool(diff.case_analysis_changed),
        ),
        (
            "globalTimingDerateChanged",
            Json::Bool(diff.global_timing_derate_changed),
        ),
        (
            "globalCheckDerateChanged",
            Json::Bool(diff.global_check_derate_changed),
        ),
        ("designRulesChanged", Json::Bool(diff.design_rules_changed)),
        (
            "inputTransitionsChanged",
            Json::Bool(diff.input_transitions_changed),
        ),
        ("loadsChanged", Json::Bool(diff.loads_changed)),
        (
            "fullBaseRecomputeRequired",
            Json::Bool(diff.full_base_recompute_required),
        ),
        (
            "baseRecomputeRequired",
            Json::Bool(diff.base_recompute_required),
        ),
        (
            "constrainedRecomputeRequired",
            Json::Bool(diff.constrained_recompute_required),
        ),
        (
            "qualityRecomputeRequired",
            Json::Bool(diff.quality_recompute_required),
        ),
    ])
}

fn timing_scenario_diff(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing scenario diff cancelled".into());
    }
    let old_source = required_string(params, "old")?;
    let new_source = required_string(params, "new")?;
    let old = parse_timing_scenarios(&old_source)?;
    let new = parse_timing_scenarios(&new_source)?;
    if cancellation.is_cancelled() {
        return Err("timing scenario diff cancelled".into());
    }
    let diff = diff_api_timing_scenarios(&old, &new)?;
    if cancellation.is_cancelled() {
        return Err("timing scenario diff cancelled".into());
    }

    let properties = diff
        .properties
        .iter()
        .map(|(scenario, property)| {
            let changed = property
                .changed
                .iter()
                .map(|(key, (old, new))| {
                    (
                        key.clone(),
                        Json::object([
                            ("old", Json::string(old.clone())),
                            ("new", Json::string(new.clone())),
                        ]),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            (
                scenario.clone(),
                Json::object([
                    ("added", string_array(property.added.iter().cloned())),
                    ("removed", string_array(property.removed.iter().cloned())),
                    ("changed", Json::Object(changed)),
                ]),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let constraints = diff
        .constraints
        .iter()
        .map(|(scenario, constraint)| (scenario.clone(), timing_constraint_diff_json(constraint)))
        .collect::<BTreeMap<_, _>>();

    Ok(Json::object([
        ("oldScenarioCount", Json::from(old.len() as u64)),
        ("newScenarioCount", Json::from(new.len() as u64)),
        ("added", string_array(diff.added.iter().cloned())),
        ("removed", string_array(diff.removed.iter().cloned())),
        ("changed", string_array(diff.changed.iter().cloned())),
        (
            "timingChanged",
            string_array(diff.timing_changed.iter().cloned()),
        ),
        ("constraints", Json::Object(constraints)),
        ("properties", Json::Object(properties)),
        ("report", Json::string(timing_scenario_diff_report(&diff))),
    ]))
}

fn timing_waiver_json(waiver: &eda_api::TimingViolationWaiver) -> Result<Json, String> {
    Ok(Json::object([
        ("id", Json::string(waiver.id.clone())),
        (
            "scenarioPattern",
            Json::string(waiver.scenario_pattern.clone()),
        ),
        (
            "endpointPattern",
            Json::string(waiver.endpoint_pattern.clone()),
        ),
        (
            "instancePattern",
            waiver
                .instance_pattern
                .clone()
                .map_or(Json::Null, Json::string),
        ),
        (
            "kind",
            waiver
                .kind
                .map(|kind| Json::string(kind.label().to_string()))
                .unwrap_or(Json::Null),
        ),
        ("slackFloorNs", Json::number(waiver.slack_floor_ns)?),
        (
            "owner",
            waiver.owner.clone().map_or(Json::Null, Json::string),
        ),
        ("note", waiver.note.clone().map_or(Json::Null, Json::string)),
        (
            "expiresAfterGeneration",
            waiver
                .expires_after_generation
                .map(Json::from)
                .unwrap_or(Json::Null),
        ),
    ]))
}

fn timing_waiver_audit(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing waiver audit cancelled".into());
    }
    let source = required_string(params, "audit")?;
    let entries = parse_timing_waiver_audit(&source)?;
    if cancellation.is_cancelled() {
        return Err("timing waiver audit cancelled".into());
    }
    let active = active_timing_waivers_from_audit(&entries)?;
    let summaries = timing_waiver_audit_hierarchy_summaries(&entries)?;
    if cancellation.is_cancelled() {
        return Err("timing waiver audit cancelled".into());
    }

    let events = entries
        .iter()
        .map(|entry| {
            Ok(Json::object([
                ("sequence", Json::from(entry.sequence)),
                (
                    "action",
                    Json::string(match entry.action {
                        TimingWaiverAuditAction::Added => "added",
                        TimingWaiverAuditAction::Removed => "removed",
                    }),
                ),
                ("waiverId", Json::string(entry.waiver_id.clone())),
                ("generation", Json::from(entry.generation)),
                (
                    "changedBy",
                    entry.changed_by.clone().map_or(Json::Null, Json::string),
                ),
                ("waiver", timing_waiver_json(&entry.waiver)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let active_waivers = active
        .iter()
        .map(timing_waiver_json)
        .collect::<Result<Vec<_>, String>>()?;
    let hierarchy = summaries
        .iter()
        .map(|(instance, summary)| {
            let key = instance.clone().unwrap_or_else(|| "(unscoped)".to_string());
            (
                key,
                Json::object([
                    (
                        "instancePattern",
                        summary
                            .instance_pattern
                            .clone()
                            .map_or(Json::Null, Json::string),
                    ),
                    ("events", Json::from(summary.events as u64)),
                    ("addedEvents", Json::from(summary.added_events as u64)),
                    ("removedEvents", Json::from(summary.removed_events as u64)),
                    (
                        "activeWaiverIds",
                        string_array(summary.active_waiver_ids.iter().cloned()),
                    ),
                    ("latestGeneration", Json::from(summary.latest_generation)),
                ]),
            )
        })
        .collect::<BTreeMap<_, _>>();

    Ok(Json::object([
        ("eventCount", Json::from(entries.len() as u64)),
        ("activeWaiverCount", Json::from(active.len() as u64)),
        ("events", Json::Array(events)),
        ("activeWaivers", Json::Array(active_waivers)),
        ("hierarchy", Json::Object(hierarchy)),
        ("report", Json::string(timing_waiver_audit_report(&entries))),
        (
            "hierarchyReport",
            Json::string(timing_waiver_audit_hierarchy_report(&entries)?),
        ),
    ]))
}

fn timing_violation_json(violation: &eda_api::TimingViolation) -> Result<Json, String> {
    Ok(Json::object([
        ("id", Json::string(violation.id.clone())),
        ("scenario", Json::string(violation.scenario.clone())),
        ("kind", Json::string(violation.kind.label().to_string())),
        ("endpoint", Json::string(violation.endpoint.clone())),
        ("launch", Json::string(violation.launch.clone())),
        ("slackNs", Json::number(violation.slack_ns)?),
        ("arrivalNs", Json::number(violation.arrival_ns)?),
        ("requiredNs", Json::number(violation.required_ns)?),
        ("cells", string_array(violation.cells.iter().cloned())),
        ("nets", string_array(violation.nets.iter().cloned())),
        (
            "hierarchy",
            violation.hierarchy.clone().map_or(Json::Null, Json::string),
        ),
        (
            "hierarchyInstances",
            string_array(violation.hierarchy_instances.iter().cloned()),
        ),
    ]))
}

fn timing_violation_store_json(store: &eda_api::TimingViolationStore) -> Result<Json, String> {
    let violations = store
        .active_violations()
        .values()
        .map(timing_violation_json)
        .collect::<Result<Vec<_>, String>>()?;
    let waived = store
        .waived_violations()
        .iter()
        .map(|(violation, waiver)| {
            (
                violation.clone(),
                Json::object([
                    ("violation", Json::string(violation.clone())),
                    ("waiver", Json::string(waiver.clone())),
                ]),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let waivers = store
        .waivers()
        .values()
        .map(timing_waiver_json)
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("scenario", Json::string(store.scenario.clone())),
        ("generation", Json::from(store.generation)),
        ("activeViolations", Json::Array(violations)),
        ("activeViolationCount", Json::from(store.len() as u64)),
        ("waivedViolations", Json::Object(waived)),
        (
            "waivedViolationCount",
            Json::from(store.waived_len() as u64),
        ),
        ("waivers", Json::Array(waivers)),
        (
            "waiverAuditReport",
            Json::string(timing_waiver_audit_report(store.audit())),
        ),
    ]))
}

fn cross_corner_hierarchy_summary_json(
    summary: &eda_api::CrossCornerHierarchyViolationSummary,
) -> Result<Json, String> {
    let kinds = summary
        .kinds
        .iter()
        .map(|(kind, count)| (kind.label().to_string(), Json::from(*count as u64)))
        .collect::<BTreeMap<_, _>>();
    Ok(Json::object([
        (
            "hierarchy",
            summary.hierarchy.clone().map_or(Json::Null, Json::string),
        ),
        ("violations", Json::from(summary.violations as u64)),
        ("worstSlackNs", Json::number(summary.worst_slack_ns)?),
        (
            "worstScenario",
            Json::string(summary.worst_scenario.clone()),
        ),
        ("kinds", Json::Object(kinds)),
    ]))
}

fn cross_corner_timing_violation_json(
    summary: &eda_api::CrossCornerTimingViolation,
) -> Result<Json, String> {
    let scenarios = summary
        .scenarios
        .iter()
        .map(|(scenario, violation)| {
            Ok((
                scenario.clone(),
                Json::object([
                    ("id", Json::string(violation.id.clone())),
                    ("slackNs", Json::number(violation.slack_ns)?),
                ]),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let hierarchy = summary
        .hierarchy_summary
        .iter()
        .map(|(owner, values)| {
            Ok((
                owner.clone().unwrap_or_else(|| "(top)".to_string()),
                cross_corner_hierarchy_summary_json(values)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    Ok(Json::object([
        ("key", Json::string(summary.key.clone())),
        ("kind", Json::string(summary.kind.label().to_string())),
        ("endpoint", Json::string(summary.endpoint.clone())),
        ("scenarios", Json::Object(scenarios)),
        ("hierarchy", Json::Object(hierarchy)),
        (
            "worstScenario",
            Json::string(summary.worst_scenario.clone()),
        ),
        ("worstSlackNs", Json::number(summary.worst_slack_ns)?),
    ]))
}

fn timing_violation_registry_json(
    registry: &eda_api::TimingViolationRegistry,
) -> Result<Json, String> {
    let scenarios = registry
        .stores()
        .iter()
        .map(|(name, store)| Ok((name.clone(), timing_violation_store_json(store)?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let summaries = registry
        .summaries()
        .values()
        .map(cross_corner_timing_violation_json)
        .collect::<Result<Vec<_>, String>>()?;
    let hierarchy_summaries = registry
        .hierarchy_summaries()
        .into_iter()
        .map(|(owner, summary)| {
            Ok((
                owner.unwrap_or_else(|| "(top)".to_string()),
                cross_corner_hierarchy_summary_json(&summary)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    Ok(Json::object([
        ("scenarioCount", Json::from(registry.stores().len() as u64)),
        ("generation", Json::from(registry.generation())),
        ("activeViolationCount", Json::from(registry.len() as u64)),
        ("hasActiveViolations", Json::Bool(!registry.is_empty())),
        ("scenarios", Json::Object(scenarios)),
        ("summaries", Json::Array(summaries)),
        ("hierarchySummaries", Json::Object(hierarchy_summaries)),
        (
            "report",
            Json::string(timing_violation_registry_report(registry, None)),
        ),
    ]))
}

fn timing_waiver_registry_response(
    registry_id: &str,
    state: &TimingViolationRegistryState,
    update: Option<Json>,
    cache_hit: Option<bool>,
) -> Result<Json, String> {
    let mut fields = timing_violation_registry_json(&state.registry)?
        .as_object()
        .cloned()
        .ok_or_else(|| "timing violation registry serialization failed".to_string())?;
    fields.insert("registryId".into(), Json::string(registry_id));
    fields.insert(
        "scenarioNames".into(),
        Json::Array(
            state
                .definitions
                .iter()
                .map(|definition| Json::string(definition.name.clone()))
                .collect(),
        ),
    );
    fields.insert(
        "activeWaiverIds".into(),
        Json::Array(
            state
                .registry
                .waivers()
                .keys()
                .map(|id| Json::string(id.clone()))
                .collect(),
        ),
    );
    fields.insert(
        "audit".into(),
        Json::string(write_timing_waiver_audit(state.registry.audit())?),
    );
    if let Some(update) = update {
        fields.insert("update".into(), update);
    }
    if let Some(cache_hit) = cache_hit {
        fields.insert("cacheHit".into(), Json::Bool(cache_hit));
    }
    fields.insert(
        "mappedStats".into(),
        Json::object([
            (
                "cells",
                Json::number(state.flow.netlist.cell_count() as f64)?,
            ),
            ("nets", Json::number(state.flow.netlist.net_count() as f64)?),
            ("libCells", Json::number(state.flow.stats.lib_cells as f64)?),
        ]),
    );
    Ok(Json::Object(fields))
}

fn timing_waiver_registry_create(
    params: &Json,
    cancellation: &FlowCancellation,
    flow_cache: &SharedFlowCache,
    registries: &SharedTimingViolationRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing waiver registry creation cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    if registry_id.trim().is_empty() {
        return Err("timing waiver registry ID cannot be empty".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let scenario_source = required_string(params, "scenarios")?;
    let definitions = parse_timing_scenarios(&scenario_source)?;
    if definitions.is_empty() {
        return Err("timing waiver registry requires at least one scenario".into());
    }
    {
        let entries = registries
            .lock()
            .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
        if entries.contains_key(&registry_id) {
            return Err(format!(
                "timing waiver registry '{registry_id}' already exists"
            ));
        }
        if entries.len() >= TIMING_WAIVER_REGISTRY_CAPACITY {
            return Err(format!(
                "timing waiver registry store is full ({TIMING_WAIVER_REGISTRY_CAPACITY} entries)"
            ));
        }
    }

    let library = CellLibrary::std();
    let key = FlowCacheKey::new(&source, top, &library);
    let cached = flow_cache.get_or_insert_with(&key, || {
        run_flow_with_library_cancellable(&source, top, &library, cancellation)
    })?;
    if cancellation.is_cancelled() {
        return Err("timing waiver registry creation cancelled".into());
    }
    let scenarios = definitions
        .iter()
        .map(|definition| TimingScenario {
            name: definition.name.as_str(),
            netlist: &cached.flow.netlist,
            constraints: &definition.constraints,
        })
        .collect::<Vec<_>>();
    let registry = timing_violation_registry(scenarios.iter().copied())?;
    let state = TimingViolationRegistryState {
        flow: cached.flow,
        definitions,
        registry,
    };
    {
        let mut entries = registries
            .lock()
            .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
        if entries.contains_key(&registry_id) {
            return Err(format!(
                "timing waiver registry '{registry_id}' already exists"
            ));
        }
        if entries.len() >= TIMING_WAIVER_REGISTRY_CAPACITY {
            return Err(format!(
                "timing waiver registry store is full ({TIMING_WAIVER_REGISTRY_CAPACITY} entries)"
            ));
        }
        entries.insert(registry_id.clone(), state);
    }
    let entries = registries
        .lock()
        .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get(&registry_id)
        .ok_or_else(|| "internal error: timing waiver registry vanished".to_string())?;
    timing_waiver_registry_response(&registry_id, state, None, Some(cached.cache_hit))
}

fn timing_waiver_registry_get(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedTimingViolationRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing waiver registry lookup cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let entries = registries
        .lock()
        .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get(&registry_id)
        .ok_or_else(|| format!("timing waiver registry '{registry_id}' does not exist"))?;
    timing_waiver_registry_response(&registry_id, state, None, None)
}

fn timing_waiver_registry_list(
    cancellation: &FlowCancellation,
    registries: &SharedTimingViolationRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing waiver registry listing cancelled".into());
    }
    let entries = registries
        .lock()
        .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
    let registry_summaries = entries
        .iter()
        .map(|(id, state)| {
            Ok(Json::object([
                ("registryId", Json::string(id.clone())),
                ("generation", Json::from(state.registry.generation())),
                ("scenarioCount", Json::from(state.definitions.len() as u64)),
                (
                    "activeViolationCount",
                    Json::from(state.registry.len() as u64),
                ),
                (
                    "activeWaiverCount",
                    Json::from(state.registry.waivers().len() as u64),
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("count", Json::from(entries.len() as u64)),
        (
            "capacity",
            Json::from(TIMING_WAIVER_REGISTRY_CAPACITY as u64),
        ),
        ("registries", Json::Array(registry_summaries)),
    ]))
}

fn timing_waiver_registry_add_waivers(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedTimingViolationRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing waiver registry update cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let waiver_source = required_string(params, "waivers")?;
    let waivers = parse_timing_violation_waivers(&waiver_source)?;
    if waivers.is_empty() {
        return Err("timing waiver registry update requires at least one waiver".into());
    }
    let actor = match params.as_object().and_then(|fields| fields.get("actor")) {
        None | Some(Json::Null) => None,
        Some(Json::String(actor)) => Some(actor.clone()),
        Some(_) => return Err("RPC parameter 'actor' must be a string".into()),
    };
    let mut entries = registries
        .lock()
        .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get_mut(&registry_id)
        .ok_or_else(|| format!("timing waiver registry '{registry_id}' does not exist"))?;
    for waiver in waivers {
        add_timing_violation_registry_waiver_with_actor(
            &mut state.registry,
            waiver,
            actor.as_deref(),
        )?;
    }
    timing_waiver_registry_response(&registry_id, state, None, None)
}

fn timing_waiver_registry_remove_waiver(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedTimingViolationRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing waiver registry update cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let waiver_id = required_string(params, "waiverId")?;
    let actor = match params.as_object().and_then(|fields| fields.get("actor")) {
        None | Some(Json::Null) => None,
        Some(Json::String(actor)) => Some(actor.clone()),
        Some(_) => return Err("RPC parameter 'actor' must be a string".into()),
    };
    let mut entries = registries
        .lock()
        .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get_mut(&registry_id)
        .ok_or_else(|| format!("timing waiver registry '{registry_id}' does not exist"))?;
    remove_timing_violation_registry_waiver_with_actor(
        &mut state.registry,
        &waiver_id,
        actor.as_deref(),
    )?;
    timing_waiver_registry_response(&registry_id, state, None, None)
}

fn timing_waiver_registry_update_scenarios(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedTimingViolationRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing waiver registry scenario update cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let scenario_source = required_string(params, "scenarios")?;
    let definitions = parse_timing_scenarios(&scenario_source)?;
    if definitions.is_empty() {
        return Err("timing waiver registry requires at least one scenario".into());
    }
    let mut entries = registries
        .lock()
        .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get_mut(&registry_id)
        .ok_or_else(|| format!("timing waiver registry '{registry_id}' does not exist"))?;
    let scenarios = definitions
        .iter()
        .map(|definition| TimingScenario {
            name: definition.name.as_str(),
            netlist: &state.flow.netlist,
            constraints: &definition.constraints,
        })
        .collect::<Vec<_>>();
    let update = update_timing_violation_registry(&mut state.registry, scenarios.iter().copied())?;
    if cancellation.is_cancelled() {
        return Err("timing waiver registry scenario update cancelled".into());
    }
    state.definitions = definitions;
    timing_waiver_registry_response(
        &registry_id,
        state,
        Some(timing_violation_update_json(&update)?),
        None,
    )
}

fn timing_waiver_registry_delete(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedTimingViolationRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing waiver registry deletion cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let mut entries = registries
        .lock()
        .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
    entries
        .remove(&registry_id)
        .ok_or_else(|| format!("timing waiver registry '{registry_id}' does not exist"))?;
    Ok(Json::object([
        ("registryId", Json::string(registry_id)),
        ("deleted", Json::Bool(true)),
        ("remaining", Json::from(entries.len() as u64)),
    ]))
}

fn timing_violation_change_json(change: &eda_api::TimingViolationChange) -> Result<Json, String> {
    Ok(Json::object([
        ("id", Json::string(change.id.clone())),
        ("oldSlackNs", Json::number(change.old_slack_ns)?),
        ("newSlackNs", Json::number(change.new_slack_ns)?),
        ("current", timing_violation_json(&change.current)?),
    ]))
}

fn timing_violation_diff_json(diff: &eda_api::TimingViolationDiff) -> Result<Json, String> {
    let map_violations = |values: &BTreeMap<String, eda_api::TimingViolation>| {
        values
            .values()
            .map(timing_violation_json)
            .collect::<Result<Vec<_>, String>>()
    };
    let map_changes = |values: &BTreeMap<String, eda_api::TimingViolationChange>| {
        values
            .values()
            .map(timing_violation_change_json)
            .collect::<Result<Vec<_>, String>>()
    };
    Ok(Json::object([
        ("added", Json::Array(map_violations(&diff.added)?)),
        ("removed", Json::Array(map_violations(&diff.removed)?)),
        ("improved", Json::Array(map_changes(&diff.improved)?)),
        ("degraded", Json::Array(map_changes(&diff.degraded)?)),
        ("unchanged", Json::Array(map_violations(&diff.unchanged)?)),
        ("addedCount", Json::from(diff.added.len() as u64)),
        ("removedCount", Json::from(diff.removed.len() as u64)),
        ("improvedCount", Json::from(diff.improved.len() as u64)),
        ("degradedCount", Json::from(diff.degraded.len() as u64)),
        ("unchangedCount", Json::from(diff.unchanged.len() as u64)),
    ]))
}

fn cross_corner_timing_violation_change_json(
    change: &eda_api::CrossCornerTimingViolationChange,
) -> Result<Json, String> {
    Ok(Json::object([
        ("key", Json::string(change.current.key.clone())),
        (
            "oldWorstScenario",
            Json::string(change.old_worst_scenario.clone()),
        ),
        ("oldWorstSlackNs", Json::number(change.old_worst_slack_ns)?),
        (
            "current",
            cross_corner_timing_violation_json(&change.current)?,
        ),
    ]))
}

fn cross_corner_timing_violation_diff_json(
    diff: &eda_api::CrossCornerTimingViolationDiff,
) -> Result<Json, String> {
    let map_summaries = |values: &BTreeMap<String, eda_api::CrossCornerTimingViolation>| {
        values
            .values()
            .map(cross_corner_timing_violation_json)
            .collect::<Result<Vec<_>, String>>()
    };
    let map_changes = |values: &BTreeMap<String, eda_api::CrossCornerTimingViolationChange>| {
        values
            .values()
            .map(cross_corner_timing_violation_change_json)
            .collect::<Result<Vec<_>, String>>()
    };
    Ok(Json::object([
        ("added", Json::Array(map_summaries(&diff.added)?)),
        ("removed", Json::Array(map_summaries(&diff.removed)?)),
        ("improved", Json::Array(map_changes(&diff.improved)?)),
        ("degraded", Json::Array(map_changes(&diff.degraded)?)),
        ("unchanged", Json::Array(map_summaries(&diff.unchanged)?)),
        ("addedCount", Json::from(diff.added.len() as u64)),
        ("removedCount", Json::from(diff.removed.len() as u64)),
        ("improvedCount", Json::from(diff.improved.len() as u64)),
        ("degradedCount", Json::from(diff.degraded.len() as u64)),
        ("unchangedCount", Json::from(diff.unchanged.len() as u64)),
    ]))
}

fn timing_violation_update_json(
    update: &eda_api::TimingViolationRegistryUpdate,
) -> Result<Json, String> {
    let added = update
        .added_scenarios
        .iter()
        .map(|(name, store)| {
            Ok(Json::object([
                ("name", Json::string(name.clone())),
                ("store", timing_violation_store_json(store)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let removed = update
        .removed_scenarios
        .iter()
        .map(|(name, store)| {
            Ok(Json::object([
                ("name", Json::string(name.clone())),
                ("store", timing_violation_store_json(store)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let scenario_diffs = update
        .scenario_diffs
        .iter()
        .map(|(name, diff)| Ok((name.clone(), timing_violation_diff_json(diff)?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    Ok(Json::object([
        ("addedScenarios", Json::Array(added)),
        ("removedScenarios", Json::Array(removed)),
        ("scenarioDiffs", Json::Object(scenario_diffs)),
        (
            "crossCorner",
            cross_corner_timing_violation_diff_json(&update.cross_corner)?,
        ),
        (
            "addedScenarioCount",
            Json::from(update.added_scenarios.len() as u64),
        ),
        (
            "removedScenarioCount",
            Json::from(update.removed_scenarios.len() as u64),
        ),
        (
            "scenarioDiffCount",
            Json::from(update.scenario_diffs.len() as u64),
        ),
    ]))
}

fn rtl_incremental_mmmc_timing_violations(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL incremental MMMC timing violations cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let old_source = required_string(params, "old")?;
    let new_source = required_string(params, "new")?;
    let old = parse_timing_scenarios(&old_source)?;
    let new = parse_timing_scenarios(&new_source)?;
    if old.is_empty() || new.is_empty() {
        return Err("RTL incremental MMMC timing violations require old and new scenarios".into());
    }
    let waivers = match params.as_object().and_then(|fields| fields.get("waivers")) {
        None | Some(Json::Null) => Vec::new(),
        Some(Json::String(source)) => parse_timing_violation_waivers(source)?,
        Some(_) => return Err("RPC parameter 'waivers' must be a string".into()),
    };
    let actor = match params.as_object().and_then(|fields| fields.get("actor")) {
        None | Some(Json::Null) => None,
        Some(Json::String(actor)) => Some(actor.clone()),
        Some(_) => return Err("RPC parameter 'actor' must be a string".into()),
    };

    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL incremental MMMC timing violations cancelled".into());
    }
    let old_scenarios = old
        .iter()
        .map(|definition| TimingScenario {
            name: definition.name.as_str(),
            netlist: &flow.netlist,
            constraints: &definition.constraints,
        })
        .collect::<Vec<_>>();
    let mut registry = timing_violation_registry(old_scenarios.iter().copied())?;
    if cancellation.is_cancelled() {
        return Err("RTL incremental MMMC timing violations cancelled".into());
    }
    for waiver in waivers {
        add_timing_violation_registry_waiver_with_actor(&mut registry, waiver, actor.as_deref())?;
        if cancellation.is_cancelled() {
            return Err("RTL incremental MMMC timing violations cancelled".into());
        }
    }
    let baseline_report = timing_violation_registry_report(&registry, None);

    let new_scenarios = new
        .iter()
        .map(|definition| TimingScenario {
            name: definition.name.as_str(),
            netlist: &flow.netlist,
            constraints: &definition.constraints,
        })
        .collect::<Vec<_>>();
    let update = update_timing_violation_registry(&mut registry, new_scenarios.iter().copied())?;
    if cancellation.is_cancelled() {
        return Err("RTL incremental MMMC timing violations cancelled".into());
    }
    let update_report = timing_violation_registry_report(&registry, Some(&update));
    let mut fields = timing_violation_registry_json(&registry)?
        .as_object()
        .cloned()
        .ok_or_else(|| "timing violation registry serialization failed".to_string())?;
    fields.insert("oldScenarioCount".into(), Json::from(old.len() as u64));
    fields.insert("newScenarioCount".into(), Json::from(new.len() as u64));
    fields.insert("baselineReport".into(), Json::string(baseline_report));
    fields.insert("update".into(), timing_violation_update_json(&update)?);
    fields.insert("updateReport".into(), Json::string(update_report));
    fields.insert(
        "mappedStats".into(),
        Json::object([
            ("cells", Json::number(flow.netlist.cell_count() as f64)?),
            ("nets", Json::number(flow.netlist.net_count() as f64)?),
            ("libCells", Json::number(flow.stats.lib_cells as f64)?),
        ]),
    );
    Ok(Json::Object(fields))
}

fn rtl_mmmc_timing_violations(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing violations cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let scenario_source = required_string(params, "scenarios")?;
    let definitions = parse_timing_scenarios(&scenario_source)?;
    if definitions.is_empty() {
        return Err("RTL MMMC timing violations require at least one scenario".into());
    }
    let waivers = match params.as_object().and_then(|fields| fields.get("waivers")) {
        None | Some(Json::Null) => Vec::new(),
        Some(Json::String(source)) => parse_timing_violation_waivers(source)?,
        Some(_) => return Err("RPC parameter 'waivers' must be a string".into()),
    };
    let actor = match params.as_object().and_then(|fields| fields.get("actor")) {
        None | Some(Json::Null) => None,
        Some(Json::String(actor)) => Some(actor.clone()),
        Some(_) => return Err("RPC parameter 'actor' must be a string".into()),
    };

    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing violations cancelled".into());
    }
    let scenarios = definitions
        .iter()
        .map(|definition| TimingScenario {
            name: definition.name.as_str(),
            netlist: &flow.netlist,
            constraints: &definition.constraints,
        })
        .collect::<Vec<_>>();
    let mut registry = timing_violation_registry(scenarios.iter().copied())?;
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing violations cancelled".into());
    }
    for waiver in waivers {
        add_timing_violation_registry_waiver_with_actor(&mut registry, waiver, actor.as_deref())?;
        if cancellation.is_cancelled() {
            return Err("RTL MMMC timing violations cancelled".into());
        }
    }

    let scenario_results = registry
        .stores()
        .iter()
        .map(|(name, store)| {
            let violations = store
                .active_violations()
                .values()
                .map(timing_violation_json)
                .collect::<Result<Vec<_>, String>>()?;
            let waived = store
                .waived_violations()
                .iter()
                .map(|(violation, waiver)| {
                    (
                        violation.clone(),
                        Json::object([
                            ("violation", Json::string(violation.clone())),
                            ("waiver", Json::string(waiver.clone())),
                        ]),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let waivers = store
                .waivers()
                .values()
                .map(timing_waiver_json)
                .collect::<Result<Vec<_>, String>>()?;
            Ok((
                name.clone(),
                Json::object([
                    ("scenario", Json::string(store.scenario.clone())),
                    ("generation", Json::from(store.generation)),
                    ("activeViolations", Json::Array(violations)),
                    ("activeViolationCount", Json::from(store.len() as u64)),
                    ("waivedViolations", Json::Object(waived)),
                    (
                        "waivedViolationCount",
                        Json::from(store.waived_len() as u64),
                    ),
                    ("waivers", Json::Array(waivers)),
                    (
                        "waiverAuditReport",
                        Json::string(timing_waiver_audit_report(store.audit())),
                    ),
                ]),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;

    let summaries = registry
        .summaries()
        .values()
        .map(|summary| {
            let scenarios = summary
                .scenarios
                .iter()
                .map(|(scenario, violation)| -> Result<(String, Json), String> {
                    Ok((
                        scenario.clone(),
                        Json::object([
                            ("id", Json::string(violation.id.clone())),
                            ("slackNs", Json::number(violation.slack_ns)?),
                        ]),
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()?;
            let hierarchy = summary
                .hierarchy_summary
                .iter()
                .map(|(owner, values)| -> Result<(String, Json), String> {
                    let key = owner.clone().unwrap_or_else(|| "(top)".to_string());
                    let kinds = values
                        .kinds
                        .iter()
                        .map(|(kind, count)| (kind.label().to_string(), Json::from(*count as u64)))
                        .collect::<BTreeMap<_, _>>();
                    Ok((
                        key,
                        Json::object([
                            (
                                "hierarchy",
                                values.hierarchy.clone().map_or(Json::Null, Json::string),
                            ),
                            ("violations", Json::from(values.violations as u64)),
                            ("worstSlackNs", Json::number(values.worst_slack_ns)?),
                            ("worstScenario", Json::string(values.worst_scenario.clone())),
                            ("kinds", Json::Object(kinds)),
                        ]),
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()?;
            Ok(Json::object([
                ("key", Json::string(summary.key.clone())),
                ("kind", Json::string(summary.kind.label().to_string())),
                ("endpoint", Json::string(summary.endpoint.clone())),
                ("scenarios", Json::Object(scenarios)),
                ("hierarchy", Json::Object(hierarchy)),
                (
                    "worstScenario",
                    Json::string(summary.worst_scenario.clone()),
                ),
                ("worstSlackNs", Json::number(summary.worst_slack_ns)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let hierarchy_summaries = registry
        .hierarchy_summaries()
        .into_iter()
        .map(|(owner, summary)| -> Result<(String, Json), String> {
            let key = owner.unwrap_or_else(|| "(top)".to_string());
            let kinds = summary
                .kinds
                .iter()
                .map(|(kind, count)| (kind.label().to_string(), Json::from(*count as u64)))
                .collect::<BTreeMap<_, _>>();
            Ok((
                key,
                Json::object([
                    (
                        "hierarchy",
                        summary.hierarchy.clone().map_or(Json::Null, Json::string),
                    ),
                    ("violations", Json::from(summary.violations as u64)),
                    ("worstSlackNs", Json::number(summary.worst_slack_ns)?),
                    (
                        "worstScenario",
                        Json::string(summary.worst_scenario.clone()),
                    ),
                    ("kinds", Json::Object(kinds)),
                ]),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;

    Ok(Json::object([
        ("scenarioCount", Json::from(registry.stores().len() as u64)),
        ("generation", Json::from(registry.generation())),
        ("activeViolationCount", Json::from(registry.len() as u64)),
        ("hasActiveViolations", Json::Bool(!registry.is_empty())),
        ("scenarios", Json::Object(scenario_results)),
        ("summaries", Json::Array(summaries)),
        ("hierarchySummaries", Json::Object(hierarchy_summaries)),
        (
            "report",
            Json::string(timing_violation_registry_report(&registry, None)),
        ),
        (
            "mappedStats",
            Json::object([
                ("cells", Json::number(flow.netlist.cell_count() as f64)?),
                ("nets", Json::number(flow.netlist.net_count() as f64)?),
                ("libCells", Json::number(flow.stats.lib_cells as f64)?),
            ]),
        ),
    ]))
}

fn rtl_mmmc_timing(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing analysis cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let scenario_source = required_string(params, "scenarios")?;
    let definitions = parse_timing_scenarios(&scenario_source)?;
    if definitions.is_empty() {
        return Err("RTL MMMC timing requires at least one scenario".into());
    }
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing analysis cancelled".into());
    }
    let result = analyze_timing_scenarios_from_definitions(&flow.netlist, &definitions)?;
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing analysis cancelled".into());
    }

    let optional_number = |value: Option<f64>| -> Result<Json, String> {
        value.map_or(Ok(Json::Null), |value| {
            if value.is_finite() {
                Json::number(value)
            } else {
                Ok(Json::Null)
            }
        })
    };
    let corners = result
        .corners
        .iter()
        .map(|corner| {
            Ok(Json::object([
                ("scenario", Json::string(corner.scenario.clone())),
                ("clock", Json::string(corner.timing.clock.clone())),
                (
                    "criticalPathDelayNs",
                    optional_number(Some(corner.timing.base.critical_path_delay))?,
                ),
                ("periodNs", optional_number(Some(corner.timing.period))?),
                (
                    "worstSlackNs",
                    optional_number(Some(corner.timing.worst_slack))?,
                ),
                (
                    "setupSlackNs",
                    optional_number(Some(corner.timing.reg2reg_slack))?,
                ),
                (
                    "holdSlackNs",
                    optional_number(Some(corner.timing.reg2reg_hold_slack))?,
                ),
                (
                    "outputSlackNs",
                    optional_number(Some(corner.timing.output_slack))?,
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let scenario_properties = definitions
        .iter()
        .map(|definition| {
            (
                definition.name.clone(),
                Json::Object(
                    definition
                        .properties
                        .iter()
                        .map(|(key, value)| (key.clone(), Json::string(value.clone())))
                        .collect(),
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();

    Ok(Json::object([
        ("scenarioCount", Json::number(result.corners.len() as f64)?),
        (
            "worstScenario",
            result.worst_scenario().map_or(Json::Null, Json::string),
        ),
        (
            "worstSetupSlackNs",
            optional_number(result.worst_setup_slack())?,
        ),
        (
            "worstHoldSlackNs",
            optional_number(result.worst_hold_slack())?,
        ),
        ("corners", Json::Array(corners)),
        ("properties", Json::Object(scenario_properties)),
        (
            "mappedStats",
            Json::object([
                ("cells", Json::number(flow.netlist.cell_count() as f64)?),
                ("nets", Json::number(flow.netlist.net_count() as f64)?),
                ("libCells", Json::number(flow.stats.lib_cells as f64)?),
            ]),
        ),
        ("report", Json::string(multi_corner_timing_report(&result))),
    ]))
}

fn rtl_incremental_mmmc_timing(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL incremental MMMC timing cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let old_source = required_string(params, "old")?;
    let new_source = required_string(params, "new")?;
    let old = parse_timing_scenarios(&old_source)?;
    let new = parse_timing_scenarios(&new_source)?;
    if old.is_empty() || new.is_empty() {
        return Err("RTL incremental MMMC timing requires old and new scenarios".into());
    }
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL incremental MMMC timing cancelled".into());
    }
    let previous = analyze_timing_scenarios_from_definitions(&flow.netlist, &old)?;
    if cancellation.is_cancelled() {
        return Err("RTL incremental MMMC timing cancelled".into());
    }
    let incremental = update_timing_scenarios_incrementally(&flow.netlist, &previous, &old, &new)?;
    if cancellation.is_cancelled() {
        return Err("RTL incremental MMMC timing cancelled".into());
    }
    let diff = &incremental.diff;

    let finite_number = |value: f64| -> Result<Json, String> {
        if value.is_finite() {
            Json::number(value)
        } else {
            Ok(Json::Null)
        }
    };
    let optional_number = |value: Option<f64>| -> Result<Json, String> {
        value.map_or(Ok(Json::Null), finite_number)
    };
    let updates = incremental
        .updates
        .iter()
        .map(|(scenario, update)| {
            Ok((
                scenario.clone(),
                Json::object([
                    (
                        "recomputedNets",
                        string_array(update.recomputed_nets.iter().cloned()),
                    ),
                    (
                        "recomputedCells",
                        string_array(update.recomputed_cells.iter().cloned()),
                    ),
                    (
                        "recomputedNetCount",
                        Json::from(update.recomputed_nets.len() as u64),
                    ),
                    (
                        "recomputedCellCount",
                        Json::from(update.recomputed_cells.len() as u64),
                    ),
                    ("totalCells", Json::from(update.total_cells as u64)),
                    ("fullBaseRecompute", Json::Bool(update.full_base_recompute)),
                    (
                        "criticalPathDelayNs",
                        finite_number(update.timing.critical_path_delay)?,
                    ),
                    (
                        "worstSlackNs",
                        finite_number(update.constrained.worst_slack)?,
                    ),
                    ("constraintDiff", timing_constraint_diff_json(&update.diff)),
                ]),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let corners = incremental
        .result
        .corners
        .iter()
        .map(|corner| {
            Ok(Json::object([
                ("scenario", Json::string(corner.scenario.clone())),
                ("clock", Json::string(corner.timing.clock.clone())),
                ("periodNs", finite_number(corner.timing.period)?),
                (
                    "criticalPathDelayNs",
                    finite_number(corner.timing.base.critical_path_delay)?,
                ),
                ("worstSlackNs", finite_number(corner.timing.worst_slack)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(Json::object([
        ("oldScenarioCount", Json::from(old.len() as u64)),
        ("newScenarioCount", Json::from(new.len() as u64)),
        (
            "baselineWorstScenario",
            previous.worst_scenario().map_or(Json::Null, Json::string),
        ),
        (
            "finalWorstScenario",
            incremental
                .result
                .worst_scenario()
                .map_or(Json::Null, Json::string),
        ),
        (
            "finalWorstSetupSlackNs",
            optional_number(incremental.result.worst_setup_slack())?,
        ),
        (
            "finalWorstHoldSlackNs",
            optional_number(incremental.result.worst_hold_slack())?,
        ),
        ("added", string_array(diff.added.iter().cloned())),
        ("removed", string_array(diff.removed.iter().cloned())),
        ("changed", string_array(diff.changed.iter().cloned())),
        (
            "timingChanged",
            string_array(diff.timing_changed.iter().cloned()),
        ),
        ("reused", string_array(incremental.reused.iter().cloned())),
        ("updates", Json::Object(updates)),
        ("corners", Json::Array(corners)),
        (
            "diffReport",
            Json::string(timing_scenario_diff_report(diff)),
        ),
        (
            "finalReport",
            Json::string(multi_corner_timing_report(&incremental.result)),
        ),
        (
            "mappedStats",
            Json::object([
                ("cells", Json::number(flow.netlist.cell_count() as f64)?),
                ("nets", Json::number(flow.netlist.net_count() as f64)?),
                ("libCells", Json::number(flow.stats.lib_cells as f64)?),
            ]),
        ),
    ]))
}

fn finite_json(value: f64) -> Result<Json, String> {
    if value.is_finite() {
        Json::number(value)
    } else {
        Ok(Json::Null)
    }
}

fn optional_finite_json(value: Option<f64>) -> Result<Json, String> {
    value.map_or(Ok(Json::Null), finite_json)
}

fn timing_object_json(kind: &str, name: &str) -> Json {
    Json::object([("kind", Json::string(kind)), ("name", Json::string(name))])
}

fn timing_launch_json(launch: &eda_api::TimingLaunch) -> Json {
    match launch {
        eda_api::TimingLaunch::PrimaryInput(name) => timing_object_json("primaryInput", name),
        eda_api::TimingLaunch::SequentialCell(name) => timing_object_json("sequentialCell", name),
    }
}

fn timing_endpoint_json(endpoint: &eda_api::TimingPathEndpoint) -> Json {
    match endpoint {
        eda_api::TimingPathEndpoint::SequentialCell(name) => {
            timing_object_json("sequentialCell", name)
        }
        eda_api::TimingPathEndpoint::OutputPort(name) => timing_object_json("outputPort", name),
    }
}

fn timing_path_kind_json(kind: eda_api::TimingPathKind) -> Json {
    Json::string(match kind {
        eda_api::TimingPathKind::InputToSequential => "inputToSequential",
        eda_api::TimingPathKind::SequentialToSequential => "sequentialToSequential",
        eda_api::TimingPathKind::InputToOutput => "inputToOutput",
        eda_api::TimingPathKind::SequentialToOutput => "sequentialToOutput",
    })
}

fn timing_arc_kind_json(kind: eda_api::TimingArcKind) -> Json {
    Json::string(match kind {
        eda_api::TimingArcKind::PrimaryInputLaunch => "primaryInputLaunch",
        eda_api::TimingArcKind::ClockToOutput => "clockToOutput",
        eda_api::TimingArcKind::Combinational => "combinational",
    })
}

fn timing_path_json(nl: &Netlist, path: &eda_api::TimingPath) -> Result<Json, String> {
    let cell_names = path
        .cells
        .iter()
        .map(|&cell| Json::string(nl.cell(cell).name.clone()))
        .collect::<Vec<_>>();
    let net_names = path
        .nets
        .iter()
        .map(|&net| Json::string(nl.net(net).name.clone()))
        .collect::<Vec<_>>();
    let arcs = path
        .arcs
        .iter()
        .map(|arc| {
            Ok(Json::object([
                ("kind", timing_arc_kind_json(arc.kind)),
                (
                    "cell",
                    arc.cell
                        .map(|cell| Json::from(cell as i64))
                        .unwrap_or(Json::Null),
                ),
                (
                    "cellName",
                    arc.cell
                        .map(|cell| Json::string(nl.cell(cell).name.clone()))
                        .unwrap_or(Json::Null),
                ),
                (
                    "fromNet",
                    arc.from_net
                        .map(|net| Json::from(net as i64))
                        .unwrap_or(Json::Null),
                ),
                (
                    "fromNetName",
                    arc.from_net
                        .map(|net| Json::string(nl.net(net).name.clone()))
                        .unwrap_or(Json::Null),
                ),
                ("toNet", Json::from(arc.to_net as i64)),
                ("toNetName", Json::string(nl.net(arc.to_net).name.clone())),
                ("fromArrivalNs", optional_finite_json(arc.from_arrival_ns)?),
                ("toArrivalNs", finite_json(arc.to_arrival_ns)?),
                ("delayNs", finite_json(arc.delay_ns)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("kind", timing_path_kind_json(path.kind)),
        ("launch", timing_launch_json(&path.launch)),
        ("endpoint", timing_endpoint_json(&path.endpoint)),
        (
            "cellIds",
            Json::Array(
                path.cells
                    .iter()
                    .map(|cell| Json::from(*cell as i64))
                    .collect(),
            ),
        ),
        ("cellNames", Json::Array(cell_names)),
        (
            "netIds",
            Json::Array(
                path.nets
                    .iter()
                    .map(|net| Json::from(*net as i64))
                    .collect(),
            ),
        ),
        ("netNames", Json::Array(net_names)),
        ("arcs", Json::Array(arcs)),
        ("arrivalNs", finite_json(path.arrival_ns)?),
    ]))
}

fn corner_slacks_json(slacks: &[eda_api::ScenarioSlack]) -> Result<Json, String> {
    let values = slacks
        .iter()
        .map(|slack| {
            Ok(Json::object([
                ("scenario", Json::string(slack.scenario.clone())),
                ("slackNs", optional_finite_json(slack.slack_ns)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::Array(values))
}

fn rtl_mmmc_timing_paths(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing paths cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let scenario_source = required_string(params, "scenarios")?;
    let Some(fields) = params.as_object() else {
        return Err("params must be an object".into());
    };
    let max_paths = match fields.get("maxPaths") {
        None => 10,
        Some(Json::Number(value)) => {
            if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
                return Err("RPC parameter 'maxPaths' must be a positive integer".into());
            }
            *value as usize
        }
        Some(_) => return Err("RPC parameter 'maxPaths' must be a positive integer".into()),
    };
    let definitions = parse_timing_scenarios(&scenario_source)?;
    if definitions.is_empty() {
        return Err("RTL MMMC timing paths require at least one scenario".into());
    }
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing paths cancelled".into());
    }
    let scenarios = definitions
        .iter()
        .map(|definition| TimingScenario {
            name: definition.name.as_str(),
            netlist: &flow.netlist,
            constraints: &definition.constraints,
        })
        .collect::<Vec<_>>();
    let result = analyze_timing_scenarios_from_definitions(&flow.netlist, &definitions)?;
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing paths cancelled".into());
    }

    let setup_paths =
        multi_corner_setup_timing_paths(&result, scenarios.iter().copied(), max_paths)?;
    let hold_paths = multi_corner_hold_timing_paths(&result, scenarios.iter().copied(), max_paths)?;
    let reset_paths =
        multi_corner_reset_timing_paths(&result, scenarios.iter().copied(), max_paths)?;
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing paths cancelled".into());
    }

    let setup = setup_paths
        .iter()
        .map(|path| {
            Ok(Json::object([
                ("endpoint", timing_endpoint_json(&path.endpoint)),
                ("scenario", Json::string(path.scenario.clone())),
                ("slackNs", finite_json(path.path.slack_ns)?),
                ("requiredNs", finite_json(path.path.required_ns)?),
                ("path", timing_path_json(path.netlist, &path.path.path)?),
                ("cornerSlacks", corner_slacks_json(&path.corner_slacks)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let hold = hold_paths
        .iter()
        .map(|path| {
            Ok(Json::object([
                ("endpoint", timing_endpoint_json(&path.endpoint)),
                ("scenario", Json::string(path.scenario.clone())),
                ("slackNs", finite_json(path.path.slack_ns)?),
                ("requiredNs", finite_json(path.path.required_ns)?),
                ("holdNs", finite_json(path.path.path.hold_ns)?),
                (
                    "path",
                    timing_path_json(path.netlist, &path.path.path.path)?,
                ),
                ("cornerSlacks", corner_slacks_json(&path.corner_slacks)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let reset = reset_paths
        .iter()
        .map(|path| {
            Ok(Json::object([
                ("endpoint", timing_endpoint_json(&path.endpoint)),
                (
                    "check",
                    Json::string(match path.check {
                        eda_api::ResetCheckKind::Recovery => "recovery",
                        eda_api::ResetCheckKind::Removal => "removal",
                    }),
                ),
                ("scenario", Json::string(path.scenario.clone())),
                ("slackNs", finite_json(path.path.slack_ns)?),
                ("requiredNs", finite_json(path.path.required_ns)?),
                ("path", timing_path_json(path.netlist, &path.path.path)?),
                ("cornerSlacks", corner_slacks_json(&path.corner_slacks)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(Json::object([
        ("scenarioCount", Json::from(definitions.len() as u64)),
        ("maxPaths", Json::from(max_paths as u64)),
        (
            "worstScenario",
            result.worst_scenario().map_or(Json::Null, Json::string),
        ),
        ("setupPaths", Json::Array(setup)),
        ("holdPaths", Json::Array(hold)),
        ("resetPaths", Json::Array(reset)),
        (
            "setupReport",
            Json::string(multi_corner_setup_timing_path_report(&setup_paths)),
        ),
        (
            "holdReport",
            Json::string(multi_corner_hold_timing_path_report(&hold_paths)),
        ),
        (
            "resetReport",
            Json::string(multi_corner_reset_timing_path_report(&reset_paths)),
        ),
    ]))
}

fn parse_drc_options(params: &Json) -> Result<eda_api::DrcOptions, String> {
    let Some(options) = params
        .as_object()
        .and_then(|fields| fields.get("drcOptions"))
    else {
        return Ok(eda_api::DrcOptions::default());
    };
    let fields = options
        .as_object()
        .ok_or_else(|| "'drcOptions' must be an object".to_string())?;
    let max_antenna_ratio = match fields.get("maxAntennaRatio") {
        None | Some(Json::Null) => eda_api::DrcOptions::default().max_antenna_ratio,
        Some(Json::Number(value)) => *value,
        Some(_) => return Err("DRC option 'maxAntennaRatio' must be a number".into()),
    };
    let options = eda_api::DrcOptions { max_antenna_ratio };
    options.validate()?;
    Ok(options)
}

fn rtl_pnr(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let drc_options = parse_drc_options(params)?;
    let result = if drc_options == eda_api::DrcOptions::default() {
        implement_rtl_pnr_cancellable(&source, top, cancellation)?
    } else {
        implement_rtl_pnr_with_drc_options_cancellable(&source, top, &drc_options, cancellation)?
    };
    let wire_length = result
        .routing
        .segments
        .iter()
        .map(|segment| segment.length())
        .sum::<usize>();
    let drc_violations = result
        .drc
        .violations
        .iter()
        .map(|violation| {
            Ok(Json::object([
                ("code", Json::string(violation.code.clone())),
                (
                    "severity",
                    Json::string(match violation.severity {
                        eda_pnr::DrcSeverity::Warning => "warning",
                        eda_pnr::DrcSeverity::Error => "error",
                    }),
                ),
                ("message", Json::string(violation.message.clone())),
                (
                    "cell",
                    violation
                        .cell
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "net",
                    violation
                        .net
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "row",
                    violation
                        .row
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "site",
                    violation
                        .site
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        (
            "placement",
            Json::object([
                ("rows", Json::number(result.placement.rows as f64)?),
                (
                    "placedCells",
                    Json::number(result.placement.cell_pos.len() as f64)?,
                ),
                (
                    "blockages",
                    Json::number(result.placement.blockages.len() as f64)?,
                ),
            ]),
        ),
        (
            "routing",
            Json::object([
                (
                    "segments",
                    Json::number(result.routing.segments.len() as f64)?,
                ),
                ("vias", Json::number(result.routing.vias as f64)?),
                (
                    "explicitViaLocations",
                    Json::number(result.routing.via_locations.len() as f64)?,
                ),
                (
                    "tracksPerChannel",
                    Json::number(result.routing.tracks_per_channel as f64)?,
                ),
                (
                    "congestedChannels",
                    Json::number(result.routing.congested_channels.len() as f64)?,
                ),
                ("wireLength", Json::number(wire_length as f64)?),
            ]),
        ),
        (
            "drc",
            Json::object([
                (
                    "errors",
                    Json::number(
                        result
                            .drc
                            .violations
                            .iter()
                            .filter(|violation| violation.severity == eda_pnr::DrcSeverity::Error)
                            .count() as f64,
                    )?,
                ),
                (
                    "warnings",
                    Json::number(
                        result
                            .drc
                            .violations
                            .iter()
                            .filter(|violation| violation.severity == eda_pnr::DrcSeverity::Warning)
                            .count() as f64,
                    )?,
                ),
                ("violations", Json::Array(drc_violations)),
            ]),
        ),
        (
            "mappedStats",
            Json::object([
                ("nets", Json::number(result.mapped_stats.nets as f64)?),
                ("cells", Json::number(result.mapped_stats.cells as f64)?),
                ("comb", Json::number(result.mapped_stats.comb as f64)?),
                ("seq", Json::number(result.mapped_stats.seq as f64)?),
                ("consts", Json::number(result.mapped_stats.consts as f64)?),
                (
                    "libCells",
                    Json::number(result.mapped_stats.lib_cells as f64)?,
                ),
            ]),
        ),
        ("lef", Json::string(result.lef)),
        ("def", Json::string(result.def)),
    ]))
}

fn parse_antenna_repair_options(params: &Json) -> Result<eda_api::AntennaRepairOptions, String> {
    let Some(options) = params
        .as_object()
        .and_then(|fields| fields.get("antennaOptions"))
    else {
        return Ok(eda_api::AntennaRepairOptions::default());
    };
    let fields = options
        .as_object()
        .ok_or_else(|| "'antennaOptions' must be an object".to_string())?;
    let patch_name = match fields.get("patchName") {
        None | Some(Json::Null) => "antenna_repair".to_string(),
        Some(Json::String(value)) => value.clone(),
        Some(_) => return Err("antenna option 'patchName' must be a string".into()),
    };
    let max_diodes_per_net = match fields.get("maxDiodesPerNet") {
        None | Some(Json::Null) => 4_usize,
        Some(Json::Number(value)) => {
            if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
                return Err("antenna option 'maxDiodesPerNet' must be a positive usize".into());
            }
            *value as usize
        }
        Some(_) => return Err("antenna option 'maxDiodesPerNet' must be a positive usize".into()),
    };
    let mut route = eda_api::RouteOptions::default();
    if let Some(route_fields) = fields
        .get("routeOptions")
        .map(|value| {
            value
                .as_object()
                .ok_or_else(|| "antenna option 'routeOptions' must be an object".to_string())
        })
        .transpose()?
    {
        if let Some(value) = route_fields.get("tracksPerChannel") {
            let Json::Number(value) = value else {
                return Err("route option 'tracksPerChannel' must be a positive usize".into());
            };
            if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
                return Err("route option 'tracksPerChannel' must be a positive usize".into());
            }
            route.tracks_per_channel = *value as usize;
        }
        if let Some(value) = route_fields.get("allowCongestion") {
            let Json::Bool(value) = value else {
                return Err("route option 'allowCongestion' must be a boolean".into());
            };
            route.allow_congestion = *value;
        }
    }
    let options = eda_api::AntennaRepairOptions {
        patch_name,
        max_diodes_per_net,
        route,
    };
    options.validate()?;
    Ok(options)
}

fn pnr_drc_json(drc: &eda_api::DrcResult) -> Result<Json, String> {
    let violations = drc
        .violations
        .iter()
        .map(|violation| {
            Ok(Json::object([
                ("code", Json::string(violation.code.clone())),
                (
                    "severity",
                    Json::string(match violation.severity {
                        eda_pnr::DrcSeverity::Warning => "warning",
                        eda_pnr::DrcSeverity::Error => "error",
                    }),
                ),
                ("message", Json::string(violation.message.clone())),
                (
                    "cell",
                    violation
                        .cell
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "net",
                    violation
                        .net
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "row",
                    violation
                        .row
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "site",
                    violation
                        .site
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("errors", Json::from(drc.error_count() as u64)),
        ("warnings", Json::from(drc.warning_count() as u64)),
        ("violations", Json::Array(violations)),
    ]))
}

fn rtl_antenna_repair(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL antenna repair cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let options = parse_antenna_repair_options(params)?;
    let result = repair_rtl_antennas_cancellable(&source, top, &options, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL antenna repair cancelled".into());
    }
    let repairs = result
        .repair
        .repairs
        .iter()
        .map(|repair| {
            Ok(Json::object([
                ("net", Json::string(repair.net.clone())),
                ("oldRatio", Json::number(repair.old_ratio)?),
                ("newRatio", Json::number(repair.new_ratio)?),
                ("diodesInserted", Json::from(repair.diodes_inserted as u64)),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let wire_length = result
        .repair
        .routing
        .segments
        .iter()
        .map(|segment| segment.length())
        .sum::<usize>();
    Ok(Json::object([
        ("baselineDrc", pnr_drc_json(&result.baseline_drc)?),
        (
            "repairCount",
            Json::from(result.repair.repairs.len() as u64),
        ),
        (
            "diodesInserted",
            Json::from(result.repair.diodes_inserted as u64),
        ),
        (
            "antennaWarnings",
            Json::from(result.repair.antenna_warnings as u64),
        ),
        (
            "allAntennasRepaired",
            Json::Bool(result.repair.all_antennas_repaired),
        ),
        ("repairs", Json::Array(repairs)),
        (
            "placement",
            Json::object([
                (
                    "rows",
                    Json::number(result.repair.physical_placement.rows as f64)?,
                ),
                (
                    "placedCells",
                    Json::number(result.repair.physical_placement.cell_pos.len() as f64)?,
                ),
            ]),
        ),
        (
            "routing",
            Json::object([
                (
                    "segments",
                    Json::number(result.repair.routing.segments.len() as f64)?,
                ),
                ("vias", Json::number(result.repair.routing.vias as f64)?),
                (
                    "tracksPerChannel",
                    Json::number(result.repair.routing.tracks_per_channel as f64)?,
                ),
                ("wireLength", Json::number(wire_length as f64)?),
            ]),
        ),
        (
            "mappedStats",
            Json::object([
                ("nets", Json::number(result.final_mapped_stats.nets as f64)?),
                (
                    "cells",
                    Json::number(result.final_mapped_stats.cells as f64)?,
                ),
                (
                    "libCells",
                    Json::number(result.final_mapped_stats.lib_cells as f64)?,
                ),
            ]),
        ),
        (
            "report",
            Json::string(eda_api::antenna_repair_report(&result.repair)),
        ),
    ]))
}

fn rtl_sim(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let vectors = parse_rtl_simulation_vectors(params)?;
    let result = simulate_rtl_cancellable(&source, top, &vectors, cancellation)?;
    let samples = result
        .samples
        .iter()
        .map(|sample| {
            Ok(Json::object([
                ("timeNs", Json::number(sample.time_ns as f64)?),
                (
                    "outputs",
                    Json::Object(
                        sample
                            .outputs
                            .iter()
                            .map(|(name, value)| (name.clone(), Json::string(value.clone())))
                            .collect(),
                    ),
                ),
                (
                    "displays",
                    Json::Array(sample.displays.iter().map(Json::string).collect()),
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("samples", Json::Array(samples)),
        ("finished", Json::Bool(result.finished)),
        ("finalTimeNs", Json::number(result.final_time_ns as f64)?),
    ]))
}

fn rtl_power(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let vectors = parse_rtl_simulation_vectors(params)?;
    let options = parse_power_options(params)?;
    let result = analyze_rtl_power_cancellable(&source, top, &vectors, &options, cancellation)?;
    let activities = result
        .report
        .activity_by_net
        .iter()
        .map(|(net, activity)| {
            Ok((
                net.clone(),
                Json::object([
                    ("transitions", Json::number(activity.transitions as f64)?),
                    ("knownSamples", Json::number(activity.known_samples as f64)?),
                    ("oneSamples", Json::number(activity.one_samples as f64)?),
                    (
                        "toggleProbability",
                        Json::number(activity.toggle_probability(result.report.intervals as u64))?,
                    ),
                    (
                        "staticProbability",
                        Json::number(activity.static_probability())?,
                    ),
                ]),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let cells = result
        .report
        .cells
        .iter()
        .map(|cell| {
            Ok(Json::object([
                ("name", Json::string(cell.name.clone())),
                ("widthSites", Json::number(cell.width_sites as f64)?),
                ("activity", Json::number(cell.activity)?),
                ("dynamicPowerUw", Json::number(cell.dynamic_power_uw)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("frequencyMhz", Json::number(result.report.frequency_mhz)?),
        ("vectors", Json::number(result.report.vectors as f64)?),
        ("intervals", Json::number(result.report.intervals as f64)?),
        (
            "dynamicPowerUw",
            Json::number(result.report.dynamic_power_uw)?,
        ),
        (
            "staticPowerUw",
            Json::number(result.report.static_power_uw)?,
        ),
        ("totalPowerUw", Json::number(result.report.total_power_uw)?),
        ("activityByNet", Json::Object(activities)),
        ("cells", Json::Array(cells)),
        (
            "mappedStats",
            Json::object([
                ("nets", Json::number(result.mapped_stats.nets as f64)?),
                ("cells", Json::number(result.mapped_stats.cells as f64)?),
                ("comb", Json::number(result.mapped_stats.comb as f64)?),
                ("seq", Json::number(result.mapped_stats.seq as f64)?),
                ("consts", Json::number(result.mapped_stats.consts as f64)?),
                (
                    "libCells",
                    Json::number(result.mapped_stats.lib_cells as f64)?,
                ),
            ]),
        ),
        ("report", Json::string(power_report(&result.report))),
    ]))
}

fn rtl_ir(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let vectors = parse_rtl_simulation_vectors(params)?;
    let power_options = parse_power_options(params)?;
    let ir_options = parse_ir_options(params)?;
    let result = analyze_rtl_ir_cancellable(
        &source,
        top,
        &vectors,
        &power_options,
        &ir_options,
        cancellation,
    )?;
    let rows = result
        .ir
        .rows
        .iter()
        .map(|row| {
            Ok(Json::object([
                ("row", Json::number(row.row as f64)?),
                ("totalCurrentA", Json::number(row.total_current_a)?),
                ("vddWorstDropV", Json::number(row.vdd_worst_drop_v)?),
                ("gndWorstDropV", Json::number(row.gnd_worst_drop_v)?),
                ("totalDropV", Json::number(row.total_drop_v)?),
                ("emMargin", Json::number(row.em_margin)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let cells = result
        .ir
        .cells
        .iter()
        .map(|cell| {
            Ok(Json::object([
                ("cell", Json::number(cell.cell as f64)?),
                ("name", Json::string(cell.name.clone())),
                ("row", Json::number(cell.row as f64)?),
                ("siteStart", Json::number(cell.site_start as f64)?),
                ("siteEnd", Json::number(cell.site_end as f64)?),
                ("currentMa", Json::number(cell.current_ma)?),
                ("localDropV", Json::number(cell.local_drop_v)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let findings = result
        .ir
        .findings
        .iter()
        .map(|finding| {
            Ok(Json::object([
                ("code", Json::string(finding.code.clone())),
                (
                    "severity",
                    Json::string(match finding.severity {
                        eda_api::IrSeverity::Warning => "warning",
                        eda_api::IrSeverity::Error => "error",
                    }),
                ),
                (
                    "row",
                    finding
                        .row
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                ("message", Json::string(finding.message.clone())),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        (
            "power",
            Json::object([
                ("vectors", Json::number(result.power.vectors as f64)?),
                (
                    "dynamicPowerUw",
                    Json::number(result.power.dynamic_power_uw)?,
                ),
                ("staticPowerUw", Json::number(result.power.static_power_uw)?),
                ("totalPowerUw", Json::number(result.power.total_power_uw)?),
            ]),
        ),
        (
            "ir",
            Json::object([
                ("voltageV", Json::number(result.ir.voltage_v)?),
                ("totalPowerUw", Json::number(result.ir.total_power_uw)?),
                ("totalCurrentA", Json::number(result.ir.total_current_a)?),
                ("worstDropV", Json::number(result.ir.worst_drop_v)?),
                (
                    "worstRow",
                    result
                        .ir
                        .worst_row
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                ("rows", Json::Array(rows)),
                ("cells", Json::Array(cells)),
                ("findings", Json::Array(findings)),
                ("hasErrors", Json::Bool(result.ir.has_errors())),
            ]),
        ),
        (
            "placement",
            Json::object([
                ("rows", Json::number(result.placement.rows as f64)?),
                (
                    "placedCells",
                    Json::number(result.placement.cell_pos.len() as f64)?,
                ),
                (
                    "blockages",
                    Json::number(result.placement.blockages.len() as f64)?,
                ),
            ]),
        ),
        (
            "mappedStats",
            Json::object([
                ("nets", Json::number(result.mapped_stats.nets as f64)?),
                ("cells", Json::number(result.mapped_stats.cells as f64)?),
                ("comb", Json::number(result.mapped_stats.comb as f64)?),
                ("seq", Json::number(result.mapped_stats.seq as f64)?),
                ("consts", Json::number(result.mapped_stats.consts as f64)?),
                (
                    "libCells",
                    Json::number(result.mapped_stats.lib_cells as f64)?,
                ),
            ]),
        ),
        ("report", Json::string(ir_report(&result.ir))),
    ]))
}

fn formal_trace_json(trace: Option<&[std::collections::HashMap<String, u64>]>) -> Json {
    let Some(frames) = trace else {
        return Json::Null;
    };
    Json::Array(
        frames
            .iter()
            .map(|frame| {
                Json::Object(
                    frame
                        .iter()
                        .map(|(name, value)| (name.clone(), Json::from(*value)))
                        .collect(),
                )
            })
            .collect(),
    )
}

fn rtl_bmc(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let properties = required_string(params, "properties")?;
    let depth = optional_positive_usize(params, "depth", 10)?;
    let result = check_rtl_properties_cancellable(&source, top, &properties, depth, cancellation)?;
    let property_results = result
        .results
        .iter()
        .map(|property| {
            Ok(Json::object([
                ("assertion", Json::string(property.assertion.clone())),
                (
                    "kind",
                    Json::string(match property.kind {
                        eda_api::AssertionKind::Assert => "assert",
                        eda_api::AssertionKind::Assume => "assume",
                        eda_api::AssertionKind::Cover => "cover",
                    }),
                ),
                (
                    "clock",
                    property.clock.clone().map_or(Json::Null, Json::string),
                ),
                (
                    "status",
                    Json::string(match property.status {
                        PropertyBmcStatus::BoundedPass => "boundedPass",
                        PropertyBmcStatus::Violated => "violated",
                        PropertyBmcStatus::Satisfied => "satisfied",
                        PropertyBmcStatus::NotSatisfied => "notSatisfied",
                        PropertyBmcStatus::AssumptionsUnsat => "assumptionsUnsat",
                    }),
                ),
                ("violated", Json::Bool(property.violated())),
                ("covered", Json::Bool(property.covered())),
                ("depthChecked", Json::number(property.depth_checked as f64)?),
                (
                    "framesChecked",
                    Json::number(property.frames_checked as f64)?,
                ),
                (
                    "triggerFrame",
                    property
                        .trigger_frame
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "repeatCount",
                    property
                        .repeat_count
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "endFrame",
                    property
                        .end_frame
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                ("inputTrace", formal_trace_json(property.trace.as_deref())),
                ("netTrace", formal_trace_json(property.net_trace.as_deref())),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("depth", Json::number(depth as f64)?),
        ("properties", Json::number(result.results.len() as f64)?),
        (
            "assumptions",
            Json::Array(result.assumptions.iter().map(Json::string).collect()),
        ),
        (
            "violations",
            Json::number(
                result
                    .results
                    .iter()
                    .filter(|property| property.violated())
                    .count() as f64,
            )?,
        ),
        ("results", Json::Array(property_results)),
        (
            "mappedStats",
            Json::object([
                ("nets", Json::number(result.mapped_stats.nets as f64)?),
                ("cells", Json::number(result.mapped_stats.cells as f64)?),
                ("comb", Json::number(result.mapped_stats.comb as f64)?),
                ("seq", Json::number(result.mapped_stats.seq as f64)?),
                ("consts", Json::number(result.mapped_stats.consts as f64)?),
                (
                    "libCells",
                    Json::number(result.mapped_stats.lib_cells as f64)?,
                ),
            ]),
        ),
        ("report", Json::string(result.report)),
    ]))
}

fn rtl_dft(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let options = parse_scan_options(params)?;
    let result = insert_rtl_scan_cancellable(&source, top, &options, cancellation)?;
    let chain = result
        .scan
        .chain
        .iter()
        .map(|cell| {
            Ok(Json::object([
                ("cell", Json::number(cell.cell as f64)?),
                ("name", Json::string(cell.name.clone())),
                ("muxCell", Json::number(cell.mux_cell as f64)?),
                ("qNet", Json::number(cell.q_net as f64)?),
                (
                    "functionalDNet",
                    Json::number(cell.functional_d_net as f64)?,
                ),
                ("scanDNet", Json::number(cell.scan_d_net as f64)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        (
            "scan",
            Json::object([
                ("chainName", Json::string(options.chain_name.clone())),
                (
                    "scanEnPort",
                    Json::string(result.scan.netlist.net(result.scan.scan_en).name.clone()),
                ),
                (
                    "scanInPort",
                    Json::string(result.scan.netlist.net(result.scan.scan_in).name.clone()),
                ),
                (
                    "scanOutPort",
                    Json::string(result.scan.netlist.net(result.scan.scan_out).name.clone()),
                ),
                ("chain", Json::Array(chain)),
            ]),
        ),
        (
            "dft",
            Json::object([
                (
                    "sequentialCells",
                    Json::number(result.dft.sequential_cells as f64)?,
                ),
                ("scanCells", Json::number(result.dft.scan_cells as f64)?),
                ("scanChains", Json::number(result.dft.scan_chains as f64)?),
                (
                    "combinationalCells",
                    Json::number(result.dft.combinational_cells as f64)?,
                ),
                (
                    "unscannedSequential",
                    Json::Array(
                        result
                            .dft
                            .unscanned_sequential
                            .iter()
                            .map(Json::string)
                            .collect(),
                    ),
                ),
            ]),
        ),
        (
            "originalStats",
            Json::object([
                ("cells", Json::number(result.original_stats.cells as f64)?),
                ("seq", Json::number(result.original_stats.seq as f64)?),
            ]),
        ),
        (
            "scannedStats",
            Json::object([
                ("cells", Json::number(result.scanned_stats.cells as f64)?),
                ("seq", Json::number(result.scanned_stats.seq as f64)?),
                (
                    "libCells",
                    Json::number(result.scanned_stats.lib_cells as f64)?,
                ),
            ]),
        ),
        ("report", Json::string(result.report)),
    ]))
}

fn cdc_path_json(path: &eda_api::CdcPath) -> Result<Json, String> {
    Ok(Json::object([
        (
            "sourceCell",
            path.source_cell
                .map_or(Json::Null, |value| Json::from(value as i64)),
        ),
        ("sourceName", Json::string(path.source_name.clone())),
        (
            "sourceHierarchy",
            path.source_hierarchy
                .clone()
                .map_or(Json::Null, Json::string),
        ),
        ("sourceClock", Json::string(path.source_clock.clone())),
        ("destinationCell", Json::from(path.destination_cell as i64)),
        (
            "destinationName",
            Json::string(path.destination_name.clone()),
        ),
        (
            "destinationHierarchy",
            path.destination_hierarchy
                .clone()
                .map_or(Json::Null, Json::string),
        ),
        (
            "destinationClock",
            Json::string(path.destination_clock.clone()),
        ),
        ("destinationPin", Json::string(path.destination_pin.clone())),
        (
            "combinationalCells",
            Json::Array(
                path.combinational_cells
                    .iter()
                    .map(|cell| Json::from(*cell as i64))
                    .collect(),
            ),
        ),
        (
            "hierarchyInstances",
            Json::Array(path.hierarchy_instances.iter().map(Json::string).collect()),
        ),
        ("synchronized", Json::Bool(path.synchronized)),
    ]))
}

fn rtl_cdc(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let result = analyze_rtl_cdc_cancellable(&source, top, cancellation)?;
    let clocks = result
        .cdc
        .clocks
        .iter()
        .map(|clock| {
            Ok(Json::object([
                ("name", Json::string(clock.name.clone())),
                ("rootNet", Json::number(clock.root_net as f64)?),
                ("rootName", Json::string(clock.root_name.clone())),
                (
                    "clockNets",
                    Json::Array(
                        clock
                            .clock_nets
                            .iter()
                            .map(|net| Json::from(*net as i64))
                            .collect(),
                    ),
                ),
                (
                    "sequentialCells",
                    Json::Array(
                        clock
                            .sequential_cells
                            .iter()
                            .map(|cell| Json::from(*cell as i64))
                            .collect(),
                    ),
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let crossings = result
        .cdc
        .crossings
        .iter()
        .map(cdc_path_json)
        .collect::<Result<Vec<_>, String>>()?;
    let findings = result
        .cdc
        .findings
        .iter()
        .map(|finding| {
            Ok(Json::object([
                ("kind", Json::string(finding.kind.clone())),
                (
                    "severity",
                    Json::string(match finding.severity {
                        CdcSeverity::Info => "info",
                        CdcSeverity::Warning => "warning",
                        CdcSeverity::Error => "error",
                    }),
                ),
                ("path", cdc_path_json(&finding.path)?),
                ("message", Json::string(finding.message.clone())),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("clocks", Json::Array(clocks)),
        ("crossings", Json::Array(crossings)),
        ("findings", Json::Array(findings)),
        ("hasErrors", Json::Bool(result.cdc.has_errors())),
        ("clean", Json::Bool(result.cdc.clean())),
        (
            "mappedStats",
            Json::object([
                ("nets", Json::number(result.mapped_stats.nets as f64)?),
                ("cells", Json::number(result.mapped_stats.cells as f64)?),
                ("seq", Json::number(result.mapped_stats.seq as f64)?),
                (
                    "libCells",
                    Json::number(result.mapped_stats.lib_cells as f64)?,
                ),
            ]),
        ),
        ("report", Json::string(cdc_report(&result.cdc))),
    ]))
}

fn optional_top_field<'a>(params: &'a Json, name: &str) -> Result<Option<&'a str>, String> {
    match params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?
        .get(name)
    {
        None | Some(Json::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| format!("RPC parameter '{name}' must be a string")),
    }
}

fn rtl_lvs(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL LVS cancelled".into());
    }
    let reference_source = required_string(params, "reference")?;
    let layout_source = required_string(params, "layout")?;
    let shared_top = optional_top(params)?;
    let reference_top = optional_top_field(params, "referenceTop")?.or(shared_top);
    let layout_top = optional_top_field(params, "layoutTop")?.or(shared_top);

    let reference_flow = run_flow_cancellable(&reference_source, reference_top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL LVS cancelled".into());
    }
    let layout_flow = run_flow_cancellable(&layout_source, layout_top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL LVS cancelled".into());
    }
    let result = eda_api::check_layout_vs_schematic(&reference_flow.netlist, &layout_flow.netlist)?;

    let cell_matches = result
        .cell_matches
        .iter()
        .map(|match_item| {
            Json::object([
                (
                    "referenceCell",
                    Json::string(match_item.reference_cell.clone()),
                ),
                ("layoutCell", Json::string(match_item.layout_cell.clone())),
                ("deviceType", Json::string(match_item.device_type.clone())),
            ])
        })
        .collect::<Vec<_>>();
    let net_matches = result
        .net_matches
        .iter()
        .map(|match_item| {
            Json::object([
                (
                    "referenceNet",
                    Json::string(match_item.reference_net.clone()),
                ),
                ("layoutNet", Json::string(match_item.layout_net.clone())),
            ])
        })
        .collect::<Vec<_>>();

    Ok(Json::object([
        ("matches", Json::Bool(result.matches)),
        ("cellMatches", Json::Array(cell_matches)),
        ("netMatches", Json::Array(net_matches)),
        (
            "shortedNets",
            Json::Array(
                result
                    .shorted_nets
                    .iter()
                    .map(|name| Json::string(name.clone()))
                    .collect(),
            ),
        ),
        (
            "openNets",
            Json::Array(
                result
                    .open_nets
                    .iter()
                    .map(|name| Json::string(name.clone()))
                    .collect(),
            ),
        ),
        (
            "notes",
            Json::Array(
                result
                    .notes
                    .iter()
                    .map(|note| Json::string(note.clone()))
                    .collect(),
            ),
        ),
        (
            "referenceStats",
            Json::object([
                (
                    "cells",
                    Json::number(reference_flow.netlist.cell_count() as f64)?,
                ),
                (
                    "nets",
                    Json::number(reference_flow.netlist.net_count() as f64)?,
                ),
            ]),
        ),
        (
            "layoutStats",
            Json::object([
                (
                    "cells",
                    Json::number(layout_flow.netlist.cell_count() as f64)?,
                ),
                (
                    "nets",
                    Json::number(layout_flow.netlist.net_count() as f64)?,
                ),
            ]),
        ),
        ("report", Json::string(eda_api::lvs_report(&result))),
    ]))
}

fn rtl_power_intent(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL power-intent check cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let upf_source = required_string(params, "upf")?;
    let design = eda_api::compile(&source, top)?;
    if cancellation.is_cancelled() {
        return Err("RTL power-intent check cancelled".into());
    }
    let upf = eda_api::parse_power_intent(&upf_source)?;
    let result = eda_api::power_intent_check(&design, &upf);
    if cancellation.is_cancelled() {
        return Err("RTL power-intent check cancelled".into());
    }

    let findings = result
        .findings
        .iter()
        .map(|finding| {
            Json::object([
                ("code", Json::string(finding.code.clone())),
                (
                    "severity",
                    Json::string(match finding.severity {
                        eda_api::UpfSeverity::Warning => "warning",
                        eda_api::UpfSeverity::Error => "error",
                    }),
                ),
                ("message", Json::string(finding.message.clone())),
            ])
        })
        .collect::<Vec<_>>();
    Ok(Json::object([
        ("domainCount", Json::number(result.domain_count as f64)?),
        (
            "supplyNetCount",
            Json::number(result.supply_net_count as f64)?,
        ),
        ("switchCount", Json::number(result.switch_count as f64)?),
        (
            "powerModeCount",
            Json::number(result.power_mode_count as f64)?,
        ),
        (
            "isolationCount",
            Json::number(result.isolation_count as f64)?,
        ),
        (
            "retentionCount",
            Json::number(result.retention_count as f64)?,
        ),
        (
            "levelShifterCount",
            Json::number(result.level_shifter_count as f64)?,
        ),
        ("findings", Json::Array(findings)),
        ("hasErrors", Json::Bool(result.has_errors())),
        ("report", Json::string(eda_api::upf_report(&result))),
    ]))
}

fn rtl_safety(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL safety proof cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let bad_net = required_string(params, "badNet")?;
    let Some(fields) = params.as_object() else {
        return Err("params must be an object".into());
    };
    let depth = match fields.get("depth") {
        None => 8,
        Some(Json::Number(value)) => {
            if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
                return Err("RPC parameter 'depth' must be a positive integer".into());
            }
            *value as usize
        }
        Some(_) => return Err("RPC parameter 'depth' must be a positive integer".into()),
    };

    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL safety proof cancelled".into());
    }
    let proof = prove_safety_cancellable(&flow.netlist, &bad_net, depth, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL safety proof cancelled".into());
    }

    Ok(Json::object([
        (
            "status",
            Json::string(match proof.status {
                ProofStatus::Proved => "proved",
                ProofStatus::Refuted => "refuted",
                ProofStatus::Unknown => "unknown",
            }),
        ),
        ("depthChecked", Json::number(proof.depth_checked as f64)?),
        ("inductive", Json::Bool(proof.inductive)),
        (
            "hasCounterexample",
            Json::Bool(proof.counterexample.is_some()),
        ),
        (
            "failedInvariant",
            proof
                .failed_invariant
                .clone()
                .map_or(Json::Null, Json::string),
        ),
        (
            "mappedStats",
            Json::object([
                ("cells", Json::number(flow.netlist.cell_count() as f64)?),
                ("nets", Json::number(flow.netlist.net_count() as f64)?),
                ("libCells", Json::number(flow.stats.lib_cells as f64)?),
            ]),
        ),
        ("report", Json::string(proof_report(&proof))),
    ]))
}

fn parse_physical_timing_options(params: &Json) -> Result<PhysicalTimingFlowOptions, String> {
    let Some(Json::Object(fields)) = params
        .as_object()
        .map(|fields| fields.get("physicalTimingOptions"))
        .and_then(|value| value.cloned())
    else {
        return Ok(PhysicalTimingFlowOptions::default());
    };
    let mut options = PhysicalTimingFlowOptions::default();
    for (json_name, target) in [
        ("tracksPerChannel", &mut options.route.tracks_per_channel),
        ("iterations", &mut options.iterations),
        ("placementPasses", &mut options.placement_passes),
        ("routingPasses", &mut options.routing_passes),
        (
            "antennaMaxDiodesPerNet",
            &mut options.antenna_max_diodes_per_net,
        ),
    ] {
        if let Some(value) = fields.get(json_name) {
            let Some(value) = value.as_number() else {
                return Err(format!(
                    "physical timing option '{json_name}' must be a positive integer"
                ));
            };
            if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
                return Err(format!(
                    "physical timing option '{json_name}' must be a positive integer"
                ));
            }
            *target = *value as usize;
        }
    }
    if let Some(value) = fields.get("repairAntennas") {
        match value {
            Json::Bool(value) => options.repair_antennas = *value,
            _ => return Err("physical timing option 'repairAntennas' must be a boolean".into()),
        }
    }

    let interconnect_names = [
        (
            "wireResistanceOhmPerSite",
            &mut options.interconnect.wire_resistance_ohm_per_site,
        ),
        (
            "wireCapacitancePfPerSite",
            &mut options.interconnect.wire_capacitance_pf_per_site,
        ),
        (
            "couplingCapacitancePfPerSite",
            &mut options.interconnect.coupling_capacitance_pf_per_site,
        ),
        (
            "viaResistanceOhm",
            &mut options.interconnect.via_resistance_ohm,
        ),
        (
            "viaCapacitancePf",
            &mut options.interconnect.via_capacitance_pf,
        ),
    ];
    for (json_name, target) in interconnect_names {
        if let Some(value) = fields.get(json_name) {
            let Some(value) = value.as_number() else {
                return Err(format!(
                    "physical timing interconnect option '{json_name}' must be a number"
                ));
            };
            *target = *value;
        }
    }
    Ok(options)
}

fn rtl_physical_timing(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL physical timing analysis cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let options = parse_physical_timing_options(params)?;
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL physical timing analysis cancelled".into());
    }
    let result = run_physical_timing_flow_cancellable(&flow.netlist, &options, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL physical timing analysis cancelled".into());
    }
    let best = result.best();
    let spef = write_parasitics(&best.parasitics)?;
    let sdf = write_delay_file(&best.sdf)?;

    Ok(Json::object([
        (
            "baselineDelayNs",
            Json::number(result.baseline.timing.critical_path_delay)?,
        ),
        (
            "bestDelayNs",
            Json::number(best.timing.critical_path_delay)?,
        ),
        ("bestIndex", Json::number(result.best_index as f64)?),
        (
            "optimizedIterations",
            Json::number(result.optimized.len() as f64)?,
        ),
        (
            "wirelength",
            Json::number(best.routing.wirelength() as f64)?,
        ),
        (
            "rcNets",
            Json::number(best.parasitic_report.net_count as f64)?,
        ),
        ("drcErrors", Json::number(best.drc.error_count() as f64)?),
        ("spef", Json::string(spef)),
        ("sdf", Json::string(sdf)),
        (
            "mappedStats",
            Json::object([
                ("cells", Json::number(flow.netlist.cell_count() as f64)?),
                ("nets", Json::number(flow.netlist.net_count() as f64)?),
                ("libCells", Json::number(flow.stats.lib_cells as f64)?),
            ]),
        ),
        ("report", Json::string(physical_timing_flow_report(&result))),
    ]))
}

fn fault_json(fault: &eda_api::Fault) -> Result<Json, String> {
    Ok(Json::object([
        ("index", Json::number(fault.index as f64)?),
        ("net", Json::number(fault.net as f64)?),
        ("netName", Json::string(fault.net_name.clone())),
        (
            "stuckAt",
            Json::string(match fault.stuck_at {
                eda_api::StuckAt::Zero => "0",
                eda_api::StuckAt::One => "1",
            }),
        ),
    ]))
}

enum PostRoutePowerStimulus {
    Vectors(Vec<HashMap<String, LogicWord>>),
    Waveform(eda_api::Vcd),
}

impl PostRoutePowerStimulus {
    fn sample_count(&self) -> usize {
        match self {
            Self::Vectors(vectors) => vectors.len(),
            Self::Waveform(waveform) => waveform.time_points().len(),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Vectors(_) => "vectors",
            Self::Waveform(_) => "vcd",
        }
    }
}

fn parse_post_route_power_stimulus(
    params: &Json,
    flow: &Flow,
) -> Result<PostRoutePowerStimulus, String> {
    let fields = params.as_object();
    let has_vcd = fields.is_some_and(|fields| fields.contains_key("vcd"));
    let has_vectors = fields.is_some_and(|fields| fields.contains_key("vectors"));
    if has_vcd && has_vectors {
        return Err("RPC parameters 'vcd' and 'vectors' are mutually exclusive".into());
    }
    if has_vcd {
        let source = required_string(params, "vcd")?;
        return Ok(PostRoutePowerStimulus::Waveform(parse_waveform(&source)?));
    }

    let simulation_vectors = if params
        .as_object()
        .is_some_and(|fields| fields.contains_key("vectors"))
    {
        parse_rtl_simulation_vectors(params)?
    } else {
        Vec::new()
    };
    if simulation_vectors.is_empty() {
        return Ok(PostRoutePowerStimulus::Vectors(default_power_vectors(
            &flow.netlist,
        )));
    }

    let widths = flow
        .netlist
        .ports
        .iter()
        .filter(|port| port.dir == eda_core::netlist::Dir::In)
        .map(|port| (port.name.clone(), port.bits.len()))
        .collect::<BTreeMap<_, _>>();
    let inputs = simulation_vectors
        .iter()
        .map(|vector| {
            let mut inputs = HashMap::new();
            for (name, width) in &widths {
                let text = vector
                    .inputs
                    .get(name)
                    .ok_or_else(|| format!("post-route vector lacks input '{name}'"))?;
                let word = LogicWord::parse(*width, text)
                    .ok_or_else(|| format!("invalid post-route value for input '{name}'"))?;
                inputs.insert(name.clone(), word);
            }
            Ok(inputs)
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(PostRoutePowerStimulus::Vectors(inputs))
}

fn rtl_post_route_power_ir(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL post-route power/IR analysis cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let scenario_source = required_string(params, "scenarios")?;
    let timing_definitions = parse_timing_scenarios(&scenario_source)?;
    let scenarios = post_route_power_scenarios_from_timing_scenarios(&timing_definitions)?;
    if scenarios.is_empty() {
        return Err("RTL post-route power/IR requires at least one scenario".into());
    }
    let physical_options = parse_physical_timing_options(params)?;
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL post-route power/IR analysis cancelled".into());
    }
    let stimulus = parse_post_route_power_stimulus(params, &flow)?;
    let physical =
        run_physical_timing_flow_cancellable(&flow.netlist, &physical_options, cancellation)?;
    let iteration = physical.best();

    let mut results = Vec::with_capacity(scenarios.len());
    for scenario in &scenarios {
        if cancellation.is_cancelled() {
            return Err("RTL post-route power/IR analysis cancelled".into());
        }
        results.push(match &stimulus {
            PostRoutePowerStimulus::Vectors(vectors) => {
                analyze_physical_timing_power_ir(iteration, vectors, &scenario.options)?
            }
            PostRoutePowerStimulus::Waveform(waveform) => {
                analyze_physical_timing_power_ir_from_waveform(
                    iteration,
                    waveform,
                    &scenario.options,
                )?
            }
        });
    }
    if cancellation.is_cancelled() {
        return Err("RTL post-route power/IR analysis cancelled".into());
    }

    let mut worst_power_corner = 0;
    let mut worst_drop_corner = 0;
    let mut error_findings = 0usize;
    for (index, result) in results.iter().enumerate() {
        if result.power.total_power_uw > results[worst_power_corner].power.total_power_uw {
            worst_power_corner = index;
        }
        if result.ir.worst_drop_v > results[worst_drop_corner].ir.worst_drop_v {
            worst_drop_corner = index;
        }
        error_findings += result
            .ir
            .findings
            .iter()
            .filter(|finding| finding.severity == eda_api::IrSeverity::Error)
            .count();
    }

    let corners = scenarios
        .iter()
        .zip(results.iter())
        .map(|(scenario, result)| {
            let findings = result
                .ir
                .findings
                .iter()
                .map(|finding| {
                    Ok(Json::object([
                        ("code", Json::string(finding.code.clone())),
                        (
                            "severity",
                            Json::string(match finding.severity {
                                eda_api::IrSeverity::Warning => "warning",
                                eda_api::IrSeverity::Error => "error",
                            }),
                        ),
                        (
                            "row",
                            finding.row.map_or(Json::Null, |row| Json::from(row as i64)),
                        ),
                        ("message", Json::string(finding.message.clone())),
                    ]))
                })
                .collect::<Result<Vec<_>, String>>()?;
            let effective_capacitance_pf = result.effective_capacitance_pf.values().sum();
            Ok(Json::object([
                ("scenario", Json::string(scenario.name.clone())),
                ("voltageV", Json::number(result.ir.voltage_v)?),
                (
                    "dynamicPowerUw",
                    Json::number(result.power.dynamic_power_uw)?,
                ),
                ("staticPowerUw", Json::number(result.power.static_power_uw)?),
                ("totalPowerUw", Json::number(result.power.total_power_uw)?),
                (
                    "parasiticDynamicPowerUw",
                    Json::number(result.parasitic_dynamic_power_uw)?,
                ),
                (
                    "effectiveCapacitancePf",
                    Json::number(effective_capacitance_pf)?,
                ),
                (
                    "effectiveCapacitanceNets",
                    Json::from(result.effective_capacitance_pf.len() as u64),
                ),
                ("totalCurrentA", Json::number(result.ir.total_current_a)?),
                ("worstDropV", Json::number(result.ir.worst_drop_v)?),
                (
                    "worstRow",
                    result
                        .ir
                        .worst_row
                        .map_or(Json::Null, |row| Json::from(row as i64)),
                ),
                ("hasIrErrors", Json::Bool(result.ir.has_errors())),
                ("findings", Json::Array(findings)),
                ("report", Json::string(post_route_power_ir_report(result))),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(Json::object([
        ("scenarioCount", Json::from(scenarios.len() as u64)),
        ("vectorCount", Json::from(stimulus.sample_count() as u64)),
        ("stimulus", Json::string(stimulus.kind())),
        (
            "worstPowerScenario",
            Json::string(scenarios[worst_power_corner].name.clone()),
        ),
        (
            "worstDropScenario",
            Json::string(scenarios[worst_drop_corner].name.clone()),
        ),
        ("errorFindings", Json::from(error_findings as u64)),
        ("corners", Json::Array(corners)),
        (
            "physicalTimingReport",
            Json::string(physical_timing_flow_report(&physical)),
        ),
        (
            "mappedStats",
            Json::object([
                ("cells", Json::number(flow.netlist.cell_count() as f64)?),
                ("nets", Json::number(flow.netlist.net_count() as f64)?),
                ("libCells", Json::number(flow.stats.lib_cells as f64)?),
            ]),
        ),
    ]))
}

fn rtl_post_route_power_mode_ir(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL post-route power-mode IR analysis cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let scenario_source = required_string(params, "scenarios")?;
    let upf_source = required_string(params, "upf")?;
    let timing_definitions = parse_timing_scenarios(&scenario_source)?;
    let mut scenarios = post_route_power_scenarios_from_timing_scenarios(&timing_definitions)?;
    if scenarios.is_empty() {
        return Err("RTL post-route power-mode IR requires at least one scenario".into());
    }

    let mesh_enabled = params
        .as_object()
        .and_then(|fields| fields.get("powerModeOptions"))
        .and_then(Json::as_object)
        .and_then(|fields| fields.get("multilayerPowerMesh"))
        .is_some_and(|value| matches!(value, Json::Bool(enabled) if *enabled));
    let mesh_rows = params
        .as_object()
        .and_then(|fields| fields.get("powerModeOptions"))
        .and_then(Json::as_object)
        .and_then(|fields| fields.get("rows"))
        .and_then(Json::as_number)
        .map_or(3, |value| value.max(1.0) as usize);
    let mesh_columns = params
        .as_object()
        .and_then(|fields| fields.get("powerModeOptions"))
        .and_then(Json::as_object)
        .and_then(|fields| fields.get("columns"))
        .and_then(Json::as_number)
        .map_or(3, |value| value.max(1.0) as usize);
    if mesh_enabled {
        for scenario in &mut scenarios {
            let ir = scenario.options.ir.clone();
            scenario.options.multilayer_power_mesh = Some(MultilayerPowerMeshOptions {
                rows: mesh_rows,
                columns: mesh_columns,
                voltage_v: ir.voltage_v,
                max_total_drop_v: ir.max_total_drop_v,
                max_node_current_a: ir.max_rail_current_a,
                max_segment_current_a: ir.max_rail_current_a,
                ..MultilayerPowerMeshOptions::default()
            });
        }
    }

    let power_intent = parse_power_intent(&upf_source)?;
    let physical_options = parse_physical_timing_options(params)?;
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL post-route power-mode IR analysis cancelled".into());
    }
    let stimulus = parse_post_route_power_stimulus(params, &flow)?;
    let physical =
        run_physical_timing_flow_cancellable(&flow.netlist, &physical_options, cancellation)?;
    let iteration = physical.best();

    let mut results = Vec::with_capacity(scenarios.len());
    for scenario in &scenarios {
        if cancellation.is_cancelled() {
            return Err("RTL post-route power-mode IR analysis cancelled".into());
        }
        results.push(match &stimulus {
            PostRoutePowerStimulus::Vectors(vectors) => {
                analyze_physical_timing_power_ir_power_modes_with_upf(
                    iteration,
                    vectors,
                    &scenario.options,
                    &power_intent,
                )?
            }
            PostRoutePowerStimulus::Waveform(waveform) => {
                analyze_physical_timing_power_ir_power_modes_from_waveform_with_upf(
                    iteration,
                    waveform,
                    &scenario.options,
                    &power_intent,
                )?
            }
        });
    }
    if cancellation.is_cancelled() {
        return Err("RTL post-route power-mode IR analysis cancelled".into());
    }

    let mut worst_power = (0usize, 0usize);
    let mut worst_drop = (0usize, 0usize);
    let mut worst_em_lifetime = (0usize, 0usize);
    let mut worst_power_value = f64::NEG_INFINITY;
    let mut worst_drop_value = f64::NEG_INFINITY;
    let mut worst_em_value = None::<f64>;
    let mut error_findings = 0usize;
    for (corner_index, result) in results.iter().enumerate() {
        for (mode_index, mode) in result.modes.iter().enumerate() {
            if mode.result.total_power_uw > worst_power_value {
                worst_power_value = mode.result.total_power_uw;
                worst_power = (corner_index, mode_index);
            }
            if mode.result.worst_drop_v > worst_drop_value {
                worst_drop_value = mode.result.worst_drop_v;
                worst_drop = (corner_index, mode_index);
            }
            if let Some(lifetime) = mode
                .worst_em_lifetime_hours
                .filter(|value| value.is_finite())
            {
                if worst_em_value.is_none_or(|current| lifetime < current) {
                    worst_em_value = Some(lifetime);
                    worst_em_lifetime = (corner_index, mode_index);
                }
            }
        }
        error_findings += result.error_findings;
    }

    let corners = scenarios
        .iter()
        .zip(results.iter())
        .map(|(scenario, result)| {
            let modes = result
                .modes
                .iter()
                .map(|mode| {
                    let findings = mode
                        .result
                        .findings
                        .iter()
                        .map(|finding| {
                            Ok(Json::object([
                                ("code", Json::string(finding.code.clone())),
                                (
                                    "severity",
                                    Json::string(match finding.severity {
                                        eda_api::IrSeverity::Warning => "warning",
                                        eda_api::IrSeverity::Error => "error",
                                    }),
                                ),
                                (
                                    "row",
                                    finding.row.map_or(Json::Null, |row| Json::from(row as i64)),
                                ),
                                ("message", Json::string(finding.message.clone())),
                            ]))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    let domains = mode
                        .result
                        .domains
                        .iter()
                        .map(|domain| {
                            Ok(Json::object([
                                ("name", Json::string(domain.name.clone())),
                                ("powered", Json::Bool(domain.powered)),
                                ("voltageV", Json::number(domain.voltage_v)?),
                                ("cellCount", Json::from(domain.cell_count as u64)),
                                ("totalPowerUw", Json::number(domain.total_power_uw)?),
                                ("totalCurrentA", Json::number(domain.total_current_a)?),
                                ("worstDropV", Json::number(domain.worst_drop_v)?),
                            ]))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    Ok(Json::object([
                        ("powerMode", Json::string(mode.power_mode.clone())),
                        ("totalPowerUw", Json::number(mode.result.total_power_uw)?),
                        ("totalCurrentA", Json::number(mode.result.total_current_a)?),
                        (
                            "switchLeakagePowerUw",
                            Json::number(mode.result.switch_leakage_power_uw)?,
                        ),
                        (
                            "strategyPowerUw",
                            Json::number(mode.result.strategy_power_uw)?,
                        ),
                        ("worstDropV", Json::number(mode.result.worst_drop_v)?),
                        (
                            "worstPowerDomain",
                            mode.result
                                .worst_power_domain
                                .clone()
                                .map_or(Json::Null, Json::string),
                        ),
                        (
                            "worstRow",
                            mode.result
                                .worst_row
                                .map_or(Json::Null, |row| Json::from(row as i64)),
                        ),
                        (
                            "coupledMultilayerMesh",
                            Json::Bool(mode.coupled_multilayer_mesh),
                        ),
                        (
                            "worstEmLifetimeHours",
                            mode.worst_em_lifetime_hours
                                .filter(|lifetime| lifetime.is_finite())
                                .map_or(Ok(Json::Null), Json::number)?,
                        ),
                        (
                            "worstEmLifetimeUnbounded",
                            Json::Bool(
                                mode.worst_em_lifetime_hours
                                    .is_some_and(|lifetime| lifetime.is_infinite()),
                            ),
                        ),
                        ("domains", Json::Array(domains)),
                        ("hasErrors", Json::Bool(mode.result.has_errors())),
                        ("findings", Json::Array(findings)),
                    ]))
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(Json::object([
                ("scenario", Json::string(scenario.name.clone())),
                ("modeCount", Json::from(result.modes.len() as u64)),
                (
                    "worstPowerMode",
                    result
                        .modes
                        .get(result.worst_power_mode)
                        .map_or(Json::Null, |mode| Json::string(mode.power_mode.clone())),
                ),
                (
                    "worstDropMode",
                    result
                        .modes
                        .get(result.worst_drop_mode)
                        .map_or(Json::Null, |mode| Json::string(mode.power_mode.clone())),
                ),
                ("modes", Json::Array(modes)),
                ("report", Json::string(upf_power_mode_ir_report(result))),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(Json::object([
        ("scenarioCount", Json::from(scenarios.len() as u64)),
        ("vectorCount", Json::from(stimulus.sample_count() as u64)),
        ("stimulus", Json::string(stimulus.kind())),
        (
            "worstPowerScenario",
            Json::string(scenarios[worst_power.0].name.clone()),
        ),
        (
            "worstPowerMode",
            Json::string(
                results[worst_power.0].modes[worst_power.1]
                    .power_mode
                    .clone(),
            ),
        ),
        (
            "worstDropScenario",
            Json::string(scenarios[worst_drop.0].name.clone()),
        ),
        (
            "worstDropMode",
            Json::string(results[worst_drop.0].modes[worst_drop.1].power_mode.clone()),
        ),
        (
            "worstEmLifetimeScenario",
            Json::string(scenarios[worst_em_lifetime.0].name.clone()),
        ),
        (
            "worstEmLifetimeMode",
            Json::string(
                results[worst_em_lifetime.0].modes[worst_em_lifetime.1]
                    .power_mode
                    .clone(),
            ),
        ),
        (
            "worstEmLifetimeHours",
            worst_em_value.map_or(Ok(Json::Null), Json::number)?,
        ),
        ("errorFindings", Json::from(error_findings as u64)),
        ("corners", Json::Array(corners)),
        (
            "physicalTimingReport",
            Json::string(physical_timing_flow_report(&physical)),
        ),
        (
            "mappedStats",
            Json::object([
                ("cells", Json::number(flow.netlist.cell_count() as f64)?),
                ("nets", Json::number(flow.netlist.net_count() as f64)?),
                ("libCells", Json::number(flow.stats.lib_cells as f64)?),
            ]),
        ),
    ]))
}

fn rtl_fault(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let options = parse_pattern_options(params)?;
    let result = analyze_rtl_faults_cancellable(&source, top, &options, cancellation)?;
    let patterns = result
        .faults
        .patterns
        .iter()
        .map(|pattern| {
            Json::Object(
                pattern
                    .iter()
                    .map(|(name, value)| (name.clone(), Json::from(*value)))
                    .collect(),
            )
        })
        .collect::<Vec<_>>();
    let detections = result
        .faults
        .detections
        .iter()
        .map(|detection| {
            Ok(Json::object([
                ("fault", fault_json(&detection.fault)?),
                (
                    "detectingPatterns",
                    Json::Array(
                        detection
                            .detecting_patterns
                            .iter()
                            .map(|index| Json::from(*index as i64))
                            .collect(),
                    ),
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let undetected = result
        .faults
        .undetected_faults
        .iter()
        .map(fault_json)
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("patterns", Json::Array(patterns)),
        ("exhaustive", Json::Bool(result.faults.exhaustive)),
        (
            "totalFaults",
            Json::number(result.faults.total_faults as f64)?,
        ),
        (
            "detectedFaults",
            Json::number(result.faults.detected_faults as f64)?,
        ),
        ("undetectedFaults", Json::Array(undetected)),
        ("detections", Json::Array(detections)),
        (
            "coveragePercent",
            Json::number(result.faults.coverage_percent)?,
        ),
        (
            "mappedStats",
            Json::object([
                ("nets", Json::number(result.mapped_stats.nets as f64)?),
                ("cells", Json::number(result.mapped_stats.cells as f64)?),
                (
                    "libCells",
                    Json::number(result.mapped_stats.lib_cells as f64)?,
                ),
            ]),
        ),
        ("stil", Json::string(result.stil.clone())),
        ("report", Json::string(fault_report(&result.faults))),
    ]))
}

fn parse_rtl_simulation_vectors(params: &Json) -> Result<Vec<RtlSimulationVector>, String> {
    let vectors_value = required_value(params, "vectors")?;
    let Json::Array(vector_values) = vectors_value else {
        return Err("'vectors' must be an array".into());
    };
    let mut vectors = Vec::with_capacity(vector_values.len());
    for vector in &vector_values {
        let fields = vector
            .as_object()
            .ok_or_else(|| "each simulation vector must be an object".to_string())?;
        let time_ns = match fields.get("timeNs") {
            Some(Json::Number(value))
                if *value >= 0.0 && value.fract() == 0.0 && *value <= u64::MAX as f64 =>
            {
                *value as u64
            }
            _ => return Err("'timeNs' must be a non-negative integer".into()),
        };
        let Some(Json::Object(inputs)) = fields.get("inputs") else {
            return Err("simulation vector 'inputs' must be an object".into());
        };
        let mut parsed_inputs = BTreeMap::new();
        for (name, value) in inputs {
            let Some(value) = value.as_str() else {
                return Err(format!("simulation input '{name}' must be a string"));
            };
            parsed_inputs.insert(name.clone(), value.to_string());
        }
        vectors.push(RtlSimulationVector {
            time_ns,
            inputs: parsed_inputs,
        });
    }
    Ok(vectors)
}

fn parse_power_options(params: &Json) -> Result<PowerOptions, String> {
    let Some(Json::Object(fields)) = params
        .as_object()
        .map(|fields| fields.get("powerOptions"))
        .and_then(|value| value.cloned())
    else {
        return Ok(PowerOptions::default());
    };
    let mut options = PowerOptions::default();
    for (json_name, target) in [
        ("frequencyMhz", &mut options.frequency_mhz),
        ("baseCellEnergyPj", &mut options.base_cell_energy_pj),
        ("energyPerSitePj", &mut options.energy_per_site_pj),
        (
            "capacitiveEnergyPerPfPj",
            &mut options.capacitive_energy_per_pf_pj,
        ),
        ("leakagePerSiteNw", &mut options.leakage_per_site_nw),
    ] {
        if let Some(value) = fields.get(json_name) {
            let Some(value) = value.as_number() else {
                return Err(format!("power option '{json_name}' must be a number"));
            };
            *target = *value;
        }
    }
    Ok(options)
}

fn parse_ir_options(params: &Json) -> Result<IrOptions, String> {
    let Some(Json::Object(fields)) = params
        .as_object()
        .map(|fields| fields.get("irOptions"))
        .and_then(|value| value.cloned())
    else {
        return Ok(IrOptions::default());
    };
    let mut options = IrOptions::default();
    for (json_name, target) in [
        ("voltageV", &mut options.voltage_v),
        ("vddOhmPerSite", &mut options.vdd_ohm_per_site),
        ("gndOhmPerSite", &mut options.gnd_ohm_per_site),
        ("maxTotalDropV", &mut options.max_total_drop_v),
        ("maxRailCurrentA", &mut options.max_rail_current_a),
    ] {
        if let Some(value) = fields.get(json_name) {
            let Some(value) = value.as_number() else {
                return Err(format!("IR option '{json_name}' must be a number"));
            };
            *target = *value;
        }
    }
    Ok(options)
}

fn parse_scan_options(params: &Json) -> Result<ScanOptions, String> {
    let Some(Json::Object(fields)) = params
        .as_object()
        .map(|fields| fields.get("scanOptions"))
        .and_then(|value| value.cloned())
    else {
        return Ok(ScanOptions::default());
    };
    let mut options = ScanOptions::default();
    for (json_name, target) in [
        ("chainName", &mut options.chain_name),
        ("scanEnPort", &mut options.scan_en_port),
        ("scanInPort", &mut options.scan_in_port),
        ("scanOutPort", &mut options.scan_out_port),
    ] {
        if let Some(value) = fields.get(json_name) {
            let Some(value) = value.as_str() else {
                return Err(format!("scan option '{json_name}' must be a string"));
            };
            *target = value.to_string();
        }
    }
    Ok(options)
}

fn parse_pattern_options(params: &Json) -> Result<PatternOptions, String> {
    let Some(Json::Object(fields)) = params
        .as_object()
        .map(|fields| fields.get("patternOptions"))
        .and_then(|value| value.cloned())
    else {
        return Ok(PatternOptions::default());
    };
    let mut options = PatternOptions::default();
    if let Some(value) = fields.get("maxRandomPatterns") {
        let Json::Number(value) = value else {
            return Err("pattern option 'maxRandomPatterns' must be a positive integer".into());
        };
        if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
            return Err("pattern option 'maxRandomPatterns' must be a positive integer".into());
        }
        options.max_random_patterns = *value as usize;
    }
    if let Some(value) = fields.get("exhaustiveInputLimit") {
        let Json::Number(value) = value else {
            return Err(
                "pattern option 'exhaustiveInputLimit' must be an integer from 1 through 64".into(),
            );
        };
        if *value < 1.0 || value.fract() != 0.0 || *value > 64.0 {
            return Err(
                "pattern option 'exhaustiveInputLimit' must be an integer from 1 through 64".into(),
            );
        }
        options.exhaustive_input_limit = *value as usize;
    }
    if let Some(value) = fields.get("randomSeed") {
        let Json::Number(value) = value else {
            return Err("pattern option 'randomSeed' must be a non-negative integer".into());
        };
        if *value < 0.0 || value.fract() != 0.0 || *value > u64::MAX as f64 {
            return Err("pattern option 'randomSeed' must be a non-negative integer".into());
        }
        options.random_seed = *value as u64;
    }
    Ok(options)
}

fn pdk_signoff(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let manifest = required_string(params, "pdk")?;
    let fields = params.as_object().ok_or("params must be an object")?;
    let top = match fields.get("top") {
        None | Some(Json::Null) => None,
        Some(value) => Some(value.as_str().ok_or("'top' must be a string")?),
    };
    let pdk = parse_pdk(&manifest)?;
    let result = run_pdk_signoff_cancellable(&source, top, &pdk, cancellation)?;
    Ok(Json::object([
        ("passed", Json::Bool(result.passed)),
        ("pdk", Json::string(result.pdk_name.clone())),
        ("version", Json::string(result.pdk_version.clone())),
        ("design", Json::string(result.design_name.clone())),
        (
            "layoutRuleDeckSha256",
            result
                .layout_rule_deck_sha256
                .clone()
                .map_or(Json::Null, Json::string),
        ),
        (
            "layoutRuleDeckValidated",
            Json::Bool(result.layout_rule_deck_validated),
        ),
        ("report", Json::string(report_pdk_signoff(&result))),
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

#[derive(Clone, Debug, PartialEq)]
pub enum RpcJobStatus {
    Queued,
    Running,
    Completed(Json),
    Failed(String),
    Cancelled,
}

enum WorkerCommand {
    Submit(u64, RpcRequest, Option<FlowCancellation>),
    Shutdown,
}

type SharedJobs = Arc<Mutex<BTreeMap<u64, RpcJobStatus>>>;
type SharedRunning = Arc<Mutex<BTreeMap<u64, FlowCancellation>>>;

/// Typed, persistent job queue backed by one deterministic worker thread.
pub struct RpcJobManager {
    next_id: AtomicU64,
    jobs: SharedJobs,
    running: SharedRunning,
    sender: Sender<WorkerCommand>,
    worker: Option<JoinHandle<()>>,
}

impl RpcJobManager {
    pub fn new() -> Self {
        Self::new_with_dispatcher(Arc::new(|request, cancellation| {
            dispatch_result_cancellable(&request, &cancellation)
        }))
    }

    fn new_with_dispatcher(dispatcher: RpcDispatcher) -> Self {
        let (sender, receiver) = channel();
        let jobs = Arc::new(Mutex::new(BTreeMap::new()));
        let worker_jobs = Arc::clone(&jobs);
        let running = Arc::new(Mutex::new(BTreeMap::new()));
        let worker_running = Arc::clone(&running);
        let worker_dispatcher = Arc::clone(&dispatcher);
        let worker =
            spawn(move || worker_loop(receiver, worker_jobs, worker_running, worker_dispatcher));
        Self {
            next_id: AtomicU64::new(1),
            jobs,
            running,
            sender,
            worker: Some(worker),
        }
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

    pub fn poll(&self, id: u64) -> Result<Option<RpcJobStatus>, String> {
        Ok(self
            .jobs
            .lock()
            .map_err(|_| "job state mutex poisoned".to_string())?
            .get(&id)
            .cloned())
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
        self.sender
            .send(WorkerCommand::Shutdown)
            .map_err(|_| "daemon worker already stopped".to_string())?;
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| "daemon worker panicked".to_string())?;
        }
        Ok(())
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
    receiver: Receiver<WorkerCommand>,
    jobs: SharedJobs,
    running: SharedRunning,
    dispatcher: RpcDispatcher,
) {
    while let Ok(command) = receiver.recv() {
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
        let listener = TcpListener::bind(address)
            .map_err(|error| format!("failed to bind EDA daemon to {address}: {error}"))?;
        let local_address = listener
            .local_addr()
            .map(|address| address.to_string())
            .map_err(|error| format!("failed to resolve EDA daemon address: {error}"))?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let service = RpcService::new();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::io::Read;

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

        let add =
            RpcRequest::new(85, "timingWaiverRegistryAddWaivers").with_params(Json::object([
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
                &RpcRequest::new(91, "jobSubmit")
                    .with_params(Json::object([("request", nested_add)])),
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
        let source =
            "module top(input a,b,c, output y); wire n; assign n=a&b; assign y=n|c; endmodule";
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

    #[test]
    fn rpc_layout_drc_parses_polygons_and_vias() {
        let request = RpcRequest::new(9, "layoutDrc").with_params(Json::object([
            (
                "layerRules",
                Json::Array(vec![Json::object([
                    ("layer", Json::string("M1")),
                    ("minWidth", Json::Number(10.0)),
                    ("minSpacing", Json::Number(20.0)),
                    ("minArea", Json::Number(100.0)),
                ])]),
            ),
            (
                "shapes",
                Json::Array(vec![
                    Json::object([
                        ("id", Json::string("a")),
                        ("layer", Json::string("M1")),
                        ("net", Json::string("a")),
                        (
                            "rect",
                            Json::Array(vec![
                                Json::Array(vec![Json::Number(0.0), Json::Number(0.0)]),
                                Json::Array(vec![Json::Number(50.0), Json::Number(50.0)]),
                            ]),
                        ),
                    ]),
                    Json::object([
                        ("id", Json::string("b")),
                        ("layer", Json::string("M1")),
                        ("net", Json::string("b")),
                        (
                            "rect",
                            Json::Array(vec![
                                Json::Array(vec![Json::Number(60.0), Json::Number(0.0)]),
                                Json::Array(vec![Json::Number(110.0), Json::Number(50.0)]),
                            ]),
                        ),
                    ]),
                ]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["clean"], Json::Bool(false));
        assert_eq!(fields["errorCount"], Json::from(1_u64));
        let Json::Array(violations) = &fields["violations"] else {
            panic!("layout DRC response has no violation array");
        };
        assert_eq!(violations.len(), 1);
        let Json::Object(violation) = &violations[0] else {
            panic!("layout violation is not an object");
        };
        assert_eq!(violation["code"], Json::string("LAYOUT.SPACING"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
            "RPC job cancelled"
        );
    }

    #[test]
    fn rpc_layout_drc_checks_layer_density() {
        let request = RpcRequest::new(100, "layoutDrc").with_params(Json::object([
            (
                "layerRules",
                Json::Array(vec![Json::object([
                    ("layer", Json::string("M1")),
                    ("minWidth", Json::from(10_i64)),
                    ("minSpacing", Json::from(20_i64)),
                    ("minArea", Json::from(100_i64)),
                    ("minDensityBp", Json::from(2_000_i64)),
                    ("maxDensityBp", Json::from(8_000_i64)),
                    ("densityWindow", Json::from(100_i64)),
                ])]),
            ),
            (
                "shapes",
                Json::Array(vec![
                    Json::object([
                        ("id", Json::string("overlap1")),
                        ("layer", Json::string("M1")),
                        ("net", Json::string("a")),
                        (
                            "rect",
                            Json::Array(vec![
                                Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                                Json::Array(vec![Json::from(50_i64), Json::from(50_i64)]),
                            ]),
                        ),
                    ]),
                    Json::object([
                        ("id", Json::string("overlap2")),
                        ("layer", Json::string("M1")),
                        ("net", Json::string("a")),
                        (
                            "rect",
                            Json::Array(vec![
                                Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                                Json::Array(vec![Json::from(50_i64), Json::from(50_i64)]),
                            ]),
                        ),
                    ]),
                ]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["clean"], Json::Bool(true));
        assert_eq!(fields["violationCount"], Json::from(0_u64));

        let mut sparse = request.params.clone();
        let Json::Object(sparse_fields) = &mut sparse else {
            panic!("params should be an object");
        };
        sparse_fields.insert(
            "shapes".into(),
            Json::Array(vec![Json::object([
                ("id", Json::string("sparse")),
                ("layer", Json::string("M1")),
                ("net", Json::string("a")),
                (
                    "rect",
                    Json::Array(vec![
                        Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                        Json::Array(vec![Json::from(10_i64), Json::from(10_i64)]),
                    ]),
                ),
            ])]),
        );
        let sparse_response = dispatch(&RpcRequest::new(101, "layoutDrc").with_params(sparse))
            .result
            .unwrap();
        let sparse_result = sparse_response.as_object().unwrap();
        assert_eq!(sparse_result["clean"], Json::Bool(false));
        let Json::Array(violations) = &sparse_result["violations"] else {
            panic!("density DRC response should contain violations");
        };
        let violation = violations[0].as_object().unwrap();
        assert_eq!(violation["code"], Json::string("LAYOUT.MIN_DENSITY"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
            "RPC job cancelled"
        );
    }

    #[test]
    fn rpc_layout_drc_checks_cross_layer_spacing() {
        let request = RpcRequest::new(106, "layoutDrc").with_params(Json::object([
            (
                "layerRules",
                Json::Array(vec![
                    Json::object([
                        ("layer", Json::string("M1")),
                        ("minWidth", Json::from(10_i64)),
                        ("minSpacing", Json::from(20_i64)),
                        ("minArea", Json::from(100_i64)),
                        ("spacingTo", Json::object([("M2", Json::from(30_i64))])),
                    ]),
                    Json::object([
                        ("layer", Json::string("M2")),
                        ("minWidth", Json::from(10_i64)),
                        ("minSpacing", Json::from(20_i64)),
                        ("minArea", Json::from(100_i64)),
                        ("spacingTo", Json::object([("M1", Json::from(40_i64))])),
                    ]),
                ]),
            ),
            (
                "shapes",
                Json::Array(vec![
                    Json::object([
                        ("id", Json::string("m1")),
                        ("layer", Json::string("M1")),
                        ("net", Json::string("a")),
                        (
                            "rect",
                            Json::Array(vec![
                                Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                                Json::Array(vec![Json::from(50_i64), Json::from(10_i64)]),
                            ]),
                        ),
                    ]),
                    Json::object([
                        ("id", Json::string("m2")),
                        ("layer", Json::string("M2")),
                        ("net", Json::string("b")),
                        (
                            "rect",
                            Json::Array(vec![
                                Json::Array(vec![Json::from(0_i64), Json::from(45_i64)]),
                                Json::Array(vec![Json::from(50_i64), Json::from(55_i64)]),
                            ]),
                        ),
                    ]),
                ]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["clean"], Json::Bool(false));
        let Json::Array(violations) = &fields["violations"] else {
            panic!("cross-layer DRC response should contain violations");
        };
        assert!(violations.iter().any(|violation| {
            let violation = violation.as_object().unwrap();
            violation["code"] == Json::string("LAYOUT.SPACING")
                && violation["message"]
                    .as_str()
                    .unwrap()
                    .contains("below layer minimum 40")
        }));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
            "RPC job cancelled"
        );
    }

    #[test]
    fn rpc_layout_drc_rejects_different_net_overlap() {
        let request = RpcRequest::new(107, "layoutDrc").with_params(Json::object([
            (
                "layerRules",
                Json::Array(vec![Json::object([
                    ("layer", Json::string("M1")),
                    ("minWidth", Json::from(10_i64)),
                    ("minSpacing", Json::from(0_i64)),
                    ("minArea", Json::from(100_i64)),
                ])]),
            ),
            (
                "shapes",
                Json::Array(vec![
                    Json::object([
                        ("id", Json::string("left")),
                        ("layer", Json::string("M1")),
                        ("net", Json::string("a")),
                        (
                            "rect",
                            Json::Array(vec![
                                Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                                Json::Array(vec![Json::from(20_i64), Json::from(20_i64)]),
                            ]),
                        ),
                    ]),
                    Json::object([
                        ("id", Json::string("right")),
                        ("layer", Json::string("M1")),
                        ("net", Json::string("b")),
                        (
                            "rect",
                            Json::Array(vec![
                                Json::Array(vec![Json::from(10_i64), Json::from(0_i64)]),
                                Json::Array(vec![Json::from(30_i64), Json::from(20_i64)]),
                            ]),
                        ),
                    ]),
                ]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["clean"], Json::Bool(false));
        let Json::Array(violations) = &fields["violations"] else {
            panic!("short DRC response should contain violations");
        };
        assert!(violations.iter().any(|violation| {
            let violation = violation.as_object().unwrap();
            violation["code"] == Json::string("LAYOUT.SHORT")
                && violation["object"] == Json::string("left|right")
        }));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
            "RPC job cancelled"
        );
    }

    #[test]
    fn rpc_layout_rule_deck_normalizes_typed_rules() {
        let deck = r#"
version 1
layer M1 min_width 10 min_spacing 20 min_area 100 min_density_bp 2000 max_density_bp 8000 density_window 100 min_notch 25
layer M2 min_width 10 min_spacing 20 min_area 100
spacing M1 M2 30
via VIA1 cut_layer V1 bottom_layer M1 top_layer M2 cut_width 20 cut_height 20 bottom_enclosure 1 top_enclosure 2 bottom_extension_x 10 bottom_extension_y 5 top_extension_x 6 top_extension_y 11 cut_spacing 20
# typed rule deck comments are allowed
"#;
        let request = RpcRequest::new(108, "layoutRuleDeck")
            .with_params(Json::object([("deck", Json::string(deck))]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["valid"], Json::Bool(true));
        assert_eq!(fields["layerCount"], Json::from(2_u64));
        assert_eq!(fields["crossLayerSpacingCount"], Json::from(1_u64));
        assert_eq!(fields["viaRuleCount"], Json::from(1_u64));
        let normalized = fields["normalized"].as_str().unwrap();
        assert!(normalized.contains("min_density_bp 2000"));
        assert!(normalized.contains("spacing M1 M2 30"));
        assert!(normalized.contains("cut_spacing 20"));

        let invalid = RpcRequest::new(109, "layoutRuleDeck")
            .with_params(Json::object([("deck", Json::string("version 2\n"))]));
        assert!(dispatch(&invalid).result.unwrap_err().contains("version"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
            "RPC job cancelled"
        );
    }

    #[test]
    fn rpc_layout_drc_applies_auditable_waivers() {
        let request = RpcRequest::new(110, "layoutDrc").with_params(Json::object([
            (
                "layerRules",
                Json::Array(vec![Json::object([
                    ("layer", Json::string("M1")),
                    ("minWidth", Json::from(10_i64)),
                    ("minSpacing", Json::from(20_i64)),
                    ("minArea", Json::from(100_i64)),
                ])]),
            ),
            (
                "shapes",
                Json::Array(vec![Json::object([
                    ("id", Json::string("narrow")),
                    ("layer", Json::string("M1")),
                    ("net", Json::string("a")),
                    (
                        "rect",
                        Json::Array(vec![
                            Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                            Json::Array(vec![Json::from(5_i64), Json::from(40_i64)]),
                        ]),
                    ),
                ])]),
            ),
            (
                "waivers",
                Json::Array(vec![Json::object([
                    ("id", Json::string("w1")),
                    ("code", Json::string("LAYOUT.MIN_WIDTH")),
                    ("layer", Json::string("M1")),
                    ("net", Json::string("a")),
                    ("object", Json::string("narrow")),
                    ("owner", Json::string("physical-owner")),
                    ("reason", Json::string("approved review exception")),
                ])]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["clean"], Json::Bool(true));
        assert_eq!(fields["violationCount"], Json::from(0_u64));
        assert_eq!(fields["preWaiverViolationCount"], Json::from(1_u64));
        assert_eq!(fields["preWaiverErrorCount"], Json::from(1_u64));
        assert_eq!(fields["waiverCount"], Json::from(1_u64));
        assert_eq!(fields["waivedViolationCount"], Json::from(1_u64));
        assert_eq!(
            fields["usedWaiverIds"],
            Json::Array(vec![Json::string("w1")])
        );
        let Json::Array(waived) = &fields["waivedViolations"] else {
            panic!("waived violations should be an array");
        };
        let waived = waived[0].as_object().unwrap();
        assert_eq!(waived["waiverId"], Json::string("w1"));
        assert_eq!(waived["owner"], Json::string("physical-owner"));
        assert_eq!(waived["reason"], Json::string("approved review exception"));
        let waived_violation = waived["violation"].as_object().unwrap();
        assert_eq!(waived_violation["code"], Json::string("LAYOUT.MIN_WIDTH"));
        assert!(fields["waiverReport"]
            .as_str()
            .unwrap()
            .contains("waived LAYOUT.MIN_WIDTH [narrow] by 'w1'"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
            "RPC job cancelled"
        );
    }

    #[test]
    fn rpc_service_persists_layout_drc_waiver_registry() {
        let service = RpcService::new();
        let empty = service
            .dispatch(&RpcRequest::new(111, "layoutDrcWaiverRegistryList"))
            .result
            .unwrap();
        assert_eq!(empty.as_object().unwrap()["count"], Json::from(0_u64));

        let create =
            RpcRequest::new(112, "layoutDrcWaiverRegistryCreate").with_params(Json::object([
                ("registryId", Json::string("phys")),
                ("actor", Json::string("physical-owner")),
                (
                    "waivers",
                    Json::Array(vec![Json::object([
                        ("id", Json::string("w1")),
                        ("code", Json::string("LAYOUT.MIN_WIDTH")),
                        ("layer", Json::string("M1")),
                        ("net", Json::string("a")),
                        ("object", Json::string("narrow")),
                        ("owner", Json::string("physical-owner")),
                        ("reason", Json::string("approved review exception")),
                    ])]),
                ),
            ]));
        let created = service.dispatch(&create).result.unwrap();
        let created_fields = created.as_object().unwrap();
        assert_eq!(created_fields["activeWaiverCount"], Json::from(1_u64));
        assert_eq!(created_fields["auditEventCount"], Json::from(1_u64));
        assert!(created_fields["report"]
            .as_str()
            .unwrap()
            .contains("#1 added 'w1' by physical-owner"));

        let drc = RpcRequest::new(113, "layoutDrc").with_params(Json::object([
            (
                "layerRules",
                Json::Array(vec![Json::object([
                    ("layer", Json::string("M1")),
                    ("minWidth", Json::from(10_i64)),
                    ("minSpacing", Json::from(20_i64)),
                    ("minArea", Json::from(100_i64)),
                ])]),
            ),
            (
                "shapes",
                Json::Array(vec![Json::object([
                    ("id", Json::string("narrow")),
                    ("layer", Json::string("M1")),
                    ("net", Json::string("a")),
                    (
                        "rect",
                        Json::Array(vec![
                            Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                            Json::Array(vec![Json::from(5_i64), Json::from(40_i64)]),
                        ]),
                    ),
                ])]),
            ),
            ("waiverRegistryId", Json::string("phys")),
        ]));
        let waived = service.dispatch(&drc).result.unwrap();
        let waived_fields = waived.as_object().unwrap();
        assert_eq!(waived_fields["clean"], Json::Bool(true));
        assert_eq!(waived_fields["waivedViolationCount"], Json::from(1_u64));

        let nested_drc = parse_json(&drc.wire()).unwrap();
        let submitted = service
            .dispatch(
                &RpcRequest::new(116, "jobSubmit")
                    .with_params(Json::object([("request", nested_drc)])),
            )
            .result
            .unwrap();
        let job_id = submitted.as_object().unwrap()["jobId"].clone();
        let waited = service
            .dispatch(&RpcRequest::new(117, "jobWait").with_params(Json::object([
                ("jobId", job_id),
                ("timeoutMs", Json::Number(2_000.0)),
            ])))
            .result
            .unwrap();
        let waited_fields = waited.as_object().unwrap();
        assert_eq!(waited_fields["state"], Json::string("completed"));
        assert_eq!(
            waited_fields["result"].as_object().unwrap()["clean"],
            Json::Bool(true)
        );

        let remove = RpcRequest::new(114, "layoutDrcWaiverRegistryRemoveWaiver").with_params(
            Json::object([
                ("registryId", Json::string("phys")),
                ("waiverId", Json::string("w1")),
                ("actor", Json::string("reviewer")),
            ]),
        );
        let removed = service.dispatch(&remove).result.unwrap();
        let removed_fields = removed.as_object().unwrap();
        assert_eq!(removed_fields["activeWaiverCount"], Json::from(0_u64));
        assert_eq!(removed_fields["auditEventCount"], Json::from(2_u64));
        assert!(removed_fields["report"]
            .as_str()
            .unwrap()
            .contains("#2 removed 'w1' by reviewer"));

        let restored = service.dispatch(&drc).result.unwrap();
        assert_eq!(restored.as_object().unwrap()["clean"], Json::Bool(false));

        let delete = RpcRequest::new(115, "layoutDrcWaiverRegistryDelete")
            .with_params(Json::object([("registryId", Json::string("phys"))]));
        let deleted = service.dispatch(&delete).result.unwrap();
        assert_eq!(deleted.as_object().unwrap()["remaining"], Json::from(0_u64));
    }

    #[test]
    fn rpc_layout_drc_checks_via_cut_spacing() {
        let via_json = |x: i64| {
            Json::object([
                ("id", Json::string(if x == 0 { "left" } else { "near" })),
                ("rule", Json::string("VIA1")),
                ("net", Json::string("a")),
                ("cutLayer", Json::string("V1")),
                ("bottomLayer", Json::string("M1")),
                ("topLayer", Json::string("M2")),
                (
                    "cut",
                    Json::Array(vec![
                        Json::Array(vec![Json::from(x), Json::from(0_i64)]),
                        Json::Array(vec![Json::from(x + 20), Json::from(20_i64)]),
                    ]),
                ),
            ])
        };
        let request = RpcRequest::new(102, "layoutDrc").with_params(Json::object([
            (
                "viaRules",
                Json::Array(vec![Json::object([
                    ("name", Json::string("VIA1")),
                    ("cutLayer", Json::string("V1")),
                    ("bottomLayer", Json::string("M1")),
                    ("topLayer", Json::string("M2")),
                    ("cutWidth", Json::from(20_i64)),
                    ("cutHeight", Json::from(20_i64)),
                    ("minBottomEnclosure", Json::from(0_i64)),
                    ("minTopEnclosure", Json::from(0_i64)),
                    ("minCutSpacing", Json::from(20_i64)),
                ])]),
            ),
            ("vias", Json::Array(vec![via_json(0), via_json(30)])),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["clean"], Json::Bool(false));
        let Json::Array(violations) = &fields["violations"] else {
            panic!("via DRC response should contain violations");
        };
        let cut_spacing = violations
            .iter()
            .map(|violation| violation.as_object().unwrap())
            .find(|violation| violation["code"] == Json::string("LAYOUT.VIA_CUT_SPACING"))
            .expect("via cut spacing violation");
        assert_eq!(cut_spacing["object"], Json::string("left|near"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
            "RPC job cancelled"
        );
    }

    #[test]
    fn rpc_layout_drc_rejects_overlapping_via_cuts() {
        let via_json = |id: &str, x1: i64| {
            Json::object([
                ("id", Json::string(id)),
                ("rule", Json::string("VIA1")),
                ("net", Json::string("a")),
                ("cutLayer", Json::string("V1")),
                ("bottomLayer", Json::string("M1")),
                ("topLayer", Json::string("M2")),
                (
                    "cut",
                    Json::Array(vec![
                        Json::Array(vec![Json::from(x1), Json::from(0_i64)]),
                        Json::Array(vec![Json::from(x1 + 20), Json::from(20_i64)]),
                    ]),
                ),
            ])
        };
        let request = RpcRequest::new(105, "layoutDrc").with_params(Json::object([
            (
                "viaRules",
                Json::Array(vec![Json::object([
                    ("name", Json::string("VIA1")),
                    ("cutLayer", Json::string("V1")),
                    ("bottomLayer", Json::string("M1")),
                    ("topLayer", Json::string("M2")),
                    ("cutWidth", Json::from(20_i64)),
                    ("cutHeight", Json::from(20_i64)),
                    ("minBottomEnclosure", Json::from(0_i64)),
                    ("minTopEnclosure", Json::from(0_i64)),
                ])]),
            ),
            (
                "vias",
                Json::Array(vec![via_json("left", 0), via_json("right", 10)]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["clean"], Json::Bool(false));
        let Json::Array(violations) = &fields["violations"] else {
            panic!("via overlap DRC response should contain violations");
        };
        assert!(violations.iter().any(|violation| {
            let violation = violation.as_object().unwrap();
            violation["code"] == Json::string("LAYOUT.VIA_CUT_OVERLAP")
                && violation["object"] == Json::string("left|right")
        }));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
            "RPC job cancelled"
        );
    }

    #[test]
    fn rpc_layout_drc_checks_minimum_notch() {
        let request = RpcRequest::new(103, "layoutDrc").with_params(Json::object([
            (
                "layerRules",
                Json::Array(vec![Json::object([
                    ("layer", Json::string("M1")),
                    ("minWidth", Json::from(10_i64)),
                    ("minSpacing", Json::from(20_i64)),
                    ("minArea", Json::from(100_i64)),
                    ("minNotch", Json::from(25_i64)),
                ])]),
            ),
            (
                "shapes",
                Json::Array(vec![Json::object([
                    ("id", Json::string("narrow")),
                    ("layer", Json::string("M1")),
                    ("net", Json::string("a")),
                    (
                        "polygon",
                        Json::Array(vec![
                            Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                            Json::Array(vec![Json::from(100_i64), Json::from(0_i64)]),
                            Json::Array(vec![Json::from(100_i64), Json::from(100_i64)]),
                            Json::Array(vec![Json::from(60_i64), Json::from(100_i64)]),
                            Json::Array(vec![Json::from(60_i64), Json::from(40_i64)]),
                            Json::Array(vec![Json::from(40_i64), Json::from(40_i64)]),
                            Json::Array(vec![Json::from(40_i64), Json::from(100_i64)]),
                            Json::Array(vec![Json::from(0_i64), Json::from(100_i64)]),
                        ]),
                    ),
                ])]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["clean"], Json::Bool(false));
        let Json::Array(violations) = &fields["violations"] else {
            panic!("notch DRC response should contain violations");
        };
        let violation = violations[0].as_object().unwrap();
        assert_eq!(violation["code"], Json::string("LAYOUT.MIN_NOTCH"));
        assert!(violation["message"]
            .as_str()
            .unwrap()
            .contains("notch 20 is below layer minimum 25"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
            "RPC job cancelled"
        );
    }

    #[test]
    fn rpc_layout_drc_checks_directional_via_extensions() {
        let request_for = |bottom_x1: i64| {
            RpcRequest::new(104, "layoutDrc").with_params(Json::object([
                (
                    "layerRules",
                    Json::Array(vec![
                        Json::object([
                            ("layer", Json::string("M1")),
                            ("minWidth", Json::from(10_i64)),
                            ("minSpacing", Json::from(20_i64)),
                            ("minArea", Json::from(100_i64)),
                        ]),
                        Json::object([
                            ("layer", Json::string("M2")),
                            ("minWidth", Json::from(10_i64)),
                            ("minSpacing", Json::from(20_i64)),
                            ("minArea", Json::from(100_i64)),
                        ]),
                    ]),
                ),
                (
                    "viaRules",
                    Json::Array(vec![Json::object([
                        ("name", Json::string("VIA1")),
                        ("cutLayer", Json::string("V1")),
                        ("bottomLayer", Json::string("M1")),
                        ("topLayer", Json::string("M2")),
                        ("cutWidth", Json::from(20_i64)),
                        ("cutHeight", Json::from(20_i64)),
                        ("minBottomEnclosure", Json::from(0_i64)),
                        ("minTopEnclosure", Json::from(0_i64)),
                        ("minBottomExtensionX", Json::from(10_i64)),
                        ("minBottomExtensionY", Json::from(5_i64)),
                        ("minTopExtensionX", Json::from(5_i64)),
                        ("minTopExtensionY", Json::from(10_i64)),
                    ])]),
                ),
                (
                    "shapes",
                    Json::Array(vec![
                        Json::object([
                            ("id", Json::string("bottom")),
                            ("layer", Json::string("M1")),
                            ("net", Json::string("a")),
                            (
                                "rect",
                                Json::Array(vec![
                                    Json::Array(vec![Json::from(bottom_x1), Json::from(-5_i64)]),
                                    Json::Array(vec![Json::from(30_i64), Json::from(25_i64)]),
                                ]),
                            ),
                        ]),
                        Json::object([
                            ("id", Json::string("top")),
                            ("layer", Json::string("M2")),
                            ("net", Json::string("a")),
                            (
                                "rect",
                                Json::Array(vec![
                                    Json::Array(vec![Json::from(-5_i64), Json::from(-10_i64)]),
                                    Json::Array(vec![Json::from(25_i64), Json::from(30_i64)]),
                                ]),
                            ),
                        ]),
                    ]),
                ),
                (
                    "vias",
                    Json::Array(vec![Json::object([
                        ("id", Json::string("via")),
                        ("rule", Json::string("VIA1")),
                        ("net", Json::string("a")),
                        ("cutLayer", Json::string("V1")),
                        ("bottomLayer", Json::string("M1")),
                        ("topLayer", Json::string("M2")),
                        (
                            "cut",
                            Json::Array(vec![
                                Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                                Json::Array(vec![Json::from(20_i64), Json::from(20_i64)]),
                            ]),
                        ),
                    ])]),
                ),
            ]))
        };
        let clean = dispatch(&request_for(-10)).result.unwrap();
        assert_eq!(clean.as_object().unwrap()["clean"], Json::Bool(true));

        let short = dispatch(&request_for(-5)).result.unwrap();
        let fields = short.as_object().unwrap();
        assert_eq!(fields["clean"], Json::Bool(false));
        let Json::Array(violations) = &fields["violations"] else {
            panic!("extension DRC response should contain violations");
        };
        assert!(violations.iter().any(|violation| {
            let violation = violation.as_object().unwrap();
            violation["code"] == Json::string("LAYOUT.VIA_ENCLOSURE")
                && violation["message"]
                    .as_str()
                    .unwrap()
                    .contains("extension (0x10, 0x5)")
        }));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            dispatch_result_cancellable(&request_for(-10), &cancellation).unwrap_err(),
            "RPC job cancelled"
        );
    }

    #[test]
    fn rpc_layout_any_angle_route_returns_exact_segments() {
        let request = RpcRequest::new(93, "layoutAnyAngleRoute").with_params(Json::object([
            (
                "requests",
                Json::Array(vec![
                    Json::object([
                        ("net", Json::from(7_u64)),
                        (
                            "start",
                            Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                        ),
                        (
                            "end",
                            Json::Array(vec![Json::from(3_i64), Json::from(5_i64)]),
                        ),
                    ]),
                    Json::object([
                        ("net", Json::from(8_u64)),
                        (
                            "start",
                            Json::Array(vec![Json::from(10_i64), Json::from(0_i64)]),
                        ),
                        (
                            "end",
                            Json::Array(vec![Json::from(13_i64), Json::from(5_i64)]),
                        ),
                    ]),
                ]),
            ),
            ("blockages", Json::Array(Vec::new())),
            ("options", Json::object([("minSpacing", Json::from(0_i64))])),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["routeCount"], Json::from(2_u64));
        assert_eq!(fields["blockageCount"], Json::from(0_u64));
        assert_eq!(fields["minSpacing"], Json::from(0_u64));
        let Json::Array(routes) = &fields["routes"] else {
            panic!("any-angle routes should be an array");
        };
        let route = routes[0].as_object().unwrap();
        assert_eq!(route["net"], Json::from(7_u64));
        assert_eq!(route["lengthSquaredNumerator"], Json::string("34"));
        assert_eq!(route["lengthSquaredDenominator"], Json::string("1"));
        assert!((route["length"].as_number().unwrap() - f64::sqrt(34.0)).abs() < 1e-12);
        assert!((fields["totalLength"].as_number().unwrap() - 2.0 * f64::sqrt(34.0)).abs() < 1e-12);

        let mut blocked = request.params.clone();
        let blocked_fields = match &mut blocked {
            Json::Object(fields) => fields,
            _ => panic!("params should be an object"),
        };
        blocked_fields.insert(
            "blockages".into(),
            Json::Array(vec![Json::object([(
                "points",
                Json::Array(vec![
                    Json::Array(vec![Json::from(-2_i64), Json::from(-2_i64)]),
                    Json::Array(vec![Json::from(2_i64), Json::from(-2_i64)]),
                    Json::Array(vec![Json::from(0_i64), Json::from(3_i64)]),
                ]),
            )])]),
        );
        blocked_fields.insert(
            "requests".into(),
            Json::Array(vec![Json::object([
                ("net", Json::from(0_u64)),
                (
                    "start",
                    Json::Array(vec![Json::from(-4_i64), Json::from(0_i64)]),
                ),
                (
                    "end",
                    Json::Array(vec![Json::from(4_i64), Json::from(0_i64)]),
                ),
            ])]),
        );
        let error = dispatch_result_cancellable(
            &RpcRequest::new(94, "layoutAnyAngleRoute").with_params(blocked),
            &FlowCancellation::new(),
        )
        .unwrap_err();
        assert_eq!(error, "any-angle route for net 0 crosses a layout blockage");

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
            "RPC job cancelled"
        );
    }

    #[test]
    fn rpc_layout_any_angle_maze_route_detours_around_blockage() {
        let request = RpcRequest::new(98, "layoutAnyAngleMazeRoute").with_params(Json::object([
            (
                "requests",
                Json::Array(vec![Json::object([
                    ("net", Json::from(4_u64)),
                    (
                        "start",
                        Json::Array(vec![Json::from(-10_i64), Json::from(0_i64)]),
                    ),
                    (
                        "end",
                        Json::Array(vec![Json::from(10_i64), Json::from(0_i64)]),
                    ),
                ])]),
            ),
            (
                "blockages",
                Json::Array(vec![Json::object([(
                    "points",
                    Json::Array(vec![
                        Json::Array(vec![Json::from(-2_i64), Json::from(-2_i64)]),
                        Json::Array(vec![Json::from(2_i64), Json::from(-2_i64)]),
                        Json::Array(vec![Json::from(0_i64), Json::from(3_i64)]),
                    ]),
                )])]),
            ),
            (
                "options",
                Json::object([
                    ("minSpacing", Json::from(1_i64)),
                    ("maxSearchNodes", Json::from(128_u64)),
                ]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["routeCount"], Json::from(1_u64));
        assert_eq!(fields["blockageCount"], Json::from(1_u64));
        assert_eq!(fields["minSpacing"], Json::from(1_u64));
        assert_eq!(fields["maxSearchNodes"], Json::from(128_u64));
        let Json::Array(routes) = &fields["routes"] else {
            panic!("maze routes should be an array");
        };
        let route = routes[0].as_object().unwrap();
        let segment_count = route["segmentCount"]
            .as_number()
            .copied()
            .unwrap_or_default();
        assert!(segment_count > 1.0, "{route:?}");
        let Json::Array(points) = &route["points"] else {
            panic!("maze route points should be an array");
        };
        assert!(points.len() > 2);
        assert!(route["length"].as_number().unwrap() > &20.0);
        assert!(fields["totalLength"].as_number().unwrap() > &20.0);

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
            "RPC job cancelled"
        );
    }

    #[test]
    fn rpc_layout_any_angle_maze_route_ripgup_resolves_crossing_nets() {
        let request = RpcRequest::new(99, "layoutAnyAngleMazeRoute").with_params(Json::object([
            (
                "requests",
                Json::Array(vec![
                    Json::object([
                        ("net", Json::from(0_u64)),
                        (
                            "start",
                            Json::Array(vec![Json::from(-10_i64), Json::from(0_i64)]),
                        ),
                        (
                            "end",
                            Json::Array(vec![Json::from(10_i64), Json::from(0_i64)]),
                        ),
                    ]),
                    Json::object([
                        ("net", Json::from(1_u64)),
                        (
                            "start",
                            Json::Array(vec![Json::from(0_i64), Json::from(-10_i64)]),
                        ),
                        (
                            "end",
                            Json::Array(vec![Json::from(0_i64), Json::from(10_i64)]),
                        ),
                    ]),
                ]),
            ),
            ("blockages", Json::Array(Vec::new())),
            (
                "options",
                Json::object([
                    ("minSpacing", Json::from(0_i64)),
                    ("maxSearchNodes", Json::from(96_u64)),
                ]),
            ),
            ("ripgup", Json::Bool(true)),
            ("maxPasses", Json::from(8_u64)),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["ripgup"], Json::Bool(true));
        assert_eq!(fields["routeCount"], Json::from(2_u64));
        assert!(fields["passes"].as_number().unwrap() <= &8.0);
        let Json::Array(initial_conflicts) = &fields["initialConflicts"] else {
            panic!("initial conflicts should be an array");
        };
        assert!(!initial_conflicts.is_empty());
        let Json::Array(routes) = &fields["routes"] else {
            panic!("rip-up routes should be an array");
        };
        assert!(routes
            .iter()
            .any(|route| route.as_object().unwrap()["segmentCount"]
                .as_number()
                .unwrap()
                > &1.0));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
            "RPC job cancelled"
        );
    }

    #[test]
    fn rpc_layout_any_angle_boolean_returns_rational_polygons() {
        let polygon = |points: &[i64]| {
            Json::object([(
                "points",
                Json::Array(
                    points
                        .chunks(2)
                        .map(|point| Json::Array(vec![Json::from(point[0]), Json::from(point[1])]))
                        .collect(),
                ),
            )])
        };
        let request = RpcRequest::new(95, "layoutAnyAngleBoolean").with_params(Json::object([
            ("left", polygon(&[0, 0, 6, 0, 6, 6, 0, 6])),
            ("right", polygon(&[3, 3, 9, 3, 9, 9, 3, 9])),
            ("operation", Json::string("union")),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["operation"], Json::string("union"));
        assert_eq!(fields["polygonCount"], Json::from(1_u64));
        assert!((fields["totalArea"].as_number().unwrap() - 63.0).abs() < 1e-12);

        let rational = RpcRequest::new(96, "layoutAnyAngleBoolean").with_params(Json::object([
            ("left", polygon(&[0, 0, 4, 0, 0, 4])),
            ("right", polygon(&[1, 1, 5, 2, 2, 5])),
            ("operation", Json::string("intersection")),
        ]));
        let rational_response = dispatch(&rational).result.unwrap();
        let rational_fields = rational_response.as_object().unwrap();
        assert!((rational_fields["totalArea"].as_number().unwrap() - 1.2).abs() < 1e-12);
        let Json::Array(result_polygons) = &rational_fields["polygons"] else {
            panic!("Boolean result polygons should be an array");
        };
        let result_polygon = result_polygons[0].as_object().unwrap();
        let Json::Array(points) = &result_polygon["points"] else {
            panic!("Boolean result points should be an array");
        };
        assert!(points.iter().any(|point| {
            let point = point.as_object().unwrap();
            point["xDenominator"] == Json::string("5") && point["yDenominator"] == Json::string("5")
        }));

        let mut invalid = rational.params.clone();
        if let Json::Object(fields) = &mut invalid {
            fields.insert("operation".into(), Json::string("xor"));
        }
        assert_eq!(
            dispatch_result_cancellable(
                &RpcRequest::new(97, "layoutAnyAngleBoolean").with_params(invalid),
                &FlowCancellation::new(),
            )
            .unwrap_err(),
            "any-angle Boolean operation must be union, intersection, or difference, found 'xor'"
        );

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
            "RPC job cancelled"
        );
    }

    #[test]
    fn rpc_spice_dc_returns_typed_operating_point() {
        let source = "rpc divider\nVin in 0 DC 1.5\nR1 in out 1k\nR2 out 0 2k\n.end";
        let request = RpcRequest::new(3, "spiceDc")
            .with_params(Json::object([("source", Json::string(source))]));
        let response = dispatch(&request);
        let wire = response.wire();
        assert!(wire.contains("\"converged\":true"), "{wire}");
        assert!(wire.contains("\"out\":1"), "{wire}");

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            spice_dc(&request.params, &cancellation).unwrap_err(),
            "DC operating point RPC cancelled"
        );
    }

    #[test]
    fn rpc_spice_run_dispatches_any_directive_result() {
        let source = "rpc analyses\nVin in 0 DC 1 AC 1\nR1 in out 1k\nC1 out 0 1u\n.ac lin 1 159.155 159.155\n.tran 1u 1m\n.end";
        let request = RpcRequest::new(31, "spiceRun").with_params(Json::object([
            ("source", Json::string(source)),
            ("analysisIndex", Json::Number(0.0)),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["kind"], Json::string("acSweep"));
        assert!(fields["report"].as_str().unwrap().contains("159.155"));

        let request = RpcRequest::new(32, "spiceRun").with_params(Json::object([
            ("source", Json::string(source)),
            ("all", Json::Bool(true)),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(
            fields["kinds"],
            Json::Array(vec![Json::string("acSweep"), Json::string("transient")])
        );
        assert_eq!(fields["count"], Json::Number(2.0));
    }

    #[test]
    fn rpc_spice_run_honors_cooperative_cancellation() {
        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let source = "cancelled rpc transient\nVin in 0 PULSE(0 1 0 1p 1p 1n 2n)\nR1 in out 1k\nC1 out 0 1n\n.tran 1p 2n\n.end";
        let error = spice_run(
            &Json::object([("source", Json::string(source))]),
            &cancellation,
        )
        .unwrap_err();
        assert_eq!(error, "transient analysis cancelled");
    }

    #[test]
    fn rpc_rtl_flow_returns_implementation_artifacts() {
        let source = "module top(input a, output y); assign y = ~a; endmodule";
        let request = RpcRequest::new(4, "rtlFlow").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["equivalent"], Json::Bool(true));
        let timing = fields["timing"].as_object().unwrap();
        assert!(timing["criticalPathDelayNs"]
            .as_number()
            .is_some_and(|value| value.is_finite() && *value >= 0.0));
        let physical = fields["physical"].as_object().unwrap();
        assert!(physical["placedCells"]
            .as_number()
            .is_some_and(|value| *value >= 1.0));
        assert!(physical["drcErrors"]
            .as_number()
            .is_some_and(|value| *value == 0.0));
        assert!(fields["verilog"].as_str().unwrap().contains("module"));
        assert!(fields["lef"].as_str().unwrap().contains("VERSION"));
    }

    #[test]
    fn rpc_rtl_equivalence_returns_structured_formal_result() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let request = RpcRequest::new(6, "rtlEquiv").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["equivalent"], Json::Bool(true));
        assert_eq!(fields["counterexample"], Json::Object(BTreeMap::new()));
        match &fields["notes"] {
            Json::Array(notes) => assert!(notes
                .iter()
                .any(|note| { matches!(note, Json::String(text) if text.contains("localized")) })),
            other => panic!("notes should be an array, got {other:?}"),
        }
        assert!(fields["mappedStats"].as_object().is_some());

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = rtl_equiv(&request.params, &cancellation).unwrap_err();
        assert_eq!(error, "RTL equivalence cancelled");
    }

    #[test]
    fn rpc_rtl_equivalence_views_compares_two_sources() {
        let source_a = "module top(input a, b, output y); assign y = a & b; endmodule";
        let equivalent_b = "module top(input a, b, output y); assign y = ~(~a | ~b); endmodule";
        let request = RpcRequest::new(66, "rtlEquivViews").with_params(Json::object([
            ("sourceA", Json::string(source_a)),
            ("sourceB", Json::string(equivalent_b)),
            ("topA", Json::string("top")),
            ("topB", Json::string("top")),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["equivalent"], Json::Bool(true));
        assert_eq!(fields["counterexample"], Json::Object(BTreeMap::new()));
        assert!(fields["viewA"].as_object().is_some());
        assert!(fields["viewB"].as_object().is_some());

        let changed_b = "module top(input a, b, output y); assign y = a | b; endmodule";
        let request = RpcRequest::new(67, "rtlEquivViews").with_params(Json::object([
            ("sourceA", Json::string(source_a)),
            ("sourceB", Json::string(changed_b)),
            ("topA", Json::string("top")),
            ("topB", Json::string("top")),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["equivalent"], Json::Bool(false));
        assert!(!fields["counterexample"].as_object().unwrap().is_empty());

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            rtl_equiv_views(&request.params, &cancellation).unwrap_err(),
            "RTL view equivalence cancelled"
        );
    }

    #[test]
    fn rpc_rtl_lvs_returns_structured_match_and_mismatch() {
        let reference = "module top(input a, b, output y); assign y = a & b; endmodule";
        let request = RpcRequest::new(61, "rtlLvs").with_params(Json::object([
            ("reference", Json::string(reference)),
            ("layout", Json::string(reference)),
            ("top", Json::string("top")),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["matches"], Json::Bool(true));
        match &fields["cellMatches"] {
            Json::Array(matches) => assert!(!matches.is_empty()),
            other => panic!("cell matches should be an array, got {other:?}"),
        }
        assert!(fields["report"].as_str().unwrap().contains("MATCH"));

        let changed = "module top(input a, b, output y); assign y = a | b; endmodule";
        let request = RpcRequest::new(62, "rtlLvs").with_params(Json::object([
            ("reference", Json::string(reference)),
            ("layout", Json::string(changed)),
            ("top", Json::string("top")),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["matches"], Json::Bool(false));
        assert!(fields["report"].as_str().unwrap().contains("MISMATCH"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            rtl_lvs(&request.params, &cancellation).unwrap_err(),
            "RTL LVS cancelled"
        );
    }

    #[test]
    fn rpc_rtl_power_intent_returns_structured_check() {
        let source = "module m(input a, sleep, output y); assign y = a & sleep; endmodule";
        let upf = r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_port vdd -domain TOP
create_supply_port vss -domain TOP
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
connect_supply_net vdd -ports {vdd}
connect_supply_net vss -ports {vss}
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
"#;
        let request = RpcRequest::new(63, "rtlPowerIntent").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("m")),
            ("upf", Json::string(upf)),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["domainCount"], Json::from(1_u64));
        assert_eq!(fields["supplyNetCount"], Json::from(2_u64));
        assert_eq!(fields["hasErrors"], Json::Bool(false));
        assert!(fields["report"].as_str().unwrap().contains("UPF"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            rtl_power_intent(&request.params, &cancellation).unwrap_err(),
            "RTL power-intent check cancelled"
        );
    }

    #[test]
    fn rpc_rtl_safety_returns_structured_k_induction_result() {
        let source = r#"
            module m(input clk, d, rst, output reg q, output bad);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 1'b0; else q <= d;
                assign bad = q & rst;
            endmodule
        "#;
        let request = RpcRequest::new(64, "rtlSafety").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("m")),
            ("badNet", Json::string("bad")),
            ("depth", Json::Number(8.0)),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["status"], Json::string("proved"));
        assert_eq!(fields["inductive"], Json::Bool(true));
        assert_eq!(fields["hasCounterexample"], Json::Bool(false));
        assert_eq!(fields["failedInvariant"], Json::Null);
        assert!(fields["depthChecked"].as_number().unwrap() > &0.0);
        assert!(fields["report"].as_str().unwrap().contains("induction"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            rtl_safety(&request.params, &cancellation).unwrap_err(),
            "RTL safety proof cancelled"
        );
    }

    #[test]
    fn rpc_rtl_physical_timing_returns_spef_and_sdf() {
        let source = "module top(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let request = RpcRequest::new(65, "rtlPhysicalTiming").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            (
                "physicalTimingOptions",
                Json::object([
                    ("tracksPerChannel", Json::Number(64.0)),
                    ("iterations", Json::Number(1.0)),
                    ("placementPasses", Json::Number(2.0)),
                    ("routingPasses", Json::Number(2.0)),
                    ("wireResistanceOhmPerSite", Json::Number(0.05)),
                    ("wireCapacitancePfPerSite", Json::Number(0.002)),
                ]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert!(fields["baselineDelayNs"]
            .as_number()
            .is_some_and(|value| value.is_finite() && *value > 0.0));
        assert!(fields["bestDelayNs"]
            .as_number()
            .is_some_and(|value| value.is_finite() && *value > 0.0));
        assert!(fields["wirelength"]
            .as_number()
            .is_some_and(|value| *value > 0.0));
        assert!(fields["rcNets"]
            .as_number()
            .is_some_and(|value| *value > 0.0));
        assert_eq!(fields["drcErrors"], Json::from(0_u64));
        assert!(fields["spef"].as_str().unwrap().contains("*SPEF"));
        assert!(fields["sdf"].as_str().unwrap().contains("(SDF"));
        assert!(fields["report"]
            .as_str()
            .unwrap()
            .contains("Physical timing flow"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            rtl_physical_timing(&request.params, &cancellation).unwrap_err(),
            "RTL physical timing analysis cancelled"
        );
    }

    #[test]
    fn rpc_rtl_post_route_power_ir_returns_multi_scenario_results() {
        let source = "module top(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let scenarios = r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_scenario_property post_route.voltage_v 0.90
    set_scenario_property post_route.vdd_ohm_per_site 0.05
    set_scenario_property post_route.gnd_ohm_per_site 0.06
    set_scenario_property post_route.max_total_drop_v 0.04
endscenario
scenario fast
    create_clock -name clk -period 2.0 [get_ports a]
    set_scenario_property post_route.voltage_v 1.10
    set_scenario_property post_route.vdd_ohm_per_site 0.04
    set_scenario_property post_route.gnd_ohm_per_site 0.05
    set_scenario_property post_route.max_total_drop_v 0.05
endscenario
"#;
        let request = RpcRequest::new(74, "rtlPostRoutePowerIr").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            ("scenarios", Json::string(scenarios)),
            (
                "physicalTimingOptions",
                Json::object([
                    ("tracksPerChannel", Json::Number(64.0)),
                    ("iterations", Json::Number(1.0)),
                    ("placementPasses", Json::Number(2.0)),
                    ("routingPasses", Json::Number(2.0)),
                ]),
            ),
            (
                "vectors",
                Json::Array(vec![
                    Json::object([
                        ("timeNs", Json::Number(0.0)),
                        (
                            "inputs",
                            Json::object([
                                ("a", Json::string("0")),
                                ("b", Json::string("0")),
                                ("c", Json::string("0")),
                            ]),
                        ),
                    ]),
                    Json::object([
                        ("timeNs", Json::Number(1.0)),
                        (
                            "inputs",
                            Json::object([
                                ("a", Json::string("1")),
                                ("b", Json::string("1")),
                                ("c", Json::string("1")),
                            ]),
                        ),
                    ]),
                ]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["scenarioCount"], Json::from(2_u64));
        assert_eq!(fields["vectorCount"], Json::from(2_u64));
        assert!(fields["worstPowerScenario"].as_str().is_some());
        assert!(fields["worstDropScenario"].as_str().is_some());
        let Json::Array(corners) = &fields["corners"] else {
            panic!("corners should be an array");
        };
        assert_eq!(corners.len(), 2);
        for corner in corners {
            let corner = corner.as_object().unwrap();
            assert!(corner["voltageV"].as_number().unwrap() > &0.0);
            assert!(corner["totalPowerUw"].as_number().unwrap() > &0.0);
            assert!(corner["dynamicPowerUw"].as_number().unwrap() > &0.0);
            assert!(corner["staticPowerUw"].as_number().unwrap() > &0.0);
            assert!(corner["parasiticDynamicPowerUw"].as_number().unwrap() >= &0.0);
            assert!(corner["effectiveCapacitancePf"].as_number().unwrap() > &0.0);
            assert!(corner["effectiveCapacitanceNets"].as_number().unwrap() > &0.0);
            assert!(corner["totalCurrentA"].as_number().unwrap() > &0.0);
            assert!(corner["worstDropV"].as_number().unwrap() >= &0.0);
            assert!(corner["report"]
                .as_str()
                .unwrap()
                .contains("Post-route power/IR"));
        }
        let slow = corners[0].as_object().unwrap();
        let fast = corners[1].as_object().unwrap();
        assert_eq!(slow["scenario"], Json::string("slow"));
        assert_eq!(fast["scenario"], Json::string("fast"));
        assert_eq!(slow["voltageV"], Json::Number(0.9));
        assert_eq!(fast["voltageV"], Json::Number(1.1));
        assert!(fields["physicalTimingReport"]
            .as_str()
            .unwrap()
            .contains("Physical timing flow"));
        assert!(fields["mappedStats"].as_object().is_some());

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            rtl_post_route_power_ir(&request.params, &cancellation).unwrap_err(),
            "RTL post-route power/IR analysis cancelled"
        );
    }

    #[test]
    fn rpc_rtl_post_route_power_mode_ir_returns_mode_matrix() {
        let source = "module top(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let scenarios = r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_scenario_property post_route.voltage_v 0.90
    set_scenario_property post_route.vdd_ohm_per_site 0.05
    set_scenario_property post_route.gnd_ohm_per_site 0.05
endscenario
scenario fast
    create_clock -name clk -period 2.0 [get_ports a]
    set_scenario_property post_route.voltage_v 1.10
    set_scenario_property post_route.vdd_ohm_per_site 0.04
    set_scenario_property post_route.gnd_ohm_per_site 0.04
endscenario
"#;
        let upf = r#"
upf_version 2.1
set_design_top top
create_power_domain TOP -elements {top}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_on_resistance sw 0.025
create_power_mode active
set_power_mode_switch active sw on
set_power_mode_domain_activity active TOP 1.0
create_power_mode idle
set_power_mode_switch idle sw on
set_power_mode_domain_activity idle TOP 0.25
set_active_power_mode active
"#;
        let request = RpcRequest::new(75, "rtlPostRoutePowerModeIr").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            ("scenarios", Json::string(scenarios)),
            ("upf", Json::string(upf)),
            (
                "physicalTimingOptions",
                Json::object([
                    ("tracksPerChannel", Json::Number(64.0)),
                    ("iterations", Json::Number(1.0)),
                    ("placementPasses", Json::Number(2.0)),
                    ("routingPasses", Json::Number(2.0)),
                    ("wireResistanceOhmPerSite", Json::Number(0.05)),
                    ("wireCapacitancePfPerSite", Json::Number(0.002)),
                ]),
            ),
            (
                "powerModeOptions",
                Json::object([
                    ("multilayerPowerMesh", Json::Bool(true)),
                    ("rows", Json::Number(2.0)),
                    ("columns", Json::Number(2.0)),
                ]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["scenarioCount"], Json::from(2_u64));
        assert!(fields["worstPowerScenario"].as_str().is_some());
        assert!(fields["worstPowerMode"].as_str().is_some());
        assert!(fields["worstDropScenario"].as_str().is_some());
        assert!(fields["worstDropMode"].as_str().is_some());
        assert!(fields["worstEmLifetimeScenario"].as_str().is_some());
        assert!(fields["worstEmLifetimeMode"].as_str().is_some());
        assert_eq!(fields["worstEmLifetimeHours"], Json::Null);

        let Json::Array(corners) = &fields["corners"] else {
            panic!("corners should be an array");
        };
        assert_eq!(corners.len(), 2);
        for corner in corners {
            let corner = corner.as_object().unwrap();
            assert_eq!(corner["modeCount"], Json::from(2_u64));
            assert!(corner["worstPowerMode"].as_str().is_some());
            assert!(corner["worstDropMode"].as_str().is_some());
            let Json::Array(modes) = &corner["modes"] else {
                panic!("modes should be an array");
            };
            assert_eq!(modes.len(), 2);
            let active = modes[0].as_object().unwrap();
            let idle = modes[1].as_object().unwrap();
            assert_eq!(active["powerMode"], Json::string("active"));
            assert_eq!(idle["powerMode"], Json::string("idle"));
            assert!(active["totalPowerUw"].as_number().unwrap() > &0.0);
            assert!(idle["totalPowerUw"].as_number().unwrap() > &0.0);
            assert!(
                active["totalPowerUw"].as_number().unwrap()
                    > idle["totalPowerUw"].as_number().unwrap()
            );
            assert!(active["worstDropV"].as_number().unwrap() >= &0.0);
            assert!(idle["worstDropV"].as_number().unwrap() >= &0.0);
            assert_eq!(active["worstEmLifetimeHours"], Json::Null);
            assert_eq!(idle["worstEmLifetimeHours"], Json::Null);
            assert_eq!(active["worstEmLifetimeUnbounded"], Json::Bool(true));
            assert_eq!(idle["worstEmLifetimeUnbounded"], Json::Bool(true));
            assert!(corner["report"]
                .as_str()
                .unwrap()
                .contains("UPF power-mode IR report"));
        }
        assert!(fields["physicalTimingReport"]
            .as_str()
            .unwrap()
            .contains("Physical timing flow"));
        assert!(fields["mappedStats"].as_object().is_some());

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            rtl_post_route_power_mode_ir(&request.params, &cancellation).unwrap_err(),
            "RTL post-route power-mode IR analysis cancelled"
        );
    }

    #[test]
    fn rpc_rtl_post_route_power_ir_accepts_vcd_activity() {
        let source = "module top(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let scenarios = r#"
scenario nominal
    create_clock -name clk -period 4.0 [get_ports a]
    set_scenario_property post_route.voltage_v 1.00
    set_scenario_property post_route.vdd_ohm_per_site 0.05
    set_scenario_property post_route.gnd_ohm_per_site 0.05
endscenario
"#;
        let vcd = r#"
$timescale 1ns $end
$scope module top $end
$var wire 1 ! a $end
$var wire 1 " b $end
$var wire 1 # c $end
$var wire 1 $ y $end
$upscope $end
$enddefinitions $end
#0
0!
0"
0#
0$
#10
1!
1"
0#
1$
#20
0!
1"
1#
1$
"#;
        let request = RpcRequest::new(80, "rtlPostRoutePowerIr").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            ("scenarios", Json::string(scenarios)),
            ("vcd", Json::string(vcd)),
            (
                "physicalTimingOptions",
                Json::object([
                    ("tracksPerChannel", Json::Number(64.0)),
                    ("iterations", Json::Number(1.0)),
                    ("placementPasses", Json::Number(2.0)),
                    ("routingPasses", Json::Number(2.0)),
                ]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["scenarioCount"], Json::from(1_u64));
        assert_eq!(fields["vectorCount"], Json::from(3_u64));
        assert_eq!(fields["stimulus"], Json::string("vcd"));
        assert_eq!(fields["worstPowerScenario"], Json::string("nominal"));
        let Json::Array(corners) = &fields["corners"] else {
            panic!("corners should be an array");
        };
        let corner = corners[0].as_object().unwrap();
        assert!(corner["dynamicPowerUw"]
            .as_number()
            .is_some_and(|v| *v > 0.0));
        assert!(corner["totalPowerUw"].as_number().is_some_and(|v| *v > 0.0));

        let ambiguous = Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            ("scenarios", Json::string(scenarios)),
            ("vcd", Json::string(vcd)),
            ("vectors", Json::Array(Vec::new())),
        ]);
        assert_eq!(
            rtl_post_route_power_ir(&ambiguous, &FlowCancellation::new()).unwrap_err(),
            "RPC parameters 'vcd' and 'vectors' are mutually exclusive"
        );

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            rtl_post_route_power_ir(&request.params, &cancellation).unwrap_err(),
            "RTL post-route power/IR analysis cancelled"
        );
    }

    #[test]
    fn rpc_rtl_post_route_power_mode_ir_accepts_vcd_activity() {
        let source = "module top(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let scenarios = r#"
scenario nominal
    create_clock -name clk -period 4.0 [get_ports a]
    set_scenario_property post_route.voltage_v 1.00
    set_scenario_property post_route.vdd_ohm_per_site 0.05
    set_scenario_property post_route.gnd_ohm_per_site 0.05
endscenario
"#;
        let upf = r#"
upf_version 2.1
set_design_top top
create_power_domain TOP -elements {top}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_on_resistance sw 0.025
create_power_mode active
set_power_mode_switch active sw on
set_power_mode_domain_activity active TOP 1.0
create_power_mode idle
set_power_mode_switch idle sw on
set_power_mode_domain_activity idle TOP 0.25
set_active_power_mode active
"#;
        let vcd = r#"
$timescale 1ns $end
$scope module top $end
$var wire 1 ! a $end
$var wire 1 " b $end
$var wire 1 # c $end
$var wire 1 $ y $end
$upscope $end
$enddefinitions $end
#0
0!
0"
0#
0$
#10
1!
1"
0#
1$
#20
0!
1"
1#
1$
"#;
        let request = RpcRequest::new(81, "rtlPostRoutePowerModeIr").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            ("scenarios", Json::string(scenarios)),
            ("upf", Json::string(upf)),
            ("vcd", Json::string(vcd)),
            (
                "physicalTimingOptions",
                Json::object([
                    ("tracksPerChannel", Json::Number(64.0)),
                    ("iterations", Json::Number(1.0)),
                    ("placementPasses", Json::Number(2.0)),
                    ("routingPasses", Json::Number(2.0)),
                ]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["scenarioCount"], Json::from(1_u64));
        assert_eq!(fields["vectorCount"], Json::from(3_u64));
        assert_eq!(fields["stimulus"], Json::string("vcd"));
        let Json::Array(corners) = &fields["corners"] else {
            panic!("corners should be an array");
        };
        let corner = corners[0].as_object().unwrap();
        assert_eq!(corner["modeCount"], Json::from(2_u64));
        let Json::Array(modes) = &corner["modes"] else {
            panic!("modes should be an array");
        };
        let active = modes[0].as_object().unwrap();
        let idle = modes[1].as_object().unwrap();
        assert!(active["totalPowerUw"].as_number().is_some_and(|v| *v > 0.0));
        assert!(idle["totalPowerUw"].as_number().is_some_and(|v| *v > 0.0));
        assert!(
            active["totalPowerUw"].as_number().unwrap() > idle["totalPowerUw"].as_number().unwrap()
        );

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            rtl_post_route_power_mode_ir(&request.params, &cancellation).unwrap_err(),
            "RTL post-route power-mode IR analysis cancelled"
        );
    }

    #[test]
    fn rpc_rtl_sta_returns_structured_timing() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let request = RpcRequest::new(8, "rtlSta").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert!(fields["criticalPathDelayNs"]
            .as_number()
            .is_some_and(|value| value.is_finite() && *value >= 0.0));
        assert_eq!(fields["caseAnalysisNets"], Json::Object(BTreeMap::new()));
        assert!(fields["mappedStats"].as_object().is_some_and(|stats| {
            stats
                .get("libCells")
                .and_then(Json::as_number)
                .is_some_and(|value| *value > 0.0)
        }));

        let strict = RpcRequest::new(120, "rtlPnr").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            (
                "drcOptions",
                Json::object([("maxAntennaRatio", Json::Number(0.001))]),
            ),
        ]));
        let strict_response = dispatch(&strict).result.unwrap();
        let strict_fields = strict_response.as_object().unwrap();
        let strict_drc = strict_fields["drc"].as_object().unwrap();
        assert_eq!(strict_drc["errors"], Json::from(0_u64));
        assert!(strict_drc["warnings"]
            .as_number()
            .is_some_and(|value| *value > 0.0));
        let Json::Array(strict_violations) = &strict_drc["violations"] else {
            panic!("strict PnR DRC violations should be an array");
        };
        assert!(strict_violations.iter().any(|violation| {
            violation.as_object().unwrap()["code"] == Json::string("ANT-001")
        }));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = rtl_sta(&request.params, &cancellation).unwrap_err();
        assert_eq!(error, "RTL timing analysis cancelled");
    }

    #[test]
    fn rpc_rtl_mmmc_timing_returns_structured_scenarios() {
        let source = "module top(input a, output y); assign y = ~a; endmodule";
        let scenarios = r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_scenario_property voltage_v 0.90
endscenario
scenario fast
    create_clock -name clk -period 2.0 [get_ports a]
    set_scenario_property voltage_v 1.10
endscenario
"#;
        let request = RpcRequest::new(68, "rtlMmmcTiming").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            ("scenarios", Json::string(scenarios)),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["scenarioCount"], Json::from(2_u64));
        assert_eq!(fields["worstScenario"], Json::string("fast"));
        assert!(fields["worstSetupSlackNs"].as_number().is_some());
        assert_eq!(fields["worstHoldSlackNs"], Json::Null);
        let Json::Array(corners) = &fields["corners"] else {
            panic!("corners should be an array");
        };
        assert_eq!(corners.len(), 2);
        for corner in corners {
            let corner = corner.as_object().unwrap();
            assert_eq!(corner["clock"], Json::string("clk"));
            assert!(corner["periodNs"].as_number().unwrap() > &0.0);
            assert!(corner["criticalPathDelayNs"].as_number().unwrap() > &0.0);
            assert!(corner["worstSlackNs"].as_number().is_some());
        }
        let properties = fields["properties"].as_object().unwrap();
        let fast = properties["fast"].as_object().unwrap();
        assert_eq!(fast["voltage_v"], Json::string("1.10"));
        assert!(fields["report"].as_str().unwrap().contains("[fast]"));
        assert!(fields["mappedStats"].as_object().is_some());

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            rtl_mmmc_timing(&request.params, &cancellation).unwrap_err(),
            "RTL MMMC timing analysis cancelled"
        );
    }

    #[test]
    fn rpc_rtl_mmmc_timing_paths_returns_structured_paths() {
        let source =
            "module top(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule";
        let scenarios = r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports clk]
endscenario
scenario fast
    create_clock -name clk -period 2.0 [get_ports clk]
endscenario
"#;
        let request = RpcRequest::new(73, "rtlMmmcTimingPaths").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            ("scenarios", Json::string(scenarios)),
            ("maxPaths", Json::Number(5.0)),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["scenarioCount"], Json::from(2_u64));
        assert_eq!(fields["maxPaths"], Json::from(5_u64));
        assert_eq!(fields["worstScenario"], Json::string("slow"));

        let Json::Array(setup_paths) = &fields["setupPaths"] else {
            panic!("setup paths should be an array");
        };
        assert!(!setup_paths.is_empty());
        let setup = setup_paths[0].as_object().unwrap();
        assert_eq!(setup["scenario"], Json::string("fast"));
        let endpoint = setup["endpoint"].as_object().unwrap();
        assert_eq!(endpoint["kind"], Json::string("outputPort"));
        assert_eq!(endpoint["name"], Json::string("q"));
        let path = setup["path"].as_object().unwrap();
        assert_eq!(path["kind"], Json::string("sequentialToOutput"));
        let launch = path["launch"].as_object().unwrap();
        assert_eq!(launch["kind"], Json::string("sequentialCell"));
        assert_eq!(launch["name"], Json::string("u1_dff"));
        assert!(matches!(&path["cellNames"], Json::Array(values) if !values.is_empty()));
        assert!(matches!(&path["netNames"], Json::Array(values) if !values.is_empty()));
        assert!(matches!(&path["arcs"], Json::Array(values) if !values.is_empty()));
        let Json::Array(corner_slacks) = &setup["cornerSlacks"] else {
            panic!("corner slacks should be an array");
        };
        assert_eq!(corner_slacks.len(), 2);
        assert!(corner_slacks
            .iter()
            .all(|slack| slack.as_object().unwrap()["slackNs"].as_number().is_some()));

        let Json::Array(hold_paths) = &fields["holdPaths"] else {
            panic!("hold paths should be an array");
        };
        assert!(!hold_paths.is_empty());
        assert!(hold_paths[0].as_object().unwrap().contains_key("holdNs"));
        assert_eq!(fields["resetPaths"], Json::Array(Vec::new()));
        assert!(fields["setupReport"]
            .as_str()
            .unwrap()
            .contains("Multi-corner setup timing path report"));
        assert!(fields["holdReport"]
            .as_str()
            .unwrap()
            .contains("Multi-corner hold timing path report"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            rtl_mmmc_timing_paths(&request.params, &cancellation).unwrap_err(),
            "RTL MMMC timing paths cancelled"
        );
    }

    #[test]
    fn rpc_rtl_mmmc_timing_violations_applies_waivers() {
        let source = "module top(input a, output y); assign y = ~a; endmodule";
        let scenarios = r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
endscenario
scenario fast
    create_clock -name clk -period 0.2 [get_ports a]
endscenario
"#;
        let request = RpcRequest::new(76, "rtlMmmcTimingViolations").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            ("scenarios", Json::string(scenarios)),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["scenarioCount"], Json::from(2_u64));
        assert_eq!(fields["activeViolationCount"], Json::from(1_u64));
        assert_eq!(fields["hasActiveViolations"], Json::Bool(true));
        let scenario_reports = fields["scenarios"].as_object().unwrap();
        let slow = scenario_reports["slow"].as_object().unwrap();
        let fast = scenario_reports["fast"].as_object().unwrap();
        assert_eq!(slow["activeViolationCount"], Json::from(0_u64));
        assert_eq!(fast["activeViolationCount"], Json::from(1_u64));
        let Json::Array(active) = &fast["activeViolations"] else {
            panic!("fast active violations should be an array");
        };
        let violation = active[0].as_object().unwrap();
        assert_eq!(violation["scenario"], Json::string("fast"));
        assert_eq!(violation["kind"], Json::string("output"));
        assert_eq!(violation["endpoint"], Json::string("y"));
        assert!(violation["slackNs"].as_number().unwrap() < &0.0);
        let Json::Array(summaries) = &fields["summaries"] else {
            panic!("summaries should be an array");
        };
        assert_eq!(summaries.len(), 1);
        let summary = summaries[0].as_object().unwrap();
        assert_eq!(summary["key"], Json::string("output:y"));
        assert_eq!(summary["worstScenario"], Json::string("fast"));
        assert!(fields["hierarchySummaries"]
            .as_object()
            .unwrap()
            .contains_key("top"));
        assert!(fields["report"]
            .as_str()
            .unwrap()
            .contains("Multi-corner timing violation report"));

        let waived_request =
            RpcRequest::new(77, "rtlMmmcTimingViolations").with_params(Json::object([
                ("source", Json::string(source)),
                ("top", Json::string("top")),
                ("scenarios", Json::string(scenarios)),
                (
                    "waivers",
                    Json::string("waiver \"fast-only\" \"fast\" \"y\" \"-1.0\"\n"),
                ),
                ("actor", Json::string("timing-owner")),
            ]));
        let response = dispatch(&waived_request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["activeViolationCount"], Json::from(0_u64));
        assert_eq!(fields["hasActiveViolations"], Json::Bool(false));
        let scenario_reports = fields["scenarios"].as_object().unwrap();
        let fast = scenario_reports["fast"].as_object().unwrap();
        assert_eq!(fast["activeViolationCount"], Json::from(0_u64));
        assert_eq!(fast["waivedViolationCount"], Json::from(1_u64));
        let waived = fast["waivedViolations"].as_object().unwrap();
        let waived_entry = waived["fast:output:y"].as_object().unwrap();
        assert_eq!(waived_entry["waiver"], Json::string("fast-only"));
        assert!(fast["waiverAuditReport"]
            .as_str()
            .unwrap()
            .contains("by timing-owner"));
        assert_eq!(fields["summaries"], Json::Array(Vec::new()));
        assert_eq!(fields["hierarchySummaries"], Json::Object(BTreeMap::new()));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            rtl_mmmc_timing_violations(&request.params, &cancellation).unwrap_err(),
            "RTL MMMC timing violations cancelled"
        );
    }

    #[test]
    fn rpc_rtl_incremental_mmmc_timing_violations_reports_lifecycle() {
        let source = "module top(input a, output y); assign y = ~a; endmodule";
        let old = r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
endscenario
scenario fast
    create_clock -name clk -period 0.2 [get_ports a]
endscenario
"#;
        let new = r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
endscenario
scenario fast
    create_clock -name clk -period 0.1 [get_ports a]
endscenario
scenario typical
    create_clock -name clk -period 3.0 [get_ports a]
endscenario
"#;
        let request =
            RpcRequest::new(78, "rtlIncrementalMmmcTimingViolations").with_params(Json::object([
                ("source", Json::string(source)),
                ("top", Json::string("top")),
                ("old", Json::string(old)),
                ("new", Json::string(new)),
            ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["oldScenarioCount"], Json::from(2_u64));
        assert_eq!(fields["newScenarioCount"], Json::from(3_u64));
        assert_eq!(fields["activeViolationCount"], Json::from(1_u64));
        let update = fields["update"].as_object().unwrap();
        assert_eq!(update["addedScenarioCount"], Json::from(1_u64));
        assert_eq!(update["removedScenarioCount"], Json::from(0_u64));
        assert_eq!(update["scenarioDiffCount"], Json::from(2_u64));
        let scenario_diffs = update["scenarioDiffs"].as_object().unwrap();
        let fast = scenario_diffs["fast"].as_object().unwrap();
        assert_eq!(fast["degradedCount"], Json::from(1_u64));
        let Json::Array(degraded) = &fast["degraded"] else {
            panic!("fast degradation should be an array");
        };
        let degradation = degraded[0].as_object().unwrap();
        assert_eq!(degradation["id"], Json::string("fast:output:y"));
        let old_slack = degradation["oldSlackNs"].as_number().copied().unwrap();
        let new_slack = degradation["newSlackNs"].as_number().copied().unwrap();
        assert!((old_slack + 0.2).abs() < 1e-12);
        assert!((new_slack + 0.3).abs() < 1e-12);
        let cross_corner = update["crossCorner"].as_object().unwrap();
        assert_eq!(cross_corner["degradedCount"], Json::from(1_u64));
        let Json::Array(cross_degraded) = &cross_corner["degraded"] else {
            panic!("cross-corner degradation should be an array");
        };
        let cross_change = cross_degraded[0].as_object().unwrap();
        assert_eq!(cross_change["key"], Json::string("output:y"));
        let scenarios = fields["scenarios"].as_object().unwrap();
        assert!(scenarios.contains_key("typical"));
        let final_fast = scenarios["fast"].as_object().unwrap();
        assert_eq!(final_fast["activeViolationCount"], Json::from(1_u64));
        assert!(fields["baselineReport"]
            .as_str()
            .unwrap()
            .contains("generation 1"));
        assert!(fields["updateReport"]
            .as_str()
            .unwrap()
            .contains("cross-corner: added=0, removed=0, improved=0, degraded=1"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            rtl_incremental_mmmc_timing_violations(&request.params, &cancellation).unwrap_err(),
            "RTL incremental MMMC timing violations cancelled"
        );
    }

    #[test]
    fn rpc_rtl_incremental_mmmc_timing_violations_preserves_waivers() {
        let source = "module top(input a, output y); assign y = ~a; endmodule";
        let old = r#"
scenario fast
    create_clock -name clk -period 0.2 [get_ports a]
endscenario
"#;
        let new = r#"
scenario fast
    create_clock -name clk -period 0.1 [get_ports a]
endscenario
"#;
        let request =
            RpcRequest::new(79, "rtlIncrementalMmmcTimingViolations").with_params(Json::object([
                ("source", Json::string(source)),
                ("top", Json::string("top")),
                ("old", Json::string(old)),
                ("new", Json::string(new)),
                (
                    "waivers",
                    Json::string("waiver \"fast-only\" \"fast\" \"y\" \"-1.0\"\n"),
                ),
                ("actor", Json::string("timing-owner")),
            ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["activeViolationCount"], Json::from(0_u64));
        assert_eq!(fields["hasActiveViolations"], Json::Bool(false));
        let final_scenarios = fields["scenarios"].as_object().unwrap();
        let fast = final_scenarios["fast"].as_object().unwrap();
        assert_eq!(fast["activeViolationCount"], Json::from(0_u64));
        assert_eq!(fast["waivedViolationCount"], Json::from(1_u64));
        assert!(fast["waiverAuditReport"]
            .as_str()
            .unwrap()
            .contains("by timing-owner"));
        let update = fields["update"].as_object().unwrap();
        let scenario_diffs = update["scenarioDiffs"].as_object().unwrap();
        let fast_diff = scenario_diffs["fast"].as_object().unwrap();
        assert_eq!(fast_diff["degradedCount"], Json::from(1_u64));
        let cross_corner = update["crossCorner"].as_object().unwrap();
        assert_eq!(cross_corner["degradedCount"], Json::from(0_u64));
        assert!(fields["updateReport"]
            .as_str()
            .unwrap()
            .contains("waived violations: 1"));
    }

    #[test]
    fn rpc_timing_scenario_diff_returns_structured_changes() {
        let old = r#"
scenario removed
    create_clock -name clk -period 5.0 [get_ports a]
endscenario
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_input_delay 0.2 -clock clk [get_ports a]
    set_scenario_property voltage_v 0.90
endscenario
scenario fast
    create_clock -name clk -period 2.0 [get_ports a]
    set_scenario_property voltage_v 1.00
endscenario
"#;
        let new = r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_input_delay 0.8 -clock clk [get_ports a]
    set_scenario_property voltage_v 0.90
endscenario
scenario fast
    create_clock -name clk -period 2.0 [get_ports a]
    set_scenario_property voltage_v 1.10
endscenario
scenario ss
    create_clock -name clk -period 1.0 [get_ports a]
endscenario
"#;
        let request = RpcRequest::new(69, "timingScenarioDiff").with_params(Json::object([
            ("old", Json::string(old)),
            ("new", Json::string(new)),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["oldScenarioCount"], Json::from(3_u64));
        assert_eq!(fields["newScenarioCount"], Json::from(3_u64));
        assert_eq!(fields["added"], Json::Array(vec![Json::string("ss")]));
        assert_eq!(
            fields["removed"],
            Json::Array(vec![Json::string("removed")])
        );
        assert_eq!(
            fields["changed"],
            Json::Array(vec![Json::string("fast"), Json::string("slow"),])
        );
        assert_eq!(
            fields["timingChanged"],
            Json::Array(vec![Json::string("slow")])
        );

        let constraints = fields["constraints"].as_object().unwrap();
        let slow = constraints["slow"].as_object().unwrap();
        assert_eq!(
            slow["changedInputPorts"],
            Json::Array(vec![Json::string("a")])
        );
        assert_eq!(slow["baseRecomputeRequired"], Json::Bool(true));
        assert_eq!(slow["constrainedRecomputeRequired"], Json::Bool(true));
        assert!(!constraints.contains_key("fast"));

        let properties = fields["properties"].as_object().unwrap();
        let fast = properties["fast"].as_object().unwrap();
        let changed = fast["changed"].as_object().unwrap();
        let voltage = changed["voltage_v"].as_object().unwrap();
        assert_eq!(voltage["old"], Json::string("1.00"));
        assert_eq!(voltage["new"], Json::string("1.10"));

        let report = fields["report"].as_str().unwrap();
        assert!(report.contains("added [ss]"), "{report}");
        assert!(report.contains("removed [removed]"), "{report}");
        assert!(report.contains("timing changed [slow]"), "{report}");

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            timing_scenario_diff(&request.params, &cancellation).unwrap_err(),
            "timing scenario diff cancelled"
        );
    }

    #[test]
    fn rpc_rtl_signal_quality_reports_structured_violations() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let request = RpcRequest::new(71, "rtlSignalQuality").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            (
                "qualityOptions",
                Json::object([
                    ("inputTransitionNs", Json::Number(0.05)),
                    ("maxTransitionNs", Json::Number(10.0)),
                    ("maxCapacitancePf", Json::Number(0.1)),
                    ("maxFanout", Json::Number(16.0)),
                    (
                        "scopedMaxTransitionNs",
                        Json::Array(vec![Json::object([
                            ("kind", Json::string("port")),
                            ("name", Json::string("y")),
                            ("limit", Json::Number(10.0)),
                        ])]),
                    ),
                    (
                        "inputTransitions",
                        Json::Array(vec![Json::object([
                            ("kind", Json::string("port")),
                            ("name", Json::string("a")),
                            ("limit", Json::Number(0.05)),
                        ])]),
                    ),
                    (
                        "extraCapacitancePf",
                        Json::object([("y", Json::Number(0.2))]),
                    ),
                ]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["hasViolations"], Json::Bool(true));
        assert!(fields["violationCount"].as_number().unwrap() > &0.0);
        assert!(fields["transitionsChecked"].as_number().unwrap() > &0.0);
        assert!(fields["capacitancesChecked"].as_number().unwrap() > &0.0);
        assert!(fields["fanoutsChecked"].as_number().unwrap() > &0.0);
        let Json::Array(violations) = &fields["violations"] else {
            panic!("violations should be an array");
        };
        let capacitance = violations
            .iter()
            .map(|violation| violation.as_object().unwrap())
            .find(|violation| violation["kind"] == Json::string("maxCapacitance"))
            .unwrap();
        assert_eq!(capacitance["netName"], Json::string("y"));
        assert!(capacitance["actual"].as_number().unwrap() > &0.1);
        assert_eq!(capacitance["limit"], Json::Number(0.1));
        assert!(capacitance["message"].as_str().unwrap().contains("y"));
        assert!(fields["report"]
            .as_str()
            .unwrap()
            .contains("Signal-quality report"));
        assert!(fields["mappedStats"].as_object().is_some());

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            rtl_signal_quality(&request.params, &cancellation).unwrap_err(),
            "RTL signal-quality analysis cancelled"
        );
    }

    #[test]
    fn rpc_rtl_incremental_mmmc_timing_reuses_unchanged_scenarios() {
        let source = "module top(input a, output y); assign y = ~a; endmodule";
        let old = r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_input_delay 0.2 -clock clk [get_ports a]
endscenario
scenario fast
    create_clock -name clk -period 2.0 [get_ports a]
    set_scenario_property voltage_v 1.00
endscenario
"#;
        let new = r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_input_delay 0.8 -clock clk [get_ports a]
endscenario
scenario fast
    create_clock -name clk -period 2.0 [get_ports a]
    set_scenario_property voltage_v 1.10
endscenario
scenario ss
    create_clock -name clk -period 1.0 [get_ports a]
endscenario
"#;
        let request = RpcRequest::new(70, "rtlIncrementalMmmcTiming").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            ("old", Json::string(old)),
            ("new", Json::string(new)),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["oldScenarioCount"], Json::from(2_u64));
        assert_eq!(fields["newScenarioCount"], Json::from(3_u64));
        assert_eq!(fields["baselineWorstScenario"], Json::string("fast"));
        assert_eq!(fields["finalWorstScenario"], Json::string("ss"));
        assert_eq!(
            fields["timingChanged"],
            Json::Array(vec![Json::string("slow")])
        );
        assert_eq!(fields["reused"], Json::Array(vec![Json::string("fast")]));

        let updates = fields["updates"].as_object().unwrap();
        assert_eq!(updates.len(), 1);
        let slow = updates["slow"].as_object().unwrap();
        assert_eq!(
            slow["constraintDiff"].as_object().unwrap()["changedInputPorts"],
            Json::Array(vec![Json::string("a")])
        );
        assert!(slow["totalCells"].as_number().unwrap() > &0.0);
        assert!(slow["criticalPathDelayNs"].as_number().is_some());

        let Json::Array(corners) = &fields["corners"] else {
            panic!("corners should be an array");
        };
        assert_eq!(corners.len(), 3);
        assert!(fields["diffReport"]
            .as_str()
            .unwrap()
            .contains("timing changed [slow]"));
        assert!(fields["finalReport"].as_str().unwrap().contains("[ss]"));
        assert!(fields["mappedStats"].as_object().is_some());

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            rtl_incremental_mmmc_timing(&request.params, &cancellation).unwrap_err(),
            "RTL incremental MMMC timing cancelled"
        );
    }

    #[test]
    fn rpc_timing_waiver_audit_returns_events_and_active_rules() {
        let audit = r#"
audit 1 added 1 "frontend"
waiver "u1" "fast" "y" "-1.0"
instance "top.u1.**"
end
audit 2 removed 2 "reviewer"
waiver "u1" "fast" "y" "-1.0"
instance "top.u1.**"
end
audit 3 added 2 "frontend"
waiver "u2" "fast" "y" "-1.0"
instance "top.u2"
end
"#;
        let request = RpcRequest::new(72, "timingWaiverAudit")
            .with_params(Json::object([("audit", Json::string(audit))]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["eventCount"], Json::from(3_u64));
        assert_eq!(fields["activeWaiverCount"], Json::from(1_u64));

        let Json::Array(events) = &fields["events"] else {
            panic!("events should be an array");
        };
        assert_eq!(events.len(), 3);
        let first = events[0].as_object().unwrap();
        assert_eq!(first["sequence"], Json::from(1_u64));
        assert_eq!(first["action"], Json::string("added"));
        assert_eq!(first["waiverId"], Json::string("u1"));
        assert_eq!(first["changedBy"], Json::string("frontend"));
        let first_waiver = first["waiver"].as_object().unwrap();
        assert_eq!(first_waiver["scenarioPattern"], Json::string("fast"));
        assert_eq!(first_waiver["endpointPattern"], Json::string("y"));
        assert_eq!(first_waiver["instancePattern"], Json::string("top.u1.**"));
        assert_eq!(first_waiver["slackFloorNs"], Json::Number(-1.0));

        let Json::Array(active) = &fields["activeWaivers"] else {
            panic!("active waivers should be an array");
        };
        assert_eq!(active.len(), 1);
        let active_waiver = active[0].as_object().unwrap();
        assert_eq!(active_waiver["id"], Json::string("u2"));
        assert_eq!(active_waiver["instancePattern"], Json::string("top.u2"));

        let hierarchy = fields["hierarchy"].as_object().unwrap();
        let first_scope = hierarchy["top.u1.**"].as_object().unwrap();
        assert_eq!(first_scope["events"], Json::from(2_u64));
        assert_eq!(first_scope["addedEvents"], Json::from(1_u64));
        assert_eq!(first_scope["removedEvents"], Json::from(1_u64));
        assert_eq!(first_scope["activeWaiverIds"], Json::Array(Vec::new()));
        let second_scope = hierarchy["top.u2"].as_object().unwrap();
        assert_eq!(
            second_scope["activeWaiverIds"],
            Json::Array(vec![Json::string("u2")])
        );

        assert!(fields["report"].as_str().unwrap().contains("3 event(s)"));
        assert!(fields["hierarchyReport"]
            .as_str()
            .unwrap()
            .contains("'top.u1.**': events=2, added=1, removed=1, active=0"));
        assert!(fields["hierarchyReport"]
            .as_str()
            .unwrap()
            .contains("'top.u2': events=1, added=1, removed=0, active=1"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            timing_waiver_audit(&request.params, &cancellation).unwrap_err(),
            "timing waiver audit cancelled"
        );
    }

    #[test]
    fn rpc_rtl_pnr_returns_physical_views() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let request = RpcRequest::new(9, "rtlPnr").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        let placement = fields["placement"].as_object().unwrap();
        assert!(placement["placedCells"]
            .as_number()
            .is_some_and(|value| *value > 0.0));
        let drc = fields["drc"].as_object().unwrap();
        assert_eq!(drc["errors"], Json::Number(0.0));
        assert!(fields["lef"].as_str().unwrap().contains("VERSION"));
        assert!(fields["def"].as_str().unwrap().contains("DESIGN"));
        assert!(fields["mappedStats"].as_object().is_some_and(|stats| {
            stats
                .get("libCells")
                .and_then(Json::as_number)
                .is_some_and(|value| *value > 0.0)
        }));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = rtl_pnr(&request.params, &cancellation).unwrap_err();
        assert_eq!(error, "RTL PnR cancelled");
    }

    #[test]
    fn rpc_rtl_antenna_repair_returns_structured_result() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let request = RpcRequest::new(118, "rtlAntennaRepair").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            (
                "antennaOptions",
                Json::object([
                    ("patchName", Json::string("remote_antenna_fix")),
                    ("maxDiodesPerNet", Json::from(2_u64)),
                    (
                        "routeOptions",
                        Json::object([
                            ("tracksPerChannel", Json::from(8_u64)),
                            ("allowCongestion", Json::Bool(false)),
                        ]),
                    ),
                ]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        let baseline_drc = fields["baselineDrc"].as_object().unwrap();
        assert_eq!(baseline_drc["errors"], Json::from(0_u64));
        assert_eq!(fields["repairCount"], Json::from(0_u64));
        assert_eq!(fields["diodesInserted"], Json::from(0_u64));
        assert_eq!(fields["antennaWarnings"], Json::from(0_u64));
        assert_eq!(fields["allAntennasRepaired"], Json::Bool(true));
        let placement = fields["placement"].as_object().unwrap();
        assert!(placement["placedCells"]
            .as_number()
            .is_some_and(|value| *value > 0.0));
        assert!(fields["report"]
            .as_str()
            .unwrap()
            .contains("Antenna repair: 0 diode(s), 0 net(s), status PASS"));

        let invalid = RpcRequest::new(119, "rtlAntennaRepair").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            (
                "antennaOptions",
                Json::object([("maxDiodesPerNet", Json::from(0_u64))]),
            ),
        ]));
        assert!(dispatch(&invalid)
            .result
            .unwrap_err()
            .contains("'maxDiodesPerNet' must be a positive usize"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            rtl_antenna_repair(&request.params, &cancellation).unwrap_err(),
            "RTL antenna repair cancelled"
        );
    }

    #[test]
    fn rpc_rtl_power_returns_structured_activity() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let request = RpcRequest::new(10, "rtlPower").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            (
                "vectors",
                Json::Array(vec![
                    Json::object([
                        ("timeNs", Json::Number(0.0)),
                        (
                            "inputs",
                            Json::object([("a", Json::string("0")), ("b", Json::string("0"))]),
                        ),
                    ]),
                    Json::object([
                        ("timeNs", Json::Number(1.0)),
                        (
                            "inputs",
                            Json::object([("a", Json::string("1")), ("b", Json::string("1"))]),
                        ),
                    ]),
                ]),
            ),
            (
                "powerOptions",
                Json::object([("frequencyMhz", Json::Number(250.0))]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["vectors"], Json::Number(2.0));
        assert_eq!(fields["frequencyMhz"], Json::Number(250.0));
        assert!(fields["totalPowerUw"]
            .as_number()
            .is_some_and(|value| *value > 0.0));
        assert!(!fields["activityByNet"].as_object().unwrap().is_empty());
        assert!(fields["report"].as_str().unwrap().contains("Power report"));
        assert!(fields["mappedStats"].as_object().is_some_and(|stats| {
            stats
                .get("libCells")
                .and_then(Json::as_number)
                .is_some_and(|value| *value > 0.0)
        }));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = rtl_power(&request.params, &cancellation).unwrap_err();
        assert_eq!(error, "RTL power analysis cancelled");
    }

    #[test]
    fn rpc_rtl_ir_returns_structured_power_grid_result() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let request = RpcRequest::new(12, "rtlIr").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            (
                "vectors",
                Json::Array(vec![
                    Json::object([
                        ("timeNs", Json::Number(0.0)),
                        (
                            "inputs",
                            Json::object([("a", Json::string("0")), ("b", Json::string("0"))]),
                        ),
                    ]),
                    Json::object([
                        ("timeNs", Json::Number(1.0)),
                        (
                            "inputs",
                            Json::object([("a", Json::string("1")), ("b", Json::string("1"))]),
                        ),
                    ]),
                ]),
            ),
            (
                "irOptions",
                Json::object([
                    ("voltageV", Json::Number(1.2)),
                    ("vddOhmPerSite", Json::Number(0.002)),
                    ("gndOhmPerSite", Json::Number(0.002)),
                    ("maxTotalDropV", Json::Number(0.05)),
                    ("maxRailCurrentA", Json::Number(0.05)),
                ]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        let power = fields["power"].as_object().unwrap();
        assert_eq!(power["vectors"], Json::Number(2.0));
        assert!(power["totalPowerUw"]
            .as_number()
            .is_some_and(|value| *value > 0.0));
        let ir = fields["ir"].as_object().unwrap();
        assert_eq!(ir["voltageV"], Json::Number(1.2));
        assert!(ir["totalCurrentA"]
            .as_number()
            .is_some_and(|value| *value > 0.0));
        assert!(ir["worstDropV"]
            .as_number()
            .is_some_and(|value| *value > 0.0));
        assert!(ir["worstRow"].as_number().is_some());
        assert!(matches!(&ir["rows"], Json::Array(rows) if !rows.is_empty()));
        assert!(matches!(&ir["cells"], Json::Array(cells) if !cells.is_empty()));
        assert_eq!(ir["hasErrors"], Json::Bool(false));
        let placement = fields["placement"].as_object().unwrap();
        assert!(placement["placedCells"]
            .as_number()
            .is_some_and(|value| *value > 0.0));
        assert!(fields["report"].as_str().unwrap().contains("IR report"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = rtl_ir(&request.params, &cancellation).unwrap_err();
        assert_eq!(error, "RTL IR analysis cancelled");

        let invalid = RpcRequest::new(13, "rtlIr").with_params(Json::object([
            ("source", Json::string(source)),
            (
                "vectors",
                Json::Array(vec![Json::object([
                    ("timeNs", Json::Number(0.0)),
                    (
                        "inputs",
                        Json::object([("a", Json::string("0")), ("b", Json::string("0"))]),
                    ),
                ])]),
            ),
            (
                "irOptions",
                Json::object([("voltageV", Json::Number(-1.0))]),
            ),
        ]));
        let error = dispatch(&invalid).result.unwrap_err();
        assert!(error.contains("voltage"), "{error}");
    }

    #[test]
    fn rpc_rtl_bmc_returns_structured_property_results() {
        let source = r#"
module top(input clk, d, output reg q1, q2);
always @(posedge clk) begin
  q1 <= d;
  q2 <= q1;
end
endmodule
"#;
        let properties = r#"
assert property (@(posedge clk) d |-> ##[2:2] q2);
assert property (@(posedge clk) d |-> ##[1:1] q2);
assume property (@(posedge clk) d || !d);
"#;
        let request = RpcRequest::new(14, "rtlBmc").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            ("properties", Json::string(properties)),
            ("depth", Json::Number(3.0)),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["depth"], Json::Number(3.0));
        assert_eq!(fields["properties"], Json::Number(2.0));
        assert_eq!(fields["violations"], Json::Number(1.0));
        assert!(matches!(&fields["assumptions"], Json::Array(names) if names.len() == 1));
        let results = match &fields["results"] {
            Json::Array(results) => results,
            other => panic!("expected property array, got {other:?}"),
        };
        assert_eq!(
            results[0].as_object().unwrap()["status"],
            Json::string("boundedPass")
        );
        let violated = results[1].as_object().unwrap();
        assert_eq!(violated["status"], Json::string("violated"));
        assert_eq!(violated["violated"], Json::Bool(true));
        assert!(matches!(
            &violated["inputTrace"],
            Json::Array(frames) if !frames.is_empty()
        ));
        assert!(matches!(
            &violated["netTrace"],
            Json::Array(frames) if !frames.is_empty()
        ));
        assert!(fields["report"]
            .as_str()
            .unwrap()
            .contains("Temporal BMC report"));
        assert!(fields["mappedStats"].as_object().is_some_and(|stats| {
            stats
                .get("seq")
                .and_then(Json::as_number)
                .is_some_and(|value| *value > 0.0)
        }));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = rtl_bmc(&request.params, &cancellation).unwrap_err();
        assert_eq!(error, "RTL property BMC cancelled");

        let invalid = RpcRequest::new(15, "rtlBmc").with_params(Json::object([
            ("source", Json::string(source)),
            ("properties", Json::string(properties)),
            ("depth", Json::Number(0.0)),
        ]));
        let error = dispatch(&invalid).result.unwrap_err();
        assert_eq!(error, "'depth' must be a positive integer");
    }

    #[test]
    fn rpc_functional_coverage_returns_structured_bins() {
        let vcd = r#"
$timescale 1ns $end
$var wire 1 ! clk $end
$var wire 1 " req $end
$var wire 2 # state [1:0] $end
$enddefinitions $end
#0
0!
0"
b00 #
#10
1!
1"
b01 #
#20
0!
0"
b01 #
#30
1!
1"
b11 #
"#;
        let coverage = r#"
covergroup cg @(posedge clk);
  cp_req: coverpoint req {
    bins zero = {0};
    bins one = {1};
  }
  cp_state: coverpoint state[1:0] {
    bins low = {[0:1]};
    bins high = {[2:3]};
  }
  x: cross cp_req, cp_state;
endgroup
toggle req;
"#;
        let request = RpcRequest::new(16, "coverage").with_params(Json::object([
            ("vcd", Json::string(vcd)),
            ("coverage", Json::string(coverage)),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["samples"], Json::Number(2.0));
        assert!(matches!(&fields["bins"], Json::Array(bins) if bins.len() == 4));
        assert!(matches!(&fields["crosses"], Json::Array(crosses) if crosses.len() == 4));
        assert!(matches!(&fields["toggles"], Json::Array(toggles) if toggles.len() == 1));
        assert!(fields["percentage"]
            .as_number()
            .is_some_and(|value| *value > 0.0));
        assert!(fields["report"]
            .as_str()
            .is_some_and(|report| report.contains("Coverage report")));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = functional_coverage_rpc(&request.params, &cancellation).unwrap_err();
        assert_eq!(error, "functional coverage analysis cancelled");

        let invalid = RpcRequest::new(17, "coverage").with_params(Json::object([
            ("vcd", Json::string(vcd)),
            ("coverage", Json::string("toggle;")),
        ]));
        let error = dispatch(&invalid).result.unwrap_err();
        assert!(!error.is_empty());
    }

    #[test]
    fn rpc_rtl_dft_returns_structured_scan_chain() {
        let source = r#"
module top(input clk, d, output reg q1, q2);
always @(posedge clk) begin
  q1 <= d;
  q2 <= q1;
end
endmodule
"#;
        let request = RpcRequest::new(18, "rtlDft").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            (
                "scanOptions",
                Json::object([
                    ("chainName", Json::string("agent_chain")),
                    ("scanEnPort", Json::string("test_en")),
                    ("scanInPort", Json::string("test_in")),
                    ("scanOutPort", Json::string("test_out")),
                ]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        let scan = fields["scan"].as_object().unwrap();
        assert_eq!(scan["chainName"], Json::string("agent_chain"));
        assert_eq!(scan["scanEnPort"], Json::string("test_en"));
        assert_eq!(scan["scanInPort"], Json::string("test_in"));
        assert_eq!(scan["scanOutPort"], Json::string("test_out"));
        assert!(matches!(&scan["chain"], Json::Array(chain) if chain.len() == 2));
        let dft = fields["dft"].as_object().unwrap();
        assert_eq!(dft["sequentialCells"], Json::Number(2.0));
        assert_eq!(dft["scanCells"], Json::Number(2.0));
        assert_eq!(dft["scanChains"], Json::Number(1.0));
        assert_eq!(dft["unscannedSequential"], Json::Array(Vec::new()));
        assert!(fields["scannedStats"].as_object().is_some_and(|stats| {
            stats
                .get("cells")
                .and_then(Json::as_number)
                .is_some_and(|value| *value > 2.0)
        }));
        assert!(fields["report"]
            .as_str()
            .is_some_and(|report| report.contains("DFT scan report")));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = rtl_dft(&request.params, &cancellation).unwrap_err();
        assert_eq!(error, "RTL scan insertion cancelled");

        let invalid = RpcRequest::new(19, "rtlDft").with_params(Json::object([
            ("source", Json::string(source)),
            (
                "scanOptions",
                Json::object([("scanEnPort", Json::Number(1.0))]),
            ),
        ]));
        let error = dispatch(&invalid).result.unwrap_err();
        assert_eq!(error, "scan option 'scanEnPort' must be a string");
    }

    #[test]
    fn rpc_rtl_cdc_returns_structured_crossings() {
        let source = r#"
module top(input clk_a, clk_b, d, output reg a_q, b_q);
always @(posedge clk_a) a_q <= d;
always @(posedge clk_b) b_q <= a_q;
endmodule
"#;
        let request = RpcRequest::new(20, "rtlCdc").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert!(matches!(&fields["clocks"], Json::Array(clocks) if clocks.len() == 2));
        assert!(matches!(&fields["crossings"], Json::Array(crossings) if !crossings.is_empty()));
        let findings = match &fields["findings"] {
            Json::Array(findings) => findings,
            other => panic!("expected CDC findings, got {other:?}"),
        };
        assert!(!findings.is_empty());
        assert_eq!(
            findings[0].as_object().unwrap()["severity"],
            Json::string("error")
        );
        assert_eq!(fields["hasErrors"], Json::Bool(true));
        assert_eq!(fields["clean"], Json::Bool(false));
        assert!(fields["report"].as_str().unwrap().contains("CDC report"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = rtl_cdc(&request.params, &cancellation).unwrap_err();
        assert_eq!(error, "RTL CDC analysis cancelled");
    }

    #[test]
    fn rpc_rtl_fault_returns_structured_coverage_and_stil() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let request = RpcRequest::new(21, "rtlFault").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            (
                "patternOptions",
                Json::object([
                    ("maxRandomPatterns", Json::Number(32.0)),
                    ("exhaustiveInputLimit", Json::Number(8.0)),
                    ("randomSeed", Json::Number(123.0)),
                ]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert!(matches!(&fields["patterns"], Json::Array(patterns) if patterns.len() == 4));
        assert_eq!(fields["exhaustive"], Json::Bool(true));
        assert_eq!(
            fields["detectedFaults"]
                .as_number()
                .map(|value| *value as usize),
            fields["totalFaults"]
                .as_number()
                .map(|value| *value as usize)
        );
        assert_eq!(fields["coveragePercent"], Json::Number(100.0));
        assert_eq!(fields["undetectedFaults"], Json::Array(Vec::new()));
        assert!(matches!(&fields["detections"], Json::Array(detections) if !detections.is_empty()));
        assert!(fields["stil"].as_str().unwrap().contains("STIL 1.0"));
        assert!(fields["report"]
            .as_str()
            .is_some_and(|report| report.contains("Fault report")));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = rtl_fault(&request.params, &cancellation).unwrap_err();
        assert_eq!(error, "RTL fault analysis cancelled");

        let invalid = RpcRequest::new(22, "rtlFault").with_params(Json::object([
            ("source", Json::string(source)),
            (
                "patternOptions",
                Json::object([("exhaustiveInputLimit", Json::Number(65.0))]),
            ),
        ]));
        let error = dispatch(&invalid).result.unwrap_err();
        assert_eq!(
            error,
            "pattern option 'exhaustiveInputLimit' must be an integer from 1 through 64"
        );
    }

    #[test]
    fn rpc_rtl_simulation_returns_timed_vectors() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let request = RpcRequest::new(7, "rtlSim").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            (
                "vectors",
                Json::Array(vec![
                    Json::object([
                        ("timeNs", Json::Number(0.0)),
                        (
                            "inputs",
                            Json::object([("a", Json::string("0")), ("b", Json::string("1"))]),
                        ),
                    ]),
                    Json::object([
                        ("timeNs", Json::Number(2.0)),
                        (
                            "inputs",
                            Json::object([("a", Json::string("'b1")), ("b", Json::string("0x1"))]),
                        ),
                    ]),
                ]),
            ),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["finished"], Json::Bool(false));
        assert_eq!(fields["finalTimeNs"], Json::Number(2.0));
        let Json::Array(samples) = &fields["samples"] else {
            panic!("simulation samples are not an array");
        };
        assert_eq!(samples.len(), 2);
        assert_eq!(
            samples[0].as_object().unwrap()["outputs"]
                .as_object()
                .unwrap()["y"],
            Json::string("0")
        );
        assert_eq!(
            samples[1].as_object().unwrap()["outputs"]
                .as_object()
                .unwrap()["y"],
            Json::string("1")
        );

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = rtl_sim(&request.params, &cancellation).unwrap_err();
        assert_eq!(error, "RTL simulation cancelled");
    }

    #[test]
    fn rpc_pdk_signoff_returns_structured_result() {
        let source = "module top(input a, output y); assign y = ~a; endmodule";
        let manifest = r#"pdk rpc-pdk
version 2.0
corner vendor process TT voltage_v 1.8 temperature_c 25 delay_scale 1
liberty vendor <<END
library (rpc-vendor) {
    time_unit : "1ns";
    cell (INV) {
        area : 1.2;
        pin (A) { direction : input; capacitance : 0.01; }
        pin (Y) {
            direction : output;
            function : "!A";
            timing () { related_pin : "A"; cell_rise(values("0.35,0.42")); }
        }
    }
}
END
correlate nmos VTO nominal 0.45 golden 0.452 tolerance 0.01
signoff model-correlation
signoff equivalence
signoff timing
signoff drc
signoff physical-database
.end
"#;
        let request = RpcRequest::new(5, "pdkSignoff").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            ("pdk", Json::string(manifest)),
        ]));
        let response = dispatch(&request).result.unwrap();
        let fields = response.as_object().unwrap();
        assert_eq!(fields["passed"], Json::Bool(true));
        assert_eq!(fields["pdk"], Json::string("rpc-pdk"));
        let report = fields["report"].as_str().unwrap();
        assert!(report.contains("imported-liberty=true"));
        assert!(report.contains("Overall signoff: PASS"));
    }

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
            &RpcRequest::new(3, "jobStatus")
                .with_params(Json::object([("jobId", Json::Number(1.0))])),
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
            &RpcRequest::new(4, "jobCancel")
                .with_params(Json::object([("jobId", Json::Number(99.0))])),
        );
        assert!(cancel.wire().contains("\"cancelled\":true"));
        service.shutdown().unwrap();
    }

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
        let request = RpcRequest::new(1, "rtlFlow")
            .with_params(Json::object([("source", Json::string(source))]));
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
}
