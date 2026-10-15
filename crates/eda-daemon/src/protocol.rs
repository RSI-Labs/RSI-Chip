//! Dependency-free JSON and RPC request/response wire types.

use std::collections::BTreeMap;
use std::fmt::Write as _;

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
