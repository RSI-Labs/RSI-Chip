//! Persistent JSON-RPC-style job service for long-running agent sessions.
//!
//! The protocol is deliberately line-delimited JSON so an agent can use a
//! socket without linking the Rust API. Jobs are also exposed as a native,
//! typed manager for in-process automation.

use eda_api::{
    analyze_rtl_cdc_cancellable, analyze_rtl_faults_cancellable, analyze_rtl_ir_cancellable,
    analyze_rtl_power_cancellable, analyze_rtl_timing_cancellable, apply_eco, assertion_report,
    capability_report, cdc_report, check_assertions, check_rtl_equivalence_cancellable,
    check_rtl_properties_cancellable, clock_tree_synthesis_with_placement, coverage_report,
    cts_report, dc_operating_point, eco_report, export_verilog, fault_report, functional_coverage,
    implement_rtl_pnr_cancellable, insert_rtl_scan_cancellable, ir_report, parse_assertions,
    parse_covergroups, parse_eco, parse_pdk, parse_spice, parse_waveform, power_report,
    report_ams_result, report_pdk_signoff, run_all_ams_analyses_cancellable,
    run_ams_analysis_cancellable, run_flow_cancellable, run_flow_with_library_cancellable,
    run_pdk_signoff_cancellable, simulate_rtl_cancellable,
    timing_driven_clock_tree_synthesis_with_placement, timing_driven_cts_report, AmsResult,
    CellLibrary, Flow, FlowCacheKey, FlowCancellation, IrOptions, PatternOptions, PowerOptions,
    PropertyBmcStatus, RtlSimulationVector, ScanOptions, Severity as CdcSeverity, SharedFlowCache,
    TimingDrivenCtsOptions,
};
use std::collections::BTreeMap;
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

/// Stateful RPC front end shared by all TCP connections.
///
/// Direct requests remain synchronous. `jobSubmit`, `jobStatus`, `jobCancel`,
/// and `jobWait` expose the persistent worker queue to remote agents.
#[derive(Clone)]
pub struct RpcService {
    manager: Arc<Mutex<RpcJobManager>>,
    flow_cache: SharedFlowCache,
}

impl RpcService {
    pub fn new() -> Self {
        Self::with_flow_cache_capacity(128).expect("default RPC flow cache capacity")
    }

