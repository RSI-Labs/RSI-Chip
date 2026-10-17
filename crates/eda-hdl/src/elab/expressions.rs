//! Class-handle comparisons and expression elaboration.

use super::*;

impl<'a> Elaborator<'a> {
    pub(super) fn class_handle_null_compare(
        &self,
        expression: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<Option<ExprIR>, String> {
        let ast::Expr::Bin(op, a, b) = expression else {
            return Ok(None);
        };
        let equality = match op {
            ast::BinOpSrc::Eq => true,
            ast::BinOpSrc::Ne => false,
            _ => return Ok(None),
        };
        let is_null =
            |expression: &ast::Expr| matches!(expression, ast::Expr::Ident(name) if name == "null");
        if is_null(a) && is_null(b) {
            return Ok(Some(ExprIR::Const(LogicWord::from_u64(
                1,
                u64::from(equality),
            ))));
        }
        let handle = if is_null(a) {
            b
        } else if is_null(b) {
            a
        } else {
            return Ok(None);
        };
        let flag = self
            .class_handle_flag_expression(handle, env)?
            .ok_or_else(|| "null comparisons require a class handle".to_string())?;
        let comparison = ExprIR::Bin {
            op: if equality { BinOp::Eq } else { BinOp::Ne },
            a: Box::new(flag),
            b: Box::new(ExprIR::Const(LogicWord::zero(1))),
        };
        Ok(Some(comparison))
    }

    pub(super) fn class_handle_equality_compare(
        &self,
        expression: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<Option<ExprIR>, String> {
        let ast::Expr::Bin(op, a, b) = expression else {
            return Ok(None);
        };
        let equality = match op {
            ast::BinOpSrc::Eq => true,
            ast::BinOpSrc::Ne => false,
            _ => return Ok(None),
        };
        let Some(left_flag) = self.class_handle_flag_expression(a, env)? else {
            return Ok(None);
        };
        let Some(right_flag) = self.class_handle_flag_expression(b, env)? else {
            return Ok(None);
        };
        let Some(left_oid) = self.class_handle_object_id_expression(a, env)? else {
            return Ok(None);
        };
        let Some(right_oid) = self.class_handle_object_id_expression(b, env)? else {
            return Ok(None);
        };
        let object_ids_equal = ExprIR::Bin {
            op: BinOp::Eq,
            a: Box::new(left_oid),
            b: Box::new(right_oid),
        };
        let both_allocated = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(left_flag),
            b: Box::new(right_flag),
        };
        let equal = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(both_allocated),
            b: Box::new(object_ids_equal),
        };
        Ok(Some(if equality {
            equal
        } else {
            ExprIR::Un {
                op: UnOp::Not,
                a: Box::new(equal),
            }
        }))
    }

