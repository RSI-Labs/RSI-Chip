//! Net declaration, unpacked-array, dynamic-array, indexed-select, and member helpers.

use super::*;

impl<'a> Elaborator<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn declare_net(
        &mut self,
        name: &str,
        prefix: &str,
        range: &Option<(Box<ast::Expr>, Box<ast::Expr>)>,
        params: &HashMap<String, i64>,
        is_reg: bool,
        is_signed: bool,
        type_width: Option<usize>,
        type_name: Option<&str>,
    ) -> Result<NetEntry, String> {
        let (msb, lsb) = match (range, type_width) {
            (Some((a, b)), _) => (
                self.const_eval(a, params, "declaration")?,
                self.const_eval(b, params, "declaration")?,
            ),
            (None, Some(width)) => (width as i64 - 1, 0),
            (None, None) => (0, 0),
        };
        let width = (msb - lsb).unsigned_abs() as usize + 1;
        let base = self.design.n_nets;
        self.design.net_decls.push(NetDecl {
            name: format!("{}{}", prefix, name),
            width,
            is_reg,
            is_signed,
            base,
        });
        self.design.n_nets += width;
        Ok(NetEntry {
            base,
            width,
            msb,
            lsb,
            type_name: type_name.map(str::to_string),
            is_signed,
            array: None,
            memory: None,
        })
    }

    pub(super) fn declaration_type_width(
        &self,
        type_name: Option<&str>,
        local_types: &HashMap<String, ResolvedType>,
        module_name: &str,
    ) -> Result<Option<usize>, String> {
        let Some(name) = type_name else {
            return Ok(None);
        };
        if self.classes.contains_key(name) {
            return Ok(Some(1));
        }
        if name == "string" {
            return Ok(Some(ast::STRING_WIDTH));
        }
        local_types
            .get(name)
            .map(|definition| Some(definition.width()))
            .ok_or_else(|| {
                format!(
                    "module '{}': unknown type definition '{}'",
                    module_name, name
                )
            })
    }

    pub(super) fn indexed_part_range(
        &self,
        name: &str,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<(usize, usize), String> {
        let entry = env.net(name)?;
        if entry.array.is_some() {
            return Err(format!(
                "indexed part-select is not supported on unpacked array '{}'",
                name
            ));
        }
        if entry.msb < entry.lsb {
            return Err(format!(
                "indexed part-select on reversed packed range [{}:{}] is not yet supported",
                entry.msb, entry.lsb
            ));
        }
        let start = self.const_eval(start, env.params, env.module_name)?;
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        let (high, low) = if ascending {
            (start + width as i64 - 1, start)
        } else {
            (start, start - width as i64 + 1)
        };
        let high_offset = entry.offset(high)?;
        let low_offset = entry.offset(low)?;
        Ok((high_offset.max(low_offset), high_offset.min(low_offset)))
    }

    pub(super) fn reversed_indexed_part_lvalue(
        &self,
        entry: &NetEntry,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<LValue, String> {
        let start = self.const_eval(start, env.params, env.module_name)?;
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > entry.width {
            return Err(format!(
                "indexed part-select width {width} exceeds declaration width {}",
                entry.width
            ));
        }
        let logical_start = if ascending {
            start
        } else {
            start - width as i64 + 1
        };
        let logical_end = logical_start + width as i64 - 1;
        let high_offset = entry.offset(logical_end)?;
        let low_offset = entry.offset(logical_start)?;
        Ok(LValue {
            net: entry.base,
            decl_width: entry.width,
            range: Range::Span {
                msb: high_offset.max(low_offset),
                lsb: high_offset.min(low_offset),
            },
        })
    }

    pub(super) fn array_element_location(
        &self,
        name: &str,
        indexes: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<(NetEntry, usize, usize), String> {
        let entry = env.net(name)?;
        let array = entry
            .array
            .as_ref()
            .ok_or_else(|| format!("'{name}' is not an unpacked array"))?;
        let mut resolved = Vec::with_capacity(indexes.len());
        for index in indexes {
            resolved.push(self.const_eval(index, env.params, env.module_name)?);
        }
        let offset = entry.array_offset(&resolved)?;
        Ok((entry.clone(), offset, array.element_width))
    }

    pub(super) fn normalize_array_part(
        &self,
        name: &str,
        indexes: &[ast::Expr],
        msb: &ast::Expr,
        lsb: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<ast::Expr, String> {
        let msb = self.const_eval(msb, env.params, env.module_name)?;
        let lsb = self.const_eval(lsb, env.params, env.module_name)?;
        if msb >= lsb {
            Ok(ast::Expr::ArrayIndexedPart {
                name: name.to_string(),
                indexes: indexes.to_vec(),
                start: Box::new(constant_expr(lsb)),
                width: Box::new(constant_expr(msb - lsb + 1)),
                ascending: true,
            })
        } else {
            Ok(ast::Expr::ArrayIndexedPart {
                name: name.to_string(),
                indexes: indexes.to_vec(),
                start: Box::new(constant_expr(msb)),
                width: Box::new(constant_expr(lsb - msb + 1)),
                ascending: false,
            })
        }
    }

    pub(super) fn array_element_indexed_expr(
        &self,
        name: &str,
        indexes: &[ast::Expr],
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        if env.net(name)?.memory.is_some() {
            let entry_ref = env.net(name)?;
            return self.memory_part_read_expr(entry_ref, indexes, start, width, ascending, env);
        }
        if indexes
            .iter()
            .any(|index| self.const_eval(index, env.params, env.module_name).is_err())
        {
            return self
                .dynamic_array_element_indexed_expr(name, indexes, start, width, ascending, env);
        }
        let (entry, offset, element_width) = self.array_element_location(name, indexes, env)?;
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > element_width {
            return Err(format!(
                "indexed part-select width {width} exceeds array element width {element_width}"
            ));
        }

        let (element_msb, element_lsb) = entry
            .array
            .as_ref()
            .map(|array| (array.element_msb, array.element_lsb))
            .unwrap_or((entry.msb, entry.lsb));
        let reversed = element_msb < element_lsb;
        let logical_base = if reversed { element_msb } else { element_lsb };
        let start = self.elab_expr(start, env)?;
        let logical_start = if ascending {
            start
        } else {
            ExprIR::Bin {
                op: BinOp::Sub,
                a: Box::new(start),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, width as u64 - 1))),
            }
        };
        let normalized_start = ExprIR::Bin {
            op: BinOp::Sub,
            a: Box::new(logical_start),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(32, logical_base as u64))),
        };
        let normalized_source = if reversed {
            ExprIR::Concat(
                (offset..offset + element_width)
                    .map(|physical_bit| ExprIR::Ref {
                        net: entry.base,
                        decl_width: entry.width,
                        range: Range::Bit(physical_bit),
                        signed: false,
                    })
                    .collect(),
            )
        } else {
            ExprIR::Ref {
                net: entry.base,
                decl_width: entry.width,
                range: Range::Span {
                    msb: offset + element_width - 1,
                    lsb: offset,
                },
                signed: false,
            }
        };
        let max_offset = element_width - width;
        let in_range = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Ge,
                a: Box::new(normalized_start.clone()),
                b: Box::new(ExprIR::Const(LogicWord::zero(32))),
            }),
            b: Box::new(ExprIR::Bin {
                op: BinOp::Le,
                a: Box::new(normalized_start.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, max_offset as u64))),
            }),
        };
        let selected = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Shr,
                a: Box::new(normalized_source),
                b: Box::new(normalized_start),
            }),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(
                width,
                (1_u64 << width) - 1,
            ))),
        };
        Ok(ExprIR::Cond {
            c: Box::new(in_range),
            t: Box::new(selected),
            f: Box::new(ExprIR::Const(LogicWord::xes(width))),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn dynamic_array_element_indexed_expr(
        &self,
        name: &str,
        indexes: &[ast::Expr],
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        let entry = env.net(name)?;
        let array = entry
            .array
            .as_ref()
            .ok_or_else(|| format!("'{name}' is not an unpacked array"))?;
        if indexes.len() != array.dimensions.len() {
            return Err(format!(
                "array expects {} index(es), received {}",
                array.dimensions.len(),
                indexes.len()
            ));
        }
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        let element_width = array.element_width;
        if width > element_width {
            return Err(format!(
                "indexed part-select width {width} exceeds array element width {element_width}"
            ));
        }

        let runtime_indexes = indexes
            .iter()
            .map(|index| self.elab_expr(index, env))
            .collect::<Result<Vec<_>, _>>()?;
        if width == element_width
            && ascending
            && array.element_msb >= array.element_lsb
            && self.const_eval(start, env.params, env.module_name).ok() == Some(0)
        {
            if let Some(selected) = self.runtime_whole_array_element_expr(
                entry.base,
                entry.width,
                array,
                &runtime_indexes,
            ) {
                return Ok(selected);
            }
        }
        let mut selected_element = ExprIR::Const(LogicWord::xes(element_width));
        let mut any_index_valid = ExprIR::Const(LogicWord::from_u64(1, 0));
        for physical_element in 0..array.length {
            let mut physical_offset = physical_element;
            let mut scale = array.length;
            let mut logical_indexes = Vec::with_capacity(array.dimensions.len());
            for dimension in &array.dimensions {
                scale /= dimension.length;
                let dimension_offset = physical_offset / scale;
                physical_offset %= scale;
                logical_indexes.push(dimension.base + dimension.step * dimension_offset as i64);
            }

            let mut candidate_valid = ExprIR::Const(LogicWord::from_u64(1, 1));
            for (runtime_index, logical_index) in runtime_indexes.iter().zip(&logical_indexes) {
                let equals = ExprIR::Bin {
                    op: BinOp::Eq,
                    a: Box::new(runtime_index.clone()),
                    b: Box::new(ExprIR::Const(LogicWord::from_u64(
                        32,
                        *logical_index as u64,
                    ))),
                };
                candidate_valid = ExprIR::Bin {
                    op: BinOp::And,
                    a: Box::new(candidate_valid),
                    b: Box::new(equals),
                };
            }
            any_index_valid = ExprIR::Bin {
                op: BinOp::Or,
                a: Box::new(any_index_valid),
                b: Box::new(candidate_valid.clone()),
            };

            let physical_lsb = physical_element * element_width;
            let candidate = if array.element_msb < array.element_lsb {
                ExprIR::Concat(
                    (physical_lsb..physical_lsb + element_width)
                        .map(|physical_bit| ExprIR::Ref {
                            net: entry.base,
                            decl_width: entry.width,
                            range: Range::Bit(physical_bit),
                            signed: false,
                        })
                        .collect(),
                )
            } else {
                ExprIR::Ref {
                    net: entry.base,
                    decl_width: entry.width,
                    range: Range::Span {
                        msb: physical_lsb + element_width - 1,
                        lsb: physical_lsb,
                    },
                    signed: false,
                }
            };
            selected_element = ExprIR::Cond {
                c: Box::new(candidate_valid),
                t: Box::new(candidate),
                f: Box::new(selected_element),
            };
        }

        let (element_msb, element_lsb) = (array.element_msb, array.element_lsb);
        let reversed = element_msb < element_lsb;
        let logical_base = if reversed { element_msb } else { element_lsb };
        let start = self.elab_expr(start, env)?;
        let logical_start = if ascending {
            start
        } else {
            ExprIR::Bin {
                op: BinOp::Sub,
                a: Box::new(start),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, width as u64 - 1))),
            }
        };
        let normalized_start = ExprIR::Bin {
            op: BinOp::Sub,
            a: Box::new(logical_start),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(32, logical_base as u64))),
        };
        let max_offset = element_width - width;
        let packed_in_range = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Ge,
                a: Box::new(normalized_start.clone()),
                b: Box::new(ExprIR::Const(LogicWord::zero(32))),
            }),
            b: Box::new(ExprIR::Bin {
                op: BinOp::Le,
                a: Box::new(normalized_start.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, max_offset as u64))),
            }),
        };
        let packed_selected = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Shr,
                a: Box::new(selected_element),
                b: Box::new(normalized_start),
            }),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(
                width,
                (1_u64 << width) - 1,
            ))),
        };
        let packed_result = ExprIR::Cond {
            c: Box::new(packed_in_range),
            t: Box::new(packed_selected),
            f: Box::new(ExprIR::Const(LogicWord::xes(width))),
        };
        Ok(ExprIR::Cond {
            c: Box::new(any_index_valid),
            t: Box::new(packed_result),
            f: Box::new(ExprIR::Const(LogicWord::xes(width))),
        })
    }

    fn runtime_array_physical_index(
        &self,
        array: &NetArray,
        runtime_indexes: &[ExprIR],
    ) -> Option<(ExprIR, ExprIR)> {
        if array
            .dimensions
            .iter()
            .any(|dimension| dimension.base < 0 || dimension.base > i64::from(u32::MAX))
        {
            return None;
        }

        let mut physical_index = ExprIR::Const(LogicWord::zero(32));
        let mut all_in_range = ExprIR::Const(LogicWord::from_u64(1, 1));
        let mut stride = array.length;
        for (dimension, runtime_index) in array.dimensions.iter().zip(runtime_indexes) {
            stride /= dimension.length;
            let base = dimension.base;
            let last = base + dimension.step * (dimension.length as i64 - 1);
            let lower = base.min(last);
            let upper = base.max(last);
            let lower_bound = ExprIR::Bin {
                op: BinOp::Ge,
                a: Box::new(runtime_index.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, lower.unsigned_abs()))),
            };
            let upper_bound = ExprIR::Bin {
                op: BinOp::Le,
                a: Box::new(runtime_index.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, upper.unsigned_abs()))),
            };
            all_in_range = ExprIR::Bin {
                op: BinOp::LogAnd,
                a: Box::new(all_in_range),
                b: Box::new(ExprIR::Bin {
                    op: BinOp::LogAnd,
                    a: Box::new(lower_bound),
                    b: Box::new(upper_bound),
                }),
            };
            let normalized = if base <= last {
                ExprIR::Bin {
                    op: BinOp::Sub,
                    a: Box::new(runtime_index.clone()),
                    b: Box::new(ExprIR::Const(LogicWord::from_u64(32, base.unsigned_abs()))),
                }
            } else {
                ExprIR::Bin {
                    op: BinOp::Sub,
                    a: Box::new(ExprIR::Const(LogicWord::from_u64(32, base.unsigned_abs()))),
                    b: Box::new(runtime_index.clone()),
                }
            };
            physical_index = ExprIR::Bin {
                op: BinOp::Add,
                a: Box::new(physical_index),
                b: Box::new(ExprIR::Bin {
                    op: BinOp::Mul,
                    a: Box::new(normalized),
                    b: Box::new(ExprIR::Const(LogicWord::from_u64(32, stride as u64))),
                }),
            };
        }
        Some((physical_index, all_in_range))
    }

    fn runtime_whole_array_element_expr(
        &self,
        net: usize,
        total_width: usize,
        array: &NetArray,
        runtime_indexes: &[ExprIR],
    ) -> Option<ExprIR> {
        let (physical_index, in_range) =
            self.runtime_array_physical_index(array, runtime_indexes)?;
        let bit_offset = ExprIR::Bin {
            op: BinOp::Mul,
            a: Box::new(physical_index),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(
                32,
                array.element_width as u64,
            ))),
        };
        let shifted = ExprIR::Bin {
            op: BinOp::Shr,
            a: Box::new(ExprIR::Ref {
                net,
                decl_width: total_width,
                range: Range::Span {
                    msb: total_width - 1,
                    lsb: 0,
                },
                signed: false,
            }),
            b: Box::new(bit_offset),
        };
        let selected = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(shifted),
            b: Box::new(ExprIR::Const(low_mask(total_width, array.element_width))),
        };
        Some(ExprIR::Cond {
            c: Box::new(in_range),
            t: Box::new(selected),
            f: Box::new(ExprIR::Const(LogicWord::xes(array.element_width))),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn array_element_indexed_span(
        &self,
        entry: &NetEntry,
        offset: usize,
        element_width: usize,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<(usize, usize, bool), String> {
        let start = self.const_eval(start, env.params, env.module_name)?;
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > element_width {
            return Err(format!(
                "indexed part-select width {width} exceeds array element width {element_width}"
            ));
        }
        let logical_start = if ascending {
            start
        } else {
            start - width as i64 + 1
        };
        let (element_msb, element_lsb) = entry
            .array
            .as_ref()
            .map(|array| (array.element_msb, array.element_lsb))
            .unwrap_or((entry.msb, entry.lsb));
        let reversed = element_msb < element_lsb;
        let logical_base = if reversed { element_msb } else { element_lsb };
        let normalized_start = logical_start - logical_base;
        if normalized_start < 0 || normalized_start + width as i64 > element_width as i64 {
            return Err(format!(
                "indexed part-select [{logical_start}+:{}] is out of array-element range",
                width
            ));
        }
        let normalized_start = normalized_start as usize;
        let physical_lsb = if reversed {
            offset + element_width - 1 - (normalized_start + width - 1)
        } else {
            offset + normalized_start
        };
        Ok((physical_lsb, width, reversed))
    }

    pub(super) fn array_element_indexed_start(
        &self,
        entry: &NetEntry,
        _element_width: usize,
        start: &ast::Expr,
        width: usize,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        let start = self.elab_expr(start, env)?;
        let logical_start = if ascending {
            start
        } else {
            ExprIR::Bin {
                op: BinOp::Sub,
                a: Box::new(start),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, width as u64 - 1))),
            }
        };
        let (element_msb, element_lsb) = entry
            .array
            .as_ref()
            .map(|array| (array.element_msb, array.element_lsb))
            .unwrap_or((entry.msb, entry.lsb));
        let reversed = element_msb < element_lsb;
        let logical_base = if reversed { element_msb } else { element_lsb };
        Ok(ExprIR::Bin {
            op: BinOp::Sub,
            a: Box::new(logical_start),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(32, logical_base as u64))),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn array_element_indexed_statement(
        &self,
        name: &str,
        indexes: &[ast::Expr],
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        value: &ast::Expr,
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        if env.net(name)?.memory.is_some() {
            let entry = env.net(name)?;
            return self.memory_part_write_statement(
                entry,
                indexes,
                start,
                width,
                ascending,
                value,
                nonblocking,
                env,
            );
        }
        if indexes
            .iter()
            .any(|index| self.const_eval(index, env.params, env.module_name).is_err())
        {
            return self.runtime_array_element_indexed_statement(
                name,
                indexes,
                start,
                width,
                ascending,
                value,
                nonblocking,
                env,
            );
        }
        let (entry, base_offset, element_width) =
            self.array_element_location(name, indexes, env)?;
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > element_width {
            return Err(format!(
                "indexed part-select width {width} exceeds array element width {element_width}"
            ));
        }
        let start =
            self.array_element_indexed_start(&entry, element_width, start, width, ascending, env)?;
        let value = self.elab_expr(value, env)?;
        let value = Self::resize_indexed_value(value, width);
        let reversed = entry
            .array
            .as_ref()
            .map(|array| array.element_msb < array.element_lsb)
            .unwrap_or_else(|| entry.msb < entry.lsb);
        let common = (
            entry.base,
            entry.width,
            base_offset,
            element_width,
            start,
            width,
        );
        if nonblocking {
            Ok(StmtIR::NBlIndexed {
                net: common.0,
                decl_width: common.1,
                base_offset: common.2,
                element_width: common.3,
                start: common.4,
                width: common.5,
                ascending: true,
                reversed,
                e: value,
            })
        } else {
            Ok(StmtIR::BlIndexed {
                net: common.0,
                decl_width: common.1,
                base_offset: common.2,
                element_width: common.3,
                start: common.4,
                width: common.5,
                ascending: true,
                reversed,
                e: value,
            })
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn runtime_array_element_indexed_statement(
        &self,
        name: &str,
        indexes: &[ast::Expr],
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        value: &ast::Expr,
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let entry = env.net(name)?;
        let array = entry
            .array
            .as_ref()
            .ok_or_else(|| format!("'{name}' is not an unpacked array"))?;
        if indexes.len() != array.dimensions.len() {
            return Err(format!(
                "array expects {} index(es), received {}",
                array.dimensions.len(),
                indexes.len()
            ));
        }
        let runtime_indexes = indexes
            .iter()
            .map(|index| self.elab_expr(index, env))
            .collect::<Result<Vec<_>, _>>()?;
        let selected_width =
            usize::try_from(self.const_eval(width, env.params, env.module_name)?)
                .map_err(|_| "indexed part-select width is out of range".to_string())?;
        if selected_width == array.element_width
            && ascending
            && array.element_msb >= array.element_lsb
            && self.const_eval(start, env.params, env.module_name).ok() == Some(0)
        {
            let value = self.elab_expr(value, env)?;
            if let Some(statement) = self.runtime_whole_array_element_statement(
                entry.base,
                entry.width,
                array,
                &runtime_indexes,
                value,
                nonblocking,
            ) {
                return Ok(statement);
            }
        }

        let mut result = None;
        for physical_element in 0..array.length {
            let mut physical_offset = physical_element;
            let mut scale = array.length;
            let mut constant_indexes = Vec::with_capacity(array.dimensions.len());
            let mut candidate_valid = ExprIR::Const(LogicWord::from_u64(1, 1));
            for dimension in &array.dimensions {
                scale /= dimension.length;
                let dimension_offset = physical_offset / scale;
                physical_offset %= scale;
                let logical_index = dimension.base + dimension.step * dimension_offset as i64;
                candidate_valid = ExprIR::Bin {
                    op: BinOp::And,
                    a: Box::new(candidate_valid),
                    b: Box::new(ExprIR::Bin {
                        op: BinOp::Eq,
                        a: Box::new(runtime_indexes[constant_indexes.len()].clone()),
                        b: Box::new(ExprIR::Const(LogicWord::from_u64(32, logical_index as u64))),
                    }),
                };
                constant_indexes.push(constant_expr(logical_index));
            }
            let candidate = self.array_element_indexed_statement(
                name,
                &constant_indexes,
                start,
                width,
                ascending,
                value,
                nonblocking,
                env,
            )?;
            result = Some(StmtIR::If {
                c: candidate_valid,
                t: Box::new(candidate),
                f: result.map(Box::new),
            });
        }
        result.ok_or_else(|| format!("array '{name}' has no elements"))
    }

    #[allow(clippy::too_many_arguments)]
    fn runtime_whole_array_element_statement(
        &self,
        net: usize,
        total_width: usize,
        array: &NetArray,
        runtime_indexes: &[ExprIR],
        value: ExprIR,
        nonblocking: bool,
    ) -> Option<StmtIR> {
        let (physical_index, in_range) =
            self.runtime_array_physical_index(array, runtime_indexes)?;
        let element_width = array.element_width;
        let value = Self::resize_indexed_value(value, element_width);
        let wide_value = Self::resize_indexed_value(value, total_width);
        let bit_offset = ExprIR::Bin {
            op: BinOp::Mul,
            a: Box::new(physical_index),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(32, element_width as u64))),
        };
        let shifted_value = ExprIR::Bin {
            op: BinOp::Or,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Shl,
                a: Box::new(wide_value),
                b: Box::new(bit_offset.clone()),
            }),
            b: Box::new(ExprIR::Const(LogicWord::zero(total_width))),
        };
        let selected_mask = ExprIR::Bin {
            op: BinOp::Shl,
            a: Box::new(ExprIR::Const(low_mask(total_width, element_width))),
            b: Box::new(bit_offset),
        };
        let clear_mask = ExprIR::Un {
            op: UnOp::Not,
            a: Box::new(selected_mask),
        };
        let old_value = ExprIR::Ref {
            net,
            decl_width: total_width,
            range: Range::Span {
                msb: total_width - 1,
                lsb: 0,
            },
            signed: false,
        };
        let merged = ExprIR::Bin {
            op: BinOp::Or,
            a: Box::new(ExprIR::Bin {
                op: BinOp::And,
                a: Box::new(old_value.clone()),
                b: Box::new(clear_mask),
            }),
            b: Box::new(shifted_value),
        };
        let value = ExprIR::Cond {
            c: Box::new(in_range),
            t: Box::new(merged),
            f: Box::new(old_value),
        };
        let lvalue = LValue {
            net,
            decl_width: total_width,
            range: Range::Span {
                msb: total_width - 1,
                lsb: 0,
            },
        };
        Some(if nonblocking {
            StmtIR::NBl {
                lv: lvalue,
                e: value,
            }
        } else {
            StmtIR::Bl {
                lv: lvalue,
                e: value,
            }
        })
    }

    pub(super) fn dynamic_indexed_part_expr(
        &self,
        name: &str,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        let entry = env.net(name)?;
        if entry.array.is_some() {
            return Err(format!(
                "indexed part-select is not supported on unpacked array '{}'",
                name
            ));
        }
        if entry.msb < entry.lsb {
            return self.reversed_indexed_part_expr(entry, start, width, ascending, env);
        }
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > entry.width {
            return Err(format!(
                "indexed part-select width {width} exceeds declaration width {}",
                entry.width
            ));
        }

        let start = self.elab_expr(start, env)?;
        let start = if entry.lsb == 0 {
            start
        } else {
            ExprIR::Bin {
                op: BinOp::Sub,
                a: Box::new(start),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(
                    32,
                    entry.lsb.unsigned_abs(),
                ))),
            }
        };
        let offset = if ascending {
            start
        } else {
            ExprIR::Bin {
                op: BinOp::Sub,
                a: Box::new(start),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, width as u64 - 1))),
            }
        };
        let max_offset = entry.width - width;
        let in_range = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Ge,
                a: Box::new(offset.clone()),
                b: Box::new(ExprIR::Const(LogicWord::zero(32))),
            }),
            b: Box::new(ExprIR::Bin {
                op: BinOp::Le,
                a: Box::new(offset.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, max_offset as u64))),
            }),
        };
        let selected = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Shr,
                a: Box::new(ExprIR::Ref {
                    net: entry.base,
                    decl_width: entry.width,
                    range: full_range(entry.width),
                    signed: false,
                }),
                b: Box::new(offset),
            }),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(
                width,
                (1_u64 << width) - 1,
            ))),
        };
        Ok(ExprIR::Cond {
            c: Box::new(in_range),
            t: Box::new(selected),
            f: Box::new(ExprIR::Const(LogicWord::xes(width))),
        })
    }

    pub(super) fn reversed_indexed_part_expr(
        &self,
        entry: &NetEntry,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > entry.width {
            return Err(format!(
                "indexed part-select width {width} exceeds declaration width {}",
                entry.width
            ));
        }

        let start = self.elab_expr(start, env)?;
        let logical_start = if ascending {
            start
        } else {
            ExprIR::Bin {
                op: BinOp::Sub,
                a: Box::new(start),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, width as u64 - 1))),
            }
        };
        let normalized_start = ExprIR::Bin {
            op: BinOp::Sub,
            a: Box::new(logical_start),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(32, entry.msb as u64))),
        };

        // A reversed declaration stores logical index `msb` in the highest
        // physical bit. Concatenating physical bits in ascending offset order
        // normalizes bit 0 to that logical left bound.
        let normalized_source = ExprIR::Concat(
            (0..entry.width)
                .map(|offset| ExprIR::Ref {
                    net: entry.base,
                    decl_width: entry.width,
                    range: Range::Bit(offset),
                    signed: false,
                })
                .collect(),
        );
        let max_offset = entry.width - width;
        let in_range = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Ge,
                a: Box::new(normalized_start.clone()),
                b: Box::new(ExprIR::Const(LogicWord::zero(32))),
            }),
            b: Box::new(ExprIR::Bin {
                op: BinOp::Le,
                a: Box::new(normalized_start.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, max_offset as u64))),
            }),
        };
        let selected = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Shr,
                a: Box::new(normalized_source),
                b: Box::new(normalized_start),
            }),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(
                width,
                (1_u64 << width) - 1,
            ))),
        };
        Ok(ExprIR::Cond {
            c: Box::new(in_range),
            t: Box::new(selected),
            f: Box::new(ExprIR::Const(LogicWord::xes(width))),
        })
    }

    pub(super) fn dynamic_indexed_part_nbl(
        &self,
        name: &str,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        value: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let entry = env.net(name)?;
        if entry.array.is_some() {
            return Err(format!(
                "indexed part-select is not supported on unpacked array '{}'",
                name
            ));
        }
        if entry.msb < entry.lsb {
            let reversed_width = self.const_eval(width, env.params, env.module_name)?;
            if reversed_width <= 0 {
                return Err(format!(
                    "indexed part-select width must be positive, got {reversed_width}"
                ));
            }
            let reversed_width = reversed_width as usize;
            let start =
                self.reversed_indexed_start(entry, start, reversed_width, ascending, env)?;
            let value = self.elab_expr(value, env)?;
            let value = Self::resize_indexed_value(value, reversed_width);
            return Ok(StmtIR::NBlIndexed {
                net: entry.base,
                decl_width: entry.width,
                base_offset: 0,
                element_width: entry.width,
                start,
                width: reversed_width,
                ascending: true,
                reversed: true,
                e: value,
            });
        }
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > entry.width {
            return Err(format!(
                "indexed part-select width {width} exceeds declaration width {}",
                entry.width
            ));
        }

        let start = self.elab_expr(start, env)?;
        let start = if entry.lsb == 0 {
            start
        } else {
            ExprIR::Bin {
                op: BinOp::Sub,
                a: Box::new(start),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(
                    32,
                    entry.lsb.unsigned_abs(),
                ))),
            }
        };
        let value = self.elab_expr(value, env)?;
        let value = if value.width() < width {
            ExprIR::Concat(vec![
                ExprIR::Const(LogicWord::zero(width - value.width())),
                value,
            ])
        } else if value.width() > width {
            ExprIR::Index {
                e: Box::new(value),
                msb: width - 1,
                lsb: 0,
            }
        } else {
            value
        };
        Ok(StmtIR::NBlIndexed {
            net: entry.base,
            decl_width: entry.width,
            base_offset: 0,
            element_width: entry.width,
            start,
            width,
            ascending,
            reversed: false,
            e: value,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn dynamic_indexed_part_bl(
        &self,
        name: &str,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        value: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        match self.dynamic_indexed_part_nbl(name, start, width, ascending, value, env)? {
            StmtIR::NBlIndexed {
                net,
                decl_width,
                base_offset,
                element_width,
                start,
                width,
                ascending,
                reversed,
                e,
            } => Ok(StmtIR::BlIndexed {
                net,
                decl_width,
                base_offset,
                element_width,
                start,
                width,
                ascending,
                reversed,
                e,
            }),
            _ => unreachable!("dynamic indexed NBA lowering always returns BlIndexed fields"),
        }
    }

    pub(super) fn reversed_indexed_start(
        &self,
        entry: &NetEntry,
        start: &ast::Expr,
        width: usize,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        if width > entry.width {
            return Err(format!(
                "indexed part-select width {width} exceeds declaration width {}",
                entry.width
            ));
        }
        let start = self.elab_expr(start, env)?;
        let logical_start = if ascending {
            start
        } else {
            ExprIR::Bin {
                op: BinOp::Sub,
                a: Box::new(start),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, width as u64 - 1))),
            }
        };
        Ok(ExprIR::Bin {
            op: BinOp::Sub,
            a: Box::new(logical_start),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(32, entry.msb as u64))),
        })
    }

    pub(super) fn dynamic_indexed_part_bl_reversed(
        &self,
        entry: &NetEntry,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        value: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > entry.width {
            return Err(format!(
                "indexed part-select width {width} exceeds declaration width {}",
                entry.width
            ));
        }
        let start = self.reversed_indexed_start(entry, start, width, ascending, env)?;
        let value = self.elab_expr(value, env)?;
        let value = Self::resize_indexed_value(value, width);
        Ok(StmtIR::BlIndexed {
            net: entry.base,
            decl_width: entry.width,
            base_offset: 0,
            element_width: entry.width,
            start,
            width,
            ascending: true,
            reversed: true,
            e: value,
        })
    }

    pub(super) fn resize_indexed_value(value: ExprIR, width: usize) -> ExprIR {
        if value.width() < width {
            ExprIR::Concat(vec![
                ExprIR::Const(LogicWord::zero(width - value.width())),
                value,
            ])
        } else if value.width() > width {
            ExprIR::Index {
                e: Box::new(value),
                msb: width - 1,
                lsb: 0,
            }
        } else {
            value
        }
    }

    pub(super) fn reverse_ast_bits(value: ast::Expr, width: usize) -> ast::Expr {
        ast::Expr::Concat(
            (0..width)
                .map(|bit| ast::Expr::Slice {
                    value: Box::new(value.clone()),
                    msb: Box::new(constant_expr(bit as i64)),
                    lsb: Box::new(constant_expr(bit as i64)),
                })
                .collect(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn function_array_indexed_update(
        old_element: ast::Expr,
        element_width: usize,
        offset: ast::Expr,
        width: usize,
        value: ast::Expr,
    ) -> ast::Expr {
        let max_offset = element_width - width;
        let in_range = ast::Expr::Bin(
            ast::BinOpSrc::LogAnd,
            Box::new(ast::Expr::Bin(
                ast::BinOpSrc::Ge,
                Box::new(offset.clone()),
                Box::new(constant_expr(0)),
            )),
            Box::new(ast::Expr::Bin(
                ast::BinOpSrc::Le,
                Box::new(offset.clone()),
                Box::new(constant_expr(max_offset as i64)),
            )),
        );
        let mask = ast::Expr::Num {
            width: Some(element_width),
            word: {
                let mut bits = LogicWord::zero(element_width).bits;
                for bit in bits.iter_mut().take(width) {
                    *bit = LogicValue::One;
                }
                LogicWord { bits }
            },
            orig: format!("function_array_indexed_mask_{width}"),
        };
        let shifted_mask =
            ast::Expr::Bin(ast::BinOpSrc::Shl, Box::new(mask), Box::new(offset.clone()));
        let value = ast::Expr::Resize {
            width: element_width,
            value: Box::new(value),
        };
        let inserted = ast::Expr::Bin(
            ast::BinOpSrc::And,
            Box::new(ast::Expr::Bin(
                ast::BinOpSrc::Shl,
                Box::new(value),
                Box::new(offset),
            )),
            Box::new(shifted_mask.clone()),
        );
        let preserved = ast::Expr::Bin(
            ast::BinOpSrc::And,
            Box::new(old_element.clone()),
            Box::new(ast::Expr::Un(ast::UnOpSrc::Not, Box::new(shifted_mask))),
        );
        let merged = ast::Expr::Bin(ast::BinOpSrc::Or, Box::new(preserved), Box::new(inserted));
        ast::Expr::Cond(Box::new(in_range), Box::new(merged), Box::new(old_element))
    }

    pub(super) fn expand_runtime_indexed_part_assignment(
        &self,
        name: &str,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        value: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<ast::Stmt, String> {
        let entry = env.net(name)?;
        if entry.array.is_some() {
            return Err(format!(
                "indexed part-select is not supported on unpacked array '{}'",
                name
            ));
        }
        if entry.msb < entry.lsb {
            return Err(format!(
                "indexed part-select on reversed packed range [{}:{}] is not yet supported",
                entry.msb, entry.lsb
            ));
        }
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > entry.width {
            return Err(format!(
                "indexed part-select width {width} exceeds declaration width {}",
                entry.width
            ));
        }

        let offset = if ascending {
            start.clone()
        } else {
            ast::Expr::Bin(
                ast::BinOpSrc::Sub,
                Box::new(start.clone()),
                Box::new(constant_expr(width as i64 - 1)),
            )
        };
        let mut mask = LogicWord::zero(entry.width);
        for bit in mask.bits.iter_mut().take(width) {
            *bit = LogicValue::One;
        }
        let mask = ast::Expr::Num {
            width: Some(entry.width),
            word: mask,
            orig: format!("runtime_indexed_mask_{width}"),
        };
        let shifted_mask =
            ast::Expr::Bin(ast::BinOpSrc::Shl, Box::new(mask), Box::new(offset.clone()));
        let target = ast::Expr::Ident(name.to_string());
        let inserted = ast::Expr::Bin(
            ast::BinOpSrc::And,
            Box::new(ast::Expr::Bin(
                ast::BinOpSrc::Shl,
                Box::new(ast::Expr::Resize {
                    width: entry.width,
                    value: Box::new(value.clone()),
                }),
                Box::new(offset.clone()),
            )),
            Box::new(shifted_mask.clone()),
        );
        let preserved = ast::Expr::Bin(
            ast::BinOpSrc::And,
            Box::new(target.clone()),
            Box::new(ast::Expr::Un(ast::UnOpSrc::Not, Box::new(shifted_mask))),
        );
        let merged = ast::Expr::Bin(ast::BinOpSrc::Or, Box::new(preserved), Box::new(inserted));
        let max_offset = entry.width - width;
        let in_range = ast::Expr::Bin(
            ast::BinOpSrc::LogAnd,
            Box::new(ast::Expr::Bin(
                ast::BinOpSrc::Ge,
                Box::new(offset.clone()),
                Box::new(constant_expr(0)),
            )),
            Box::new(ast::Expr::Bin(
                ast::BinOpSrc::Le,
                Box::new(offset),
                Box::new(constant_expr(max_offset as i64)),
            )),
        );
        let updated = ast::Expr::Cond(Box::new(in_range), Box::new(merged), Box::new(target));
        Ok(ast::Stmt::BL {
            lv: ast::Expr::Ident(name.to_string()),
            e: updated,
        })
    }

    pub(super) fn resolve_unpacked_array(
        &self,
        array_ranges: &[ast::RangeAst],
        element_width: usize,
        params: &HashMap<String, i64>,
        module_name: &str,
        object_name: &str,
        object_kind: &str,
    ) -> Result<Option<NetArray>, String> {
        const MAX_MULTIDIMENSION_ARRAY_ELEMENTS: usize = 4096;
        const MAX_SPARSE_ARRAY_ELEMENTS: usize = 1 << 24;
        const MAX_UNPACKED_ARRAY_BITS: usize = 1 << 20;
        let one_dimensional = array_ranges.len() == 1;
        let maximum_dimension = if array_ranges.len() == 1 {
            MAX_SPARSE_ARRAY_ELEMENTS
        } else {
            256
        };
        if array_ranges.len() == 1 && array_ranges[0].is_none() {
            element_width
                .checked_mul(256)
                .filter(|total| *total <= 4096)
                .ok_or_else(|| {
                    format!(
                        "module '{module_name}': dynamic {object_kind} '{object_name}' exceeds the packed-width limit"
                    )
                })?;
            return Ok(Some(NetArray {
                dimensions: vec![NetArrayDimension {
                    base: 0,
                    step: 1,
                    length: 256,
                }],
                length: 256,
                element_width,
                element_msb: element_width as i64 - 1,
                element_lsb: 0,
                dynamic: true,
                dynamic_size_base: usize::MAX,
            }));
        }
        let mut dimensions = Vec::with_capacity(array_ranges.len());
        let mut length = 1_usize;
        for array_range in array_ranges {
            let Some((left, right)) = array_range else {
                return Err(format!(
                    "module '{module_name}': unpacked {object_kind} '{object_name}' has an empty range"
                ));
            };
            let base = self.const_eval(left, params, module_name)?;
            let last = self.const_eval(right, params, module_name)?;
            let step = if base <= last { 1 } else { -1 };
            let dimension_length = (base - last).unsigned_abs() as usize + 1;
            if dimension_length == 0 || dimension_length > maximum_dimension {
                return Err(format!(
                    "module '{module_name}': unpacked {object_kind} '{object_name}' has invalid dimensions"
                ));
            }
            length = length
                .checked_mul(dimension_length)
                .filter(|total| {
                    *total <= if one_dimensional {
                        MAX_SPARSE_ARRAY_ELEMENTS
                    } else {
                        MAX_MULTIDIMENSION_ARRAY_ELEMENTS
                    }
                })
                .ok_or_else(|| {
                    format!(
                        "module '{module_name}': unpacked {object_kind} '{object_name}' exceeds the element limit"
                    )
                })?;
            dimensions.push(NetArrayDimension {
                base,
                step,
                length: dimension_length,
            });
        }
        if dimensions.is_empty() {
            return Ok(None);
        }
        if !one_dimensional {
            element_width
                .checked_mul(length)
                .filter(|total| *total <= MAX_UNPACKED_ARRAY_BITS)
                .ok_or_else(|| {
                    format!(
                        "module '{module_name}': unpacked {object_kind} '{object_name}' exceeds the packed-width limit"
                    )
                })?;
        }
        Ok(Some(NetArray {
            dimensions,
            length,
            element_width,
            element_msb: element_width as i64 - 1,
            element_lsb: 0,
            dynamic: false,
            dynamic_size_base: usize::MAX,
        }))
    }

    pub(super) fn apply_element_range(
        &self,
        array: &mut Option<NetArray>,
        range: &ast::RangeAst,
        params: &HashMap<String, i64>,
        module_name: &str,
    ) -> Result<(), String> {
        let Some(resolved) = array.as_mut() else {
            return Ok(());
        };
        if let Some((msb, lsb)) = range {
            resolved.element_msb = self.const_eval(msb, params, module_name)?;
            resolved.element_lsb = self.const_eval(lsb, params, module_name)?;
        }
        Ok(())
    }

    pub(super) fn declare_memory(&mut self, name: &str, prefix: &str, array: &NetArray) -> usize {
        let memory = self.design.memories.len();
        self.design.memories.push(MemoryDecl {
            name: format!("{prefix}{name}"),
            element_width: array.element_width,
            element_msb: array.element_msb,
            element_lsb: array.element_lsb,
            dimensions: array
                .dimensions
                .iter()
                .map(|dimension| MemoryDimension {
                    base: dimension.base,
                    step: dimension.step,
                    length: dimension.length,
                })
                .collect(),
            flat_length: array.length as u64,
            dynamic: false,
            queue: false,
            associative: false,
            index_width: 0,
            index_signed: false,
            storage: MemoryStorage::Ordinary,
            kind: MemoryKind::Ram,
            template: MemoryTemplate::Unmapped,
            write_mask: None,
            byte_enable: None,
            banking: MemoryBanking::default(),
        });
        memory
    }

    pub(super) fn memory_read_expr(
        &self,
        entry: &NetEntry,
        indexes: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        let array = entry
            .array
            .as_ref()
            .ok_or_else(|| "identifier is not an unpacked array".to_string())?;
        let memory = entry
            .memory
            .ok_or_else(|| "unpacked array has no memory object".to_string())?;
        let mut elaborated = Vec::with_capacity(indexes.len());
        for index in indexes {
            elaborated.push(self.elab_expr(index, env)?);
        }
        let read = ExprIR::MemoryRead {
            memory,
            indexes: elaborated,
            width: array.element_width,
            signed: entry.is_signed,
        };
        Ok(if array.element_msb < array.element_lsb {
            Self::reverse_bits_expr(read, array.element_width)
        } else {
            read
        })
    }

    pub(super) fn memory_write_statement(
        &self,
        entry: &NetEntry,
        indexes: &[ast::Expr],
        value: &ast::Expr,
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let array = entry
            .array
            .as_ref()
            .ok_or_else(|| "identifier is not an unpacked array".to_string())?;
        let memory = entry
            .memory
            .ok_or_else(|| "unpacked array has no memory object".to_string())?;
        let mut elaborated = Vec::with_capacity(indexes.len());
        for index in indexes {
            elaborated.push(self.elab_expr(index, env)?);
        }
        let value = self.elab_expr(value, env)?;
        let value = if array.element_msb < array.element_lsb {
            Self::reverse_bits_expr(value, array.element_width)
        } else {
            value
        };
        Ok(if nonblocking {
            StmtIR::MemoryNBl {
                memory,
                indexes: elaborated,
                e: value,
            }
        } else {
            StmtIR::MemoryBl {
                memory,
                indexes: elaborated,
                e: value,
            }
        })
    }

    fn reverse_bits_expr(value: ExprIR, width: usize) -> ExprIR {
        ExprIR::Concat(
            (0..width)
                .map(|bit| ExprIR::Index {
                    e: Box::new(value.clone()),
                    msb: bit,
                    lsb: bit,
                })
                .collect(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn memory_part_read_expr(
        &self,
        entry: &NetEntry,
        indexes: &[ast::Expr],
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        let array = entry
            .array
            .as_ref()
            .ok_or_else(|| "identifier is not an unpacked array".to_string())?;
        let memory = entry
            .memory
            .ok_or_else(|| "unpacked array has no memory object".to_string())?;
        let selected_width = usize::try_from(
            self.const_eval(width, env.params, env.module_name)
                .map_err(|_| "indexed part-select width must be constant".to_string())?,
        )
        .map_err(|_| "indexed part-select width is out of range".to_string())?;
        if selected_width == 0 || selected_width > array.element_width {
            return Err(format!(
                "indexed part-select width {selected_width} exceeds memory element width {}",
                array.element_width
            ));
        }
        let mut elaborated = Vec::with_capacity(indexes.len());
        for index in indexes {
            elaborated.push(self.elab_expr(index, env)?);
        }
        Ok(ExprIR::MemoryReadPart {
            memory,
            indexes: elaborated,
            start: Box::new(self.elab_expr(start, env)?),
            width: selected_width,
            ascending,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn memory_part_write_statement(
        &self,
        entry: &NetEntry,
        indexes: &[ast::Expr],
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        value: &ast::Expr,
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let array = entry
            .array
            .as_ref()
            .ok_or_else(|| "identifier is not an unpacked array".to_string())?;
        let memory = entry
            .memory
            .ok_or_else(|| "unpacked array has no memory object".to_string())?;
        let selected_width = usize::try_from(
            self.const_eval(width, env.params, env.module_name)
                .map_err(|_| "indexed part-select width must be constant".to_string())?,
        )
        .map_err(|_| "indexed part-select width is out of range".to_string())?;
        if selected_width == 0 || selected_width > array.element_width {
            return Err(format!(
                "indexed part-select width {selected_width} exceeds memory element width {}",
                array.element_width
            ));
        }
        let mut elaborated = Vec::with_capacity(indexes.len());
        for index in indexes {
            elaborated.push(self.elab_expr(index, env)?);
        }
        let value = self.elab_expr(value, env)?;
        let statement = if nonblocking {
            StmtIR::MemoryNBlPart {
                memory,
                indexes: elaborated,
                start: Box::new(self.elab_expr(start, env)?),
                width: selected_width,
                ascending,
                e: value,
            }
        } else {
            StmtIR::MemoryBlPart {
                memory,
                indexes: elaborated,
                start: Box::new(self.elab_expr(start, env)?),
                width: selected_width,
                ascending,
                e: value,
            }
        };
        Ok(statement)
    }

    pub(super) fn flat_array_range(total_width: usize) -> ast::RangeAst {
        Some((
            Box::new(ast::Expr::Num {
                width: Some(32),
                word: LogicWord::from_u64(32, total_width as u64 - 1),
                orig: (total_width - 1).to_string(),
            }),
            Box::new(ast::Expr::Num {
                width: Some(32),
                word: LogicWord::from_u64(32, 0),
                orig: "0".to_string(),
            }),
        ))
    }

    pub(super) fn member_reference(
        &self,
        env: &Env<'_>,
        object: &str,
        field: &str,
        write: bool,
    ) -> Result<(usize, usize, Range), String> {
        if let Some(result) = self.class_field_reference(env, object, field) {
            return result;
        }
        if let Some(fields) = env.interface_ports.get(object) {
            let entry = fields
                .get(field)
                .ok_or_else(|| format!("interface port '{}' has no signal '{}'", object, field))?;
            if let Some(directions) = env.interface_directions.get(object) {
                match directions.get(field).copied() {
                    None => {
                        return Err(format!(
                            "modport for interface '{}' does not expose '{}'",
                            object, field
                        ));
                    }
                    Some(ast::DirSrc::In) if write => {
                        return Err(format!(
                            "modport '{}' exposes '{}' as input; cannot assign it",
                            object, field
                        ));
                    }
                    Some(ast::DirSrc::Out) if !write => {
                        return Err(format!(
                            "modport '{}' exposes '{}' as output; cannot read it",
                            object, field
                        ));
                    }
                    _ => {}
                }
            }
            return Ok((entry.base, entry.width, full_range(entry.width)));
        }
        let entry = env.net(object)?;
        let type_name = entry
            .type_name
            .as_deref()
            .ok_or_else(|| format!("'{}' is not a typed struct object", object))?;
        let definition = env
            .types
            .get(type_name)
            .ok_or_else(|| format!("unknown struct type '{}'", type_name))?;
        let definition = match definition {
            ResolvedType::Struct(definition) | ResolvedType::Union(definition) => definition,
            _ => return Err(format!("type '{}' is not a packed struct/union", type_name)),
        };
        let &(offset, width) = definition
            .fields
            .get(field)
            .ok_or_else(|| format!("packed type '{}' has no field '{}'", type_name, field))?;
        Ok((
            entry.base,
            entry.width,
            Range::Span {
                msb: offset + width - 1,
                lsb: offset,
            },
        ))
    }
}

fn low_mask(total_width: usize, selected_width: usize) -> LogicWord {
    LogicWord {
        bits: (0..total_width)
            .map(|bit| LogicValue::bit(bit < selected_width))
            .collect(),
    }
}
