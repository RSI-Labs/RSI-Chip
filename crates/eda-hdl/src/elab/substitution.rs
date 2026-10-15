//! Parameter substitution for generate constructs, functions, and statements.

use crate::ast;
use eda_core::logic::LogicWord;
use std::collections::{HashMap, HashSet};

pub(super) fn substitute_generate_var(item: &ast::Item, var: &str, value: i64) -> ast::Item {
    if value < 0 {
        return item.clone();
    }
    match item {
        ast::Item::Params(list) => ast::Item::Params(
            list.iter()
                .map(|(name, expr)| (name.clone(), subst_expr(expr, var, value)))
                .collect(),
        ),
        ast::Item::Imports(_) => item.clone(),
        ast::Item::TypeDef(_) => item.clone(),
        ast::Item::ClassDef(_) => item.clone(),
        ast::Item::Function(_) => item.clone(),
        ast::Item::Modport { .. } => item.clone(),
        ast::Item::AssertProperty {
            kind,
            label,
            clock,
            disable,
            antecedent,
            consequent,
            composition,
            overlapping,
            delay,
            repetition,
            message,
        } => ast::Item::AssertProperty {
            kind: *kind,
            label: label.clone(),
            clock: subst_sens(clock, var, value),
            disable: subst_expr(disable, var, value),
            antecedent: subst_expr(antecedent, var, value),
            consequent: subst_expr(consequent, var, value),
            composition: composition.as_ref().map(|composition| {
                Box::new(ast::PropertyComposition {
                    op: composition.op,
                    right: subst_expr(&composition.right, var, value),
                    right_delay: match &composition.right_delay {
                        ast::PropertyDelay::Fixed(delay) => {
                            ast::PropertyDelay::Fixed(subst_expr(delay, var, value))
                        }
                        ast::PropertyDelay::Range(start, end) => ast::PropertyDelay::Range(
                            subst_expr(start, var, value),
                            subst_expr(end, var, value),
                        ),
                    },
                })
            }),
            overlapping: *overlapping,
            delay: Box::new(match delay.as_ref() {
                ast::PropertyDelay::Fixed(delay) => {
                    ast::PropertyDelay::Fixed(subst_expr(delay, var, value))
                }
                ast::PropertyDelay::Range(start, end) => ast::PropertyDelay::Range(
                    subst_expr(start, var, value),
                    subst_expr(end, var, value),
                ),
            }),
            repetition: repetition.as_deref().map(|repetition| {
                Box::new(match repetition {
                    ast::PropertyRepetition::Count(count) => {
                        ast::PropertyRepetition::Count(subst_expr(count, var, value))
                    }
                    ast::PropertyRepetition::Range(start, end) => ast::PropertyRepetition::Range(
                        subst_expr(start, var, value),
                        subst_expr(end, var, value),
                    ),
                    ast::PropertyRepetition::Goto(count) => {
                        ast::PropertyRepetition::Goto(subst_expr(count, var, value))
                    }
                    ast::PropertyRepetition::GotoRange(start, end) => {
                        ast::PropertyRepetition::GotoRange(
                            subst_expr(start, var, value),
                            subst_expr(end, var, value),
                        )
                    }
                    ast::PropertyRepetition::Nonconsecutive(count) => {
                        ast::PropertyRepetition::Nonconsecutive(subst_expr(count, var, value))
                    }
                    ast::PropertyRepetition::NonconsecutiveRange(start, end) => {
                        ast::PropertyRepetition::NonconsecutiveRange(
                            subst_expr(start, var, value),
                            subst_expr(end, var, value),
                        )
                    }
                })
            }),
            message: message.clone(),
        },
        ast::Item::Generate(constructs) => ast::Item::Generate(
            constructs
                .iter()
                .map(|construct| subst_generate_construct(construct, var, value))
                .collect(),
        ),
        ast::Item::Port {
            dir,
            is_reg,
            is_signed,
            is_virtual,
            interface_name,
            modport_name,
            type_name,
            range,
            array_ranges,
            names,
        } => ast::Item::Port {
            dir: *dir,
            is_reg: *is_reg,
            is_signed: *is_signed,
            is_virtual: *is_virtual,
            interface_name: interface_name.clone(),
            modport_name: modport_name.clone(),
            type_name: type_name.clone(),
            range: range.as_ref().map(|(a, b)| {
                (
                    Box::new(subst_expr(a, var, value)),
                    Box::new(subst_expr(b, var, value)),
                )
            }),
            array_ranges: array_ranges
                .iter()
                .map(|range| {
                    range.as_ref().map(|(a, b)| {
                        (
                            Box::new(subst_expr(a, var, value)),
                            Box::new(subst_expr(b, var, value)),
                        )
                    })
                })
                .collect(),
            names: names.clone(),
        },
        ast::Item::Net {
            is_reg,
            is_signed,
            type_name,
            range,
            array_ranges,
            names,
        } => ast::Item::Net {
            is_reg: *is_reg,
            is_signed: *is_signed,
            type_name: type_name.clone(),
            range: range.as_ref().map(|(a, b)| {
                (
                    Box::new(subst_expr(a, var, value)),
                    Box::new(subst_expr(b, var, value)),
                )
            }),
            array_ranges: array_ranges
                .iter()
                .map(|range| {
                    range.as_ref().map(|(a, b)| {
                        (
                            Box::new(subst_expr(a, var, value)),
                            Box::new(subst_expr(b, var, value)),
                        )
                    })
                })
                .collect(),
            names: names
                .iter()
                .map(|(name, init)| {
                    (
                        name.clone(),
                        init.as_ref().map(|init| subst_expr(init, var, value)),
                    )
                })
                .collect(),
        },
        ast::Item::Assign { delay, lv, e } => ast::Item::Assign {
            delay: *delay,
            lv: subst_expr(lv, var, value),
            e: subst_expr(e, var, value),
        },
        ast::Item::Always { sens, body } => ast::Item::Always {
            sens: subst_sens(sens, var, value),
            body: subst_stmt(body, var, value),
        },
        ast::Item::Initial { body } => ast::Item::Initial {
            body: subst_stmt(body, var, value),
        },
        ast::Item::Gate { kind, name, args } => ast::Item::Gate {
            kind: *kind,
            name: name.clone(),
            args: args.iter().map(|arg| subst_expr(arg, var, value)).collect(),
        },
        ast::Item::Inst {
            module,
            name,
            array_ranges,
            params,
            conns,
        } => ast::Item::Inst {
            module: module.clone(),
            name: name.clone(),
            array_ranges: array_ranges
                .iter()
                .map(|(msb, lsb)| {
                    (
                        Box::new(subst_expr(msb, var, value)),
                        Box::new(subst_expr(lsb, var, value)),
                    )
                })
                .collect(),
            params: params
                .iter()
                .map(|(name, value_expr)| (name.clone(), subst_expr(value_expr, var, value)))
                .collect(),
            conns: conns
                .iter()
                .map(|conn| match conn {
                    ast::Conn::Named(name, expr) => ast::Conn::Named(
                        name.clone(),
                        expr.as_ref().map(|expr| subst_expr(expr, var, value)),
                    ),
                    ast::Conn::Pos(expr) => {
                        ast::Conn::Pos(expr.as_ref().map(|expr| subst_expr(expr, var, value)))
                    }
                })
                .collect(),
        },
    }
}

