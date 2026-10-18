//! Expression operator and constant helpers for direct slang lowering.

use crate::ast_util::{
    constant_json_word, expression_symbol, expression_symbol_id, integer_literal_word,
    named_constant_word, string_field, usize_field, with_source_span,
};
use crate::types::AstPackedDimension;
use crate::types::{simple_type_signed, simple_type_width};
use eda_core::logic::{LogicValue, LogicWord};
use serde_json::Value;

use super::hierarchy::full_range;
use super::{AstEventArrayCandidate, AstEventArraySelection, AstLoweredSymbol};

pub(super) fn binary_operator(operator: Option<&str>) -> Result<eda_core::ir::BinOp, String> {
    match operator {
        Some("Add") => Ok(eda_core::ir::BinOp::Add),
        Some("Subtract") => Ok(eda_core::ir::BinOp::Sub),
        Some("Multiply") => Ok(eda_core::ir::BinOp::Mul),
        Some("Divide") => Ok(eda_core::ir::BinOp::Div),
        Some("Modulus") => Ok(eda_core::ir::BinOp::Mod),
        Some("Power") => Ok(eda_core::ir::BinOp::Pow),
        Some("BinaryAnd") => Ok(eda_core::ir::BinOp::And),
        Some("BinaryOr") => Ok(eda_core::ir::BinOp::Or),
        Some("BinaryXor") => Ok(eda_core::ir::BinOp::Xor),
        Some("LogicalShiftLeft") => Ok(eda_core::ir::BinOp::Shl),
        Some("LogicalShiftRight") => Ok(eda_core::ir::BinOp::Shr),
        Some("ArithmeticShiftLeft") => Ok(eda_core::ir::BinOp::Shl),
        Some("ArithmeticShiftRight") => Ok(eda_core::ir::BinOp::Shr),
        Some("LogicalAnd") => Ok(eda_core::ir::BinOp::LogAnd),
        Some("LogicalOr") => Ok(eda_core::ir::BinOp::LogOr),
        Some("Equality") => Ok(eda_core::ir::BinOp::Eq),
        Some("Inequality") => Ok(eda_core::ir::BinOp::Ne),
        Some("CaseEquality") => Ok(eda_core::ir::BinOp::CaseEq),
        Some("CaseInequality") => Ok(eda_core::ir::BinOp::CaseNe),
        Some("LessThan") => Ok(eda_core::ir::BinOp::Lt),
        Some("LessThanEqual") => Ok(eda_core::ir::BinOp::Le),
        Some("GreaterThan") => Ok(eda_core::ir::BinOp::Gt),
        Some("GreaterThanEqual") => Ok(eda_core::ir::BinOp::Ge),
        Some(other) => Err(format!(
            "direct slang-AST lowering does not support binary operator {other:?}"
        )),
        None => Err("binary expression has no operator".into()),
    }
}

pub(super) fn continuous_assignment_delay(member: &Value) -> Result<u64, String> {
    let Some(delay) = member.get("delay") else {
        return Ok(0);
    };
    if delay.is_null() {
        return Ok(0);
    }
    if string_field(delay, "kind").as_deref() != Some("Delay") {
        return Err("continuous assignment has an unsupported delay form".to_string());
    }
    if delay.get("minValue").is_some()
        || delay.get("typicalValue").is_some()
        || delay.get("maxValue").is_some()
    {
        return Err("min:typ:max continuous-assignment delays are not supported".to_string());
    }
    let expression = delay
        .get("expr")
        .ok_or_else(|| "continuous-assignment delay has no expression".to_string())?;
    let word = integer_literal_word(expression)
        .or_else(|| named_constant_word(expression))
        .and_then(|word| word.to_u64())
        .ok_or_else(|| "continuous-assignment delay must be an integer constant".to_string())?;
    Ok(word)
}

pub(super) fn constant_index(value: &serde_json::Value) -> Result<usize, String> {
    integer_literal_word(value)
        .or_else(|| named_constant_word(value))
        .or_else(|| constant_json_word(value))
        .and_then(|word| word.to_u64())
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| "direct lowering requires an integer constant index".to_string())
}

pub(super) fn constant_i64(value: &serde_json::Value) -> Result<i64, String> {
    let word = integer_literal_word(value)
        .or_else(|| named_constant_word(value))
        .or_else(|| constant_json_word(value))
        .ok_or_else(|| "direct lowering requires an integer constant index".to_string())?;
    if word.width() == 0 {
        return Err("integer constant index has zero width".to_string());
    }
    if word.width() > u64::BITS as usize {
        return Err("integer constant index is too large".to_string());
    }
    let mut magnitude = 0_u128;
    for (bit, value) in word.bits.iter().enumerate() {
        if value.as_bool() == Some(true) {
            magnitude |= 1_u128 << bit;
        }
    }
    let signed = string_field(value, "type")
        .and_then(|type_name| simple_type_signed(&type_name))
        .unwrap_or(false);
    let sign_bit = 1_u128 << (word.width() - 1);
    let signed_value = if signed && word.width() > 0 && magnitude & sign_bit != 0 {
        magnitude as i128 - (1_i128 << word.width())
    } else {
        magnitude as i128
    };
    i64::try_from(signed_value).map_err(|_| "integer constant index is out of range".to_string())
}

impl super::SlangAstLowerer {
    fn clocking_variable_symbol(
        &self,
        value: &serde_json::Value,
    ) -> Result<Option<AstLoweredSymbol>, String> {
        if let Some(symbol_id) = expression_symbol_id(value) {
            if let Some(variable) = self.clocking_variables.get(&symbol_id) {
                if variable.direction == eda_core::ir::Dir::Out {
                    return Err("clocking output variable drive semantics are not supported".into());
                }
                return Ok(Some(variable.symbol.clone()));
            }
        }
        Ok(None)
    }

    pub(super) fn named_symbol(
        &self,
        value: &serde_json::Value,
    ) -> Result<AstLoweredSymbol, String> {
        let display_name =
            expression_symbol(value).ok_or_else(|| "expression has no symbol".to_string())?;
        if let Some(symbol_id) = expression_symbol_id(value) {
            if let Some(symbol) = self.external_symbols.get(&symbol_id) {
                if self.external_symbol_directions.get(&symbol_id) == Some(&eda_core::ir::Dir::In) {
                    return Err("modport input variable cannot be assigned directly".into());
                }
                return Ok(symbol.clone());
            }
        }
        if let Some(symbol_id) = expression_symbol_id(value) {
            if let Some(variable) = self.clocking_variables.get(&symbol_id) {
                if variable.direction == eda_core::ir::Dir::In {
                    return Err("clocking input variable cannot be assigned directly".into());
                }
                return Ok(variable.symbol.clone());
            }
        }
        if let Some(symbol_id) = expression_symbol_id(value) {
            if let Some(symbol) = self.local_symbols.get(&symbol_id) {
                return Ok(symbol.clone());
            }
        }
        self.symbols
            .get(&display_name)
            .cloned()
            .ok_or_else(|| format!("unknown direct-lowering symbol '{display_name}'"))
    }

    pub(super) fn lower_lvalue(
        &self,
        value: &serde_json::Value,
    ) -> Result<eda_core::ir::LValue, String> {
        self.lower_lvalue_inner(value)
            .map_err(|error| with_source_span(value, error))
    }

