//! Recursive-descent Verilog parser (subset, see README for the supported
//! construct list).

use crate::ast::*;
use crate::lexer::{Lexer, Tok, Token};
use eda_core::logic::{LogicValue, LogicWord};

pub fn parse(source: &str) -> Result<SourceFile, String> {
    let toks = Lexer::tokenize(source)?;
    let mut p = Parser {
        toks,
        pos: 0,
        block_le: false,
        known_classes: Default::default(),
        class_specializations: Vec::new(),
        allow_symbolic_class_actual: false,
    };
    let mut modules = Vec::new();
    let mut packages = Vec::new();
    let mut imports = Vec::new();
    let mut classes = Vec::new();
    while !p.at_eof() {
        if p.at_kw("package") {
            packages.push(p.parse_package()?);
        } else if p.at_kw("import") {
            imports.extend(p.parse_import()?);
        } else if p.at_kw("interface") {
            modules.push(p.parse_interface()?);
        } else if p.at_kw("class")
            || (p.at_kw("virtual")
                && matches!(
                    p.toks.get(p.pos + 1).map(|token| &token.tok),
                    Some(Tok::Ident(word)) if word == "class"
                ))
        {
            let is_virtual = p.eat_kw("virtual");
            if let Item::ClassDef(definition) = p.parse_class(p.line(), is_virtual)? {
                classes.push(definition);
            }
        } else {
            modules.push(p.parse_module()?);
        }
    }
    Ok(SourceFile {
        modules,
        packages,
        imports,
        classes,
        class_specializations: p.class_specializations,
    })
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    /// While parsing a procedural assignment target, `<=` must be kept for
    /// the statement layer instead of being consumed as the `Le` operator.
    block_le: bool,
    known_classes: std::collections::HashSet<String>,
    class_specializations: Vec<crate::ast::ClassSpecialization>,
    allow_symbolic_class_actual: bool,
}

