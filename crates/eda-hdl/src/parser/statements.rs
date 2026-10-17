//! Procedural statements and expression parsing.

use super::*;

impl Parser {
    pub(super) fn parse_stmt(&mut self) -> Result<Stmt, String> {
        let line = self.line();
        if self.at_kw("assert") {
            self.bump();
            if self.at_kw("property") {
                return Err(format!(
                    "line {}: concurrent assert property is not yet supported",
                    line
                ));
            }
            self.expect_punct("(")?;
            let cond = self.parse_expr()?;
            self.expect_punct(")")?;
            let mut message = None;
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
                                    "line {}: assertion message must be a string literal, found {:?}",
                                    line, other
                                ));
                            }
                        }
                        self.expect_punct(")")?;
                    }
                    other => {
                        return Err(format!(
                            "line {}: immediate assertions support $error/$fatal actions only, found {:?}",
                            line, other
                        ));
                    }
                }
            }
            self.expect_punct(";")?;
            return Ok(Stmt::Assert { cond, message });
        }
        if self.eat_kw("for") {
            self.expect_punct("(")?;
            let _ = self.eat_kw("int") || self.eat_kw("integer");
            let var = self.ident()?;
            self.expect_punct("=")?;
            let start = self.parse_expr()?;
            self.expect_punct(";")?;
            let cond = self.parse_expr()?;
            self.expect_punct(";")?;
            let step_var = self.ident()?;
            if step_var != var {
                return Err(format!(
                    "line {}: for-loop step must assign loop variable '{var}'",
                    line
                ));
            }
            self.expect_punct("=")?;
            let step = self.parse_expr()?;
            self.expect_punct(")")?;
            let body = Box::new(self.parse_stmt()?);
            return Ok(Stmt::For {
                var,
                start,
                cond,
                step,
                body,
            });
        }
        if self.eat_kw("while") {
            self.expect_punct("(")?;
            let cond = self.parse_expr()?;
            self.expect_punct(")")?;
            let body = Box::new(self.parse_stmt()?);
            return Ok(Stmt::While { cond, body });
        }
        if self.eat_kw("repeat") {
            self.expect_punct("(")?;
            let count = self.parse_expr()?;
            self.expect_punct(")")?;
            let body = Box::new(self.parse_stmt()?);
            return Ok(Stmt::Repeat { count, body });
        }
        if self.at_net_type() || self.at_custom_type() {
            let (type_name, range) = if self.at_net_type() {
                let net_type = self.parse_net_type().expect("checked net type");
                let explicit_range = self.parse_opt_range()?;
                (
                    None,
                    Self::net_range(Some(net_type), explicit_range)
                        .or_else(|| Some(default_range(net_type.default_width))),
                )
            } else {
                let type_name = self.parse_custom_type_name()?;
                (Some(type_name), self.parse_opt_range()?)
            };
            let mut names = Vec::new();
            let mut declared_array_ranges = Vec::new();
            loop {
                let name = self.ident()?;
                let array_ranges = self.parse_unpacked_ranges("function local")?;
                if type_name.is_some() && !array_ranges.is_empty() {
                    return Err(format!(
                        "line {}: typed aggregate arrays are not yet supported",
                        line
                    ));
                }
                let init = if self.eat_punct("=") {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                if !array_ranges.is_empty() && init.is_some() {
                    return Err(format!(
                        "line {}: unpacked array locals do not support initializers",
                        line
                    ));
                }
                names.push((name, init));
                if !array_ranges.is_empty() {
                    declared_array_ranges = array_ranges;
                    if self.at_punct(",") {
                        return Err(format!(
                            "line {}: unpacked array locals must be declared one per statement",
                            line
                        ));
                    }
                }
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_punct(";")?;
            return Ok(Stmt::Local {
                type_name,
                range,
                array_ranges: declared_array_ranges,
                names,
            });
        }
        if self.eat_kw("begin") {
            let mut stmts = Vec::new();
            while !self.at_kw("end") {
                if self.at_eof() {
                    return Err(format!("line {}: missing 'end'", line));
                }
                stmts.push(self.parse_stmt()?);
            }
            self.expect_kw("end")?;
            return Ok(Stmt::Block(stmts));
        }
        if self.eat_kw("if") {
            self.expect_punct("(")?;
            let c = self.parse_expr()?;
            self.expect_punct(")")?;
            let t = Box::new(self.parse_stmt()?);
            let f = if self.eat_kw("else") {
                Some(Box::new(self.parse_stmt()?))
            } else {
                None
            };
            return Ok(Stmt::If { c, t, f });
        }
        if self.at_kw("case") || self.at_kw("casez") || self.at_kw("casex") {
            let kind = if self.eat_kw("casez") {
                CaseKind::CaseZ
            } else if self.eat_kw("casex") {
                CaseKind::CaseX
            } else {
                self.bump();
                CaseKind::Exact
            };
            self.expect_punct("(")?;
            let e = self.parse_expr()?;
            self.expect_punct(")")?;
            let mut arms = Vec::new();
            let mut dflt = None;
            while !self.at_kw("endcase") {
                if self.at_eof() {
                    return Err(format!("line {}: missing 'endcase'", line));
                }
                if self.eat_kw("default") {
                    self.expect_punct(":")?;
                    dflt = Some(Box::new(self.parse_stmt()?));
                } else {
                    let mut labels = vec![self.parse_expr()?];
                    while self.eat_punct(",") {
                        labels.push(self.parse_expr()?);
                    }
                    self.expect_punct(":")?;
                    let body = self.parse_stmt()?;
                    arms.push((labels, body));
                }
            }
            self.expect_kw("endcase")?;
            return Ok(Stmt::Case {
                e,
                arms,
                dflt,
                kind,
            });
        }
        if self.at_punct("#") {
            let ns = self.parse_opt_delay()?;
            if self.eat_punct(";") {
                return Ok(Stmt::Delay {
                    ns,
                    body: Box::new(Stmt::Block(Vec::new())),
                });
            }
            let body = Box::new(self.parse_stmt()?);
            return Ok(Stmt::Delay { ns, body });
        }
        if self.eat_kw("forever") {
            let body = Box::new(self.parse_stmt()?);
            return Ok(Stmt::Forever(body));
        }
        if let Tok::Ident(s) = self.peek().clone() {
            if matches!(
                s.as_str(),
                "$readmemh" | "$readmemb" | "$writememh" | "$writememb"
            ) {
                let is_write = s.starts_with("$write");
                let radix = if s.ends_with('h') { 16 } else { 2 };
                self.bump();
                self.expect_punct("(")?;
                let path = match self.peek().clone() {
                    Tok::Str(path) => {
                        self.bump();
                        path
                    }
                    _ => {
                        return Err(format!(
                            "line {}: {s} requires a string literal file path",
                            line
                        ))
                    }
                };
                self.expect_punct(",")?;
                let memory = self.ident()?;
                let mut bounds = Vec::new();
                while self.eat_punct(",") {
                    if bounds.len() == 2 {
                        return Err(format!(
                            "line {}: {s} supports at most (file, memory, start, finish)",
                            line
                        ));
                    }
                    bounds.push(self.parse_expr()?);
                }
                self.expect_punct(")")?;
                self.expect_punct(";")?;
                let start = bounds.first().cloned();
                let finish = bounds.get(1).cloned();
                let fields = (radix, path, memory, start, finish);
                return Ok(if is_write {
                    let (radix, path, memory, start, finish) = fields;
                    Stmt::WriteMem {
                        radix,
                        path,
                        memory,
                        start,
                        finish,
                    }
                } else {
                    let (radix, path, memory, start, finish) = fields;
                    Stmt::ReadMem {
                        radix,
                        path,
                        memory,
                        start,
                        finish,
                    }
                });
            }
            if s == "$display"
                || s == "$write"
                || s == "$monitor"
                || s == "$displayb"
                || s == "$displayh"
                || s == "$displayd"
            {
                let monitor = s == "$monitor";
                self.bump();
                let default_spec = match s.as_str() {
                    "$displayb" => 'b',
                    "$displayh" => 'h',
                    _ => 'd',
                };
                self.expect_punct("(")?;
                let fmt_str = match self.peek().clone() {
                    Tok::Str(x) => {
                        self.bump();
                        x
                    }
                    _ => String::new(),
                };
                let mut args = Vec::new();
                while self.eat_punct(",") {
                    args.push(self.parse_expr()?);
                }
                self.expect_punct(")")?;
                self.expect_punct(";")?;
                let fmt = parse_format(&fmt_str, default_spec, &args, line)?;
                return Ok(Stmt::Display { fmt, args, monitor });
            }
            if s == "$uvm_info" || s == "$uvm_warning" || s == "$uvm_error" || s == "$uvm_fatal" {
                let severity = s[1..].to_uppercase();
                let has_verbosity = s == "$uvm_info";
                self.bump();
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
                let min_args = if has_verbosity { 3 } else { 2 };
                if args.len() < min_args {
                    return Err(format!(
                        "line {}: {s} requires (tag, format, ...{})",
                        line,
                        if has_verbosity { ", verbosity" } else { "" }
                    ));
                }
                let verbosity = if has_verbosity {
                    let verbosity = args.pop().unwrap();
                    let verb_name = match &verbosity {
                        Expr::Ident(name) => name.clone(),
                        _ => {
                            return Err(format!(
                                "line {}: {s} verbosity must be a UVM_* identifier",
                                line
                            ))
                        }
                    };
                    const VERBOSITIES: &[&str] = &[
                        "UVM_NONE",
                        "UVM_LOW",
                        "UVM_MEDIUM",
                        "UVM_HIGH",
                        "UVM_FULL",
                        "UVM_DEBUG",
                    ];
                    if !VERBOSITIES.contains(&verb_name.as_str()) {
                        return Err(format!(
                            "line {}: unknown {s} verbosity '{verb_name}'",
                            line
                        ));
                    }
                    format!("[{verb_name}]")
                } else {
                    String::new()
                };
                let Expr::Str(tag) = &args[0] else {
                    return Err(format!("line {}: {s} tag must be a string literal", line));
                };
                let Expr::Str(fmt_str) = &args[1] else {
                    return Err(format!(
                        "line {}: {s} message must be a format string literal",
                        line
                    ));
                };
                let value_args = args[2..].to_vec();
                let prefixed = format!("[{severity}][{tag}]{verbosity} {fmt_str}");
                let fmt = parse_format(&prefixed, 'd', &value_args, line)?;
                let message = Stmt::Display {
                    fmt,
                    args: value_args,
                    monitor: false,
                };
                if s == "$uvm_fatal" {
                    // UVM 语义：fatal 消息后结束仿真。
                    return Ok(Stmt::Block(vec![message, Stmt::Finish]));
                }
                return Ok(message);
            }
            if s == "$finish" || s == "$stop" {
                self.bump();
                if self.eat_punct("(") {
                    self.expect_punct(")")?;
                }
                self.expect_punct(";")?;
                return Ok(Stmt::Finish);
            }
        }
        // assignment statements
        let lv_or_call = self.parse_expr_stmt_target()?;
        if let Expr::MethodCall {
            object,
            method,
            arguments,
        } = lv_or_call
        {
            if self.eat_punct(";") {
                return Ok(Stmt::MethodCall {
                    object,
                    method,
                    arguments: arguments.to_vec(),
                });
            }
            return Err(format!(
                "line {}: method call must be a statement or assignment operand",
                line
            ));
        }
        if let Expr::Call(ref name, ref arguments) = lv_or_call {
            if name == "$cast" && self.eat_punct(";") {
                return Ok(Stmt::MethodCall {
                    object: "$cast".to_string(),
                    method: "cast".to_string(),
                    arguments: arguments.to_vec(),
                });
            }
        }
        if let Expr::MethodCallChain {
            path,
            method,
            arguments,
        } = lv_or_call
        {
            if self.eat_punct(";") {
                return Ok(Stmt::MethodCall {
                    object: path.join("."),
                    method,
                    arguments,
                });
            }
            return Err(format!(
                "line {}: method call must be a statement or assignment operand",
                line
            ));
        }
        let lv = lv_or_call;
        if self.eat_punct("<=") {
            let e = self.parse_expr()?;
            self.expect_punct(";")?;
            return Ok(Stmt::NBL { lv, e });
        }
        if self.eat_punct("=") {
            let e = self.parse_expr()?;
            self.expect_punct(";")?;
            return Ok(Stmt::BL { lv, e });
        }
        Err(format!(
            "line {}: expected '<=' or '=' after assignment target",
            line
        ))
    }

    // ---------- expressions (precedence climbing) ----------

    pub fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_cond()
    }

    /// Parse a procedural-assignment target. A top-level `<=` remains
    /// unconsumed so `parse_stmt` can classify the statement as nonblocking.
    pub(super) fn parse_expr_stmt_target(&mut self) -> Result<Expr, String> {
        self.block_le = true;
        let result = self.parse_cond();
        self.block_le = false;
        result
    }

    pub(super) fn parse_cond(&mut self) -> Result<Expr, String> {
        let c = self.parse_binary(1)?;
        if self.eat_punct("?") {
            let t = self.parse_cond()?;
            self.expect_punct(":")?;
            let f = self.parse_cond()?;
            return Ok(Expr::Cond(Box::new(c), Box::new(t), Box::new(f)));
        }
        Ok(c)
    }

    pub(super) fn parse_binary(&mut self, min_prec: u8) -> Result<Expr, String> {
        let mut lhs = self.parse_unary()?;
        let mut first_operator = true;
        loop {
            let (op, prec) = match self.peek() {
                Tok::Punct("||") => (BinOpSrc::LogOr, 2),
                Tok::Punct("&&") => (BinOpSrc::LogAnd, 3),
                Tok::Punct("|") => (BinOpSrc::Or, 4),
                Tok::Punct("^") | Tok::Punct("^~") | Tok::Punct("~^") => (BinOpSrc::Xor, 5),
                Tok::Punct("&") => (BinOpSrc::And, 6),
                Tok::Punct("==") => (BinOpSrc::Eq, 7),
                Tok::Punct("!=") => (BinOpSrc::Ne, 7),
                Tok::Punct("===") => (BinOpSrc::CaseEq, 7),
                Tok::Punct("!==") => (BinOpSrc::CaseNe, 7),
                Tok::Punct("<") => (BinOpSrc::Lt, 8),
                Tok::Punct("<=") if !(self.block_le && first_operator) => (BinOpSrc::Le, 8),
                Tok::Punct(">") => (BinOpSrc::Gt, 8),
                Tok::Punct(">=") => (BinOpSrc::Ge, 8),
                Tok::Punct("<<") => (BinOpSrc::Shl, 9),
                Tok::Punct(">>") => (BinOpSrc::Shr, 9),
                Tok::Punct("+") => (BinOpSrc::Add, 10),
                Tok::Punct("-") => (BinOpSrc::Sub, 10),
                Tok::Punct("*") => (BinOpSrc::Mul, 11),
                Tok::Punct("/") => (BinOpSrc::Div, 11),
                Tok::Punct("%") => (BinOpSrc::Mod, 11),
                _ => break,
            };
            if prec < min_prec {
                break;
            }
            first_operator = false;
            self.bump();
            let rhs = self.parse_binary(prec + 1)?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    pub(super) fn parse_unary(&mut self) -> Result<Expr, String> {
        let op = match self.peek() {
            Tok::Punct("!") => Some(UnOpSrc::LogNot),
            Tok::Punct("~") => Some(UnOpSrc::Not),
            Tok::Punct("~&") => Some(UnOpSrc::NandR),
            Tok::Punct("~|") => Some(UnOpSrc::NorR),
            Tok::Punct("~^") | Tok::Punct("^~") => Some(UnOpSrc::XnorR),
            Tok::Punct("&") => Some(UnOpSrc::AndR),
            Tok::Punct("|") => Some(UnOpSrc::OrR),
            Tok::Punct("^") => Some(UnOpSrc::XorR),
            Tok::Punct("+") => Some(UnOpSrc::Plus),
            Tok::Punct("-") => Some(UnOpSrc::Minus),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let a = self.parse_unary()?;
            return Ok(Expr::Un(op, Box::new(a)));
        }
        self.parse_primary()
    }

    pub(super) fn parse_primary(&mut self) -> Result<Expr, String> {
        let line = self.line();
        match self.peek().clone() {
            Tok::Num {
                size,
                based,
                signed,
                radix,
                text,
            } => {
                self.bump();
                let width = if based {
                    size.unwrap_or(32) as usize
                } else {
                    32
                };
                let word = build_word(width, radix, &text)?;
                let number = Expr::Num {
                    width: Some(width),
                    word,
                    orig: text,
                };
                if signed {
                    Ok(Expr::Call("$signed".to_string(), vec![number]))
                } else {
                    Ok(number)
                }
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Expr::Str(s))
            }
            Tok::Ident(name) => {
                if is_reserved(&name) {
                    return Err(format!("line {}: unexpected keyword '{}'", line, name));
                }
                self.bump();
                if self.eat_punct("::") {
                    let scoped_name = self.ident()?;
                    return Ok(Expr::ScopedIdent {
                        package: name,
                        name: scoped_name,
                    });
                }
                if self.eat_punct(".") {
                    // Collect a dotted member/method chain: `a.b.c.m(...)`.
                    let mut segments = vec![name.clone()];
                    let mut method = None;
                    loop {
                        let field = self.ident()?;
                        if self.at_punct("(") {
                            method = Some(field);
                            break;
                        }
                        segments.push(field);
                        if !self.at_punct(".") {
                            break;
                        }
                        self.bump();
                    }
                    if let Some(method) = method {
                        if segments.len() == 1 {
                            self.bump();
                            let mut arguments = Vec::new();
                            if !self.at_punct(")") {
                                loop {
                                    arguments.push(self.parse_expr()?);
                                    if !self.eat_punct(",") {
                                        break;
                                    }
                                }
                            }
                            self.expect_punct(")")?;
                            return Ok(Expr::MethodCall {
                                object: name,
                                method,
                                arguments,
                            });
                        }
                        self.bump();
                        let mut arguments = Vec::new();
                        if !self.at_punct(")") {
                            loop {
                                arguments.push(self.parse_expr()?);
                                if !self.eat_punct(",") {
                                    break;
                                }
                            }
                        }
                        self.expect_punct(")")?;
                        return Ok(Expr::MethodCallChain {
                            path: segments,
                            method,
                            arguments,
                        });
                    }
                    if segments.len() == 2 {
                        return Ok(Expr::Member(name, segments.remove(1)));
                    }
                    return Ok(Expr::MemberChain(segments));
                }
                if name == "new" && self.at_punct("[") {
                    self.bump();
                    let size = self.parse_expr()?;
                    self.expect_punct("]")?;
                    return Ok(Expr::New(Box::new(size)));
                }
                if name == "new" {
                    if self.eat_punct("(") {
                        let mut arguments = Vec::new();
                        if !self.at_punct(")") {
                            loop {
                                arguments.push(self.parse_expr()?);
                                if !self.eat_punct(",") {
                                    break;
                                }
                            }
                        }
                        self.expect_punct(")")?;
                        return Ok(Expr::NewClass(arguments));
                    }
                    if !self.at_punct(";")
                        && !self.at_punct(",")
                        && !self.at_punct(")")
                        && !self.at_punct("]")
                    {
                        let source = self.parse_expr()?;
                        return Ok(Expr::New(Box::new(source)));
                    }
                    return Ok(Expr::NewClass(Vec::new()));
                }
                if self.at_punct("[") {
                    if matches!(
                        self.toks.get(self.pos + 1).map(|token| &token.tok),
                        Some(Tok::Punct("*")) | Some(Tok::Punct("->")) | Some(Tok::Punct("="))
                    ) {
                        return Ok(Expr::Ident(name));
                    }
                    self.bump();
                    let a = self.parse_expr()?;
                    if self.eat_punct("+:") || self.eat_punct("-:") {
                        let ascending = self.toks[self.pos - 1].tok == Tok::Punct("+:");
                        let width = self.parse_expr()?;
                        self.expect_punct("]")?;
                        return Ok(Expr::IndexedPart {
                            name,
                            start: Box::new(a),
                            width: Box::new(width),
                            ascending,
                        });
                    }
                    if self.eat_punct(":") {
                        let b = self.parse_expr()?;
                        self.expect_punct("]")?;
                        return Ok(Expr::Part(name, Box::new(a), Box::new(b)));
                    }
                    self.expect_punct("]")?;
                    if self.at_punct("[") {
                        let mut indices = vec![a];
                        while self.at_punct("[") {
                            self.bump();
                            let next = self.parse_expr()?;
                            if self.eat_punct("+:") || self.eat_punct("-:") {
                                let ascending = self.toks[self.pos - 1].tok == Tok::Punct("+:");
                                let width = self.parse_expr()?;
                                self.expect_punct("]")?;
                                return Ok(Expr::ArrayIndexedPart {
                                    name,
                                    indexes: indices,
                                    start: Box::new(next),
                                    width: Box::new(width),
                                    ascending,
                                });
                            }
                            if self.eat_punct(":") {
                                let msb = next;
                                let lsb = self.parse_expr()?;
                                self.expect_punct("]")?;
                                return Ok(Expr::ArrayPart {
                                    name,
                                    indexes: indices,
                                    msb: Box::new(msb),
                                    lsb: Box::new(lsb),
                                });
                            }
                            self.expect_punct("]")?;
                            indices.push(next);
                        }
                        return Ok(Expr::ArraySelect(name, indices));
                    }
                    return Ok(Expr::Select(name, Box::new(a)));
                }
                if self.at_punct("(") {
                    self.bump();
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
                    return Ok(Expr::Call(name, args));
                }
                Ok(Expr::Ident(name))
            }
            Tok::Punct("(") => {
                self.bump();
                let e = self.parse_expr()?;
                self.expect_punct(")")?;
                Ok(e)
            }
            Tok::Punct("{") => {
                self.bump();
                let first = self.parse_expr()?;
                if self.at_punct("{") {
                    // replication {n{expr}}
                    self.bump();
                    let inner = self.parse_expr()?;
                    self.expect_punct("}")?;
                    self.expect_punct("}")?;
                    return Ok(Expr::Replicate(Box::new(first), Box::new(inner)));
                }
                let mut parts = vec![first];
                while self.eat_punct(",") {
                    parts.push(self.parse_expr()?);
                }
                self.expect_punct("}")?;
                Ok(Expr::Concat(parts))
            }
            other => Err(format!(
                "line {}: unexpected token {:?} in expression",
                line, other
            )),
        }
    }
}