    fn lower_lvalue_inner(
        &self,
        value: &serde_json::Value,
    ) -> Result<eda_core::ir::LValue, String> {
        match string_field(value, "kind").as_deref() {
            Some("NamedValue" | "HierarchicalValue") => {
                let symbol = self.named_symbol(value)?;
                Ok(eda_core::ir::LValue {
                    net: symbol.base,
                    decl_width: symbol.width,
                    range: full_range(symbol.width),
                })
            }
            Some("Conversion") => {
                let operand = value
                    .get("operand")
                    .ok_or_else(|| "conversion lvalue has no operand".to_string())?;
                self.lower_lvalue(operand)
            }
            Some("ElementSelect") => {
                if self.memory_element_select(value)?.is_some() {
                    return Err(
                        "sparse-memory selects require the memory assignment lowering path".into(),
                    );
                }
                let (lv, index) = self.lower_constant_select_lvalue(value)?;
                Ok(eda_core::ir::LValue {
                    net: lv.net,
                    decl_width: lv.decl_width,
                    range: eda_core::ir::Range::Bit(index),
                })
            }
            Some("RangeSelect") => {
                if self.memory_range_select(value)?.is_some() {
                    return Err(
                        "sparse-memory part selects require the memory assignment lowering path"
                            .into(),
                    );
                }
                let (lv, msb, lsb) = self.lower_constant_range_lvalue(value)?;
                Ok(eda_core::ir::LValue {
                    net: lv.net,
                    decl_width: lv.decl_width,
                    range: eda_core::ir::Range::Span { msb, lsb },
                })
            }
            Some("MemberAccess") => {
                let (lv, msb, lsb) = self.lower_struct_member_lvalue(value)?;
                Ok(eda_core::ir::LValue {
                    net: lv.net,
                    decl_width: lv.decl_width,
                    range: eda_core::ir::Range::Span { msb, lsb },
                })
            }
            Some(kind) => Err(format!(
                "direct slang-AST lowering does not support lvalue kind '{kind}'"
            )),
            None => Err("slang lvalue has no kind".into()),
        }
    }

    pub(super) fn lower_expression(
        &self,
        value: &serde_json::Value,
    ) -> Result<eda_core::ir::Expr, String> {
        self.lower_expression_inner(value)
            .map_err(|error| with_source_span(value, error))
    }

