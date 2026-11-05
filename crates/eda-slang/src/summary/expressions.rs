//! Expression summaries, fingerprints, and IR-shape comparison helpers.

use crate::ast_util::{
    bool_field, constant_system_real, expression_symbol, expression_symbol_id,
    expression_target_names, instance_parameters, integer_literal_word, literal_word,
    named_constant_word, parameter_summary, slang_constant_u64, string_field, string_literal,
    unwrap_conversion,
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
    if let Some(summary) = constant_typename_summary(value) {
        return summary;
    }
    if let Some(summary) = constant_isunbounded_summary(value) {
        return summary;
    }
    if let Some(summary) = constant_real_math_summary(value) {
        return summary;
    }
    if let Some(summary) = constant_clog2_summary(value) {
        return summary;
    }
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
    let type_signed = if kind == "RealLiteral" || (kind == "NamedValue" && literal.is_some()) {
        Some(false)
    } else {
        string_field(value, "type").and_then(|type_name| simple_type_signed(&type_name))
    };
    let type_width = if kind == "RealLiteral" {
        Some(64)
    } else {
        string_field(value, "type").and_then(|type_name| simple_type_width(&type_name))
    };
    let mut type_width = type_width;
    let mut type_signed = type_signed;
    let mut symbol = expression_symbol(value);
    if kind == "Call"
        && operator
            .as_deref()
            .is_some_and(|name| name.split_whitespace().next_back() == Some("self"))
        && string_field(value, "type")
            .is_some_and(|type_name| type_name.split_whitespace().next_back() == Some("process"))
    {
        type_width = Some(64);
        type_signed = Some(false);
    }
    if kind == "Call"
        && operator.as_deref().is_some_and(|name| {
            matches!(
                name.split_whitespace().next_back(),
                Some("kill") | Some("await") | Some("status")
            )
        })
    {
        if let Some(receiver) = value.get("thisClass") {
            if string_field(receiver, "type").is_some_and(|type_name| {
                type_name.split_whitespace().next_back() == Some("process")
            }) {
                symbol = expression_symbol(receiver);
            }
        }
    }
    let plusarg_name = if kind == "Call"
        && matches!(
            operator.as_deref(),
            Some("$test$plusargs") | Some("$value$plusargs")
        ) {
        plusarg_query(value)
    } else {
        None
    };
    SlangExpressionSummary {
        kind,
        type_name: string_field(value, "type").unwrap_or_default(),
        operator: operator.unwrap_or_default(),
        selection_kind: string_field(value, "selectionKind").unwrap_or_default(),
        type_width,
        type_signed,
        symbol,
        symbol_id: expression_symbol_id(value),
        literal,
        repeat_count: repeat_count(value),
        simulation_time_unit: None,
        enum_definition: None,
        member_path,
        member_range: None,
        interface_path: None,
        streaming_slice_size,
        plusarg_name,
        operands: expression_operands(value),
    }
}

fn constant_typename_summary(value: &Value) -> Option<SlangExpressionSummary> {
    if string_field(value, "kind").as_deref() != Some("Call")
        || string_field(value, "subroutine")
            .as_deref()?
            .split_whitespace()
            .next_back()
            != Some("$typename")
    {
        return None;
    }
    let argument = value
        .get("arguments")
        .and_then(Value::as_array)
        .and_then(|arguments| arguments.first())?;
    let argument = unwrap_conversion(argument);
    let type_name = string_field(argument, "type")?;
    simple_type_shape(&type_name)?;
    if type_name.contains('$') {
        return None;
    }
    let word = string_literal_word(&type_name)?;
    let mut summary = empty_expression_summary();
    summary.operands = vec![expression_summary(argument)];
    summary.kind = "StringLiteral".into();
    summary.operator = "$typename".into();
    summary.type_name = "string".into();
    summary.type_width = Some(STRING_WIDTH);
    summary.type_signed = Some(false);
    summary.literal = Some(format!("{STRING_WIDTH}:{}", word.to_bin_str()));
    Some(summary)
}

pub(crate) fn annotate_typename_alias_canonical_names(
    expression: &mut SlangExpressionSummary,
    aliases: &std::collections::BTreeMap<String, String>,
) {
    if expression.kind == "StringLiteral" && expression.operator == "$typename" {
        if let Some(operand) = expression.operands.first() {
            let source_type = operand.type_name.clone();
            if let Some(type_name) = resolve_simple_type_alias_name(&source_type, aliases) {
                if let Some(word) = string_literal_word(&type_name) {
                    expression.literal = Some(format!("{STRING_WIDTH}:{}", word.to_bin_str()));
                    if let Some(operand) = expression.operands.first_mut() {
                        operand.type_name = type_name;
                    }
                }
            }
        }
    }
    for operand in &mut expression.operands {
        annotate_typename_alias_canonical_names(operand, aliases);
    }
}

fn resolve_simple_type_alias_name(
    type_name: &str,
    aliases: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    let type_reference = type_name.split('$').next()?.trim();
    let mut current = if type_reference.is_empty() {
        type_name.to_string()
    } else {
        aliases
            .get(type_reference)
            .cloned()
            .unwrap_or_else(|| type_reference.to_string())
    };
    for _ in 0..64 {
        if simple_type_shape(&current).is_some() {
            return Some(current);
        }
        let scoped_name = current.split_whitespace().next_back()?.to_string();
        let next = aliases
            .get(&scoped_name)
            .or_else(|| {
                let name = scoped_name.rsplit(['.', '#', ':']).next()?;
                aliases.get(name)
            })?
            .clone();
        if next == current {
            return None;
        }
        current = next;
    }
    None
}

fn constant_isunbounded_summary(value: &Value) -> Option<SlangExpressionSummary> {
    if string_field(value, "kind").as_deref() != Some("Call")
        || string_field(value, "subroutine")
            .as_deref()?
            .split_whitespace()
            .next_back()
            != Some("$isunbounded")
    {
        return None;
    }
    let argument = value
        .get("arguments")
        .and_then(Value::as_array)
        .and_then(|arguments| arguments.first())?;
    let argument = unwrap_conversion(argument);
    let unbounded = string_field(argument, "type").as_deref() == Some("$");
    let mut summary = empty_expression_summary();
    summary.kind = "IntegerLiteral".into();
    summary.type_name = "bit".into();
    summary.type_width = Some(1);
    summary.type_signed = Some(false);
    summary.literal = Some(format!("1:{}", if unbounded { "1" } else { "0" }));
    Some(summary)
}

fn constant_real_math_summary(value: &Value) -> Option<SlangExpressionSummary> {
    let result = constant_system_real(value)?;
    let mut summary = empty_expression_summary();
    summary.kind = "RealLiteral".into();
    summary.type_name = "real".into();
    summary.type_width = Some(64);
    summary.type_signed = Some(false);
    summary.literal = Some(result.to_string());
    Some(summary)
}

fn constant_clog2_summary(value: &Value) -> Option<SlangExpressionSummary> {
    if string_field(value, "kind").as_deref() != Some("Call")
        || string_field(value, "subroutine")
            .as_deref()?
            .split_whitespace()
            .next_back()
            != Some("$clog2")
    {
        return None;
    }
    let argument = value
        .get("arguments")
        .and_then(Value::as_array)
        .and_then(|arguments| arguments.first())?;
    let word = integer_literal_word(unwrap_conversion(argument))?;
    let input = word.to_u64()?;
    let result = if input <= 1 {
        0_u32
    } else {
        (u128::from(input) - 1).ilog2() + 1
    };
    let word = LogicWord::from_u64(32, u64::from(result));
    let mut summary = empty_expression_summary();
    summary.kind = "IntegerLiteral".into();
    summary.type_name = "int".into();
    summary.type_width = Some(32);
    summary.type_signed = Some(true);
    summary.literal = Some(format!("32:{}", word.to_bin_str()));
    Some(summary)
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
        plusarg_name: None,
        operands: Vec::new(),
    }
}

