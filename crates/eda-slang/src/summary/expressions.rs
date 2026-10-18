//! Expression summaries, fingerprints, and IR-shape comparison helpers.

use crate::ast_util::{
    bool_field, expression_symbol, expression_target_names, instance_parameters,
    integer_literal_word, literal_word, named_constant_word, string_field,
};
use crate::types::{simple_type_signed, simple_type_width};
use eda_core::ir::Design;
use serde_json::Value;

use super::{
    SlangAssignmentSummary, SlangContinuousTarget, SlangExpressionSummary, SlangInstanceSummary,
    SlangParameterSummary, SlangProceduralBlockSummary,
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
        Some("ElementSelect") => value.get("selector").is_some_and(|selector| {
            integer_literal_word(selector).is_none() && string_field(selector, "constant").is_none()
        }),
        Some("MemberAccess") => value
            .get("value")
            .map(|selected| {
                string_field(selected, "kind").as_deref() == Some("ElementSelect")
                    || is_dynamic_packed_lvalue(selected)
            })
            .unwrap_or(false),
        _ => false,
    }
}

pub(crate) fn procedural_assignments(value: &Value) -> Vec<SlangAssignmentSummary> {
    let mut assignments = Vec::new();
    if string_field(value, "kind").as_deref() == Some("Assignment") {
        if let (Some(left), Some(right)) = (value.get("left"), value.get("right")) {
            let dynamic_packed_lvalue = is_dynamic_packed_lvalue(left);
            assignments.push(SlangAssignmentSummary {
                target_names: expression_target_names(left),
                rhs: expression_summary(right),
                nonblocking: bool_field(value, "isNonBlocking"),
                dynamic_packed_lvalue,
            });
        }
    }
    match value {
        Value::Array(values) => values
            .iter()
            .for_each(|nested| assignments.extend(procedural_assignments(nested))),
        Value::Object(fields) => {
            let is_assignment = string_field(value, "kind").as_deref() == Some("Assignment");
            fields
                .iter()
                .filter(|(field, _)| {
                    !(is_assignment && (field.as_str() == "left" || field.as_str() == "right"))
                })
                .for_each(|(_, nested)| assignments.extend(procedural_assignments(nested)));
        }
        _ => {}
    }
    assignments
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
    let word = literal_word(&parameter.type_name, &parameter.value)?;
    let width = simple_type_width(&parameter.type_name)?;
    let signed = simple_type_signed(&parameter.type_name)?;
    let signed_label = if signed { "s" } else { "u" };
    Some(format!(
        "Const:{width}:{signed_label}[{}:{}]",
        word.width(),
        word.to_bin_str()
    ))
}

pub(crate) fn slang_expression_fingerprint_with_parameters(
    expression: &SlangExpressionSummary,
    parameters: &[SlangParameterSummary],
) -> Option<String> {
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
        return Some(format!("Const:{width}:{signed_label}[{literal}]"));
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
                let assignments = procedural_assignments(member);
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
    }
}

pub(crate) fn design_expr_fingerprint(design: &Design, expression: &eda_core::ir::Expr) -> String {
    use eda_core::ir::Expr;
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
            return format!(
                "Const:{}:s[{}:{}]",
                word.width(),
                word.width(),
                word.to_bin_str()
            );
        }
    }
    if let Expr::Repeat { times, e } = expression {
        return format!("Repeat:{times}:u[{}]", design_expr_fingerprint(design, e));
    }
    let operands: Vec<String> = match expression {
        Expr::Const(_) | Expr::Ref { .. } | Expr::MemoryRead { .. } => Vec::new(),
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
        Expr::Ref { net, .. } => design
            .net_decls
            .iter()
            .find(|declaration| {
                *net >= declaration.base && *net < declaration.base + declaration.width
            })
            .map(|declaration| declaration.name.clone()),
        Expr::MemoryRead { memory, .. } | Expr::MemoryReadPart { memory, .. } => design
            .memories
            .get(*memory)
            .map(|memory| memory.name.clone()),
        _ => None,
    }
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
        | Stmt::WriteMem { .. }
        | Stmt::Finish => {}
    }
    assignments
}