pub(super) fn subst_generate_construct(
    construct: &ast::GenerateConstruct,
    var: &str,
    value: i64,
) -> ast::GenerateConstruct {
    match construct {
        ast::GenerateConstruct::Block(block) => {
            ast::GenerateConstruct::Block(subst_generate_block(block, var, value))
        }
        ast::GenerateConstruct::Case {
            selector,
            arms,
            default,
        } => ast::GenerateConstruct::Case {
            selector: subst_expr(selector, var, value),
            arms: arms
                .iter()
                .map(|arm| ast::GenerateCaseArm {
                    labels: arm
                        .labels
                        .iter()
                        .map(|label| subst_expr(label, var, value))
                        .collect(),
                    body: subst_generate_block(&arm.body, var, value),
                })
                .collect(),
            default: default
                .as_ref()
                .map(|block| subst_generate_block(block, var, value)),
        },
        ast::GenerateConstruct::If {
            cond,
            then,
            otherwise,
        } => ast::GenerateConstruct::If {
            cond: subst_expr(cond, var, value),
            then: subst_generate_block(then, var, value),
            otherwise: otherwise
                .as_ref()
                .map(|block| subst_generate_block(block, var, value)),
        },
        ast::GenerateConstruct::For {
            var: loop_var,
            start,
            cond,
            step,
            body,
        } => ast::GenerateConstruct::For {
            var: loop_var.clone(),
            start: subst_expr(start, var, value),
            cond: subst_expr(cond, var, value),
            step: subst_expr(step, var, value),
            body: subst_generate_block(body, var, value),
        },
    }
}

