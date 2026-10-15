//! SystemVerilog/UVM-lite class object model elaboration methods.

use super::*;

impl<'a> Elaborator<'a> {
    /// Class inheritance chain from base to derived.
    pub(super) fn class_chain(&self, name: &str) -> Result<Vec<String>, String> {
        let mut chain = Vec::new();
        let mut current = name.to_string();
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(current.clone()) {
                return Err(format!("class inheritance cycle through '{}'", current));
            }
            let Some(definition) = self.classes.get(&current) else {
                if chain.is_empty() {
                    return Err(format!("unknown class '{}'", name));
                }
                return Err(format!(
                    "class '{}' extends unknown class '{}'",
                    name, current
                ));
            };
            chain.push(current.clone());
            match &definition.extends {
                Some(base) => current = base.clone(),
                None => break,
            }
        }
        chain.reverse();
        Ok(chain)
    }

    pub(super) fn find_class_field(
        &self,
        class: &str,
        field: &str,
    ) -> Result<Option<(String, &ast::ClassProperty)>, String> {
        let chain = self.class_chain(class)?;
        for owner in chain {
            let definition = &self.classes[&owner];
            for property in &definition.properties {
                if property.names.iter().any(|(name, _)| name == field) {
                    return Ok(Some((owner, property)));
                }
            }
        }
        Ok(None)
    }

    pub(super) fn find_class_method(
        &self,
        class: &str,
        method: &str,
    ) -> Result<Option<(String, &ast::FunctionDef)>, String> {
        let chain = self.class_chain(class)?;
        // Method dispatch starts at the most-derived class.
        for owner in chain.iter().rev() {
            let definition = &self.classes[owner.as_str()];
            for candidate in &definition.methods {
                if candidate.name == method {
                    return Ok(Some((owner.clone(), candidate)));
                }
            }
        }
        Ok(None)
    }

    pub(super) fn static_field_net(class: &str, field: &str) -> String {
        format!("__{class}__{field}")
    }

    pub(super) fn resolve_class_path(
        &self,
        env: &Env<'_>,
        path: &[String],
    ) -> Result<ClassPathTarget, String> {
        let Some(first) = path.first() else {
            return Err("empty class member path".into());
        };
        let (mut prefix, mut class, mut flag_net): (String, String, String);
        if first == "this" || first == "super" {
            let (handle, dynamic_class) = env
                .this_handle
                .ok_or_else(|| format!("'{first}' is valid only inside a class method"))?;
            prefix = env.canon(handle);
            class = dynamic_class.to_string();
            flag_net = prefix.clone();
        } else if let Some(handle_class) = env.class_handles.get(first) {
            let canonical = env.canon(first);
            prefix = canonical.clone();
            class = handle_class.clone();
            flag_net = canonical;
        } else if env.class_aliases.borrow().contains_key(first) {
            let canonical = env.canon(first);
            if let Some(handle_class) = env.class_handles.get(&canonical) {
                prefix = canonical.clone();
                class = handle_class.clone();
                flag_net = canonical;
            } else {
                return Err(format!("'{first}' is not a class handle or class name"));
            }
        } else if self.classes.contains_key(first) {
            // Static class access `ClassName.field`.
            if path.len() != 2 {
                return Err(format!(
                    "class name '{first}' must select exactly one static property"
                ));
            }
            let field = &path[1];
            let (owner, property) = self
                .find_class_field(first, field)?
                .ok_or_else(|| format!("class '{first}' has no property '{field}'"))?;
            if !Self::is_static_qual(property) {
                return Err(format!(
                    "class property '{first}.{field}' is not static; access it through a handle"
                ));
            }
            return Ok(ClassPathTarget::Field(Self::static_field_net(
                &owner, field,
            )));
        } else {
            return Err(format!("'{first}' is not a class handle or class name"));
        }
        for (index, segment) in path.iter().enumerate().skip(1) {
            let is_last = index + 1 == path.len();
            let (owner, property) = self
                .find_class_field(&class, segment)?
                .ok_or_else(|| format!("class '{class}' has no property '{segment}'"))?;
            if Self::is_static_qual(property) {
                if !is_last {
                    return Err(format!(
                        "cannot select further from static property '{segment}'"
                    ));
                }
                return Ok(ClassPathTarget::Field(Self::static_field_net(
                    &owner, segment,
                )));
            }
            if let Some(nested_class) = property.type_name.clone() {
                if self.classes.contains_key(&nested_class) {
                    let raw = format!("{prefix}_{segment}");
                    let canonical = env.canon(&raw);
                    flag_net = if env.class_handles.contains_key(&canonical) {
                        canonical.clone()
                    } else {
                        format!("__{canonical}")
                    };
                    prefix = canonical;
                    class = nested_class;
                    if is_last {
                        return Ok(ClassPathTarget::Handle {
                            flag_net,
                            prefix,
                            class,
                        });
                    }
                    continue;
                }
            }
            if !is_last {
                return Err(format!(
                    "cannot select further from non-class property '{segment}'"
                ));
            }
            return Ok(ClassPathTarget::Field(Self::handle_field_net(
                &prefix, segment,
            )));
        }
        Ok(ClassPathTarget::Handle {
            flag_net,
            prefix,
            class,
        })
    }

    pub(super) fn handle_field_net(handle: &str, field: &str) -> String {
        format!("__{handle}_{field}")
    }

    pub(super) fn is_static_qual(property: &ast::ClassProperty) -> bool {
        property.quals.contains(&ast::ClassPropQual::Static)
    }

    /// Resolve `obj.field` or `this.field` to a class storage net.
    pub(super) fn class_field_reference(
        &self,
        env: &Env<'_>,
        object: &str,
        field: &str,
    ) -> Option<Result<(usize, usize, Range), String>> {
        if object == "this" || object == "super" {
            let (handle, class) = env.this_handle?;
            let handle = env.canon(handle);
            let result = (|| {
                let (owner, property) = self
                    .find_class_field(class, field)?
                    .ok_or_else(|| format!("class '{class}' has no property '{field}'"))?;
                if Self::is_static_qual(property) {
                    let net = Self::static_field_net(&owner, field);
                    let entry = env.net(&net)?;
                    return Ok((entry.base, entry.width, full_range(entry.width)));
                }
                let net = Self::handle_field_net(&handle, field);
                let entry = env.net(&net)?;
                Ok((entry.base, entry.width, full_range(entry.width)))
            })();
            return Some(result);
        }
        if let Some(class) = env.class_handles.get(object) {
            let canonical = env.canon(object);
            let result = (|| {
                let (owner, property) = self
                    .find_class_field(class, field)?
                    .ok_or_else(|| format!("class '{class}' has no property '{field}'"))?;
                if Self::is_static_qual(property) {
                    let net = Self::static_field_net(&owner, field);
                    let entry = env.net(&net)?;
                    return Ok((entry.base, entry.width, full_range(entry.width)));
                }
                let net = Self::handle_field_net(&canonical, field);
                let entry = env.net(&net)?;
                Ok((entry.base, entry.width, full_range(entry.width)))
            })();
            return Some(result);
        }
        if let Some(definition) = self.classes.get(object) {
            // Static access `ClassName.field`.
            let class = definition.name.clone();
            let result = (|| {
                let chain = self.class_chain(&class)?;
                for owner in chain {
                    let owner_definition = &self.classes[&owner];
                    for property in &owner_definition.properties {
                        if !Self::is_static_qual(property) {
                            continue;
                        }
                        if property.names.iter().any(|(name, _)| name == field) {
                            let net = Self::static_field_net(&owner, field);
                            let entry = env.net(&net)?;
                            return Ok((entry.base, entry.width, full_range(entry.width)));
                        }
                    }
                }
                Err(format!("class '{class}' has no static property '{field}'"))
            })();
            return Some(result);
        }
        None
    }

    pub(super) fn resize_class_arg(&self, value: ExprIR, width: usize) -> ExprIR {
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

    /// 类类型形式参按引用绑定：形参名在方法体内别到实参句柄的组代表。
    /// 本子集在方法体内联期间保持绑定（形参名与用户句柄重名时以形参为准）。
    pub(super) fn bind_class_formals(
        &self,
        definition: &ast::FunctionDef,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<(), String> {
        let actuals = self.expand_function_actuals(definition, actuals)?;
        for (((formal, _), type_name), actual) in definition
            .args
            .iter()
            .zip(&definition.arg_types)
            .zip(&actuals)
        {
            let Some(class_name) = type_name else {
                continue;
            };
            if !self.classes.contains_key(class_name) {
                continue;
            }
            // 实参可以是顶层句柄，也可以是嵌套路径（如 env.mon）。
            let canonical = match actual {
                ast::Expr::Ident(handle) => {
                    if !env.class_handles.contains_key(handle) {
                        return Err(format!(
                            "class-typed argument '{}' of '{}' must name a class handle",
                            formal, definition.name
                        ));
                    }
                    env.canon(handle)
                }
                ast::Expr::Member(object, field) => {
                    let path = vec![object.clone(), field.clone()];
                    match self.resolve_class_path(env, &path) {
                        Ok(ClassPathTarget::Handle { prefix, .. }) => prefix,
                        _ => {
                            return Err(format!(
                                "class-typed argument '{}' of '{}' must name a class handle",
                                formal, definition.name
                            ))
                        }
                    }
                }
                ast::Expr::MemberChain(path) => match self.resolve_class_path(env, path) {
                    Ok(ClassPathTarget::Handle { prefix, .. }) => prefix,
                    _ => {
                        return Err(format!(
                            "class-typed argument '{}' of '{}' must name a class handle",
                            formal, definition.name
                        ))
                    }
                },
                _ => {
                    return Err(format!(
                        "class-typed argument '{}' of '{}' must be a handle",
                        formal, definition.name
                    ))
                }
            };
            env.class_aliases
                .borrow_mut()
                .insert(formal.clone(), canonical.clone());
        }
        Ok(())
    }

    pub(super) fn expand_function_actuals(
        &self,
        definition: &ast::FunctionDef,
        actuals: &[ast::Expr],
    ) -> Result<Vec<ast::Expr>, String> {
        if actuals.len() != definition.args.len()
            && definition.arg_defaults.iter().all(Option::is_none)
        {
            return Err(format!(
                "method '{}' expects {} argument(s), received {}",
                definition.name,
                definition.args.len(),
                actuals.len()
            ));
        }
        if actuals.len() > definition.args.len() {
            return Err(format!(
                "method '{}' expects at most {} argument(s), received {}",
                definition.name,
                definition.args.len(),
                actuals.len()
            ));
        }
        let mut expanded = actuals.to_vec();
        for index in actuals.len()..definition.args.len() {
            let Some(default) = definition.arg_defaults.get(index).and_then(Option::as_ref) else {
                return Err(format!(
                    "method '{}' has no default for argument {}",
                    definition.name,
                    index + 1
                ));
            };
            expanded.push(default.clone());
        }
        Ok(expanded)
    }

    pub(super) fn class_actuals(
        &self,
        definition: &ast::FunctionDef,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<HashMap<String, ExprIR>, String> {
        let actuals = self.expand_function_actuals(definition, actuals)?;
        let mut bound = HashMap::new();
        for ((formal, range), actual) in definition.args.iter().zip(&actuals) {
            let width = self.ast_range_width(range, env)?;
            let value = self.elab_expr(actual, env)?;
            bound.insert(formal.clone(), self.resize_class_arg(value, width));
        }
        Ok(bound)
    }

    /// Declare storage for every instance field of `handle`'s class and for
    /// every static field of every visible class. Returns synthetic initial
    /// items that seed static-field initializers.
    pub(super) fn declare_class_storage(
        &mut self,
        prefix: &str,
        params: &HashMap<String, i64>,
        module_name: &str,
        nets: &mut HashMap<String, NetEntry>,
        static_storage_classes: &HashSet<String>,
    ) -> Result<Vec<ast::Item>, String> {
        let _ = params;
        let mut initials = Vec::new();
        let class_names: Vec<String> = self.classes.keys().cloned().collect();
        for class in class_names {
            if !static_storage_classes.contains(&class) {
                continue;
            }
            let chain = self.class_chain(&class)?;
            let mut static_initials = Vec::new();
            for owner in &chain {
                let definition = self.classes[owner].clone();
                for property in &definition.properties {
                    if !Self::is_static_qual(property) {
                        continue;
                    }
                    for (field, init) in &property.names {
                        let net_name = Self::static_field_net(owner, field);
                        let range = property.range.clone();
                        let entry = self.declare_net(
                            &net_name,
                            prefix,
                            &range,
                            &HashMap::new(),
                            true,
                            property.is_signed,
                            None,
                            property.type_name.clone().as_deref(),
                        )?;
                        nets.insert(net_name.clone(), entry.clone());
                        let value = init.clone().unwrap_or_else(|| ast::Expr::Num {
                            width: Some(entry.width),
                            word: LogicWord::zero(entry.width),
                            orig: "0".to_string(),
                        });
                        static_initials.push(ast::Stmt::BL {
                            lv: ast::Expr::Ident(net_name),
                            e: value,
                        });
                    }
                }
            }
            if !static_initials.is_empty() {
                initials.push(ast::Item::Initial {
                    body: ast::Stmt::Block(static_initials),
                });
            }
        }
        let _ = module_name;
        Ok(initials)
    }

    /// Declare the flattened per-object fields of one class handle.
    pub(super) fn declare_handle_fields(
        &mut self,
        handle_prefix: &str,
        class: &str,
        prefix: &str,
        params: &HashMap<String, i64>,
        nets: &mut HashMap<String, NetEntry>,
        depth: usize,
    ) -> Result<(), String> {
        let _ = params;
        if depth > 8 {
            return Err(format!(
                "class composition nesting under '{handle_prefix}' exceeds 8 levels"
            ));
        }
        // 运行时类型标记（虚分派用）。
        let tid_name = type_id_net(handle_prefix);
        if !nets.contains_key(&tid_name) {
            let width = type_id_width(self.classes.len()).max(1);
            let range = Some((
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::from_u64(32, width as u64 - 1),
                    orig: (width - 1).to_string(),
                }),
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::zero(32),
                    orig: "0".to_string(),
                }),
            ));
            let entry = self.declare_net(
                &tid_name,
                prefix,
                &range,
                &HashMap::new(),
                true,
                false,
                None,
                None,
            )?;
            nets.insert(tid_name.clone(), entry);
        }
        let oid_name = object_id_net(handle_prefix);
        if nets.get(&oid_name).is_none() {
            let range = Some((
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::from_u64(32, 31),
                    orig: "31".to_string(),
                }),
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::zero(32),
                    orig: "0".to_string(),
                }),
            ));
            let entry = self.declare_net(
                &oid_name,
                prefix,
                &range,
                &HashMap::new(),
                true,
                false,
                None,
                None,
            )?;
            nets.insert(oid_name, entry);
        }
        let chain = self.class_chain(class)?;
        for owner in chain {
            let definition = self.classes[&owner].clone();
            for property in &definition.properties {
                if Self::is_static_qual(property) {
                    continue;
                }
                for (field, _init) in &property.names {
                    let net_name = Self::handle_field_net(handle_prefix, field);
                    if !nets.contains_key(&net_name) {
                        let entry = self.declare_net(
                            &net_name,
                            prefix,
                            &property.range.clone(),
                            &HashMap::new(),
                            true,
                            property.is_signed,
                            None,
                            property.type_name.clone().as_deref(),
                        )?;
                        nets.insert(net_name.clone(), entry);
                    }
                    if let Some(nested_class) = property.type_name.clone() {
                        if self.classes.contains_key(&nested_class) {
                            // A directly self-referential class property is a
                            // handle reference, not recursive inline storage.
                            if nested_class == class {
                                continue;
                            }
                            let nested_prefix = format!("{handle_prefix}_{field}");
                            self.declare_handle_fields(
                                &nested_prefix,
                                &nested_class,
                                prefix,
                                params,
                                nets,
                                depth + 1,
                            )?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// `h = new(args);` — allocate fields and run the constructor chain.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn class_shallow_copy_statement(
        &mut self,
        destination: &str,
        source: &str,
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let source_canonical = env.canon(source);
        let source_class = env
            .class_handles
            .get(&source_canonical)
            .or_else(|| env.class_handles.get(source))
            .ok_or_else(|| format!("'{source}' is not a class handle"))?
            .clone();
        let destination_class = env
            .class_handles
            .get(destination)
            .cloned()
            .ok_or_else(|| format!("'{destination}' is not a class handle"))?;
        if source_class != destination_class {
            return Err(format!(
                "class shallow copy in this subset requires the exact source class '{}', not '{}'",
                destination_class, source_class
            ));
        }

        let promotion = self.detach_class_handle_alias(env, destination);
        let mut statements = Vec::new();
        if let Some((old_handle, promoted_handle)) = promotion {
            statements.extend(self.promote_class_handle_storage(
                env,
                &old_handle,
                &promoted_handle,
            ));
        }

        let destination_entry = env.net(destination)?;
        let assignment = |lv: LValue, value: ExprIR| -> StmtIR {
            if nonblocking {
                StmtIR::NBl { lv, e: value }
            } else {
                StmtIR::Bl { lv, e: value }
            }
        };
        let lvalue = |entry: &NetEntry| LValue {
            net: entry.base,
            decl_width: entry.width,
            range: full_range(entry.width),
        };
        let reference = |entry: &NetEntry| ExprIR::Ref {
            net: entry.base,
            decl_width: entry.width,
            range: full_range(entry.width),
            signed: false,
        };

        statements.push(assignment(
            lvalue(destination_entry),
            ExprIR::Const(LogicWord::ones(destination_entry.width)),
        ));
        let object_id = self.next_object_id;
        self.next_object_id = self
            .next_object_id
            .checked_add(1)
            .ok_or_else(|| "class object identity counter overflowed".to_string())?;
        if let Ok(destination_oid) = env.net(&object_id_net(destination)) {
            statements.push(assignment(
                lvalue(destination_oid),
                ExprIR::Const(LogicWord::from_u64(destination_oid.width, object_id)),
            ));
        }
        if let (Ok(destination_tid), Ok(source_tid)) = (
            env.net(&type_id_net(destination)),
            env.net(&type_id_net(&source_canonical)),
        ) {
            statements.push(assignment(lvalue(destination_tid), reference(source_tid)));
        }

        let mut property_aliases = Vec::new();
        for owner in self.class_chain(&source_class)? {
            let definition = self.classes[&owner].clone();
            for property in &definition.properties {
                if Self::is_static_qual(property) {
                    continue;
                }
                for (field, _) in &property.names {
                    if property
                        .type_name
                        .as_deref()
                        .is_some_and(|class| self.classes.contains_key(class))
                    {
                        let source_property = format!("{source_canonical}_{field}");
                        let source_property = env.canon(&source_property);
                        let destination_property = format!("{destination}_{field}");
                        property_aliases.push((destination_property, source_property));
                    } else {
                        let source_name = Self::handle_field_net(&source_canonical, field);
                        let destination_name = Self::handle_field_net(destination, field);
                        let (Some(source_entry), Some(destination_entry)) =
                            (env.nets.get(&source_name), env.nets.get(&destination_name))
                        else {
                            return Err(format!(
                                "missing class copy storage '{source_name}' or '{destination_name}'"
                            ));
                        };
                        statements.push(assignment(
                            lvalue(destination_entry),
                            reference(source_entry),
                        ));
                    }
                }
            }
        }
        if !property_aliases.is_empty() {
            let mut aliases = env.class_aliases.borrow_mut();
            for (destination_property, source_property) in property_aliases {
                aliases.insert(destination_property, source_property);
            }
        }
        Ok(StmtIR::Block(statements))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn class_new_statement(
        &mut self,
        handle: &str,
        field_prefix: &str,
        handle_class: &str,
        actuals: &[ast::Expr],
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let class = handle_class.to_string();
        // new 让该句柄指向新对象：若它原是别名组代表，则提升另一成员。
        let promotion = self.detach_class_handle_alias(env, handle);
        let mut statements = Vec::new();
        if let Some((old_handle, promoted_handle)) = promotion {
            statements.extend(self.promote_class_handle_storage(
                env,
                &old_handle,
                &promoted_handle,
            ));
        }
        let canonical = env.canon(handle);
        let handle = canonical.as_str();
        let handle_entry = env.net(handle)?;
        let make = |net: usize, width: usize, value: ExprIR| {
            if nonblocking {
                StmtIR::NBl {
                    lv: LValue {
                        net,
                        decl_width: width,
                        range: full_range(width),
                    },
                    e: value,
                }
            } else {
                StmtIR::Bl {
                    lv: LValue {
                        net,
                        decl_width: width,
                        range: full_range(width),
                    },
                    e: value,
                }
            }
        };
        // Mark the handle allocated and record its runtime type id.
        statements.push(make(
            handle_entry.base,
            handle_entry.width,
            ExprIR::Const(LogicWord::ones(handle_entry.width)),
        ));
        let object_id = self.next_object_id;
        self.next_object_id = self
            .next_object_id
            .checked_add(1)
            .ok_or_else(|| "class object identity counter overflowed".to_string())?;
        if let Ok(oid_entry) = env.net(&object_id_net(field_prefix)) {
            statements.push(make(
                oid_entry.base,
                oid_entry.width,
                ExprIR::Const(LogicWord::from_u64(oid_entry.width, object_id)),
            ));
        }
        let ids = class_type_ids(&self.classes);
        let Some(&type_id) = ids.get(&class) else {
            return Err(format!("unknown class '{class}'"));
        };
        if let Ok(tid_entry) = env.net(&type_id_net(field_prefix)) {
            statements.push(make(
                tid_entry.base,
                tid_entry.width,
                ExprIR::Const(LogicWord::from_u64(tid_entry.width, type_id as u64)),
            ));
        }
        let chain = self.class_chain(&class)?;
        for owner in &chain {
            let definition = self.classes[owner].clone();
            // Property initializers execute before the constructor body.
            for property in &definition.properties {
                if Self::is_static_qual(property) {
                    continue;
                }
                for (field, init) in &property.names {
                    let net_name = Self::handle_field_net(field_prefix, field);
                    let entry = env.net(&net_name)?;
                    let value = match init {
                        Some(expression) => {
                            let empty = HashMap::new();
                            let child = self.class_env(
                                env,
                                field_prefix,
                                class.as_str(),
                                None,
                                &empty,
                                None,
                            );
                            self.elab_expr(expression, &child)?
                        }
                        None => ExprIR::Const(LogicWord::zero(entry.width)),
                    };
                    let value = self.resize_class_arg(value, entry.width);
                    statements.push(make(entry.base, entry.width, value));
                }
            }
            // Run this class's constructor if present.
            if let Some(constructor) = definition
                .methods
                .iter()
                .find(|method| method.name == "new")
                .cloned()
            {
                // If the immediately derived class explicitly invokes
                // `super.new(...)`, skip the implicit base-constructor run to
                // avoid constructing the base twice.
                let derived_ctor_calls_super = chain
                    .iter()
                    .position(|c| c == owner)
                    .and_then(|index| chain.get(index + 1))
                    .and_then(|derived| self.classes.get(derived))
                    .and_then(|derived_definition| {
                        derived_definition
                            .methods
                            .iter()
                            .find(|method| method.name == "new")
                    })
                    .is_some_and(|derived_ctor| ctor_body_calls_super(&derived_ctor.body));
                if derived_ctor_calls_super && owner != &class {
                    continue;
                }
                let ctor_actuals: Vec<ast::Expr> = if owner == &class {
                    actuals.to_vec()
                } else {
                    // Implicit base construction uses the actuals that the
                    // immediately derived class forwards via `extends`.
                    chain
                        .iter()
                        .position(|c| c == owner)
                        .and_then(|index| chain.get(index + 1))
                        .and_then(|derived| self.classes.get(derived))
                        .map(|derived_definition| derived_definition.extends_args.clone())
                        .unwrap_or_default()
                };
                self.bind_class_formals(&constructor, &ctor_actuals, env)?;
                let bound = self.class_actuals(&constructor, &ctor_actuals, env)?;
                let child = self.class_env(
                    env,
                    field_prefix,
                    class.as_str(),
                    Some(owner.as_str()),
                    &bound,
                    None,
                );
                for statement in &constructor.body {
                    let ir = self.elab_stmt(statement, &child)?;
                    statements.push(ir);
                }
            }
        }
        Ok(StmtIR::Block(statements))
    }

    pub(super) fn class_env<'b, 'c>(
        &self,
        parent: &Env<'b>,
        handle: &'c str,
        class: &'c str,
        owner: Option<&'c str>,
        args: &'c HashMap<String, ExprIR>,
        method_return: Option<(&'c str, usize, usize)>,
    ) -> Env<'c>
    where
        'b: 'c,
    {
        Env {
            class_aliases: std::rc::Rc::clone(&parent.class_aliases),
            nets: parent.nets,
            interface_ports: parent.interface_ports,
            interface_directions: parent.interface_directions,
            params: parent.params,
            types: parent.types,
            functions: parent.functions,
            module_name: parent.module_name,
            prefix: parent.prefix,
            class_handles: parent.class_handles,
            this_handle: Some((handle, class)),
            method_owner: owner,
            method_args: args,
            method_return,
        }
    }

    pub(super) fn run_class_constructor(
        &mut self,
        handle: &str,
        dynamic_class: &str,
        owner: &str,
        constructor: &ast::FunctionDef,
        arguments: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<Vec<StmtIR>, String> {
        self.bind_class_formals(constructor, arguments, env)?;
        let bound = self.class_actuals(constructor, arguments, env)?;
        let child = self.class_env(env, handle, dynamic_class, Some(owner), &bound, None);
        let mut statements = Vec::new();
        for statement in &constructor.body {
            statements.push(self.elab_stmt(statement, &child)?);
        }
        Ok(statements)
    }

    /// Statement-form method call `h.m(args);` (or `this.m`/`super.m`).
    pub(super) fn class_method_statement(
        &mut self,
        object: &str,
        method: &str,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let (handle, class, owner_hint) = if object == "this" || object == "super" {
            let (handle, class) = env
                .this_handle
                .ok_or_else(|| "'{object}' is valid only inside a class method".to_string())?;
            let start = if object == "super" {
                env.method_owner
                    .and_then(|owner| self.classes.get(owner))
                    .and_then(|definition| definition.extends.clone())
                    .ok_or_else(|| "super call has no base class".to_string())?
            } else {
                class.to_string()
            };
            (handle.to_string(), class.to_string(), start)
        } else {
            let class = env
                .class_handles
                .get(object)
                .cloned()
                .ok_or_else(|| format!("'{object}' is not a class handle"))?;
            (object.to_string(), class.clone(), class)
        };
        self.class_method_statement_on(
            handle.as_str(),
            class.as_str(),
            owner_hint.as_str(),
            object == "super",
            method,
            actuals,
            env,
        )
    }

    #[allow(clippy::too_many_arguments)]
    /// If `class::method` is virtual, build a runtime-type dispatch over the
    /// handle's type-id net. Each arm resolves the most-derived override for
    /// one concrete derived class (overrides may reference base fields only).
    #[allow(clippy::type_complexity)]
    pub(super) fn virtual_dispatch_arms(
        &self,
        _handle: &str,
        class: &str,
        method: &str,
    ) -> Result<Option<(usize, Vec<(u32, String)>)>, String> {
        let Some((_, base_def)) = self.find_class_method(class, method)? else {
            return Ok(None);
        };
        if !base_def.is_virtual {
            return Ok(None);
        }
        let ids = class_type_ids(&self.classes);
        let tid_width = type_id_width(self.classes.len()).max(1);
        let mut arms = Vec::new();
        for derived in derived_classes(&self.classes, class) {
            let Some((owner, _)) = self.find_class_method(derived, method)? else {
                continue;
            };
            let Some(&id) = ids.get(derived) else {
                continue;
            };
            arms.push((id, owner.to_string()));
        }
        Ok(Some((tid_width, arms)))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn virtual_dispatch_statement(
        &mut self,
        handle: &str,
        class: &str,
        method: &str,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<Option<StmtIR>, String> {
        let Some((tid_width, arms)) = self.virtual_dispatch_arms(handle, class, method)? else {
            return Ok(None);
        };
        let canonical = env.canon(handle);
        let tid_net = type_id_net(&canonical);
        let Ok(tid_entry) = env.net(&tid_net) else {
            return Ok(None);
        };
        let _ = tid_width;
        let mut cases = Vec::new();
        for (type_id, owner) in arms {
            let Some(definition) = self
                .classes
                .get(owner.as_str())
                .and_then(|d| d.methods.iter().find(|m| m.name == method))
            else {
                continue;
            };
            let definition = definition.clone();
            self.bind_class_formals(&definition, actuals, env)?;
            let bound = self.class_actuals(&definition, actuals, env)?;
            let child = self.class_env(env, &canonical, &owner, Some(&owner), &bound, None);
            let mut body = Vec::new();
            for statement in &definition.body {
                body.push(self.elab_stmt(statement, &child)?);
            }
            cases.push((
                ExprIR::Const(LogicWord::from_u64(tid_entry.width, type_id as u64)),
                StmtIR::Block(body),
            ));
        }
        Ok(Some(StmtIR::Case {
            e: ExprIR::Ref {
                net: tid_entry.base,
                decl_width: tid_entry.width,
                range: full_range(tid_entry.width),
                signed: false,
            },
            arms: cases,
            dflt: Some(Box::new(StmtIR::Block(Vec::new()))),
        }))
    }

    pub(super) fn virtual_dispatch_expression(
        &self,
        handle: &str,
        class: &str,
        method: &str,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<Option<ExprIR>, String> {
        // 表达式形式的虚分派需要每个候选体是单赋值表达式；用嵌套
        // 条件表达式按 tid 选择。
        let Some((_, arms)) = self.virtual_dispatch_arms(handle, class, method)? else {
            return Ok(None);
        };
        if arms.is_empty() {
            return Ok(None);
        }
        let canonical = env.canon(handle);
        let tid_net = type_id_net(&canonical);
        let Ok(tid_entry) = env.net(&tid_net) else {
            return Ok(None);
        };
        let selector = ExprIR::Ref {
            net: tid_entry.base,
            decl_width: tid_entry.width,
            range: full_range(tid_entry.width),
            signed: false,
        };
        let mut result: Option<ExprIR> = None;
        for (type_id, owner) in arms.iter().rev() {
            let Some(definition) = self
                .classes
                .get(owner.as_str())
                .and_then(|d| d.methods.iter().find(|m| m.name == method))
            else {
                continue;
            };
            let mut assigned = None;
            for statement in &definition.body {
                match statement {
                    ast::Stmt::BL {
                        lv: ast::Expr::Ident(name),
                        e,
                    } if name == &definition.name => assigned = Some(e),
                    _ => return Err(format!(
                        "virtual method '{}.{}' used in an expression must be a single result assignment",
                        class, method
                    )),
                }
            }
            let Some(expression) = assigned else {
                return Err(format!(
                    "virtual method '{}.{}' used in an expression must assign its result",
                    class, method
                ));
            };
            self.bind_class_formals(definition, actuals, env)?;
            let bound = self.class_actuals(definition, actuals, env)?;
            let child = self.class_env(env, &canonical, owner, Some(owner), &bound, None);
            let value = self.elab_expr(expression, &child)?;
            let cond = ExprIR::Bin {
                op: BinOp::Eq,
                a: Box::new(selector.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(
                    tid_entry.width,
                    *type_id as u64,
                ))),
            };
            result = Some(match result {
                Some(else_value) => ExprIR::Cond {
                    c: Box::new(cond),
                    t: Box::new(value),
                    f: Box::new(else_value),
                },
                None => value,
            });
        }
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn class_method_statement_on(
        &mut self,
        handle: &str,
        class: &str,
        owner_hint: &str,
        from_super: bool,
        method: &str,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        // 虚方法经句柄调用：按运行时类型标记分派（super. 显式静态绑定）。
        if !from_super {
            if let Some(dispatch) =
                self.virtual_dispatch_statement(handle, class, method, actuals, env)?
            {
                return Ok(dispatch);
            }
        }
        if method == "new" {
            if !from_super {
                return Err(
                    "constructors are invoked by assignment, as in 'h = new(args);'".to_string(),
                );
            }
            // Explicit `super.new(args);` inside a derived constructor.
            let base = owner_hint;
            let Some((base_owner, base_ctor)) = self.find_class_method(base, "new")? else {
                return Ok(StmtIR::Block(Vec::new()));
            };
            let base_ctor = base_ctor.clone();
            let base_owner = base_owner.clone();
            let statements =
                self.run_class_constructor(handle, class, &base_owner, &base_ctor, actuals, env)?;
            return Ok(StmtIR::Block(statements));
        }
        let Some((owner, definition)) = self.find_class_method(owner_hint, method)? else {
            return Err(format!("class '{class}' has no method '{method}'"));
        };
        let definition = definition.clone();
        let owner = owner.clone();
        self.bind_class_formals(&definition, actuals, env)?;
        let bound = self.class_actuals(&definition, actuals, env)?;
        // Value-returning methods invoked as statements write their result to
        // a scratch net so return assignments still elaborate.
        let return_width = self
            .ast_range_width(&definition.return_range, env)
            .unwrap_or(32);
        let scratch_name = format!("__method_ret_{}_{}", handle, method);
        let scratch = self.declare_net(
            &scratch_name,
            env.prefix,
            &Some((
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::from_u64(32, return_width as u64 - 1),
                    orig: (return_width - 1).to_string(),
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
        )?;
        // 引用别名后，句柄的运行时类取其组代表句柄的声明类（嵌套
        // 前缀无直接表项时回退到静态类）。
        let canonical_handle = env.canon(handle);
        let dynamic_class = env
            .class_handles
            .get(&canonical_handle)
            .map(String::as_str)
            .unwrap_or(class);
        let child = self.class_env(
            env,
            handle,
            dynamic_class,
            Some(owner.as_str()),
            &bound,
            Some((definition.name.as_str(), scratch.base, scratch.width)),
        );
        let mut statements = Vec::new();
        for statement in &definition.body {
            statements.push(self.elab_stmt(statement, &child)?);
        }
        Ok(StmtIR::Block(statements))
    }

    /// Expression-form method call `h.m(args)` limited to methods whose body
    /// assigns the method name exactly once from a single expression.
    pub(super) fn class_method_expression_on(
        &self,
        handle: &str,
        class: &str,
        owner_hint: &str,
        method: &str,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        if let Some(dispatch) =
            self.virtual_dispatch_expression(handle, class, method, actuals, env)?
        {
            return Ok(dispatch);
        }
        if method == "new" {
            return Err(
                "constructors are invoked by assignment, as in 'h = new(args);'".to_string(),
            );
        }
        let Some((owner, definition)) = self.find_class_method(owner_hint, method)? else {
            return Err(format!("class '{class}' has no method '{method}'"));
        };
        let mut assigned = None;
        for statement in &definition.body {
            match statement {
                ast::Stmt::BL {
                    lv: ast::Expr::Ident(name),
                    e,
                } if name == &definition.name => {
                    if assigned.is_some() {
                        return Err(format!(
                            "class method '{}.{}' used in an expression must assign its result exactly once",
                            class, method
                        ));
                    }
                    assigned = Some(e);
                }
                _ => {
                    return Err(format!(
                    "class method '{}.{}' used in an expression must be a single result assignment",
                    class, method
                ))
                }
            }
        }
        let expression = assigned.ok_or_else(|| {
            format!(
                "class method '{}.{}' used in an expression must assign its result",
                class, method
            )
        })?;
        self.bind_class_formals(definition, actuals, env)?;
        let bound = self.class_actuals(definition, actuals, env)?;
        let child = self.class_env(env, handle, class, Some(owner.as_str()), &bound, None);
        self.elab_expr(expression, &child)
    }

    pub(super) fn class_method_expression(
        &self,
        object: &str,
        method: &str,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        let (handle, class, owner_hint) = if object == "this" || object == "super" {
            let (handle, class) = env
                .this_handle
                .ok_or_else(|| format!("'{object}' is valid only inside a class method"))?;
            let start = if object == "super" {
                env.method_owner
                    .and_then(|owner| self.classes.get(owner))
                    .and_then(|definition| definition.extends.clone())
                    .ok_or_else(|| "super call has no base class".to_string())?
            } else {
                class.to_string()
            };
            (handle.to_string(), class.to_string(), start)
        } else {
            let class = env
                .class_handles
                .get(object)
                .cloned()
                .ok_or_else(|| format!("'{object}' is not a class handle"))?;
            (object.to_string(), class.clone(), class)
        };
        self.class_method_expression_on(&handle, &class, &owner_hint, method, actuals, env)
    }

    pub(super) fn class_static_method_statement(
        &mut self,
        class: &str,
        method: &str,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let Some((owner, definition)) = self.find_class_method(class, method)? else {
            return Err(format!("class '{class}' has no method '{method}'"));
        };
        if !definition.is_static {
            return Err(format!(
                "non-static method '{}.{}' must be called through a class handle",
                class, method
            ));
        }
        let definition = definition.clone();
        let owner = owner.clone();
        self.bind_class_formals(&definition, actuals, env)?;
        let bound = self.class_actuals(&definition, actuals, env)?;
        let owner_name = owner.clone();
        let child = Env {
            class_aliases: std::rc::Rc::clone(&env.class_aliases),
            nets: env.nets,
            interface_ports: env.interface_ports,
            interface_directions: env.interface_directions,
            params: env.params,
            types: env.types,
            functions: env.functions,
            module_name: env.module_name,
            prefix: env.prefix,
            class_handles: env.class_handles,
            this_handle: None,
            method_owner: Some(owner_name.as_str()),
            method_args: &bound,
            method_return: None,
        };
        let mut statements = Vec::new();
        for statement in &definition.body {
            statements.push(self.elab_stmt(statement, &child)?);
        }
        Ok(StmtIR::Block(statements))
    }

    pub(super) fn class_static_method_value_assignment(
        &mut self,
        target: LValue,
        class: &str,
        method: &str,
        actuals: &[ast::Expr],
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let Some((owner, definition)) = self.find_class_method(class, method)? else {
            return Err(format!("class '{class}' has no method '{method}'"));
        };
        if !definition.is_static {
            return Err(format!(
                "non-static method '{}.{}' must be called through a class handle",
                class, method
            ));
        }
        if definition.return_range.is_none() {
            return Err(format!(
                "void static method '{}.{}' cannot be used as a value",
                class, method
            ));
        }
        let definition = definition.clone();
        let owner = owner.clone();
        let return_width = self
            .ast_range_width(&definition.return_range, env)
            .unwrap_or(32);
        let scratch_name = format!("__static_method_ret_{class}_{method}");
        let scratch = self.declare_net(
            &scratch_name,
            env.prefix,
            &Some((
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::from_u64(32, return_width as u64 - 1),
                    orig: (return_width - 1).to_string(),
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
        )?;
        self.bind_class_formals(&definition, actuals, env)?;
        let bound = self.class_actuals(&definition, actuals, env)?;
        let owner_name = owner.clone();
        let method_name = definition.name.clone();
        let child = Env {
            class_aliases: std::rc::Rc::clone(&env.class_aliases),
            nets: env.nets,
            interface_ports: env.interface_ports,
            interface_directions: env.interface_directions,
            params: env.params,
            types: env.types,
            functions: env.functions,
            module_name: env.module_name,
            prefix: env.prefix,
            class_handles: env.class_handles,
            this_handle: None,
            method_owner: Some(owner_name.as_str()),
            method_args: &bound,
            method_return: Some((method_name.as_str(), scratch.base, scratch.width)),
        };
        let mut statements = Vec::new();
        for statement in &definition.body {
            statements.push(self.elab_stmt(statement, &child)?);
        }
        let result = ExprIR::Ref {
            net: scratch.base,
            decl_width: scratch.width,
            range: full_range(scratch.width),
            signed: false,
        };
        let assignment = if nonblocking {
            StmtIR::NBl {
                lv: target,
                e: result,
            }
        } else {
            StmtIR::Bl {
                lv: target,
                e: result,
            }
        };
        statements.push(assignment);
        Ok(StmtIR::Block(statements))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn class_method_value_assignment(
        &mut self,
        lv: &ast::Expr,
        handle: &str,
        class: &str,
        owner_hint: &str,
        from_super: bool,
        method: &str,
        actuals: &[ast::Expr],
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let Some((owner, definition)) = self.find_class_method(owner_hint, method)? else {
            return Err(format!("class '{class}' has no method '{method}'"));
        };
        let definition = definition.clone();
        let owner = owner.clone();
        if definition.return_range.is_none() {
            return Err(format!(
                "void class method '{}.{}' cannot be used as a value",
                class, method
            ));
        }

        let target = self.elab_lvalue(lv, env)?;
        let return_width = self
            .ast_range_width(&definition.return_range, env)
            .unwrap_or(32);
        let scratch_name = format!("__method_value_{}_{}", handle, method);
        let scratch = self.declare_net(
            &scratch_name,
            env.prefix,
            &Some((
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::from_u64(32, return_width as u64 - 1),
                    orig: (return_width - 1).to_string(),
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
        )?;
        let result = ExprIR::Ref {
            net: scratch.base,
            decl_width: scratch.width,
            range: full_range(scratch.width),
            signed: false,
        };

        let body = if !from_super && definition.is_virtual {
            match self.virtual_dispatch_value_body(
                handle,
                class,
                method,
                actuals,
                scratch.base,
                scratch.width,
                env,
            )? {
                Some(body) => body,
                None => {
                    return Err(format!(
                        "virtual method '{}.{}' has no runtime type identity",
                        class, method
                    ))
                }
            }
        } else {
            self.bind_class_formals(&definition, actuals, env)?;
            let bound = self.class_actuals(&definition, actuals, env)?;
            let canonical_handle = env.canon(handle);
            let dynamic_class = env
                .class_handles
                .get(&canonical_handle)
                .map(String::as_str)
                .unwrap_or(class);
            let child = self.class_env(
                env,
                handle,
                dynamic_class,
                Some(owner.as_str()),
                &bound,
                Some((definition.name.as_str(), scratch.base, scratch.width)),
            );
            let mut statements = Vec::new();
            for statement in &definition.body {
                statements.push(self.elab_stmt(statement, &child)?);
            }
            StmtIR::Block(statements)
        };

        let assignment = if nonblocking {
            StmtIR::NBl {
                lv: target,
                e: result,
            }
        } else {
            StmtIR::Bl {
                lv: target,
                e: result,
            }
        };
        Ok(StmtIR::Block(vec![body, assignment]))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn virtual_dispatch_value_body(
        &mut self,
        handle: &str,
        class: &str,
        method: &str,
        actuals: &[ast::Expr],
        result_base: usize,
        result_width: usize,
        env: &Env<'_>,
    ) -> Result<Option<StmtIR>, String> {
        let Some((_, arms)) = self.virtual_dispatch_arms(handle, class, method)? else {
            return Ok(None);
        };
        let canonical = env.canon(handle);
        let tid_net = type_id_net(&canonical);
        let Ok(tid_entry) = env.net(&tid_net) else {
            return Ok(None);
        };
        let mut cases = Vec::new();
        for (type_id, owner) in arms {
            let Some(definition) = self.classes.get(owner.as_str()).and_then(|definitions| {
                definitions
                    .methods
                    .iter()
                    .find(|definition| definition.name == method)
            }) else {
                continue;
            };
            let definition = definition.clone();
            self.bind_class_formals(&definition, actuals, env)?;
            let bound = self.class_actuals(&definition, actuals, env)?;
            let child = self.class_env(
                env,
                &canonical,
                owner.as_str(),
                Some(owner.as_str()),
                &bound,
                Some((definition.name.as_str(), result_base, result_width)),
            );
            let mut body = Vec::new();
            for statement in &definition.body {
                body.push(self.elab_stmt(statement, &child)?);
            }
            cases.push((
                ExprIR::Const(LogicWord::from_u64(tid_entry.width, type_id as u64)),
                StmtIR::Block(body),
            ));
        }
        Ok(Some(StmtIR::Case {
            e: ExprIR::Ref {
                net: tid_entry.base,
                decl_width: tid_entry.width,
                range: full_range(tid_entry.width),
                signed: false,
            },
            arms: cases,
            dflt: Some(Box::new(StmtIR::Block(Vec::new()))),
        }))
    }

    /// `h2 = h1;` between handles of the same class copies every field.
    pub(super) fn class_handle_copy(
        &mut self,
        dst: &str,
        src: &str,
        _nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let dst_class = env
            .class_handles
            .get(dst)
            .ok_or_else(|| format!("'{dst}' is not a class handle"))?
            .clone();
        let src_canonical = env.canon(src);
        let src_class = env
            .class_handles
            .get(&src_canonical)
            .or_else(|| env.class_handles.get(src))
            .ok_or_else(|| format!("'{src}' is not a class handle"))?
            .clone();
        let src_chain = self.class_chain(&src_class)?;
        if !src_chain.contains(&dst_class) {
            return Err(format!(
                "cannot assign '{src}' of class '{src_class}' into '{dst}' of class '{dst_class}'"
            ));
        }
        // 引用语义：dst 的别名组整体并入 src 的组；此后经任一成员的
        // 字段读写都解析到共享存储。
        let src_canon = env.canon(src);
        let dst_canon = env.canon(dst);
        {
            let mut aliases = env.class_aliases.borrow_mut();
            let members: Vec<String> = aliases
                .iter()
                .filter(|(_, canonical)| *canonical == &dst_canon)
                .map(|(handle, _)| handle.clone())
                .collect();
            for member in members {
                aliases.insert(member, src_canon.clone());
            }
            if dst_canon != dst {
                aliases.insert(dst.to_string(), src_canon.clone());
            } else if dst != src_canon {
                // dst 原是自身组的代表：并入后其自身条目也指向新代表。
                aliases.insert(dst.to_string(), src_canon.clone());
            }
        }
        Ok(StmtIR::Block(Vec::new()))
    }

    pub(super) fn detach_class_handle_alias(
        &self,
        env: &Env<'_>,
        handle: &str,
    ) -> Option<(String, String)> {
        let mut aliases = env.class_aliases.borrow_mut();
        let old_canon = aliases
            .get(handle)
            .cloned()
            .unwrap_or_else(|| handle.to_string());
        let mut promoted_handle = None;
        if old_canon == handle {
            let members: Vec<String> = aliases
                .iter()
                .filter(|(h, canonical)| **canonical == old_canon && h.as_str() != handle)
                .map(|(h, _)| h.clone())
                .collect();
            if let Some(promoted) = members.first().cloned() {
                aliases.remove(&promoted);
                for member in members {
                    aliases.insert(member, promoted.clone());
                }
                promoted_handle = Some(promoted);
            }
        }
        aliases.remove(handle);
        promoted_handle.map(|new_handle| (old_canon, new_handle))
    }

    pub(super) fn promote_class_handle_storage(
        &self,
        env: &Env<'_>,
        old_handle: &str,
        promoted_handle: &str,
    ) -> Vec<StmtIR> {
        let old_prefix = format!("__{old_handle}_");
        let promoted_prefix = format!("__{promoted_handle}_");
        let mut assignments = Vec::new();
        for (source_name, source) in env.nets {
            let destination_name = if source_name == old_handle {
                promoted_handle.to_string()
            } else if let Some(suffix) = source_name.strip_prefix(&old_prefix) {
                format!("{promoted_prefix}{suffix}")
            } else {
                continue;
            };
            let Some(destination) = env.nets.get(&destination_name) else {
                continue;
            };
            assignments.push(StmtIR::Bl {
                lv: LValue {
                    net: destination.base,
                    decl_width: destination.width,
                    range: full_range(destination.width),
                },
                e: ExprIR::Ref {
                    net: source.base,
                    decl_width: source.width,
                    range: full_range(source.width),
                    signed: false,
                },
            });
        }
        assignments
    }

    pub(super) fn class_null_statement(
        &self,
        flag_net: &str,
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let promotion = self.detach_class_handle_alias(env, flag_net);
        let mut statements = Vec::new();
        if let Some((old_handle, promoted_handle)) = promotion {
            statements.extend(self.promote_class_handle_storage(
                env,
                &old_handle,
                &promoted_handle,
            ));
        }
        let entry = env.net(flag_net)?;
        let lvalue = LValue {
            net: entry.base,
            decl_width: entry.width,
            range: full_range(entry.width),
        };
        let value = ExprIR::Const(LogicWord::zero(entry.width));
        statements.push(if nonblocking {
            StmtIR::NBl {
                lv: lvalue,
                e: value,
            }
        } else {
            StmtIR::Bl {
                lv: lvalue,
                e: value,
            }
        });
        Ok(StmtIR::Block(statements))
    }

    pub(super) fn class_cast_effects(
        &mut self,
        arguments: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let [destination, source] = arguments else {
            return Err("$cast expects destination and source class handles".to_string());
        };
        let (destination, source) = match (destination, source) {
            (ast::Expr::Ident(destination), ast::Expr::Ident(source)) => {
                (destination.clone(), source.clone())
            }
            _ => {
                return Err(
                    "$cast in this subset requires top-level class handle arguments".to_string(),
                )
            }
        };
        let destination_class = env
            .class_handles
            .get(&destination)
            .cloned()
            .ok_or_else(|| format!("'{destination}' is not a class handle"))?;
        let source_canonical = env.canon(&source);
        let source_class = env
            .class_handles
            .get(&source_canonical)
            .or_else(|| env.class_handles.get(&source))
            .cloned()
            .ok_or_else(|| format!("'{source}' is not a class handle"))?;
        let source_chain = self.class_chain(&source_class)?;
        let destination_chain = self.class_chain(&destination_class)?;
        if source_chain.contains(&destination_class) || destination_chain.contains(&source_class) {
            return self.class_handle_copy(&destination, &source, false, env);
        }
        Ok(StmtIR::Block(Vec::new()))
    }

    pub(super) fn class_cast_assignment(
        &mut self,
        target: LValue,
        arguments: &[ast::Expr],
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let [destination, source] = arguments else {
            return Err("$cast expects destination and source class handles".to_string());
        };
        let (destination, source) = match (destination, source) {
            (ast::Expr::Ident(destination), ast::Expr::Ident(source)) => {
                (destination.clone(), source.clone())
            }
            _ => {
                return Err(
                    "$cast in this subset requires top-level class handle arguments".to_string(),
                )
            }
        };
        let destination_class = env
            .class_handles
            .get(&destination)
            .cloned()
            .ok_or_else(|| format!("'{destination}' is not a class handle"))?;
        let source_canonical = env.canon(&source);
        let source_class = env
            .class_handles
            .get(&source_canonical)
            .or_else(|| env.class_handles.get(&source))
            .cloned()
            .ok_or_else(|| format!("'{source}' is not a class handle"))?;
        let source_chain = self.class_chain(&source_class)?;
        let destination_chain = self.class_chain(&destination_class)?;
        let compatible =
            source_chain.contains(&destination_class) || destination_chain.contains(&source_class);
        let effects = if compatible {
            self.class_handle_copy(&destination, &source, false, env)?
        } else {
            StmtIR::Block(Vec::new())
        };
        let status = ExprIR::Const(LogicWord::from_u64(32, u64::from(compatible)));
        let assignment = if nonblocking {
            StmtIR::NBl {
                lv: target,
                e: status,
            }
        } else {
            StmtIR::Bl {
                lv: target,
                e: status,
            }
        };
        Ok(StmtIR::Block(vec![effects, assignment]))
    }
}
