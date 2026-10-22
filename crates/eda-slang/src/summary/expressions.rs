//! Expression summaries, fingerprints, and IR-shape comparison helpers.

use crate::ast_util::{
    bool_field, expression_symbol, expression_target_names, instance_parameters,
    integer_literal_word, literal_word, named_constant_word, slang_constant_u64, string_field,
};
use crate::types::{
    simple_type_shape, simple_type_signed, simple_type_width, string_literal_word, STRING_WIDTH,
};
use eda_core::ir::Design;
use serde_json::Value;

use super::{
    SlangAssignmentSummary, SlangContinuousTarget, SlangExpressionSummary, SlangInstanceSummary,
    SlangMemoryMemberLvalueSummary, SlangParameterSummary, SlangProceduralBlockSummary,
};

pub(crate) fn expression_summary(value: &Value) -> SlangExpressionSummary {
    let kind = string_field(value, "kind").unwrap_or_default();
    let literal = expression_literal(value);
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
        operator: string_field(value, "op").unwrap_or_default(),
        selection_kind: string_field(value, "selectionKind").unwrap_or_default(),
        type_width: string_field(value, "type").and_then(|type_name| simple_type_width(&type_name)),
        type_signed,
        symbol: expression_symbol(value),
        literal,
        repeat_count: repeat_count(value),
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
        literal: None,
        repeat_count: None,
        operands: Vec::new(),
    }
}

pub(crate) fn expression_literal(value: &Value) -> Option<String> {
    let kind = string_field(value, "kind")?;
    let raw = match kind.as_str() {
        "IntegerLiteral" => string_field(value, "value")?,
        "NamedValue" => string_field(value, "constant")?,
        _ => return None,
    };
    let type_name = string_field(value, "type")?;
    let word = literal_word(&type_name, &raw)?;
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
            if !is_class_handle_target(left, class_handles) {
                let dynamic_packed_lvalue = is_dynamic_packed_lvalue(left);
                let dynamic_packed_lvalue_selector = dynamic_packed_lvalue
                    .then(|| left.get("selector").map(expression_summary))
                    .flatten();
                let memory_member_lvalue = memory_member_lvalue_summary(left);
                assignments.push(SlangAssignmentSummary {
                    target_names: expression_target_names(left),
                    rhs: expression_summary(right),
                    nonblocking: bool_field(value, "isNonBlocking"),
                    dynamic_packed_lvalue,
                    dynamic_packed_lvalue_selector,
                    memory_member_lvalue,
                    uses_class_static: expression_uses_class_symbol(right, class_handles),
                });
            }
        }
    }
    match value {
        Value::Array(values) => values
            .iter()
            .for_each(|nested| assignments.extend(procedural_assignments(nested, class_handles))),
        Value::Object(fields) => {
            let is_assignment = string_field(value, "kind").as_deref() == Some("Assignment");
            fields
                .iter()
                .filter(|(field, _)| {
                    !(is_assignment && (field.as_str() == "left" || field.as_str() == "right"))
                })
                .for_each(|(_, nested)| {
                    assignments.extend(procedural_assignments(nested, class_handles))
                });
        }
        _ => {}
    }
    assignments
}