pub(super) fn subst_generate_block(
    block: &ast::GenerateBlock,
    var: &str,
    value: i64,
) -> ast::GenerateBlock {
    ast::GenerateBlock {
        label: block.label.clone(),
        items: block
            .items
            .iter()
            .map(|item| substitute_generate_var(item, var, value))
            .collect(),
    }
}

pub(super) fn subst_expr(expr: &ast::Expr, var: &str, value: i64) -> ast::Expr {
    match expr {
        ast::Expr::Ident(name) if name == var => ast::Expr::Num {
            width: Some(32),
            word: LogicWord::from_u64(32, value as u64),
            orig: value.to_string(),
        },
        ast::Expr::Ident(_)
        | ast::Expr::ScopedIdent { .. }
        | ast::Expr::Member(_, _)
        | ast::Expr::MemberChain(_)
        | ast::Expr::MethodCall { .. }
        | ast::Expr::MethodCallChain { .. }
        | ast::Expr::Num { .. }
        | ast::Expr::Str(_) => expr.clone(),
        ast::Expr::New(size) => ast::Expr::New(Box::new(subst_expr(size, var, value))),
        ast::Expr::NewClass(arguments) => ast::Expr::NewClass(
            arguments
                .iter()
                .map(|argument| subst_expr(argument, var, value))
                .collect(),
        ),
        ast::Expr::Select(name, index) => {
            if name == var {
                return subst_expr(&ast::Expr::Ident(name.clone()), var, value);
            }
            ast::Expr::Select(name.clone(), Box::new(subst_expr(index, var, value)))
        }
        ast::Expr::ArraySelect(name, indices) => {
            if name == var {
                return subst_expr(&ast::Expr::Ident(name.clone()), var, value);
            }
            ast::Expr::ArraySelect(
                name.clone(),
                indices
                    .iter()
                    .map(|index| subst_expr(index, var, value))
                    .collect(),
            )
        }
        ast::Expr::ArrayIndexedPart {
            name,
            indexes,
            start,
            width,
            ascending,
        } => {
            if name == var {
                return subst_expr(&ast::Expr::Ident(name.clone()), var, value);
            }
            ast::Expr::ArrayIndexedPart {
                name: name.clone(),
                indexes: indexes
                    .iter()
                    .map(|index| subst_expr(index, var, value))
                    .collect(),
                start: Box::new(subst_expr(start, var, value)),
                width: Box::new(subst_expr(width, var, value)),
                ascending: *ascending,
            }
        }
        ast::Expr::Part(name, msb, lsb) => {
            if name == var {
                return subst_expr(&ast::Expr::Ident(name.clone()), var, value);
            }
            ast::Expr::Part(
                name.clone(),
                Box::new(subst_expr(msb, var, value)),
                Box::new(subst_expr(lsb, var, value)),
            )
        }
        ast::Expr::IndexedPart {
            name,
            start,
            width,
            ascending,
        } => {
            if name == var {
                return subst_expr(&ast::Expr::Ident(name.clone()), var, value);
            }
            ast::Expr::IndexedPart {
                name: name.clone(),
                start: Box::new(subst_expr(start, var, value)),
                width: Box::new(subst_expr(width, var, value)),
                ascending: *ascending,
            }
        }
        ast::Expr::Un(op, a) => ast::Expr::Un(*op, Box::new(subst_expr(a, var, value))),
        ast::Expr::Bin(op, a, b) => ast::Expr::Bin(
            *op,
            Box::new(subst_expr(a, var, value)),
            Box::new(subst_expr(b, var, value)),
        ),
        ast::Expr::Cond(c, t, f) => ast::Expr::Cond(
            Box::new(subst_expr(c, var, value)),
            Box::new(subst_expr(t, var, value)),
            Box::new(subst_expr(f, var, value)),
        ),
        ast::Expr::Concat(parts) => ast::Expr::Concat(
            parts
                .iter()
                .map(|part| subst_expr(part, var, value))
                .collect(),
        ),
        ast::Expr::Replicate(count, value_expr) => ast::Expr::Replicate(
            Box::new(subst_expr(count, var, value)),
            Box::new(subst_expr(value_expr, var, value)),
        ),
        ast::Expr::Call(name, args) => ast::Expr::Call(
            name.clone(),
            args.iter().map(|arg| subst_expr(arg, var, value)).collect(),
        ),
        ast::Expr::Slice {
            value: expression,
            msb,
            lsb,
        } => ast::Expr::Slice {
            value: Box::new(subst_expr(expression, var, value)),
            msb: Box::new(subst_expr(msb, var, value)),
            lsb: Box::new(subst_expr(lsb, var, value)),
        },
        ast::Expr::Resize {
            width,
            value: expression,
        } => ast::Expr::Resize {
            width: *width,
            value: Box::new(subst_expr(expression, var, value)),
        },
        ast::Expr::WildcardMatch {
            kind,
            selector,
            label,
        } => ast::Expr::WildcardMatch {
            kind: *kind,
            selector: Box::new(subst_expr(selector, var, value)),
            label: Box::new(subst_expr(label, var, value)),
        },
    }
}

