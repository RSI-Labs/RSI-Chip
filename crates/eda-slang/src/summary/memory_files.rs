//! Canonicalization for compile-time `$readmemh`/`$readmemb` loads.

use crate::ast_util::{
    resolve_memory_file_path, slang_constant_u64, string_field, with_source_span,
};
use crate::types::{memory_type, simple_type_width, AstMemoryType};
use eda_core::memory_file::{filter_memory_file_finish, read_memory_file};
use serde_json::Value;

pub(super) fn canonicalize_memory_file_reads(value: &mut Value) -> Result<(), String> {
    if string_field(value, "kind").as_deref() == Some("ExpressionStatement") {
        if let Some(call) = value.get("expr") {
            if matches!(
                string_field(call, "subroutine").as_deref(),
                Some("$readmemh" | "$readmemb")
            ) {
                let statements = memory_file_statements(call, value)?;
                *value = serde_json::json!({
                    "kind": "List",
                    "list": statements,
                });
                return Ok(());
            }
        }
    }
    match value {
        Value::Array(values) => {
            for nested in values {
                canonicalize_memory_file_reads(nested)?;
            }
        }
        Value::Object(fields) => {
            for nested in fields.values_mut() {
                canonicalize_memory_file_reads(nested)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn memory_file_statements(call: &Value, statement: &Value) -> Result<Vec<Value>, String> {
    let subroutine = string_field(call, "subroutine").unwrap_or_default();
    let radix = if subroutine == "$readmemh" { 16 } else { 2 };
    let arguments = call
        .get("arguments")
        .and_then(Value::as_array)
        .ok_or_else(|| "memory-file call has no arguments".to_string())?;
    if arguments.len() < 2 || arguments.len() > 4 {
        return Err(format!(
            "memory-file call supports two to four arguments, found {}",
            arguments.len()
        ))
        .map_err(|error| with_source_span(statement, error));
    }
    let raw_path = string_field(&arguments[0], "literal")
        .ok_or_else(|| "memory-file call path must be a string literal".to_string())?;
    let target = arguments[1]
        .get("left")
        .ok_or_else(|| "memory-file call has no target".to_string())?;
    let target_type = string_field(target, "type")
        .ok_or_else(|| "memory-file call target has no elaborated type".to_string())?;
    let memory = memory_type(&target_type)
        .ok_or_else(|| "memory-file call target is not a fixed memory".to_string())?;
    if memory.dynamic || memory.queue || memory.associative {
        return Err("memory-file loads support fixed unpacked arrays only".into());
    }
    let start = arguments
        .get(2)
        .map(slang_constant_u64)
        .transpose()
        .map_err(|error| with_source_span(statement, error))?;
    let finish = arguments
        .get(3)
        .map(slang_constant_u64)
        .transpose()
        .map_err(|error| with_source_span(statement, error))?;
    let path = resolve_memory_file_path(&raw_path, statement);
    let values = read_memory_file(&path, radix, memory.element_width, start)
        .map_err(|error| with_source_span(statement, error))?;
    let values = filter_memory_file_finish(values, start, finish)
        .map_err(|error| with_source_span(statement, error))?;

    let element_type = target_type.split('$').next().unwrap_or_default().trim();
    let mut statements = Vec::with_capacity(values.len());
    for (physical_index, word) in values {
        let indexes = logical_indexes(&memory, physical_index)
            .ok_or_else(|| "memory-file address is out of range".to_string())
            .map_err(|error| with_source_span(statement, error))?;
        statements.push(memory_element_assignment(
            target,
            element_type,
            &indexes,
            &word.to_bin_str(),
        ));
    }
    Ok(statements)
}

fn logical_indexes(memory: &AstMemoryType, physical_index: u64) -> Option<Vec<i64>> {
    if physical_index >= memory.flat_length {
        return None;
    }
    let mut remaining = physical_index;
    let mut stride = memory.flat_length;
    let mut indexes = Vec::with_capacity(memory.dimensions.len());
    for dimension in &memory.dimensions {
        let dimension_length = u64::try_from(dimension.length).ok()?;
        stride = stride.checked_div(dimension_length)?;
        let offset = remaining / stride % dimension_length;
        remaining %= stride;
        indexes.push(dimension.base + dimension.step * i64::try_from(offset).ok()?);
    }
    Some(indexes)
}

fn memory_element_assignment(
    target: &Value,
    element_type: &str,
    indexes: &[i64],
    bits: &str,
) -> Value {
    let mut left = target.clone();
    for index in indexes {
        left = serde_json::json!({
            "kind": "ElementSelect",
            "type": element_type,
            "value": left,
            "selector": {
                "kind": "IntegerLiteral",
                "type": "int",
                "value": index.to_string(),
            },
        });
    }
    let width = simple_type_width(element_type).unwrap_or_default();
    let assignment = serde_json::json!({
        "kind": "Assignment",
        "type": element_type,
        "left": left,
        "right": {
            "kind": "IntegerLiteral",
            "type": element_type,
            "value": format!("{width}'b{bits}"),
        },
        "isNonBlocking": false,
    });
    serde_json::json!({
        "kind": "ExpressionStatement",
        "expr": assignment,
    })
}
