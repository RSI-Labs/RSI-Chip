//! String method, formatting, conversion, and radix helper elaboration.

use super::*;

impl<'a> Elaborator<'a> {
    pub(super) fn elab_sformatf(
        &self,
        arguments: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        let Some((fmt_expr, value_args)) = arguments.split_first() else {
            return Err("$sformatf requires a format string".into());
        };
        let ast::Expr::Str(fmt) = fmt_expr else {
            return Err("$sformatf format must be a string literal".into());
        };
        let mut rendered = String::new();
        let mut arg_index = 0usize;
        let mut chars = fmt.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '%' {
                rendered.push(c);
                continue;
            }
            if chars.peek() == Some(&'%') {
                chars.next();
                rendered.push('%');
                continue;
            }
            let mut spec = chars.next().ok_or("$sformatf format ends after '%'")?;
            if spec == '0' {
                spec = chars.next().ok_or("$sformatf format ends after '%0'")?;
            }
            let Some(argument) = value_args.get(arg_index) else {
                return Err(format!(
                    "$sformatf has {} format specifiers but {} arguments",
                    arg_index + 1,
                    value_args.len()
                ));
            };
            arg_index += 1;
            let value = self.elab_expr(argument, env)?;
            let ExprIR::Const(word) = &value else {
                return Err("$sformatf in this subset requires constant arguments".to_string());
            };
            match spec {
                'd' | 'D' => {
                    let Some(v) = word.to_u64() else {
                        return Err("$sformatf %d argument contains x/z".into());
                    };
                    rendered.push_str(&v.to_string());
                }
                'h' | 'H' => {
                    let Some(v) = word.to_u64() else {
                        return Err("$sformatf %h argument contains x/z".into());
                    };
                    rendered.push_str(&format!("{v:x}"));
                }
                'b' | 'B' => rendered.push_str(&word.to_bin_str()),
                's' | 'S' => rendered.push_str(&word.to_ascii_string_nul_terminated()),
                other => {
                    return Err(format!(
                        "$sformatf does not support '%{other}' in this subset"
                    ))
                }
            }
        }
        if arg_index != value_args.len() {
            return Err(format!(
                "$sformatf has {} format specifiers but {} arguments",
                arg_index,
                value_args.len()
            ));
        }
        if rendered.len() > ast::STRING_MAX_BYTES {
            return Err(format!(
                "$sformatf result '{}' exceeds the {}-byte string subset",
                rendered,
                ast::STRING_MAX_BYTES
            ));
        }
        Ok(ExprIR::Const(LogicWord::from_bytes_le(
            ast::STRING_WIDTH,
            rendered.as_bytes(),
        )))
    }

    pub(super) fn string_method_expression(
        &self,
        entry: &NetEntry,
        method: &str,
        arguments: &[ast::Expr],
        env: &Env,
    ) -> Result<Option<ExprIR>, String> {
        if entry.type_name.as_deref() != Some("string") {
            return Ok(None);
        }
        match method {
            "len" => {
                if !arguments.is_empty() {
                    return Err("string.len() expects no arguments".to_string());
                }
                let byte_count = entry.width / 8;
                let mut length = ExprIR::Const(LogicWord::from_u64(32, byte_count as u64));
                for byte in (0..byte_count).rev() {
                    length = ExprIR::Cond {
                        c: Box::new(ExprIR::Bin {
                            op: BinOp::Eq,
                            a: Box::new(self.string_byte_ref(entry, byte)),
                            b: Box::new(ExprIR::Const(LogicWord::zero(8))),
                        }),
                        t: Box::new(ExprIR::Const(LogicWord::from_u64(32, byte as u64))),
                        f: Box::new(length),
                    };
                }
                Ok(Some(length))
            }
            "getc" => {
                let Some(index) = arguments.first() else {
                    return Err("string.getc() expects one index argument".to_string());
                };
                if arguments.len() != 1 {
                    return Err("string.getc() expects one index argument".to_string());
                }
                let index = self.elab_expr(index, env)?;
                let byte_count = entry.width / 8;
                let mut selected = ExprIR::Const(LogicWord::zero(8));
                for byte in (0..byte_count).rev() {
                    selected = ExprIR::Cond {
                        c: Box::new(ExprIR::Bin {
                            op: BinOp::Eq,
                            a: Box::new(index.clone()),
                            b: Box::new(ExprIR::Const(LogicWord::from_u64(32, byte as u64))),
                        }),
                        t: Box::new(self.string_byte_ref(entry, byte)),
                        f: Box::new(selected),
                    };
                }
                Ok(Some(selected))
            }
            "toupper" | "tolower" => {
                if !arguments.is_empty() {
                    return Err(format!("string.{method}() expects no arguments"));
                }
                let upper = method == "toupper";
                let byte_count = entry.width / 8;
                let transformed = (0..byte_count)
                    .rev()
                    .map(|byte| self.string_case_byte(self.string_byte_ref(entry, byte), upper))
                    .collect();
                Ok(Some(ExprIR::Concat(transformed)))
            }
            "compare" | "icompare" => {
                let Some(argument) = arguments.first() else {
                    return Err(format!("string.{method}() expects one string argument"));
                };
                if arguments.len() != 1 {
                    return Err(format!("string.{method}() expects one string argument"));
                }
                let other = self.elab_expr(argument, env)?;
                Ok(Some(self.string_compare_expression(
                    entry,
                    other,
                    method == "icompare",
                )))
            }
            "substr" => {
                if arguments.len() != 2 {
                    return Err("string.substr() expects left and right indices".to_string());
                }
                let left = self.elab_expr(&arguments[0], env)?;
                let right = self.elab_expr(&arguments[1], env)?;
                Ok(Some(self.string_substr_expression(entry, left, right)))
            }
            _ => Ok(None),
        }
    }

    pub(super) fn string_substr_expression(
        &self,
        entry: &NetEntry,
        left: ExprIR,
        right: ExprIR,
    ) -> ExprIR {
        let byte_count = entry.width / 8;
        let mut bytes = Vec::with_capacity(byte_count);
        for output in 0..byte_count {
            let source_index = ExprIR::Bin {
                op: BinOp::Add,
                a: Box::new(left.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, output as u64))),
            };
            let in_range = ExprIR::Bin {
                op: BinOp::And,
                a: Box::new(ExprIR::Bin {
                    op: BinOp::Ge,
                    a: Box::new(left.clone()),
                    b: Box::new(ExprIR::Const(LogicWord::zero(32))),
                }),
                b: Box::new(ExprIR::Bin {
                    op: BinOp::Le,
                    a: Box::new(source_index.clone()),
                    b: Box::new(right.clone()),
                }),
            };
            bytes.push(ExprIR::Cond {
                c: Box::new(in_range),
                t: Box::new(self.string_indexed_byte(entry, source_index)),
                f: Box::new(ExprIR::Const(LogicWord::zero(8))),
            });
        }
        bytes.reverse();
        ExprIR::Concat(bytes)
    }

    pub(super) fn string_indexed_byte(&self, entry: &NetEntry, index: ExprIR) -> ExprIR {
        let byte_count = entry.width / 8;
        let mut selected = ExprIR::Const(LogicWord::zero(8));
        for byte in (0..byte_count).rev() {
            selected = ExprIR::Cond {
                c: Box::new(ExprIR::Bin {
                    op: BinOp::Eq,
                    a: Box::new(index.clone()),
                    b: Box::new(ExprIR::Const(LogicWord::from_u64(32, byte as u64))),
                }),
                t: Box::new(self.string_byte_ref(entry, byte)),
                f: Box::new(selected),
            };
        }
        selected
    }

    pub(super) fn string_compare_expression(
        &self,
        entry: &NetEntry,
        other: ExprIR,
        ignore_case: bool,
    ) -> ExprIR {
        let byte_count = entry.width / 8;
        let mut result = ExprIR::Const(LogicWord::zero(32));
        for byte in (0..byte_count).rev() {
            let mut left = self.string_byte_ref(entry, byte);
            let mut right = Self::string_expr_byte(other.clone(), byte);
            if ignore_case {
                left = self.string_case_byte(left, true);
                right = self.string_case_byte(right, true);
            }
            let ordering = ExprIR::Cond {
                c: Box::new(ExprIR::Bin {
                    op: BinOp::Lt,
                    a: Box::new(left.clone()),
                    b: Box::new(right.clone()),
                }),
                t: Box::new(ExprIR::Const(LogicWord::from_u64(32, (-1_i64) as u64))),
                f: Box::new(ExprIR::Const(LogicWord::from_u64(32, 1))),
            };
            result = ExprIR::Cond {
                c: Box::new(ExprIR::Bin {
                    op: BinOp::Ne,
                    a: Box::new(left),
                    b: Box::new(right),
                }),
                t: Box::new(ordering),
                f: Box::new(result),
            };
        }
        result
    }

    pub(super) fn string_expr_byte(value: ExprIR, byte: usize) -> ExprIR {
        let lsb = byte * 8;
        ExprIR::Index {
            e: Box::new(value),
            msb: lsb + 7,
            lsb,
        }
    }

    pub(super) fn string_case_byte(&self, byte: ExprIR, upper: bool) -> ExprIR {
        let (low, delta, op) = if upper {
            (b'a', 32_u64, BinOp::Sub)
        } else {
            (b'A', 32_u64, BinOp::Add)
        };
        let high = low + 25;
        let in_range = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Ge,
                a: Box::new(byte.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(8, low as u64))),
            }),
            b: Box::new(ExprIR::Bin {
                op: BinOp::Le,
                a: Box::new(byte.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(8, high as u64))),
            }),
        };
        ExprIR::Cond {
            c: Box::new(in_range),
            t: Box::new(ExprIR::Bin {
                op,
                a: Box::new(byte.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(8, delta))),
            }),
            f: Box::new(byte),
        }
    }

    pub(super) fn string_putc_statement(
        &self,
        entry: &NetEntry,
        arguments: &[ast::Expr],
        env: &Env,
    ) -> Result<StmtIR, String> {
        if arguments.len() != 2 {
            return Err("string.putc() expects index and byte arguments".to_string());
        }
        let index = self.elab_expr(&arguments[0], env)?;
        let value = self.elab_expr(&arguments[1], env)?;
        let byte_count = entry.width / 8;
        let mut statements = Vec::with_capacity(byte_count);
        for byte in 0..byte_count {
            let lsb = byte * 8;
            statements.push(StmtIR::If {
                c: ExprIR::Bin {
                    op: BinOp::Eq,
                    a: Box::new(index.clone()),
                    b: Box::new(ExprIR::Const(LogicWord::from_u64(32, byte as u64))),
                },
                t: Box::new(StmtIR::Bl {
                    lv: LValue {
                        net: entry.base,
                        decl_width: entry.width,
                        range: Range::Span { msb: lsb + 7, lsb },
                    },
                    e: value.clone(),
                }),
                f: None,
            });
        }
        Ok(StmtIR::Block(statements))
    }

    pub(super) fn string_itoa_statement(
        &mut self,
        entry: &NetEntry,
        arguments: &[ast::Expr],
        env: &Env,
    ) -> Result<StmtIR, String> {
        let Some(argument) = arguments.first() else {
            return Err("string.itoa() expects one integer argument".to_string());
        };
        if arguments.len() != 1 {
            return Err("string.itoa() expects one integer argument".to_string());
        }
        let input = self.elab_expr(argument, env)?;
        let input = ExprIR::Signed {
            e: Box::new(input),
            signed: false,
        };

        let declare_state = |self_: &mut Self, name: &str, width: usize| {
            self_.declare_net(
                name,
                "",
                &Some((
                    Box::new(ast::Expr::Num {
                        width: Some(32),
                        word: LogicWord::from_u64(32, width as i64 as u64),
                        orig: (width - 1).to_string(),
                    }),
                    Box::new(ast::Expr::Num {
                        width: Some(32),
                        word: LogicWord::zero(32),
                        orig: "0".to_string(),
                    }),
                )),
                &HashMap::new(),
                true,
                false,
                None,
                None,
            )
        };
        let sign = declare_state(self, "__string_itoa_sign", 1)?;
        let magnitude = declare_state(self, "__string_itoa_magnitude", 32)?;
        let count = declare_state(self, "__string_itoa_count", 32)?;
        let mut digits = Vec::with_capacity(10);
        for digit in 0..10 {
            digits.push(declare_state(
                self,
                &format!("__string_itoa_digit_{digit}"),
                8,
            )?);
        }

        let state_ref = |entry: &NetEntry| -> ExprIR {
            ExprIR::Ref {
                net: entry.base,
                decl_width: entry.width,
                range: full_range(entry.width),
                signed: false,
            }
        };
        let assign_state = |entry: &NetEntry, value: ExprIR| -> StmtIR {
            StmtIR::Bl {
                lv: LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: full_range(entry.width),
                },
                e: value,
            }
        };
        let const32 = |value: u64| ExprIR::Const(LogicWord::from_u64(32, value));

        let input_bit31 = ExprIR::Index {
            e: Box::new(input.clone()),
            msb: 31,
            lsb: 31,
        };
        let negated = ExprIR::Bin {
            op: BinOp::Sub,
            a: Box::new(ExprIR::Const(LogicWord::zero(32))),
            b: Box::new(input.clone()),
        };
        let mut statements = vec![
            assign_state(&sign, input_bit31),
            StmtIR::If {
                c: ExprIR::Bin {
                    op: BinOp::Eq,
                    a: Box::new(state_ref(&sign)),
                    b: Box::new(ExprIR::Const(LogicWord::ones(1))),
                },
                t: Box::new(assign_state(&magnitude, negated)),
                f: Some(Box::new(assign_state(&magnitude, input.clone()))),
            },
            assign_state(&count, const32(1)),
        ];

        let mut threshold = 10_u64;
        for _ in 1..10 {
            let more_digits = ExprIR::Cond {
                c: Box::new(ExprIR::Bin {
                    op: BinOp::Ge,
                    a: Box::new(state_ref(&magnitude)),
                    b: Box::new(const32(threshold)),
                }),
                t: Box::new(const32(1)),
                f: Box::new(const32(0)),
            };
            let next_count = ExprIR::Bin {
                op: BinOp::Add,
                a: Box::new(state_ref(&count)),
                b: Box::new(more_digits),
            };
            statements.push(assign_state(&count, next_count));
            threshold = threshold.saturating_mul(10);
        }

        for digit_entry in &digits {
            let digit_value = ExprIR::Bin {
                op: BinOp::Add,
                a: Box::new(ExprIR::Bin {
                    op: BinOp::Mod,
                    a: Box::new(state_ref(&magnitude)),
                    b: Box::new(const32(10)),
                }),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(8, b'0' as u64))),
            };
            let quotient = ExprIR::Bin {
                op: BinOp::Div,
                a: Box::new(state_ref(&magnitude)),
                b: Box::new(const32(10)),
            };
            statements.push(assign_state(digit_entry, digit_value));
            statements.push(assign_state(&magnitude, quotient));
        }

        for output in 0..entry.width / 8 {
            let mut byte_expr = ExprIR::Const(LogicWord::zero(8));
            for digit_count in 1..=10_usize {
                for signed in [false, true] {
                    let offset = usize::from(signed);
                    let digit_position = if signed && output == 0 {
                        Some(0)
                    } else {
                        output.checked_sub(offset)
                    };
                    let Some(digit_index) = digit_position else {
                        continue;
                    };
                    let Some(digit_index) = digit_count
                        .checked_sub(1)
                        .and_then(|highest| highest.checked_sub(digit_index))
                    else {
                        continue;
                    };
                    let character = if signed && output == 0 {
                        ExprIR::Const(LogicWord::from_u64(8, b'-' as u64))
                    } else {
                        state_ref(&digits[digit_index])
                    };
                    let matches = ExprIR::Bin {
                        op: BinOp::And,
                        a: Box::new(ExprIR::Bin {
                            op: BinOp::Eq,
                            a: Box::new(state_ref(&count)),
                            b: Box::new(const32(digit_count as u64)),
                        }),
                        b: Box::new(ExprIR::Bin {
                            op: BinOp::Eq,
                            a: Box::new(state_ref(&sign)),
                            b: Box::new(ExprIR::Const(LogicWord::from_u64(1, u64::from(signed)))),
                        }),
                    };
                    byte_expr = ExprIR::Cond {
                        c: Box::new(matches),
                        t: Box::new(character),
                        f: Box::new(byte_expr),
                    };
                }
            }
            let lsb = output * 8;
            statements.push(StmtIR::Bl {
                lv: LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: Range::Span { msb: lsb + 7, lsb },
                },
                e: byte_expr,
            });
        }

        Ok(StmtIR::Block(statements))
    }

    pub(super) fn string_power_radix_statement(
        &mut self,
        entry: &NetEntry,
        method: &str,
        arguments: &[ast::Expr],
        env: &Env,
    ) -> Result<StmtIR, String> {
        let Some(argument) = arguments.first() else {
            return Err(format!("string.{method}() expects one integer argument"));
        };
        if arguments.len() != 1 {
            return Err(format!("string.{method}() expects one integer argument"));
        }
        let (base_bits, digit_count) = match method {
            "hextoa" => (4_usize, 8_usize),
            "bintoa" => (1, 32),
            _ => return Err(format!("unsupported string format method '{method}'")),
        };
        let input = self.elab_expr(argument, env)?;
        let input = ExprIR::Signed {
            e: Box::new(input),
            signed: false,
        };

        let declare_state = |self_: &mut Self, name: &str, width: usize| {
            self_.declare_net(
                name,
                "",
                &Some((
                    Box::new(ast::Expr::Num {
                        width: Some(32),
                        word: LogicWord::from_u64(32, width as i64 as u64),
                        orig: (width - 1).to_string(),
                    }),
                    Box::new(ast::Expr::Num {
                        width: Some(32),
                        word: LogicWord::zero(32),
                        orig: "0".to_string(),
                    }),
                )),
                &HashMap::new(),
                true,
                false,
                None,
                None,
            )
        };
        let value = declare_state(self, "__string_radix_format_value", 32)?;
        let count = declare_state(self, "__string_radix_format_count", 32)?;
        let mut digits = Vec::with_capacity(digit_count);
        for digit in 0..digit_count {
            digits.push(declare_state(
                self,
                &format!("__string_radix_format_digit_{digit}"),
                8,
            )?);
        }

        let state_ref = |entry: &NetEntry| -> ExprIR {
            ExprIR::Ref {
                net: entry.base,
                decl_width: entry.width,
                range: full_range(entry.width),
                signed: false,
            }
        };
        let assign_state = |entry: &NetEntry, value: ExprIR| -> StmtIR {
            StmtIR::Bl {
                lv: LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: full_range(entry.width),
                },
                e: value,
            }
        };
        let const32 = |value: u64| ExprIR::Const(LogicWord::from_u64(32, value));

        let mut statements = vec![
            assign_state(&value, input.clone()),
            assign_state(&count, const32(1)),
        ];
        let base = 1_u64 << base_bits;
        let mut threshold = base;
        for _ in 1..digit_count {
            let more_digits = ExprIR::Cond {
                c: Box::new(ExprIR::Bin {
                    op: BinOp::Ge,
                    a: Box::new(state_ref(&value)),
                    b: Box::new(const32(threshold)),
                }),
                t: Box::new(const32(1)),
                f: Box::new(const32(0)),
            };
            let next_count = ExprIR::Bin {
                op: BinOp::Add,
                a: Box::new(state_ref(&count)),
                b: Box::new(more_digits),
            };
            statements.push(assign_state(&count, next_count));
            threshold = threshold.saturating_mul(base);
        }

        for digit_entry in &digits {
            let raw_digit = ExprIR::Bin {
                op: BinOp::And,
                a: Box::new(state_ref(&value)),
                b: Box::new(const32(base - 1)),
            };
            let ascii_digit = if method == "bintoa" {
                ExprIR::Bin {
                    op: BinOp::Add,
                    a: Box::new(raw_digit),
                    b: Box::new(ExprIR::Const(LogicWord::from_u64(8, b'0' as u64))),
                }
            } else {
                ExprIR::Cond {
                    c: Box::new(ExprIR::Bin {
                        op: BinOp::Lt,
                        a: Box::new(raw_digit.clone()),
                        b: Box::new(const32(10)),
                    }),
                    t: Box::new(ExprIR::Bin {
                        op: BinOp::Add,
                        a: Box::new(raw_digit.clone()),
                        b: Box::new(ExprIR::Const(LogicWord::from_u64(8, b'0' as u64))),
                    }),
                    f: Box::new(ExprIR::Bin {
                        op: BinOp::Add,
                        a: Box::new(ExprIR::Bin {
                            op: BinOp::Sub,
                            a: Box::new(raw_digit),
                            b: Box::new(const32(10)),
                        }),
                        b: Box::new(ExprIR::Const(LogicWord::from_u64(8, b'a' as u64))),
                    }),
                }
            };
            let shifted = ExprIR::Bin {
                op: BinOp::Shr,
                a: Box::new(state_ref(&value)),
                b: Box::new(const32(base_bits as u64)),
            };
            statements.push(assign_state(digit_entry, ascii_digit));
            statements.push(assign_state(&value, shifted));
        }

        for output in 0..entry.width / 8 {
            let mut byte_expr = ExprIR::Const(LogicWord::zero(8));
            for possible_count in 1..=digit_count {
                let Some(digit_index) = possible_count
                    .checked_sub(1)
                    .and_then(|highest| highest.checked_sub(output))
                else {
                    continue;
                };
                let matches = ExprIR::Bin {
                    op: BinOp::Eq,
                    a: Box::new(state_ref(&count)),
                    b: Box::new(const32(possible_count as u64)),
                };
                byte_expr = ExprIR::Cond {
                    c: Box::new(matches),
                    t: Box::new(state_ref(&digits[digit_index])),
                    f: Box::new(byte_expr),
                };
            }
            let lsb = output * 8;
            statements.push(StmtIR::Bl {
                lv: LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: Range::Span { msb: lsb + 7, lsb },
                },
                e: byte_expr,
            });
        }

        Ok(StmtIR::Block(statements))
    }

    pub(super) fn string_radix_statement(
        &mut self,
        entry: &NetEntry,
        target: LValue,
        method: &str,
        arguments: &[ast::Expr],
        _env: &Env,
    ) -> Result<StmtIR, String> {
        let base = match method {
            "atoi" => 10_u64,
            "atohex" => 16,
            "atobin" => 2,
            _ => return Err(format!("unsupported string radix method '{method}'")),
        };
        if !arguments.is_empty() {
            return Err(format!("string.{method}() expects no arguments"));
        }

        let declare_state = |self_: &mut Self, name: &str, width: usize| {
            self_.declare_net(
                name,
                "",
                &Some((
                    Box::new(ast::Expr::Num {
                        width: Some(32),
                        word: LogicWord::from_u64(32, width as i64 as u64),
                        orig: (width - 1).to_string(),
                    }),
                    Box::new(ast::Expr::Num {
                        width: Some(32),
                        word: LogicWord::zero(32),
                        orig: "0".to_string(),
                    }),
                )),
                &HashMap::new(),
                true,
                false,
                None,
                None,
            )
        };
        let value = declare_state(self, "__string_radix_value", 32)?;
        let sign = declare_state(self, "__string_radix_sign", 32)?;
        let started = declare_state(self, "__string_radix_started", 1)?;
        let sign_seen = declare_state(self, "__string_radix_sign_seen", 1)?;
        let stopped = declare_state(self, "__string_radix_stopped", 1)?;

        let state_ref = |entry: &NetEntry| -> ExprIR {
            ExprIR::Ref {
                net: entry.base,
                decl_width: entry.width,
                range: full_range(entry.width),
                signed: false,
            }
        };
        let assign_state = |entry: &NetEntry, value: ExprIR| -> StmtIR {
            StmtIR::Bl {
                lv: LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: full_range(entry.width),
                },
                e: value,
            }
        };

        let mut statements = vec![
            assign_state(&value, ExprIR::Const(LogicWord::zero(32))),
            assign_state(&sign, ExprIR::Const(LogicWord::from_u64(32, 1))),
            assign_state(&started, ExprIR::Const(LogicWord::zero(1))),
            assign_state(&sign_seen, ExprIR::Const(LogicWord::zero(1))),
            assign_state(&stopped, ExprIR::Const(LogicWord::zero(1))),
        ];

        for byte in 0..entry.width / 8 {
            let b = self.string_byte_ref(entry, byte);
            let eq = |value: u8| ExprIR::Bin {
                op: BinOp::Eq,
                a: Box::new(b.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(8, value as u64))),
            };
            let in_range = |low: u8, high: u8| ExprIR::Bin {
                op: BinOp::And,
                a: Box::new(ExprIR::Bin {
                    op: BinOp::Ge,
                    a: Box::new(b.clone()),
                    b: Box::new(ExprIR::Const(LogicWord::from_u64(8, low as u64))),
                }),
                b: Box::new(ExprIR::Bin {
                    op: BinOp::Le,
                    a: Box::new(b.clone()),
                    b: Box::new(ExprIR::Const(LogicWord::from_u64(8, high as u64))),
                }),
            };
            let (is_digit, digit) = match method {
                "atoi" => {
                    let is_digit = in_range(b'0', b'9');
                    let digit = ExprIR::Bin {
                        op: BinOp::Sub,
                        a: Box::new(b.clone()),
                        b: Box::new(ExprIR::Const(LogicWord::from_u64(8, b'0' as u64))),
                    };
                    (is_digit, digit)
                }
                "atobin" => {
                    let zero = eq(b'0');
                    let one = eq(b'1');
                    let is_digit = ExprIR::Bin {
                        op: BinOp::Or,
                        a: Box::new(zero),
                        b: Box::new(one.clone()),
                    };
                    let digit = ExprIR::Cond {
                        c: Box::new(one),
                        t: Box::new(ExprIR::Const(LogicWord::from_u64(8, 1))),
                        f: Box::new(ExprIR::Const(LogicWord::zero(8))),
                    };
                    (is_digit, digit)
                }
                _ => {
                    let decimal = in_range(b'0', b'9');
                    let lower = in_range(b'a', b'f');
                    let upper = in_range(b'A', b'F');
                    let is_digit = ExprIR::Bin {
                        op: BinOp::Or,
                        a: Box::new(decimal.clone()),
                        b: Box::new(ExprIR::Bin {
                            op: BinOp::Or,
                            a: Box::new(lower.clone()),
                            b: Box::new(upper.clone()),
                        }),
                    };
                    let decimal_digit = ExprIR::Bin {
                        op: BinOp::Sub,
                        a: Box::new(b.clone()),
                        b: Box::new(ExprIR::Const(LogicWord::from_u64(8, b'0' as u64))),
                    };
                    let add_offset = |low: u8| ExprIR::Bin {
                        op: BinOp::Add,
                        a: Box::new(ExprIR::Bin {
                            op: BinOp::Sub,
                            a: Box::new(b.clone()),
                            b: Box::new(ExprIR::Const(LogicWord::from_u64(8, low as u64))),
                        }),
                        b: Box::new(ExprIR::Const(LogicWord::from_u64(8, 10))),
                    };
                    let digit = ExprIR::Cond {
                        c: Box::new(decimal),
                        t: Box::new(decimal_digit),
                        f: Box::new(ExprIR::Cond {
                            c: Box::new(lower),
                            t: Box::new(add_offset(b'a')),
                            f: Box::new(ExprIR::Cond {
                                c: Box::new(upper),
                                t: Box::new(add_offset(b'A')),
                                f: Box::new(ExprIR::Const(LogicWord::zero(8))),
                            }),
                        }),
                    };
                    (is_digit, digit)
                }
            };

            let whitespace = ExprIR::Bin {
                op: BinOp::Or,
                a: Box::new(eq(b' ')),
                b: Box::new(ExprIR::Bin {
                    op: BinOp::Or,
                    a: Box::new(eq(b'\t')),
                    b: Box::new(ExprIR::Bin {
                        op: BinOp::Or,
                        a: Box::new(eq(b'\n')),
                        b: Box::new(eq(b'\r')),
                    }),
                }),
            };
            let plus = eq(b'+');
            let minus = eq(b'-');
            let continued = ExprIR::Bin {
                op: BinOp::Add,
                a: Box::new(ExprIR::Bin {
                    op: BinOp::Mul,
                    a: Box::new(state_ref(&value)),
                    b: Box::new(ExprIR::Const(LogicWord::from_u64(32, base))),
                }),
                b: Box::new(digit.clone()),
            };
            let digit_statements = StmtIR::Block(vec![
                StmtIR::If {
                    c: ExprIR::Bin {
                        op: BinOp::Eq,
                        a: Box::new(state_ref(&started)),
                        b: Box::new(ExprIR::Const(LogicWord::zero(1))),
                    },
                    t: Box::new(assign_state(&value, digit)),
                    f: Some(Box::new(assign_state(&value, continued))),
                },
                assign_state(&started, ExprIR::Const(LogicWord::ones(1))),
                assign_state(&sign_seen, ExprIR::Const(LogicWord::ones(1))),
            ]);
            let prefix_consumed = ExprIR::Bin {
                op: BinOp::Or,
                a: Box::new(state_ref(&started)),
                b: Box::new(state_ref(&sign_seen)),
            };
            let non_digit = StmtIR::If {
                c: prefix_consumed,
                t: Box::new(assign_state(&stopped, ExprIR::Const(LogicWord::ones(1)))),
                f: Some(Box::new(StmtIR::If {
                    c: whitespace,
                    t: Box::new(StmtIR::Block(Vec::new())),
                    f: Some(Box::new(StmtIR::If {
                        c: plus,
                        t: Box::new(assign_state(&sign_seen, ExprIR::Const(LogicWord::ones(1)))),
                        f: Some(Box::new(StmtIR::If {
                            c: minus,
                            t: Box::new(StmtIR::Block(vec![
                                assign_state(
                                    &sign,
                                    ExprIR::Const(LogicWord::from_u64(32, (-1_i64) as u64)),
                                ),
                                assign_state(&sign_seen, ExprIR::Const(LogicWord::ones(1))),
                            ])),
                            f: Some(Box::new(assign_state(
                                &stopped,
                                ExprIR::Const(LogicWord::ones(1)),
                            ))),
                        })),
                    })),
                })),
            };
            statements.push(StmtIR::If {
                c: ExprIR::Bin {
                    op: BinOp::Eq,
                    a: Box::new(state_ref(&stopped)),
                    b: Box::new(ExprIR::Const(LogicWord::ones(1))),
                },
                t: Box::new(StmtIR::Block(Vec::new())),
                f: Some(Box::new(StmtIR::If {
                    c: is_digit,
                    t: Box::new(digit_statements),
                    f: Some(Box::new(non_digit)),
                })),
            });
        }

        statements.push(StmtIR::Bl {
            lv: target,
            e: ExprIR::Bin {
                op: BinOp::Mul,
                a: Box::new(state_ref(&sign)),
                b: Box::new(state_ref(&value)),
            },
        });
        Ok(StmtIR::Block(statements))
    }

    pub(super) fn string_byte_ref(&self, entry: &NetEntry, byte: usize) -> ExprIR {
        let lsb = byte * 8;
        ExprIR::Ref {
            net: entry.base,
            decl_width: entry.width,
            range: Range::Span { msb: lsb + 7, lsb },
            signed: false,
        }
    }
}