pub(super) fn subst_sens(sens: &ast::Sens, var: &str, value: i64) -> ast::Sens {
    match sens {
        ast::Sens::Comb => ast::Sens::Comb,
        ast::Sens::Posedge(e) => ast::Sens::Posedge(subst_expr(e, var, value)),
        ast::Sens::Negedge(e) => ast::Sens::Negedge(subst_expr(e, var, value)),
        ast::Sens::List(list) => {
            ast::Sens::List(list.iter().map(|s| subst_sens(s, var, value)).collect())
        }
    }
}

pub(super) fn validate_function_body(definition: &ast::FunctionDef) -> Result<(), String> {
    let mut known = HashSet::new();
    for (name, _) in &definition.args {
        if name == &definition.name || !known.insert(name.clone()) {
            return Err(format!(
                "function '{}': argument '{}' shadows the return or a prior argument",
                definition.name, name
            ));
        }
    }
    let mut has_return_assignment = false;
    fn visit(
        statement: &ast::Stmt,
        definition: &ast::FunctionDef,
        known: &mut HashSet<String>,
        allow_declarations: bool,
        has_return_assignment: &mut bool,
    ) -> Result<(), String> {
        match statement {
            ast::Stmt::Block(statements) => {
                for statement in statements {
                    visit(
                        statement,
                        definition,
                        known,
                        allow_declarations,
                        has_return_assignment,
                    )?;
                }
            }
            ast::Stmt::Local { names, .. } => {
                if !allow_declarations {
                    return Err(format!(
                        "function '{}': local declarations are not supported inside branch control flow",
                        definition.name
                    ));
                }
                for (name, _) in names {
                    if name == &definition.name
                        || known.contains(name)
                        || !known.insert(name.clone())
                    {
                        return Err(format!(
                            "function '{}': local '{}' shadows the return, an argument, or a prior local",
                            definition.name, name
                        ));
                    }
                }
            }
            ast::Stmt::BL { lv, .. } => match lv {
                ast::Expr::Ident(name) => {
                    if name == &definition.name {
                        *has_return_assignment = true;
                    } else if !known.contains(name) {
                        return Err(format!(
                            "function '{}': assignment to undeclared local '{}'",
                            definition.name, name
                        ));
                    }
                }
                ast::Expr::Member(object, _) => {
                    if !known.contains(object) {
                        return Err(format!(
                            "function '{}': assignment to undeclared aggregate local '{}'",
                            definition.name, object
                        ));
                    }
                }
                ast::Expr::Select(object, _) => {
                    if !known.contains(object) {
                        return Err(format!(
                            "function '{}': assignment to undeclared array local '{}'",
                            definition.name, object
                        ));
                    }
                }
                ast::Expr::ArraySelect(object, _) => {
                    if !known.contains(object) {
                        return Err(format!(
                            "function '{}': assignment to undeclared array local '{}'",
                            definition.name, object
                        ));
                    }
                }
                ast::Expr::ArrayIndexedPart { name, .. } => {
                    if !known.contains(name) {
                        return Err(format!(
                            "function '{}': assignment to undeclared array local '{}'",
                            definition.name, name
                        ));
                    }
                }
                other => {
                    return Err(format!(
                        "function '{}': unsupported assignment target {:?}",
                        definition.name, other
                    ));
                }
            },
            ast::Stmt::NBL { .. } => {
                return Err(format!(
                    "function '{}': non-blocking assignments are not supported in functions",
                    definition.name
                ));
            }
            ast::Stmt::If { t, f, .. } => {
                visit(t, definition, known, false, has_return_assignment)?;
                if let Some(false_branch) = f {
                    visit(
                        false_branch,
                        definition,
                        known,
                        false,
                        has_return_assignment,
                    )?;
                }
            }
            ast::Stmt::Case { arms, dflt, .. } => {
                for (_, body) in arms {
                    visit(body, definition, known, false, has_return_assignment)?;
                }
                if let Some(default_body) = dflt {
                    visit(
                        default_body,
                        definition,
                        known,
                        false,
                        has_return_assignment,
                    )?;
                }
            }
            ast::Stmt::For { var, body, .. } => {
                if var == &definition.name || known.contains(var) {
                    return Err(format!(
                        "function '{}': loop variable '{}' shadows the return, an argument, or a local",
                        definition.name, var
                    ));
                }
                known.insert(var.clone());
                visit(body, definition, known, false, has_return_assignment)?;
                known.remove(var);
            }
            ast::Stmt::Repeat { body, .. } => {
                visit(body, definition, known, false, has_return_assignment)?;
            }
            ast::Stmt::While { body, .. } => {
                visit(body, definition, known, false, has_return_assignment)?;
            }
            other => {
                return Err(format!(
                    "function '{}': unsupported statement {:?} in function body",
                    definition.name, other
                ));
            }
        }
        Ok(())
    }
    for statement in &definition.body {
        visit(
            statement,
            definition,
            &mut known,
            true,
            &mut has_return_assignment,
        )?;
    }
    if !has_return_assignment {
        return Err(format!(
            "function '{}' never assigns its return value",
            definition.name
        ));
    }
    Ok(())
}

