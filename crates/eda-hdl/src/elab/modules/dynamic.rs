//! Elaborator module-domain implementation.

use super::super::*;

impl<'a> Elaborator<'a> {
    pub(super) fn elab_sens(&self, sens: &ast::Sens, env: &Env) -> Result<SensIR, String> {
        Ok(match sens {
            ast::Sens::Comb => SensIR::Comb,
            ast::Sens::Any(e) => SensIR::Any(self.single_net(e, env)?),
            ast::Sens::Posedge(e) => SensIR::Posedge(self.single_net(e, env)?),
            ast::Sens::Negedge(e) => SensIR::Negedge(self.single_net(e, env)?),
            ast::Sens::List(v) => SensIR::List(
                v.iter()
                    .map(|s| self.elab_sens(s, env))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        })
    }

    pub(super) fn single_net(&self, e: &ast::Expr, env: &Env) -> Result<usize, String> {
        let ir = self.elab_expr(e, env)?;
        match ir {
            ExprIR::Ref {
                net,
                decl_width,
                range,
                signed: _,
            } => {
                if decl_width == 1 {
                    Ok(net)
                } else if range.width() == 1 {
                    Ok(net
                        + match range {
                            Range::Bit(b) => b,
                            Range::Span { lsb, .. } => lsb,
                        })
                } else {
                    Err("edge-sensitive expression must be 1 bit wide".into())
                }
            }
            _ => Err("edge-sensitive expression must be a net".into()),
        }
    }

    pub(super) fn dynamic_array_entry(
        &self,
        name: &str,
        env: &Env,
    ) -> Result<(NetEntry, NetArray), String> {
        let entry = env.net(name)?;
        let array = entry
            .array
            .clone()
            .ok_or_else(|| format!("'{name}' is not a dynamic array"))?;
        if !array.dynamic {
            return Err(format!("'{name}' is not a dynamic array"));
        }
        Ok((entry.clone(), array))
    }

    pub(in crate::elab) fn dynamic_array_new(
        &self,
        name: &str,
        size: &ast::Expr,
        nonblocking: bool,
        env: &Env,
    ) -> Result<StmtIR, String> {
        let (entry, array) = self.dynamic_array_entry(name, env)?;
        let constant_size = self
            .const_eval(size, env.params, env.module_name)
            .map_err(|_| {
                format!(
                    "module '{}': dynamic array '{}' requires a constant size in this bounded subset",
                    env.module_name, name
                )
            })?;
        if !(0..=256).contains(&constant_size) {
            return Err(format!(
                "module '{}': dynamic array '{}' size must be between 0 and 256 in this bounded subset, got {}",
                env.module_name, name, constant_size
            ));
        }
        let mut size_value = self.elab_expr(size, env)?;
        size_value = if size_value.width() < 9 {
            ExprIR::Concat(vec![
                ExprIR::Const(LogicWord::zero(9 - size_value.width())),
                size_value,
            ])
        } else if size_value.width() > 9 {
            ExprIR::Index {
                e: Box::new(size_value),
                msb: 8,
                lsb: 0,
            }
        } else {
            size_value
        };
        let array_lvalue = LValue {
            net: entry.base,
            decl_width: entry.width,
            range: full_range(entry.width),
        };
        let size_lvalue = LValue {
            net: array.dynamic_size_base,
            decl_width: 9,
            range: full_range(9),
        };
        let assignment = |lv: LValue, value: ExprIR| {
            if nonblocking {
                StmtIR::NBl { lv, e: value }
            } else {
                StmtIR::Bl { lv, e: value }
            }
        };
        Ok(StmtIR::Block(vec![
            assignment(array_lvalue, ExprIR::Const(LogicWord::zero(entry.width))),
            assignment(size_lvalue, size_value),
        ]))
    }

    pub(in crate::elab) fn dynamic_array_method(
        &self,
        object: &str,
        method: &str,
        arguments: &[ast::Expr],
        env: &Env,
    ) -> Result<StmtIR, String> {
        if method != "delete" || !arguments.is_empty() {
            return Err(format!(
                "unsupported dynamic-array method '{object}.{method}'"
            ));
        }
        let (entry, array) = self.dynamic_array_entry(object, env)?;
        Ok(StmtIR::Block(vec![
            StmtIR::Bl {
                lv: LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: full_range(entry.width),
                },
                e: ExprIR::Const(LogicWord::zero(entry.width)),
            },
            StmtIR::Bl {
                lv: LValue {
                    net: array.dynamic_size_base,
                    decl_width: 9,
                    range: full_range(9),
                },
                e: ExprIR::Const(LogicWord::zero(9)),
            },
        ]))
    }
}