    pub(super) fn class_handle_flag_expression(
        &self,
        expression: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<Option<ExprIR>, String> {
        let flag_net = match expression {
            ast::Expr::Ident(name) => {
                if !env.class_handles.contains_key(name)
                    && !env.class_aliases.borrow().contains_key(name)
                {
                    return Ok(None);
                }
                env.canon(name)
            }
            ast::Expr::Member(object, field) => {
                let path = vec![object.clone(), field.clone()];
                match self.resolve_class_path(env, &path)? {
                    ClassPathTarget::Handle { flag_net, .. } => flag_net,
                    ClassPathTarget::Field(_) => return Ok(None),
                }
            }
            ast::Expr::MemberChain(path) => match self.resolve_class_path(env, path)? {
                ClassPathTarget::Handle { flag_net, .. } => flag_net,
                ClassPathTarget::Field(_) => return Ok(None),
            },
            _ => return Ok(None),
        };
        let entry = env.net(&flag_net)?;
        Ok(Some(ExprIR::Ref {
            net: entry.base,
            decl_width: entry.width,
            range: full_range(entry.width),
            signed: false,
        }))
    }

    pub(super) fn class_handle_object_id_expression(
        &self,
        expression: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<Option<ExprIR>, String> {
        let handle = match expression {
            ast::Expr::Ident(name) => {
                if !env.class_handles.contains_key(name)
                    && !env.class_aliases.borrow().contains_key(name)
                {
                    return Ok(None);
                }
                env.canon(name)
            }
            ast::Expr::Member(object, field) => {
                let path = vec![object.clone(), field.clone()];
                match self.resolve_class_path(env, &path)? {
                    ClassPathTarget::Handle { prefix, .. } => prefix,
                    ClassPathTarget::Field(_) => return Ok(None),
                }
            }
            ast::Expr::MemberChain(path) => match self.resolve_class_path(env, path)? {
                ClassPathTarget::Handle { prefix, .. } => prefix,
                ClassPathTarget::Field(_) => return Ok(None),
            },
            _ => return Ok(None),
        };
        let entry = env.net(&object_id_net(&handle))?;
        Ok(Some(ExprIR::Ref {
            net: entry.base,
            decl_width: entry.width,
            range: full_range(entry.width),
            signed: false,
        }))
    }

    pub(super) fn elab_expr(&self, e: &ast::Expr, env: &Env) -> Result<ExprIR, String> {
        if let Some(comparison) = self.class_handle_null_compare(e, env)? {
            return Ok(comparison);
        }
        if let Some(comparison) = self.class_handle_equality_compare(e, env)? {
            return Ok(comparison);
        }
        Ok(match e {
            ast::Expr::Num { width, word, .. } => {
                ExprIR::Const(word.clone().sized(width.unwrap_or(32)))
            }
            ast::Expr::Str(s) => {
                // 字符串字面量：≤64 字符，首字符在最低字节，NUL 填充到
                // 512 位，与 `string` 类型变量布局一致。
                const STR_MAX: usize = ast::STRING_MAX_BYTES;
                if s.len() > STR_MAX {
                    return Err(format!(
                        "string literals are limited to {STR_MAX} bytes in this subset, got {} bytes",
                        s.len()
                    ));
                }
                ExprIR::Const(LogicWord::from_bytes_le(ast::STRING_WIDTH, s.as_bytes()))
            }
            ast::Expr::Ident(n) => {
                if let Some(value) = env.method_args.get(n) {
                    return Ok(value.clone());
                }
                if let Some(pv) = env.params.get(n) {
                    return Ok(ExprIR::Const(LogicWord::from_u64(32, *pv as u64)));
                }
                let entry = env.net(n)?;
                ExprIR::Ref {
                    net: entry.base,
                    decl_width: entry.width,
                    range: full_range(entry.width),
                    signed: entry.is_signed,
                }
            }
            ast::Expr::ScopedIdent { package, name } => {
                let value = self
                    .packages
                    .constants
                    .get(package)
                    .and_then(|values| values.get(name))
                    .copied()
                    .ok_or_else(|| format!("unknown package constant '{}::{}'", package, name))?;
                ExprIR::Const(LogicWord::from_u64(32, value as u64))
            }
            ast::Expr::Select(n, idx) => {
                let entry = env.net(n)?;
                if entry.memory.is_some() {
                    let indexes = std::slice::from_ref(idx.as_ref());
                    return self.memory_read_expr(entry, indexes, env);
                }
                if entry.array.is_some()
                    && self.const_eval(idx, env.params, env.module_name).is_err()
                {
                    let element_width = entry
                        .array
                        .as_ref()
                        .map(|array| array.element_width)
                        .ok_or_else(|| format!("'{n}' is not an unpacked array"))?;
                    let indexes = vec![idx.as_ref().clone()];
                    let start = constant_expr(0);
                    let width = constant_expr(element_width as i64);
                    return self.array_element_indexed_expr(n, &indexes, &start, &width, true, env);
                }
                let i = self.const_eval(idx, env.params, env.module_name)?;
                let off = if entry.array.is_some() {
                    entry.array_offset(&[i])?
                } else {
                    entry.offset(i)?
                };
                let range = if let Some(array) = entry.array.as_ref() {
                    Range::Span {
                        msb: off + array.element_width - 1,
                        lsb: off,
                    }
                } else {
                    Range::Bit(off)
                };
                ExprIR::Ref {
                    net: entry.base,
                    decl_width: entry.width,
                    range,
                    signed: false,
                }
            }
            ast::Expr::ArraySelect(..) => {
                let ast::Expr::ArraySelect(n, indexes) = e else {
                    unreachable!("pattern was already checked");
                };
                let entry = env.net(n)?;
                if entry.memory.is_some() {
                    return self.memory_read_expr(entry, indexes, env);
                }
                if indexes
                    .iter()
                    .any(|index| self.const_eval(index, env.params, env.module_name).is_err())
                {
                    let element_width = entry
                        .array
                        .as_ref()
                        .map(|array| array.element_width)
                        .ok_or_else(|| format!("'{n}' is not an unpacked array"))?;
                    let start = constant_expr(0);
                    let width = constant_expr(element_width as i64);
                    return self.array_element_indexed_expr(n, indexes, &start, &width, true, env);
                }
                let mut resolved = Vec::with_capacity(indexes.len());
                for index in indexes {
                    resolved.push(self.const_eval(index, env.params, env.module_name)?);
                }
                let off = entry.array_offset(&resolved)?;
                let element_width = entry
                    .array
                    .as_ref()
                    .map(|array| array.element_width)
                    .ok_or_else(|| format!("'{n}' is not an unpacked array"))?;
                ExprIR::Ref {
                    net: entry.base,
                    decl_width: entry.width,
                    range: Range::Span {
                        msb: off + element_width - 1,
                        lsb: off,
                    },
                    signed: false,
                }
            }
            ast::Expr::ArrayIndexedPart {
                name,
                indexes,
                start,
                width,
                ascending,
            } => self.array_element_indexed_expr(name, indexes, start, width, *ascending, env)?,
            ast::Expr::ArrayPart {
                name,
                indexes,
                msb,
                lsb,
            } => {
                let normalized = self.normalize_array_part(name, indexes, msb, lsb, env)?;
                self.elab_expr(&normalized, env)?
            }
            ast::Expr::Member(object, field) => {
                let (net, decl_width, range) = self.member_reference(env, object, field, false)?;
                ExprIR::Ref {
                    net,
                    decl_width,
                    range,
                    signed: false,
                }
            }
            ast::Expr::MemberChain(path) => match self.resolve_class_path(env, path)? {
                ClassPathTarget::Field(net) => {
                    let entry = env.net(&net)?;
                    ExprIR::Ref {
                        net: entry.base,
                        decl_width: entry.width,
                        range: full_range(entry.width),
                        signed: entry.is_signed,
                    }
                }
                ClassPathTarget::Handle { .. } => {
                    return Err("class handle paths cannot be read as values".into());
                }
            },
            ast::Expr::MethodCallChain {
                path,
                method,
                arguments,
            } => match self.resolve_class_path(env, path)? {
                ClassPathTarget::Handle { prefix, class, .. } => {
                    return self.class_method_expression_on(
                        prefix.as_str(),
                        class.as_str(),
                        class.as_str(),
                        method,
                        arguments,
                        env,
                    );
                }
                ClassPathTarget::Field(net) => {
                    let entry = env.net(&net)?;
                    if let Some(value) =
                        self.string_method_expression(entry, method, arguments, env)?
                    {
                        return Ok(value);
                    }
                    return Err(format!(
                        "method '{}' called on a non-object field path",
                        method
                    ));
                }
            },
            ast::Expr::Part(n, a, b) => {
                let entry = env.net(n)?;
                let ia = self.const_eval(a, env.params, env.module_name)?;
                let ib = self.const_eval(b, env.params, env.module_name)?;
                if entry.array.is_some() {
                    let (lsb, width) = entry.array_slice(ia, ib)?;
                    return Ok(ExprIR::Ref {
                        net: entry.base,
                        decl_width: entry.width,
                        signed: false,
                        range: Range::Span {
                            msb: lsb + width - 1,
                            lsb,
                        },
                    });
                }
                let (hi, lo) = if ia >= ib { (ia, ib) } else { (ib, ia) };
                let o1 = entry.offset(hi)?;
                let o2 = entry.offset(lo)?;
                ExprIR::Ref {
                    net: entry.base,
                    decl_width: entry.width,
                    signed: false,
                    range: Range::Span {
                        msb: o1.max(o2),
                        lsb: o1.min(o2),
                    },
                }
            }
            ast::Expr::IndexedPart {
                name,
                start,
                width,
                ascending,
            } => {
                let entry = env.net(name)?;
                if entry.msb < entry.lsb {
                    return self.reversed_indexed_part_expr(entry, start, width, *ascending, env);
                }
                if self.const_eval(start, env.params, env.module_name).is_ok() {
                    let (msb, lsb) =
                        self.indexed_part_range(name, start, width, *ascending, env)?;
                    ExprIR::Ref {
                        net: entry.base,
                        decl_width: entry.width,
                        signed: false,
                        range: Range::Span { msb, lsb },
                    }
                } else {
                    self.dynamic_indexed_part_expr(name, start, width, *ascending, env)?
                }
            }
            ast::Expr::Un(op, a) => {
                let a = self.elab_expr(a, env)?;
                match op {
                    ast::UnOpSrc::Not => ExprIR::Un {
                        op: UnOp::Not,
                        a: Box::new(a),
                    },
                    ast::UnOpSrc::AndR | ast::UnOpSrc::OrR | ast::UnOpSrc::XorR => ExprIR::Un {
                        op: match op {
                            ast::UnOpSrc::AndR => UnOp::AndR,
                            ast::UnOpSrc::OrR => UnOp::OrR,
                            _ => UnOp::XorR,
                        },
                        a: Box::new(a),
                    },
                    ast::UnOpSrc::NandR => ExprIR::Un {
                        op: UnOp::Not,
                        a: Box::new(ExprIR::Un {
                            op: UnOp::AndR,
                            a: Box::new(a),
                        }),
                    },
                    ast::UnOpSrc::NorR => ExprIR::Un {
                        op: UnOp::Not,
                        a: Box::new(ExprIR::Un {
                            op: UnOp::OrR,
                            a: Box::new(a),
                        }),
                    },
                    ast::UnOpSrc::XnorR => ExprIR::Un {
                        op: UnOp::Not,
                        a: Box::new(ExprIR::Un {
                            op: UnOp::XorR,
                            a: Box::new(a),
                        }),
                    },
                    ast::UnOpSrc::Plus => a,
                    ast::UnOpSrc::Minus => {
                        let w = a.width().max(1);
                        ExprIR::Bin {
                            op: BinOp::Sub,
                            a: Box::new(ExprIR::Const(LogicWord::zero(w))),
                            b: Box::new(a),
                        }
                    }
                    ast::UnOpSrc::LogNot => ExprIR::Un {
                        op: UnOp::Not,
                        a: Box::new(ExprIR::Un {
                            op: UnOp::OrR,
                            a: Box::new(a),
                        }),
                    },
                }
            }
            ast::Expr::Bin(op, a, b) => {
                let a = self.elab_expr(a, env)?;
                let b = self.elab_expr(b, env)?;
                let ir_op = match op {
                    ast::BinOpSrc::Add => BinOp::Add,
                    ast::BinOpSrc::Sub => BinOp::Sub,
                    ast::BinOpSrc::Mul => BinOp::Mul,
                    ast::BinOpSrc::Div => BinOp::Div,
                    ast::BinOpSrc::Mod => BinOp::Mod,
                    ast::BinOpSrc::And => BinOp::And,
                    ast::BinOpSrc::Or => BinOp::Or,
                    ast::BinOpSrc::Xor => BinOp::Xor,
                    ast::BinOpSrc::Shl => BinOp::Shl,
                    ast::BinOpSrc::Shr => BinOp::Shr,
                    ast::BinOpSrc::Eq => BinOp::Eq,
                    ast::BinOpSrc::Ne => BinOp::Ne,
                    ast::BinOpSrc::Lt => BinOp::Lt,
                    ast::BinOpSrc::Le => BinOp::Le,
                    ast::BinOpSrc::Gt => BinOp::Gt,
                    ast::BinOpSrc::Ge => BinOp::Ge,
                    ast::BinOpSrc::LogAnd => BinOp::And,
                    ast::BinOpSrc::LogOr => BinOp::Or,
                    ast::BinOpSrc::CaseEq => BinOp::CaseEq,
                    ast::BinOpSrc::CaseNe => BinOp::CaseNe,
                };
                match op {
                    ast::BinOpSrc::LogAnd => ExprIR::Bin {
                        op: BinOp::And,
                        a: Box::new(ExprIR::Un {
                            op: UnOp::OrR,
                            a: Box::new(a),
                        }),
                        b: Box::new(ExprIR::Un {
                            op: UnOp::OrR,
                            a: Box::new(b),
                        }),
                    },
                    ast::BinOpSrc::LogOr => ExprIR::Bin {
                        op: BinOp::Or,
                        a: Box::new(ExprIR::Un {
                            op: UnOp::OrR,
                            a: Box::new(a),
                        }),
                        b: Box::new(ExprIR::Un {
                            op: UnOp::OrR,
                            a: Box::new(b),
                        }),
                    },
                    _ => ExprIR::Bin {
                        op: ir_op,
                        a: Box::new(a),
                        b: Box::new(b),
                    },
                }
            }
            ast::Expr::Cond(c, t, f) => ExprIR::Cond {
                c: Box::new(self.elab_expr(c, env)?),
                t: Box::new(self.elab_expr(t, env)?),
                f: Box::new(self.elab_expr(f, env)?),
            },
            ast::Expr::Concat(parts) => ExprIR::Concat(
                parts
                    .iter()
                    .map(|p| self.elab_expr(p, env))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::Replicate(count, inner) => {
                let n = self.const_eval(count, env.params, env.module_name)?;
                if !(0..=4096).contains(&n) {
                    return Err(format!("replication count {} out of range", n));
                }
                let inner = self.elab_expr(inner, env)?;
                ExprIR::Repeat {
                    times: n as usize,
                    e: Box::new(inner),
                }
            }
            ast::Expr::Slice { value, msb, lsb } => {
                let inner = self.elab_expr(value, env)?;
                let msb = self.const_eval(msb, env.params, env.module_name)?;
                let lsb = self.const_eval(lsb, env.params, env.module_name)?;
                if msb < lsb || lsb < 0 || msb as usize >= inner.width() {
                    return Err(format!(
                        "invalid part-select [{}:{}] for expression width {}",
                        msb,
                        lsb,
                        inner.width()
                    ));
                }
                ExprIR::Index {
                    e: Box::new(inner),
                    msb: msb as usize,
                    lsb: lsb as usize,
                }
            }
            ast::Expr::Resize { width, value } => {
                if *width == 0 || *width > 4096 {
                    return Err(format!("invalid assignment width {}", width));
                }
                let inner = self.elab_expr(value, env)?;
                if inner.width() > *width {
                    ExprIR::Index {
                        e: Box::new(inner),
                        msb: width - 1,
                        lsb: 0,
                    }
                } else if inner.width() < *width {
                    ExprIR::Concat(vec![
                        ExprIR::Const(LogicWord::zero(width - inner.width())),
                        inner,
                    ])
                } else {
                    inner
                }
            }
            ast::Expr::WildcardMatch {
                kind,
                selector,
                label,
            } => {
                let selector = self.elab_expr(selector, env)?;
                let label = self.elab_expr(label, env)?;
                wildcard_label_condition(selector, &label, *kind)
            }
            ast::Expr::Call(name, arguments) if name == "$signed" || name == "$unsigned" => {
                if arguments.len() != 1 {
                    return Err(format!("'{}' expects exactly one argument", name));
                }
                let inner = self.elab_expr(&arguments[0], env)?;
                ExprIR::Signed {
                    e: Box::new(inner),
                    signed: name == "$signed",
                }
            }
            ast::Expr::Call(name, arguments) if name == "$sformatf" => {
                self.elab_sformatf(arguments, env)?
            }
            ast::Expr::Call(name, _) => {
                let arguments = match e {
                    ast::Expr::Call(_, arguments) => arguments,
                    _ => unreachable!(),
                };
                return self.inline_function_call(name, arguments, env);
            }
            ast::Expr::MethodCall {
                object,
                method,
                arguments,
            } => {
                if object == "this" || object == "super" || env.class_handles.contains_key(object) {
                    return self.class_method_expression(object, method, arguments, env);
                }
                if let Ok(entry) = env.net(object) {
                    if let Some(value) =
                        self.string_method_expression(entry, method, arguments, env)?
                    {
                        return Ok(value);
                    }
                }
                if method != "size" || !arguments.is_empty() {
                    return Err(format!(
                        "unsupported dynamic-array method '{object}.{method}'"
                    ));
                }
                let entry = env.net(object)?;
                let array = entry
                    .array
                    .as_ref()
                    .ok_or_else(|| format!("'{object}' is not a dynamic array"))?;
                if !array.dynamic {
                    return Err(format!("'{object}' is not a dynamic array"));
                }
                ExprIR::Ref {
                    net: array.dynamic_size_base,
                    decl_width: 9,
                    range: full_range(9),
                    signed: false,
                }
            }
            ast::Expr::New(_) => {
                return Err("dynamic-array allocation is supported only as an assignment".into());
            }
            ast::Expr::NewClass(_) => {
                return Err(
                    "class construction is supported only as an assignment to a handle".into(),
                );
            }
        })
    }
}