pub(super) fn loop_value(value: i64) -> ast::Expr {
    ast::Expr::Num {
        width: Some(32),
        word: LogicWord::from_u64(32, value as u64),
        orig: value.to_string(),
    }
}

pub(super) fn constant_expr(value: i64) -> ast::Expr {
    loop_value(value)
}

pub(super) fn substitute_function_arguments(
    expression: &ast::Expr,
    arguments: &HashMap<String, ast::Expr>,
) -> ast::Expr {
    match expression {
        ast::Expr::Ident(name) => arguments
            .get(name.as_str())
            .cloned()
            .unwrap_or_else(|| expression.clone()),
        ast::Expr::Member(_, _)
        | ast::Expr::MemberChain(_)
        | ast::Expr::MethodCall { .. }
        | ast::Expr::MethodCallChain { .. }
        | ast::Expr::ScopedIdent { .. }
        | ast::Expr::Num { .. }
        | ast::Expr::Str(_) => expression.clone(),
        ast::Expr::New(size) => {
            ast::Expr::New(Box::new(substitute_function_arguments(size, arguments)))
        }
        ast::Expr::NewClass(actuals) => ast::Expr::NewClass(
            actuals
                .iter()
                .map(|actual| substitute_function_arguments(actual, arguments))
                .collect(),
        ),
        ast::Expr::Select(name, index) => ast::Expr::Select(
            name.clone(),
            Box::new(substitute_function_arguments(index, arguments)),
        ),
        ast::Expr::ArraySelect(name, indexes) => ast::Expr::ArraySelect(
            name.clone(),
            indexes
                .iter()
                .map(|index| substitute_function_arguments(index, arguments))
                .collect(),
        ),
        ast::Expr::ArrayIndexedPart {
            name,
            indexes,
            start,
            width,
            ascending,
        } => ast::Expr::ArrayIndexedPart {
            name: name.clone(),
            indexes: indexes
                .iter()
                .map(|index| substitute_function_arguments(index, arguments))
                .collect(),
            start: Box::new(substitute_function_arguments(start, arguments)),
            width: Box::new(substitute_function_arguments(width, arguments)),
            ascending: *ascending,
        },
        ast::Expr::Part(name, msb, lsb) => ast::Expr::Part(
            name.clone(),
            Box::new(substitute_function_arguments(msb, arguments)),
            Box::new(substitute_function_arguments(lsb, arguments)),
        ),
        ast::Expr::IndexedPart {
            name,
            start,
            width,
            ascending,
        } => ast::Expr::IndexedPart {
            name: name.clone(),
            start: Box::new(substitute_function_arguments(start, arguments)),
            width: Box::new(substitute_function_arguments(width, arguments)),
            ascending: *ascending,
        },
        ast::Expr::Un(op, value) => ast::Expr::Un(
            *op,
            Box::new(substitute_function_arguments(value, arguments)),
        ),
        ast::Expr::Bin(op, left, right) => ast::Expr::Bin(
            *op,
            Box::new(substitute_function_arguments(left, arguments)),
            Box::new(substitute_function_arguments(right, arguments)),
        ),
        ast::Expr::Cond(condition, true_value, false_value) => ast::Expr::Cond(
            Box::new(substitute_function_arguments(condition, arguments)),
            Box::new(substitute_function_arguments(true_value, arguments)),
            Box::new(substitute_function_arguments(false_value, arguments)),
        ),
        ast::Expr::Concat(parts) => ast::Expr::Concat(
            parts
                .iter()
                .map(|part| substitute_function_arguments(part, arguments))
                .collect(),
        ),
        ast::Expr::Replicate(count, value) => ast::Expr::Replicate(
            Box::new(substitute_function_arguments(count, arguments)),
            Box::new(substitute_function_arguments(value, arguments)),
        ),
        ast::Expr::Call(name, args) => ast::Expr::Call(
            name.clone(),
            args.iter()
                .map(|arg| substitute_function_arguments(arg, arguments))
                .collect(),
        ),
        ast::Expr::Slice { value, msb, lsb } => ast::Expr::Slice {
            value: Box::new(substitute_function_arguments(value, arguments)),
            msb: Box::new(substitute_function_arguments(msb, arguments)),
            lsb: Box::new(substitute_function_arguments(lsb, arguments)),
        },
        ast::Expr::Resize { width, value } => ast::Expr::Resize {
            width: *width,
            value: Box::new(substitute_function_arguments(value, arguments)),
        },
        ast::Expr::WildcardMatch {
            kind,
            selector,
            label,
        } => ast::Expr::WildcardMatch {
            kind: *kind,
            selector: Box::new(substitute_function_arguments(selector, arguments)),
            label: Box::new(substitute_function_arguments(label, arguments)),
        },
    }
}

