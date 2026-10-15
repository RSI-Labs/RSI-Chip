//! Shared token/property/number helpers.

use std::collections::BTreeMap;

pub(super) fn parse_properties(tokens: &[String]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut i = 0;
    while i < tokens.len() {
        let lower = tokens[i].to_ascii_lowercase();
        if let Some((key, value)) = lower.split_once('=') {
            out.insert(key.to_string(), value.to_string());
            i += 1;
        } else if matches!(lower.as_str(), "ic" | "temp") && i + 1 < tokens.len() {
            out.insert(lower, tokens[i + 1].clone());
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

pub(super) fn split_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut paren_depth = 0usize;
    for ch in text.chars() {
        match ch {
            '(' => {
                paren_depth += 1;
                current.push(ch);
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                current.push(ch);
            }
            c if c.is_whitespace() && paren_depth == 0 => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

pub fn parse_number(text: &str) -> Result<f64, String> {
    let cleaned: String = text
        .trim()
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '_')
        .collect();
    if cleaned.is_empty() {
        return Err(format!("invalid number '{}'", text));
    }
    let lower = cleaned.to_ascii_lowercase();
    let suffixes = [
        ("meg", 1e6),
        ("mil", 25.4e-6),
        ("t", 1e12),
        ("g", 1e9),
        ("k", 1e3),
        ("m", 1e-3),
        ("u", 1e-6),
        ("n", 1e-9),
        ("p", 1e-12),
        ("f", 1e-15),
    ];
    for (suffix, scale) in suffixes {
        if let Some(prefix) = lower.strip_suffix(suffix) {
            let mantissa: f64 = prefix
                .parse()
                .map_err(|_| format!("invalid number '{}'", text))?;
            return Ok(mantissa * scale);
        }
    }
    cleaned
        .parse()
        .map_err(|_| format!("invalid number '{}'", text))
}

pub(super) fn expect_arg_count(
    got: usize,
    expected: usize,
    line_no: usize,
    name: &str,
) -> Result<(), String> {
    if got == expected {
        Ok(())
    } else {
        Err(format!(
            "line {}: '{}' expects {} arguments, found {}",
            line_no, name, expected, got
        ))
    }
}
