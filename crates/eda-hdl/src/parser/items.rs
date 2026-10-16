//! Assertions, module items, generate constructs, delays, and sensitivity.

use super::*;

impl Parser {
    pub(super) fn parse_assert_property(
        &mut self,
        kind: PropertyKind,
        line: usize,
        label: Option<String>,
    ) -> Result<Item, String> {
        self.expect_punct("(")?;
        let clock = self.parse_sens()?;
        let disable = if self.eat_kw("disable") {
            self.expect_kw("iff")?;
            self.expect_punct("(")?;
            let condition = self.parse_expr()?;
            self.expect_punct(")")?;
            condition
        } else {
            Expr::Num {
                width: Some(1),
                word: LogicWord::zero(1),
                orig: "0".to_string(),
            }
        };
        let antecedent = self.parse_expr()?;
        let overlapping = if self.eat_punct("|->") {
            true
        } else if self.eat_punct("|=>") {
            false
        } else {
            return Err(format!(
                "line {}: concurrent assertions require '|->' or '|=>' implication",
                line
            ));
        };
        let delay = if self.eat_punct("##") {
            if !overlapping {
                return Err(format!(
                    "line {}: sequence delay is supported after '|->', not '|=>'",
                    line
                ));
            }
            self.expect_punct("[")?;
            let start = self.parse_expr()?;
            let delay = if self.eat_punct(":") {
                let end = self.parse_expr()?;
                PropertyDelay::Range(start, end)
            } else {
                PropertyDelay::Fixed(start)
            };
            self.expect_punct("]")?;
            delay
        } else {
            PropertyDelay::Fixed(Expr::Num {
                width: Some(32),
                word: LogicWord::from_u64(32, 0),
                orig: "0".to_string(),
            })
        };
        let consequent = self.parse_expr()?;
        let composition = if self.at_punct("##") {
            self.bump();
            self.expect_punct("[")?;
            let delay = self.parse_expr()?;
            if self.at_punct(":") {
                return Err(format!(
                    "line {}: sequence composition supports fixed delay only",
                    line
                ));
            }
            self.expect_punct("]")?;
            let right = self.parse_expr()?;
            Some(Box::new(PropertyComposition {
                op: PropertyCompositionOp::And,
                right,
                right_delay: PropertyDelay::Fixed(delay),
            }))
        } else if matches!(self.peek(), Tok::Ident(word) if word == "and" || word == "or") {
            let op = if matches!(self.peek(), Tok::Ident(word) if word == "and") {
                self.bump();
                PropertyCompositionOp::And
            } else {
                self.bump();
                PropertyCompositionOp::Or
            };
            let right_delay = if self.at_punct("##") {
                self.bump();
                self.expect_punct("[")?;
                let delay = self.parse_expr()?;
                if self.at_punct(":") {
                    return Err(format!(
                        "line {}: sequence composition supports fixed delay only",
                        line
                    ));
                }
                self.expect_punct("]")?;
                PropertyDelay::Fixed(delay)
            } else {
                PropertyDelay::Fixed(Expr::Num {
                    width: Some(32),
                    word: LogicWord::from_u64(32, 0),
                    orig: "0".to_string(),
                })
            };
            let right = self.parse_expr()?;
            Some(Box::new(PropertyComposition {
                op,
                right,
                right_delay,
            }))
        } else {
            None
        };
        let repetition = if self.at_punct("[")
            && matches!(
                self.toks.get(self.pos + 1).map(|token| &token.tok),
                Some(Tok::Punct("*"))
            ) {
            self.bump();
            self.bump();
            let count = self.parse_expr()?;
            let repetition = if self.eat_punct(":") {
                let end = self.parse_expr()?;
                PropertyRepetition::Range(count, end)
            } else {
                PropertyRepetition::Count(count)
            };
            self.expect_punct("]")?;
            Some(Box::new(repetition))
        } else if self.at_punct("[")
            && matches!(
                self.toks.get(self.pos + 1).map(|token| &token.tok),
                Some(Tok::Punct("->"))
            )
        {
            self.bump();
            self.bump();
            let count = self.parse_expr()?;
            let repetition = if self.eat_punct(":") {
                let end = self.parse_expr()?;
                PropertyRepetition::GotoRange(count, end)
            } else {
                PropertyRepetition::Goto(count)
            };
            self.expect_punct("]")?;
            Some(Box::new(repetition))
        } else if self.at_punct("[")
            && matches!(
                self.toks.get(self.pos + 1).map(|token| &token.tok),
                Some(Tok::Punct("="))
            )
        {
            self.bump();
            self.bump();
            let count = self.parse_expr()?;
            let repetition = if self.eat_punct(":") {
                let end = self.parse_expr()?;
                PropertyRepetition::NonconsecutiveRange(count, end)
            } else {
                PropertyRepetition::Nonconsecutive(count)
            };
            self.expect_punct("]")?;
            Some(Box::new(repetition))
        } else {
            None
        };
        if composition.is_some() && repetition.is_some() {
            return Err(format!(
                "line {}: sequence composition does not yet compose with repetition",
                line
            ));
        }
        self.expect_punct(")")?;
        let mut message = None;
        if kind == PropertyKind::Cover && self.at_kw("else") {
            return Err(format!(
                "line {}: cover properties do not support failure actions",
                line
            ));
        }
        if self.eat_kw("else") {
            match self.peek().clone() {
                Tok::Ident(name) if name == "$error" || name == "$fatal" => {
                    self.bump();
                    self.expect_punct("(")?;
                    match self.peek().clone() {
                        Tok::Str(text) => {
                            self.bump();
                            message = Some(text);
                        }
                        other => {
                            return Err(format!(
                                "line {}: property message must be a string literal, found {:?}",
                                line, other
                            ));
                        }
                    }
                    self.expect_punct(")")?;
                }
                other => {
                    return Err(format!(
                        "line {}: concurrent assertions support $error/$fatal actions only, found {:?}",
                        line, other
                    ));
                }
            }
        }
        self.expect_punct(";")?;
        Ok(Item::AssertProperty {
            kind,
            label,
            clock,
            disable,
            antecedent,
            consequent,
            composition,
            overlapping,
            delay: Box::new(delay),
            repetition,
            message,
        })
    }