pub(super) fn subst_stmt(stmt: &ast::Stmt, var: &str, value: i64) -> ast::Stmt {
    match stmt {
        ast::Stmt::Block(list) => {
            ast::Stmt::Block(list.iter().map(|s| subst_stmt(s, var, value)).collect())
        }
        ast::Stmt::Local {
            type_name,
            range,
            array_ranges,
            names,
        } => ast::Stmt::Local {
            type_name: type_name.clone(),
            array_ranges: array_ranges
                .iter()
                .map(|range| {
                    range.as_ref().map(|(msb, lsb)| {
                        (
                            Box::new(subst_expr(msb, var, value)),
                            Box::new(subst_expr(lsb, var, value)),
                        )
                    })
                })
                .collect(),
            range: range.as_ref().map(|(msb, lsb)| {
                (
                    Box::new(subst_expr(msb, var, value)),
                    Box::new(subst_expr(lsb, var, value)),
                )
            }),
            names: names
                .iter()
                .map(|(name, init)| {
                    (
                        name.clone(),
                        init.as_ref().map(|expr| subst_expr(expr, var, value)),
                    )
                })
                .collect(),
        },
        ast::Stmt::For {
            var,
            start,
            cond,
            step,
            body,
        } => ast::Stmt::For {
            var: var.clone(),
            start: subst_expr(start, var, value),
            cond: subst_expr(cond, var, value),
            step: subst_expr(step, var, value),
            body: Box::new(subst_stmt(body, var, value)),
        },
        ast::Stmt::Repeat { count, body } => ast::Stmt::Repeat {
            count: subst_expr(count, var, value),
            body: Box::new(subst_stmt(body, var, value)),
        },
        ast::Stmt::While { cond, body } => ast::Stmt::While {
            cond: subst_expr(cond, var, value),
            body: Box::new(subst_stmt(body, var, value)),
        },
        ast::Stmt::Assert { cond, message } => ast::Stmt::Assert {
            cond: subst_expr(cond, var, value),
            message: message.clone(),
        },
        ast::Stmt::MethodCall {
            object,
            method,
            arguments,
        } => ast::Stmt::MethodCall {
            object: object.clone(),
            method: method.clone(),
            arguments: arguments
                .iter()
                .map(|argument| subst_expr(argument, var, value))
                .collect(),
        },
        ast::Stmt::Forever(s) => ast::Stmt::Forever(Box::new(subst_stmt(s, var, value))),
        ast::Stmt::NBL { lv, e } => ast::Stmt::NBL {
            lv: subst_expr(lv, var, value),
            e: subst_expr(e, var, value),
        },
        ast::Stmt::BL { lv, e } => ast::Stmt::BL {
            lv: subst_expr(lv, var, value),
            e: subst_expr(e, var, value),
        },
        ast::Stmt::If { c, t, f } => ast::Stmt::If {
            c: subst_expr(c, var, value),
            t: Box::new(subst_stmt(t, var, value)),
            f: f.as_ref().map(|s| Box::new(subst_stmt(s, var, value))),
        },
        ast::Stmt::Case {
            e,
            arms,
            dflt,
            kind,
        } => ast::Stmt::Case {
            e: subst_expr(e, var, value),
            arms: arms
                .iter()
                .map(|(labels, body)| {
                    (
                        labels
                            .iter()
                            .map(|label| subst_expr(label, var, value))
                            .collect(),
                        subst_stmt(body, var, value),
                    )
                })
                .collect(),
            dflt: dflt.as_ref().map(|s| Box::new(subst_stmt(s, var, value))),
            kind: *kind,
        },
        ast::Stmt::Delay { ns, body } => ast::Stmt::Delay {
            ns: *ns,
            body: Box::new(subst_stmt(body, var, value)),
        },
        ast::Stmt::Display { fmt, args, monitor } => ast::Stmt::Display {
            fmt: fmt.clone(),
            args: args.iter().map(|arg| subst_expr(arg, var, value)).collect(),
            monitor: *monitor,
        },
        ast::Stmt::Finish => ast::Stmt::Finish,
    }
}
