//! Shared JSON/value helpers for slang AST lowering and summaries.

use crate::summary::{SlangParameterSummary, SlangSourceSpan};
use crate::types::simple_type_width;
use serde_json::Value;

pub(super) fn string_field(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_string)
}

pub(super) fn normalize_instance_bodies(value: &mut Value) {
    let mut bodies = std::collections::BTreeMap::new();
    collect_instance_bodies(value, &mut bodies);
    replace_instance_body_references(value, &bodies);
}

pub(super) fn normalize_detailed_types(value: &mut Value) {
    if let Value::Array(values) = value {
        for nested in values {
            normalize_detailed_types(nested);
        }
        return;
    }
    let Value::Object(fields) = value else {
        return;
    };
    for (key, nested) in fields.iter_mut() {
        if key == "target" {
            continue;
        }
        if (key == "type" || key == "returnType") && nested.is_object() {
            if let Some(type_name) = canonical_detailed_type(nested) {
                *nested = Value::String(type_name);
            }
            continue;
        }
        normalize_detailed_types(nested);
    }
}

fn canonical_detailed_type(value: &Value) -> Option<String> {
    let kind = value.get("kind").and_then(Value::as_str)?;
    match kind {
        "ScalarType" => {
            let name = value.get("name").and_then(Value::as_str)?;
            if value.get("isSigned").and_then(Value::as_bool) == Some(true) {
                Some(format!("{name} signed"))
            } else {
                Some(name.to_string())
            }
        }
        "PackedArrayType" => {
            let element = value.get("elementType")?;
            let range = value.get("range").and_then(Value::as_str)?;
            let element = canonical_detailed_type(element)?;
            let split = element.find('[').unwrap_or(element.len());
            Some(format!("{}{range}{}", &element[..split], &element[split..]))
        }
        "PredefinedIntegerType" | "FloatingType" | "StringType" | "EventType" => value
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string),
        "FixedSizeUnpackedArrayType" => {
            let element = value.get("elementType")?;
            let range = value.get("range").and_then(Value::as_str)?;
            let element = if let Some(element_name) = element.as_str() {
                element_name.to_string()
            } else {
                canonical_detailed_type(element)?
            };
            if element.contains('$') {
                let split = element.find('$').unwrap_or(element.len());
                Some(format!(
                    "{}{range}{}",
                    &element[..=split],
                    &element[split + 1..]
                ))
            } else {
                Some(format!("{element}${range}"))
            }
        }
        _ => None,
    }
}

fn collect_instance_bodies(value: &Value, bodies: &mut std::collections::BTreeMap<String, Value>) {
    match value {
        Value::Object(fields) => {
            if fields.get("kind").and_then(Value::as_str) == Some("Instance") {
                if let Some(body) = fields.get("body") {
                    if body.get("kind").and_then(Value::as_str) == Some("InstanceBody") {
                        if let (Some(address), Some(name)) =
                            (body.get("addr"), body.get("name").and_then(Value::as_str))
                        {
                            bodies.insert(format!("{address} {name}"), body.clone());
                        }
                    }
                }
            }
            for nested in fields.values() {
                collect_instance_bodies(nested, bodies);
            }
        }
        Value::Array(values) => {
            for nested in values {
                collect_instance_bodies(nested, bodies);
            }
        }
        _ => {}
    }
}

fn replace_instance_body_references(
    value: &mut Value,
    bodies: &std::collections::BTreeMap<String, Value>,
) {
    match value {
        Value::Object(fields) => {
            if fields.get("kind").and_then(Value::as_str) == Some("Instance") {
                let reference = fields
                    .get("body")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let Some(reference) = reference {
                    if let Some(body) = bodies.get(reference.as_str()) {
                        fields.insert("body".to_string(), body.clone());
                    }
                }
            }
            for nested in fields.values_mut() {
                replace_instance_body_references(nested, bodies);
            }
        }
        Value::Array(values) => {
            for nested in values {
                replace_instance_body_references(nested, bodies);
            }
        }
        _ => {}
    }
}

pub(super) fn bool_field(value: &Value, field: &str) -> bool {
    value.get(field).and_then(Value::as_bool).unwrap_or(false)
}

pub(super) fn usize_field(value: &Value, field: &str) -> Option<usize> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