    pub fn with_flow_cache_capacity(capacity: usize) -> Result<Self, String> {
        let flow_cache = SharedFlowCache::new(capacity)?;
        let dispatcher_cache = flow_cache.clone();
        let manager = RpcJobManager::new_with_dispatcher(Arc::new(move |request, cancellation| {
            dispatch_result_with_flow_cache(&request, &cancellation, &dispatcher_cache)
        }));
        Ok(Self {
            manager: Arc::new(Mutex::new(manager)),
            flow_cache,
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
            "rtlFlowCache" => {
                dispatch_result_with_flow_cache(request, &FlowCancellation::new(), &self.flow_cache)
            }
            "flowCacheClear" => {
                self.flow_cache.clear()?;
                Ok(Json::object([("cleared", Json::Bool(true))]))
            }
            _ => {
                dispatch_result_with_flow_cache(request, &FlowCancellation::new(), &self.flow_cache)
            }
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
        "layoutDrc" => layout_drc_rpc(&request.params, &FlowCancellation::new()),
        "spiceDc" => spice_dc(&request.params),
        "spiceRun" => spice_run(&request.params, &FlowCancellation::new()),
        "rtlFlow" => rtl_flow(&request.params, &FlowCancellation::new()),
        "rtlCts" => rtl_cts(&request.params, &FlowCancellation::new()),
        "rtlEco" => rtl_eco(&request.params, &FlowCancellation::new()),
        "rtlEquiv" => rtl_equiv(&request.params, &FlowCancellation::new()),
        "rtlSta" => rtl_sta(&request.params, &FlowCancellation::new()),
        "rtlPnr" => rtl_pnr(&request.params, &FlowCancellation::new()),
        "rtlPower" => rtl_power(&request.params, &FlowCancellation::new()),
        "rtlIr" => rtl_ir(&request.params, &FlowCancellation::new()),
        "rtlBmc" => rtl_bmc(&request.params, &FlowCancellation::new()),
        "rtlDft" => rtl_dft(&request.params, &FlowCancellation::new()),
        "rtlCdc" => rtl_cdc(&request.params, &FlowCancellation::new()),
        "rtlFault" => rtl_fault(&request.params, &FlowCancellation::new()),
        "rtlSim" => rtl_sim(&request.params, &FlowCancellation::new()),
        "pdkSignoff" => pdk_signoff(&request.params, &FlowCancellation::new()),
        method => Err(format!("unknown RPC method '{method}'")),
    }
}

fn dispatch_result_with_flow_cache(
    request: &RpcRequest,
    cancellation: &FlowCancellation,
    flow_cache: &SharedFlowCache,
) -> Result<Json, String> {
    match request.method.as_str() {
        "rtlFlowCache" => rtl_flow_cached(&request.params, cancellation, flow_cache),
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
        "layoutDrc" => layout_drc_rpc(&request.params, cancellation),
        "rtlFlow" => rtl_flow(&request.params, cancellation),
        "rtlCts" => rtl_cts(&request.params, cancellation),
        "rtlEco" => rtl_eco(&request.params, cancellation),
        "rtlEquiv" => rtl_equiv(&request.params, cancellation),
        "rtlSta" => rtl_sta(&request.params, cancellation),
        "rtlPnr" => rtl_pnr(&request.params, cancellation),
        "rtlPower" => rtl_power(&request.params, cancellation),
        "rtlIr" => rtl_ir(&request.params, cancellation),
        "rtlBmc" => rtl_bmc(&request.params, cancellation),
        "rtlDft" => rtl_dft(&request.params, cancellation),
        "rtlCdc" => rtl_cdc(&request.params, cancellation),
        "rtlFault" => rtl_fault(&request.params, cancellation),
        "rtlSim" => rtl_sim(&request.params, cancellation),
        "pdkSignoff" => pdk_signoff(&request.params, cancellation),
        "spiceRun" => spice_run(&request.params, cancellation),
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

fn layout_drc_rpc(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout DRC cancelled".into());
    }
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let mut layout = eda_api::LayoutDocument::new();
    for value in optional_json_array(fields, "layerRules")? {
        let rule = value
            .as_object()
            .ok_or_else(|| "each layer rule must be an object".to_string())?;
        layout.set_layer_rule(eda_api::LayoutLayerRule::new(
            required_object_string(rule, "layer")?,
            required_object_i64(rule, "minWidth")?,
            required_object_i64(rule, "minSpacing")?,
            required_object_i64(rule, "minArea")?,
        )?)?;
    }
    for value in optional_json_array(fields, "viaRules")? {
        let rule = value
            .as_object()
            .ok_or_else(|| "each via rule must be an object".to_string())?;
        layout.set_via_rule(eda_api::LayoutViaRule::new(
            required_object_string(rule, "name")?,
            required_object_string(rule, "cutLayer")?,
            required_object_string(rule, "bottomLayer")?,
            required_object_string(rule, "topLayer")?,
            required_object_i64(rule, "cutWidth")?,
            required_object_i64(rule, "cutHeight")?,
            required_object_i64(rule, "minBottomEnclosure")?,
            required_object_i64(rule, "minTopEnclosure")?,
        )?)?;
    }
    for value in optional_json_array(fields, "shapes")? {
        let shape = value
            .as_object()
            .ok_or_else(|| "each layout shape must be an object".to_string())?;
        let geometry = if let Some(rect) = shape.get("rect") {
            eda_api::LayoutGeometry::Rect(parse_rect(rect)?)
        } else if let Some(polygon) = shape.get("polygon") {
            eda_api::LayoutGeometry::Polygon(eda_api::LayoutPolygon::new(parse_points(polygon)?)?)
        } else {
            return Err("each layout shape needs 'rect' or 'polygon'".into());
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
    if cancellation.is_cancelled() {
        return Err("layout DRC cancelled".into());
    }
    let result = layout.check_layout_drc();
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
    ]))
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

fn spice_dc(params: &Json) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let circuit = parse_spice(&source)?;
    let op = dc_operating_point(&circuit)?;
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

fn rtl_pnr(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let result = implement_rtl_pnr_cancellable(&source, top, cancellation)?;
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
    fn rpc_spice_dc_returns_typed_operating_point() {
        let source = "rpc divider\nVin in 0 DC 1.5\nR1 in out 1k\nR2 out 0 2k\n.end";
        let request = RpcRequest::new(3, "spiceDc")
            .with_params(Json::object([("source", Json::string(source))]));
        let response = dispatch(&request);
        let wire = response.wire();
        assert!(wire.contains("\"converged\":true"), "{wire}");
        assert!(wire.contains("\"out\":1"), "{wire}");
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

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = rtl_sta(&request.params, &cancellation).unwrap_err();
        assert_eq!(error, "RTL timing analysis cancelled");
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