fn expression_uses_class_symbol(
    value: &Value,
    class_symbols: &std::collections::BTreeSet<String>,
) -> bool {
    if let Some(symbol) = string_field(value, "symbol") {
        if class_symbols.contains(&symbol) {
            return true;
        }
    }
    value
        .as_object()
        .map(|fields| {
            fields
                .values()
                .any(|nested| expression_uses_class_symbol(nested, class_symbols))
        })
        .or_else(|| {
            value.as_array().map(|values| {
                values
                    .iter()
                    .any(|nested| expression_uses_class_symbol(nested, class_symbols))
            })
        })
        .unwrap_or(false)
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
        ("BinaryOp", "BitwiseXor") => Some("Xor"),
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

fn summary_literal_i64(expression: &SlangExpressionSummary) -> Option<i64> {
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

fn constant_packed_select_fingerprint(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
) -> Option<String> {
    let selected = expression.operands.first()?;
    if selected.type_name.contains('$') {
        return None;
    }
    let shape = simple_type_shape(&selected.type_name)?;
    let dimension = shape.dimensions.first()?;
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
    let base = slang_expression_fingerprint_with_parameters(selected, parameters)?;
    Some(format!(
        "PackedSelect:{width}:u[{base},Range:{physical_msb}:{physical_lsb}]"
    ))
}

fn dynamic_packed_element_select_fingerprint(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
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
    let selector_fingerprint = slang_expression_fingerprint_with_parameters(selector, parameters)?;
    let base = slang_expression_fingerprint_with_parameters(selected, parameters)?;
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

fn canonical_literal_fingerprint(width: usize, signed: bool, literal: &str) -> Option<String> {
    let (_, bits) = literal.split_once(':')?;
    let signed_label = if signed && bits.starts_with('1') {
        "s"
    } else {
        "u"
    };
    Some(format!("Const:{width}:{signed_label}[{literal}]"))
}

pub(crate) fn slang_expression_fingerprint_with_parameters(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
) -> Option<String> {
    if expression.kind == "ElementSelect"
        && expression
            .operands
            .first()
            .is_some_and(|selected| selected.type_name.contains('$'))
    {
        let selected = expression.operands.first()?;
        let selected_symbol = selected.symbol.as_deref()?;
        let memory = selected_symbol
            .rsplit_once(' ')
            .map(|(_, memory)| memory)
            .unwrap_or(selected_symbol);
        let index =
            slang_expression_fingerprint_with_parameters(expression.operands.get(1)?, parameters)?;
        let width = expression.type_width?;
        let signed = expression.type_signed?;
        let signed_label = if signed { "s" } else { "u" };
        if selected.type_name.contains("$assoc[") {
            return Some(format!(
                "AssocRead:{width}:{signed_label}[{memory},{index}]"
            ));
        }
        return Some(format!(
            "MemoryRead:{width}:{signed_label}[{memory},{index}]"
        ));
    }
    if expression.kind == "RangeSelect"
        && expression.operands.first().is_some_and(|selected| {
            selected.kind == "ElementSelect"
                && selected
                    .operands
                    .first()
                    .is_some_and(|memory| memory.type_name.contains('$'))
        })
    {
        let selected = expression.operands.first()?;
        let memory_expression = selected.operands.first()?;
        let selected_symbol = memory_expression.symbol.as_deref()?;
        let memory = selected_symbol
            .rsplit_once(' ')
            .map(|(_, memory)| memory)
            .unwrap_or(selected_symbol);
        let indexes = selected
            .operands
            .iter()
            .skip(1)
            .map(|index| slang_expression_fingerprint_with_parameters(index, parameters))
            .collect::<Option<Vec<_>>>()?;
        let start_operand = match expression.selection_kind.as_str() {
            "Simple" => expression.operands.get(2)?,
            "IndexedUp" | "IndexedDown" => expression.operands.get(1)?,
            _ => return None,
        };
        let start = slang_expression_fingerprint_with_parameters(start_operand, parameters)?;
        let width = expression.type_width?;
        let ascending = expression.selection_kind != "IndexedDown";
        let direction = if ascending { "Ascending" } else { "Descending" };
        return Some(format!(
            "MemoryReadPart:{width}:u[{memory},{},{start},{direction}]",
            indexes.join(",")
        ));
    }
    if matches!(expression.kind.as_str(), "ElementSelect" | "RangeSelect") {
        if let Some(fingerprint) = constant_packed_select_fingerprint(expression, parameters) {
            return Some(fingerprint);
        }
        if let Some(fingerprint) = dynamic_packed_element_select_fingerprint(expression, parameters)
        {
            return Some(fingerprint);
        }
    }
    if expression.kind == "UnaryOp" && expression.operator == "LogicalNot" {
        let operand = expression.operands.first()?;
        let operand_fingerprint =
            slang_expression_fingerprint_with_parameters(operand, parameters)?;
        return Some(format!("Not:1:u[Reduction:1:u[{operand_fingerprint}]]"));
    }
    if expression.kind == "Replication" {
        let count = expression.repeat_count?;
        let operand = expression.operands.first()?;
        let operand_fingerprint =
            slang_expression_fingerprint_with_parameters(operand, parameters)?;
        return Some(format!("Repeat:{count}:u[{operand_fingerprint}]"));
    }
    if expression.kind == "Conversion" {
        let operand = expression.operands.first()?;
        if operand.kind != "IntegerLiteral"
            || expression.type_width.is_none()
            || expression.type_width != operand.type_width
            || expression.type_signed != operand.type_signed
        {
            return None;
        }
        return slang_expression_fingerprint_with_parameters(operand, parameters);
    }
    let root = slang_expression_shape(expression)?;
    let width = expression.type_width?;
    let signed = expression.type_signed?;
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
        .map(|operand| slang_expression_fingerprint_with_parameters(operand, parameters))
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

pub(crate) fn collect_generate_summary(
    value: &Value,
    prefix: &[String],
    instances: &mut Vec<SlangInstanceSummary>,
    continuous_targets: &mut Vec<SlangContinuousTarget>,
    procedural_blocks: &mut Vec<SlangProceduralBlockSummary>,
    class_handles: &std::collections::BTreeSet<String>,
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
                instances,
                continuous_targets,
                procedural_blocks,
                class_handles,
            )?;
        }
        return Ok(());
    }

    let Some(members) = value.get("members").and_then(Value::as_array) else {
        return Ok(());
    };
    let mut working_prefix = prefix.to_vec();
    if string_field(value, "kind").as_deref() == Some("GenerateBlock") {
        if let Some(name) = string_field(value, "name").filter(|name| !name.is_empty()) {
            working_prefix.push(name);
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
                let rhs = assignment
                    .get("right")
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
                let assignments = procedural_assignments(member, class_handles);
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
                    instances,
                    continuous_targets,
                    procedural_blocks,
                    class_handles,
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn design_lvalue_name(design: &Design, lvalue: &eda_core::ir::LValue) -> Option<String> {
    design
        .net_decls
        .iter()
        .find(|declaration| {
            lvalue.net >= declaration.base && lvalue.net < declaration.base + declaration.width
        })
        .map(|declaration| declaration.name.clone())
}

pub(crate) fn design_expr_shape(expression: &eda_core::ir::Expr) -> &'static str {
    use eda_core::ir::{BinOp, Expr, UnOp};
    match expression {
        Expr::Const(_) => "Const",
        Expr::Ref { .. } | Expr::MemoryRead { .. } | Expr::MemoryReadPart { .. } => "Ref",
        Expr::Un { op, .. } => match op {
            UnOp::Not => "Not",
            UnOp::AndR | UnOp::OrR | UnOp::XorR => "Reduction",
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
    use eda_core::ir::Stmt;
    let mut assignments = Vec::new();
    match statement {
        Stmt::Block(statements) => statements
            .iter()
            .for_each(|nested| assignments.extend(design_stmt_assignments(design, nested))),
        Stmt::Forever(body) | Stmt::Delay { body, .. } => {
            assignments.extend(design_stmt_assignments(design, body));
        }
        Stmt::Wait { body, .. } => {
            assignments.extend(design_stmt_assignments(design, body));
        }
        Stmt::EventWait { body, .. } => {
            assignments.extend(design_stmt_assignments(design, body));
        }
        Stmt::WaitOrder {
            success, failure, ..
        } => {
            if let Some(statement) = success {
                assignments.extend(design_stmt_assignments(design, statement));
            }
            if let Some(statement) = failure {
                assignments.extend(design_stmt_assignments(design, statement));
            }
        }
        Stmt::Fork { branches, .. } => branches
            .iter()
            .for_each(|branch| assignments.extend(design_stmt_assignments(design, branch))),
        Stmt::While { body, steps, .. } => {
            assignments.extend(design_stmt_assignments(design, body));
            steps
                .iter()
                .for_each(|step| assignments.extend(design_stmt_assignments(design, step)));
        }
        Stmt::Repeat { body, .. } => {
            assignments.extend(design_stmt_assignments(design, body));
        }
        Stmt::Force { .. } | Stmt::Release { .. } => {}
        Stmt::Break | Stmt::Continue | Stmt::ReturnBarrier | Stmt::Return => {}
        Stmt::Scope(body) => {
            assignments.extend(design_stmt_assignments(design, body));
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
            assignments.extend(design.net_decls.iter().find_map(|declaration| {
                (destination.net >= declaration.base
                    && destination.net < declaration.base + declaration.width)
                    .then(|| {
                        (
                            declaration.name.clone(),
                            format!("QueuePop:{front}:{}", memory),
                        )
                    })
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
        Stmt::BlIndexed { net, e, .. } | Stmt::NBlIndexed { net, e, .. } => {
            assignments.extend(design.net_decls.iter().find_map(|declaration| {
                (*net >= declaration.base && *net < declaration.base + declaration.width)
                    .then(|| (declaration.name.clone(), design_expr_fingerprint(design, e)))
            }));
        }
        Stmt::If { t, f, .. } => {
            assignments.extend(design_stmt_assignments(design, t));
            if let Some(f) = f {
                assignments.extend(design_stmt_assignments(design, f));
            }
        }
        Stmt::Case { arms, dflt, .. } => {
            for (_, body) in arms {
                assignments.extend(design_stmt_assignments(design, body));
            }
            if let Some(body) = dflt {
                assignments.extend(design_stmt_assignments(design, body));
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