pub(super) fn source_span(value: &Value) -> Option<SlangSourceSpan> {
    let (file, start_line, start_column, end_line, end_column) =
        if let (Some(file), Some(start_line), Some(start_column)) = (
            string_field(value, "source_file_start"),
            usize_field(value, "source_line_start"),
            usize_field(value, "source_column_start"),
        ) {
            (
                file,
                start_line,
                start_column,
                usize_field(value, "source_line_end").unwrap_or(start_line),
                usize_field(value, "source_column_end").unwrap_or(start_column),
            )
        } else {
            let file = string_field(value, "source_file")?;
            let start_line = usize_field(value, "source_line")?;
            let start_column = usize_field(value, "source_column")?;
            (file, start_line, start_column, start_line, start_column)
        };
    Some(SlangSourceSpan {
        file,
        start_line,
        start_column,
        end_line,
        end_column,
    })
}

pub(super) fn with_source_span(value: &Value, error: String) -> String {
    if error.contains(" [slang at ") {
        return error;
    }
    let Some(span) = source_span(value) else {
        return error;
    };
    format!(
        "{error} [slang at {}:{}:{}-{}:{}]",
        span.file, span.start_line, span.start_column, span.end_line, span.end_column
    )
}

pub(super) fn parameter_summary(member: &Value) -> Result<SlangParameterSummary, String> {
    let name = string_field(member, "name")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "slang AST Parameter has no name".to_string())?;
    Ok(SlangParameterSummary {
        name,
        type_name: string_field(member, "type").unwrap_or_default(),
        value: string_field(member, "value").unwrap_or_default(),
        is_local: bool_field(member, "isLocal"),
        is_port: bool_field(member, "isPort"),
    })
}

pub(super) fn instance_parameters(body: &Value) -> Result<Vec<SlangParameterSummary>, String> {
    let mut parameters = Vec::new();
    if let Some(members) = body.get("members").and_then(Value::as_array) {
        for member in members {
            if string_field(member, "kind").as_deref() == Some("Parameter") {
                parameters.push(parameter_summary(member)?);
            }
        }
    }
    Ok(parameters)
}