pub(crate) fn expression_literal(value: &Value) -> Option<String> {
    let kind = string_field(value, "kind")?;
    let raw = match kind.as_str() {
        "IntegerLiteral" => string_field(value, "value")?,
        "RealLiteral" => {
            let real = value
                .get("value")
                .and_then(Value::as_f64)
                .filter(|real| real.is_finite())?;
            if real.fract() != 0.0 {
                return Some(format!("{real}"));
            }
            if !(real >= 0.0 && real <= u64::MAX as f64) {
                return None;
            }
            return Some(format!(
                "64:{}",
                LogicWord::from_u64(64, real as u64).to_bin_str()
            ));
        }
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

fn plusarg_query(value: &Value) -> Option<String> {
    let argument = value
        .get("arguments")
        .and_then(Value::as_array)
        .and_then(|arguments| arguments.first())?;
    let literal = string_literal(argument)?;
    if string_field(value, "subroutine").as_deref() == Some("$test$plusargs") {
        return Some(literal);
    }
    let percent = literal.find('%')?;
    literal[..percent].strip_suffix('=').map(str::to_string)
}

fn value_plusarg_summary(value: &Value) -> Option<SlangExpressionSummary> {
    let query = plusarg_query(value)?;
    let format = value
        .get("arguments")
        .and_then(Value::as_array)
        .and_then(|arguments| arguments.first())
        .and_then(string_literal)?;
    let percent = format.find('%')?;
    let specifier = format[percent + 1..].chars().next()?;
    let mut summary = empty_expression_summary();
    summary.plusarg_name = Some(query);
    if specifier == 's' {
        summary.kind = "Ref".into();
        summary.type_width = Some(512);
        summary.type_signed = Some(false);
    } else {
        let radix = match specifier {
            'd' => "10",
            'h' => "16",
            'o' => "8",
            'b' => "2",
            _ => return None,
        };
        summary.kind = "StringToRadix".into();
        summary.operator = radix.into();
        summary.type_width = Some(32);
        summary.type_signed = Some(true);
    }
    Some(summary)
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
        "Assignment" => {
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
    if string_field(value, "kind").as_deref() == Some("ProceduralAssign") {
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
    if string_field(value, "kind").as_deref() == Some("Call")
        && string_field(value, "subroutine").as_deref() == Some("$value$plusargs")
    {
        if let Some(target) = value
            .get("arguments")
            .and_then(Value::as_array)
            .and_then(|arguments| arguments.get(1))
            .and_then(|argument| argument.get("left"))
        {
            if let Some(rhs) = value_plusarg_summary(value) {
                assignments.push(SlangAssignmentSummary {
                    target_names: expression_target_names(target),
                    rhs,
                    nonblocking: false,
                    dynamic_packed_lvalue: false,
                    dynamic_packed_lvalue_selector: None,
                    memory_member_lvalue: None,
                    constant_array_element_lvalue: false,
                    dynamic_array_lvalue_indexes: Vec::new(),
                    array_part_lvalue: None,
                    class_handle_source: None,
                    class_handle_target_key: None,
                    class_handle_source_key: None,
                    class_handle_dynamic_sources: Vec::new(),
                    class_handle_dynamic_targets: Vec::new(),
                });
                return assignments;
            }
        }
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
        ("Call", "self")
            if expression.type_name.split_whitespace().next_back() == Some("process") =>
        {
            Some("ProcessSelf")
        }
        ("Call", "status") if expression.symbol.is_some() => Some("ProcessStatus"),
        ("Call", "$past") => Some("Past"),
        ("Call", "$sampled") => Some("Sampled"),
        ("Call", "$rose") => Some("Rose"),
        ("Call", "$fell") => Some("Fell"),
        ("Call", "$stable") => Some("Stable"),
        ("Call", "$changed") => Some("Changed"),
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
        ("BinaryOp", "Mod" | "Modulus") => Some("Mod"),
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

fn expression_arity_is_malformed(expression: &SlangExpressionSummary) -> bool {
    let malformed = match expression.kind.as_str() {
        "UnaryOp" | "Conversion" | "Replication" => expression.operands.len() != 1,
        "BinaryOp" => expression.operands.len() != 2,
        "ConditionalOp" => expression.operands.len() != 3,
        _ => false,
    };
    malformed
        || expression
            .operands
            .iter()
            .any(expression_arity_is_malformed)
}

pub(crate) fn contains_malformed_expression_arity(expression: &SlangExpressionSummary) -> bool {
    expression
        .operands
        .iter()
        .any(expression_arity_is_malformed)
}

pub(crate) fn contains_expected_shapes(actual: &[String], expected: &[String]) -> bool {
    let mut remaining = actual.to_vec();
    for shape in expected {
        let Some(index) = remaining.iter().position(|candidate| candidate == shape) else {
            return false;
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
    if expression.kind == "UnaryOp" && expression.operator == "Minus" {
        return 0_i64.checked_sub(summary_literal_i64(expression.operands.first()?)?);
    }
    let (_, bits) = expression.literal.as_deref()?.split_once(':')?;
    if bits.len() > 64 {
        return None;
    }
    let mut value = 0_i128;
    for bit in bits.chars() {
        value = value.checked_mul(2)?;
        if bit == '1' {
            value = value.checked_add(1)?;
        } else if bit != '0' {
            return None;
        }
    }
    if expression.type_signed == Some(true) && bits.starts_with('1') {
        value -= 1_i128 << bits.len();
    }
    i64::try_from(value).ok()
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

fn type_is_realtime_compatible(type_name: &str) -> bool {
    matches!(type_name.trim(), "real" | "realtime")
}

fn type_is_process_handle(type_name: &str) -> bool {
    type_name.split_whitespace().next_back() == Some("process")
}

fn conversion_fingerprint(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    let operand = expression.operands.first()?;
    if type_is_process_handle(&expression.type_name) && type_is_process_handle(&operand.type_name) {
        return slang_expression_fingerprint_with_parameters(operand, parameters, mode);
    }
    if expression.kind == "Conversion"
        && operand.kind == "Call"
        && operand.operator.split_whitespace().next_back() == Some("status")
        && operand.symbol.is_some()
    {
        return slang_expression_fingerprint_with_parameters(operand, parameters, mode);
    }
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
    if type_is_realtime_compatible(&expression.type_name)
        && type_is_realtime_compatible(&operand.type_name)
    {
        return slang_expression_fingerprint_with_parameters(operand, parameters, mode);
    }
    if expression.type_name.trim() == "shortreal" && operand.type_name.trim() == "shortreal" {
        return slang_expression_fingerprint_with_parameters(operand, parameters, mode);
    }
    if type_is_realtime_compatible(&expression.type_name)
        && !matches!(operand.type_name.trim(), "real" | "realtime" | "shortreal")
    {
        let operand_fingerprint =
            slang_expression_fingerprint_with_parameters(operand, parameters, mode)?;
        return Some(format!("IntToReal:64:s[{operand_fingerprint}]"));
    }
    if expression.type_name.trim() == "shortreal" {
        let operand_fingerprint =
            slang_expression_fingerprint_with_parameters(operand, parameters, mode)?;
        if type_is_realtime_compatible(&operand.type_name) {
            return Some(format!("RealToShortReal:32:s[{operand_fingerprint}]"));
        }
        if operand.type_name.trim() != "shortreal" {
            return Some(format!(
                "RealToShortReal:32:s[IntToReal:64:s[{operand_fingerprint}]]"
            ));
        }
    }
    if operand.type_name.trim() == "shortreal" {
        let operand_fingerprint =
            slang_expression_fingerprint_with_parameters(operand, parameters, mode)?;
        if type_is_realtime_compatible(&expression.type_name) {
            return Some(format!("ShortRealToReal:64:s[{operand_fingerprint}]"));
        }
        if expression.type_name.trim() != "shortreal" {
            let width = expression.type_width?;
            let signed_label = if expression.type_signed? { "s" } else { "u" };
            return Some(format!(
                "RealToInt:{width}:{signed_label}[ShortRealToReal:64:s[{operand_fingerprint}]]"
            ));
        }
    }
    if type_is_realtime_compatible(&operand.type_name)
        && !type_is_realtime_compatible(&expression.type_name)
    {
        let width = expression.type_width?;
        let signed_label = if expression.type_signed? { "s" } else { "u" };
        let operand_fingerprint =
            slang_expression_fingerprint_with_parameters(operand, parameters, mode)?;
        return Some(format!(
            "RealToInt:{width}:{signed_label}[{operand_fingerprint}]"
        ));
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
    let lowered_source_signed = match mode {
        ExpressionFingerprintMode::Direct => direct_lowered_signed(operand, parameters)?,
        ExpressionFingerprintMode::BoundedCompatibility => source_signed,
    };
    if target_width == source_width
        && target_signed == source_signed
        && lowered_source_signed == target_signed
    {
        return Some(operand_fingerprint);
    }
    if target_width == source_width
        && lowered_source_signed != target_signed
        && operand.kind == "IntegerLiteral"
        && target_signed
    {
        let literal = operand.literal.as_deref()?;
        return canonical_literal_fingerprint(target_width, target_signed, literal);
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
            if type_is_realtime_compatible(&expression.type_name) {
                return Some(format!("RealNeg:64:s[{operand_fingerprint}]"));
            }
            if expression.type_name.trim() == "shortreal" {
                return Some(format!("ShortRealNeg:32:s[{operand_fingerprint}]"));
            }
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
        "IntegerLiteral" | "RealLiteral" => expression.type_signed,
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
                "Add" | "Subtract" | "Multiply" | "Divide" | "Mod" | "Modulus" | "Power"
                | "BinaryAnd" | "BinaryOr" | "BinaryXor" | "BitwiseXor" => Some(left && right),
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
        "ElementSelect" | "RangeSelect" => Some(false),
        // slang's result type is authoritative for system/method calls;
        // examples include signed `$size`-family integers and simulation time.
        "Call" => expression.type_signed,
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
        return bounded_string_substr_fingerprint(expression, parameters, mode);
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

fn bounded_string_substr_fingerprint(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    if expression.operator != "substr" {
        return None;
    }
    const BYTE_COUNT: usize = STRING_WIDTH / 8;
    let receiver = expression.operands.first()?;
    if receiver.type_name != "string" {
        return None;
    }
    let receiver = slang_expression_fingerprint_with_parameters(receiver, parameters, mode)?;
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
    let const32 = |value: u32| format!("Const:32:u[32:{value:032b}]");
    let zero_byte = "Const:8:u[8:00000000]".to_string();
    let byte = |index: usize| {
        format!(
            "PackedSelect:8:u[{receiver},Range:{}:{}]",
            index * 8 + 7,
            index * 8
        )
    };

    let mut length = const32(BYTE_COUNT as u32);
    for index in (0..BYTE_COUNT).rev() {
        length = format!(
            "Cond:32:u[Eq:1:u[{},{}],{},{}]",
            byte(index),
            zero_byte,
            const32(index as u32),
            length
        );
    }
    let indexed_byte = |index: String| {
        let mut selected = zero_byte.clone();
        for source in (0..BYTE_COUNT).rev() {
            selected = format!(
                "Cond:8:u[Eq:1:u[{},{}],{},{}]",
                index,
                const32(source as u32),
                byte(source),
                selected
            );
        }
        selected
    };

    let mut bytes = Vec::with_capacity(BYTE_COUNT);
    for output in 0..BYTE_COUNT {
        let source_index = format!("Add:32:u[{},{}]", left, const32(output as u32));
        let in_range = format!(
            "LogAnd:1:u[LogAnd:1:u[Ge:1:u[{},{}],Le:1:u[{},{}]],Lt:1:u[{},{}]]",
            left,
            const32(0),
            source_index,
            right,
            right,
            length
        );
        bytes.push(format!(
            "Cond:8:u[{},{},{}]",
            in_range,
            indexed_byte(source_index),
            zero_byte
        ));
    }
    bytes.reverse();
    Some(format!("Concat:512:u[{}]", bytes.join(",")))
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
    if selected.kind == "DataType" {
        let width = selected.type_width?;
        let bits = format!("{width:032b}");
        return Some(format!("Const:32:u[32:{bits}]"));
    }
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
            let (left, right, increment) = if let Some(dimension) = dimensions.get(physical_index) {
                let length = i64::try_from(dimension.length).ok()?;
                (
                    dimension.base,
                    dimension
                        .base
                        .saturating_add(dimension.step.saturating_mul(length - 1)),
                    -dimension.step,
                )
            } else {
                let dimension = packed_shape.dimensions.get(physical_index)?;
                let increment = if dimension.msb >= dimension.lsb {
                    1
                } else {
                    -1
                };
                (dimension.msb, dimension.lsb, increment)
            };
            let value = match method {
                "$left" => left,
                "$right" => right,
                "$low" => left.min(right),
                "$high" => left.max(right),
                _ => increment,
            };
            const32(value)
        }
        _ => None,
    }
}

fn event_triggered_fingerprint(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    if expression.kind != "Call" || expression.operator != "triggered" {
        return None;
    }
    let receiver = expression.operands.first()?;
    if receiver.kind != "ElementSelect" {
        if receiver.type_name != "event" {
            return None;
        }
        let symbol = receiver.symbol.as_deref()?;
        let event = symbol
            .rsplit_once(' ')
            .map(|(_, event)| event)
            .unwrap_or(symbol);
        return Some(format!("EventTriggered:1:u[{event}]"));
    }

    let array = receiver.operands.first()?;
    let symbol = array.symbol.as_deref()?;
    let event = symbol
        .rsplit_once(' ')
        .map(|(_, event)| event)
        .unwrap_or(symbol);
    let shape = simple_type_shape(&array.type_name)?;
    let selectors = receiver
        .operands
        .iter()
        .skip(1)
        .map(|selector| {
            Some((
                slang_expression_fingerprint_with_parameters(selector, parameters, mode)?,
                selector.type_width?,
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    if selectors.len() != shape.dimensions.len() {
        return None;
    }

    fn append_candidates(
        dimensions: &[AstPackedDimension],
        selectors: &[(String, usize)],
        level: usize,
        offset: usize,
        conditions: Vec<String>,
        candidates: &mut Vec<(usize, Vec<String>)>,
    ) -> Option<()> {
        let Some(dimension) = dimensions.get(level) else {
            candidates.push((offset, conditions));
            return Some(());
        };
        let (selector, comparison_width) = selectors.get(level)?;
        let lower = dimension.msb.min(dimension.lsb);
        let upper = dimension.msb.max(dimension.lsb);
        for logical_index in lower..=upper {
            let coordinate = if dimension.msb >= dimension.lsb {
                logical_index - dimension.lsb
            } else {
                dimension.lsb - logical_index
            };
            let coordinate = usize::try_from(coordinate).ok()?;
            let bits = format!("{logical_index:0comparison_width$b}");
            let condition =
                format!("Eq:1:u[{selector},Const:{comparison_width}:u[{comparison_width}:{bits}]]");
            let mut conditions = conditions.clone();
            conditions.push(condition);
            append_candidates(
                dimensions,
                selectors,
                level + 1,
                offset + coordinate * dimension.element_width,
                conditions,
                candidates,
            )?;
        }
        Some(())
    }

    let mut candidates = Vec::new();
    append_candidates(
        &shape.dimensions,
        &selectors,
        0,
        0,
        Vec::new(),
        &mut candidates,
    )?;
    let mut result = "Const:1:u[1:x]".to_string();
    for (offset, conditions) in candidates {
        let condition = conditions
            .iter()
            .cloned()
            .reduce(|left, right| format!("LogAnd:1:u[{left},{right}]"))?;
        let triggered = format!("EventTriggered:1:u[{event}[{offset}]]");
        result = format!("Cond:1:u[{condition},{triggered},{result}]");
    }
    Some(result)
}

pub(crate) fn slang_expression_fingerprint_with_parameters(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    if expression.kind == "StringToRadix" {
        if let Some(name) = expression.plusarg_name.as_deref() {
            return Some(format!(
                "StringToRadix:32:s[Radix{}:Ref:512:u[{}]]",
                expression.operator,
                eda_core::ir::plusarg_value_net_name(name)
            ));
        }
    }
    if expression.kind == "Ref" && expression.plusarg_name.is_some() {
        let name = expression.plusarg_name.as_deref()?;
        return Some(format!(
            "Ref:512:u[{}]",
            eda_core::ir::plusarg_value_net_name(name)
        ));
    }
    if expression.kind == "Call"
        && expression.operator.split_whitespace().next_back() == Some("self")
        && expression.type_name.split_whitespace().next_back() == Some("process")
    {
        return Some("ProcessSelf:64:u[]".to_string());
    }
    if expression.kind == "Call"
        && expression.operator.split_whitespace().next_back() == Some("status")
    {
        if let Some(receiver) = expression.symbol.as_deref() {
            return Some(format!("ProcessStatus:32:s[Ref:64:u[{receiver}]]"));
        }
    }
    if expression.kind == "Call" && expression.operator == "$past" {
        let width = expression.type_width?;
        let signed = expression.type_signed.unwrap_or(false);
        let value = expression.operands.first().and_then(|operand| {
            slang_expression_fingerprint_with_parameters(operand, parameters, mode)
        })?;
        let ticks = expression
            .operands
            .get(1)
            .and_then(|operand| operand.literal.as_deref())
            .and_then(|literal| literal.rsplit(':').next())
            .and_then(|bits| u32::from_str_radix(bits, 2).ok())
            .unwrap_or(1);
        let mut operands = vec![ticks.to_string(), value];
        if let Some(gating) = expression.operands.get(2) {
            operands.push(slang_expression_fingerprint_with_parameters(
                gating, parameters, mode,
            )?);
        }
        let signed_label = if signed { "s" } else { "u" };
        return Some(format!(
            "Past:{width}:{signed_label}[{}]",
            operands.join(",")
        ));
    }
    if expression.kind == "Call"
        && matches!(
            expression.operator.as_str(),
            "$rose" | "$fell" | "$stable" | "$changed"
        )
    {
        let shape = match expression.operator.as_str() {
            "$rose" => "Rose",
            "$fell" => "Fell",
            "$stable" => "Stable",
            _ => "Changed",
        };
        let value = expression.operands.first().and_then(|operand| {
            slang_expression_fingerprint_with_parameters(operand, parameters, mode)
        })?;
        return Some(format!("{shape}:1:u[{value}]"));
    }
    if expression.kind == "Call"
        && matches!(
            expression.operator.as_str(),
            "$itor"
                | "$rtoi"
                | "$realtobits"
                | "$bitstoreal"
                | "$shortrealtobits"
                | "$bitstoshortreal"
        )
    {
        let operand = expression.operands.first()?;
        let operand_fingerprint =
            slang_expression_fingerprint_with_parameters(operand, parameters, mode)?;
        return Some(match expression.operator.as_str() {
            "$itor" => format!("IntToReal:64:s[{operand_fingerprint}]"),
            "$rtoi" => {
                let width = expression.type_width?;
                let signed_label = if expression.type_signed? { "s" } else { "u" };
                if operand.type_name.trim() == "shortreal" {
                    format!(
                        "RealTruncToInt:{width}:{signed_label}[ShortRealToReal:64:s[{operand_fingerprint}]]"
                    )
                } else {
                    format!("RealTruncToInt:{width}:{signed_label}[{operand_fingerprint}]")
                }
            }
            "$realtobits" => operand_fingerprint,
            "$shortrealtobits" => {
                if type_is_realtime_compatible(&operand.type_name) {
                    format!("RealToShortReal:32:s[{operand_fingerprint}]")
                } else {
                    operand_fingerprint
                }
            }
            "$bitstoreal" => format!("BitsToReal:64:s[{operand_fingerprint}]"),
            _ => format!("BitsToShortReal:32:s[{operand_fingerprint}]"),
        });
    }
    if expression.kind == "Call" && expression.operator == "$sampled" {
        let width = expression.type_width?;
        let signed = expression.type_signed.unwrap_or(false);
        let value = expression.operands.first().and_then(|operand| {
            slang_expression_fingerprint_with_parameters(operand, parameters, mode)
        })?;
        return Some(format!(
            "Sampled:{width}:{}[{value}]",
            if signed { "s" } else { "u" }
        ));
    }
    if expression.kind == "Call" && expression.operator == "$countbits" {
        let value = slang_expression_fingerprint_with_parameters(
            expression.operands.first()?,
            parameters,
            mode,
        )?;
        let mut controls = 0_u8;
        for control in expression.operands.iter().skip(1) {
            let literal = control.literal.as_deref()?;
            let bit = literal.rsplit(':').next()?;
            controls |= match bit {
                "0" => 1,
                "1" => 2,
                "x" => 4,
                "z" => 8,
                _ => return None,
            };
        }
        return Some(format!("CountBits:32:s[Controls{controls},{value}]"));
    }
    if expression.kind == "Call" && expression.operator == "$countones" {
        let value = slang_expression_fingerprint_with_parameters(
            expression.operands.first()?,
            parameters,
            mode,
        )?;
        return Some(format!("CountBits:32:s[Controls2,{value}]"));
    }
    if expression.kind == "Call"
        && matches!(
            expression.operator.as_str(),
            "$isunknown" | "$onehot" | "$onehot0"
        )
    {
        let value = slang_expression_fingerprint_with_parameters(
            expression.operands.first()?,
            parameters,
            mode,
        )?;
        return Some(match expression.operator.as_str() {
            "$isunknown" => format!("IsUnknown:1:u[{value}]"),
            "$onehot" => format!("OneHot:1:u[{value}]"),
            _ => format!("OneHot0:1:u[{value}]"),
        });
    }
    if expression.kind == "RealLiteral" {
        let literal = expression.literal.as_deref()?;
        if let Some(bits) = literal.strip_prefix("64:") {
            return Some(format!("Const:64:u[64:{bits}]"));
        }
        let real = literal.parse::<f64>().ok().or_else(|| {
            expression
                .operands
                .first()
                .and_then(|operand| operand.literal.as_deref())
                .and_then(|literal| literal.parse::<f64>().ok())
        })?;
        if !real.is_finite() {
            return None;
        }
        if real.fract() == 0.0 && real >= 0.0 && real <= u64::MAX as f64 {
            let literal = expression.literal.as_deref()?;
            return Some(format!("Const:64:u[{literal}]"));
        }
        return Some(format!("RealConst:64:f[{real}]"));
    }
    if expression.kind == "Call"
        && matches!(
            expression.operator.as_str(),
            "$test$plusargs" | "$value$plusargs"
        )
    {
        let name = expression.plusarg_name.as_deref()?;
        return Some(format!("Ref:1:u[{}]", eda_core::ir::plusarg_net_name(name)));
    }
    if expression.kind == "Call"
        && matches!(
            expression.operator.as_str(),
            "$random" | "$urandom" | "$urandom_range"
        )
    {
        let signed_label = if expression.operator == "$random" {
            "s"
        } else {
            "u"
        };
        let operands = expression
            .operands
            .iter()
            .map(|operand| slang_expression_fingerprint_with_parameters(operand, parameters, mode))
            .collect::<Option<Vec<_>>>()?;
        return Some(format!(
            "Random:32:{signed_label}[{},{}]",
            expression.operator,
            operands
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    if expression.kind == "Call" && expression.operator == "$sformatf" {
        let format_argument = expression.operands.first()?;
        let format_value = format_argument.operands.first().unwrap_or(format_argument);
        let format = format_value.literal.as_deref()?;
        let args = expression
            .operands
            .iter()
            .skip(1)
            .map(|argument| {
                slang_expression_fingerprint_with_parameters(argument, parameters, mode)
            })
            .collect::<Option<Vec<_>>>()?;
        return Some(format!(
            "StringFormat:512:u[format:{format},{}]",
            args.join(",")
        ));
    }
    if expression.kind == "Call" && expression.operator == "$fopen" {
        let literal_path = expression
            .operands
            .first()?
            .operands
            .first()
            .and_then(|operand| operand.literal.as_deref());
        let literal_mode = expression
            .operands
            .get(1)?
            .operands
            .first()
            .and_then(|operand| operand.literal.as_deref());
        if let (Some(path), Some(mode)) = (literal_path, literal_mode) {
            let mode = canonical_literal_file_open_mode(mode)?;
            return Some(format!("FileOpen:32:u[{path},{mode}]"));
        }
        let path = slang_expression_fingerprint_with_parameters(
            expression.operands.first()?,
            parameters,
            mode,
        )?;
        let mode = slang_expression_fingerprint_with_parameters(
            expression.operands.get(1)?,
            parameters,
            mode,
        )?;
        return Some(format!("FileOpenDynamic:32:u[{path},{mode}]"));
    }
    if expression.kind == "Call" && expression.operator == "$fgetc" {
        let argument = expression.operands.first()?;
        let descriptor_argument = argument.operands.first().unwrap_or(argument);
        let descriptor =
            slang_expression_fingerprint_with_parameters(descriptor_argument, parameters, mode)?;
        return Some(format!("FileGetc:32:s[{descriptor}]"));
    }
    if expression.kind == "Call" && expression.operator == "$fgets" {
        let output = expression.operands.first()?;
        let target = output.operands.first()?;
        let descriptor_argument = expression.operands.get(1)?;
        let descriptor_value = descriptor_argument
            .operands
            .first()
            .unwrap_or(descriptor_argument);
        let target = slang_expression_fingerprint_with_parameters(target, parameters, mode)?;
        let descriptor =
            slang_expression_fingerprint_with_parameters(descriptor_value, parameters, mode)?;
        return Some(format!("FileGets:32:s[{target},{descriptor}]"));
    }
    if expression.kind == "Call" && expression.operator == "$ungetc" {
        let mut operands = Vec::new();
        for argument in expression.operands.iter().take(2) {
            let argument_value = argument.operands.first().unwrap_or(argument);
            operands.push(slang_expression_fingerprint_with_parameters(
                argument_value,
                parameters,
                mode,
            )?);
        }
        if operands.len() != 2 {
            return None;
        }
        return Some(format!("FileUngetc:32:s[{}]", operands.join(",")));
    }
    if expression.kind == "Call" && expression.operator == "$ferror" {
        let descriptor_argument = expression.operands.first()?;
        let descriptor_value = descriptor_argument
            .operands
            .first()
            .unwrap_or(descriptor_argument);
        let output = expression.operands.get(1)?;
        let target = output.operands.first()?;
        let message = slang_expression_fingerprint_with_parameters(target, parameters, mode)?;
        let descriptor =
            slang_expression_fingerprint_with_parameters(descriptor_value, parameters, mode)?;
        return Some(format!("FileError:32:s[{message},{descriptor}]"));
    }
    if expression.kind == "Call" && expression.operator == "$sscanf" {
        let source_argument = expression.operands.first()?;
        let source =
            slang_expression_fingerprint_with_parameters(source_argument, parameters, mode)?;
        let format = expression.operands.get(1)?.literal.as_deref()?;
        let mut targets = Vec::new();
        for output in expression.operands.iter().skip(2) {
            let target = output.operands.first()?;
            targets.push(slang_expression_fingerprint_with_parameters(
                target, parameters, mode,
            )?);
        }
        return Some(format!(
            "StringScan:32:s[{source},format:{format},{}]",
            targets.join(",")
        ));
    }
    if expression.kind == "Call" && expression.operator == "$fread" {
        let output = expression.operands.first()?;
        let target_summary = output.operands.first()?;
        let descriptor_argument = expression.operands.get(1)?;
        let descriptor_value = descriptor_argument
            .operands
            .first()
            .unwrap_or(descriptor_argument);
        let target =
            slang_expression_fingerprint_with_parameters(target_summary, parameters, mode)?;
        let descriptor =
            slang_expression_fingerprint_with_parameters(descriptor_value, parameters, mode)?;
        if target_summary.type_name.contains('$') && target_summary.type_name != "event" {
            let start = match expression.operands.get(2) {
                Some(argument) => Some(slang_expression_fingerprint_with_parameters(
                    argument, parameters, mode,
                )?),
                None => None,
            }
            .unwrap_or_else(|| "None".to_string());
            let count = match expression.operands.get(3) {
                Some(argument) => Some(slang_expression_fingerprint_with_parameters(
                    argument, parameters, mode,
                )?),
                None => None,
            }
            .unwrap_or_else(|| "None".to_string());
            return Some(format!(
                "FileReadMemory:32:s[{target},{descriptor},{start},{count}]"
            ));
        }
        return Some(format!("FileRead:32:s[{target},{descriptor}]"));
    }
    if expression.kind == "Call" && expression.operator == "$fscanf" {
        let descriptor_argument = expression.operands.first()?;
        let descriptor_value = descriptor_argument
            .operands
            .first()
            .unwrap_or(descriptor_argument);
        let format = expression.operands.get(1)?.literal.as_deref()?;
        let mut targets = Vec::new();
        for output in expression.operands.iter().skip(2) {
            let target = output.operands.first()?;
            targets.push(slang_expression_fingerprint_with_parameters(
                target, parameters, mode,
            )?);
        }
        let descriptor =
            slang_expression_fingerprint_with_parameters(descriptor_value, parameters, mode)?;
        return Some(format!(
            "FileScan:32:s[{descriptor},format:{format},{}]",
            targets.join(",")
        ));
    }
    if expression.kind == "Call" && expression.operator == "$feof" {
        let argument = expression.operands.first()?;
        let descriptor_argument = argument.operands.first().unwrap_or(argument);
        let descriptor =
            slang_expression_fingerprint_with_parameters(descriptor_argument, parameters, mode)?;
        return Some(format!("FileEof:32:s[{descriptor}]"));
    }
    if expression.kind == "Call" && expression.operator == "$ftell" {
        let argument = expression.operands.first()?;
        let descriptor_argument = argument.operands.first().unwrap_or(argument);
        let descriptor =
            slang_expression_fingerprint_with_parameters(descriptor_argument, parameters, mode)?;
        return Some(format!("FileTell:32:s[{descriptor}]"));
    }
    if expression.kind == "Call" && expression.operator == "$fseek" {
        let mut operands = Vec::new();
        for argument in expression.operands.iter() {
            let argument_value = argument.operands.first().unwrap_or(argument);
            operands.push(slang_expression_fingerprint_with_parameters(
                argument_value,
                parameters,
                mode,
            )?);
        }
        if operands.len() != 3 {
            return None;
        }
        return Some(format!("FileSeek:32:s[{}]", operands.join(",")));
    }
    if expression.kind == "Call" && expression.operator == "$rewind" {
        let argument = expression.operands.first()?;
        let descriptor_argument = argument.operands.first().unwrap_or(argument);
        let descriptor =
            slang_expression_fingerprint_with_parameters(descriptor_argument, parameters, mode)?;
        return Some(format!(
            "FileSeek:32:s[{descriptor},Const:32:u[32:{}],Const:32:u[32:{}]]",
            "0".repeat(32),
            "0".repeat(32)
        ));
    }
    if expression.kind == "Call"
        && matches!(
            expression.operator.as_str(),
            "__eda_class_handle_eq" | "__eda_class_handle_ne"
        )
    {
        let comparison = if expression.operator == "__eda_class_handle_eq" {
            "Eq"
        } else {
            "Ne"
        };
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
        return Some(format!(
            "Signed:32:s[Signed:32:u[Concat:32:u[Const:31:u[31:{}],{comparison}:1:u[{left},{right}]]]]",
            "0".repeat(31)
        ));
    }
    if expression.kind == "Call" && expression.operator == "__eda_class_array_read" {
        let width = expression.type_width?;
        let operands = expression
            .operands
            .iter()
            .map(|operand| slang_expression_fingerprint_with_parameters(operand, parameters, mode))
            .collect::<Option<Vec<_>>>()?;
        return Some(format!(
            "Cond:{width}:u[{}]",
            operands
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
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
        if let Some(fingerprint) = event_triggered_fingerprint(expression, parameters, mode) {
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
        if method == "$realtime" && mode == ExpressionFingerprintMode::Direct {
            let unit = expression.simulation_time_unit?;
            return Some(format!("RealTime:64:s[Unit{unit}]"));
        }
        if matches!(method, "$timeunit" | "$timeprecision")
            && mode == ExpressionFingerprintMode::Direct
        {
            let exponent = expression.simulation_time_unit?;
            let word = LogicWord::from_u64(32, i32::from(exponent) as u32 as u64);
            let bits = word.to_bin_str();
            let signed_label = if bits.starts_with('1') { "s" } else { "u" };
            return Some(format!("Const:32:{signed_label}[32:{bits}]",));
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
            let selected = expression.operands.first()?;
            if lsb == 0
                && msb + 1 == width
                && matches!(selected.kind.as_str(), "NamedValue" | "HierarchicalValue")
            {
                let name = selected.symbol.as_deref()?;
                if expression.type_name.trim() == "shortreal" && width == 32 {
                    return Some(format!("Ref:32:s[{name}]"));
                }
                if matches!(expression.type_name.trim(), "real" | "realtime") && width == 64 {
                    return Some(format!("Ref:64:s[{name}]"));
                }
            }
            let base = slang_expression_fingerprint_with_parameters(selected, parameters, mode)?;
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
    if matches!(expression.kind.as_str(), "NamedValue" | "HierarchicalValue") {
        let symbol = expression.symbol.as_deref()?;
        if expression.type_name.trim() == "shortreal" {
            return Some(format!("Ref:32:s[{symbol}]"));
        }
        if type_is_realtime_compatible(&expression.type_name) {
            return Some(format!("Ref:64:s[{symbol}]"));
        }
    }
    if expression.kind == "BinaryOp"
        && matches!(
            expression.operator.as_str(),
            "Add" | "Subtract" | "Multiply" | "Divide"
        )
    {
        let left = expression.operands.first()?;
        let right = expression.operands.get(1)?;
        let all_realtime = type_is_realtime_compatible(&left.type_name)
            && type_is_realtime_compatible(&right.type_name);
        let all_shortreal =
            left.type_name.trim() == "shortreal" && right.type_name.trim() == "shortreal";
        if all_realtime || all_shortreal {
            let root = slang_expression_shape(expression)?;
            let width = if all_shortreal { 32 } else { 64 };
            let left = slang_expression_fingerprint_with_parameters(left, parameters, mode)?;
            let right = slang_expression_fingerprint_with_parameters(right, parameters, mode)?;
            return Some(format!("{root}:{width}:s[{left},{right}]"));
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

fn canonical_literal_file_open_mode(mode: &str) -> Option<&'static str> {
    match mode {
        "8:01110010" | "16:0110001001110010" => Some("8:01110010"),
        "16:0010101101110010" | "24:011000100010101101110010" | "24:001010110110001001110010" => {
            Some("16:0010101101110010")
        }
        "8:01110111" | "16:0110001001110111" => Some("8:01110111"),
        "16:0010101101110111" | "24:011000100010101101110111" | "24:001010110110001001110111" => {
            Some("16:0010101101110111")
        }
        "8:01100001" | "16:0110001001100001" => Some("8:01100001"),
        "16:0010101101100001" | "24:011000100010101101100001" | "24:001010110110001001100001" => {
            Some("16:0010101101100001")
        }
        _ => None,
    }
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
            "ConcurrentAssertion" => {
                procedural_blocks.push(SlangProceduralBlockSummary {
                    procedure_kind: "ConcurrentAssertion".into(),
                    assignment_targets: Vec::new(),
                    assignments: Vec::new(),
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
        Expr::RealConst(_) => "RealConst",
        Expr::RealRef { .. } => "Ref",
        Expr::ProcessSelf => "ProcessSelf",
        Expr::ProcessStatus(_) => "ProcessStatus",
        Expr::Past { .. } => "Past",
        Expr::Sampled(_) => "Sampled",
        Expr::SampledValue { op, .. } => match op {
            eda_core::ir::SampledValueOp::Rose => "Rose",
            eda_core::ir::SampledValueOp::Fell => "Fell",
            eda_core::ir::SampledValueOp::Stable => "Stable",
            eda_core::ir::SampledValueOp::Changed => "Changed",
        },
        Expr::ShortRealConst(_) => "ShortRealConst",
        Expr::ShortRealRef { .. } => "Ref",
        Expr::ShortRealBinary { op, .. } => match op {
            eda_core::ir::BinOp::Add => "Add",
            eda_core::ir::BinOp::Sub => "Sub",
            eda_core::ir::BinOp::Mul => "Mul",
            eda_core::ir::BinOp::Div => "Div",
            _ => "ShortRealBinary",
        },
        Expr::RealBinary { op, .. } => match op {
            eda_core::ir::BinOp::Add => "Add",
            eda_core::ir::BinOp::Sub => "Sub",
            eda_core::ir::BinOp::Mul => "Mul",
            eda_core::ir::BinOp::Div => "Div",
            _ => "RealBinary",
        },
        Expr::RealCompare { op, .. } => match op {
            eda_core::ir::BinOp::Eq => "Eq",
            eda_core::ir::BinOp::Ne => "Ne",
            eda_core::ir::BinOp::Lt => "Lt",
            eda_core::ir::BinOp::Le => "Le",
            eda_core::ir::BinOp::Gt => "Gt",
            eda_core::ir::BinOp::Ge => "Ge",
            _ => "RealCompare",
        },
        Expr::RealNeg(_) => "RealNeg",
        Expr::ShortRealNeg(_) => "ShortRealNeg",
        Expr::ShortRealToReal(_) => "ShortRealToReal",
        Expr::RealToShortReal(_) => "RealToShortReal",
        Expr::BitsToReal(_) => "BitsToReal",
        Expr::BitsToShortReal(_) => "BitsToShortReal",
        Expr::RealToInt { .. } => "RealToInt",
        Expr::RealTruncToInt { .. } => "RealTruncToInt",
        Expr::IntToReal { .. } => "IntToReal",
        Expr::RealTime { .. } => "RealTime",
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
        Expr::StringFormat { .. } => "StringFormat",
        Expr::StringGetByte { .. } => "StringGetByte",
        Expr::StringCase { .. } => "StringCase",
        Expr::StringCompare { .. } => "StringCompare",
        Expr::StringPutc { .. } => "StringPutc",
        Expr::StringSubstr { .. } => "StringSubstr",
        Expr::StringToRadix { .. } => "StringToRadix",
        Expr::StringFromRadix { .. } => "StringFromRadix",
        Expr::MemoryPattern { .. } => "MemoryPattern",
        Expr::EnumPattern { .. } => "EnumPattern",
        Expr::StructPattern { .. } => "StructPattern",
        Expr::EventTriggered { .. } => "EventTriggered",
        Expr::SimulationTime { .. } => "SimulationTime",
        Expr::Random { .. } => "Random",
        Expr::CountBits { .. } => "CountBits",
        Expr::IsUnknown(_) => "IsUnknown",
        Expr::OneHot(_) => "OneHot",
        Expr::OneHot0(_) => "OneHot0",
        Expr::FileOpen { .. } => "FileOpen",
        Expr::FileOpenDynamic { .. } => "FileOpenDynamic",
        Expr::FileGetc { .. } => "FileGetc",
        Expr::FileUngetc { .. } => "FileUngetc",
        Expr::FileEof { .. } => "FileEof",
        Expr::FileTell { .. } => "FileTell",
        Expr::FileSeek { .. } => "FileSeek",
        Expr::MemorySize { .. } => "MemorySize",
        Expr::AssocRead { .. } => "AssocRead",
        Expr::AssocExists { .. } => "AssocExists",
    }
}

pub(crate) fn design_expr_fingerprint(design: &Design, expression: &eda_core::ir::Expr) -> String {
    use eda_core::ir::Expr;
    if let Expr::Signed {
        e: shifted,
        signed: true,
    } = expression
    {
        if let Expr::Bin {
            op: eda_core::ir::BinOp::Shr,
            a,
            b,
        } = shifted.as_ref()
        {
            if let Expr::Signed {
                e: logical_operand,
                signed: false,
            } = a.as_ref()
            {
                return format!(
                    "Shr:{}:s[{},{}]",
                    expression.width(),
                    design_expr_fingerprint(design, logical_operand),
                    design_expr_fingerprint(design, b)
                );
            }
        }
    }
    if let Expr::Bin {
        op: eda_core::ir::BinOp::Shr,
        a,
        b,
    } = expression
    {
        if let Expr::Signed {
            e: logical_operand,
            signed: false,
        } = a.as_ref()
        {
            return format!(
                "Shr:{}:u[{},{}]",
                expression.width(),
                design_expr_fingerprint(design, logical_operand),
                design_expr_fingerprint(design, b)
            );
        }
    }
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
    if let Expr::RealConst(real) = expression {
        if real.fract() == 0.0 && *real >= 0.0 && *real <= u64::MAX as f64 {
            let word = LogicWord::from_u64(64, *real as u64);
            return format!("Const:64:u[64:{}]", word.to_bin_str());
        }
        return format!("RealConst:64:f[{real}]");
    }
    if let Expr::ShortRealConst(real) = expression {
        return format!("ShortRealConst:32:f[{real}]");
    }
    if let Expr::RealRef { net, .. } = expression {
        let name = design_expr_symbol(design, expression).unwrap_or_else(|| design.net_name(*net));
        return format!("Ref:64:s[{name}]");
    }
    if let Expr::ProcessSelf = expression {
        return "ProcessSelf:64:u[]".to_string();
    }
    if let Expr::ProcessStatus(handle) = expression {
        return format!(
            "ProcessStatus:32:s[{}]",
            design_expr_fingerprint(design, handle)
        );
    }
    if let Expr::ShortRealRef { net } = expression {
        let name = design_expr_symbol(design, expression).unwrap_or_else(|| design.net_name(*net));
        return format!("Ref:32:s[{name}]");
    }
    if let Expr::RealTime { unit_exponent } = expression {
        return format!("RealTime:64:s[Unit{unit_exponent}]");
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
    if let Expr::Random {
        function,
        args,
        width,
        signed,
    } = expression
    {
        let name = match function {
            eda_core::ir::RandomFunction::Random => "$random",
            eda_core::ir::RandomFunction::Urandom => "$urandom",
            eda_core::ir::RandomFunction::UrandomRange => "$urandom_range",
        };
        let signed_label = if *signed { "s" } else { "u" };
        return format!(
            "Random:{width}:{signed_label}[{name},{}]",
            args.iter()
                .map(|argument| design_expr_fingerprint(design, argument))
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    if let Expr::CountBits { value, controls } = expression {
        return format!(
            "CountBits:32:s[Controls{controls},{}]",
            design_expr_fingerprint(design, value)
        );
    }
    if let Expr::IsUnknown(value) = expression {
        return format!("IsUnknown:1:u[{}]", design_expr_fingerprint(design, value));
    }
    if let Expr::OneHot(value) = expression {
        return format!("OneHot:1:u[{}]", design_expr_fingerprint(design, value));
    }
    if let Expr::OneHot0(value) = expression {
        return format!("OneHot0:1:u[{}]", design_expr_fingerprint(design, value));
    }
    if let Expr::FileOpen {
        path,
        append,
        read,
        truncate,
    } = expression
    {
        let Some(width) = path.len().checked_mul(8) else {
            return "FileOpen:32:u[<path-too-large>]".to_string();
        };
        let path_word = LogicWord::from_bytes_le(width, path.as_bytes());
        let mode = if *read && *append && *truncate {
            "16:0010101101100001"
        } else if *read && *append {
            "16:0010101101110010"
        } else if *read && *truncate {
            "16:0010101101110111"
        } else if *read {
            "8:01110010"
        } else if *append {
            "8:01100001"
        } else {
            "8:01110111"
        };
        return format!("FileOpen:32:u[{}:{},{mode}]", width, path_word.to_bin_str());
    }
    if let Expr::FileOpenDynamic { path, mode } = expression {
        return format!(
            "FileOpenDynamic:32:u[{},{}]",
            design_expr_fingerprint(design, path),
            design_expr_fingerprint(design, mode)
        );
    }
    if let Expr::StringFormat { format, args, .. } = expression {
        let Some(format_width) = format.len().checked_mul(8) else {
            return "StringFormat:512:u[<format-too-large>]".to_string();
        };
        let format_word = LogicWord::from_bytes_le(format_width, format.as_bytes());
        return format!(
            "StringFormat:512:u[format:{format_width}:{},{}]",
            format_word.to_bin_str(),
            args.iter()
                .map(|argument| design_expr_fingerprint(design, argument))
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    if let Expr::MemoryPattern { memory, .. } = expression {
        let memory_name = design
            .memories
            .get(*memory)
            .map(|memory| memory.name.as_str())
            .unwrap_or("<unknown>");
        return format!("MemoryCopy:{memory_name}");
    }
    if let Expr::EnumPattern { value, .. } = expression {
        return design_expr_fingerprint(design, value);
    }
    if let Expr::StructPattern { value, .. } = expression {
        return design_expr_fingerprint(design, value);
    }
    if let Expr::FileGetc { fd } = expression {
        return format!("FileGetc:32:s[{}]", design_expr_fingerprint(design, fd));
    }
    if let Expr::FileUngetc { value, fd } = expression {
        let mut semantic_value = value.as_ref();
        while let Expr::Signed { e: inner, .. } = semantic_value {
            if matches!(inner.as_ref(), Expr::Signed { .. }) {
                semantic_value = inner.as_ref();
            } else {
                break;
            }
        }
        return format!(
            "FileUngetc:32:s[{},{}]",
            design_expr_fingerprint(design, semantic_value),
            design_expr_fingerprint(design, fd)
        );
    }
    if let Expr::FileEof { fd } = expression {
        return format!("FileEof:32:s[{}]", design_expr_fingerprint(design, fd));
    }
    if let Expr::FileTell { fd } = expression {
        return format!("FileTell:32:s[{}]", design_expr_fingerprint(design, fd));
    }
    if let Expr::FileSeek { fd, offset, whence } = expression {
        return format!(
            "FileSeek:32:s[{},{},{}]",
            design_expr_fingerprint(design, fd),
            design_expr_fingerprint(design, offset),
            design_expr_fingerprint(design, whence)
        );
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
    if let Expr::Signed {
        e: operand,
        signed: true,
    } = expression
    {
        if let Expr::AssocRead {
            memory,
            index,
            width,
        } = operand.as_ref()
        {
            let memory_name = design
                .memories
                .get(*memory)
                .map(|memory| memory.name.as_str())
                .unwrap_or("<unknown>");
            return format!(
                "AssocRead:{width}:s[{memory_name},{}]",
                design_expr_fingerprint(design, index)
            );
        }
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
    if let Expr::EventTriggered { net } = expression {
        return format!("EventTriggered:1:u[{}]", design.net_name(*net));
    }
    let operands: Vec<String> = match expression {
        Expr::Const(_)
        | Expr::RealConst(_)
        | Expr::RealRef { .. }
        | Expr::ShortRealConst(_)
        | Expr::ShortRealRef { .. }
        | Expr::ProcessSelf
        | Expr::Ref { .. }
        | Expr::MemoryRead { .. }
        | Expr::SimulationTime { .. }
        | Expr::RealTime { .. }
        | Expr::MemorySize { .. }
        | Expr::CountBits { .. }
        | Expr::IsUnknown(_)
        | Expr::OneHot(_)
        | Expr::OneHot0(_) => Vec::new(),
        Expr::ProcessStatus(handle) => vec![design_expr_fingerprint(design, handle)],
        Expr::Past {
            value,
            ticks,
            gating,
        } => {
            let mut operands = vec![ticks.to_string(), design_expr_fingerprint(design, value)];
            if let Some(gating) = gating {
                operands.push(design_expr_fingerprint(design, gating));
            }
            operands
        }
        Expr::Sampled(value) => vec![design_expr_fingerprint(design, value)],
        Expr::SampledValue { value, .. } => vec![design_expr_fingerprint(design, value)],
        Expr::RealBinary { left, right, .. } => vec![
            design_expr_fingerprint(design, left),
            design_expr_fingerprint(design, right),
        ],
        Expr::RealCompare { left, right, .. } => vec![
            design_expr_fingerprint(design, left),
            design_expr_fingerprint(design, right),
        ],
        Expr::RealNeg(operand) => vec![design_expr_fingerprint(design, operand)],
        Expr::ShortRealBinary { left, right, .. } => vec![
            design_expr_fingerprint(design, left),
            design_expr_fingerprint(design, right),
        ],
        Expr::ShortRealNeg(operand)
        | Expr::ShortRealToReal(operand)
        | Expr::RealToShortReal(operand)
        | Expr::BitsToReal(operand)
        | Expr::BitsToShortReal(operand) => vec![design_expr_fingerprint(design, operand)],
        Expr::RealToInt { value, .. }
        | Expr::RealTruncToInt { value, .. }
        | Expr::IntToReal { value, .. } => {
            vec![design_expr_fingerprint(design, value)]
        }
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
        Expr::StringFormat { args, .. } => args
            .iter()
            .map(|argument| design_expr_fingerprint(design, argument))
            .collect(),
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
        Expr::MemoryPattern { .. } => Vec::new(),
        Expr::EnumPattern { value, .. } => vec![design_expr_fingerprint(design, value)],
        Expr::StructPattern { value, .. } => vec![design_expr_fingerprint(design, value)],
        Expr::EventTriggered { net } => vec![format!("net:{net}")],
        Expr::Random { args, .. } => args
            .iter()
            .map(|argument| design_expr_fingerprint(design, argument))
            .collect(),
        Expr::FileOpen { .. } => Vec::new(),
        Expr::FileOpenDynamic { path, mode } => vec![
            design_expr_fingerprint(design, path),
            design_expr_fingerprint(design, mode),
        ],
        Expr::FileGetc { fd } => vec![design_expr_fingerprint(design, fd)],
        Expr::FileUngetc { value, fd } => vec![
            design_expr_fingerprint(design, value),
            design_expr_fingerprint(design, fd),
        ],
        Expr::FileEof { fd } => vec![design_expr_fingerprint(design, fd)],
        Expr::FileTell { fd } => vec![design_expr_fingerprint(design, fd)],
        Expr::FileSeek { fd, offset, whence } => vec![
            design_expr_fingerprint(design, fd),
            design_expr_fingerprint(design, offset),
            design_expr_fingerprint(design, whence),
        ],
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
        Expr::Ref { net, .. } | Expr::RealRef { net, .. } | Expr::ShortRealRef { net } => {
            clocking_sample_symbol(design, *net).or_else(|| {
                design
                    .net_decls
                    .iter()
                    .find(|declaration| {
                        *net >= declaration.base && *net < declaration.base + declaration.width
                    })
                    .map(|declaration| canonical_ref_symbol_name(&declaration.name))
            })
        }
        Expr::MemoryRead { memory, .. } | Expr::MemoryReadPart { memory, .. } => design
            .memories
            .get(*memory)
            .map(|memory| memory.name.clone()),
        _ => None,
    }
}

fn canonical_ref_symbol_name(name: &str) -> String {
    if let Some(local) = name.strip_prefix("__foreach_iterator_") {
        let mut segments = local.split('_');
        let address_skipped = segments.next().is_some();
        let dimension_skipped = segments.next().is_some();
        if address_skipped && dimension_skipped {
            let iterator_name = segments.collect::<Vec<_>>().join("_");
            if !iterator_name.is_empty() {
                return iterator_name;
            }
        }
    }
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
        Stmt::NamedBlock { body, .. } => {
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
        Stmt::Force { .. }
        | Stmt::Release { .. }
        | Stmt::ProceduralAssign { .. }
        | Stmt::ProceduralDeassign { .. } => {}
        Stmt::DisableFork | Stmt::WaitFork | Stmt::NamedBlockEnd => {}
        Stmt::DisableNamedBlock { .. } => {}
        Stmt::ProcessKill(_)
        | Stmt::ProcessAwait(_)
        | Stmt::ProcessSuspend(_)
        | Stmt::ProcessResume(_) => {}
        Stmt::Break | Stmt::Continue | Stmt::ReturnBarrier | Stmt::Return => {}
        Stmt::Scope(body) => {
            assignments.extend(design_stmt_assignments_guarded(design, body, guard));
        }
        Stmt::NBl { lv, e } | Stmt::Bl { lv, e } => {
            if let Some(name) = design_lvalue_name(design, lv) {
                assignments.push((name, design_expr_fingerprint(design, e)));
            }
        }
        Stmt::FileGets { fd, string, status } => {
            let Some(status_name) = design_lvalue_name(design, status) else {
                return assignments;
            };
            let Some(string_name) = design_lvalue_name(design, string) else {
                return assignments;
            };
            assignments.push((
                status_name,
                format!(
                    "FileGets:32:s[Ref:{}:u[{string_name}],{}]",
                    string.decl_width,
                    design_expr_fingerprint(design, fd)
                ),
            ));
        }
        Stmt::FileError {
            fd,
            message,
            status,
        } => {
            let Some(status_name) = design_lvalue_name(design, status) else {
                return assignments;
            };
            let Some(message_name) = design_lvalue_name(design, message) else {
                return assignments;
            };
            assignments.push((
                status_name,
                format!(
                    "FileError:32:s[Ref:{}:u[{message_name}],{}]",
                    message.decl_width,
                    design_expr_fingerprint(design, fd)
                ),
            ));
        }
        Stmt::FileRead { fd, target, status } => {
            let Some(status_name) = design_lvalue_name(design, status) else {
                return assignments;
            };
            let Some(target_name) = design_lvalue_name(design, target) else {
                return assignments;
            };
            assignments.push((
                status_name,
                format!(
                    "FileRead:32:s[Ref:{}:u[{target_name}],{}]",
                    target.decl_width,
                    design_expr_fingerprint(design, fd)
                ),
            ));
        }
        Stmt::FileReadMemory {
            fd,
            memory,
            start,
            count,
            status,
        } => {
            let Some(status_name) = design_lvalue_name(design, status) else {
                return assignments;
            };
            let Some(memory_name) = design
                .memories
                .get(*memory)
                .map(|memory| memory.name.as_str())
            else {
                return assignments;
            };
            let start = start
                .as_ref()
                .map(|expression| design_expr_fingerprint(design, expression))
                .unwrap_or_else(|| "None".to_string());
            let count = count
                .as_ref()
                .map(|expression| design_expr_fingerprint(design, expression))
                .unwrap_or_else(|| "None".to_string());
            assignments.push((
                status_name,
                format!(
                    "FileReadMemory:32:s[MemoryCopy:{memory_name},{},{start},{count}]",
                    design_expr_fingerprint(design, fd)
                ),
            ));
        }
        Stmt::FileScan {
            fd,
            format,
            targets,
            status,
        } => {
            let Some(status_name) = design_lvalue_name(design, status) else {
                return assignments;
            };
            let format_width = format.len().saturating_mul(8);
            let format_word = LogicWord::from_bytes_le(format_width, format.as_bytes());
            let mut target_fingerprints = Vec::new();
            for target in targets {
                let Some(target_name) = design_lvalue_name(design, target) else {
                    return assignments;
                };
                let target_signed = design
                    .net_decls
                    .iter()
                    .find(|declaration| declaration.base == target.net)
                    .is_some_and(|declaration| declaration.is_signed);
                let signed_label = if target_signed { "s" } else { "u" };
                target_fingerprints.push(format!(
                    "Ref:{}:{signed_label}[{target_name}]",
                    target.decl_width
                ));
            }
            assignments.push((
                status_name,
                format!(
                    "FileScan:32:s[{},format:{format_width}:{},{}]",
                    design_expr_fingerprint(design, fd),
                    format_word.to_bin_str(),
                    target_fingerprints.join(",")
                ),
            ));
        }
        Stmt::StringScan {
            source,
            format,
            targets,
            status,
        } => {
            let Some(status_name) = design_lvalue_name(design, status) else {
                return assignments;
            };
            let format_width = format.len().saturating_mul(8);
            let format_word = LogicWord::from_bytes_le(format_width, format.as_bytes());
            let mut target_fingerprints = Vec::new();
            for target in targets {
                let Some(target_name) = design_lvalue_name(design, target) else {
                    return assignments;
                };
                let target_signed = design
                    .net_decls
                    .iter()
                    .find(|declaration| declaration.base == target.net)
                    .is_some_and(|declaration| declaration.is_signed);
                let signed_label = if target_signed { "s" } else { "u" };
                target_fingerprints.push(format!(
                    "Ref:{}:{signed_label}[{target_name}]",
                    target.decl_width
                ));
            }
            assignments.push((
                status_name,
                format!(
                    "StringScan:32:s[{},format:{format_width}:{},{}]",
                    design_expr_fingerprint(design, source),
                    format_word.to_bin_str(),
                    target_fingerprints.join(",")
                ),
            ));
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
        Stmt::ImmediateAssertion {
            pass_action,
            fail_action,
            ..
        } => {
            // Slang's procedural summary records the syntactic action-block
            // assignments without an assertion-condition guard. Mirror that
            // order: fail action before pass action.
            for action in fail_action.iter().chain(pass_action.iter()) {
                assignments.extend(design_stmt_assignments_guarded(design, action, guard));
            }
        }
        Stmt::Display { .. }
        | Stmt::Assert { .. }
        | Stmt::Assume { .. }
        | Stmt::Cover { .. }
        | Stmt::TimeFormat { .. }
        | Stmt::WriteMem { .. }
        | Stmt::AssertControl(_)
        | Stmt::FileClose { .. }
        | Stmt::FileFlush { .. }
        | Stmt::FileDisplay { .. }
        | Stmt::Finish
        | Stmt::Stop => {}
        Stmt::RandomSeeded { seed, destination } => {
            let Some(destination_name) = design_lvalue_name(design, destination) else {
                return assignments;
            };
            let Some(seed_name) = design_lvalue_name(design, seed) else {
                return assignments;
            };
            let seed_signed = design
                .net_decls
                .iter()
                .find(|declaration| declaration.base == seed.net)
                .is_some_and(|declaration| declaration.is_signed);
            let signed_label = if seed_signed { "s" } else { "u" };
            assignments.push((
                destination_name,
                format!("Random:32:s[$random,Ref:32:{signed_label}[{seed_name}]]"),
            ));
        }
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
