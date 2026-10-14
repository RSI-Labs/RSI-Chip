//! Dependency-free browser host for the interactive schematic session.
//!
//! The host serves a small HTML/JavaScript shell and a local HTTP API. Pointer
//! events, tool changes, deletion, and undo/redo are forwarded to the same
//! typed `SchematicSession` used by Rust automation and the regression suite.

use super::{SchematicDocument, SchematicEvent, SchematicSession, SchematicTool};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{spawn, JoinHandle};

const MAX_REQUEST_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct SchematicBrowserService {
    session: Arc<Mutex<SchematicSession>>,
}

impl SchematicBrowserService {
    pub fn new(document: SchematicDocument) -> Result<Self, String> {
        Ok(Self {
            session: Arc::new(Mutex::new(SchematicSession::new(document)?)),
        })
    }

    pub fn handle_request(&self, request: &str) -> BrowserHttpResponse {
        let Some((head, body)) = request.split_once("\r\n\r\n") else {
            return BrowserHttpResponse::bad_request("request must contain headers and body");
        };
        let mut lines = head.lines();
        let Some(request_line) = lines.next() else {
            return BrowserHttpResponse::bad_request("empty HTTP request");
        };
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default();
        let path = parts.next().unwrap_or_default();
        let mut content_length = 0_usize;
        for line in lines {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or_default();
            }
        }
        if body.len() != content_length {
            return BrowserHttpResponse::bad_request("Content-Length does not match body");
        }
        match (method, path) {
            ("GET", "/") => BrowserHttpResponse {
                status: "200 OK".into(),
                content_type: "text/html; charset=utf-8".into(),
                body: editor_html().into(),
            },
            ("GET", "/svg") => match self.render() {
                Ok(svg) => BrowserHttpResponse {
                    status: "200 OK".into(),
                    content_type: "image/svg+xml".into(),
                    body: svg.into(),
                },
                Err(error) => BrowserHttpResponse::bad_request(&error),
            },
            ("GET", "/properties") => match self.properties_json() {
                Ok(json) => BrowserHttpResponse {
                    status: "200 OK".into(),
                    content_type: "application/json".into(),
                    body: json.write().into(),
                },
                Err(error) => BrowserHttpResponse::server_error(&error),
            },
            ("POST", "/event") => self.event(body),
            _ => BrowserHttpResponse {
                status: "404 Not Found".into(),
                content_type: "application/json".into(),
                body: br#"{"ok":false,"error":"not found"}"#.to_vec(),
            },
        }
    }

    fn render(&self) -> Result<String, String> {
        self.session
            .lock()
            .map_err(|_| "schematic browser session mutex poisoned".to_string())?
            .render_svg()
    }

    fn properties_json(&self) -> Result<BrowserJson, String> {
        let session = self
            .session
            .lock()
            .map_err(|_| "schematic browser session mutex poisoned".to_string())?;
        Ok(properties_json(&session.properties()))
    }

    fn event(&self, body: &str) -> BrowserHttpResponse {
        let value = match BrowserJson::parse(body) {
            Ok(value) => value,
            Err(error) => return BrowserHttpResponse::bad_request(&error),
        };
        let event = match event_from_json(&value) {
            Ok(event) => event,
            Err(error) => return BrowserHttpResponse::bad_request(&error),
        };
        let mut session = match self.session.lock() {
            Ok(session) => session,
            Err(_) => {
                return BrowserHttpResponse::server_error(
                    "schematic browser session mutex poisoned",
                )
            }
        };
        match session.process(event) {
            Ok(outcome) => {
                let svg = match session.render_svg() {
                    Ok(svg) => svg,
                    Err(error) => return BrowserHttpResponse::server_error(&error),
                };
                let response = BrowserJson::object([
                    ("ok", BrowserJson::Bool(true)),
                    ("outcome", BrowserJson::string(format!("{outcome:?}"))),
                    ("svg", BrowserJson::String(svg)),
                    ("properties", properties_json(&session.properties())),
                ])
                .write();
                BrowserHttpResponse {
                    status: "200 OK".into(),
                    content_type: "application/json".into(),
                    body: response.into(),
                }
            }
            Err(error) => BrowserHttpResponse::server_error(&error),
        }
    }
}

pub struct BrowserHttpResponse {
    pub status: String,
    pub content_type: String,
    pub body: Vec<u8>,
}

