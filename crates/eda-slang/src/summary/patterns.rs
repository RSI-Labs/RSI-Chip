//! Canonicalization for supported packed aggregate assignment patterns.

use super::{lookup_type_layout, SlangTypeLayoutSummary};
use crate::ast_util::string_field;
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
