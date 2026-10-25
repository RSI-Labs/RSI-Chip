//! Canonicalization for supported aggregate assignment patterns.

use super::{lookup_type_layout, SlangTypeLayoutSummary};
use crate::ast_util::{slang_constant_u64, string_field};
use crate::types::memory_type;
use serde_json::Value;

pub(super) fn canonicalize_packed_struct_assignment_patterns(
    value: &mut Value,
    type_layouts: &std::collections::BTreeMap<String, SlangTypeLayoutSummary>,
) {
    if let Some(concatenation) = packed_struct_assignment_pattern(value, type_layouts) {
        *value = concatenation;
    }
    match value {
        Value::Array(values) => {
            for nested in values {
                canonicalize_packed_struct_assignment_patterns(nested, type_layouts);
            }
        }
        Value::Object(fields) => {
            for nested in fields.values_mut() {
                canonicalize_packed_struct_assignment_patterns(nested, type_layouts);
            }
        }
        _ => {}
    }
}

pub(super) fn canonicalize_unpacked_assignment_patterns(value: &mut Value) {
    if let Some(statements) = unpacked_assignment_pattern_statement(value) {
        *value = serde_json::json!({
            "kind": "List",
            "list": statements,
        });
    }
    match value {
        Value::Array(values) => {
            for nested in values {
                canonicalize_unpacked_assignment_patterns(nested);
            }
        }
        Value::Object(fields) => {
            for nested in fields.values_mut() {
                canonicalize_unpacked_assignment_patterns(nested);
            }
        }
        _ => {}
    }
}

fn unpacked_assignment_pattern_statement(value: &Value) -> Option<Vec<Value>> {
    if string_field(value, "kind").as_deref() != Some("ExpressionStatement") {
        return None;
    }
    let assignment = value.get("expr")?;
    if string_field(assignment, "kind").as_deref() != Some("Assignment") {
        return None;
    }
    let left = assignment.get("left")?;
    if !matches!(
        string_field(left, "kind").as_deref(),
        Some("NamedValue" | "HierarchicalValue")
    ) {
        return None;
    }
    let right = assignment.get("right")?;
    if !matches!(
        string_field(right, "kind").as_deref(),
        Some("SimpleAssignmentPattern" | "StructuredAssignmentPattern")
    ) {
        return None;
    }
    let memory = memory_type(&string_field(left, "type")?)?;
    if memory.dynamic || memory.queue || memory.associative {
        return None;
    }
    let nonblocking = assignment
        .get("isNonBlocking")
        .and_then(Value::as_bool)
        .unwrap_or_default();
    let element_type = string_field(left, "type")?
        .split('$')
        .next()?
        .trim()
        .to_string();
    let mut statements = Vec::new();
    if string_field(right, "kind").as_deref() == Some("StructuredAssignmentPattern") {
        let [dimension] = memory.dimensions.as_slice() else {
            return None;
        };
        append_structured_pattern_elements(
            left,
            right,
            dimension,
            &element_type,
            nonblocking,
            &mut statements,
        )?;
    } else {
        append_simple_pattern_elements(
            left,
            right,
            &memory.dimensions,
            0,
            Vec::new(),
            &element_type,
            nonblocking,
            &mut statements,
        )?;
    }
    Some(statements)
}

#[allow(clippy::too_many_arguments)]
fn append_simple_pattern_elements(
    left: &Value,
    pattern: &Value,
    dimensions: &[eda_core::ir::MemoryDimension],
    level: usize,
    indexes: Vec<i64>,
    element_type: &str,
    nonblocking: bool,
    statements: &mut Vec<Value>,
) -> Option<()> {
    let Some(dimension) = dimensions.get(level) else {
        let target = selected_memory_element(left, &indexes, element_type);
        statements.push(pattern_element_assignment(target, pattern, nonblocking));
        return Some(());
    };
    let elements = pattern.get("elements")?.as_array()?;
    if elements.len() != dimension.length {
        return None;
    }
    for (position, element) in elements.iter().enumerate() {
        let logical_index = dimension.base + dimension.step * position as i64;
        let mut indexes = indexes.clone();
        indexes.push(logical_index);
        append_simple_pattern_elements(
            left,
            element,
            dimensions,
            level + 1,
            indexes,
            element_type,
            nonblocking,
            statements,
        )?;
    }
    Some(())
}

