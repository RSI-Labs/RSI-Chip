//! Expression operator and constant helpers for direct slang lowering.

use crate::ast_util::{expression_symbol, integer_literal_word, named_constant_word, string_field};
use crate::types::AstPackedDimension;
use crate::types::{simple_type_signed, simple_type_width};
use eda_core::logic::{LogicValue, LogicWord};
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
        Some("ArithmeticShiftLeft") => Ok(eda_core::ir::BinOp::Shl),
        Some("ArithmeticShiftRight") => Ok(eda_core::ir::BinOp::Shr),
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
            "Inside" => self.lower_inside(value),
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
                if let Some(selected) = self.lower_packed_element_select(value)? {
                    return Ok(selected);
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

        let start = self.lower_expression(start_value)?;
        let start_width = start.width().max(1);
        let mut selected_expression = Expr::Const(LogicWord::xes(width));
        let starts = if selection_kind == "IndexedUp" {
            0..=symbol.width - width
        } else {
            width - 1..=symbol.width - 1
        };
        for physical_start in starts {
            let (msb, lsb) = if selection_kind == "IndexedUp" {
                (physical_start + width - 1, physical_start)
            } else {
                (physical_start, physical_start + 1 - width)
            };
            let candidate = Expr::Index {
                e: Box::new(self.lower_expression(selected)?),
                msb,
                lsb,
            };
            if constant_i64(start_value).is_ok() {
                let constant_start = constant_i64(start_value)?;
                if constant_start == physical_start as i64 {
                    return Ok(Some(candidate));
                }
                continue;
            }
            let condition = Expr::Bin {
                op: BinOp::Eq,
                a: Box::new(start.clone()),
                b: Box::new(Expr::Const(LogicWord::from_u64(
                    start_width,
                    physical_start as u64,
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
        if string_field(value, "kind").as_deref() != Some("NamedValue") {
            return Ok(None);
        }
        let name = expression_symbol(value)
            .ok_or_else(|| "packed select has no declaration symbol".to_string())?;
        Ok(self.symbols.get(&name).cloned())
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
