//! Expression summaries, fingerprints, and IR-shape comparison helpers.

use crate::ast_util::{
    bool_field, expression_symbol, expression_symbol_id, expression_target_names,
    instance_parameters, integer_literal_word, literal_word, named_constant_word,
    parameter_summary, slang_constant_u64, string_field,
};
use crate::types::{
    associative_memory_type, dynamic_memory_type, memory_type, memory_type_with_element,
    queue_memory_type, simple_type_shape, simple_type_signed, simple_type_width,
    string_literal_word, AstPackedDimension, STRING_WIDTH,
};
use eda_core::ir::Design;
use eda_core::logic::LogicWord;
use serde_json::Value;

use super::{
    SlangAssignmentSummary, SlangContinuousTarget, SlangEnumTypeSummary, SlangExpressionSummary,
    SlangInstanceSummary, SlangMemoryMemberLvalueSummary, SlangParameterSummary,
    SlangProceduralBlockSummary,
};

pub(crate) fn expression_summary(value: &Value) -> SlangExpressionSummary {
    let kind = string_field(value, "kind").unwrap_or_default();
    let operator = string_field(value, "op").or_else(|| {
        (kind == "Call")
            .then(|| string_field(value, "subroutine"))
            .flatten()
    });
    let literal = expression_literal(value);
    let member_path = if kind == "MemberAccess" {
        value
            .get("member")
            .and_then(|member| member.as_str())
            .map(|member| {
                member
                    .split_whitespace()
                    .next_back()
                    .map(ToString::to_string)
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let streaming_slice_size = if kind == "Streaming" {
        value
            .get("sliceSize")
            .and_then(Value::as_u64)
            .and_then(|size| usize::try_from(size).ok())
    } else {
        None
    };
    // The bounded IR folds parameter constants to unsigned 32-bit Const nodes.
    // Keep those leaves comparable without inventing a signed Constant type.
    let type_signed = if kind == "NamedValue" && literal.is_some() {
        Some(false)
    } else {
        string_field(value, "type").and_then(|type_name| simple_type_signed(&type_name))
    };
    SlangExpressionSummary {
        kind,
        type_name: string_field(value, "type").unwrap_or_default(),
        operator: operator.unwrap_or_default(),
        selection_kind: string_field(value, "selectionKind").unwrap_or_default(),
        type_width: string_field(value, "type").and_then(|type_name| simple_type_width(&type_name)),
        type_signed,
        symbol: expression_symbol(value),
        symbol_id: expression_symbol_id(value),
        literal,
        repeat_count: repeat_count(value),
        simulation_time_unit: None,
        enum_definition: None,
        member_path,
        member_range: None,
        interface_path: None,
        streaming_slice_size,
        operands: expression_operands(value),
    }
}

pub(crate) fn empty_expression_summary() -> SlangExpressionSummary {
    SlangExpressionSummary {
        kind: String::new(),
        type_name: String::new(),
        operator: String::new(),
        selection_kind: String::new(),
        type_width: None,
        type_signed: None,
        symbol: None,
        symbol_id: None,
        literal: None,
        repeat_count: None,
        simulation_time_unit: None,
        enum_definition: None,
        member_path: Vec::new(),
        member_range: None,
        interface_path: None,
        streaming_slice_size: None,
        operands: Vec::new(),
    }
}

pub(crate) fn expression_literal(value: &Value) -> Option<String> {
    let kind = string_field(value, "kind")?;
    let raw = match kind.as_str() {
        "IntegerLiteral" => string_field(value, "value")?,
        "NamedValue" => string_field(value, "constant")?,
        "StringLiteral" => string_field(value, "literal")?,
        _ => return None,
    };
    let type_name = string_field(value, "type")?;
    let word = if kind == "StringLiteral" {
        let width = simple_type_width(&type_name).unwrap_or(STRING_WIDTH);
        LogicWord::from_bytes_le(width, raw.as_bytes())
    } else {
        literal_word(&type_name, &raw)?
    };
    let width = word.width();
    Some(format!("{width}:{}", word.to_bin_str()))
}

pub(crate) fn expression_operands(value: &Value) -> Vec<SlangExpressionSummary> {
    let kind = string_field(value, "kind").unwrap_or_default();
    let mut operands = Vec::new();
    match kind.as_str() {
        "UnaryOp" => {
            if let Some(operand) = value.get("operand") {
                operands.push(expression_summary(operand));
            }
        }
        "BinaryOp" => {
            for field in ["left", "right"] {
                if let Some(operand) = value.get(field) {
                    operands.push(expression_summary(operand));
                }
            }
        }
        "Conversion" => {
            if let Some(operand) = value.get("operand") {
                operands.push(expression_summary(operand));
            }
        }
        "NewArray" => {
            if let Some(size) = value.get("sizeExpr") {
                operands.push(expression_summary(size));
            }
        }
        "Concatenation" => {
            if let Some(values) = value.get("operands").and_then(Value::as_array) {
                operands.extend(values.iter().map(expression_summary));
            }
        }
        "Replication" => {
            if let Some(concat) = value.get("concat") {
                operands.push(expression_summary(concat));
            }
        }
        "ConditionalOp" => {
            if let Some(conditions) = value.get("conditions").and_then(Value::as_array) {
                operands.extend(
                    conditions
                        .iter()
                        .filter_map(|condition| condition.get("expr"))
                        .map(expression_summary),
                );
            }
            for field in ["left", "right"] {
                if let Some(operand) = value.get(field) {
                    operands.push(expression_summary(operand));
                }
            }
        }
        "ElementSelect" => {
            if let Some(selected) = value.get("value") {
                operands.push(expression_summary(selected));
            }
            if let Some(selector) = value.get("selector") {
                operands.push(expression_summary(selector));
            }
        }
        "RangeSelect" => {
            if let Some(selected) = value.get("value") {
                operands.push(expression_summary(selected));
            }
            for field in ["left", "right"] {
                if let Some(bound) = value.get(field) {
                    operands.push(expression_summary(bound));
                }
            }
        }
        "ValueRange" => {
            for field in ["left", "right"] {
                if let Some(bound) = value.get(field) {
                    operands.push(expression_summary(bound));
                }
            }
        }
        "Inside" => {
            if let Some(left) = value.get("left") {
                operands.push(expression_summary(left));
            }
            if let Some(ranges) = value.get("rangeList").and_then(Value::as_array) {
                operands.extend(ranges.iter().map(expression_summary));
            }
        }
        "MemberAccess" => {
            if let Some(selected) = value.get("value") {
                operands.push(expression_summary(selected));
            }
        }
        "Call" => {
            if let Some(arguments) = value.get("arguments").and_then(Value::as_array) {
                operands.extend(arguments.iter().map(expression_summary));
            }
        }
        "Streaming" => {
            if let Some(streams) = value.get("streams").and_then(Value::as_array) {
                operands.extend(
                    streams
                        .iter()
                        .filter_map(|stream| stream.get("operand"))
                        .map(expression_summary),
                );
            }
        }
        _ => {}
    }
    operands
}

fn repeat_count(value: &Value) -> Option<usize> {
    let count = value.get("count")?;
    integer_literal_word(count)
        .or_else(|| named_constant_word(count))
        .and_then(|word| word.to_u64())
        .and_then(|count| usize::try_from(count).ok())
}

fn is_dynamic_packed_lvalue(value: &Value) -> bool {
    match string_field(value, "kind").as_deref() {
        Some("ElementSelect") => {
            let selected_is_memory = value
                .get("value")
                .and_then(|selected| string_field(selected, "type"))
                .is_some_and(|type_name| type_name.contains('$'));
            !selected_is_memory
                && value.get("selector").is_some_and(|selector| {
                    integer_literal_word(selector).is_none()
                        && string_field(selector, "constant").is_none()
                })
        }
        Some("MemberAccess") => value
            .get("value")
            .map(is_dynamic_packed_lvalue)
            .unwrap_or(false),
        _ => false,
    }
}

fn memory_member_lvalue_summary(value: &Value) -> Option<SlangMemoryMemberLvalueSummary> {
    if string_field(value, "kind").as_deref() != Some("MemberAccess") {
        return None;
    }
    let mut members = Vec::new();
    let mut member_access = value;
    while string_field(member_access, "kind").as_deref() == Some("MemberAccess") {
        members.push(
            string_field(member_access, "member")?
                .split_whitespace()
                .next_back()?
                .to_string(),
        );
        member_access = member_access.get("value")?;
    }
    let mut selectors = Vec::new();
    let mut base = member_access;
    while string_field(base, "kind").as_deref() == Some("ElementSelect") {
        selectors.push(
            base.get("selector")
                .map(expression_summary)
                .unwrap_or_else(empty_expression_summary),
        );
        base = base.get("value")?;
    }
    if !string_field(base, "type").is_some_and(|type_name| type_name.contains('$')) {
        return None;
    }
    selectors.reverse();
    members.reverse();
    Some(SlangMemoryMemberLvalueSummary {
        indexes: selectors,
        members,
    })
}

pub(crate) fn procedural_assignments(
    value: &Value,
    class_handles: &std::collections::BTreeSet<String>,
    class_types: ClassHandleTypes<'_>,
) -> Vec<SlangAssignmentSummary> {
    let mut assignments = Vec::new();
    if string_field(value, "kind").as_deref() == Some("ProceduralAssign")
        && bool_field(value, "isForce")
    {
        return assignments;
    }
    if string_field(value, "kind").as_deref() == Some("RepeatLoop")
        && value
            .get("count")
            .and_then(|count| slang_constant_u64(count).ok())
            .is_some_and(|count| count == 0)
    {
        return assignments;
    }
    if string_field(value, "kind").as_deref() == Some("Assignment") {
        if let (Some(left), Some(right)) = (value.get("left"), value.get("right")) {
            let target_is_named_handle = matches!(
                string_field(left, "kind").as_deref(),
                Some("NamedValue" | "HierarchicalValue")
            ) && is_class_handle_target(left, class_handles);
            let mut class_handle_target_key = None;
            let mut class_handle_source_key = None;
            let mut class_handle_dynamic_sources = Vec::new();
            let class_handle_dynamic_targets =
                dynamic_class_array_source_paths(left, class_types.field_bearing);
            let target_is_dynamic_class_array = !class_handle_dynamic_targets.is_empty();
            let class_handle_source = if target_is_named_handle {
                if let Some(path) = class_handle_source_path(right, class_types.field_bearing) {
                    Some(path)
                } else if let Some((path, source_key)) =
                    named_class_handle_identity(right, class_types.all)
                {
                    class_handle_target_key =
                        string_field(left, "symbol").filter(|symbol| !symbol.trim().is_empty());
                    class_handle_source_key = Some(source_key);
                    Some(path)
                } else {
                    class_handle_dynamic_sources =
                        dynamic_class_array_source_paths(right, class_types.field_bearing);
                    None
                }
            } else {
                None
            };
            let dynamic_class_array_source = if target_is_dynamic_class_array {
                if let Some(path) = class_handle_source_path(right, class_types.field_bearing) {
                    Some(path)
                } else {
                    class_handle_dynamic_sources =
                        dynamic_class_array_source_paths(right, class_types.field_bearing);
                    None
                }
            } else {
                None
            };
            let class_handle_source = dynamic_class_array_source.or(class_handle_source);
            if class_handle_source.is_some()
                || !class_handle_dynamic_sources.is_empty()
                || !is_class_handle_target(left, class_handles)
            {
                let dynamic_packed_lvalue = is_dynamic_packed_lvalue(left);
                let dynamic_packed_lvalue_selector = dynamic_packed_lvalue
                    .then(|| left.get("selector").map(expression_summary))
                    .flatten();
                let memory_member_lvalue = memory_member_lvalue_summary(left);
                let dynamic_array_lvalue_indexes = dynamic_array_lvalue_indexes(left);
                let array_part_lvalue = array_part_lvalue_summary(left);
                assignments.push(SlangAssignmentSummary {
                    target_names: expression_target_names(left),
                    rhs: expression_summary(right),
                    nonblocking: bool_field(value, "isNonBlocking"),
                    dynamic_packed_lvalue,
                    dynamic_packed_lvalue_selector,
                    memory_member_lvalue,
                    constant_array_element_lvalue: constant_array_element_lvalue(left),
                    dynamic_array_lvalue_indexes,
                    array_part_lvalue,
                    class_handle_source,
                    class_handle_target_key,
                    class_handle_source_key,
                    class_handle_dynamic_sources,
                    class_handle_dynamic_targets,
                });
            }
        }
    }
    match value {
        Value::Array(values) => values.iter().for_each(|nested| {
            assignments.extend(procedural_assignments(nested, class_handles, class_types))
        }),
        Value::Object(fields) => {
            let is_assignment = string_field(value, "kind").as_deref() == Some("Assignment");
            fields
                .iter()
                .filter(|(field, _)| {
                    !(is_assignment && (field.as_str() == "left" || field.as_str() == "right"))
                })
                .for_each(|(_, nested)| {
                    assignments.extend(procedural_assignments(nested, class_handles, class_types))
                });
        }
        _ => {}
    }
    assignments
}

#[derive(Clone, Copy)]
pub(crate) struct ClassHandleTypes<'a> {
    pub field_bearing: &'a std::collections::BTreeSet<String>,
    pub all: &'a std::collections::BTreeSet<String>,
}

fn class_handle_copy_block(
    design: &Design,
    statements: &[eda_core::ir::Stmt],
) -> Option<(String, String)> {
    use eda_core::ir::{Expr, Stmt};

    const METADATA_PREFIXES: [&str; 5] = [
        "__class_tag_",
        "__class_object_id_",
        "__class_object_name_",
        "__class_object_full_name_",
        "__class_parent_id_",
    ];
    if statements.len() < METADATA_PREFIXES.len() {
        return None;
    }

    let mut target_metadata_key = String::new();
    let mut source_metadata_key = String::new();
    for (statement, prefix) in statements.iter().zip(METADATA_PREFIXES.iter()) {
        let Stmt::Bl { lv, e } = statement else {
            return None;
        };
        let Expr::Ref { .. } = e else {
            return None;
        };
        let target = design_lvalue_name(design, lv)?;
        let source = design_expr_symbol(design, e)?;
        let statement_target_key = target.strip_prefix(prefix)?.to_string();
        let statement_source_key = source.strip_prefix(prefix)?.to_string();
        if target_metadata_key.is_empty() {
            target_metadata_key = statement_target_key;
        } else if target_metadata_key != statement_target_key {
            return None;
        }
        if source_metadata_key.is_empty() {
            source_metadata_key = statement_source_key;
        } else if source_metadata_key != statement_source_key {
            return None;
        }
    }

    let fields = &statements[METADATA_PREFIXES.len()..];
    let mut target_object = String::new();
    let mut source_object = String::new();
    for statement in fields {
        let Stmt::Bl { lv, e } = statement else {
            return None;
        };
        let Expr::Ref { .. } = e else {
            return None;
        };
        let target_field = design_lvalue_name(design, lv)?;
        let source_field = design_expr_symbol(design, e)?;
        let (statement_target, _) = target_field.rsplit_once('.')?;
        let (statement_source, _) = source_field.rsplit_once('.')?;
        if target_object.is_empty() {
            target_object = statement_target.to_string();
        } else if target_object != statement_target {
            return None;
        }
        if source_object.is_empty() {
            source_object = statement_source.to_string();
        } else if source_object != statement_source {
            return None;
        }
    }
    if target_object.is_empty() || source_object.is_empty() {
        return Some((
            target_metadata_key,
            format!("ClassHandleCopy:{source_metadata_key}"),
        ));
    }
    Some((target_object, format!("ClassHandleCopy:{source_object}")))
}

fn constant_array_element_lvalue(value: &Value) -> bool {
    match string_field(value, "kind").as_deref() {
        Some("ElementSelect") => constant_array_indexes(value).is_some(),
        Some("RangeSelect") => {
            let Some(selected) = value.get("value") else {
                return false;
            };
            constant_array_indexes(selected).is_some()
        }
        _ => false,
    }
}

fn constant_array_indexes(value: &Value) -> Option<Vec<SlangExpressionSummary>> {
    let indexes = array_indexes(value)?;
    indexes
        .iter()
        .all(|index| summary_literal_i64(index).is_some())
        .then_some(indexes)
}

fn array_indexes(value: &Value) -> Option<Vec<SlangExpressionSummary>> {
    if string_field(value, "kind").as_deref() != Some("ElementSelect") {
        return None;
    }
    let selected = value.get("value")?;
    let mut indexes = array_indexes(selected).or_else(|| {
        let selected_is_array = matches!(
            string_field(selected, "kind").as_deref(),
            Some("NamedValue" | "HierarchicalValue")
        ) && string_field(selected, "type")
            .is_some_and(|type_name| type_name.contains('$'));
        selected_is_array.then_some(Vec::new())
    })?;
    let selector = value.get("selector")?;
    indexes.push(expression_summary(selector));
    Some(indexes)
}

fn array_part_lvalue_summary(value: &Value) -> Option<super::SlangArrayPartLvalueSummary> {
    if string_field(value, "kind").as_deref() != Some("RangeSelect") {
        return None;
    }
    let selected = value.get("value")?;
    let indexes = array_indexes(selected)?;
    let selection_kind = string_field(value, "selectionKind")?;
    let left = value.get("left")?;
    let right = value.get("right")?;
    let (start, width, normalized_selection_kind) = match selection_kind.as_str() {
        "IndexedUp" | "IndexedDown" => {
            let width = usize::try_from(slang_constant_u64(right).ok()?).ok()?;
            (left.clone(), width, selection_kind.clone())
        }
        "Simple" => {
            let left_value = slang_constant_u64(left).ok()?;
            let right_value = slang_constant_u64(right).ok()?;
            let width = usize::try_from(left_value.abs_diff(right_value) + 1).ok()?;
            if left_value >= right_value {
                (right.clone(), width, "IndexedUp".to_string())
            } else {
                (left.clone(), width, "IndexedDown".to_string())
            }
        }
        _ => return None,
    };
    Some(super::SlangArrayPartLvalueSummary {
        indexes,
        start: expression_summary(&start),
        width,
        selection_kind: normalized_selection_kind,
    })
}

fn dynamic_array_lvalue_indexes(value: &Value) -> Vec<SlangExpressionSummary> {
    if string_field(value, "kind").as_deref() != Some("ElementSelect") {
        return Vec::new();
    }
    let Some(indexes) = array_indexes(value) else {
        return Vec::new();
    };
    if indexes
        .iter()
        .any(|index| summary_literal_i64(index).is_none())
    {
        indexes
    } else {
        Vec::new()
    }
}

fn class_handle_source_path(
    value: &Value,
    class_types: &std::collections::BTreeSet<String>,
) -> Option<String> {
    if string_field(value, "kind").as_deref() == Some("Conversion") {
        return class_handle_source_path(value.get("operand")?, class_types);
    }
    if matches!(
        string_field(value, "kind").as_deref(),
        Some("NamedValue" | "HierarchicalValue")
    ) {
        let symbol = string_field(value, "symbol")?;
        let name = symbol.split_whitespace().next_back()?.to_string();
        let class_name = canonical_class_name(&string_field(value, "type")?)?;
        return class_types.contains(&class_name).then_some(name);
    }
    if string_field(value, "kind").as_deref() != Some("MemberAccess") {
        return None;
    }

    let mut members = Vec::new();
    let mut selected = value;
    while string_field(selected, "kind").as_deref() == Some("MemberAccess") {
        members.push(
            string_field(selected, "member")?
                .split_whitespace()
                .next_back()?
                .to_string(),
        );
        selected = selected.get("value")?;
    }
    if !matches!(
        string_field(selected, "kind").as_deref(),
        Some("NamedValue" | "HierarchicalValue")
    ) {
        return None;
    }
    let symbol = string_field(selected, "symbol")?;
    let mut path = symbol.split_whitespace().next_back()?.to_string();
    let class_name = canonical_class_name(&string_field(value, "type")?)?;
    if !class_types.contains(&class_name) {
        return None;
    }
    members.reverse();
    for member in members {
        path.push('.');
        path.push_str(&member);
    }
    Some(path)
}

fn dynamic_class_array_source_paths(
    value: &Value,
    class_types: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    let Some(result) = dynamic_class_array_source_path(value, class_types) else {
        return Vec::new();
    };
    result
}

fn dynamic_class_array_source_path(
    value: &Value,
    class_types: &std::collections::BTreeSet<String>,
) -> Option<Vec<String>> {
    if string_field(value, "kind").as_deref() == Some("Conversion") {
        return dynamic_class_array_source_path(value.get("operand")?, class_types);
    }
    if string_field(value, "kind").as_deref() != Some("ElementSelect") {
        return None;
    }
    let selected = value.get("value")?;
    if !matches!(
        string_field(selected, "kind").as_deref(),
        Some("NamedValue" | "HierarchicalValue")
    ) {
        return None;
    }
    let selector = value.get("selector")?;
    if integer_literal_word(selector).is_some() || string_field(selector, "constant").is_some() {
        return None;
    }
    let class_name = canonical_class_name(&string_field(value, "type")?)?;
    if !class_types.contains(&class_name) {
        return None;
    }

    let type_name = string_field(selected, "type")?;
    let range_start = type_name.find("$[")? + 2;
    let range_end = type_name.rfind(']')?;
    if range_end <= range_start {
        return None;
    }
    let range = &type_name[range_start..range_end];
    let (left, right) = range.split_once(':')?;
    let lower = left.trim().parse::<i64>().ok()?;
    let upper = right.trim().parse::<i64>().ok()?;
    let indexes = if lower <= upper {
        (lower..=upper).collect::<Vec<_>>()
    } else {
        (upper..=lower).rev().collect::<Vec<_>>()
    };
    let symbol = string_field(selected, "symbol")?;
    let name = symbol.split_whitespace().next_back()?.to_string();
    Some(
        indexes
            .into_iter()
            .map(|index| format!("{name}[{index}]"))
            .collect(),
    )
}

fn named_class_handle_identity(
    value: &Value,
    class_types: &std::collections::BTreeSet<String>,
) -> Option<(String, String)> {
    if string_field(value, "kind").as_deref() == Some("Conversion") {
        return named_class_handle_identity(value.get("operand")?, class_types);
    }
    if !matches!(
        string_field(value, "kind").as_deref(),
        Some("NamedValue" | "HierarchicalValue")
    ) {
        return None;
    }
    let symbol = string_field(value, "symbol")?;
    let name = symbol.split_whitespace().next_back()?.to_string();
    let class_name = canonical_class_name(&string_field(value, "type")?)?;
    class_types.contains(&class_name).then_some((name, symbol))
}

fn canonical_class_name(type_name: &str) -> Option<String> {
    let class_name = type_name
        .split_whitespace()
        .next_back()?
        .rsplit(['.', '#', ':'])
        .next()?
        .to_string();
    (!class_name.is_empty()).then_some(class_name)
}

fn is_class_handle_target(
    value: &Value,
    class_handles: &std::collections::BTreeSet<String>,
) -> bool {
    if string_field(value, "kind").as_deref() == Some("MemberAccess") {
        return value
            .get("value")
            .map(|selected| is_class_handle_target(selected, class_handles))
            .unwrap_or(false);
    }
    if string_field(value, "kind").as_deref() == Some("ElementSelect") {
        return value
            .get("value")
            .map(|selected| is_class_handle_target(selected, class_handles))
            .unwrap_or(false);
    }
    let Some(symbol) = string_field(value, "symbol") else {
        return false;
    };
    class_handles.contains(&symbol)
}

pub(crate) fn slang_expression_shape(expression: &SlangExpressionSummary) -> Option<&'static str> {
    match (expression.kind.as_str(), expression.operator.as_str()) {
        ("Concatenation", _) if expression.type_name == "string" => None,
        ("NamedValue", _) if expression.literal.is_some() => Some("Const"),
        ("NamedValue", _) => Some("Ref"),
        ("IntegerLiteral" | "RealLiteral" | "StringLiteral", _) => Some("Const"),
        // Selects are canonicalized differently by the bounded frontend:
        // constant selects may become direct references while dynamic selects
        // become Index nodes. Defer exact select-shape equivalence to typed
        // AST lowering rather than guessing here.
        ("ElementSelect" | "RangeSelect", _) => None,
        ("Concat" | "Concatenation", _) => Some("Concat"),
        ("Inside", _) => Some("Inside"),
        ("Replication", _) => Some("Repeat"),
        ("ConditionalOp", _) => Some("Cond"),
        ("UnaryOp", "BitwiseNot") => Some("Not"),
        ("UnaryOp", "LogicalNot") => Some("Not"),
        ("BinaryOp", "Add") => Some("Add"),
        ("BinaryOp", "Subtract") => Some("Sub"),
        ("BinaryOp", "Multiply") => Some("Mul"),
        ("BinaryOp", "Power") => Some("Pow"),
        ("BinaryOp", "Divide") => Some("Div"),
        ("BinaryOp", "Modulus") => Some("Mod"),
        ("BinaryOp", "BinaryAnd") => Some("And"),
        ("BinaryOp", "BinaryOr") => Some("Or"),
        ("BinaryOp", "BinaryXor" | "BitwiseXor") => Some("Xor"),
        ("BinaryOp", "LogicalShiftLeft" | "ArithmeticShiftLeft") => Some("Shl"),
        ("BinaryOp", "LogicalShiftRight" | "ArithmeticShiftRight") => Some("Shr"),
        ("BinaryOp", "LogicalAnd") => Some("LogAnd"),
        ("BinaryOp", "LogicalOr") => Some("LogOr"),
        ("BinaryOp", "Equality") => Some("Eq"),
        ("BinaryOp", "Inequality") => Some("Ne"),
        ("BinaryOp", "CaseEquality") => Some("CaseEq"),
        ("BinaryOp", "CaseInequality") => Some("CaseNe"),
        ("BinaryOp", "LessThan") => Some("Lt"),
        ("BinaryOp", "LessThanEqual") => Some("Le"),
        ("BinaryOp", "GreaterThan") => Some("Gt"),
        ("BinaryOp", "GreaterThanEqual") => Some("Ge"),
        _ => None,
    }
}

pub(crate) fn contains_expected_shapes(actual: &[String], expected: &[String]) -> bool {
    let mut remaining = actual.to_vec();
    for shape in expected {
        let index = if shape == "?" {
            if remaining.is_empty() {
                return false;
            }
            0
        } else {
            let Some(index) = remaining.iter().position(|candidate| candidate == shape) else {
                return false;
            };
            index
        };
        remaining.swap_remove(index);
    }
    true
}

pub(crate) fn parameter_constant_fingerprint(parameter: &SlangParameterSummary) -> Option<String> {
    let (word, width, signed) = if parameter.type_name == "string" {
        (
            string_literal_word(parameter.value.trim().trim_matches('"'))?,
            STRING_WIDTH,
            false,
        )
    } else {
        (
            literal_word(&parameter.type_name, &parameter.value)?,
            simple_type_width(&parameter.type_name)?,
            simple_type_signed(&parameter.type_name)?,
        )
    };
    let bits = word.to_bin_str();
    let signed_label = if signed && bits.starts_with('1') {
        "s"
    } else {
        "u"
    };
    Some(format!(
        "Const:{width}:{signed_label}[{}:{}]",
        word.width(),
        bits
    ))
}

pub(crate) fn summary_literal_i64(expression: &SlangExpressionSummary) -> Option<i64> {
    let (_, bits) = expression.literal.as_deref()?.split_once(':')?;
    let mut value = 0_i64;
    for bit in bits.chars() {
        value = value.checked_mul(2)?;
        if bit == '1' {
            value = value.checked_add(1)?;
        } else if bit != '0' {
            return None;
        }
    }
    Some(value)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExpressionFingerprintMode {
    Direct,
    BoundedCompatibility,
}

fn constant_packed_select_fingerprint(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    let selected = expression.operands.first()?;
    if selected.type_name.contains('$') {
        return None;
    }
    let shape = simple_type_shape(&selected.type_name)?;
    let implicit_dimension = AstPackedDimension {
        msb: i64::try_from(shape.width.checked_sub(1)?).ok()?,
        lsb: 0,
        length: shape.width,
        element_width: 1,
    };
    let dimension = shape.dimensions.first().unwrap_or(&implicit_dimension);
    let width = expression.type_width?;
    if width == 0 || width > shape.width {
        return None;
    }

    let physical_bit = |logical: i64| -> Option<i64> {
        let lower = dimension.msb.min(dimension.lsb);
        let upper = dimension.msb.max(dimension.lsb);
        if logical < lower || logical > upper {
            return None;
        }
        let coordinate = if dimension.msb >= dimension.lsb {
            logical - dimension.lsb
        } else {
            dimension.lsb - logical
        };
        coordinate.checked_mul(dimension.element_width as i64)
    };

    let (logical_msb, logical_lsb) = match expression.kind.as_str() {
        "ElementSelect" => {
            let logical = summary_literal_i64(expression.operands.get(1)?)?;
            (logical, logical)
        }
        "RangeSelect" => match expression.selection_kind.as_str() {
            "Simple" => {
                let left = summary_literal_i64(expression.operands.get(1)?)?;
                let right = summary_literal_i64(expression.operands.get(2)?)?;
                (left.max(right), left.min(right))
            }
            "IndexedUp" => {
                let start = summary_literal_i64(expression.operands.get(1)?)?;
                let width = summary_literal_i64(expression.operands.get(2)?)?;
                if width <= 0 {
                    return None;
                }
                let width = usize::try_from(width).ok()?;
                let end = start.checked_add(width as i64 - 1)?;
                (end, start)
            }
            "IndexedDown" => {
                let start = summary_literal_i64(expression.operands.get(1)?)?;
                let width = summary_literal_i64(expression.operands.get(2)?)?;
                if width <= 0 {
                    return None;
                }
                let width = usize::try_from(width).ok()?;
                let end = start.checked_sub(width as i64 - 1)?;
                (start, end)
            }
            _ => return None,
        },
        _ => return None,
    };

    let physical_msb_bit = physical_bit(logical_msb)?;
    let physical_lsb_bit = physical_bit(logical_lsb)?;
    let (physical_msb, physical_lsb, width) = if expression.kind == "ElementSelect" {
        let physical_lsb = physical_msb_bit.min(physical_lsb_bit);
        let physical_msb = physical_lsb + width as i64 - 1;
        (physical_msb, physical_lsb, width)
    } else {
        let physical_msb = physical_msb_bit.max(physical_lsb_bit);
        let physical_lsb = physical_msb_bit.min(physical_lsb_bit);
        let width = usize::try_from(physical_msb - physical_lsb + 1).ok()?;
        (physical_msb, physical_lsb, width)
    };
    let base = slang_expression_fingerprint_with_parameters(selected, parameters, mode)?;
    Some(format!(
        "PackedSelect:{width}:u[{base},Range:{physical_msb}:{physical_lsb}]"
    ))
}

fn dynamic_packed_element_select_fingerprint(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    if expression.kind != "ElementSelect" {
        return None;
    }
    let selected = expression.operands.first()?;
    if selected.type_name.contains('$') {
        return None;
    }
    let selector = expression.operands.get(1)?;
    if selector.literal.is_some() {
        return None;
    }
    let shape = simple_type_shape(&selected.type_name)?;
    let dimension = shape.dimensions.first()?;
    let width = dimension.element_width;
    if expression.type_width? != width {
        return None;
    }

    let selector_width = selector.type_width?.max(1);
    let selector_fingerprint =
        slang_expression_fingerprint_with_parameters(selector, parameters, mode)?;
    let base = slang_expression_fingerprint_with_parameters(selected, parameters, mode)?;
    let lower = dimension.msb.min(dimension.lsb);
    let upper = dimension.msb.max(dimension.lsb);
    let mut selected_fingerprint = format!("Const:{width}:u[{width}:{}]", "x".repeat(width));

    for logical_index in lower..=upper {
        let coordinate = if dimension.msb >= dimension.lsb {
            logical_index - dimension.lsb
        } else {
            dimension.lsb - logical_index
        };
        let physical_lsb = usize::try_from(coordinate).ok()?.checked_mul(width)?;
        let physical_msb = physical_lsb + width - 1;
        let logical_bits = logical_index as u64;
        let mut constant_bits = String::new();
        for bit in (0..selector_width).rev() {
            constant_bits.push(if (logical_bits >> bit) & 1 == 0 {
                '0'
            } else {
                '1'
            });
        }
        let constant_fingerprint =
            format!("Const:{selector_width}:u[{selector_width}:{constant_bits}]");
        let condition = format!("Eq:1:u[{selector_fingerprint},{constant_fingerprint}]");
        let candidate =
            format!("PackedSelect:{width}:u[{base},Range:{physical_msb}:{physical_lsb}]");
        selected_fingerprint =
            format!("Cond:{width}:u[{condition},{candidate},{selected_fingerprint}]");
    }
    Some(selected_fingerprint)
}

fn dynamic_indexed_packed_range_select_fingerprint(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    if expression.kind != "RangeSelect"
        || !matches!(
            expression.selection_kind.as_str(),
            "IndexedUp" | "IndexedDown"
        )
    {
        return None;
    }
    let selected = expression.operands.first()?;
    if selected.type_name.contains('$') {
        return None;
    }
    let selector = expression.operands.get(1)?;
    if selector.literal.is_some() {
        return None;
    }
    let width = expression.type_width?;
    let declared_width = summary_literal_i64(expression.operands.get(2)?)?;
    if declared_width != i64::try_from(width).ok()? {
        return None;
    }

    let shape = simple_type_shape(&selected.type_name)?;
    let implicit_dimension = AstPackedDimension {
        msb: i64::try_from(shape.width.checked_sub(1)?).ok()?,
        lsb: 0,
        length: shape.width,
        element_width: 1,
    };
    let dimension = shape.dimensions.first().unwrap_or(&implicit_dimension);
    if width == 0 || width > dimension.length {
        return None;
    }
    let width_i64 = i64::try_from(width).ok()?;
    let lower = dimension.msb.min(dimension.lsb);
    let upper = dimension.msb.max(dimension.lsb);
    let starts = if expression.selection_kind == "IndexedUp" {
        lower..=upper.checked_add(1)?.checked_sub(width_i64)?
    } else {
        lower.checked_add(width_i64)?.checked_sub(1)?..=upper
    };

    let selector_width = selector.type_width?.max(1);
    let selector_fingerprint =
        slang_expression_fingerprint_with_parameters(selector, parameters, mode)?;
    let base = slang_expression_fingerprint_with_parameters(selected, parameters, mode)?;
    let physical_bit = |logical: i64| -> Option<i64> {
        if logical < lower || logical > upper {
            return None;
        }
        if dimension.msb >= dimension.lsb {
            logical.checked_sub(dimension.lsb)
        } else {
            dimension.lsb.checked_sub(logical)
        }
    };
    let mut selected_fingerprint = format!("Const:{width}:u[{width}:{}]", "x".repeat(width));
    for logical_start in starts {
        let (low_logical, high_logical) = if expression.selection_kind == "IndexedUp" {
            (logical_start, logical_start.checked_add(width_i64 - 1)?)
        } else {
            (logical_start.checked_sub(width_i64 - 1)?, logical_start)
        };
        let first = physical_bit(low_logical)?;
        let second = physical_bit(high_logical)?;
        let msb = first.max(second);
        let lsb = first.min(second);

        let mut constant_bits = String::with_capacity(selector_width);
        for bit in (0..selector_width).rev() {
            constant_bits.push(if ((logical_start as u64) >> bit) & 1 == 0 {
                '0'
            } else {
                '1'
            });
        }
        let condition = format!(
            "Eq:1:u[{selector_fingerprint},Const:{selector_width}:u[{selector_width}:{constant_bits}]]"
        );
        let candidate = format!("PackedSelect:{width}:u[{base},Range:{msb}:{lsb}]");
        selected_fingerprint =
            format!("Cond:{width}:u[{condition},{candidate},{selected_fingerprint}]");
    }
    Some(selected_fingerprint)
}

fn canonical_literal_fingerprint(width: usize, signed: bool, literal: &str) -> Option<String> {
    let (_, bits) = literal.split_once(':')?;
    let signed_label = if signed && bits.starts_with('1') {
        "s"
    } else {
        "u"
    };
    Some(format!("Const:{width}:{signed_label}[{literal}]"))
}

fn conversion_fingerprint(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    let operand = expression.operands.first()?;
    if expression.type_name == "string"
        && mode == ExpressionFingerprintMode::BoundedCompatibility
        && operand.kind == "StringLiteral"
    {
        let (_, bits) = operand.literal.as_deref()?.split_once(':')?;
        if bits.len() > STRING_WIDTH {
            return None;
        }
        let padded_bits = format!("{bits:0>width$}", width = STRING_WIDTH);
        return Some(format!(
            "Const:{STRING_WIDTH}:u[{STRING_WIDTH}:{padded_bits}]"
        ));
    }
    if expression.type_name == "string" && mode == ExpressionFingerprintMode::Direct {
        return direct_string_conversion_fingerprint(operand, parameters, mode);
    }
    if expression.kind == "Conversion" && operand.kind == "Streaming" {
        return streaming_conversion_fingerprint(expression, operand, parameters, mode);
    }
    if expression.type_name == "string" || operand.type_name == "string" {
        return None;
    }
    let target_width = expression.type_width.or_else(|| {
        expression
            .enum_definition
            .as_ref()
            .map(|definition| definition.width)
    })?;
    if expression.kind == "Conversion"
        && expression
            .enum_definition
            .as_ref()
            .is_some_and(|definition| definition.width == target_width)
    {
        return slang_expression_fingerprint_with_parameters(operand, parameters, mode);
    }
    let operand_fingerprint =
        slang_expression_fingerprint_with_parameters(operand, parameters, mode)?;
    if mode == ExpressionFingerprintMode::BoundedCompatibility {
        return Some(operand_fingerprint);
    }
    if operand.kind == "IntegerLiteral" && expression.type_name == "time" {
        return Some(operand_fingerprint);
    }
    if simple_type_shape(&expression.type_name).is_none()
        && target_width == operand.type_width?
        && expression.type_signed.unwrap_or(false) == operand.type_signed.unwrap_or(false)
    {
        return Some(operand_fingerprint);
    }

    let target_shape = simple_type_shape(&expression.type_name)?;
    let source_shape = simple_type_shape(&operand.type_name)?;
    let target_signed = simple_type_signed(&expression.type_name)?;
    let source_signed = simple_type_signed(&operand.type_name)?;
    let target_width = target_shape.width;
    let source_width = source_shape.width;
    if target_width == source_width && target_signed == source_signed {
        return Some(operand_fingerprint);
    }

    let resized = if target_width < source_width {
        let msb = target_width - 1;
        format!("PackedSelect:{target_width}:u[{operand_fingerprint},Range:{msb}:0]")
    } else if target_width > source_width {
        let extension = target_width - source_width;
        let padding = if source_signed {
            let sign_bit = source_width - 1;
            format!(
                "Repeat:{extension}:u[PackedSelect:1:u[{operand_fingerprint},Range:{sign_bit}:{sign_bit}]]"
            )
        } else {
            let zeros = "0".repeat(extension);
            format!("Const:{extension}:u[{extension}:{zeros}]")
        };
        format!("Concat:{target_width}:u[{padding},{operand_fingerprint}]")
    } else {
        operand_fingerprint
    };
    let signed_label = if target_signed { "s" } else { "u" };
    Some(format!("Signed:{target_width}:{signed_label}[{resized}]"))
}

fn streaming_conversion_fingerprint(
    conversion: &SlangExpressionSummary,
    streaming: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    let target_width = conversion.type_width?;
    let parts = streaming
        .operands
        .iter()
        .map(|operand| {
            Some((
                operand.type_width?,
                slang_expression_fingerprint_with_parameters(operand, parameters, mode)?,
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    if parts.is_empty() {
        return None;
    }
    let source_width = parts.iter().map(|(width, _)| *width).sum::<usize>();
    if source_width != target_width {
        return None;
    }
    let source = if let [(_, only)] = parts.as_slice() {
        only.clone()
    } else {
        format!(
            "Concat:{source_width}:u[{}]",
            parts
                .iter()
                .map(|(_, fingerprint)| fingerprint.as_str())
                .collect::<Vec<_>>()
                .join(",")
        )
    };

    let slice_size = streaming.streaming_slice_size?;
    if slice_size == 0 {
        return Some(source);
    }
    if slice_size > source_width || source_width % slice_size != 0 {
        return None;
    }
    let blocks = source_width / slice_size;
    let mut reversed_blocks = Vec::with_capacity(blocks);
    for block in 0..blocks {
        let lsb = block * slice_size;
        let msb = lsb + slice_size - 1;
        reversed_blocks.push(format!(
            "PackedSelect:{slice_size}:u[{source},Range:{msb}:{lsb}]"
        ));
    }
    Some(format!(
        "Concat:{target_width}:u[{}]",
        reversed_blocks.join(",")
    ))
}

fn direct_string_conversion_fingerprint(
    operand: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    if operand.kind == "Concatenation" {
        let left =
            direct_string_conversion_fingerprint(operand.operands.first()?, parameters, mode)?;
        let right =
            direct_string_conversion_fingerprint(operand.operands.get(1)?, parameters, mode)?;
        return Some(format!("StringConcat:512:u[{left},{right}]"));
    }
    if operand.type_name == "string" {
        return slang_expression_fingerprint_with_parameters(operand, parameters, mode);
    }

    let source_shape = simple_type_shape(&operand.type_name)?;
    let source_width = source_shape.width;
    let source_signed = simple_type_signed(&operand.type_name)?;
    let operand_fingerprint =
        slang_expression_fingerprint_with_parameters(operand, parameters, mode)?;
    if source_width == STRING_WIDTH && !source_signed {
        return Some(operand_fingerprint);
    }
    if source_width > STRING_WIDTH {
        return None;
    }

    let extension = STRING_WIDTH - source_width;
    let padding = if source_signed {
        let sign_bit = source_width.saturating_sub(1);
        format!(
            "Repeat:{extension}:u[PackedSelect:1:u[{operand_fingerprint},Range:{sign_bit}:{sign_bit}]]"
        )
    } else {
        let zeros = "0".repeat(extension);
        format!("Const:{extension}:u[{extension}:{zeros}]")
    };
    Some(format!(
        "Signed:512:u[Concat:512:u[{padding},{operand_fingerprint}]]"
    ))
}

fn unary_composition_fingerprint(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    let operand = expression.operands.first()?;
    let operand_fingerprint =
        slang_expression_fingerprint_with_parameters(operand, parameters, mode)?;
    match expression.operator.as_str() {
        "Plus" => Some(operand_fingerprint),
        "Minus" => {
            let width = expression.type_width?;
            let signed_label = if expression.type_signed? { "s" } else { "u" };
            let zeros = "0".repeat(width);
            Some(format!(
                "Sub:{width}:{signed_label}[Const:{width}:u[{width}:{zeros}],{operand_fingerprint}]"
            ))
        }
        "BitwiseAnd" => Some(format!("AndR:1:u[{operand_fingerprint}]")),
        "BitwiseOr" => Some(format!("OrR:1:u[{operand_fingerprint}]")),
        "BitwiseXor" => Some(format!("XorR:1:u[{operand_fingerprint}]")),
        "BitwiseNand" => Some(format!("Not:1:u[AndR:1:u[{operand_fingerprint}]]")),
        "BitwiseNor" => Some(format!("Not:1:u[OrR:1:u[{operand_fingerprint}]]")),
        "BitwiseXnor" => Some(format!("Not:1:u[XorR:1:u[{operand_fingerprint}]]")),
        _ => None,
    }
}

fn binary_composition_fingerprint(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    let left = slang_expression_fingerprint_with_parameters(
        expression.operands.first()?,
        parameters,
        mode,
    )?;
    let right = slang_expression_fingerprint_with_parameters(
        expression.operands.get(1)?,
        parameters,
        mode,
    )?;
    let width = expression.type_width?;
    let signed_label = if expression.type_signed? { "s" } else { "u" };
    match expression.operator.as_str() {
        "BinaryXnor" => Some(format!(
            "Not:{width}:{signed_label}[Xor:{width}:{signed_label}[{left},{right}]]"
        )),
        "LogicalImplication" => Some(format!("LogOr:1:u[Not:1:u[{left}],{right}]")),
        "LogicalEquivalence" => Some(format!(
            "LogAnd:1:u[LogOr:1:u[Not:1:u[{left}],{right}],LogOr:1:u[Not:1:u[{right}],{left}]]"
        )),
        _ => None,
    }
}

fn wildcard_equality_fingerprint(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    if mode != ExpressionFingerprintMode::Direct {
        return None;
    }
    if !matches!(
        expression.operator.as_str(),
        "WildcardEquality" | "WildcardInequality"
    ) {
        return None;
    }
    let left = slang_expression_fingerprint_with_parameters(
        expression.operands.first()?,
        parameters,
        mode,
    )?;
    let pattern = expression.operands.get(1)?;
    let (width, bits) = pattern.literal.as_deref()?.split_once(':')?;
    let width = width.parse::<usize>().ok()?;
    let mut mask_bits = String::with_capacity(bits.len());
    let mut expected_bits = String::with_capacity(bits.len());
    for bit in bits.chars() {
        if matches!(bit, 'x' | 'X' | 'z' | 'Z') {
            mask_bits.push('0');
            expected_bits.push('0');
        } else if matches!(bit, '0' | '1') {
            mask_bits.push('1');
            expected_bits.push(bit);
        } else {
            return None;
        }
    }
    let equality = format!(
        "Eq:1:u[And:{width}:u[{left},Const:{width}:u[{width}:{mask_bits}]],Const:{width}:u[{width}:{expected_bits}]]"
    );
    if expression.operator == "WildcardInequality" {
        return Some(format!("Not:1:u[{equality}]"));
    }
    Some(equality)
}

fn inside_fingerprint(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    let left = slang_expression_fingerprint_with_parameters(
        expression.operands.first()?,
        parameters,
        mode,
    )?;
    let mut result = Some("Const:1:u[1:0]".to_string());
    for range in expression.operands.iter().skip(1) {
        let comparison = match range.kind.as_str() {
            "Conversion" => {
                let value = slang_expression_fingerprint_with_parameters(
                    range.operands.first()?,
                    parameters,
                    mode,
                )?;
                format!("Eq:1:u[{left},{value}]")
            }
            "ValueRange" => {
                let lower = slang_expression_fingerprint_with_parameters(
                    range.operands.first()?,
                    parameters,
                    mode,
                )?;
                let upper = slang_expression_fingerprint_with_parameters(
                    range.operands.get(1)?,
                    parameters,
                    mode,
                )?;
                format!("LogAnd:1:u[Ge:1:u[{left},{lower}],Le:1:u[{left},{upper}]]")
            }
            _ => return None,
        };
        let previous = result?;
        result = Some(format!("LogOr:1:u[{previous},{comparison}]"));
    }
    result
}

fn direct_lowered_signed(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
) -> Option<bool> {
    match expression.kind.as_str() {
        "IntegerLiteral" => expression.type_signed,
        "NamedValue" | "HierarchicalValue" => {
            let symbol = expression.symbol.as_deref()?;
            if let Some(parameter) = parameters.iter().find(|parameter| parameter.name == symbol) {
                return simple_type_signed(&parameter.type_name);
            }
            expression.type_signed
        }
        "Conversion" => {
            let operand = expression.operands.first()?;
            let operand_signed = direct_lowered_signed(operand, parameters)?;
            let target_width = simple_type_shape(&expression.type_name)?.width;
            let source_width = simple_type_shape(&operand.type_name)?.width;
            let target_signed = simple_type_signed(&expression.type_name)?;
            let source_signed = simple_type_signed(&operand.type_name)?;
            if target_width == source_width
                && target_signed == source_signed
                && operand_signed == target_signed
            {
                Some(operand_signed)
            } else {
                Some(target_signed)
            }
        }
        "UnaryOp" => match expression.operator.as_str() {
            "BitwiseNot" | "LogicalNot" => {
                direct_lowered_signed(expression.operands.first()?, parameters)
            }
            "BitwiseAnd" | "BitwiseOr" | "BitwiseXor" | "BitwiseNand" | "BitwiseNor"
            | "BitwiseXnor" | "Plus" | "Minus" => Some(false),
            _ => None,
        },
        "BinaryOp" => {
            let left = direct_binary_operand_signed(expression.operands.first()?, parameters)?;
            let right = direct_binary_operand_signed(expression.operands.get(1)?, parameters)?;
            match expression.operator.as_str() {
                "LogicalShiftLeft"
                | "ArithmeticShiftLeft"
                | "LogicalShiftRight"
                | "ArithmeticShiftRight" => Some(left),
                "Add" | "Subtract" | "Multiply" | "Divide" | "Modulus" | "Power" | "BinaryAnd"
                | "BinaryOr" | "BinaryXor" | "BitwiseXor" => Some(left && right),
                "Equality" | "Inequality" | "CaseEquality" | "CaseInequality" | "LessThan"
                | "LessThanEqual" | "GreaterThan" | "GreaterThanEqual" | "LogicalAnd"
                | "LogicalOr" | "LogicalImplication" | "LogicalEquivalence" => Some(false),
                _ => None,
            }
        }
        "ConditionalOp" => {
            if expression.operands.len() == 3 {
                let left = direct_lowered_signed(expression.operands.get(1)?, parameters)?;
                let right = direct_lowered_signed(expression.operands.get(2)?, parameters)?;
                Some(left && right)
            } else {
                None
            }
        }
        "Concatenation" | "StringLiteral" | "NewArray" => Some(false),
        "Replication" => direct_lowered_signed(expression.operands.first()?, parameters),
        _ => None,
    }
}

fn direct_binary_operand_signed(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
) -> Option<bool> {
    if matches!(expression.kind.as_str(), "NamedValue" | "HierarchicalValue") {
        if let Some(symbol) = expression.symbol.as_deref() {
            if parameters.iter().any(|parameter| parameter.name == symbol) {
                return Some(false);
            }
        }
    }
    direct_lowered_signed(expression, parameters)
}

fn memory_selection(
    expression: &SlangExpressionSummary,
) -> Option<(&SlangExpressionSummary, Vec<&SlangExpressionSummary>)> {
    if matches!(expression.kind.as_str(), "NamedValue" | "HierarchicalValue")
        && expression.type_name.contains('$')
        && expression.type_name != "event"
    {
        return Some((expression, Vec::new()));
    }
    if expression.kind != "ElementSelect" {
        return None;
    }
    let selected = expression.operands.first()?;
    let selector = expression.operands.get(1)?;
    let (memory, mut indexes) = memory_selection(selected)?;
    indexes.push(selector);
    Some((memory, indexes))
}

fn memory_symbol_name(memory: &SlangExpressionSummary) -> Option<&str> {
    let symbol = memory.symbol.as_deref()?;
    Some(
        symbol
            .rsplit_once(' ')
            .map(|(_, name)| name)
            .unwrap_or(symbol),
    )
}

fn string_method_fingerprint(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    if mode == ExpressionFingerprintMode::BoundedCompatibility {
        return None;
    }
    let receiver = expression.operands.first()?;
    if receiver.type_name != "string" {
        return None;
    }
    let receiver = slang_expression_fingerprint_with_parameters(receiver, parameters, mode)?;
    match expression.operator.as_str() {
        "len" => Some(format!("StringLength:32:s[{receiver}]")),
        "getc" => {
            let index = slang_expression_fingerprint_with_parameters(
                expression.operands.get(1)?,
                parameters,
                mode,
            )?;
            Some(format!("StringGetByte:8:u[{receiver},{index}]"))
        }
        "toupper" => Some(format!("StringCase:512:u[Upper:{receiver}]")),
        "tolower" => Some(format!("StringCase:512:u[Lower:{receiver}]")),
        "compare" => {
            let right = slang_expression_fingerprint_with_parameters(
                expression.operands.get(1)?,
                parameters,
                mode,
            )?;
            Some(format!(
                "StringCompare:32:s[CaseSensitive:{receiver},{right}]"
            ))
        }
        "icompare" => {
            let right = slang_expression_fingerprint_with_parameters(
                expression.operands.get(1)?,
                parameters,
                mode,
            )?;
            Some(format!("StringCompare:32:s[IgnoreCase:{receiver},{right}]"))
        }
        "substr" => {
            let left = slang_expression_fingerprint_with_parameters(
                expression.operands.get(1)?,
                parameters,
                mode,
            )?;
            let right = slang_expression_fingerprint_with_parameters(
                expression.operands.get(2)?,
                parameters,
                mode,
            )?;
            Some(format!("StringSubstr:512:u[{receiver},{left},{right}]"))
        }
        "atoi" => Some(format!("StringToRadix:32:s[Radix10:{receiver}]")),
        "atohex" => Some(format!("StringToRadix:32:s[Radix16:{receiver}]")),
        "atobin" => Some(format!("StringToRadix:32:s[Radix2:{receiver}]")),
        _ => None,
    }
}

fn enum_name_receiver_fingerprint(
    receiver: &SlangExpressionSummary,
    definition: &SlangEnumTypeSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    let mut normalized = receiver.clone();
    normalized.type_name = definition.base_type_name.clone();
    normalized.type_width = Some(definition.width);
    normalized.type_signed = Some(definition.signed);
    normalized.enum_definition = None;
    if normalized.literal.is_none() && matches!(normalized.kind.as_str(), "NamedValue") {
        let symbol = normalized.symbol.as_deref()?;
        let constant_name = symbol
            .split_whitespace()
            .next_back()
            .unwrap_or_default()
            .rsplit(['.', '#', ':'])
            .next()
            .unwrap_or_default();
        normalized.literal = definition
            .members
            .iter()
            .find(|member| member.name == constant_name)
            .map(|member| member.literal.clone())
            .or_else(|| {
                let parameter = parameters
                    .iter()
                    .find(|parameter| parameter.name == constant_name)?;
                literal_word(&definition.base_type_name, &parameter.value)
                    .map(|word| format!("{}:{}", word.width(), word.to_bin_str()))
                    .or_else(|| {
                        definition
                            .members
                            .iter()
                            .find(|member| member.name == parameter.value.trim())
                            .map(|member| member.literal.clone())
                    })
            });
    }
    slang_expression_fingerprint_with_parameters(&normalized, parameters, mode)
}

fn enum_name_fingerprint(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    if mode != ExpressionFingerprintMode::Direct {
        return None;
    }
    let definition = expression.enum_definition.as_ref()?;
    let receiver = expression.operands.first()?;
    let receiver = enum_name_receiver_fingerprint(receiver, definition, parameters, mode)?;

    let zeros = "0".repeat(STRING_WIDTH);
    let mut result = format!("Const:{STRING_WIDTH}:u[{STRING_WIDTH}:{zeros}]");
    for member in &definition.members {
        let enum_value = format!("Const:{}:u[{}]", definition.width, member.literal);
        let name = string_literal_word(&member.name)?;
        let rendered = format!(
            "Const:{STRING_WIDTH}:u[{STRING_WIDTH}:{}]",
            name.to_bin_str()
        );
        let condition = format!("Eq:1:u[{receiver},{enum_value}]");
        result = format!("Cond:{STRING_WIDTH}:u[{condition},{rendered},{result}]");
    }
    Some(result)
}

fn enum_method_fingerprint(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    if mode != ExpressionFingerprintMode::Direct {
        return None;
    }
    let definition = expression.enum_definition.as_ref()?;
    let method = expression
        .operator
        .split_whitespace()
        .next_back()
        .unwrap_or_default();
    if matches!(method, "first" | "last") {
        let member = if method == "first" {
            definition.members.first()?
        } else {
            definition.members.last()?
        };
        return Some(format!("Const:{}:u[{}]", definition.width, member.literal));
    }
    if method == "num" {
        let count = definition.members.len();
        return Some(format!("Const:32:u[32:{count:032b}]"));
    }
    if !matches!(method, "next" | "prev") {
        return None;
    }

    let receiver =
        enum_name_receiver_fingerprint(expression.operands.first()?, definition, parameters, mode)?;
    let mut count = match expression.operands.get(1) {
        Some(count) => summary_literal_i64(count)
            .or_else(|| count.operands.first().and_then(summary_literal_i64))?,
        None => 1,
    };
    if method == "prev" {
        count = -count;
    }
    let length = i64::try_from(definition.members.len()).ok()?;
    let offset = count.rem_euclid(length);

    let mut result = format!(
        "Const:{}:u[{}:{}]",
        definition.width,
        definition.width,
        "x".repeat(definition.width)
    );
    for (index, current) in definition.members.iter().enumerate() {
        let target_index = (index as i64 + offset) % length;
        let target = definition
            .members
            .get(usize::try_from(target_index).ok()?)?;
        let condition = format!(
            "Eq:1:u[{receiver},Const:{}:u[{}]]",
            definition.width, current.literal
        );
        result = format!(
            "Cond:{}:u[{condition},Const:{}:u[{}],{result}]",
            definition.width, definition.width, target.literal
        );
    }
    Some(result)
}

fn array_system_query_fingerprint(expression: &SlangExpressionSummary) -> Option<String> {
    const QUERY_METHODS: [&str; 8] = [
        "$dimensions",
        "$unpacked_dimensions",
        "$bits",
        "$left",
        "$right",
        "$low",
        "$high",
        "$increment",
    ];
    let method = expression
        .operator
        .split_whitespace()
        .next_back()
        .unwrap_or_default();
    if !QUERY_METHODS.contains(&method) {
        return None;
    }
    let selected = expression.operands.first()?;
    let packed_type = selected
        .type_name
        .split('$')
        .next()
        .unwrap_or_default()
        .trim();
    let packed_shape = simple_type_shape(packed_type)?;
    let memory_shape = if selected.type_name.contains('$') {
        if selected.type_name.ends_with("$[]") {
            dynamic_memory_type(&selected.type_name)
        } else if selected.type_name.contains("$queue") {
            queue_memory_type(&selected.type_name)
        } else if selected.type_name.contains("$assoc[") {
            associative_memory_type(&selected.type_name)
        } else {
            let element_msb = i64::try_from(packed_shape.width.checked_sub(1)?).ok()?;
            memory_type_with_element(
                &selected.type_name,
                packed_shape.width,
                packed_shape.bounds.unwrap_or((element_msb, 0)),
                simple_type_signed(packed_type)?,
            )
        }
    } else {
        None
    };
    let dimensions = memory_shape
        .as_ref()
        .map(|memory| memory.dimensions.clone())
        .unwrap_or_default();
    let unpacked_count = dimensions.len();
    let total_dimensions = unpacked_count + packed_shape.dimensions.len();

    let const32 = |value: i64| {
        let (signed_label, bits) = if value < 0 {
            ("s", format!("{:032b}", value as u32))
        } else {
            ("u", format!("{value:032b}"))
        };
        Some(format!("Const:32:{signed_label}[32:{bits}]"))
    };
    match method {
        "$dimensions" => const32(i64::try_from(total_dimensions).ok()?),
        "$unpacked_dimensions" => const32(i64::try_from(unpacked_count).ok()?),
        "$bits" => {
            if selected.type_name.contains("$[]")
                || selected.type_name.contains("$queue")
                || selected.type_name.contains("$assoc[")
            {
                let memory = selected.symbol.as_deref()?;
                let memory = memory
                    .rsplit_once(' ')
                    .map(|(_, memory)| memory)
                    .unwrap_or(memory);
                let memory_shape = memory_shape?;
                return Some(format!(
                    "Mul:32:s[MemorySize:32:s[{memory}],{}]",
                    const32(i64::try_from(memory_shape.element_width).ok()?)?
                ));
            }
            let bits = if let Some(memory) = memory_shape.as_ref() {
                usize::try_from(memory.flat_length)
                    .ok()?
                    .checked_mul(memory.element_width)?
            } else {
                packed_shape.width
            };
            const32(i64::try_from(bits).ok()?)
        }
        "$left" | "$right" | "$low" | "$high" | "$increment" => {
            let index = expression
                .operands
                .get(1)
                .and_then(summary_literal_i64)
                .unwrap_or(1);
            let index = usize::try_from(index).ok()?;
            let physical_index = index.checked_sub(1)?;
            let (base, step, length) = if let Some(dimension) = dimensions.get(physical_index) {
                (
                    dimension.base,
                    dimension.step,
                    i64::try_from(dimension.length).ok()?,
                )
            } else {
                let dimension = packed_shape.dimensions.get(physical_index)?;
                (
                    dimension.msb,
                    if dimension.msb >= dimension.lsb {
                        1
                    } else {
                        -1
                    },
                    i64::try_from(dimension.length).ok()?,
                )
            };
            let value = match method {
                "$left" => base,
                "$right" => base + step * (length - 1),
                "$low" => base.min(base + step * (length - 1)),
                "$high" => base.max(base + step * (length - 1)),
                _ => step,
            };
            const32(value)
        }
        _ => None,
    }
}

pub(crate) fn slang_expression_fingerprint_with_parameters(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    if expression.kind == "StringLiteral" {
        let literal = expression.literal.as_deref()?;
        let width = expression.type_width.unwrap_or(STRING_WIDTH);
        return canonical_literal_fingerprint(width, false, literal);
    }
    if expression.kind == "Concatenation"
        && expression.type_name == "string"
        && mode == ExpressionFingerprintMode::Direct
    {
        let left = slang_expression_fingerprint_with_parameters(
            expression.operands.first()?,
            parameters,
            mode,
        )?;
        let right = slang_expression_fingerprint_with_parameters(
            expression.operands.get(1)?,
            parameters,
            mode,
        )?;
        return Some(format!("StringConcat:512:u[{left},{right}]"));
    }
    if expression.kind == "Call" {
        let method = expression
            .operator
            .split_whitespace()
            .next_back()
            .unwrap_or_default();
        let operation = match method {
            "first" => "First",
            "last" => "Last",
            "next" => "Next",
            "prev" => "Prev",
            _ => "",
        };
        if let Some(fingerprint) = array_system_query_fingerprint(expression) {
            return Some(fingerprint);
        }
        if !operation.is_empty() {
            let selected = expression.operands.first()?;
            if selected.type_name.contains("$assoc[") {
                let selected_symbol = selected.symbol.as_deref()?;
                let memory = selected_symbol
                    .rsplit_once(' ')
                    .map(|(_, memory)| memory)
                    .unwrap_or(selected_symbol);
                let key = slang_expression_fingerprint_with_parameters(
                    expression.operands.get(1)?,
                    parameters,
                    mode,
                )?;
                return Some(format!("AssocTraversal:{operation}:u[{memory},{key}]"));
            }
        }
        if method == "name" {
            if let Some(fingerprint) = enum_name_fingerprint(expression, parameters, mode) {
                return Some(fingerprint);
            }
        }
        if let Some(fingerprint) = enum_method_fingerprint(expression, parameters, mode) {
            return Some(fingerprint);
        }
        if let Some(fingerprint) = string_method_fingerprint(expression, parameters, mode) {
            return Some(fingerprint);
        }
        if matches!(method, "$time" | "$stime") && mode == ExpressionFingerprintMode::Direct {
            let width = if method == "$time" { 64 } else { 32 };
            let signed_label = if method == "$time" { "s" } else { "u" };
            let unit = expression.simulation_time_unit?;
            return Some(format!("SimulationTime:{width}:{signed_label}[Unit{unit}]"));
        }
        if matches!(method, "size" | "num") {
            let selected = expression.operands.first()?;
            if selected.type_name.contains('$') {
                let selected_symbol = selected.symbol.as_deref()?;
                let memory = selected_symbol
                    .rsplit_once(' ')
                    .map(|(_, memory)| memory)
                    .unwrap_or(selected_symbol);
                return Some(format!("MemorySize:32:s[{memory}]"));
            }
        }
        if method == "$size" {
            let selected = expression.operands.first()?;
            if selected.type_name.contains('$') {
                let selected_symbol = selected.symbol.as_deref()?;
                let memory = selected_symbol
                    .rsplit_once(' ')
                    .map(|(_, memory)| memory)
                    .unwrap_or(selected_symbol);
                if selected.type_name.contains("$[]")
                    || selected.type_name.contains("$queue")
                    || selected.type_name.contains("$assoc[")
                {
                    return Some(format!("MemorySize:32:s[{memory}]"));
                }
                if let Some(array) = memory_type(&selected.type_name) {
                    let dimension = expression
                        .operands
                        .get(1)
                        .and_then(summary_literal_i64)
                        .unwrap_or(1);
                    let dimension = usize::try_from(dimension).ok()?;
                    let dimension = array.dimensions.get(dimension.checked_sub(1)?)?;
                    let bits = format!("32:{:032b}", dimension.length);
                    return Some(format!("Const:32:u[{bits}]"));
                }
            }
        }
        if method == "exists" {
            let selected = expression.operands.first()?;
            if selected.type_name.contains("$assoc[") {
                let selected_symbol = selected.symbol.as_deref()?;
                let memory = selected_symbol
                    .rsplit_once(' ')
                    .map(|(_, memory)| memory)
                    .unwrap_or(selected_symbol);
                let key = slang_expression_fingerprint_with_parameters(
                    expression.operands.get(1)?,
                    parameters,
                    mode,
                )?;
                return Some(format!("AssocExists:1:u[{memory},{key}]"));
            }
        }
    }
    if expression.kind == "NewArray" {
        let size = slang_expression_fingerprint_with_parameters(
            expression.operands.first()?,
            parameters,
            mode,
        )?;
        return Some(format!("MemoryNew:{size}"));
    }
    if expression.kind == "Call" && matches!(expression.operator.as_str(), "pop_front" | "pop_back")
    {
        let selected = expression.operands.first()?;
        if selected.type_name.contains("$queue") {
            let front = expression.operator == "pop_front";
            let selected_symbol = selected.symbol.as_deref()?;
            let memory = selected_symbol
                .rsplit_once(' ')
                .map(|(_, memory)| memory)
                .unwrap_or(selected_symbol);
            return Some(format!("QueuePop:{front}:{memory}"));
        }
    }
    if matches!(expression.kind.as_str(), "NamedValue" | "HierarchicalValue")
        && expression.type_name.contains('$')
        && expression.type_name != "event"
    {
        let symbol = expression.symbol.as_deref()?;
        return Some(format!("MemoryCopy:{symbol}"));
    }
    if expression.kind == "ElementSelect" {
        if let Some((memory_base, indexes)) = memory_selection(expression) {
            let memory = memory_symbol_name(memory_base)?;
            let indexes = indexes
                .into_iter()
                .map(|index| slang_expression_fingerprint_with_parameters(index, parameters, mode))
                .collect::<Option<Vec<_>>>()?;
            let memory_shape = memory_type(&memory_base.type_name);
            let width = expression
                .type_width
                .or_else(|| memory_shape.as_ref().map(|shape| shape.element_width))?;
            let signed = expression
                .type_signed
                .or_else(|| memory_shape.as_ref().map(|shape| shape.signed))?;
            let signed_label = if signed { "s" } else { "u" };
            if memory_base.type_name.contains("$assoc[") {
                return Some(format!(
                    "AssocRead:{width}:{signed_label}[{memory},{}]",
                    indexes.join(",")
                ));
            }
            return Some(format!(
                "MemoryRead:{width}:{signed_label}[{memory},{}]",
                indexes.join(",")
            ));
        }
    }
    if expression.kind == "ElementSelect"
        && expression
            .operands
            .first()
            .is_some_and(|selected| selected.type_name == "string")
        && mode == ExpressionFingerprintMode::Direct
    {
        let selected = slang_expression_fingerprint_with_parameters(
            expression.operands.first()?,
            parameters,
            mode,
        )?;
        let index = slang_expression_fingerprint_with_parameters(
            expression.operands.get(1)?,
            parameters,
            mode,
        )?;
        return Some(format!("StringGetByte:8:u[{selected},{index}]"));
    }
    if expression.kind == "RangeSelect" {
        let selected = expression.operands.first()?;
        if let Some((memory_base, indexes)) = memory_selection(selected) {
            let memory = memory_symbol_name(memory_base)?;
            let indexes = indexes
                .iter()
                .map(|index| slang_expression_fingerprint_with_parameters(index, parameters, mode))
                .collect::<Option<Vec<_>>>()?;
            let start_operand = match expression.selection_kind.as_str() {
                "Simple" => expression.operands.get(2)?,
                "IndexedUp" | "IndexedDown" => expression.operands.get(1)?,
                _ => return None,
            };
            let start =
                slang_expression_fingerprint_with_parameters(start_operand, parameters, mode)?;
            let width = expression.type_width?;
            let ascending = expression.selection_kind != "IndexedDown";
            let direction = if ascending { "Ascending" } else { "Descending" };
            return Some(format!(
                "MemoryReadPart:{width}:u[{memory},{},{start},{direction}]",
                indexes.join(",")
            ));
        }
    }
    if matches!(expression.kind.as_str(), "ElementSelect" | "RangeSelect") {
        if let Some(fingerprint) = constant_packed_select_fingerprint(expression, parameters, mode)
        {
            return Some(fingerprint);
        }
        if let Some(fingerprint) =
            dynamic_packed_element_select_fingerprint(expression, parameters, mode)
        {
            return Some(fingerprint);
        }
        if let Some(fingerprint) =
            dynamic_indexed_packed_range_select_fingerprint(expression, parameters, mode)
        {
            return Some(fingerprint);
        }
    }
    if expression.kind == "UnaryOp" {
        if let Some(fingerprint) = unary_composition_fingerprint(expression, parameters, mode) {
            return Some(fingerprint);
        }
    }
    if expression.kind == "UnaryOp" && expression.operator == "LogicalNot" {
        let operand = expression.operands.first()?;
        let operand_fingerprint =
            slang_expression_fingerprint_with_parameters(operand, parameters, mode)?;
        return Some(format!("Not:1:u[OrR:1:u[{operand_fingerprint}]]"));
    }
    if expression.kind == "Replication" {
        let count = expression.repeat_count?;
        let operand = expression.operands.first()?;
        let operand_fingerprint =
            slang_expression_fingerprint_with_parameters(operand, parameters, mode)?;
        return Some(format!("Repeat:{count}:u[{operand_fingerprint}]"));
    }
    if expression.kind == "Conversion" {
        return conversion_fingerprint(expression, parameters, mode);
    }
    if expression.kind == "BinaryOp"
        && expression
            .operands
            .first()
            .zip(expression.operands.get(1))
            .is_some_and(|(left, right)| left.type_name == "string" || right.type_name == "string")
    {
        let left = slang_expression_fingerprint_with_parameters(
            expression.operands.first()?,
            parameters,
            mode,
        )?;
        let right = slang_expression_fingerprint_with_parameters(
            expression.operands.get(1)?,
            parameters,
            mode,
        )?;
        let comparison = format!("StringCompare:32:s[CaseSensitive:{left},{right}]");
        let root = slang_expression_shape(expression)?;
        return Some(format!(
            "{root}:1:u[{comparison},Const:32:u[32:{}]]",
            "0".repeat(32)
        ));
    }
    if expression.kind == "BinaryOp" {
        if let Some(fingerprint) = binary_composition_fingerprint(expression, parameters, mode) {
            return Some(fingerprint);
        }
        if let Some(fingerprint) = wildcard_equality_fingerprint(expression, parameters, mode) {
            return Some(fingerprint);
        }
    }
    if expression.kind == "Inside" {
        if let Some(fingerprint) = inside_fingerprint(expression, parameters, mode) {
            return Some(fingerprint);
        }
    }
    if matches!(expression.kind.as_str(), "NamedValue" | "HierarchicalValue")
        && expression.enum_definition.is_some()
    {
        let definition = expression.enum_definition.as_ref()?;
        return enum_name_receiver_fingerprint(expression, definition, parameters, mode);
    }
    if expression.kind == "MemberAccess" {
        if let Some((msb, lsb)) = expression.member_range {
            let width = expression.type_width?;
            let base = slang_expression_fingerprint_with_parameters(
                expression.operands.first()?,
                parameters,
                mode,
            )?;
            return Some(format!("PackedSelect:{width}:u[{base},Range:{msb}:{lsb}]"));
        } else if let Some(path) = expression.interface_path.as_deref() {
            let width = expression.type_width?;
            let signed = expression.type_signed.unwrap_or(false);
            let signed_label = if signed { "s" } else { "u" };
            return Some(format!("Ref:{width}:{signed_label}[{path}]"));
        }
    }
    if expression.kind == "HierarchicalValue" {
        if let Some(path) = expression.interface_path.as_deref() {
            let width = expression.type_width?;
            let signed = expression.type_signed.unwrap_or(false);
            let signed_label = if signed { "s" } else { "u" };
            return Some(format!("Ref:{width}:{signed_label}[{path}]"));
        }
    }
    let root = slang_expression_shape(expression)?;
    let enum_definition = expression.enum_definition.as_ref();
    let width = expression
        .type_width
        .or_else(|| enum_definition.map(|definition| definition.width))?;
    let signed = if mode == ExpressionFingerprintMode::Direct
        && matches!(
            expression.kind.as_str(),
            "BinaryOp" | "ConditionalOp" | "Replication"
        ) {
        direct_lowered_signed(expression, parameters)
            .or(expression.type_signed)
            .or_else(|| enum_definition.map(|definition| definition.signed))?
    } else {
        expression
            .type_signed
            .or_else(|| enum_definition.map(|definition| definition.signed))?
    };
    let signed_label = if signed { "s" } else { "u" };
    if root == "Ref" {
        let symbol = expression.symbol.as_deref()?;
        if let Some(parameter) = parameters.iter().find(|parameter| parameter.name == symbol) {
            return parameter_constant_fingerprint(parameter);
        }
        return Some(format!("Ref:{width}:{signed_label}[{symbol}]"));
    }
    if root == "Const" {
        let literal = expression.literal.as_deref()?;
        return canonical_literal_fingerprint(width, signed, literal);
    }
    let operands = expression
        .operands
        .iter()
        .map(|operand| slang_expression_fingerprint_with_parameters(operand, parameters, mode))
        .collect::<Option<Vec<_>>>()?;
    Some(format!(
        "{}:{width}:{signed_label}[{}]",
        root,
        operands
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(",")
    ))
}

pub(crate) fn collect_kinds(value: &Value, kinds: &mut std::collections::BTreeSet<String>) {
    if let Some(kind) = value.get("kind").and_then(Value::as_str) {
        kinds.insert(kind.to_string());
    }
    match value {
        Value::Array(values) => values.iter().for_each(|value| collect_kinds(value, kinds)),
        Value::Object(fields) => fields
            .values()
            .for_each(|value| collect_kinds(value, kinds)),
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_generate_summary(
    value: &Value,
    prefix: &[String],
    parameters: &[SlangParameterSummary],
    instances: &mut Vec<SlangInstanceSummary>,
    continuous_targets: &mut Vec<SlangContinuousTarget>,
    procedural_blocks: &mut Vec<SlangProceduralBlockSummary>,
    class_handles: &std::collections::BTreeSet<String>,
    class_types: ClassHandleTypes<'_>,
) -> Result<(), String> {
    if string_field(value, "kind").as_deref() == Some("GenerateBlockArray") {
        let array_name = string_field(value, "name")
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "slang GenerateBlockArray has no name".to_string())?;
        let array_members = value
            .get("members")
            .and_then(Value::as_array)
            .ok_or_else(|| "slang GenerateBlockArray has no members".to_string())?;
        for (position, member) in array_members.iter().enumerate() {
            let kind = string_field(member, "kind").unwrap_or_default();
            if kind == "Genvar" || bool_field(member, "isUninstantiated") {
                continue;
            }
            if !kind.starts_with("GenerateBlock") {
                continue;
            }
            let construct_index = member
                .get("constructIndex")
                .and_then(Value::as_u64)
                .unwrap_or(position as u64);
            let mut nested_prefix = prefix.to_vec();
            nested_prefix.push(format!("{array_name}[{construct_index}]"));
            collect_generate_summary(
                member,
                &nested_prefix,
                parameters,
                instances,
                continuous_targets,
                procedural_blocks,
                class_handles,
                class_types,
            )?;
        }
        return Ok(());
    }

    let Some(members) = value.get("members").and_then(Value::as_array) else {
        return Ok(());
    };
    let mut working_prefix = prefix.to_vec();
    let mut scoped_parameters = parameters.to_vec();
    if string_field(value, "kind").as_deref() == Some("GenerateBlock") {
        if let Some(name) = string_field(value, "name").filter(|name| !name.is_empty()) {
            working_prefix.push(name);
        }
        for member in members {
            if string_field(member, "kind").as_deref() == Some("Parameter") {
                let parameter = parameter_summary(member)?;
                if let Some(existing) = scoped_parameters
                    .iter_mut()
                    .find(|existing| existing.name == parameter.name)
                {
                    *existing = parameter;
                } else {
                    scoped_parameters.push(parameter);
                }
            }
        }
    }
    for member in members {
        let Some(kind) = string_field(member, "kind") else {
            continue;
        };
        match kind.as_str() {
            "Instance" => {
                let instance_name = string_field(member, "name")
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| "slang AST Instance has no name".to_string())?;
                let instance_name = working_prefix
                    .iter()
                    .chain(std::iter::once(&instance_name))
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(".");
                let definition_name = member
                    .get("body")
                    .and_then(|body| string_field(body, "name"))
                    .unwrap_or_default();
                let parameters = member
                    .get("body")
                    .map(instance_parameters)
                    .transpose()?
                    .unwrap_or_default();
                instances.push(SlangInstanceSummary {
                    instance_name,
                    definition_name,
                    parameters,
                });
            }
            "InstanceArray" => {
                let array_name = string_field(member, "name")
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| "slang AST InstanceArray has no name".to_string())?;
                let array_prefix = working_prefix
                    .iter()
                    .chain(std::iter::once(&array_name))
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(".");
                super::collect_instance_array_summary(member, &array_prefix, instances)?;
            }
            "ContinuousAssign" => {
                let Some(assignment) = member.get("assignment") else {
                    continue;
                };
                let Some(left) = assignment.get("left") else {
                    continue;
                };
                let rhs_value = assignment
                    .get("right")
                    .map(|rhs| materialize_generate_constants(rhs, &scoped_parameters));
                let rhs = rhs_value
                    .as_ref()
                    .map(expression_summary)
                    .unwrap_or_else(empty_expression_summary);
                for name in expression_target_names(left) {
                    continuous_targets.push(SlangContinuousTarget {
                        name,
                        source: "ContinuousAssign".into(),
                        rhs: rhs.clone(),
                    });
                }
            }
            "ProceduralBlock" => {
                let procedure_kind = string_field(member, "procedureKind").unwrap_or_default();
                let assignments = procedural_assignments(member, class_handles, class_types);
                let assignment_targets = assignments
                    .iter()
                    .flat_map(|assignment| assignment.target_names.iter().cloned())
                    .collect();
                procedural_blocks.push(SlangProceduralBlockSummary {
                    procedure_kind,
                    assignment_targets,
                    assignments,
                });
            }
            nested if nested.starts_with("GenerateBlock") => {
                collect_generate_summary(
                    member,
                    &working_prefix,
                    &scoped_parameters,
                    instances,
                    continuous_targets,
                    procedural_blocks,
                    class_handles,
                    class_types,
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn materialize_generate_constants(value: &Value, parameters: &[SlangParameterSummary]) -> Value {
    let mut materialized = value.clone();
    if let Some(fields) = materialized.as_object_mut() {
        if fields.get("kind").and_then(Value::as_str) == Some("NamedValue") {
            let symbol = fields
                .get("symbol")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let name = symbol.split_whitespace().next_back().unwrap_or_default();
            if let Some(parameter) = parameters.iter().find(|parameter| parameter.name == name) {
                fields
                    .entry("constant")
                    .or_insert_with(|| Value::String(parameter.value.clone()));
            }
        }
        for nested in fields.values_mut() {
            *nested = materialize_generate_constants(nested, parameters);
        }
    } else if let Some(values) = materialized.as_array_mut() {
        for nested in values {
            *nested = materialize_generate_constants(nested, parameters);
        }
    }
    materialized
}

pub(crate) fn design_lvalue_name(design: &Design, lvalue: &eda_core::ir::LValue) -> Option<String> {
    design
        .net_decls
        .iter()
        .find(|declaration| {
            lvalue.net >= declaration.base && lvalue.net < declaration.base + declaration.width
        })
        .map(|declaration| canonical_ref_symbol_name(&declaration.name))
}

pub(crate) fn design_expr_shape(expression: &eda_core::ir::Expr) -> &'static str {
    use eda_core::ir::{BinOp, Expr, UnOp};
    match expression {
        Expr::Const(_) => "Const",
        Expr::Ref { .. } | Expr::MemoryRead { .. } | Expr::MemoryReadPart { .. } => "Ref",
        Expr::Un { op, .. } => match op {
            UnOp::Not => "Not",
            UnOp::AndR => "AndR",
            UnOp::OrR => "OrR",
            UnOp::XorR => "XorR",
        },
        Expr::Bin { op, .. } => match op {
            BinOp::Add => "Add",
            BinOp::Sub => "Sub",
            BinOp::Mul => "Mul",
            BinOp::Pow => "Pow",
            BinOp::Div => "Div",
            BinOp::Mod => "Mod",
            BinOp::And => "And",
            BinOp::Or => "Or",
            BinOp::Xor => "Xor",
            BinOp::Shl => "Shl",
            BinOp::Shr => "Shr",
            BinOp::Eq => "Eq",
            BinOp::Ne => "Ne",
            BinOp::Lt => "Lt",
            BinOp::Le => "Le",
            BinOp::Gt => "Gt",
            BinOp::Ge => "Ge",
            BinOp::LogAnd => "LogAnd",
            BinOp::LogOr => "LogOr",
            BinOp::CaseEq => "CaseEq",
            BinOp::CaseNe => "CaseNe",
        },
        Expr::Cond { .. } => "Cond",
        Expr::Concat(_) => "Concat",
        Expr::Repeat { .. } => "Repeat",
        Expr::Signed { .. } => "Signed",
        Expr::Index { .. } => "Index",
        Expr::StringLength { .. } => "StringLength",
        Expr::StringConcat { .. } => "StringConcat",
        Expr::StringGetByte { .. } => "StringGetByte",
        Expr::StringCase { .. } => "StringCase",
        Expr::StringCompare { .. } => "StringCompare",
        Expr::StringPutc { .. } => "StringPutc",
        Expr::StringSubstr { .. } => "StringSubstr",
        Expr::StringToRadix { .. } => "StringToRadix",
        Expr::StringFromRadix { .. } => "StringFromRadix",
        Expr::EventTriggered { .. } => "EventTriggered",
        Expr::SimulationTime { .. } => "SimulationTime",
        Expr::MemorySize { .. } => "MemorySize",
        Expr::AssocRead { .. } => "AssocRead",
        Expr::AssocExists { .. } => "AssocExists",
    }
}

pub(crate) fn design_expr_fingerprint(design: &Design, expression: &eda_core::ir::Expr) -> String {
    use eda_core::ir::Expr;
    if let Expr::MemoryRead {
        memory,
        indexes,
        width,
        signed,
    } = expression
    {
        let memory_name = design
            .memories
            .get(*memory)
            .map(|memory| memory.name.as_str())
            .unwrap_or("<unknown>");
        let index_fingerprints = indexes
            .iter()
            .map(|index| design_expr_fingerprint(design, index))
            .collect::<Vec<_>>();
        let signed_label = if *signed { "s" } else { "u" };
        return format!(
            "MemoryRead:{width}:{signed_label}[{memory_name},{}]",
            index_fingerprints.join(",")
        );
    }
    if let Expr::MemoryReadPart {
        memory,
        indexes,
        start,
        width,
        ascending,
    } = expression
    {
        let memory_name = design
            .memories
            .get(*memory)
            .map(|memory| memory.name.as_str())
            .unwrap_or("<unknown>");
        let index_fingerprints = indexes
            .iter()
            .map(|index| design_expr_fingerprint(design, index))
            .chain(std::iter::once(design_expr_fingerprint(design, start)))
            .collect::<Vec<_>>();
        let direction = if *ascending {
            "Ascending"
        } else {
            "Descending"
        };
        return format!(
            "MemoryReadPart:{width}:u[{memory_name},{},{direction}]",
            index_fingerprints.join(",")
        );
    }
    if let Expr::Index { e, msb, lsb } = expression {
        let base = design_expr_fingerprint(design, e);
        return format!(
            "PackedSelect:{}:u[{base},Range:{msb}:{lsb}]",
            expression.width()
        );
    }
    if let Expr::Ref {
        net: _,
        decl_width,
        range,
        signed,
    } = expression
    {
        let is_full_range = match range {
            eda_core::ir::Range::Bit(bit) => *decl_width == 1 && *bit == 0,
            eda_core::ir::Range::Span { msb, lsb } => {
                *lsb == 0 && *msb == decl_width.saturating_sub(1)
            }
        };
        if !is_full_range {
            if let Some(name) = design_expr_symbol(design, expression) {
                let signed_label = if *signed { "s" } else { "u" };
                let base = format!("Ref:{decl_width}:{signed_label}[{name}]");
                return match range {
                    eda_core::ir::Range::Bit(bit) => {
                        format!("PackedSelect:1:{signed_label}[{base},Range:{bit}:{bit}]")
                    }
                    eda_core::ir::Range::Span { msb, lsb } => format!(
                        "PackedSelect:{}:{signed_label}[{base},Range:{msb}:{lsb}]",
                        msb - lsb + 1
                    ),
                };
            }
        }
    }
    let root = design_expr_shape(expression);
    if root == "Ref" {
        if let Some(name) = design_expr_symbol(design, expression) {
            return format!(
                "Ref:{}:{}[{name}]",
                expression.width(),
                if expression.is_signed() { "s" } else { "u" }
            );
        }
    }
    if let Expr::Const(word) = expression {
        return format!(
            "Const:{}:u[{}:{}]",
            word.width(),
            word.width(),
            word.to_bin_str()
        );
    }
    if let Expr::Signed {
        e: signed_operand,
        signed: true,
    } = expression
    {
        if let Expr::Const(word) = signed_operand.as_ref() {
            let bits = word.to_bin_str();
            let signed_label = if bits.starts_with('1') { "s" } else { "u" };
            return format!(
                "Const:{}:{signed_label}[{}:{}]",
                word.width(),
                word.width(),
                bits
            );
        }
    }
    if let Expr::Repeat { times, e } = expression {
        return format!("Repeat:{times}:u[{}]", design_expr_fingerprint(design, e));
    }
    if let Expr::StringCase { e, upper } = expression {
        let direction = if *upper { "Upper" } else { "Lower" };
        return format!(
            "StringCase:{}:u[{}:{}]",
            expression.width(),
            direction,
            design_expr_fingerprint(design, e)
        );
    }
    if let Expr::StringCompare {
        left,
        right,
        ignore_case,
    } = expression
    {
        let mode = if *ignore_case {
            "IgnoreCase"
        } else {
            "CaseSensitive"
        };
        return format!(
            "StringCompare:{}:s[{}:{},{}]",
            expression.width(),
            mode,
            design_expr_fingerprint(design, left),
            design_expr_fingerprint(design, right)
        );
    }
    if let Expr::StringToRadix { e, radix } = expression {
        return format!(
            "StringToRadix:{}:s[Radix{radix}:{}]",
            expression.width(),
            design_expr_fingerprint(design, e)
        );
    }
    if let Expr::SimulationTime {
        width,
        signed,
        unit_exponent,
    } = expression
    {
        let signed_label = if *signed { "s" } else { "u" };
        return format!("SimulationTime:{width}:{signed_label}[Unit{unit_exponent}]");
    }
    if let Expr::MemorySize {
        memory,
        width,
        signed,
    } = expression
    {
        let memory_name = design
            .memories
            .get(*memory)
            .map(|memory| memory.name.as_str())
            .unwrap_or("<unknown>");
        let signed_label = if *signed { "s" } else { "u" };
        return format!("MemorySize:{width}:{signed_label}[{memory_name}]");
    }
    if let Expr::AssocRead {
        memory,
        index,
        width,
    } = expression
    {
        let memory_name = design
            .memories
            .get(*memory)
            .map(|memory| memory.name.as_str())
            .unwrap_or("<unknown>");
        return format!(
            "AssocRead:{width}:u[{memory_name},{}]",
            design_expr_fingerprint(design, index)
        );
    }
    if let Expr::AssocExists { memory, index } = expression {
        let memory_name = design
            .memories
            .get(*memory)
            .map(|memory| memory.name.as_str())
            .unwrap_or("<unknown>");
        return format!(
            "AssocExists:1:u[{memory_name},{}]",
            design_expr_fingerprint(design, index)
        );
    }
    let operands: Vec<String> = match expression {
        Expr::Const(_)
        | Expr::Ref { .. }
        | Expr::MemoryRead { .. }
        | Expr::SimulationTime { .. }
        | Expr::MemorySize { .. } => Vec::new(),
        Expr::AssocRead { index, .. } | Expr::AssocExists { index, .. } => {
            vec![design_expr_fingerprint(design, index)]
        }
        Expr::MemoryReadPart { indexes, start, .. } => indexes
            .iter()
            .chain(std::iter::once(start.as_ref()))
            .map(|operand| design_expr_fingerprint(design, operand))
            .collect(),
        Expr::Un { a, .. } => vec![design_expr_fingerprint(design, a)],
        Expr::Bin { a, b, .. } => {
            vec![
                design_expr_fingerprint(design, a),
                design_expr_fingerprint(design, b),
            ]
        }
        Expr::Cond { c, t, f } => vec![
            design_expr_fingerprint(design, c),
            design_expr_fingerprint(design, t),
            design_expr_fingerprint(design, f),
        ],
        Expr::Concat(parts) => parts
            .iter()
            .map(|operand| design_expr_fingerprint(design, operand))
            .collect(),
        Expr::Repeat { e, .. } => vec![design_expr_fingerprint(design, e)],
        Expr::Signed { e, .. } => vec![design_expr_fingerprint(design, e)],
        Expr::Index { e, .. } => vec![design_expr_fingerprint(design, e)],
        Expr::StringLength { e, .. } => vec![design_expr_fingerprint(design, e)],
        Expr::StringConcat { left, right } => vec![
            design_expr_fingerprint(design, left),
            design_expr_fingerprint(design, right),
        ],
        Expr::StringGetByte { e, index } => vec![
            design_expr_fingerprint(design, e),
            design_expr_fingerprint(design, index),
        ],
        Expr::StringCase { e, .. } => vec![design_expr_fingerprint(design, e)],
        Expr::StringCompare { left, right, .. } => vec![
            design_expr_fingerprint(design, left),
            design_expr_fingerprint(design, right),
        ],
        Expr::StringPutc { e, index, value } => vec![
            design_expr_fingerprint(design, e),
            design_expr_fingerprint(design, index),
            design_expr_fingerprint(design, value),
        ],
        Expr::StringSubstr { e, left, right } => vec![
            design_expr_fingerprint(design, e),
            design_expr_fingerprint(design, left),
            design_expr_fingerprint(design, right),
        ],
        Expr::StringToRadix { e, .. } => vec![design_expr_fingerprint(design, e)],
        Expr::StringFromRadix { value, .. } => vec![design_expr_fingerprint(design, value)],
        Expr::EventTriggered { net } => vec![format!("net:{net}")],
    };
    format!(
        "{}:{}:{}[{}]",
        root,
        expression.width(),
        if expression.is_signed() { "s" } else { "u" },
        operands
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(",")
    )
}

pub(crate) fn design_expr_symbol(
    design: &Design,
    expression: &eda_core::ir::Expr,
) -> Option<String> {
    use eda_core::ir::Expr;
    match expression {
        Expr::Ref { net, .. } => clocking_sample_symbol(design, *net).or_else(|| {
            design
                .net_decls
                .iter()
                .find(|declaration| {
                    *net >= declaration.base && *net < declaration.base + declaration.width
                })
                .map(|declaration| canonical_ref_symbol_name(&declaration.name))
        }),
        Expr::MemoryRead { memory, .. } | Expr::MemoryReadPart { memory, .. } => design
            .memories
            .get(*memory)
            .map(|memory| memory.name.clone()),
        _ => None,
    }
}

fn canonical_ref_symbol_name(name: &str) -> String {
    if let Some(local) = name.strip_prefix("__class_static_") {
        let skip_address = local.find('_').map(|end| end + 1);
        if let Some(start) = skip_address {
            let tail = &local[start..];
            if let Some(class_end) = tail.find('@') {
                let after_class_address = &tail[class_end + 1..];
                let property = after_class_address
                    .find('_')
                    .map(|end| &after_class_address[end + 1..])
                    .unwrap_or(after_class_address);
                return format!("{}_{property}", &tail[..class_end]);
            }
            return tail.to_string();
        }
    }
    name.strip_prefix("__local_")
        .and_then(|local| local.rsplit('_').next())
        .map(str::to_string)
        .unwrap_or_else(|| name.to_string())
}

fn clocking_sample_symbol(design: &Design, net: usize) -> Option<String> {
    for block in &design.clocking_blocks {
        for input in &block.inputs {
            let sample_width = input.sample.range.width();
            if net < input.sample.net || net >= input.sample.net + sample_width {
                continue;
            }
            let offset = net - input.sample.net;
            let target_net = match input.target.range {
                eda_core::ir::Range::Bit(bit) => input.target.net + bit + offset,
                eda_core::ir::Range::Span { lsb, .. } => input.target.net + lsb + offset,
            };
            return Some(design.net_name(target_net));
        }
    }
    None
}

pub(crate) fn design_stmt_assignments(
    design: &Design,
    statement: &eda_core::ir::Stmt,
) -> Vec<(String, String)> {
    design_stmt_assignments_guarded(design, statement, None)
}

fn combine_assignment_guards(existing: Option<&str>, condition: &str) -> String {
    match existing {
        Some(existing) => format!("And:1:u[{existing},{condition}]"),
        None => condition.to_string(),
    }
}

fn design_stmt_assignments_guarded(
    design: &Design,
    statement: &eda_core::ir::Stmt,
    guard: Option<&str>,
) -> Vec<(String, String)> {
    use eda_core::ir::Stmt;
    let mut assignments = Vec::new();
    match statement {
        Stmt::Block(statements) => {
            if let Some(copy) = class_handle_copy_block(design, statements) {
                assignments.push(copy);
            } else {
                statements.iter().for_each(|nested| {
                    assignments.extend(design_stmt_assignments_guarded(design, nested, guard))
                });
            }
        }

        Stmt::Forever(body) | Stmt::Delay { body, .. } => {
            assignments.extend(design_stmt_assignments_guarded(design, body, guard));
        }
        Stmt::Wait { body, .. } => {
            assignments.extend(design_stmt_assignments_guarded(design, body, guard));
        }
        Stmt::EventWait { body, .. } => {
            assignments.extend(design_stmt_assignments_guarded(design, body, guard));
        }
        Stmt::WaitOrder {
            success, failure, ..
        } => {
            if let Some(statement) = success {
                assignments.extend(design_stmt_assignments_guarded(design, statement, guard));
            }
            if let Some(statement) = failure {
                assignments.extend(design_stmt_assignments_guarded(design, statement, guard));
            }
        }
        Stmt::Fork { branches, .. } => branches.iter().for_each(|branch| {
            assignments.extend(design_stmt_assignments_guarded(design, branch, guard))
        }),
        Stmt::While { body, steps, .. } => {
            assignments.extend(design_stmt_assignments_guarded(design, body, guard));
            steps.iter().for_each(|step| {
                assignments.extend(design_stmt_assignments_guarded(design, step, guard))
            });
        }
        Stmt::Repeat { body, .. } => {
            assignments.extend(design_stmt_assignments_guarded(design, body, guard));
        }
        Stmt::Force { .. } | Stmt::Release { .. } => {}
        Stmt::Break | Stmt::Continue | Stmt::ReturnBarrier | Stmt::Return => {}
        Stmt::Scope(body) => {
            assignments.extend(design_stmt_assignments_guarded(design, body, guard));
        }
        Stmt::NBl { lv, e } | Stmt::Bl { lv, e } => {
            if let Some(name) = design_lvalue_name(design, lv) {
                assignments.push((name, design_expr_fingerprint(design, e)));
            }
        }
        Stmt::MemoryBl { memory, e, .. }
        | Stmt::MemoryNBl { memory, e, .. }
        | Stmt::MemoryBlPart { memory, e, .. }
        | Stmt::MemoryNBlPart { memory, e, .. } => {
            assignments.extend(
                design
                    .memories
                    .get(*memory)
                    .map(|memory| (memory.name.clone(), design_expr_fingerprint(design, e))),
            );
        }
        Stmt::MemoryCopy {
            source,
            destination,
            ..
        } => {
            assignments.extend(design.memories.get(*destination).map(|destination| {
                let source = design
                    .memories
                    .get(*source)
                    .map(|source| source.name.as_str())
                    .unwrap_or("<unknown>");
                (destination.name.clone(), format!("MemoryCopy:{source}"))
            }));
        }
        Stmt::AssocDelete { .. } => {}
        Stmt::AssocTraversal {
            memory,
            operation,
            key,
            destination,
        } => {
            let Some(declaration) = design.memories.get(*memory) else {
                return Vec::new();
            };
            let operation = match operation {
                eda_core::ir::AssocTraversalOp::First => "First",
                eda_core::ir::AssocTraversalOp::Last => "Last",
                eda_core::ir::AssocTraversalOp::Next => "Next",
                eda_core::ir::AssocTraversalOp::Prev => "Prev",
            };
            let signed_label = if declaration.index_signed { "s" } else { "u" };
            let key_name =
                design_lvalue_name(design, key).unwrap_or_else(|| "<unknown>".to_string());
            if let Some(destination_name) = design_lvalue_name(design, destination) {
                assignments.push((
                    destination_name,
                    format!(
                        "AssocTraversal:{operation}:u[{},Ref:{}:{signed_label}[{}]]",
                        declaration.name, declaration.index_width, key_name
                    ),
                ));
            }
        }
        Stmt::MemoryNew { memory, size, .. } => {
            assignments.extend(design.memories.get(*memory).map(|memory| {
                (
                    memory.name.clone(),
                    format!("MemoryNew:{}", design_expr_fingerprint(design, size)),
                )
            }));
        }
        Stmt::MemoryDelete { memory } => {
            assignments.extend(
                design
                    .memories
                    .get(*memory)
                    .map(|memory| (memory.name.clone(), "MemoryDelete".to_string())),
            );
        }
        Stmt::QueuePush { memory, value, .. } => {
            assignments.extend(design.memories.get(*memory).map(|memory| {
                (
                    memory.name.clone(),
                    format!("QueuePush:{}", design_expr_fingerprint(design, value)),
                )
            }));
        }
        Stmt::QueuePop {
            memory,
            destination,
            front,
        } => {
            if let Some(destination_name) = design_lvalue_name(design, destination) {
                assignments.push((
                    destination_name,
                    format!(
                        "QueuePop:{front}:{}",
                        design
                            .memories
                            .get(*memory)
                            .map(|memory| memory.name.as_str())
                            .unwrap_or("<unknown>")
                    ),
                ));
            }
        }
        Stmt::QueueInsert {
            memory,
            index,
            value,
        } => {
            assignments.extend(design.memories.get(*memory).map(|memory| {
                (
                    memory.name.clone(),
                    format!(
                        "QueueInsert:{},{}",
                        design_expr_fingerprint(design, index),
                        design_expr_fingerprint(design, value)
                    ),
                )
            }));
        }
        Stmt::QueueDelete { memory, index } => {
            assignments.extend(design.memories.get(*memory).map(|memory| {
                (
                    memory.name.clone(),
                    format!("QueueDelete:{}", design_expr_fingerprint(design, index)),
                )
            }));
        }
        Stmt::AssocWrite { memory, value, .. } => {
            assignments.extend(
                design
                    .memories
                    .get(*memory)
                    .map(|memory| (memory.name.clone(), design_expr_fingerprint(design, value))),
            );
        }
        Stmt::BlIndexed {
            net,
            base_offset,
            element_width,
            start,
            width,
            reversed,
            e,
            ..
        }
        | Stmt::NBlIndexed {
            net,
            base_offset,
            element_width,
            start,
            width,
            reversed,
            e,
            ..
        } => {
            if let Some(declaration) = design.net_decls.iter().find(|declaration| {
                *net >= declaration.base && *net < declaration.base + declaration.width
            }) {
                let body = format!(
                    "{},Element:{element_width},Offset:{base_offset},Reversed:{reversed},{},{}",
                    canonical_ref_symbol_name(&declaration.name),
                    design_expr_fingerprint(design, start),
                    design_indexed_value_fingerprint(design, e)
                );
                let shape = if let Some(guard) = guard {
                    format!("GuardedArrayPartWrite:{width}:u[{guard},{body}]")
                } else {
                    format!("ArrayPartWrite:{width}:u[{body}]")
                };
                assignments.push((canonical_ref_symbol_name(&declaration.name), shape));
            }
        }
        Stmt::If { c, t, f } => {
            let condition = design_expr_fingerprint(design, c);
            let true_guard = combine_assignment_guards(guard, &condition);
            let false_condition = format!("Not:1:u[{condition}]");
            let false_guard = combine_assignment_guards(guard, &false_condition);
            assignments.extend(design_stmt_assignments_guarded(
                design,
                t,
                Some(&true_guard),
            ));
            if let Some(f) = f {
                assignments.extend(design_stmt_assignments_guarded(
                    design,
                    f,
                    Some(&false_guard),
                ));
            }
        }
        Stmt::Case { arms, dflt, .. } => {
            for (_, body) in arms {
                assignments.extend(design_stmt_assignments_guarded(design, body, guard));
            }
            if let Some(body) = dflt {
                assignments.extend(design_stmt_assignments_guarded(design, body, guard));
            }
        }
        Stmt::Display { .. }
        | Stmt::Assert { .. }
        | Stmt::Assume { .. }
        | Stmt::Cover { .. }
        | Stmt::TimeFormat { .. }
        | Stmt::WriteMem { .. }
        | Stmt::Finish => {}
    }
    assignments
}

fn design_indexed_value_fingerprint(design: &Design, expression: &eda_core::ir::Expr) -> String {
    use eda_core::ir::{Expr, Range};

    let Expr::Ref {
        decl_width, range, ..
    } = expression
    else {
        return design_expr_fingerprint(design, expression);
    };
    let is_full_range = match range {
        Range::Bit(bit) => *decl_width == 1 && *bit == 0,
        Range::Span { msb, lsb } => *lsb == 0 && *msb == decl_width.saturating_sub(1),
    };
    if is_full_range {
        return design_expr_fingerprint(design, expression);
    }
    let Some(name) = design_expr_symbol(design, expression) else {
        return design_expr_fingerprint(design, expression);
    };
    let signed_label = if expression.is_signed() { "s" } else { "u" };
    match range {
        Range::Bit(bit) => {
            format!("PackedSelect:1:u[Ref:{decl_width}:{signed_label}[{name}],Range:{bit}:{bit}]")
        }
        Range::Span { msb, lsb } => {
            let width = msb - lsb + 1;
            format!(
                "PackedSelect:{width}:u[Ref:{decl_width}:{signed_label}[{name}],Range:{msb}:{lsb}]"
            )
        }
    }
}