fn append_structured_pattern_elements(
    left: &Value,
    pattern: &Value,
    dimension: &eda_core::ir::MemoryDimension,
    element_type: &str,
    nonblocking: bool,
    statements: &mut Vec<Value>,
) -> Option<()> {
    let default = pattern.get("defaultSetter")?;
    let setters = pattern.get("indexSetters")?.as_array()?;
    let mut values_by_index = std::collections::BTreeMap::new();
    for setter in setters {
        let index = slang_constant_u64(setter.get("index")?).ok()?;
        let expression = setter.get("expr")?;
        if values_by_index.insert(index, expression).is_some() {
            return None;
        }
    }
    for position in 0..dimension.length {
        let logical_index = dimension.base + dimension.step * position as i64;
        let unsigned_index = u64::try_from(logical_index).ok()?;
        let expression = values_by_index
            .get(&unsigned_index)
            .copied()
            .unwrap_or(default);
        let target = selected_memory_element(left, &[logical_index], element_type);
        statements.push(pattern_element_assignment(target, expression, nonblocking));
    }
    Some(())
}

fn selected_memory_element(left: &Value, indexes: &[i64], element_type: &str) -> Value {
    let mut selected = left.clone();
    for logical_index in indexes {
        selected = serde_json::json!({
            "kind": "ElementSelect",
            "type": element_type,
            "value": selected,
            "selector": {
                "kind": "IntegerLiteral",
                "type": "int",
                "value": logical_index.to_string(),
            },
        });
    }
    selected
}

fn pattern_element_assignment(target: Value, expression: &Value, nonblocking: bool) -> Value {
    let assignment = serde_json::json!({
        "kind": "Assignment",
        "type": string_field(expression, "type").unwrap_or_default(),
        "left": target,
        "right": expression,
        "isNonBlocking": nonblocking,
    });
    serde_json::json!({
        "kind": "ExpressionStatement",
        "expr": assignment,
    })
}

fn packed_struct_assignment_pattern(
    value: &Value,
    type_layouts: &std::collections::BTreeMap<String, SlangTypeLayoutSummary>,
) -> Option<Value> {
    if !matches!(
        string_field(value, "kind").as_deref(),
        Some("SimpleAssignmentPattern" | "StructuredAssignmentPattern")
    ) {
        return None;
    }
    let type_name = string_field(value, "type")?;
    let layout = lookup_type_layout(&type_name, type_layouts)?;
    let mut fields = layout.fields.clone();
    fields.sort_by(|left, right| right.offset.cmp(&left.offset));
    if fields.is_empty() {
        return None;
    }
    let mut expected_lsb = layout.element_width;
    for field in &fields {
        if field.offset + field.width != expected_lsb {
            return None;
        }
        expected_lsb = field.offset;
    }
    if expected_lsb != 0 {
        return None;
    }

    let elements = match string_field(value, "kind").as_deref() {
        Some("SimpleAssignmentPattern") => {
            let elements = value.get("elements")?.as_array()?;
            if elements.len() != fields.len() {
                return None;
            }
            fields.iter().cloned().zip(elements).collect::<Vec<_>>()
        }
        Some("StructuredAssignmentPattern") => {
            let setters = value.get("memberSetters")?.as_array()?;
            let mut elements_by_name = std::collections::BTreeMap::new();
            for setter in setters {
                let member = string_field(setter, "member")?
                    .split_whitespace()
                    .next_back()?
                    .to_string();
                let expression = setter.get("expr")?;
                if elements_by_name.insert(member, expression).is_some() {
                    return None;
                }
            }
            let mut elements = Vec::with_capacity(fields.len());
            for field in &fields {
                elements.push((field.clone(), elements_by_name.get(&field.name).cloned()?));
            }
            elements
        }
        _ => return None,
    };

    Some(serde_json::json!({
        "kind": "Concatenation",
        "type": format!("logic[{}:0]", layout.element_width - 1),
        "operands": elements.into_iter().map(|(_, expression)| expression).collect::<Vec<_>>(),
    }))
}