pub(super) fn expression_target_names(value: &Value) -> Vec<String> {
    if string_field(value, "kind").as_deref() == Some("NamedValue") {
        let symbol = string_field(value, "symbol")
            .or_else(|| string_field(value, "name"))
            .unwrap_or_default();
        let name = symbol.split_whitespace().next_back().unwrap_or_default();
        if name.is_empty() {
            Vec::new()
        } else {
            vec![name.to_string()]
        }
    } else if matches!(
        string_field(value, "kind").as_deref(),
        Some("ElementSelect" | "RangeSelect")
    ) {
        value
            .get("value")
            .map(expression_target_names)
            .unwrap_or_default()
    } else {
        value
            .as_object()
            .map(|fields| {
                fields
                    .values()
                    .flat_map(expression_target_names)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }
}

pub(super) fn literal_word(type_name: &str, raw: &str) -> Option<eda_core::logic::LogicWord> {
    let width = simple_type_width(type_name)?;
    if let Some(separator) = raw.find('\'') {
        let reported_width = raw[..separator].parse::<usize>().ok()?;
        if reported_width != width {
            return None;
        }
        let mut literal = &raw[separator + 1..];
        if let Some(unsigned_body) = literal
            .strip_prefix('s')
            .or_else(|| literal.strip_prefix('S'))
        {
            literal = unsigned_body;
        }
        return eda_core::logic::LogicWord::parse(width, &format!("'{literal}"));
    }
    let digits = raw.replace('_', "");
    let signed_value = digits.parse::<i128>().ok()?;
    let value = u64::try_from(signed_value).unwrap_or(signed_value as u128 as u64);
    Some(eda_core::logic::LogicWord::from_u64(width, value))
}

pub(super) fn constant_json_word(value: &serde_json::Value) -> Option<eda_core::logic::LogicWord> {
    let type_name = string_field(value, "type")?;
    let raw = string_field(value, "constant")?;
    literal_word(&type_name, &raw)
}

pub(super) fn slang_constant_u64(value: &serde_json::Value) -> Result<u64, String> {
    integer_literal_word(value)
        .or_else(|| named_constant_word(value))
        .or_else(|| constant_json_word(value))
        .and_then(|word| word.to_u64())
        .ok_or_else(|| "memory-file start/finish bounds must be integer constants".to_string())
}

pub(super) fn memory_initializer_statements(
    memory: usize,
    dimensions: &[eda_core::ir::MemoryDimension],
    pattern: &serde_json::Value,
) -> Result<Vec<eda_core::ir::Stmt>, String> {
    fn visit(
        memory: usize,
        dimensions: &[eda_core::ir::MemoryDimension],
        pattern: &serde_json::Value,
        indexes: &mut Vec<i64>,
        statements: &mut Vec<eda_core::ir::Stmt>,
    ) -> Result<(), String> {
        if indexes.len() == dimensions.len() {
            let word = constant_json_word(pattern).ok_or_else(|| {
                "sparse-memory initializer elements must be integer constants".to_string()
            })?;
            statements.push(eda_core::ir::Stmt::MemoryBl {
                memory,
                indexes: indexes
                    .iter()
                    .map(|index| {
                        eda_core::ir::Expr::Const(eda_core::logic::LogicWord::from_u64(
                            32,
                            *index as u64,
                        ))
                    })
                    .collect(),
                e: eda_core::ir::Expr::Const(word),
            });
            return Ok(());
        }
        if string_field(pattern, "kind").as_deref() != Some("SimpleAssignmentPattern") {
            return Err(
                "direct slang-AST lowering supports only simple sparse-memory initializers".into(),
            );
        }
        let elements = pattern
            .get("elements")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "sparse-memory initializer has no elements".to_string())?;
        let dimension = dimensions[indexes.len()];
        if elements.len() != dimension.length {
            return Err(format!(
                "sparse-memory initializer dimension has {} elements, expected {}",
                elements.len(),
                dimension.length
            ));
        }
        for (position, element) in elements.iter().enumerate() {
            let logical_index = dimension.base + dimension.step * position as i64;
            if logical_index < 0 {
                return Err(
                    "negative sparse-memory initializer indexes are not yet lowered".into(),
                );
            }
            indexes.push(logical_index);
            visit(memory, dimensions, element, indexes, statements)?;
            indexes.pop();
        }
        Ok(())
    }

    let mut statements = Vec::new();
    let mut indexes = Vec::new();
    visit(memory, dimensions, pattern, &mut indexes, &mut statements)?;
    Ok(statements)
}

pub(super) fn resolve_memory_file_path(
    raw_path: &str,
    statement: &serde_json::Value,
) -> std::path::PathBuf {
    let raw_path = std::path::PathBuf::from(raw_path);
    if raw_path.is_absolute() {
        return raw_path;
    }
    let source_file = source_span(statement)
        .map(|span| std::path::PathBuf::from(span.file))
        .unwrap_or_default();
    let source_relative = source_file
        .parent()
        .map(|parent| parent.join(&raw_path))
        .unwrap_or_else(|| raw_path.clone());
    if source_relative.is_file() {
        source_relative
    } else {
        raw_path
    }
}

pub(super) fn resolve_memory_output_path(
    raw_path: &str,
    statement: &serde_json::Value,
) -> std::path::PathBuf {
    let raw_path = std::path::PathBuf::from(raw_path);
    let source_file = source_span(statement)
        .map(|span| std::path::PathBuf::from(span.file))
        .unwrap_or_default();
    let resolved = if raw_path.is_absolute() {
        raw_path
    } else {
        source_file
            .parent()
            .map(|parent| parent.join(&raw_path))
            .unwrap_or(raw_path)
    };
    let Some(parent) = resolved.parent() else {
        return resolved;
    };
    let Ok(canonical_parent) = parent.canonicalize() else {
        return resolved;
    };
    match resolved.file_name() {
        Some(file_name) => canonical_parent.join(file_name),
        None => resolved,
    }
}

pub(super) fn integer_literal_word(
    value: &serde_json::Value,
) -> Option<eda_core::logic::LogicWord> {
    if string_field(value, "kind").as_deref() != Some("IntegerLiteral") {
        return None;
    }
    literal_word(
        &string_field(value, "type")?,
        &string_field(value, "value")?,
    )
}

pub(super) fn named_constant_word(value: &serde_json::Value) -> Option<eda_core::logic::LogicWord> {
    if string_field(value, "kind").as_deref() != Some("NamedValue") {
        return None;
    }
    literal_word(
        &string_field(value, "type")?,
        &string_field(value, "constant")?,
    )
}

pub(super) fn expression_symbol(value: &Value) -> Option<String> {
    string_field(value, "symbol")
        .as_deref()
        .and_then(|symbol| symbol.split_whitespace().next_back())
        .filter(|symbol| !symbol.is_empty())
        .map(str::to_string)
}

pub(super) fn expression_symbol_id(value: &Value) -> Option<String> {
    string_field(value, "symbol").filter(|symbol| !symbol.trim().is_empty())
}