const DIRS: &[&str] = &["input", "output", "inout"];
const GATES: &[&str] = &["and", "nand", "or", "nor", "xor", "xnor", "buf", "not"];
const NET_TYPES: &[&str] = &[
    "wire", "tri", "reg", "logic", "bit", "byte", "shortint", "int", "longint", "integer", "string",
];
type PackedRange = (Box<Expr>, Box<Expr>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NetTypeInfo {
    is_variable: bool,
    is_signed: bool,
    default_width: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek_str(&self) -> &str {
        match self.peek() {
            Tok::Ident(s) => s,
            Tok::Punct(p) => p,
            _ => "",
        }
    }

    fn line(&self) -> usize {
        self.toks[self.pos].line
    }

    fn at_net_type(&self) -> bool {
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

    fn at_custom_type(&self) -> bool {
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

    fn parse_custom_type_name(&mut self) -> Result<String, String> {
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

    fn parse_class_specialization_name(&mut self, base_name: String) -> Result<String, String> {
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

    fn at_instance_header(&self) -> bool {
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

    fn parse_net_type(&mut self) -> Option<NetTypeInfo> {
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
        Some(info)
    }

    fn net_range(info: Option<NetTypeInfo>, explicit: Option<PackedRange>) -> Option<PackedRange> {
        explicit.or_else(|| {
            let width = info?.default_width;
            (width > 1).then(|| default_range(width))
        })
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), Tok::Eof)
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn at_punct(&self, p: &str) -> bool {
        matches!(self.peek(), Tok::Punct(q) if *q == p)
    }

    fn eat_punct(&mut self, p: &str) -> bool {
        if self.at_punct(p) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, p: &str) -> Result<(), String> {
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

    fn at_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == kw)
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.at_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_kw(&mut self, kw: &str) -> Result<(), String> {
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

    fn ident(&mut self) -> Result<String, String> {
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

    fn ident_any(&mut self) -> Result<String, String> {
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

    fn parse_interface(&mut self) -> Result<Module, String> {
        let line = self.line();
        self.expect_kw("interface")?;
        let name = self.ident()?;
        self.expect_punct(";")?;
        let mut items = Vec::new();
        while !self.at_kw("endinterface") {
            if self.at_eof() {
                return Err(format!(
                    "line {}: interface '{name}' is missing 'endinterface'",
                    line
                ));
            }
            match self.parse_item()? {
                matched @ Item::Net { .. } => items.push(matched),
                matched @ Item::Params(_) => items.push(matched),
                matched @ Item::Modport { .. } => items.push(matched),
                other => {
                    return Err(format!(
                        "line {}: interface '{name}' supports signal and parameter declarations only, found {:?}",
                        line, other
                    ));
                }
            }
        }
        self.expect_kw("endinterface")?;
        Ok(Module {
            name,
            items,
            is_interface: true,
        })
    }

    fn merge_non_ansi_port_declaration(items: &mut [Item], declaration: &Item) -> bool {
        let Item::Port {
            dir,
            is_reg,
            is_signed,
            is_virtual: _,
            interface_name: _,
            modport_name: _,
            type_name,
            range,
            array_ranges,
            names,
        } = declaration
        else {
            return false;
        };
        let mut targets = Vec::with_capacity(names.len());
        for name in names {
            let target = items.iter().position(|existing| {
                let Item::Port {
                    names: existing_names,
                    range: existing_range,
                    type_name: existing_type,
                    array_ranges: existing_arrays,
                    interface_name: existing_interface,
                    is_virtual: existing_virtual,
                    ..
                } = existing
                else {
                    return false;
                };
                existing_names.len() == 1
                    && existing_names[0] == *name
                    && existing_range.is_none()
                    && existing_type.is_none()
                    && existing_arrays.is_empty()
                    && existing_interface.is_none()
                    && !*existing_virtual
            });
            let Some(target) = target else {
                return false;
            };
            targets.push(target);
        }
        for (name, target) in names.iter().zip(targets) {
            if let Item::Port {
                dir: existing_dir,
                is_reg: existing_is_reg,
                is_signed: existing_is_signed,
                type_name: existing_type,
                range: existing_range,
                array_ranges: existing_arrays,
                names: existing_names,
                ..
            } = &mut items[target]
            {
                *existing_dir = *dir;
                *existing_is_reg = *is_reg;
                *existing_is_signed = *is_signed;
                *existing_type = type_name.clone();
                *existing_range = range.clone();
                *existing_arrays = array_ranges.clone();
                *existing_names = vec![name.clone()];
            }
        }
        true
    }

    fn parse_module(&mut self) -> Result<Module, String> {
        if !self.eat_kw("module") {
            return Err(format!(
                "line {}: expected 'module', found '{}'",
                self.line(),
                self.peek_str()
            ));
        }
        let name = self.ident()?;
        let mut items = Vec::new();
        // Module-level parameter declarations: #(parameter W = 4, ...)
        if self.eat_punct("#") {
            self.expect_punct("(")?;
            let mut params = Vec::new();
            loop {
                if !(self.eat_kw("parameter") || self.eat_kw("localparam")) {
                    return Err(format!(
                        "line {}: expected 'parameter' in module header",
                        self.line()
                    ));
                }
                let _ = self.eat_kw("integer");
                loop {
                    let n = self.ident()?;
                    self.expect_punct("=")?;
                    let v = self.parse_expr()?;
                    params.push((n, v));
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                if !self.eat_punct(",") {
                    break;
                }
            }
            items.push(Item::Params(params));
            self.expect_punct(")")?;
        }
        if self.eat_punct("(") {
            // Port header: either ANSI (with directions) or plain name list.
            loop {
                if self.at_punct(")") {
                    break;
                }
                let is_virtual = self.eat_kw("virtual");
                if self.eat_kw("interface") {
                    let interface_name = self.ident_any()?;
                    let mut modport_name = None;
                    if self.eat_punct(".") {
                        modport_name = Some(self.ident_any()?);
                    }
                    loop {
                        let pname = self.ident()?;
                        items.push(Item::Port {
                            dir: DirSrc::InOut,
                            is_reg: false,
                            is_signed: false,
                            is_virtual,
                            interface_name: Some(interface_name.clone()),
                            modport_name: modport_name.clone(),
                            range: None,
                            type_name: None,
                            array_ranges: Vec::new(),
                            names: vec![pname],
                        });
                        if !self.eat_punct(",") {
                            break;
                        }
                        if self.at_punct(")")
                            || self.at_kw("interface")
                            || self.at_kw("virtual")
                            || matches!(self.peek(), Tok::Ident(s) if DIRS.contains(&s.as_str()))
                        {
                            break;
                        }
                    }
                    if self.at_punct(")") {
                        break;
                    }
                    continue;
                }
                if matches!(self.peek(), Tok::Ident(s) if DIRS.contains(&s.as_str())) {
                    let dir = match self.ident_any()?.as_str() {
                        "input" => DirSrc::In,
                        "output" => DirSrc::Out,
                        _ => DirSrc::InOut,
                    };
                    let net_type = self.parse_net_type();
                    let mut cur_is_signed = net_type.is_some_and(|info| info.is_signed);
                    let mut custom_type = None;
                    if net_type.is_none() && self.at_custom_type() {
                        custom_type = Some(self.parse_custom_type_name()?);
                    }
                    let explicit_range = self.parse_opt_range()?;
                    let mut cur_type = custom_type;
                    let mut cur_range = if cur_type.is_some() {
                        explicit_range
                    } else {
                        Self::net_range(net_type, explicit_range)
                    };
                    let mut cur_is_reg = net_type.is_some_and(|info| info.is_variable);
                    loop {
                        let pname = self.ident()?;
                        let cur_array_ranges = self.parse_unpacked_ranges("port")?;
                        items.push(Item::Port {
                            dir,
                            is_reg: cur_is_reg,
                            is_signed: cur_is_signed,
                            is_virtual: false,
                            interface_name: None,
                            modport_name: None,
                            range: cur_range.clone(),
                            type_name: cur_type.clone(),
                            array_ranges: cur_array_ranges,
                            names: vec![pname],
                        });
                        if !self.eat_punct(",") {
                            break;
                        }
                        if matches!(self.peek(), Tok::Ident(s) if DIRS.contains(&s.as_str()))
                            || self.at_punct(")")
                        {
                            break; // next port declares a new direction / list ends
                        }
                        // Verilog allows each identifier in a direction list
                        // to restate its net type, e.g. `output reg q, reg r`.
                        if self.at_net_type() {
                            let net_type = self.parse_net_type();
                            cur_type = None;
                            cur_is_reg = net_type.is_some_and(|info| info.is_variable);
                            cur_is_signed = net_type.is_some_and(|info| info.is_signed);
                            let explicit_range = self.parse_opt_range()?;
                            cur_range = Self::net_range(net_type, explicit_range);
                        } else if self.at_custom_type() {
                            cur_type = Some(self.parse_custom_type_name()?);
                            cur_is_reg = true;
                            cur_is_signed = false;
                            cur_range = self.parse_opt_range()?;
                        }
                        // per-port range without repeating the direction
                        if self.at_punct("[") {
                            cur_range = self.parse_opt_range()?;
                        }
                    }
                    if self.at_punct(")") {
                        break;
                    }
                    if !matches!(self.peek(), Tok::Ident(s) if DIRS.contains(&s.as_str())) {
                        // comma already consumed inside the inner loop
                    }
                } else {
                    let pname = self.ident()?;
                    items.push(Item::Port {
                        dir: DirSrc::In,
                        is_reg: false,
                        is_signed: false,
                        is_virtual: false,
                        interface_name: None,
                        modport_name: None,
                        range: None,
                        type_name: None,
                        array_ranges: Vec::new(),
                        names: vec![pname],
                    });
                    // direction refined by body declarations at elaboration
                    if !self.eat_punct(",") {
                        break;
                    }
                }
            }
            self.expect_punct(")")?;
        }
        self.expect_punct(";")?;
        loop {
            if self.eat_kw("endmodule") {
                break;
            }
            if self.at_eof() {
                return Err(format!("module '{}': missing 'endmodule'", name));
            }
            let item = self.parse_item()?;
            if Self::merge_non_ansi_port_declaration(&mut items, &item) {
                continue;
            }
            items.push(item);
        }
        Ok(Module {
            name,
            items,
            is_interface: false,
        })
    }

    fn parse_package(&mut self) -> Result<Package, String> {
        let line = self.line();
        self.expect_kw("package")?;
        let name = self.ident()?;
        self.expect_punct(";")?;
        let mut params = Vec::new();
        let mut types = Vec::new();
        let mut functions = Vec::new();
        let mut classes = Vec::new();
        while !self.at_kw("endpackage") {
            if self.at_eof() {
                return Err(format!(
                    "line {}: package '{name}' is missing 'endpackage'",
                    line
                ));
            }
            match self.parse_item()? {
                Item::Params(list) => params.extend(list),
                Item::Imports(_) => {}
                Item::TypeDef(definition) => types.push(definition),
                Item::Function(definition) => functions.push(definition),
                Item::ClassDef(definition) => classes.push(definition),
                _ => {
                    return Err(format!(
                    "package '{name}' supports parameters, imports, typedefs, functions, and classes only"
                ))
                }
            }
        }
        self.expect_kw("endpackage")?;
        Ok(Package {
            name,
            params,
            types,
            functions,
            classes,
        })
    }

    fn parse_import(&mut self) -> Result<Vec<Import>, String> {
        self.expect_kw("import")?;
        let mut imports = Vec::new();
        loop {
            let package = self.ident()?;
            self.expect_punct("::")?;
            let mut names = Vec::new();
            if !self.eat_punct("*") {
                names.push(self.ident()?);
            }
            imports.push(Import { package, names });
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct(";")?;
        Ok(imports)
    }

    fn parse_typedef(&mut self) -> Result<Item, String> {
        let line = self.line();
        self.expect_kw("typedef")?;
        if self.at_kw("struct") {
            return self.parse_struct_typedef(line);
        }
        if self.at_kw("union") {
            return self.parse_struct_typedef(line);
        }
        self.expect_kw("enum")?;
        let _ = self.parse_net_type();
        let range = self.parse_opt_range()?;
        self.expect_punct("{")?;
        let mut constants = Vec::new();
        loop {
            let name = self.ident()?;
            let value = if self.eat_punct("=") {
                Some(self.parse_expr()?)
            } else {
                None
            };
            constants.push((name, value));
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct("}")?;
        let name = self.ident()?;
        self.expect_punct(";")?;
        if constants.len() < 2 {
            return Err(format!(
                "line {}: enum typedef '{}' needs at least two constants",
                line, name
            ));
        }
        Ok(Item::TypeDef(TypeDef::Enum(EnumTypeDef {
            name,
            range,
            constants,
        })))
    }

    fn parse_function(&mut self) -> Result<Item, String> {
        let line = self.line();
        self.expect_kw("function")?;
        let _ = self.eat_kw("automatic");
        let net_type = self.parse_net_type();
        // Classic Verilog allows `function [msb:lsb] name(...)` with no
        // explicit return type keyword.
        let return_range = if let Some(info) = net_type {
            let explicit_range = self.parse_opt_range()?;
            Self::net_range(Some(info), explicit_range)
                .or_else(|| Some(default_range(info.default_width)))
        } else if self.at_punct("[") {
            self.parse_opt_range()?
        } else {
            return Err(format!(
                "line {}: function requires a return net type",
                line
            ));
        };
        let name = self.ident()?;
        self.expect_punct("(")?;
        let mut args = Vec::new();
        let mut arg_defaults = Vec::new();
        if !self.at_punct(")") {
            loop {
                self.expect_kw("input")?;
                let arg_type = self.parse_net_type();
                // Classic Verilog `input [msb:lsb] name` has no type keyword.
                let mut range = if let Some(arg_info) = arg_type {
                    let explicit_range = self.parse_opt_range()?;
                    Self::net_range(Some(arg_info), explicit_range)
                        .or_else(|| Some(default_range(arg_info.default_width)))
                } else if self.at_punct("[") {
                    self.parse_opt_range()?
                } else {
                    return Err(format!("line {}: function argument requires a type", line));
                };
                loop {
                    let arg_name = self.ident()?;
                    args.push((arg_name, range.clone()));
                    let default = if self.eat_punct("=") {
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    arg_defaults.push(default);
                    if !self.eat_punct(",") {
                        break;
                    }
                    if self.at_kw("input") {
                        break;
                    }
                    if self.at_punct("[") {
                        range = self.parse_opt_range()?;
                    }
                }
                if !self.eat_punct(",") && !self.at_kw("input") {
                    break;
                }
            }
        }
        self.expect_punct(")")?;
        self.expect_punct(";")?;
        let mut body = Vec::new();
        while !self.at_kw("endfunction") {
            if self.at_eof() {
                return Err(format!(
                    "line {}: function '{name}' is missing 'endfunction'",
                    line
                ));
            }
            body.push(self.parse_stmt()?);
        }
        self.expect_kw("endfunction")?;
        let _ = self.eat_punct(";");
        Ok(Item::Function(FunctionDef {
            name,
            return_range,
            args,
            arg_defaults,
            arg_types: Vec::new(),
            body,
            is_virtual: false,
            is_static: false,
        }))
    }

    fn parse_class(&mut self, line: usize, is_virtual: bool) -> Result<Item, String> {
        self.expect_kw("class")?;
        let name = self.ident()?;
        let mut params = Vec::new();
        if self.eat_punct("#") {
            self.expect_punct("(")?;
            while !self.at_punct(")") {
                if !self.eat_kw("parameter") && !self.eat_kw("localparam") {
                    return Err(format!(
                        "line {}: class parameter declaration must start with 'parameter'",
                        line
                    ));
                }
                let _ = self.parse_net_type();
                let parameter = self.ident()?;
                let default = if self.eat_punct("=") {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                params.push((parameter, default));
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_punct(")")?;
        }
        let mut extends = None;
        let mut extends_args = Vec::new();
        if self.eat_kw("extends") {
            let previous = self.allow_symbolic_class_actual;
            self.allow_symbolic_class_actual = true;
            extends = Some(self.parse_custom_type_name()?);
            self.allow_symbolic_class_actual = previous;
            if self.eat_punct("(") {
                if !self.at_punct(")") {
                    loop {
                        extends_args.push(self.parse_expr()?);
                        if !self.eat_punct(",") {
                            break;
                        }
                    }
                }
                self.expect_punct(")")?;
            }
        }
        self.expect_punct(";")?;
        let mut properties = Vec::new();
        let mut methods = Vec::new();
        while !self.at_kw("endclass") {
            if self.at_eof() {
                return Err(format!(
                    "line {}: class '{name}' is missing 'endclass'",
                    line
                ));
            }
            if self.at_kw("function")
                || self.at_kw("task")
                || ((self.at_kw("virtual") || self.at_kw("static"))
                    && matches!(
                        self.toks.get(self.pos + 1).map(|token| &token.tok),
                        Some(Tok::Ident(word)) if word == "function" || word == "task"
                    ))
            {
                methods.push(self.parse_class_method()?);
                continue;
            }
            properties.push(self.parse_class_property()?);
        }
        self.expect_kw("endclass")?;
        if self.eat_punct(":") {
            let label = self.ident_any()?;
            if label != name {
                return Err(format!(
                    "line {}: endclass label '{label}' does not match class '{name}'",
                    line
                ));
            }
        }
        let _ = self.eat_punct(";");
        self.known_classes.insert(name.clone());
        Ok(Item::ClassDef(ClassDef {
            name,
            params,
            extends,
            extends_args,
            is_virtual,
            properties,
            methods,
        }))
    }

    fn parse_class_method(&mut self) -> Result<FunctionDef, String> {
        let line = self.line();
        let is_virtual = self.eat_kw("virtual");
        let is_static = self.eat_kw("static");
        let is_task = if self.at_kw("function") {
            self.bump();
            false
        } else {
            self.expect_kw("task")?;
            true
        };
        let _ = self.eat_kw("automatic");
        let is_constructor = matches!(self.peek(), Tok::Ident(w) if w == "new")
            && matches!(
                self.toks.get(self.pos + 1).map(|token| &token.tok),
                Some(Tok::Punct("("))
            );
        let is_void = !is_constructor && self.at_kw("void");
        let net_type = if is_constructor || is_void || is_task {
            let _ = self.eat_kw("void");
            None
        } else {
            self.parse_net_type()
        };
        let explicit_range = self.parse_opt_range()?;
        let return_range = match net_type {
            Some(info) => Self::net_range(Some(info), explicit_range)
                .or_else(|| Some(default_range(info.default_width))),
            None => None,
        };
        if return_range.is_none() && !is_constructor && !is_void && !is_task {
            return Err(format!(
                "line {}: class method requires a return type",
                line
            ));
        }
        let name = self.ident_any()?;
        self.expect_punct("(")?;
        let mut args = Vec::new();
        let mut arg_types = Vec::new();
        let mut arg_defaults = Vec::new();
        if !self.at_punct(")") {
            loop {
                if self.at_punct(")") {
                    break;
                }
                let _ = self.eat_kw("input");
                let arg_type = self.parse_net_type();
                let mut arg_type_name = None;
                let mut range = if let Some(info) = arg_type {
                    let explicit = self.parse_opt_range()?;
                    Self::net_range(Some(info), explicit)
                        .or_else(|| Some(default_range(info.default_width)))
                } else if self.at_custom_type() {
                    // 类类型形参：按句柄引用别名绑定（见精化器）。
                    arg_type_name = Some(self.parse_custom_type_name()?);
                    Some(default_range(1))
                } else {
                    return Err(format!(
                        "line {}: class method argument requires a type",
                        line
                    ));
                };
                loop {
                    let arg_name = self.ident()?;
                    args.push((arg_name, range.clone()));
                    arg_types.push(arg_type_name.clone());
                    let default = if self.eat_punct("=") {
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    arg_defaults.push(default);
                    if !self.eat_punct(",") {
                        break;
                    }
                    if self.at_kw("input") || self.at_net_type() || self.at_custom_type() {
                        break;
                    }
                    if self.at_punct("[") {
                        range = self.parse_opt_range()?;
                    }
                }
            }
        }
        self.expect_punct(")")?;
        self.expect_punct(";")?;
        let terminator = if is_task { "endtask" } else { "endfunction" };
        let mut body = Vec::new();
        while !self.at_kw(terminator) {
            if self.at_eof() {
                return Err(format!(
                    "line {}: class method '{name}' is missing '{terminator}'",
                    line
                ));
            }
            body.push(self.parse_stmt()?);
        }
        self.expect_kw(terminator)?;
        let _ = self.eat_punct(";");
        Ok(FunctionDef {
            name,
            return_range,
            args,
            arg_defaults,
            arg_types,
            body,
            is_virtual,
            is_static,
        })
    }

    fn parse_class_property(&mut self) -> Result<ClassProperty, String> {
        let line = self.line();
        let mut quals = Vec::new();
        loop {
            let qual = match self.peek_str() {
                "rand" => ClassPropQual::Rand,
                "randc" => ClassPropQual::Randc,
                "static" => ClassPropQual::Static,
                "local" => ClassPropQual::Local,
                "protected" => ClassPropQual::Protected,
                "const" => ClassPropQual::Const,
                _ => break,
            };
            self.bump();
            if quals.contains(&qual) {
                return Err(format!(
                    "line {}: duplicate class property qualifier {:?}",
                    line, qual
                ));
            }
            quals.push(qual);
        }
        let is_string = self.peek_str() == "string";
        let net_type = self.parse_net_type();
        let (type_name, range) = if let Some(info) = net_type {
            let explicit = self.parse_opt_range()?;
            (
                is_string.then(|| "string".to_string()),
                Self::net_range(Some(info), explicit)
                    .or_else(|| Some(default_range(info.default_width))),
            )
        } else if self.at_custom_type() {
            let name = self.parse_custom_type_name()?;
            (Some(name), self.parse_opt_range()?)
        } else {
            return Err(format!("line {}: class property requires a type", line));
        };
        let is_reg = net_type.is_some_and(|info| info.is_variable);
        let is_signed = net_type.is_some_and(|info| info.is_signed);
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
        Ok(ClassProperty {
            quals,
            is_reg,
            is_signed,
            type_name,
            range,
            names,
        })
    }

    fn parse_struct_typedef(&mut self, line: usize) -> Result<Item, String> {
        let is_union = if self.at_kw("struct") {
            self.bump();
            false
        } else {
            self.expect_kw("union")?;
            true
        };
        if !self.eat_kw("packed") {
            return Err(format!(
                "line {}: only packed SystemVerilog structs/unions are supported",
                line
            ));
        }
        self.expect_punct("{")?;
        let mut fields = Vec::new();
        while !self.at_punct("}") {
            if self.at_eof() {
                return Err(format!("line {}: packed struct is missing '}}'", line));
            }
            let net_type = self.parse_net_type();
            let (type_name, range) = if let Some(info) = net_type {
                let explicit_range = self.parse_opt_range()?;
                (
                    None,
                    Self::net_range(Some(info), explicit_range)
                        .or_else(|| Some(default_range(info.default_width))),
                )
            } else if self.at_custom_type() {
                let name = self.parse_custom_type_name()?;
                (Some(name), self.parse_opt_range()?)
            } else {
                return Err(format!(
                    "line {}: packed struct field must have a type",
                    line
                ));
            };
            let field_name = self.ident()?;
            self.expect_punct(";")?;
            fields.push(StructField {
                name: field_name,
                type_name,
                range,
            });
        }
        self.expect_punct("}")?;
        let name = self.ident()?;
        self.expect_punct(";")?;
        if fields.is_empty() {
            return Err(format!(
                "line {}: packed struct typedef '{}' has no fields",
                line, name
            ));
        }
        let definition = StructTypeDef { name, fields };
        let definition = if is_union {
            TypeDef::Union(definition)
        } else {
            TypeDef::Struct(definition)
        };
        Ok(Item::TypeDef(definition))
    }

    fn parse_modport(&mut self, line: usize) -> Result<Item, String> {
        let name = self.ident()?;
        self.expect_punct("(")?;
        let mut ports: Vec<ModportPort> = Vec::new();
        while !self.at_punct(")") {
            if self.at_eof() {
                return Err(format!("line {}: modport '{name}' is missing ')'", line));
            }
            let direction = self.ident_any()?;
            let dir = match direction.as_str() {
                "input" => DirSrc::In,
                "output" => DirSrc::Out,
                "inout" => DirSrc::InOut,
                other => {
                    return Err(format!(
                        "line {}: modport direction must be input, output, or inout, found '{}'",
                        line, other
                    ));
                }
            };
            loop {
                let port_name = self.ident()?;
                if ports.iter().any(|port| port.name == port_name) {
                    return Err(format!(
                        "line {}: modport '{name}' lists '{port_name}' more than once",
                        line
                    ));
                }
                ports.push(ModportPort {
                    name: port_name,
                    dir,
                });
                if !self.eat_punct(",") {
                    break;
                }
                if matches!(self.peek(), Tok::Ident(s) if DIRS.contains(&s.as_str()))
                    || self.at_punct(")")
                {
                    break;
                }
            }
        }
        self.expect_punct(")")?;
        self.expect_punct(";")?;
        Ok(Item::Modport { name, ports })
    }

    fn parse_assert_property(
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

    fn parse_opt_range(&mut self) -> Result<Option<PackedRange>, String> {
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

    fn parse_unpacked_ranges(&mut self, context: &str) -> Result<Vec<Option<PackedRange>>, String> {
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

    fn parse_item(&mut self) -> Result<Item, String> {
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

    fn parse_generate_constructs(&mut self) -> Result<Vec<GenerateConstruct>, String> {
        let mut out = Vec::new();
        while !self.at_kw("endgenerate") {
            if self.at_eof() {
                return Err("generate region is missing 'endgenerate'".into());
            }
            if self.eat_kw("case") {
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

    fn parse_generate_block(&mut self) -> Result<GenerateBlock, String> {
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

    fn parse_opt_delay(&mut self) -> Result<u64, String> {
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

    fn parse_sens(&mut self) -> Result<Sens, String> {
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

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
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
    fn parse_expr_stmt_target(&mut self) -> Result<Expr, String> {
        self.block_le = true;
        let result = self.parse_cond();
        self.block_le = false;
        result
    }

    fn parse_cond(&mut self) -> Result<Expr, String> {
        let c = self.parse_binary(1)?;
        if self.eat_punct("?") {
            let t = self.parse_cond()?;
            self.expect_punct(":")?;
            let f = self.parse_cond()?;
            return Ok(Expr::Cond(Box::new(c), Box::new(t), Box::new(f)));
        }
        Ok(c)
    }

    fn parse_binary(&mut self, min_prec: u8) -> Result<Expr, String> {
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

    fn parse_unary(&mut self) -> Result<Expr, String> {
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

    fn parse_primary(&mut self) -> Result<Expr, String> {
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

fn is_reserved(s: &str) -> bool {
    matches!(
        s,
        "module"
            | "endmodule"
            | "interface"
            | "endinterface"
            | "virtual"
            | "modport"
            | "assert"
            | "property"
            | "disable"
            | "iff"
            | "package"
            | "endpackage"
            | "import"
            | "typedef"
            | "enum"
            | "union"
            | "function"
            | "endfunction"
            | "automatic"
            | "input"
            | "output"
            | "inout"
            | "wire"
            | "reg"
            | "tri"
            | "logic"
            | "bit"
            | "byte"
            | "shortint"
            | "int"
            | "longint"
            | "integer"
            | "signed"
            | "unsigned"
            | "parameter"
            | "localparam"
            | "generate"
            | "endgenerate"
            | "genvar"
            | "assign"
            | "always"
            | "initial"
            | "begin"
            | "end"
            | "if"
            | "else"
            | "case"
            | "casez"
            | "casex"
            | "endcase"
            | "default"
            | "posedge"
            | "negedge"
            | "or"
            | "forever"
            | "and"
            | "nand"
            | "nor"
            | "xor"
            | "xnor"
            | "buf"
            | "not"
            | "always_comb"
            | "always_ff"
    )
}

fn default_range(width: usize) -> PackedRange {
    let msb = Expr::Num {
        width: Some(32),
        word: LogicWord::from_u64(32, width as u64 - 1),
        orig: (width - 1).to_string(),
    };
    let lsb = Expr::Num {
        width: Some(32),
        word: LogicWord::from_u64(32, 0),
        orig: "0".into(),
    };
    (Box::new(msb), Box::new(lsb))
}

fn numeric_class_actual(expression: &Expr) -> Option<i64> {
    match expression {
        Expr::Num { word, .. } => word.to_u64().map(|value| value as i64),
        Expr::Un(UnOpSrc::Minus, inner) => numeric_class_actual(inner).map(|value| -value),
        _ => None,
    }
}

fn symbolic_class_actual(expression: &Expr) -> Option<ClassParameterValue> {
    match expression {
        Expr::Num { .. } => numeric_class_actual(expression).map(ClassParameterValue::Constant),
        Expr::Un(UnOpSrc::Minus, inner) => numeric_class_actual(expression)
            .map(ClassParameterValue::Constant)
            .or_else(|| {
                symbolic_class_actual(inner).map(|value| match value {
                    ClassParameterValue::Constant(value) => ClassParameterValue::Constant(-value),
                    parameter => parameter,
                })
            }),
        Expr::Ident(name) => Some(ClassParameterValue::Parameter(name.clone())),
        _ => None,
    }
}

fn class_specialization_name(base_name: &str, values: &[ClassParameterValue]) -> String {
    let mut name = format!("__eda_class_{base_name}");
    for value in values {
        name.push('_');
        match value {
            ClassParameterValue::Constant(value) => {
                if *value < 0 {
                    name.push('n');
                }
                name.push_str(&value.unsigned_abs().to_string());
            }
            ClassParameterValue::Parameter(parameter) => name.push_str(parameter),
        }
    }
    name
}

fn parse_format(
    fmt: &str,
    default_spec: char,
    args: &[Expr],
    line: usize,
) -> Result<Vec<DispPartSrc>, String> {
    let mut parts = Vec::new();
    let mut lit = String::new();
    let mut argi = 0usize;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            if chars.peek() == Some(&'%') {
                chars.next();
                lit.push('%');
                continue;
            }
            let mut spec = chars.next().unwrap_or(default_spec);
            // 跳过 %0 之类的宽度修饰（本子集无填充语义差异）。
            if spec == '0' {
                spec = chars.next().unwrap_or(default_spec);
            }
            let spec = match spec {
                'b' | 'B' => 'b',
                'd' | 'D' => 'd',
                'h' | 'H' => 'h',
                's' | 'S' => 's',
                't' | 'T' => {
                    parts.push(DispPartSrc::Str(std::mem::take(&mut lit)));
                    parts.push(DispPartSrc::Time);
                    continue;
                }
                _ => default_spec,
            };
            parts.push(DispPartSrc::Str(std::mem::take(&mut lit)));
            parts.push(DispPartSrc::Arg(spec));
            argi += 1;
        } else {
            lit.push(c);
        }
    }
    if !lit.is_empty() {
        parts.push(DispPartSrc::Str(lit));
    }
    if argi != args.len() {
        return Err(format!(
            "line {}: $display has {} format specifiers but {} arguments",
            line,
            argi,
            args.len()
        ));
    }
    Ok(parts)
}

/// Build a sized [`LogicWord`] from a based literal digit string.
pub fn build_word(width: usize, radix: u32, text: &str) -> Result<LogicWord, String> {
    if radix == 10 {
        if text
            .chars()
            .any(|c| matches!(c, 'x' | 'X' | 'z' | 'Z' | '?'))
        {
            return Err("decimal literals cannot contain x/z".into());
        }
        let v: u64 = text
            .parse()
            .map_err(|_| format!("bad decimal literal '{}'", text))?;
        return Ok(LogicWord::from_u64(width, v));
    }
    let mut flat: Vec<LogicValue> = Vec::new(); // big-endian bit list
    for c in text.chars() {
        let n = match radix {
            2 => 1,
            8 => 3,
            16 => 4,
            _ => 1,
        };
        if matches!(c, 'x' | 'X' | 'z' | 'Z' | '?') {
            let v = if c == '?' {
                LogicValue::Z
            } else {
                LogicValue::from_char(c).unwrap()
            };
            for _ in 0..n {
                flat.push(v);
            }
        } else {
            let d = c
                .to_digit(radix)
                .ok_or_else(|| format!("digit '{}' invalid for radix {}", c, radix))?;
            for k in (0..n).rev() {
                flat.push(LogicValue::bit((d >> k) & 1 == 1));
            }
        }
    }
    let mut bits = vec![LogicValue::Zero; width];
    for i in 0..width.min(flat.len()) {
        bits[i] = flat[flat.len() - 1 - i];
    }
    Ok(LogicWord { bits })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> SourceFile {
        parse(src).unwrap_or_else(|e| panic!("parse failed: {}", e))
    }

    #[test]
    fn simple_module() {
        let f = parse_ok("module m(input a, output y); assign y = ~a; endmodule");
        assert_eq!(f.modules.len(), 1);
        assert_eq!(f.modules[0].name, "m");
        assert_eq!(f.modules[0].items.len(), 3); // 2 ports + 1 assign
    }

    #[test]
    fn non_ansi_ports() {
        let f = parse_ok("module m(a, b, y); input a, b; output y; assign y = a & b; endmodule");
        assert_eq!(f.modules[0].items.len(), 4);
    }

    #[test]
    fn unpacked_array_ports() {
        let f =
            parse_ok("module m(input logic [7:0] a [0:1][2:3], output logic b [3:2]); endmodule");
        let ports: Vec<_> = f
            .modules
            .iter()
            .flat_map(|module| module.items.iter())
            .filter_map(|item| match item {
                Item::Port {
                    names,
                    array_ranges,
                    ..
                } => Some((names, array_ranges)),
                _ => None,
            })
            .collect();
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].0, &vec!["a".to_string()]);
        assert_eq!(ports[0].1.len(), 2);
        assert_eq!(ports[1].0, &vec!["b".to_string()]);
        assert_eq!(ports[1].1.len(), 1);
    }

    #[test]
    fn always_and_initial() {
        let f = parse_ok(
            "module tb; reg clk; initial begin clk = 0; #5 clk = 1; #5 $display(\"clk=%b\", clk); $finish; end endmodule",
        );
        match &f.modules[0].items[1] {
            Item::Initial { body } => {
                assert!(matches!(body, Stmt::Block(_)));
            }
            other => panic!("expected initial, got {:?}", other),
        }
    }

    #[test]
    fn precedence() {
        let f = parse_ok("module m(a,b,c,y); assign y = a | b & ~c; endmodule");
        if let Item::Assign { e, .. } = &f.modules[0].items[4] {
            // must be a | (b & (~c)) : top is Or
            assert!(matches!(e, Expr::Bin(BinOpSrc::Or, _, _)));
            let rhs = match e {
                Expr::Bin(_, _, r) => r,
                _ => unreachable!(),
            };
            assert!(matches!(**rhs, Expr::Bin(BinOpSrc::And, _, _)));
        } else {
            panic!("no assign");
        }
    }

    #[test]
    fn indexed_part_selects() {
        let f = parse_ok(
            "module m(input [15:0] a, output [15:0] y); \
             assign y[6 +: 4] = a[11 -: 4]; endmodule",
        );
        match &f.modules[0].items[2] {
            Item::Assign { lv, .. } => {
                assert!(matches!(
                    lv,
                    Expr::IndexedPart {
                        ascending: true,
                        ..
                    }
                ));
            }
            other => panic!("expected assignment, got {:?}", other),
        }
    }

    #[test]
    fn conditional_and_concat() {
        let f = parse_ok("module m(s,a,b,y); assign y = s ? {a, b[3:0]} : 4'b0; endmodule");
        if let Item::Assign { e, .. } = &f.modules[0].items[4] {
            assert!(matches!(e, Expr::Cond(..)));
        } else {
            panic!("no assign");
        }
    }

    #[test]
    fn case_statement() {
        let f = parse_ok(
            "module m(sel, y); always @(*) case (sel) 2'b00, 2'b01: y = 1; default: y = 0; endcase endmodule",
        );
        match &f.modules[0].items[2] {
            Item::Always {
                body: Stmt::Case { arms, dflt, .. },
                ..
            } => {
                assert_eq!(arms.len(), 1);
                assert_eq!(arms[0].0.len(), 2); // two labels
                assert!(dflt.is_some());
            }
            other => panic!("expected case, got {:?}", other),
        }
    }

    #[test]
    fn instance_with_params() {
        let f =
            parse_ok("module top; wire [3:0] s; adder #(4) u1(.a(s), .b(4'd1), .y()); endmodule");
        match &f.modules[0].items[1] {
            Item::Inst {
                module,
                name,
                array_ranges: _,
                params,
                conns,
            } => {
                assert_eq!(module, "adder");
                assert_eq!(name, "u1");
                assert_eq!(params.len(), 1);
                assert_eq!(conns.len(), 3);
                assert!(matches!(conns[2], Conn::Named(_, None)));
            }
            other => panic!("expected inst, got {:?}", other),
        }
    }

    #[test]
    fn gate_primitives() {
        let f = parse_ok("module m(a,b,y); and g1(y, a, b); not (n1, a); endmodule");
        assert!(
            matches!(&f.modules[0].items[3], Item::Gate { kind: GateKind::And, name, .. } if name == "g1")
        );
        assert!(
            matches!(&f.modules[0].items[4], Item::Gate { kind: GateKind::Not, name, .. } if name.is_empty())
        );
    }

    #[test]
    fn syntax_errors_reported() {
        assert!(parse("module m(").is_err());
        assert!(parse("module m begin endmodule").is_err());
        assert!(parse("module m; wire [3:0] w = ; endmodule").is_err());
        assert!(parse("module m; always @(*) y = ; endmodule").is_err());
        assert!(parse("module m; $display(\"%b %b\", a); endmodule").is_err()); // spec/arg count
    }

    #[test]
    fn based_literals_ast() {
        let f = parse_ok("module m(y); assign y = 4'b10xz; endmodule");
        if let Item::Assign { e, .. } = &f.modules[0].items[1] {
            if let Expr::Num { width, word, .. } = e {
                assert_eq!(*width, Some(4));
                assert_eq!(word.to_bin_str(), "10xz");
            } else {
                panic!("not a number");
            }
        }
    }

    #[test]
    fn question_based_digit_is_high_impedance() {
        let word = build_word(4, 2, "10??").unwrap();
        assert_eq!(word.to_bin_str(), "10zz");
    }

    #[test]
    fn replicate() {
        let f = parse_ok("module m(y); assign y = {2{1'b1}}; endmodule");
        if let Item::Assign { e, .. } = &f.modules[0].items[1] {
            assert!(matches!(e, Expr::Replicate(..)));
        }
    }
}
