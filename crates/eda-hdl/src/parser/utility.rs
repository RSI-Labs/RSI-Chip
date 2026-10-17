//! Parser token/stream and low-level syntax helpers.

use super::*;

impl Parser {
    pub(super) fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    pub(super) fn peek_str(&self) -> &str {
        match self.peek() {
            Tok::Ident(s) => s,
            Tok::Punct(p) => p,
            _ => "",
        }
    }

    pub(super) fn line(&self) -> usize {
        self.toks[self.pos].line
    }

    pub(super) fn at_net_type(&self) -> bool {
        let type_position = if self.at_kw("signed") || self.at_kw("unsigned") {
            self.pos + 1
        } else {
            self.pos
        };
        matches!(
            self.toks.get(type_position).map(|token| &token.tok),
            Some(Tok::Ident(name)) if NET_TYPES.contains(&name.as_str())
        )
    }

    pub(super) fn at_custom_type(&self) -> bool {
        let Tok::Ident(name) = &self.toks[self.pos].tok else {
            return false;
        };
        if is_reserved(name) {
            return false;
        }
        if self.known_classes.contains(name)
            && matches!(
                self.toks.get(self.pos + 1).map(|token| &token.tok),
                Some(Tok::Punct("#"))
            )
            && matches!(
                self.toks.get(self.pos + 2).map(|token| &token.tok),
                Some(Tok::Punct("("))
            )
        {
            return true;
        }
        // Package-qualified custom type: `pkg::Type name;`.
        if matches!(
            self.toks.get(self.pos + 1).map(|token| &token.tok),
            Some(Tok::Punct("::"))
        ) {
            return matches!(
                self.toks.get(self.pos + 2).map(|token| &token.tok),
                Some(Tok::Ident(_))
            ) && matches!(
                self.toks.get(self.pos + 3).map(|token| &token.tok),
                Some(Tok::Ident(_))
            );
        }
        let Some(next) = self.toks.get(self.pos + 1) else {
            return false;
        };
        let Some(after) = self.toks.get(self.pos + 2) else {
            return false;
        };
        matches!(next.tok, Tok::Ident(_))
            && matches!(
                after.tok,
                Tok::Punct(";")
                    | Tok::Punct(",")
                    | Tok::Punct("=")
                    | Tok::Punct("[")
                    | Tok::Punct(")")
            )
    }

    pub(super) fn parse_custom_type_name(&mut self) -> Result<String, String> {
        let first = self.ident_any()?;
        if self.known_classes.contains(&first)
            && matches!(
                self.toks.get(self.pos).map(|token| &token.tok),
                Some(Tok::Punct("#"))
            )
        {
            return self.parse_class_specialization_name(first);
        }
        if self.eat_punct("::") {
            let second = self.ident_any()?;
            Ok(format!("{first}::{second}"))
        } else {
            Ok(first)
        }
    }