    fn lower_expression_inner(
        &self,
        value: &serde_json::Value,
    ) -> Result<eda_core::ir::Expr, String> {
        use eda_core::ir::{Expr, UnOp};
        let kind = string_field(value, "kind").unwrap_or_default();
        match kind.as_str() {
            "NamedValue" | "HierarchicalValue" => {
                let name = expression_symbol(value)
                    .ok_or_else(|| "expression has no symbol".to_string())?;
                if let Some(symbol) = self.clocking_variable_symbol(value)? {
                    return Ok(Expr::Ref {
                        net: symbol.base,
                        decl_width: symbol.width,
                        range: full_range(symbol.width),
                        signed: symbol.signed,
                    });
                }
                if let Some(symbol_id) = expression_symbol_id(value) {
                    if let Some(symbol) = self.external_symbols.get(&symbol_id) {
                        return Ok(Expr::Ref {
                            net: symbol.base,
                            decl_width: symbol.width,
                            range: full_range(symbol.width),
                            signed: symbol.signed,
                        });
                    }
                }
                if let Some(symbol_id) = expression_symbol_id(value) {
                    if let Some(symbol) = self.local_symbols.get(&symbol_id) {
                        return Ok(Expr::Ref {
                            net: symbol.base,
                            decl_width: symbol.width,
                            range: full_range(symbol.width),
                            signed: symbol.signed,
                        });
                    }
                }
                if let Some(word) = self.constants.get(&name).cloned() {
                    return Ok(Expr::Const(word));
                }
                if let Some(word) = self.enum_constants.get(&name).cloned() {
                    return Ok(Expr::Const(word));
                }
                if let Some(symbol) = self.symbols.get(&name) {
                    Ok(Expr::Ref {
                        net: symbol.base,
                        decl_width: symbol.width,
                        range: full_range(symbol.width),
                        signed: symbol.signed,
                    })
                } else {
                    named_constant_word(value)
                        .map(Expr::Const)
                        .ok_or_else(|| format!("unknown direct-lowering symbol '{name}'"))
                }
            }
            "IntegerLiteral" => {
                let word = integer_literal_word(value)
                    .or_else(|| constant_json_word(value))
                    .ok_or_else(|| "invalid slang integer literal".to_string())?;
                let signed = string_field(value, "type")
                    .and_then(|type_name| simple_type_signed(&type_name))
                    .unwrap_or(false);
                if signed {
                    Ok(Expr::Signed {
                        e: Box::new(Expr::Const(word)),
                        signed,
                    })
                } else {
                    Ok(Expr::Const(word))
                }
            }
            "StringLiteral" => {
                let literal = string_field(value, "literal")
                    .ok_or_else(|| "string literal has no value".to_string())?;
                let bytes = literal.as_bytes();
                let width = string_field(value, "type")
                    .and_then(|type_name| simple_type_width(&type_name))
                    .ok_or_else(|| "string literal has unresolved width".to_string())?;
                if bytes.len() * 8 > width {
                    return Err(format!(
                        "string literal length {} exceeds its {}-bit packed representation",
                        bytes.len(),
                        width
                    ));
                }
                Ok(Expr::Const(LogicWord::from_bytes_le(width, bytes)))
            }
            "Call" => self.lower_function_call(value),
            "Conversion" => {
                let operand = value
                    .get("operand")
                    .ok_or_else(|| "conversion has no operand".to_string())?;
                if string_field(operand, "kind").as_deref() == Some("Streaming") {
                    let target_type = string_field(value, "type").unwrap_or_default();
                    let target_width = simple_type_width(&target_type).ok_or_else(|| {
                        format!("streaming conversion has unresolved target type '{target_type}'")
                    })?;
                    return self.lower_streaming(operand, target_width);
                }
                if string_field(value, "type").as_deref() == Some("string")
                    && string_field(operand, "kind").as_deref() == Some("Concatenation")
                {
                    let operands = operand
                        .get("operands")
                        .and_then(serde_json::Value::as_array)
                        .ok_or_else(|| "string concatenation has no operands".to_string())?;
                    return self.lower_string_concatenation(operands);
                }
                self.lower_integral_conversion(value)
            }
            "UnaryOp" => {
                let operand = value
                    .get("operand")
                    .ok_or_else(|| "unary expression has no operand".to_string())?;
                match string_field(value, "op").as_deref() {
                    Some("LogicalNot") => Ok(Expr::Un {
                        op: UnOp::Not,
                        a: Box::new(Expr::Un {
                            op: UnOp::OrR,
                            a: Box::new(self.lower_expression(operand)?),
                        }),
                    }),
                    Some("BitwiseNot" | "BitwiseAnd" | "BitwiseOr" | "BitwiseXor") => {
                        let op = match string_field(value, "op").as_deref() {
                            Some("BitwiseAnd") => UnOp::AndR,
                            Some("BitwiseOr") => UnOp::OrR,
                            Some("BitwiseXor") => UnOp::XorR,
                            _ => UnOp::Not,
                        };
                        Ok(Expr::Un {
                            op,
                            a: Box::new(self.lower_expression(operand)?),
                        })
                    }
                    Some("BitwiseNand" | "BitwiseNor" | "BitwiseXnor") => {
                        let reduction = match string_field(value, "op").as_deref() {
                            Some("BitwiseNand") => UnOp::AndR,
                            Some("BitwiseNor") => UnOp::OrR,
                            _ => UnOp::XorR,
                        };
                        Ok(Expr::Un {
                            op: UnOp::Not,
                            a: Box::new(Expr::Un {
                                op: reduction,
                                a: Box::new(self.lower_expression(operand)?),
                            }),
                        })
                    }
                    Some("Plus") => self.lower_expression(operand),
                    Some("Minus") => {
                        let expression = self.lower_expression(operand)?;
                        let type_name = string_field(value, "type").unwrap_or_default();
                        let width = simple_type_width(&type_name)
                            .unwrap_or(expression.width())
                            .max(1);
                        let zero = Expr::Const(LogicWord::zero(width));
                        let zero = if simple_type_signed(&type_name).unwrap_or(false) {
                            Expr::Signed {
                                e: Box::new(zero),
                                signed: true,
                            }
                        } else {
                            zero
                        };
                        Ok(Expr::Bin {
                            op: eda_core::ir::BinOp::Sub,
                            a: Box::new(zero),
                            b: Box::new(expression),
                        })
                    }
                    Some(op) => Err(format!(
                        "direct slang-AST lowering does not support unary operator '{op}'"
                    )),
                    None => Err("unary expression has no operator".into()),
                }
            }
            "BinaryOp" => {
                if matches!(
                    string_field(value, "op").as_deref(),
                    Some(
                        "Equality"
                            | "Inequality"
                            | "LessThan"
                            | "LessThanEqual"
                            | "GreaterThan"
                            | "GreaterThanEqual"
                    )
                ) && value
                    .get("left")
                    .is_some_and(|left| string_field(left, "type").as_deref() == Some("string"))
                {
                    let left = value
                        .get("left")
                        .ok_or_else(|| "string comparison has no left operand".to_string())?;
                    let right = value
                        .get("right")
                        .ok_or_else(|| "string comparison has no right operand".to_string())?;
                    let comparison = self.lower_string_compare_operands(left, right, false)?;
                    let comparison_operator = match string_field(value, "op").as_deref() {
                        Some("Equality") => eda_core::ir::BinOp::Eq,
                        Some("Inequality") => eda_core::ir::BinOp::Ne,
                        Some("LessThan") => eda_core::ir::BinOp::Lt,
                        Some("LessThanEqual") => eda_core::ir::BinOp::Le,
                        Some("GreaterThan") => eda_core::ir::BinOp::Gt,
                        _ => eda_core::ir::BinOp::Ge,
                    };
                    return Ok(Expr::Bin {
                        op: comparison_operator,
                        a: Box::new(comparison),
                        b: Box::new(Expr::Signed {
                            e: Box::new(Expr::Const(LogicWord::zero(32))),
                            signed: true,
                        }),
                    });
                }
                if matches!(
                    string_field(value, "op").as_deref(),
                    Some("WildcardEquality" | "WildcardInequality")
                ) {
                    let equality = self.lower_wildcard_equality(value)?;
                    if string_field(value, "op").as_deref() == Some("WildcardInequality") {
                        return Ok(Expr::Un {
                            op: UnOp::Not,
                            a: Box::new(equality),
                        });
                    }
                    return Ok(equality);
                }
                if string_field(value, "op").as_deref() == Some("BinaryXnor") {
                    let left = value
                        .get("left")
                        .ok_or_else(|| "binary expression has no left operand".to_string())?;
                    let right = value
                        .get("right")
                        .ok_or_else(|| "binary expression has no right operand".to_string())?;
                    return Ok(Expr::Un {
                        op: UnOp::Not,
                        a: Box::new(Expr::Bin {
                            op: eda_core::ir::BinOp::Xor,
                            a: Box::new(self.lower_expression(left)?),
                            b: Box::new(self.lower_expression(right)?),
                        }),
                    });
                }
                if matches!(
                    string_field(value, "op").as_deref(),
                    Some("LogicalImplication" | "LogicalEquivalence")
                ) {
                    let left = value
                        .get("left")
                        .ok_or_else(|| "logical expression has no left operand".to_string())?;
                    let right = value
                        .get("right")
                        .ok_or_else(|| "logical expression has no right operand".to_string())?;
                    let left = self.lower_expression(left)?;
                    let right = self.lower_expression(right)?;
                    let not_left = Expr::Un {
                        op: UnOp::Not,
                        a: Box::new(left.clone()),
                    };
                    if string_field(value, "op").as_deref() == Some("LogicalImplication") {
                        return Ok(Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(not_left),
                            b: Box::new(right.clone()),
                        });
                    }
                    let not_right = Expr::Un {
                        op: UnOp::Not,
                        a: Box::new(right.clone()),
                    };
                    return Ok(Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(not_left),
                            b: Box::new(right.clone()),
                        }),
                        b: Box::new(Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(not_right),
                            b: Box::new(left),
                        }),
                    });
                }
                if string_field(value, "op").as_deref() == Some("Power") {
                    let too_wide = ["left", "right"].iter().any(|field| {
                        value
                            .get(field)
                            .and_then(|operand| string_field(operand, "type"))
                            .and_then(|type_name| simple_type_width(&type_name))
                            .is_some_and(|width| width > u64::BITS as usize)
                    });
                    if too_wide {
                        return Err(
                            "direct slang-AST lowering supports power operands only up to 64 bits"
                                .to_string(),
                        );
                    }
                }
                let op = binary_operator(string_field(value, "op").as_deref())?;
                let left = value
                    .get("left")
                    .ok_or_else(|| "binary expression has no left operand".to_string())?;
                let right = value
                    .get("right")
                    .ok_or_else(|| "binary expression has no right operand".to_string())?;
                Ok(Expr::Bin {
                    op,
                    a: Box::new(self.lower_expression(left)?),
                    b: Box::new(self.lower_expression(right)?),
                })
            }
            "Concatenation" => {
                let operands = value
                    .get("operands")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| "concatenation has no operands".to_string())?;
                if string_field(value, "type").as_deref() == Some("string") {
                    return self.lower_string_concatenation(operands);
                }
                let mut parts = Vec::new();
                for operand in operands {
                    parts.push(self.lower_expression(operand)?);
                }
                Ok(Expr::Concat(parts))
            }
            "Replication" => {
                let count = value
                    .get("count")
                    .ok_or_else(|| "replication has no count".to_string())?;
                let concat = value
                    .get("concat")
                    .ok_or_else(|| "replication has no concatenated value".to_string())?;
                let times = usize::try_from(constant_i64(count)?)
                    .map_err(|_| "replication count is out of range".to_string())?;
                Ok(Expr::Repeat {
                    times,
                    e: Box::new(self.lower_expression(concat)?),
                })
            }
            "Streaming" => {
                Err("streaming expressions require a sized conversion context".to_string())
            }
            "Inside" => self.lower_inside(value),
            "SimpleAssignmentPattern" | "StructuredAssignmentPattern" => {
                self.lower_struct_assignment_pattern(value)
            }
            "ConditionalOp" => {
                let conditions = value
                    .get("conditions")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| "conditional expression has no conditions".to_string())?;
                if conditions.len() != 1 {
                    return Err(
                        "direct slang-AST lowering supports only one conditional condition".into(),
                    );
                }
                let condition = conditions
                    .first()
                    .and_then(|condition| condition.get("expr"))
                    .ok_or_else(|| "conditional condition has no expression".to_string())?;
                let left = value
                    .get("left")
                    .ok_or_else(|| "conditional expression has no true value".to_string())?;
                let right = value
                    .get("right")
                    .ok_or_else(|| "conditional expression has no false value".to_string())?;
                Ok(Expr::Cond {
                    c: Box::new(self.lower_expression(condition)?),
                    t: Box::new(self.lower_expression(left)?),
                    f: Box::new(self.lower_expression(right)?),
                })
            }
            "ElementSelect" => {
                if let Some(selected) = value.get("value") {
                    if string_field(selected, "type").as_deref() == Some("string") {
                        let selector = value
                            .get("selector")
                            .ok_or_else(|| "string select has no selector".to_string())?;
                        return self.lower_string_get_byte(selected, selector);
                    }
                }
                if let Some((memory, indexes)) = self.memory_element_select(value)? {
                    let symbol = self.design.memories[memory].clone();
                    let signed = self
                        .design
                        .net_decls
                        .iter()
                        .find(|declaration| declaration.name == symbol.name)
                        .map(|declaration| declaration.is_signed)
                        .unwrap_or(false);
                    return Ok(Expr::MemoryRead {
                        memory,
                        indexes,
                        width: symbol.element_width,
                        signed,
                    });
                }
                if let Some(selected) = self.lower_packed_element_select(value)? {
                    return Ok(selected);
                }
                if let Some(selected) = value.get("value") {
                    if string_field(selected, "kind").as_deref() == Some("NamedValue")
                        && self.direct_packed_symbol(selected)?.is_none()
                    {
                        let selector = value
                            .get("selector")
                            .ok_or_else(|| "element select has no selector".to_string())?;
                        let index = constant_i64(selector)?;
                        let base = self.lower_expression(selected)?;
                        if index < 0 || index >= base.width() as i64 {
                            return Err(format!(
                                "constant expression select index {index} is out of range for width {}",
                                base.width()
                            ));
                        }
                        let index = index as usize;
                        return Ok(Expr::Index {
                            e: Box::new(base),
                            msb: index,
                            lsb: index,
                        });
                    }
                }
                let (base, index) = self.lower_constant_select(value)?;
                Ok(Expr::Index {
                    e: Box::new(base),
                    msb: index,
                    lsb: index,
                })
            }
            "RangeSelect" => {
                if let Some((memory, indexes, start, width, ascending)) =
                    self.memory_range_select(value)?
                {
                    return Ok(Expr::MemoryReadPart {
                        memory,
                        indexes,
                        start: Box::new(start),
                        width,
                        ascending,
                    });
                }
                if let Some(selected) = self.lower_dynamic_indexed_range_select(value)? {
                    return Ok(selected);
                }
                let (base, msb, lsb) = self.lower_constant_range(value)?;
                Ok(Expr::Index {
                    e: Box::new(base),
                    msb,
                    lsb,
                })
            }
            "MemberAccess" => {
                let (base, msb, lsb) = self.lower_struct_member(value)?;
                Ok(Expr::Index {
                    e: Box::new(base),
                    msb,
                    lsb,
                })
            }
            _ => Err(format!(
                "direct slang-AST lowering does not support expression kind '{kind}'"
            )),
        }
    }

    pub(super) fn lower_packed_element_select(
        &self,
        value: &serde_json::Value,
    ) -> Result<Option<eda_core::ir::Expr>, String> {
        if string_field(value, "kind").as_deref() != Some("ElementSelect") {
            return Ok(None);
        }
        let mut selectors = Vec::new();
        let mut selected = value;
        while string_field(selected, "kind").as_deref() == Some("ElementSelect") {
            selectors.push(
                selected
                    .get("selector")
                    .cloned()
                    .ok_or_else(|| "packed select has no selector".to_string())?,
            );
            selected = selected
                .get("value")
                .ok_or_else(|| "packed select has no selected value".to_string())?;
        }
        let Some(symbol) = self.direct_packed_symbol(selected)? else {
            return Ok(None);
        };
        selectors.reverse();
        if selectors.is_empty() || selectors.len() > symbol.dimensions.len() {
            return Err(format!(
                "packed select rank {} exceeds the declaration's {} packed dimensions",
                selectors.len(),
                symbol.dimensions.len()
            ));
        }
        let base = self.lower_expression(selected)?;
        self.select_packed_dimension(&symbol, &selectors, 0, base)
            .map(Some)
    }

    pub(super) fn lower_dynamic_indexed_range_select(
        &self,
        value: &serde_json::Value,
    ) -> Result<Option<eda_core::ir::Expr>, String> {
        use eda_core::ir::{BinOp, Expr};

        if string_field(value, "kind").as_deref() != Some("RangeSelect") {
            return Ok(None);
        }
        let selection_kind = string_field(value, "selectionKind").unwrap_or_default();
        if !matches!(selection_kind.as_str(), "IndexedUp" | "IndexedDown") {
            return Ok(None);
        }
        let selected = value
            .get("value")
            .ok_or_else(|| "indexed packed select has no selected value".to_string())?;
        let Some(symbol) = self.direct_packed_symbol(selected)? else {
            return Ok(None);
        };
        let Some((declaration_msb, declaration_lsb)) = symbol.bounds else {
            return Err(
                "multi-packed indexed range selects require dimension-aware lowering and are not supported yet"
                    .to_string(),
            );
        };
        let start_value = value
            .get("left")
            .ok_or_else(|| "indexed packed select has no start".to_string())?;
        let width_value = value
            .get("right")
            .ok_or_else(|| "indexed packed select has no width".to_string())?;
        let width = constant_index(width_value)?;
        if width == 0 || width > symbol.width {
            return Err(format!(
                "indexed packed select width {width} exceeds declaration width {}",
                symbol.width
            ));
        }
        let width_i64 = i64::try_from(width)
            .map_err(|_| format!("indexed packed select width {width} is outside i64 bounds"))?;

        let start = self.lower_expression(start_value)?;
        let start_width = start.width().max(1);
        let mut selected_expression = Expr::Const(LogicWord::xes(width));
        let lower = declaration_msb.min(declaration_lsb);
        let upper = declaration_msb.max(declaration_lsb);
        let starts = if selection_kind == "IndexedUp" {
            lower..=upper + 1 - width_i64
        } else {
            lower + width_i64 - 1..=upper
        };
        for logical_start in starts {
            let (low_logical, high_logical) = if selection_kind == "IndexedUp" {
                (logical_start, logical_start + width_i64 - 1)
            } else {
                (logical_start - width_i64 + 1, logical_start)
            };
            let first = self.declared_bit_index(&symbol, low_logical)?;
            let second = self.declared_bit_index(&symbol, high_logical)?;
            let (msb, lsb) = (first.max(second), first.min(second));
            let candidate = Expr::Index {
                e: Box::new(self.lower_expression(selected)?),
                msb,
                lsb,
            };
            if constant_i64(start_value).is_ok() {
                let constant_start = constant_i64(start_value)?;
                if constant_start == logical_start {
                    return Ok(Some(candidate));
                }
                continue;
            }
            let condition = Expr::Bin {
                op: BinOp::Eq,
                a: Box::new(start.clone()),
                b: Box::new(Expr::Const(LogicWord::from_u64(
                    start_width,
                    logical_start as u64,
                ))),
            };
            selected_expression = Expr::Cond {
                c: Box::new(condition),
                t: Box::new(candidate),
                f: Box::new(selected_expression),
            };
        }
        if constant_i64(start_value).is_ok() {
            return Err("constant indexed packed select start is out of range".to_string());
        }
        Ok(Some(selected_expression))
    }

    pub(super) fn lower_streaming(
        &self,
        value: &serde_json::Value,
        target_width: usize,
    ) -> Result<eda_core::ir::Expr, String> {
        use eda_core::ir::Expr;

        if string_field(value, "kind").as_deref() != Some("Streaming") {
            return Err("streaming lowering requires a Streaming expression".to_string());
        }
        let streams = value
            .get("streams")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "streaming expression has no stream operands".to_string())?;
        if streams.is_empty() {
            return Err("streaming expression has an empty stream list".to_string());
        }
        let mut parts = Vec::with_capacity(streams.len());
        for stream in streams {
            let operand = stream
                .get("operand")
                .ok_or_else(|| "stream operand has no expression".to_string())?;
            parts.push(self.lower_expression(operand)?);
        }
        let source_width: usize = parts.iter().map(Expr::width).sum();
        if source_width != target_width {
            return Err(format!(
                "streaming source width {source_width} does not match target width {target_width}"
            ));
        }
        let source = if parts.len() == 1 {
            parts.remove(0)
        } else {
            Expr::Concat(parts)
        };

        let slice_size = usize_field(value, "sliceSize").unwrap_or(1);
        if slice_size == 0 {
            // `{>>{...}}` streams blocks left-to-right without reordering.
            return Ok(source);
        }
        if slice_size > source_width || !source_width.is_multiple_of(slice_size) {
            return Err(format!(
                "streaming slice size {slice_size} does not evenly divide source width {source_width}"
            ));
        }
        let blocks = source_width / slice_size;
        let mut reversed_blocks = Vec::with_capacity(blocks);
        for block in 0..blocks {
            let lsb = block * slice_size;
            reversed_blocks.push(Expr::Index {
                e: Box::new(source.clone()),
                msb: lsb + slice_size - 1,
                lsb,
            });
        }
        Ok(Expr::Concat(reversed_blocks))
    }

    pub(super) fn lower_string_concatenation(
        &self,
        operands: &[serde_json::Value],
    ) -> Result<eda_core::ir::Expr, String> {
        let Some((first, rest)) = operands.split_first() else {
            return Err("string concatenation has no operands".to_string());
        };
        let mut result = self.lower_string_operand(first)?;
        for operand in rest {
            let value = self.lower_string_operand(operand)?;
            result = self.lower_string_concat_pair(result, value)?;
        }
        Ok(result)
    }

    fn lower_string_operand(
        &self,
        value: &serde_json::Value,
    ) -> Result<eda_core::ir::Expr, String> {
        let converted = serde_json::json!({
            "kind": "Conversion",
            "type": "string",
            "operand": value,
        });
        self.lower_expression(&converted)
    }

    pub(super) fn lower_string_compare_operands(
        &self,
        left: &serde_json::Value,
        right: &serde_json::Value,
        ignore_case: bool,
    ) -> Result<eda_core::ir::Expr, String> {
        use eda_core::ir::Expr;

        if string_field(left, "type").as_deref() != Some("string")
            || string_field(right, "type").as_deref() != Some("string")
        {
            return Err("string comparison requires two string receivers".into());
        }
        Ok(Expr::StringCompare {
            left: Box::new(self.lower_string_operand(left)?),
            right: Box::new(self.lower_string_operand(right)?),
            ignore_case,
        })
    }

    fn lower_string_concat_pair(
        &self,
        left: eda_core::ir::Expr,
        right: eda_core::ir::Expr,
    ) -> Result<eda_core::ir::Expr, String> {
        use eda_core::ir::Expr;

        if left.width() != crate::types::STRING_WIDTH || right.width() != crate::types::STRING_WIDTH
        {
            return Err("string concatenation requires 512-bit string operands".into());
        }
        Ok(Expr::StringConcat {
            left: Box::new(left),
            right: Box::new(right),
        })
    }

    pub(super) fn lower_string_length(
        &self,
        value: eda_core::ir::Expr,
    ) -> Result<eda_core::ir::Expr, String> {
        use eda_core::ir::Expr;

        if value.width() != crate::types::STRING_WIDTH {
            return Err("string len requires a 512-bit string receiver".into());
        }
        Ok(Expr::StringLength { e: Box::new(value) })
    }

    pub(super) fn lower_string_get_byte(
        &self,
        selected: &serde_json::Value,
        selector: &serde_json::Value,
    ) -> Result<eda_core::ir::Expr, String> {
        use eda_core::ir::Expr;

        let value = self.lower_expression(selected)?;
        if value.width() != crate::types::STRING_WIDTH {
            return Err("string byte selection requires a 512-bit string".into());
        }
        let index = self.lower_expression(selector)?;
        if index.width() > 32 {
            return Err("string byte index exceeds 32 bits".into());
        }
        Ok(Expr::StringGetByte {
            e: Box::new(value),
            index: Box::new(index),
        })
    }

    pub(super) fn lower_struct_member(
        &self,
        value: &serde_json::Value,
    ) -> Result<(eda_core::ir::Expr, usize, usize), String> {
        let selected = value
            .get("value")
            .ok_or_else(|| "struct member access has no selected value".to_string())?;
        let member = string_field(value, "member")
            .and_then(|member| member.split_whitespace().next_back().map(str::to_string))
            .ok_or_else(|| "struct member access has no member".to_string())?;
        let field = self.struct_field_for_type(selected, &member)?;
        let base = self.lower_expression(selected)?;
        Ok((base, field.offset + field.width - 1, field.offset))
    }

    pub(super) fn lower_struct_assignment_pattern(
        &self,
        value: &serde_json::Value,
    ) -> Result<eda_core::ir::Expr, String> {
        use eda_core::ir::Expr;

        let type_name = string_field(value, "type").unwrap_or_default();
        let alias_name = type_name
            .split_whitespace()
            .next_back()
            .unwrap_or_default()
            .rsplit(['.', '#'])
            .next()
            .unwrap_or_default();
        let definition = self.type_aliases.get(alias_name).ok_or_else(|| {
            format!("assignment pattern requires a packed struct type, found '{type_name}'")
        })?;
        if definition.is_union {
            return Err("packed union assignment patterns are not supported".into());
        }
        let mut fields = definition.fields.values().copied().collect::<Vec<_>>();
        fields.sort_by(|left, right| right.offset.cmp(&left.offset));

        let mut elements_by_field = std::collections::BTreeMap::new();
        match string_field(value, "kind").as_deref() {
            Some("SimpleAssignmentPattern") => {
                let elements = value
                    .get("elements")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| "simple assignment pattern has no elements".to_string())?;
                if elements.len() != fields.len() {
                    return Err(format!(
                        "packed struct assignment pattern has {} elements, expected {}",
                        elements.len(),
                        fields.len()
                    ));
                }
                let mut positional_fields = fields.clone();
                positional_fields.sort_by(|left, right| left.field_index.cmp(&right.field_index));
                for (field, element) in positional_fields.iter().zip(elements) {
                    elements_by_field.insert(field.offset, element);
                }
            }
            Some("StructuredAssignmentPattern") => {
                let setters = value
                    .get("memberSetters")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| {
                        "structured assignment pattern has no member setters".to_string()
                    })?;
                let mut seen = std::collections::BTreeSet::new();
                for setter in setters {
                    let member = string_field(setter, "member")
                        .and_then(|member| {
                            member.split_whitespace().next_back().map(str::to_string)
                        })
                        .ok_or_else(|| "structured assignment pattern has no member".to_string())?;
                    let field = definition.fields.get(&member).ok_or_else(|| {
                        format!("packed struct has no assignment-pattern member '{member}'")
                    })?;
                    let expression = setter
                        .get("expr")
                        .ok_or_else(|| "assignment-pattern member has no expression".to_string())?;
                    if !seen.insert(member) {
                        return Err("duplicate assignment-pattern member".into());
                    }
                    elements_by_field.insert(field.offset, expression);
                }
            }
            _ => return Err("unsupported packed struct assignment pattern".into()),
        }

        let mut parts = Vec::with_capacity(fields.len());
        let mut expected_lsb = definition.width;
        for field in fields {
            if field.offset + field.width != expected_lsb {
                return Err(
                    "packed struct assignment patterns with padding are not supported".into(),
                );
            }
            expected_lsb = field.offset;
            let element = elements_by_field
                .get(&field.offset)
                .ok_or_else(|| "packed struct assignment pattern is missing a field".to_string())?;
            let expression = self.lower_expression(element)?;
            if expression.width() != field.width {
                return Err(format!(
                    "packed struct assignment-pattern field width {} does not match field width {}",
                    expression.width(),
                    field.width
                ));
            }
            parts.push(expression);
        }
        if expected_lsb != 0 {
            return Err("packed struct assignment patterns with padding are not supported".into());
        }
        Ok(Expr::Concat(parts))
    }

    pub(super) fn lower_struct_member_lvalue(
        &self,
        value: &serde_json::Value,
    ) -> Result<(eda_core::ir::LValue, usize, usize), String> {
        let selected = value
            .get("value")
            .ok_or_else(|| "struct member access has no selected value".to_string())?;
        let member = string_field(value, "member")
            .and_then(|member| member.split_whitespace().next_back().map(str::to_string))
            .ok_or_else(|| "struct member access has no member".to_string())?;
        let selected_offset = self.selected_struct_offset(selected)?;
        let field = self.struct_field_for_type(selected, &member)?;
        let base = self.lower_lvalue(selected)?;
        let lsb = selected_offset + field.offset;
        Ok((base, lsb + field.width - 1, lsb))
    }

    fn selected_struct_offset(&self, value: &serde_json::Value) -> Result<usize, String> {
        if string_field(value, "kind").as_deref() == Some("NamedValue") {
            self.direct_packed_symbol(value)?
                .ok_or_else(|| "struct member access requires a named declaration".to_string())?;
            return Ok(0);
        }
        if string_field(value, "kind").as_deref() != Some("MemberAccess") {
            return Err("nested struct member access has an unsupported selected value".into());
        }
        let selected = value
            .get("value")
            .ok_or_else(|| "struct member access has no selected value".to_string())?;
        let selected_offset = self.selected_struct_offset(selected)?;
        let member = string_field(value, "member")
            .and_then(|member| member.split_whitespace().next_back().map(str::to_string))
            .ok_or_else(|| "struct member access has no member".to_string())?;
        let field = self.struct_field_for_type(selected, &member)?;
        Ok(selected_offset + field.offset)
    }

    pub(super) fn struct_field_for_type(
        &self,
        value: &serde_json::Value,
        member: &str,
    ) -> Result<super::AstStructField, String> {
        let type_name = string_field(value, "type").unwrap_or_default();
        let alias_name = type_name
            .split_whitespace()
            .next_back()
            .unwrap_or_default()
            .rsplit(['.', '#'])
            .next()
            .unwrap_or_default();
        if let Some(definition) = self.type_aliases.get(alias_name) {
            return definition
                .fields
                .get(member)
                .copied()
                .ok_or_else(|| format!("struct type has no member '{member}'"));
        }
        if string_field(value, "kind").as_deref() == Some("NamedValue") {
            if let Some(symbol) = self.direct_packed_symbol(value)? {
                return symbol
                    .struct_fields
                    .get(member)
                    .copied()
                    .ok_or_else(|| format!("struct has no member '{member}'"));
            }
        }
        Err(format!(
            "struct member '{member}' requires a known packed struct type"
        ))
    }

    pub(super) fn lower_integral_conversion(
        &self,
        value: &serde_json::Value,
    ) -> Result<eda_core::ir::Expr, String> {
        use eda_core::ir::Expr;

        let operand = value
            .get("operand")
            .ok_or_else(|| "conversion has no operand".to_string())?;
        let target_type = string_field(value, "type").unwrap_or_default();
        let source_type = string_field(operand, "type").unwrap_or_default();
        let (target_shape, target_signed, _) = self.net_type(&target_type)?;
        let (source_shape, source_signed, _) = self.net_type(&source_type)?;
        let target_width = target_shape.width;
        let source_width = source_shape.width;
        if target_width == 0 || source_width == 0 {
            return Err("integral conversion width cannot be zero".into());
        }

        let expression = self.lower_expression(operand)?;
        if target_width == source_width
            && target_signed == source_signed
            && expression.is_signed() == target_signed
        {
            return Ok(expression);
        }

        let resized = if target_width < source_width {
            Expr::Index {
                e: Box::new(expression),
                msb: target_width - 1,
                lsb: 0,
            }
        } else if target_width > source_width {
            let extension = target_width - source_width;
            let padding = if source_signed {
                let sign = Expr::Index {
                    e: Box::new(expression.clone()),
                    msb: source_width - 1,
                    lsb: source_width - 1,
                };
                Expr::Repeat {
                    times: extension,
                    e: Box::new(sign),
                }
            } else {
                Expr::Const(LogicWord::zero(extension))
            };
            Expr::Concat(vec![padding, expression])
        } else {
            expression
        };
        Ok(Expr::Signed {
            e: Box::new(resized),
            signed: target_signed,
        })
    }

    pub(super) fn lower_wildcard_equality(
        &self,
        value: &serde_json::Value,
    ) -> Result<eda_core::ir::Expr, String> {
        use eda_core::ir::{BinOp, Expr};

        let left = value
            .get("left")
            .ok_or_else(|| "wildcard equality has no left operand".to_string())?;
        let right = value
            .get("right")
            .ok_or_else(|| "wildcard equality has no right operand".to_string())?;
        let width = string_field(left, "type")
            .and_then(|type_name| simple_type_width(&type_name))
            .ok_or_else(|| "wildcard equality has unresolved width".to_string())?;
        let pattern = integer_literal_word(right)
            .or_else(|| named_constant_word(right))
            .ok_or_else(|| {
                "wildcard equality requires an integer constant pattern operand".to_string()
            })?;
        if pattern.width() != width {
            return Err(format!(
                "wildcard equality pattern width {} does not match expression width {width}",
                pattern.width()
            ));
        }

        let mask_bits = pattern
            .bits
            .iter()
            .map(|bit| {
                if bit.is_unknown() {
                    LogicValue::Zero
                } else {
                    LogicValue::One
                }
            })
            .collect();
        let expected_bits = pattern
            .bits
            .iter()
            .map(|bit| {
                if bit.is_unknown() {
                    LogicValue::Zero
                } else {
                    *bit
                }
            })
            .collect();
        let value = self.lower_expression(left)?;
        let masked = Expr::Bin {
            op: BinOp::And,
            a: Box::new(value),
            b: Box::new(Expr::Const(LogicWord { bits: mask_bits })),
        };
        Ok(Expr::Bin {
            op: BinOp::Eq,
            a: Box::new(masked),
            b: Box::new(Expr::Const(LogicWord {
                bits: expected_bits,
            })),
        })
    }

    pub(super) fn lower_inside(
        &self,
        value: &serde_json::Value,
    ) -> Result<eda_core::ir::Expr, String> {
        use eda_core::ir::{BinOp, Expr};

        let left = value
            .get("left")
            .ok_or_else(|| "inside expression has no left operand".to_string())?;
        let ranges = value
            .get("rangeList")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "inside expression has no range list".to_string())?;
        let left = self.lower_expression(left)?;
        let mut result = Expr::Const(LogicWord::zero(1));
        for range in ranges {
            let item = if string_field(range, "kind").as_deref() == Some("ValueRange") {
                if string_field(range, "rangeKind").as_deref() != Some("Simple") {
                    return Err(
                        "direct slang-AST lowering supports only simple inside value ranges"
                            .to_string(),
                    );
                }
                let lower = range
                    .get("left")
                    .ok_or_else(|| "inside value range has no lower bound".to_string())?;
                let upper = range
                    .get("right")
                    .ok_or_else(|| "inside value range has no upper bound".to_string())?;
                let lower = self.lower_expression(lower)?;
                let upper = self.lower_expression(upper)?;
                Expr::Bin {
                    op: BinOp::LogAnd,
                    a: Box::new(Expr::Bin {
                        op: BinOp::Ge,
                        a: Box::new(left.clone()),
                        b: Box::new(lower),
                    }),
                    b: Box::new(Expr::Bin {
                        op: BinOp::Le,
                        a: Box::new(left.clone()),
                        b: Box::new(upper),
                    }),
                }
            } else {
                let candidate = self.lower_expression(range)?;
                Expr::Bin {
                    op: BinOp::Eq,
                    a: Box::new(left.clone()),
                    b: Box::new(candidate),
                }
            };
            result = Expr::Bin {
                op: BinOp::LogOr,
                a: Box::new(result),
                b: Box::new(item),
            };
        }
        Ok(result)
    }

    fn select_packed_dimension(
        &self,
        symbol: &AstLoweredSymbol,
        selectors: &[serde_json::Value],
        level: usize,
        base: eda_core::ir::Expr,
    ) -> Result<eda_core::ir::Expr, String> {
        use eda_core::ir::{BinOp, Expr};

        let Some(dimension) = symbol.dimensions.get(level) else {
            return Err("packed select has too many dimensions".to_string());
        };
        let selector = &selectors[level];
        let selector_width = string_field(selector, "type")
            .and_then(|type_name| simple_type_width(&type_name))
            .unwrap_or(32)
            .max(1);
        let lower = dimension.msb.min(dimension.lsb);
        let upper = dimension.msb.max(dimension.lsb);
        let mut constant_index = None;
        if integer_literal_word(selector).is_some() || string_field(selector, "constant").is_some()
        {
            let index = constant_i64(selector)?;
            if index < lower || index > upper {
                return Err(format!("packed select index {index} is out of range"));
            }
            constant_index = Some(index);
        }

        let lowered_selector = self.lower_expression(selector)?;
        let mut selected = Expr::Const(LogicWord::xes(dimension.element_width));
        for logical_index in lower..=upper {
            let candidate = self.packed_dimension_candidate(
                dimension,
                logical_index,
                level,
                symbol,
                selectors,
                base.clone(),
            )?;
            if constant_index == Some(logical_index) {
                return Ok(candidate);
            }
            if constant_index.is_some() {
                continue;
            }
            let condition = Expr::Bin {
                op: BinOp::Eq,
                a: Box::new(lowered_selector.clone()),
                b: Box::new(Expr::Const(LogicWord::from_u64(
                    selector_width,
                    logical_index as u64,
                ))),
            };
            selected = Expr::Cond {
                c: Box::new(condition),
                t: Box::new(candidate),
                f: Box::new(selected),
            };
        }
        if constant_index.is_some() {
            return Err("constant packed select did not match a declared index".to_string());
        }
        Ok(selected)
    }

    #[allow(clippy::too_many_arguments)]
    fn packed_dimension_candidate(
        &self,
        dimension: &AstPackedDimension,
        logical_index: i64,
        level: usize,
        symbol: &AstLoweredSymbol,
        selectors: &[serde_json::Value],
        base: eda_core::ir::Expr,
    ) -> Result<eda_core::ir::Expr, String> {
        let coordinate = if dimension.msb >= dimension.lsb {
            logical_index - dimension.lsb
        } else {
            dimension.lsb - logical_index
        };
        let coordinate = usize::try_from(coordinate)
            .ok()
            .filter(|coordinate| *coordinate < dimension.length)
            .ok_or_else(|| {
                format!(
                    "packed select maps to an invalid physical coordinate: index {logical_index}, dimension [{}:{}]",
                    dimension.msb,
                    dimension.lsb
                )
            })?;
        let width = dimension.element_width;
        let start = coordinate * width;
        let candidate = eda_core::ir::Expr::Index {
            e: Box::new(base),
            msb: start + width - 1,
            lsb: start,
        };
        if level + 1 == selectors.len() {
            Ok(candidate)
        } else {
            self.select_packed_dimension(symbol, selectors, level + 1, candidate)
        }
    }

    pub(super) fn lower_constant_select(
        &self,
        value: &serde_json::Value,
    ) -> Result<(eda_core::ir::Expr, usize), String> {
        let selected = value
            .get("value")
            .ok_or_else(|| "element select has no value".to_string())?;
        let selector = value
            .get("selector")
            .ok_or_else(|| "element select has no selector".to_string())?;
        let symbol = self
            .direct_packed_symbol(selected)?
            .ok_or_else(|| "direct packed selects require a named declaration".to_string())?;
        let index = self.declared_bit_index(&symbol, constant_i64(selector)?)?;
        let base = self.lower_expression(selected)?;
        Ok((base, index))
    }

    pub(super) fn direct_packed_symbol(
        &self,
        value: &serde_json::Value,
    ) -> Result<Option<AstLoweredSymbol>, String> {
        if !matches!(
            string_field(value, "kind").as_deref(),
            Some("NamedValue" | "HierarchicalValue")
        ) {
            return Ok(None);
        }
        if let Some(symbol_id) = expression_symbol_id(value) {
            if let Some(symbol) = self.external_symbols.get(&symbol_id) {
                return Ok(Some(symbol.clone()));
            }
            if let Some(symbol) = self.local_symbols.get(&symbol_id) {
                return Ok(Some(symbol.clone()));
            }
        }
        let name = expression_symbol(value)
            .ok_or_else(|| "packed select has no declaration symbol".to_string())?;
        Ok(self.symbols.get(&name).cloned())
    }

    pub(super) fn event_array_selection(
        &self,
        value: &serde_json::Value,
    ) -> Result<Option<AstEventArraySelection>, String> {
        if string_field(value, "kind").as_deref() != Some("ElementSelect") {
            return Ok(None);
        }
        let mut selectors = Vec::new();
        let mut selected = value;
        while string_field(selected, "kind").as_deref() == Some("ElementSelect") {
            selectors.push(
                selected
                    .get("selector")
                    .ok_or_else(|| "event array select has no selector".to_string())?,
            );
            selected = selected
                .get("value")
                .ok_or_else(|| "event array select has no selected value".to_string())?;
        }
        let Some(symbol) = self.direct_packed_symbol(selected)? else {
            return Ok(None);
        };
        if symbol.memory.is_some() || symbol.dimensions.is_empty() {
            return Ok(None);
        }
        selectors.reverse();
        if selectors.len() != symbol.dimensions.len() {
            return Err(format!(
                "event select rank {} does not match the declaration's {} dimensions",
                selectors.len(),
                symbol.dimensions.len()
            ));
        }
        Ok(Some(AstEventArraySelection {
            base: symbol.base,
            dimensions: symbol.dimensions,
            selectors: selectors.into_iter().cloned().collect(),
        }))
    }

    pub(super) fn event_array_candidates(
        &self,
        selection: &AstEventArraySelection,
        selectors: &[serde_json::Value],
    ) -> Result<Vec<AstEventArrayCandidate>, String> {
        if selection.dimensions.is_empty() {
            return Err("event array declaration has no dimensions".into());
        }
        let lowered_selectors = selectors
            .iter()
            .map(|selector| self.lower_expression(selector))
            .collect::<Result<Vec<_>, _>>()?;
        let mut candidates = Vec::new();
        self.append_event_array_candidates(
            selection,
            &lowered_selectors,
            selectors,
            0,
            0,
            Vec::new(),
            &mut candidates,
        )?;
        Ok(candidates)
    }

    #[allow(clippy::too_many_arguments)]
    fn append_event_array_candidates(
        &self,
        selection: &AstEventArraySelection,
        lowered_selectors: &[eda_core::ir::Expr],
        selectors: &[serde_json::Value],
        level: usize,
        offset: usize,
        conditions: Vec<eda_core::ir::Expr>,
        candidates: &mut Vec<AstEventArrayCandidate>,
    ) -> Result<(), String> {
        let Some(dimension) = selection.dimensions.get(level) else {
            candidates.push(AstEventArrayCandidate {
                net: selection.base + offset,
                conditions,
            });
            return Ok(());
        };
        let selector = &selectors[level];
        let lower = dimension.msb.min(dimension.lsb);
        let upper = dimension.msb.max(dimension.lsb);
        if integer_literal_word(selector).is_some() || string_field(selector, "constant").is_some()
        {
            let index = constant_i64(selector)?;
            if index < lower || index > upper {
                return Err(format!("event array index {index} is out of range"));
            }
        }
        let lowered_selector = &lowered_selectors[level];
        let comparison_width = lowered_selector.width().max(1);
        for logical_index in lower..=upper {
            let coordinate = if dimension.msb >= dimension.lsb {
                logical_index - dimension.lsb
            } else {
                dimension.lsb - logical_index
            };
            let coordinate = usize::try_from(coordinate)
                .ok()
                .filter(|coordinate| *coordinate < dimension.length)
                .ok_or_else(|| {
                    format!(
                        "event array index {logical_index} maps to an invalid physical coordinate"
                    )
                })?;
            let condition = eda_core::ir::Expr::Bin {
                op: eda_core::ir::BinOp::Eq,
                a: Box::new(lowered_selector.clone()),
                b: Box::new(eda_core::ir::Expr::Const(LogicWord::from_u64(
                    comparison_width,
                    logical_index as u64,
                ))),
            };
            let mut conditions = conditions.clone();
            conditions.push(condition);
            self.append_event_array_candidates(
                selection,
                lowered_selectors,
                selectors,
                level + 1,
                offset + coordinate * dimension.element_width,
                conditions,
                candidates,
            )?;
        }
        Ok(())
    }

    pub(super) fn declared_bit_index(
        &self,
        symbol: &AstLoweredSymbol,
        index: i64,
    ) -> Result<usize, String> {
        let Some((msb, lsb)) = symbol.bounds else {
            return Err(
                "multi-packed selects require dimension-aware lowering and are not supported yet"
                    .to_string(),
            );
        };
        let lower = msb.min(lsb);
        let upper = msb.max(lsb);
        if index < lower || index > upper {
            return Err(format!("packed select index {index} is out of range"));
        }
        let canonical = if msb >= lsb { index - lsb } else { lsb - index };
        usize::try_from(canonical)
            .ok()
            .filter(|index| *index < symbol.width)
            .ok_or_else(|| "packed select maps to an invalid physical bit".to_string())
    }

    pub(super) fn memory_element_select(
        &self,
        value: &serde_json::Value,
    ) -> Result<Option<(usize, Vec<eda_core::ir::Expr>)>, String> {
        if string_field(value, "kind").as_deref() != Some("ElementSelect") {
            return Ok(None);
        }
        let selected = value
            .get("value")
            .ok_or_else(|| "element select has no value".to_string())?;
        let selector = value
            .get("selector")
            .ok_or_else(|| "memory element select has no selector".to_string())?;
        let selector = self.lower_expression(selector)?;
        match string_field(selected, "kind").as_deref() {
            Some("NamedValue") => {
                let name = expression_symbol(selected)
                    .ok_or_else(|| "memory element select has no symbol".to_string())?;
                let Some(symbol) = self.symbols.get(&name) else {
                    return Ok(None);
                };
                let Some(memory) = symbol.memory else {
                    return Ok(None);
                };
                Ok(Some((memory, vec![selector])))
            }
            Some("ElementSelect") => {
                let Some((memory, mut indexes)) = self.memory_element_select(selected)? else {
                    return Ok(None);
                };
                indexes.push(selector);
                Ok(Some((memory, indexes)))
            }
            _ => Ok(None),
        }
    }

    #[allow(clippy::type_complexity)]
    pub(super) fn memory_range_select(
        &self,
        value: &serde_json::Value,
    ) -> Result<
        Option<(
            usize,
            Vec<eda_core::ir::Expr>,
            eda_core::ir::Expr,
            usize,
            bool,
        )>,
        String,
    > {
        if string_field(value, "kind").as_deref() != Some("RangeSelect") {
            return Ok(None);
        }
        let Some(selected) = value.get("value") else {
            return Ok(None);
        };
        let Some((memory, indexes)) = self.memory_element_select(selected)? else {
            return Ok(None);
        };
        let element_width = self.design.memories[memory].element_width;
        let selection_kind = string_field(value, "selectionKind").unwrap_or_default();
        let (start, width, ascending) = match selection_kind.as_str() {
            "Simple" => {
                let left = value
                    .get("left")
                    .ok_or_else(|| "memory range select has no left bound".to_string())?;
                let right = value
                    .get("right")
                    .ok_or_else(|| "memory range select has no right bound".to_string())?;
                let msb = constant_index(left)?;
                let lsb = constant_index(right)?;
                if msb < lsb {
                    return Err(
                        "direct slang-AST lowering does not yet support reversed simple ranges"
                            .into(),
                    );
                }
                (
                    eda_core::ir::Expr::Const(eda_core::logic::LogicWord::from_u64(32, lsb as u64)),
                    msb - lsb + 1,
                    true,
                )
            }
            "IndexedUp" | "IndexedDown" => {
                let left = value
                    .get("left")
                    .ok_or_else(|| "indexed memory select has no start".to_string())?;
                let right = value
                    .get("right")
                    .ok_or_else(|| "indexed memory select has no width".to_string())?;
                let width = constant_index(right)?;
                (
                    self.lower_expression(left)?,
                    width,
                    selection_kind == "IndexedUp",
                )
            }
            other => {
                return Err(format!(
                    "direct slang-AST lowering does not support memory selection kind '{other}'"
                ))
            }
        };
        if width == 0 || width > element_width {
            return Err(format!(
                "memory part-select width {width} exceeds element width {element_width}"
            ));
        }
        Ok(Some((memory, indexes, start, width, ascending)))
    }

    pub(super) fn lower_constant_select_lvalue(
        &self,
        value: &serde_json::Value,
    ) -> Result<(eda_core::ir::LValue, usize), String> {
        let selected = value
            .get("value")
            .ok_or_else(|| "element select has no value".to_string())?;
        let selector = value
            .get("selector")
            .ok_or_else(|| "element select has no selector".to_string())?;
        let symbol = self
            .direct_packed_symbol(selected)?
            .ok_or_else(|| "direct packed selects require a named declaration".to_string())?;
        let index = self.declared_bit_index(&symbol, constant_i64(selector)?)?;
        Ok((self.lower_lvalue(selected)?, index))
    }

    pub(super) fn lower_constant_range(
        &self,
        value: &serde_json::Value,
    ) -> Result<(eda_core::ir::Expr, usize, usize), String> {
        let selected = value
            .get("value")
            .ok_or_else(|| "range select has no value".to_string())?;
        if self.direct_packed_symbol(selected)?.is_none()
            && string_field(selected, "kind").as_deref() == Some("MemberAccess")
        {
            let left = value
                .get("left")
                .ok_or_else(|| "range select has no left bound".to_string())?;
            let right = value
                .get("right")
                .ok_or_else(|| "range select has no right bound".to_string())?;
            let (left, right) = (constant_i64(left)?, constant_i64(right)?);
            if left < right {
                return Err("range selects on struct members must use descending bounds".into());
            }
            let base = self.lower_expression(selected)?;
            if left >= base.width() as i64 || right < 0 {
                return Err("range select is out of range for struct member".into());
            }
            return Ok((base, left as usize, right as usize));
        }
        let (msb, lsb) = self.declared_range_indices(value)?;
        let base = self.lower_expression(selected)?;
        Ok((base, msb, lsb))
    }

    pub(super) fn declared_range_indices(
        &self,
        value: &serde_json::Value,
    ) -> Result<(usize, usize), String> {
        let selected = value
            .get("value")
            .ok_or_else(|| "range select has no value".to_string())?;
        let left_value = value
            .get("left")
            .ok_or_else(|| "range select has no left bound".to_string())?;
        let right_value = value
            .get("right")
            .ok_or_else(|| "range select has no right bound".to_string())?;
        let symbol = self
            .direct_packed_symbol(selected)?
            .ok_or_else(|| "direct packed selects require a named declaration".to_string())?;
        let Some((declaration_msb, declaration_lsb)) = symbol.bounds else {
            return Err(
                "multi-packed range selects require dimension-aware lowering and are not supported yet"
                    .to_string(),
            );
        };
        let left = constant_i64(left_value)?;
        let right = constant_i64(right_value)?;
        let lower = declaration_msb.min(declaration_lsb);
        let upper = declaration_msb.max(declaration_lsb);
        let direction_valid = if declaration_msb >= declaration_lsb {
            left >= right
        } else {
            left <= right
        };
        if !direction_valid || left < lower || left > upper || right < lower || right > upper {
            return Err(format!(
                "packed range select [{left}:{right}] is invalid for declaration [{declaration_msb}:{declaration_lsb}]"
            ));
        }
        let mapped_left = self.declared_bit_index(&symbol, left)?;
        let mapped_right = self.declared_bit_index(&symbol, right)?;
        Ok((mapped_left.max(mapped_right), mapped_left.min(mapped_right)))
    }

    pub(super) fn lower_constant_range_lvalue(
        &self,
        value: &serde_json::Value,
    ) -> Result<(eda_core::ir::LValue, usize, usize), String> {
        let selected = value
            .get("value")
            .ok_or_else(|| "range select has no value".to_string())?;
        let (msb, lsb) = self.declared_range_indices(value)?;
        Ok((self.lower_lvalue(selected)?, msb, lsb))
    }
}
