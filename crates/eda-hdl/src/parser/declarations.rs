//! Interface/module/package/type/function/class declarations.

use super::*;

impl Parser {
    pub(super) fn parse_interface(&mut self) -> Result<Module, String> {
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

    pub(super) fn merge_non_ansi_port_declaration(items: &mut [Item], declaration: &Item) -> bool {
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

    pub(super) fn parse_module(&mut self) -> Result<Module, String> {
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

    pub(super) fn parse_package(&mut self) -> Result<Package, String> {
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

    pub(super) fn parse_import(&mut self) -> Result<Vec<Import>, String> {
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

    pub(super) fn parse_typedef(&mut self) -> Result<Item, String> {
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

    pub(super) fn parse_function(&mut self) -> Result<Item, String> {
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

    pub(super) fn parse_class(&mut self, line: usize, is_virtual: bool) -> Result<Item, String> {
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

    pub(super) fn parse_class_method(&mut self) -> Result<FunctionDef, String> {
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

    pub(super) fn parse_class_property(&mut self) -> Result<ClassProperty, String> {
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

    pub(super) fn parse_struct_typedef(&mut self, line: usize) -> Result<Item, String> {
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

    pub(super) fn parse_modport(&mut self, line: usize) -> Result<Item, String> {
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
}