impl BrowserHttpResponse {
    pub fn write(&self, writer: &mut impl Write) -> std::io::Result<()> {
        let head = format!(
            "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
            self.status,
            self.content_type,
            self.body.len()
        );
        writer.write_all(head.as_bytes())?;
        writer.write_all(&self.body)?;
        writer.flush()
    }

    fn bad_request(message: &str) -> Self {
        let body = format!("{{\"ok\":false,\"error\":\"{}\"}}", json_escape(message));
        Self {
            status: "400 Bad Request".into(),
            content_type: "application/json".into(),
            body: body.into(),
        }
    }

    fn server_error(message: &str) -> Self {
        let body = format!("{{\"ok\":false,\"error\":\"{}\"}}", json_escape(message));
        Self {
            status: "500 Internal Server Error".into(),
            content_type: "application/json".into(),
            body: body.into(),
        }
    }
}

pub struct SchematicBrowserServer {
    address: String,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl SchematicBrowserServer {
    pub fn bind(document: SchematicDocument, address: &str) -> Result<Self, String> {
        let listener = TcpListener::bind(address)
            .map_err(|error| format!("failed to bind schematic editor to {address}: {error}"))?;
        let address = listener
            .local_addr()
            .map(|address| address.to_string())
            .map_err(|error| format!("failed to resolve schematic editor address: {error}"))?;
        let service = SchematicBrowserService::new(document)?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let worker = spawn(move || accept_loop(listener, service, worker_shutdown));
        Ok(Self {
            address,
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
                .map_err(|_| "schematic editor worker panicked".to_string())?;
        }
        Ok(())
    }
}

fn accept_loop(listener: TcpListener, service: SchematicBrowserService, shutdown: Arc<AtomicBool>) {
    for stream in listener.incoming() {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else {
            continue;
        };
        let service = service.clone();
        spawn(move || serve_connection(stream, service));
    }
}

fn serve_connection(stream: TcpStream, service: SchematicBrowserService) {
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut head = Vec::new();
    let mut writer = stream;
    let bytes = match reader.read_until(b'\n', &mut head) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };
    if bytes == 0 {
        return;
    }
    let mut request = String::from_utf8_lossy(&head).into_owned();
    loop {
        head.clear();
        let bytes = match reader.read_until(b'\n', &mut head) {
            Ok(bytes) => bytes,
            Err(_) => return,
        };
        if bytes == 0 {
            return;
        }
        request.push_str(&String::from_utf8_lossy(&head));
        if head == b"\r\n" || head == b"\n" {
            break;
        }
    }
    let content_length = request
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .next()
        .unwrap_or(0);
    if content_length > MAX_REQUEST_BYTES {
        let response = BrowserHttpResponse::bad_request("request body is too large");
        let _ = response.write(&mut writer);
        return;
    }
    let mut body = vec![0_u8; content_length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    request.push_str(&String::from_utf8_lossy(&body));
    let response = service.handle_request(&request);
    let _ = response.write(&mut writer);
}

#[derive(Clone, Debug, PartialEq)]
enum BrowserJson {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Self>),
    Object(BTreeMap<String, Self>),
}

impl BrowserJson {
    fn object(entries: impl IntoIterator<Item = (impl Into<String>, Self)>) -> Self {
        Self::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    fn string(value: impl Into<String>) -> Self {
        Self::String(value.into())
    }

    fn parse(text: &str) -> Result<Self, String> {
        let mut parser = BrowserJsonParser {
            bytes: text.as_bytes(),
            position: 0,
        };
        parser.skip_whitespace();
        let value = parser.value()?;
        parser.skip_whitespace();
        if parser.position != parser.bytes.len() {
            return Err("trailing JSON characters".into());
        }
        Ok(value)
    }

    fn object_field(&self, name: &str) -> Option<&Self> {
        match self {
            Self::Object(fields) => fields.get(name),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    fn write(&self) -> String {
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

struct BrowserJsonParser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> BrowserJsonParser<'a> {
    fn skip_whitespace(&mut self) {
        while matches!(
            self.bytes.get(self.position),
            Some(b' ' | b'\t' | b'\r' | b'\n')
        ) {
            self.position += 1;
        }
    }

    fn value(&mut self) -> Result<BrowserJson, String> {
        match self.bytes.get(self.position) {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(BrowserJson::String(self.string()?)),
            Some(b't') => self.literal("true", BrowserJson::Bool(true)),
            Some(b'f') => self.literal("false", BrowserJson::Bool(false)),
            Some(b'n') => self.literal("null", BrowserJson::Null),
            Some(byte @ (b'-' | b'0'..=b'9')) => self.number(*byte),
            _ => Err("invalid JSON value".into()),
        }
    }

    fn literal(&mut self, text: &str, value: BrowserJson) -> Result<BrowserJson, String> {
        if self.bytes[self.position..].starts_with(text.as_bytes()) {
            self.position += text.len();
            Ok(value)
        } else {
            Err("invalid JSON literal".into())
        }
    }

    fn number(&mut self, first: u8) -> Result<BrowserJson, String> {
        let start = self.position;
        self.position += 1;
        while matches!(
            self.bytes.get(self.position),
            Some(b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
        ) {
            self.position += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.position])
            .map_err(|_| "invalid JSON number".to_string())?;
        let value = text
            .parse::<f64>()
            .map_err(|_| "invalid JSON number".to_string())?;
        if first == b'-' {
            let _ = first;
        }
        Ok(BrowserJson::Number(value))
    }

    fn array(&mut self) -> Result<BrowserJson, String> {
        self.position += 1;
        let mut values = Vec::new();
        self.skip_whitespace();
        if self.bytes.get(self.position) == Some(&b']') {
            self.position += 1;
            return Ok(BrowserJson::Array(values));
        }
        loop {
            self.skip_whitespace();
            values.push(self.value()?);
            self.skip_whitespace();
            match self.bytes.get(self.position) {
                Some(b',') => self.position += 1,
                Some(b']') => {
                    self.position += 1;
                    return Ok(BrowserJson::Array(values));
                }
                _ => return Err("unterminated JSON array".into()),
            }
        }
    }

    fn object(&mut self) -> Result<BrowserJson, String> {
        self.position += 1;
        let mut fields = BTreeMap::new();
        self.skip_whitespace();
        if self.bytes.get(self.position) == Some(&b'}') {
            self.position += 1;
            return Ok(BrowserJson::Object(fields));
        }
        loop {
            self.skip_whitespace();
            let key = self.string()?;
            self.skip_whitespace();
            if self.bytes.get(self.position) != Some(&b':') {
                return Err("JSON object field requires ':'".into());
            }
            self.position += 1;
            self.skip_whitespace();
            let value = self.value()?;
            if fields.insert(key, value).is_some() {
                return Err("duplicate JSON object field".into());
            }
            self.skip_whitespace();
            match self.bytes.get(self.position) {
                Some(b',') => self.position += 1,
                Some(b'}') => {
                    self.position += 1;
                    return Ok(BrowserJson::Object(fields));
                }
                _ => return Err("unterminated JSON object".into()),
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        if self.bytes.get(self.position) != Some(&b'"') {
            return Err("expected JSON string".into());
        }
        self.position += 1;
        let mut out = String::new();
        while let Some(byte) = self.bytes.get(self.position) {
            match byte {
                b'"' => {
                    self.position += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.position += 1;
                    let escape = self
                        .bytes
                        .get(self.position)
                        .ok_or_else(|| "unterminated JSON escape".to_string())?;
                    self.position += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex = self
                                .bytes
                                .get(self.position..self.position + 4)
                                .ok_or_else(|| "short unicode escape".to_string())?;
                            let text =
                                std::str::from_utf8(hex).map_err(|_| "bad unicode escape")?;
                            let code =
                                u32::from_str_radix(text, 16).map_err(|_| "bad unicode escape")?;
                            let character = char::from_u32(code)
                                .ok_or_else(|| "invalid unicode escape".to_string())?;
                            out.push(character);
                            self.position += 4;
                        }
                        _ => return Err("invalid JSON escape".into()),
                    }
                }
                0x00..=0x1f => return Err("control character in JSON string".into()),
                _ => {
                    let start = self.position;
                    while self
                        .bytes
                        .get(self.position)
                        .is_some_and(|byte| *byte != b'"' && *byte != b'\\' && *byte > 0x1f)
                    {
                        self.position += 1;
                    }
                    out.push_str(&String::from_utf8_lossy(&self.bytes[start..self.position]));
                }
            }
        }
        Err("unterminated JSON string".into())
    }
}

fn event_from_json(value: &BrowserJson) -> Result<SchematicEvent, String> {
    let kind = value
        .object_field("type")
        .and_then(BrowserJson::as_str)
        .ok_or_else(|| "event requires string 'type'".to_string())?;
    match kind {
        "pointerDown" | "pointerMove" | "pointerUp" => {
            let point = point_from_json(value)?;
            Ok(match kind {
                "pointerDown" => SchematicEvent::PointerDown { point },
                "pointerMove" => SchematicEvent::PointerMove { point },
                _ => SchematicEvent::PointerUp { point },
            })
        }
        "setTool" => {
            let tool = value
                .object_field("tool")
                .and_then(BrowserJson::as_str)
                .ok_or_else(|| "setTool requires string 'tool'".to_string())?;
            let tool = match tool {
                "select" => SchematicTool::Select,
                "wire" => SchematicTool::Wire,
                _ => return Err("tool must be 'select' or 'wire'".into()),
            };
            Ok(SchematicEvent::SetTool { tool })
        }
        "undo" => Ok(SchematicEvent::Undo),
        "redo" => Ok(SchematicEvent::Redo),
        "delete" => Ok(SchematicEvent::Delete),
        _ => Err(format!("unsupported schematic event type '{kind}'")),
    }
}

fn properties_json(properties: &BTreeMap<String, String>) -> BrowserJson {
    BrowserJson::Object(
        properties
            .iter()
            .map(|(key, value)| (key.clone(), BrowserJson::String(value.clone())))
            .collect(),
    )
}

fn write_json_string(value: &str, out: &mut String) {
    out.push('"');
    out.push_str(&json_escape(value));
    out.push('"');
}

fn point_from_json(value: &BrowserJson) -> Result<super::Point, String> {
    let point = value
        .object_field("point")
        .ok_or_else(|| "pointer event requires 'point'".to_string())?;
    let x = point
        .object_field("x")
        .and_then(BrowserJson::as_f64)
        .ok_or_else(|| "point.x must be a number".to_string())?;
    let y = point
        .object_field("y")
        .and_then(BrowserJson::as_f64)
        .ok_or_else(|| "point.y must be a number".to_string())?;
    Ok(super::Point::new(x, y))
}

fn json_escape(value: &str) -> String {
    let mut out = String::new();
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if character < ' ' => out.push_str(&format!("\\u{:04x}", character as u32)),
            character => out.push(character),
        }
    }
    out
}

fn editor_html() -> &'static str {
    r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>Rust EDA Schematic Editor</title>
  <style>
    body { margin: 0; font: 14px system-ui, sans-serif; background: #f8fafc; color: #111827; }
    header { display: flex; gap: 8px; align-items: center; padding: 10px 14px; background: #111827; color: #f9fafb; }
    button { border: 0; border-radius: 5px; padding: 6px 10px; cursor: pointer; background: #e5e7eb; }
    button[aria-pressed=true] { background: #2563eb; color: white; }
    main { display: flex; gap: 12px; padding: 12px 14px; align-items: flex-start; }
    #stage { flex: 1 1 auto; height: calc(100vh - 115px); background: white; border: 1px solid #cbd5e1; overflow: auto; resize: both; }
    svg { display: block; min-width: 100%; }
    #properties { flex: 0 0 300px; background: white; border: 1px solid #cbd5e1; padding: 10px; overflow: auto; max-height: calc(100vh - 115px); }
    #properties table { width: 100%; border-collapse: collapse; }
    #properties td { border-bottom: 1px solid #e2e8f0; padding: 5px 4px; vertical-align: top; word-break: break-word; }
    #properties td:first-child { width: 42%; color: #475569; }
    #status { padding: 0 14px 14px; color: #475569; white-space: pre-wrap; }
  </style>
</head>
<body>
<header>
  <strong>Rust EDA Schematic Editor</strong>
  <button id="select" aria-pressed="true">Select</button>
  <button id="wire" aria-pressed="false">Wire</button>
  <button id="undo">Undo</button>
  <button id="redo">Redo</button>
  <button id="delete">Delete</button>
  <button id="zoom-out">-</button><button id="zoom-in">+</button>
  <button id="pan-left">←</button><button id="pan-right">→</button>
  <button id="pan-up">↑</button><button id="pan-down">↓</button>
  <button id="reset-view">Reset view</button>
</header>
<main>
<div id="stage"></div>
<aside id="properties" aria-label="Selection properties"><p class="empty">No selection</p></aside>
</main>
<div id="status">Ready</div>
<script>
const stage = document.getElementById('stage');
const status = document.getElementById('status');
const properties = document.getElementById('properties');
let active = false;
let panning = false;
let panLast = null;
let zoom = 1;
function setPressed(tool) {
  selectButton.ariaPressed = String(tool === 'select');
  wireButton.ariaPressed = String(tool === 'wire');
}
async function refresh() {
  const response = await fetch('/svg', {cache: 'no-store'});
  if (!response.ok) throw new Error(await response.text());
  stage.innerHTML = await response.text();
  applyView();
  await loadProperties();
}
async function send(payload) {
  const response = await fetch('/event', {
    method: 'POST',
    headers: {'Content-Type': 'application/json'},
    body: JSON.stringify(payload)
  });
  const result = await response.json();
  if (!result.ok) throw new Error(result.error);
  status.textContent = result.outcome;
  if (result.svg) stage.innerHTML = result.svg;
  applyView();
  if (result.properties) renderProperties(result.properties);
}
function applyView() {
  const svg = stage.querySelector('svg');
  if (!svg) return;
  zoom = Math.min(10, Math.max(0.1, zoom));
  svg.style.width = `${zoom * 100}%`;
  svg.style.height = 'auto';
}
function setZoom(next) {
  zoom = Math.min(10, Math.max(0.1, next));
  applyView();
}
function pan(dx, dy) {
  stage.scrollLeft += dx;
  stage.scrollTop += dy;
}
async function loadProperties() {
  const response = await fetch('/properties', {cache: 'no-store'});
  if (!response.ok) throw new Error(await response.text());
  renderProperties(await response.json());
}
function renderProperties(values) {
  const entries = Object.entries(values || {});
  properties.innerHTML = entries.length
    ? `<table><tbody>${entries.map(([key, value]) =>
        `<tr><td>${key}</td><td>${value}</td></tr>`).join('')}</tbody></table>`
    : '<p class="empty">No properties</p>';
}
function documentPoint(event) {
  const svg = stage.querySelector('svg');
  const transform = svg.getScreenCTM().inverse();
  const point = new DOMPoint(event.clientX, event.clientY).matrixTransform(transform);
  return {x: point.x, y: point.y};
}
stage.addEventListener('pointerdown', async event => {
  if (event.button === 1 || event.altKey) {
    panning = true;
    panLast = {x: event.clientX, y: event.clientY};
    event.preventDefault();
    return;
  }
  event.preventDefault();
  active = true;
  await send({type: 'pointerDown', point: documentPoint(event)});
});
stage.addEventListener('pointermove', async event => {
  if (panning) {
    if (panLast) {
      pan(panLast.x - event.clientX, panLast.y - event.clientY);
      panLast = {x: event.clientX, y: event.clientY};
    }
    return;
  }
  if (!active) return;
  await send({type: 'pointerMove', point: documentPoint(event)});
});
window.addEventListener('pointerup', async event => {
  if (panning) {
    panning = false;
    panLast = null;
    return;
  }
  if (!active) return;
  active = false;
  await send({type: 'pointerUp', point: documentPoint(event)});
});
selectButton.onclick = () => send({type: 'setTool', tool: 'select'}).then(() => setPressed('select'));
wireButton.onclick = () => send({type: 'setTool', tool: 'wire'}).then(() => setPressed('wire'));
undoButton.onclick = () => send({type: 'undo'});
redoButton.onclick = () => send({type: 'redo'});
deleteButton.onclick = () => send({type: 'delete'});
zoomInButton.onclick = () => setZoom(zoom * 1.25);
zoomOutButton.onclick = () => setZoom(zoom / 1.25);
panLeftButton.onclick = () => pan(-100, 0);
panRightButton.onclick = () => pan(100, 0);
panUpButton.onclick = () => pan(0, -100);
panDownButton.onclick = () => pan(0, 100);
resetViewButton.onclick = () => { zoom = 1; stage.scrollLeft = 0; stage.scrollTop = 0; applyView(); };
stage.addEventListener('wheel', event => {
  if (!event.ctrlKey) return;
  event.preventDefault();
  setZoom(zoom * (event.deltaY < 0 ? 1.1 : 1 / 1.1));
}, {passive: false});
window.addEventListener('keydown', event => {
  if (event.key === 'Escape') selectButton.click();
  if (event.key === 'Delete' || event.key === 'Backspace') deleteButton.click();
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'z') {
    event.shiftKey ? redoButton.click() : undoButton.click();
  }
  if (event.key === '+') zoomInButton.click();
  if (event.key === '-') zoomOutButton.click();
  if (event.key === 'ArrowLeft') panLeftButton.click();
  if (event.key === 'ArrowRight') panRightButton.click();
  if (event.key === 'ArrowUp') panUpButton.click();
  if (event.key === 'ArrowDown') panDownButton.click();
});
refresh().catch(error => status.textContent = error);
</script>
</body>
</html>"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InstanceTransform, Point, Symbol, SymbolPin};

    fn document() -> SchematicDocument {
        let mut document = SchematicDocument::new("browser");
        document
            .add_symbol(Symbol {
                name: "res".into(),
                width: 20.0,
                height: 8.0,
                pins: vec![
                    SymbolPin {
                        name: "A".into(),
                        position: Point::new(0.0, 4.0),
                    },
                    SymbolPin {
                        name: "B".into(),
                        position: Point::new(20.0, 4.0),
                    },
                ],
            })
            .unwrap();
        document
            .add_instance("R1", "res", InstanceTransform::default())
            .unwrap();
        document
    }

    #[test]
    fn browser_service_serves_html_svg_and_events() {
        let service = SchematicBrowserService::new(document()).unwrap();
        let html = service.handle_request("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(html.status, "200 OK");
        assert!(std::str::from_utf8(&html.body)
            .unwrap()
            .contains("Rust EDA Schematic Editor"));
        let html = std::str::from_utf8(&html.body).unwrap();
        assert!(html.contains("id=\"zoom-in\"") && html.contains("id=\"pan-left\""));
        assert!(html.contains("id=\"properties\"") && html.contains("fetch('/properties'"));
        let svg = service.handle_request("GET /svg HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert!(std::str::from_utf8(&svg.body).unwrap().contains("<svg"));
        let properties =
            service.handle_request("GET /properties HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert!(std::str::from_utf8(&properties.body)
            .unwrap()
            .contains("\"selection\":\"none\""));
        let response = service.handle_request(
            "POST /event HTTP/1.1\r\nHost: localhost\r\nContent-Length: 45\r\n\r\n{\"type\":\"pointerDown\",\"point\":{\"x\":10,\"y\":4}}",
        );
        assert_eq!(response.status, "200 OK");
        assert!(std::str::from_utf8(&response.body)
            .unwrap()
            .contains("SelectionChanged"));
        assert!(std::str::from_utf8(&response.body)
            .unwrap()
            .contains("\"name\":\"R1\""));
        assert!(std::str::from_utf8(&response.body)
            .unwrap()
            .contains("\"symbol\":\"res\""));
    }

    #[test]
    fn browser_json_and_http_errors_are_typed() {
        assert_eq!(
            BrowserJson::parse("{\"a\":[1,2,{\"b\":\"x\\n\"}]}").unwrap(),
            BrowserJson::Object(BTreeMap::from([(
                "a".into(),
                BrowserJson::Array(vec![
                    BrowserJson::Number(1.0),
                    BrowserJson::Number(2.0),
                    BrowserJson::Object(BTreeMap::from([(
                        "b".into(),
                        BrowserJson::String("x\n".into())
                    )]))
                ])
            )]))
        );
        assert!(BrowserJson::parse("{\"a\":1,\"a\":2}").is_err());
        let service = SchematicBrowserService::new(document()).unwrap();
        let response =
            service.handle_request("POST /event HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}");
        assert_eq!(response.status, "400 Bad Request");
    }

    #[test]
    fn browser_server_serves_one_tcp_request() {
        let server = SchematicBrowserServer::bind(document(), "127.0.0.1:0").unwrap();
        let address = server.local_address();
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .write_all(b"GET /svg HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("<svg"));
        server.stop().unwrap();
    }
}