    pub(super) fn parse_opt_range(&mut self) -> Result<Option<PackedRange>, String> {
        if self.eat_punct("[") {
            let msb = self.parse_expr()?;
            self.expect_punct(":")?;
            let lsb = self.parse_expr()?;
            self.expect_punct("]")?;
            Ok(Some((Box::new(msb), Box::new(lsb))))
        } else {
            Ok(None)
        }
    }

    pub(super) fn parse_unpacked_ranges(
        &mut self,
        context: &str,
    ) -> Result<Vec<Option<PackedRange>>, String> {
        let mut ranges = Vec::new();
        while self.at_punct("[") {
            let line = self.line();
            let Some(range) = self.parse_opt_range()? else {
                return Err(format!(
                    "line {}: unpacked {context} range cannot be empty",
                    line
                ));
            };
            ranges.push(Some(range));
        }
        Ok(ranges)
    }

    // ---------- items ----------

    pub(super) fn parse_item(&mut self) -> Result<Item, String> {
        let line = self.line();
        match self.peek().clone() {
            Tok::Ident(s)
                if s == "virtual"
                    && matches!(
                        self.toks.get(self.pos + 1).map(|token| &token.tok),
                        Some(Tok::Ident(word)) if word == "class"
                    ) =>
            {
                self.bump();
                self.parse_class(line, true)
            }
            Tok::Ident(s) if s == "class" => self.parse_class(line, false),
            Tok::Ident(label)
                if matches!(
                    self.toks.get(self.pos + 1).map(|token| &token.tok),
                    Some(Tok::Punct(":"))
                ) && matches!(self.toks.get(self.pos + 2).map(|token| &token.tok), Some(Tok::Ident(word)) if word == "cover")
                    && matches!(self.toks.get(self.pos + 3).map(|token| &token.tok), Some(Tok::Ident(word)) if word == "property") =>
            {
                self.bump();
                self.expect_punct(":")?;
                self.expect_kw("cover")?;
                self.expect_kw("property")?;
                self.parse_assert_property(PropertyKind::Cover, line, Some(label))
            }
            Tok::Ident(s)
                if matches!(s.as_str(), "assert" | "assume" | "cover")
                    && matches!(self.toks.get(self.pos + 1).map(|token| &token.tok), Some(Tok::Ident(property)) if property == "property") =>
            {
                let kind = match s.as_str() {
                    "assume" => PropertyKind::Assume,
                    "cover" => PropertyKind::Cover,
                    _ => PropertyKind::Assert,
                };
                self.bump();
                self.bump();
                self.parse_assert_property(kind, line, None)
            }
            Tok::Ident(s) if s == "modport" => {
                self.bump();
                self.parse_modport(line)
            }
            Tok::Ident(s) if DIRS.contains(&s.as_str()) => {
                let dir = match s.as_str() {
                    "input" => DirSrc::In,
                    "output" => DirSrc::Out,
                    _ => DirSrc::InOut,
                };
                self.bump();
                let is_string = self.peek_str() == "string";
                let net_type = self.parse_net_type();
                let custom_type = if net_type.is_none() && self.at_custom_type() {
                    Some(self.parse_custom_type_name()?)
                } else {
                    None
                };
                let explicit_range = self.parse_opt_range()?;
                let range = if custom_type.is_some() {
                    explicit_range
                } else {
                    Self::net_range(net_type, explicit_range)
                };
                let is_reg = net_type.is_some_and(|info| info.is_variable);
                let is_signed = net_type.is_some_and(|info| info.is_signed);
                let mut names = vec![self.ident()?];
                let array_ranges = self.parse_unpacked_ranges("port")?;
                if !array_ranges.is_empty() && self.at_punct(",") {
                    return Err(format!(
                        "line {}: declare each unpacked array port separately",
                        line
                    ));
                }
                while self.eat_punct(",") {
                    names.push(self.ident()?);
                }
                self.expect_punct(";")?;
                Ok(Item::Port {
                    dir,
                    is_reg,
                    is_signed,
                    is_virtual: false,
                    interface_name: None,
                    modport_name: None,
                    range,
                    type_name: if is_string {
                        Some("string".to_string())
                    } else {
                        custom_type
                    },
                    array_ranges,
                    names,
                })
            }
            Tok::Ident(_) if self.at_net_type() => {
                let is_string = self.peek_str() == "string";
                let net_type = self.parse_net_type().expect("checked net type");
                let is_reg = net_type.is_variable;
                let explicit_range = self.parse_opt_range()?;
                let range = Self::net_range(Some(net_type), explicit_range);
                let mut names = Vec::new();
                let mut declared_array_ranges = Vec::new();
                loop {
                    let n = self.ident()?;
                    let mut element_array_ranges = Vec::new();
                    while self.at_punct("[") {
                        let range = self.parse_opt_range()?;
                        element_array_ranges.push(range);
                    }
                    let init = if self.eat_punct("=") {
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    if !element_array_ranges.is_empty() && init.is_some() {
                        return Err(format!(
                            "line {}: unpacked module net arrays do not support initializers",
                            line
                        ));
                    }
                    names.push((n, init));
                    if !element_array_ranges.is_empty() {
                        if !declared_array_ranges.is_empty() {
                            return Err(format!(
                                "line {}: declare each unpacked module net array separately",
                                line
                            ));
                        }
                        declared_array_ranges = element_array_ranges;
                    }
                    if !declared_array_ranges.is_empty() && self.at_punct(",") {
                        return Err(format!(
                            "line {}: declare each unpacked module net array separately",
                            line
                        ));
                    }
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_punct(";")?;
                Ok(Item::Net {
                    is_reg,
                    is_signed: net_type.is_signed,
                    range,
                    array_ranges: declared_array_ranges,
                    type_name: is_string.then(|| "string".to_string()),
                    names,
                })
            }
            Tok::Ident(_) if self.at_custom_type() && !self.at_instance_header() => {
                let type_name = self.parse_custom_type_name()?;
                let range = self.parse_opt_range()?;
                let mut names = Vec::new();
                loop {
                    let n = self.ident()?;
                    let init = if self.eat_punct("=") {
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    names.push((n, init));
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_punct(";")?;
                Ok(Item::Net {
                    is_reg: true,
                    is_signed: false,
                    type_name: Some(type_name),
                    range,
                    array_ranges: Vec::new(),
                    names,
                })
            }
            Tok::Ident(s) if s == "parameter" || s == "localparam" => {
                self.bump();
                // optional type like "integer" — skip if present
                if self.at_kw("integer") {
                    self.bump();
                }
                let mut params = Vec::new();
                loop {
                    let n = self.ident()?;
                    self.expect_punct("=")?;
                    let v = self.parse_expr()?;
                    params.push((n, v));
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_punct(";")?;
                Ok(Item::Params(params))
            }
            Tok::Ident(s) if s == "import" => {
                let imports = self.parse_import()?;
                Ok(Item::Imports(imports))
            }
            Tok::Ident(s) if s == "genvar" => {
                let names = self.parse_genvar_declaration()?;
                Ok(Item::Genvar(names))
            }
            Tok::Ident(s) if s == "typedef" => {
                let _ = s;
                self.parse_typedef()
            }
            Tok::Ident(s) if s == "function" => {
                let _ = s;
                self.parse_function()
            }
            Tok::Ident(s) if s == "generate" => {
                self.bump();
                let constructs = self.parse_generate_constructs()?;
                self.expect_kw("endgenerate")?;
                let _ = self.eat_punct(";");
                Ok(Item::Generate(constructs))
            }
            Tok::Ident(s) if s == "assign" => {
                self.bump();
                let delay = self.parse_opt_delay()?;
                let lv = self.parse_expr()?;
                self.expect_punct("=")?;
                let e = self.parse_expr()?;
                self.expect_punct(";")?;
                Ok(Item::Assign { delay, lv, e })
            }
            Tok::Ident(s) if s == "always" || s == "always_comb" || s == "always_ff" => {
                self.bump();
                let sens = if s == "always_comb" {
                    Sens::Comb
                } else if self.at_punct("@") {
                    self.parse_sens()?
                } else if self.at_punct("#") {
                    // `always #5 clk = ~clk;` — a forever-delay generator
                    let ns = self.parse_opt_delay()?;
                    let body = self.parse_stmt()?;
                    return Ok(Item::Always {
                        sens: Sens::Comb,
                        body: Stmt::Forever(Box::new(Stmt::Delay {
                            ns,
                            body: Box::new(body),
                        })),
                    });
                } else {
                    return Err(format!("line {}: expected '@' after always", line));
                };
                let body = self.parse_stmt()?;
                Ok(Item::Always { sens, body })
            }
            Tok::Ident(s) if s == "initial" => {
                self.bump();
                let body = self.parse_stmt()?;
                Ok(Item::Initial { body })
            }
            Tok::Ident(s) if GATES.contains(&s.as_str()) => {
                let kind = match s.as_str() {
                    "and" => GateKind::And,
                    "nand" => GateKind::Nand,
                    "or" => GateKind::Or,
                    "nor" => GateKind::Nor,
                    "xor" => GateKind::Xor,
                    "xnor" => GateKind::Xnor,
                    "buf" => GateKind::Buf,
                    _ => GateKind::Not,
                };
                self.bump();
                let name = if matches!(self.peek(), Tok::Ident(_)) && !self.at_punct("(") {
                    self.ident()?
                } else {
                    String::new()
                };
                self.expect_punct("(")?;
                let mut args = Vec::new();
                if !self.at_punct(")") {
                    loop {
                        args.push(self.parse_expr()?);
                        if !self.eat_punct(",") {
                            break;
                        }
                    }
                }
                self.expect_punct(")")?;
                self.expect_punct(";")?;
                Ok(Item::Gate { kind, name, args })
            }
            Tok::Ident(_) => {
                let module = self.ident()?;
                let mut params = Vec::new();
                if self.eat_punct("#") {
                    self.expect_punct("(")?;
                    if self.at_punct(".") {
                        loop {
                            self.expect_punct(".")?;
                            let n = self.ident()?;
                            self.expect_punct("(")?;
                            let v = self.parse_expr()?;
                            self.expect_punct(")")?;
                            params.push((Some(n), v));
                            if !self.eat_punct(",") {
                                break;
                            }
                        }
                    } else {
                        loop {
                            params.push((None, self.parse_expr()?));
                            if !self.eat_punct(",") {
                                break;
                            }
                        }
                    }
                    self.expect_punct(")")?;
                }
                let name = self.ident()?;
                let mut array_ranges = Vec::new();
                while self.at_punct("[") {
                    let Some(range) = self.parse_opt_range()? else {
                        return Err(format!(
                            "line {}: instance array range cannot be empty",
                            line
                        ));
                    };
                    array_ranges.push(range);
                }
                self.expect_punct("(")?;
                let mut conns = Vec::new();
                if !self.at_punct(")") {
                    loop {
                        if self.eat_punct(".") {
                            let pname = self.ident()?;
                            let e = if self.eat_punct("(") {
                                let e = if self.at_punct(")") {
                                    None
                                } else {
                                    Some(self.parse_expr()?)
                                };
                                self.expect_punct(")")?;
                                e
                            } else {
                                return Err(format!(
                                    "line {}: named port connection requires parentheses",
                                    line
                                ));
                            };
                            conns.push(Conn::Named(pname, e));
                        } else {
                            conns.push(Conn::Pos(Some(self.parse_expr()?)));
                        }
                        if !self.eat_punct(",") {
                            break;
                        }
                    }
                }
                self.expect_punct(")")?;
                self.expect_punct(";")?;
                Ok(Item::Inst {
                    module,
                    name,
                    array_ranges,
                    params,
                    conns,
                })
            }
            other => Err(format!(
                "line {}: unexpected token {:?} in module body",
                line, other
            )),
        }
    }

    pub(super) fn parse_generate_constructs(&mut self) -> Result<Vec<GenerateConstruct>, String> {
        let mut out = Vec::new();
        while !self.at_kw("endgenerate") {
            if self.at_eof() {
                return Err("generate region is missing 'endgenerate'".into());
            }
            if self.at_kw("genvar") {
                let names = self.parse_genvar_declaration()?;
                out.push(GenerateConstruct::Genvar(names));
            } else if self.eat_kw("case") {
                self.expect_punct("(")?;
                let selector = self.parse_expr()?;
                self.expect_punct(")")?;
                let mut arms = Vec::new();
                let mut default = None;
                while !self.at_kw("endcase") {
                    if self.at_eof() {
                        return Err("generate case is missing 'endcase'".into());
                    }
                    if self.eat_kw("default") {
                        self.expect_punct(":")?;
                        default = Some(self.parse_generate_block()?);
                    } else {
                        let mut labels = vec![self.parse_expr()?];
                        while self.eat_punct(",") {
                            labels.push(self.parse_expr()?);
                        }
                        self.expect_punct(":")?;
                        let body = self.parse_generate_block()?;
                        arms.push(GenerateCaseArm { labels, body });
                    }
                }
                self.expect_kw("endcase")?;
                out.push(GenerateConstruct::Case {
                    selector,
                    arms,
                    default,
                });
            } else if self.eat_kw("if") {
                self.expect_punct("(")?;
                let cond = self.parse_expr()?;
                self.expect_punct(")")?;
                let then = self.parse_generate_block()?;
                let otherwise = if self.eat_kw("else") {
                    Some(self.parse_generate_block()?)
                } else {
                    None
                };
                out.push(GenerateConstruct::If {
                    cond,
                    then,
                    otherwise,
                });
            } else if self.eat_kw("for") {
                self.expect_punct("(")?;
                let _ = self.eat_kw("genvar");
                let var = self.ident()?;
                self.expect_punct("=")?;
                let start = self.parse_expr()?;
                self.expect_punct(";")?;
                let cond = self.parse_expr()?;
                self.expect_punct(";")?;
                let assignment_var = self.ident()?;
                if assignment_var != var {
                    return Err(format!(
                        "generate loop increment must assign '{var}', found '{assignment_var}'"
                    ));
                }
                self.expect_punct("=")?;
                let step = self.parse_expr()?;
                self.expect_punct(")")?;
                let body = self.parse_generate_block()?;
                out.push(GenerateConstruct::For {
                    var,
                    start,
                    cond,
                    step,
                    body,
                });
            } else {
                out.push(GenerateConstruct::Block(self.parse_generate_block()?));
            }
        }
        Ok(out)
    }

    fn parse_genvar_declaration(&mut self) -> Result<Vec<String>, String> {
        self.expect_kw("genvar")?;
        let mut names = Vec::new();
        loop {
            names.push(self.ident()?);
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct(";")?;
        Ok(names)
    }

    pub(super) fn parse_generate_block(&mut self) -> Result<GenerateBlock, String> {
        let line = self.line();
        self.expect_kw("begin")?;
        let label = if self.eat_punct(":") {
            self.ident()?
        } else {
            String::new()
        };
        let mut items = Vec::new();
        while !self.at_kw("end") {
            if self.at_eof() {
                return Err(format!("line {}: generate block is missing 'end'", line));
            }
            items.push(self.parse_item()?);
        }
        self.expect_kw("end")?;
        Ok(GenerateBlock {
            label: (!label.is_empty()).then_some(label),
            items,
        })
    }

    pub(super) fn parse_opt_delay(&mut self) -> Result<u64, String> {
        if self.eat_punct("#") {
            match self.peek().clone() {
                Tok::Num {
                    size,
                    based,
                    signed: _,
                    radix,
                    text,
                } => {
                    self.bump();
                    let size = size.unwrap_or(32) as usize;
                    let w = build_word(size, radix, &text)?;
                    let _ = based;
                    w.to_u64()
                        .filter(|v| *v <= 1_000_000)
                        .ok_or_else(|| "delay out of range".to_string())
                }
                _ => Err(format!("line {}: expected number after '#'", self.line())),
            }
        } else {
            Ok(0)
        }
    }

    pub(super) fn parse_sens(&mut self) -> Result<Sens, String> {
        self.expect_punct("@")?;
        // `@*`
        if self.at_punct("*") {
            self.bump();
            return Ok(Sens::Comb);
        }
        self.expect_punct("(")?;
        if self.eat_punct("*") {
            self.expect_punct(")")?;
            return Ok(Sens::Comb);
        }
        let mut edges = Vec::new();
        let mut level = false;
        loop {
            if self.eat_kw("posedge") {
                edges.push(Sens::Posedge(self.parse_expr()?));
            } else if self.eat_kw("negedge") {
                edges.push(Sens::Negedge(self.parse_expr()?));
            } else {
                // plain identifier: level-sensitive
                let _ = self.parse_expr()?;
                level = true;
            }
            if self.eat_kw("or") || self.eat_punct(",") {
                continue;
            }
            break;
        }
        self.expect_punct(")")?;
        if level {
            Ok(Sens::Comb)
        } else if edges.len() == 1 {
            Ok(edges.pop().unwrap())
        } else {
            Ok(Sens::List(edges))
        }
    }

    // ---------- statements ----------
}
