//! Expression operator and constant helpers for direct slang lowering.

use crate::ast_util::{expression_symbol, integer_literal_word, named_constant_word, string_field};
use crate::types::{simple_type_signed, simple_type_width};
use serde_json::Value;

use super::hierarchy::full_range;
use super::AstLoweredSymbol;

pub(super) fn binary_operator(operator: Option<&str>) -> Result<eda_core::ir::BinOp, String> {
    match operator {
        Some("Add") => Ok(eda_core::ir::BinOp::Add),
        Some("Subtract") => Ok(eda_core::ir::BinOp::Sub),
        Some("Multiply") => Ok(eda_core::ir::BinOp::Mul),
        Some("Divide") => Ok(eda_core::ir::BinOp::Div),
        Some("Modulus") => Ok(eda_core::ir::BinOp::Mod),
        Some("BinaryAnd") => Ok(eda_core::ir::BinOp::And),
        Some("BinaryOr") => Ok(eda_core::ir::BinOp::Or),
        Some("BitwiseXor") => Ok(eda_core::ir::BinOp::Xor),
        Some("LogicalShiftLeft") => Ok(eda_core::ir::BinOp::Shl),
        Some("LogicalShiftRight") => Ok(eda_core::ir::BinOp::Shr),
        Some("LogicalAnd") => Ok(eda_core::ir::BinOp::LogAnd),
        Some("LogicalOr") => Ok(eda_core::ir::BinOp::LogOr),
        Some("Equality") => Ok(eda_core::ir::BinOp::Eq),
        Some("Inequality") => Ok(eda_core::ir::BinOp::Ne),
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
        .and_then(|word| word.to_u64())
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| "direct lowering requires an integer constant index".to_string())
}

pub(super) fn constant_i64(value: &serde_json::Value) -> Result<i64, String> {
    let word = integer_literal_word(value)
        .or_else(|| named_constant_word(value))
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
    pub(super) fn lower_lvalue(
        &self,
        value: &serde_json::Value,
    ) -> Result<eda_core::ir::LValue, String> {
        match string_field(value, "kind").as_deref() {
            Some("NamedValue") => {
                let name = expression_symbol(value)
                    .ok_or_else(|| "assignment target has no symbol".to_string())?;
                let symbol = self
                    .symbols
                    .get(&name)
                    .ok_or_else(|| format!("unknown direct-lowering symbol '{name}'"))?;
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
        use eda_core::ir::{Expr, UnOp};
        let kind = string_field(value, "kind").unwrap_or_default();
        match kind.as_str() {
            "NamedValue" => {
                let name = expression_symbol(value)
                    .ok_or_else(|| "expression has no symbol".to_string())?;
                if let Some(word) = self.constants.get(&name).cloned() {
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
                    .ok_or_else(|| "invalid slang integer literal".to_string())?;
                Ok(Expr::Const(word))
            }
            "Conversion" => {
                let operand = value
                    .get("operand")
                    .ok_or_else(|| "conversion has no operand".to_string())?;
                let target_type = string_field(value, "type").unwrap_or_default();
                let operand_type = string_field(operand, "type").unwrap_or_default();
                let target = (
                    simple_type_width(&target_type),
                    simple_type_signed(&target_type),
                );
                let source = (
                    simple_type_width(&operand_type),
                    simple_type_signed(&operand_type),
                );
                if target != source {
                    return Err(format!(
                        "direct slang-AST lowering does not support conversion '{target_type}' from '{operand_type}'"
                    ));
                }
                self.lower_expression(operand)
            }
            "UnaryOp" => {
                let op = match string_field(value, "op").as_deref() {
                    Some("BitwiseNot") => UnOp::Not,
                    Some("LogicalNot") => UnOp::Not,
                    Some("BitwiseAnd") => UnOp::AndR,
                    Some("BitwiseOr") => UnOp::OrR,
                    Some("BitwiseXor") => UnOp::XorR,
                    Some(op) => {
                        return Err(format!(
                            "direct slang-AST lowering does not support unary operator '{op}'"
                        ))
                    }
                    None => return Err("unary expression has no operator".into()),
                };
                let operand = value
                    .get("operand")
                    .ok_or_else(|| "unary expression has no operand".to_string())?;
                if string_field(value, "op").as_deref() == Some("LogicalNot") {
                    return Ok(Expr::Un {
                        op: UnOp::Not,
                        a: Box::new(Expr::Un {
                            op: UnOp::OrR,
                            a: Box::new(self.lower_expression(operand)?),
                        }),
                    });
                }
                Ok(Expr::Un {
                    op,
                    a: Box::new(self.lower_expression(operand)?),
                })
            }
            "BinaryOp" => {
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
                let mut parts = Vec::new();
                for operand in operands {
                    parts.push(self.lower_expression(operand)?);
                }
                Ok(Expr::Concat(parts))
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
                let (base, msb, lsb) = self.lower_constant_range(value)?;
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
        let index = self.declared_bit_index(symbol, constant_i64(selector)?)?;
        let base = self.lower_expression(selected)?;
        Ok((base, index))
    }

    pub(super) fn direct_packed_symbol(
        &self,
        value: &serde_json::Value,
    ) -> Result<Option<AstLoweredSymbol>, String> {
        if string_field(value, "kind").as_deref() != Some("NamedValue") {
            return Ok(None);
        }
        let name = expression_symbol(value)
            .ok_or_else(|| "packed select has no declaration symbol".to_string())?;
        Ok(self.symbols.get(&name).copied())
    }

    pub(super) fn declared_bit_index(
        &self,
        symbol: AstLoweredSymbol,
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
        let index = self.declared_bit_index(symbol, constant_i64(selector)?)?;
        Ok((self.lower_lvalue(selected)?, index))
    }

    pub(super) fn lower_constant_range(
        &self,
        value: &serde_json::Value,
    ) -> Result<(eda_core::ir::Expr, usize, usize), String> {
        let selected = value
            .get("value")
            .ok_or_else(|| "range select has no value".to_string())?;
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
        let mapped_left = self.declared_bit_index(symbol, left)?;
        let mapped_right = self.declared_bit_index(symbol, right)?;
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