    pub(super) fn parse_class_specialization_name(
        &mut self,
        base_name: String,
    ) -> Result<String, String> {
        self.expect_punct("#")?;
        self.expect_punct("(")?;
        let mut parameter_names = Vec::new();
        let mut values = Vec::new();
        while !self.at_punct(")") {
            let parameter_name = if self.eat_punct(".") {
                let name = self.ident()?;
                self.expect_punct("(")?;
                Some(name)
            } else {
                None
            };
            let expression = self.parse_expr()?;
            let value = if self.allow_symbolic_class_actual {
                symbolic_class_actual(&expression)
            } else {
                numeric_class_actual(&expression).map(ClassParameterValue::Constant)
            };
            let Some(value) = value else {
                return Err(format!(
                    "class specialization actual must be an integer constant near line {}",
                    self.line()
                ));
            };
            if parameter_name.is_some() {
                self.expect_punct(")")?;
            }
            parameter_names.push(parameter_name);
            values.push(value);
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct(")")?;
        let canonical_name = class_specialization_name(&base_name, &values);
        self.class_specializations
            .push(crate::ast::ClassSpecialization {
                base_name,
                canonical_name: canonical_name.clone(),
                parameter_names,
                values,
            });
        Ok(canonical_name)
    }

    pub(super) fn at_instance_header(&self) -> bool {
        matches!(self.peek(), Tok::Ident(name) if !is_reserved(name))
            && matches!(
                self.toks.get(self.pos + 1).map(|token| &token.tok),
                Some(Tok::Ident(name)) if !is_reserved(name)
            )
            && matches!(
                self.toks.get(self.pos + 2).map(|token| &token.tok),
                Some(Tok::Punct("(")) | Some(Tok::Punct("["))
            )
    }

    pub(super) fn parse_net_type(&mut self) -> Option<NetTypeInfo> {
        if !self.at_net_type() {
            return None;
        }
        let leading_signed = self.eat_kw("signed");
        let leading_unsigned = self.eat_kw("unsigned");
        let name = match self.peek() {
            Tok::Ident(name) => name.clone(),
            _ => return None,
        };
        self.bump();
        let is_net = matches!(name.as_str(), "wire" | "tri");
        let name = if is_net {
            match self.peek() {
                Tok::Ident(data_type)
                    if matches!(
                        data_type.as_str(),
                        "reg"
                            | "logic"
                            | "bit"
                            | "byte"
                            | "shortint"
                            | "int"
                            | "integer"
                            | "longint"
                    ) =>
                {
                    let data_type = data_type.clone();
                    self.bump();
                    data_type
                }
                _ => name,
            }
        } else {
            name
        };
        let trailing_signed = self.eat_kw("signed");
        let trailing_unsigned = self.eat_kw("unsigned");
        let is_signed =
            (leading_signed || trailing_signed) && !(leading_unsigned || trailing_unsigned);
        let info = match name.as_str() {
            "string" => NetTypeInfo {
                is_variable: true,
                is_signed: false,
                default_width: STRING_WIDTH,
            },
            "wire" | "tri" => NetTypeInfo {
                is_variable: false,
                is_signed,
                default_width: 1,
            },
            "bit" | "reg" | "logic" => NetTypeInfo {
                is_variable: true,
                is_signed,
                default_width: 1,
            },
            "byte" => NetTypeInfo {
                is_variable: true,
                is_signed,
                default_width: 8,
            },
            "shortint" => NetTypeInfo {
                is_variable: true,
                is_signed,
                default_width: 16,
            },
            "int" | "integer" => NetTypeInfo {
                is_variable: true,
                is_signed,
                default_width: 32,
            },
            "longint" => NetTypeInfo {
                is_variable: true,
                is_signed,
                default_width: 64,
            },
            _ => return None,
        };
        Some(if is_net {
            NetTypeInfo {
                is_variable: false,
                ..info
            }
        } else {
            info
        })
    }

    pub(super) fn net_range(
        info: Option<NetTypeInfo>,
        explicit: Option<PackedRange>,
    ) -> Option<PackedRange> {
        explicit.or_else(|| {
            let width = info?.default_width;
            (width > 1).then(|| default_range(width))
        })
    }

    pub(super) fn at_eof(&self) -> bool {
        matches!(self.peek(), Tok::Eof)
    }

    pub(super) fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    pub(super) fn at_punct(&self, p: &str) -> bool {
        matches!(self.peek(), Tok::Punct(q) if *q == p)
    }

    pub(super) fn eat_punct(&mut self, p: &str) -> bool {
        if self.at_punct(p) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(super) fn expect_punct(&mut self, p: &str) -> Result<(), String> {
        if self.eat_punct(p) {
            Ok(())
        } else {
            Err(format!(
                "line {}: expected '{}', found '{}'",
                self.line(),
                p,
                self.peek_str()
            ))
        }
    }

    pub(super) fn at_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == kw)
    }

    pub(super) fn eat_kw(&mut self, kw: &str) -> bool {
        if self.at_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(super) fn expect_kw(&mut self, kw: &str) -> Result<(), String> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            Err(format!(
                "line {}: expected '{}', found '{}'",
                self.line(),
                kw,
                self.peek_str()
            ))
        }
    }

    pub(super) fn ident(&mut self) -> Result<String, String> {
        match self.peek().clone() {
            Tok::Ident(s) if !is_reserved(&s) => {
                self.bump();
                Ok(s)
            }
            other => Err(format!(
                "line {}: expected identifier, found {:?}",
                self.line(),
                other
            )),
        }
    }

    pub(super) fn ident_any(&mut self) -> Result<String, String> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                Ok(s)
            }
            other => Err(format!(
                "line {}: expected identifier, found {:?}",
                self.line(),
                other
            )),
        }
    }

    // ---------- module ----------
}
