//! Elaborator module-domain implementation.

use super::super::*;

impl<'a> Elaborator<'a> {
    pub(in crate::elab) fn elab_module_inner(
        &mut self,
        module: &ast::Module,
        prefix: &str,
        parent_params: &HashMap<String, i64>,
        interface_ports: &HashMap<String, HashMap<String, NetEntry>>,
    ) -> Result<(), String> {
        let root_instance = prefix.is_empty();
        let net_start = self.design.n_nets;
        let mut root_index = None;
        if root_instance {
            root_index = Some(self.design.hierarchy.len());
            self.design.hierarchy.push(HierarchicalInstance {
                path: module.name.clone(),
                module: module.name.clone(),
                parent: None,
                depth: 0,
                net_start,
                net_end: net_start,
                ports: Vec::new(),
            });
        }
        // 1. parameters (may reference earlier parameters). Values passed
        //    down from a parent instance take precedence over defaults.
        let mut params = parent_params.clone();
        let mut local_types = HashMap::new();
        let mut local_functions = HashMap::new();
        for import in self.file_imports {
            self.apply_import(
                import,
                &mut params,
                &mut local_types,
                &mut local_functions,
                parent_params,
                &module.name,
            )?;
        }
        for item in &module.items {
            if let ast::Item::Imports(imports) = item {
                for import in imports {
                    self.apply_import(
                        import,
                        &mut params,
                        &mut local_types,
                        &mut local_functions,
                        parent_params,
                        &module.name,
                    )?;
                }
            }
        }
        let mut declared_parameter_names = std::collections::HashSet::new();
        for item in &module.items {
            if let ast::Item::Params(list) = item {
                for (name, value) in list {
                    if !declared_parameter_names.insert(name.clone()) {
                        return Err(format!(
                            "module '{}': duplicate parameter '{}'",
                            module.name, name
                        ));
                    }
                    if parent_params.contains_key(name) {
                        continue;
                    }
                    let v = self.const_eval(value, &params, &module.name)?;
                    params.insert(name.clone(), v);
                }
            }
        }
        for item in &module.items {
            if let ast::Item::TypeDef(definition) = item {
                let type_name = match definition {
                    ast::TypeDef::Enum(definition) => &definition.name,
                    ast::TypeDef::Struct(definition) => &definition.name,
                    ast::TypeDef::Union(definition) => &definition.name,
                };
                if local_types.contains_key(type_name) {
                    return Err(format!(
                        "module '{}': duplicate type definition '{}'",
                        module.name, type_name
                    ));
                }
                let resolved = resolve_type(definition, &params, &module.name, &local_types)?;
                if let ResolvedType::Enum(enum_definition) = &resolved {
                    for (constant, value) in &enum_definition.constants {
                        if !parent_params.contains_key(constant) {
                            params.insert(constant.clone(), *value);
                        }
                    }
                }
                local_types.insert(type_name.clone(), resolved);
            }
        }
        let effective_items =
            flatten_generate_items(&module.items, &params, &module.name, self.packages)?;
        for item in &effective_items {
            if let ast::Item::Function(definition) = item {
                if local_functions.contains_key(&definition.name) {
                    return Err(format!(
                        "module '{}': duplicate function '{}'",
                        module.name, definition.name
                    ));
                }
                let mut argument_names = std::collections::HashSet::new();
                for (argument, _) in &definition.args {
                    if !argument_names.insert(argument.as_str()) {
                        return Err(format!(
                            "module '{}': function '{}' has duplicate argument '{}'",
                            module.name, definition.name, argument
                        ));
                    }
                }
                local_functions.insert(definition.name.clone(), definition.clone());
            }
        }

        // 2. Ordered port list (header order) with directions.
        let mut port_dirs: HashMap<String, ast::DirSrc> = HashMap::new();
        let mut port_order: Vec<String> = Vec::new();
        {
            let mut seen = std::collections::HashSet::new();
            for item in &effective_items {
                if let ast::Item::Port {
                    dir,
                    interface_name: None,
                    names,
                    ..
                } = item
                {
                    for n in names {
                        if port_dirs.contains_key(n) {
                            return Err(format!(
                                "module '{}': duplicate port '{}'",
                                module.name, n
                            ));
                        }
                        port_dirs.insert(n.clone(), *dir);
                        if seen.insert(n.clone()) {
                            port_order.push(n.clone());
                        }
                    }
                }
            }
        }
        if root_instance {
            if let Some(name) = port_order
                .iter()
                .find(|name| port_dirs.get(*name) == Some(&ast::DirSrc::InOut))
            {
                return Err(format!(
                    "top module '{}': inout port '{}' is not supported",
                    module.name, name
                ));
            }
        }

        // 3. Declare nets from explicit declarations and ports.
        let mut nets: HashMap<String, NetEntry> = HashMap::new();
        let mut class_handles: HashMap<String, String> = HashMap::new();
        let mut interface_directions = HashMap::new();
        for item in &effective_items {
            match item {
                ast::Item::Net {
                    is_reg,
                    is_signed,
                    type_name,
                    range,
                    array_ranges,
                    names,
                } => {
                    for (n, _) in names {
                        if nets.contains_key(n) {
                            return Err(format!(
                                "module '{}': duplicate declaration of '{}'",
                                module.name, n
                            ));
                        }
                        let type_width = self.declaration_type_width(
                            type_name.as_deref(),
                            &local_types,
                            &module.name,
                        )?;
                        let is_class_handle = type_name
                            .as_deref()
                            .is_some_and(|t| self.classes.contains_key(t));
                        if is_class_handle {
                            class_handles.insert(n.clone(), type_name.clone().unwrap_or_default());
                        }
                        // Class handles are procedurally assigned storage.
                        let is_reg = *is_reg || is_class_handle;
                        let element_width = match type_width {
                            Some(width) => width,
                            None => match range {
                                Some((msb, lsb)) => {
                                    let hi = self.const_eval(msb, &params, &module.name)?;
                                    let lo = self.const_eval(lsb, &params, &module.name)?;
                                    (hi - lo).unsigned_abs() as usize + 1
                                }
                                None => 1,
                            },
                        };
                        let mut array = self.resolve_unpacked_array(
                            array_ranges,
                            element_width,
                            &params,
                            &module.name,
                            n,
                            "net",
                        )?;
                        self.apply_element_range(&mut array, range, &params, &module.name)?;
                        let memory = array.as_ref().and_then(|array| {
                            let use_sparse_memory = !array.dynamic
                                && array.dimensions.len() == 1
                                && array.element_width * array.length > 4096;
                            use_sparse_memory.then(|| self.declare_memory(n, prefix, array))
                        });
                        let declaration_range = if array.is_some() {
                            match memory {
                                Some(_) => None,
                                None => {
                                    let length =
                                        array.as_ref().map_or(1, |resolved| resolved.length);
                                    Self::flat_array_range(element_width * length)
                                }
                            }
                        } else {
                            range.clone()
                        };
                        let mut e = self.declare_net(
                            n,
                            prefix,
                            &declaration_range,
                            &params,
                            is_reg,
                            *is_signed,
                            type_width,
                            type_name.as_deref(),
                        )?;
                        e.array = array;
                        e.memory = memory;
                        if e.array.as_ref().is_some_and(|array| array.dynamic) {
                            let size_name = format!("__{n}_dynamic_size");
                            let size_entry = self.declare_net(
                                &size_name,
                                prefix,
                                &Self::flat_array_range(9),
                                &HashMap::new(),
                                true,
                                false,
                                None,
                                None,
                            )?;
                            let size_base = size_entry.base;
                            e.array
                                .as_mut()
                                .expect("dynamic array metadata was just checked")
                                .dynamic_size_base = size_base;
                        }
                        nets.insert(n.clone(), e);
                    }
                }
                ast::Item::Port {
                    interface_name: Some(definition),
                    modport_name,
                    names,
                    ..
                } => {
                    for n in names {
                        if !interface_ports.contains_key(n) {
                            return Err(format!(
                                "module '{}': interface port '{}' has no binding",
                                module.name, n
                            ));
                        }
                        let fields = &interface_ports[n];
                        let interface = self.mods.get(definition).ok_or_else(|| {
                            format!(
                                "module '{}': unknown interface '{}'",
                                module.name, definition
                            )
                        })?;
                        if !interface.is_interface {
                            return Err(format!("'{}' is not an interface", definition));
                        }
                        let expected = interface
                            .items
                            .iter()
                            .filter_map(|item| match item {
                                ast::Item::Net { names, .. } => Some(names),
                                _ => None,
                            })
                            .flatten()
                            .count();
                        if fields.len() != expected {
                            return Err(format!(
                                "module '{}': interface port '{}' has incorrect signal binding",
                                module.name, n
                            ));
                        }
                        if let Some(modport_name) = modport_name {
                            let modport = interface
                                .items
                                .iter()
                                .find_map(|item| match item {
                                    ast::Item::Modport { name, ports } if name == modport_name => {
                                        Some(ports)
                                    }
                                    _ => None,
                                })
                                .ok_or_else(|| {
                                    format!(
                                        "module '{}': interface '{}' has no modport '{}'",
                                        module.name, definition, modport_name
                                    )
                                })?;
                            let signal_names = interface
                                .items
                                .iter()
                                .filter_map(|item| match item {
                                    ast::Item::Net { names, .. } => Some(names),
                                    _ => None,
                                })
                                .flatten()
                                .map(|(name, _)| name.as_str())
                                .collect::<std::collections::HashSet<_>>();
                            for port in modport {
                                if !signal_names.contains(port.name.as_str()) {
                                    return Err(format!(
                                        "interface '{}': modport '{}' lists unknown signal '{}'",
                                        definition, modport_name, port.name
                                    ));
                                }
                            }
                            interface_directions.insert(
                                n.clone(),
                                modport
                                    .iter()
                                    .map(|port| (port.name.clone(), port.dir))
                                    .collect::<HashMap<_, _>>(),
                            );
                        }
                    }
                }
                ast::Item::Port {
                    dir,
                    range,
                    array_ranges,
                    type_name,
                    names,
                    is_reg,
                    is_signed,
                    interface_name: None,
                    ..
                } => {
                    for n in names {
                        if !nets.contains_key(n) {
                            let type_width = self.declaration_type_width(
                                type_name.as_deref(),
                                &local_types,
                                &module.name,
                            )?;
                            let element_width = match type_width {
                                Some(width) => width,
                                None => match range {
                                    Some((msb, lsb)) => {
                                        let hi = self.const_eval(msb, &params, &module.name)?;
                                        let lo = self.const_eval(lsb, &params, &module.name)?;
                                        (hi - lo).unsigned_abs() as usize + 1
                                    }
                                    None => 1,
                                },
                            };
                            let mut array = self.resolve_unpacked_array(
                                array_ranges,
                                element_width,
                                &params,
                                &module.name,
                                n,
                                "port",
                            )?;
                            if array.as_ref().is_some_and(|resolved| resolved.dynamic) {
                                return Err(format!(
                                    "module '{}': dynamic unpacked array port '{}' is not supported",
                                    module.name, n
                                ));
                            }
                            if let Some(resolved) = array.as_ref() {
                                let total_width = resolved.element_width * resolved.length;
                                let sparse_inout = *dir == ast::DirSrc::InOut
                                    && resolved.dimensions.len() == 1
                                    && total_width > 4096;
                                if total_width > 1 << 20 && !sparse_inout {
                                    return Err(format!(
                                        "module '{}': unpacked array port '{}' exceeds the 1 MiB flat budget; sparse memory ports are not supported yet",
                                        module.name, n
                                    ));
                                }
                            }
                            self.apply_element_range(&mut array, range, &params, &module.name)?;
                            let memory = array.as_ref().and_then(|resolved| {
                                let total_width = resolved.element_width * resolved.length;
                                (*dir == ast::DirSrc::InOut
                                    && resolved.dimensions.len() == 1
                                    && total_width > 4096)
                                    .then(|| self.declare_memory(n, prefix, resolved))
                            });
                            let declaration_range = if array.is_some() {
                                match memory {
                                    Some(_) => None,
                                    None => {
                                        let length =
                                            array.as_ref().map_or(1, |resolved| resolved.length);
                                        Self::flat_array_range(element_width * length)
                                    }
                                }
                            } else {
                                range.clone()
                            };
                            let mut e = self.declare_net(
                                n,
                                prefix,
                                &declaration_range,
                                &params,
                                *is_reg,
                                *is_signed,
                                type_width,
                                type_name.as_deref(),
                            )?;
                            e.array = array;
                            e.memory = memory;
                            nets.insert(n.clone(), e);
                        } else if !array_ranges.is_empty() {
                            return Err(format!(
                                "module '{}': cannot refine existing port '{}' as an unpacked array",
                                module.name, n
                            ));
                        }
                    }
                }
                _ => {}
            }
        }

        // 3.5 Class storage: per-handle instance fields and shared statics.
        for (handle, class) in class_handles.clone() {
            self.declare_handle_fields(&handle, &class, prefix, &params, &mut nets, 0)?;
        }
        let mut static_storage_classes: HashSet<String> = self
            .classes
            .keys()
            .filter(|class| !BUILTIN_UVM_CLASS_NAMES.contains(&class.as_str()))
            .cloned()
            .collect();
        static_storage_classes.extend(class_handles.values().cloned());
        let effective_item_debug = format!("{effective_items:?}");
        for class in BUILTIN_UVM_CLASS_NAMES {
            if effective_item_debug.contains(&format!("\"{class}\"")) {
                static_storage_classes.insert(class.to_string());
            }
        }
        let class_static_initials = self.declare_class_storage(
            prefix,
            &params,
            &module.name,
            &mut nets,
            &static_storage_classes,
        )?;

        // 4. Record top-level ports.
        if prefix.is_empty() {
            for n in &port_order {
                let e = &nets[n];
                let dir = port_dirs.get(n).copied().unwrap_or(ast::DirSrc::In);
                self.design.ports.push(PortDecl {
                    name: n.clone(),
                    dir: if dir == ast::DirSrc::Out {
                        Dir::Out
                    } else {
                        Dir::In
                    },
                    base: e.base,
                    width: e.width,
                });
            }
        }

        // 5. Elaborate bodies.
        let mut interface_instances = HashMap::new();
        let env = Env {
            class_aliases: std::rc::Rc::new(std::cell::RefCell::new(HashMap::new())),
            nets: &nets,
            interface_ports,
            interface_directions: &interface_directions,
            params: &params,
            types: &local_types,
            functions: &local_functions,
            module_name: &module.name,
            prefix,
            class_handles: &class_handles,
            this_handle: None,
            method_owner: None,
            method_args: &HashMap::new(),
            method_return: None,
        };
        let mut body_items = self.expand_module_instance_arrays(&effective_items, &env)?;
        // Static-field seeds must run before any user initial block that may
        // observe or mutate them.
        body_items.splice(0..0, class_static_initials);
        for item in &body_items {
            match item {
                ast::Item::Net { names, .. } => {
                    for (n, init) in names {
                        if let Some(e) = init {
                            if let ast::Expr::NewClass(actuals) = e {
                                let body = self.elab_stmt(
                                    &ast::Stmt::BL {
                                        lv: ast::Expr::Ident(n.clone()),
                                        e: ast::Expr::NewClass(actuals.clone()),
                                    },
                                    &env,
                                )?;
                                self.design.initials.push(body);
                                continue;
                            }
                            let e = self.elab_expr(e, &env)?;
                            let lv = self.elab_lvalue(&ast::Expr::Ident(n.clone()), &env)?;
                            self.design.assigns.push(Assign { lv, e, delay: 0 });
                        }
                    }
                }
                ast::Item::Assign { delay, lv, e } => {
                    let memory_target = match lv {
                        ast::Expr::Select(name, index) => {
                            Some((name, std::slice::from_ref(index.as_ref())))
                        }
                        ast::Expr::ArraySelect(name, indexes) => Some((name, indexes.as_slice())),
                        _ => None,
                    };
                    if let Some((name, indexes)) = memory_target {
                        let entry = env.net(name)?;
                        if entry.memory.is_some() {
                            if *delay != 0 {
                                return Err(
                                    "delayed sparse-memory continuous lvalue is not supported"
                                        .to_string(),
                                );
                            }
                            let body =
                                self.memory_write_statement(entry, indexes, e, false, &env)?;
                            self.design.processes.push(Process {
                                sens: SensIR::Comb,
                                body,
                            });
                            continue;
                        }
                    }
                    if let ast::Expr::ArrayIndexedPart {
                        name,
                        indexes,
                        start,
                        width,
                        ascending,
                    } = lv
                    {
                        let entry = env.net(name)?;
                        if entry.memory.is_some() {
                            if *delay != 0 {
                                return Err(
                                    "delayed sparse-memory continuous lvalue is not supported"
                                        .to_string(),
                                );
                            }
                            let body = self.memory_part_write_statement(
                                entry, indexes, start, width, *ascending, e, false, &env,
                            )?;
                            self.design.processes.push(Process {
                                sens: SensIR::Comb,
                                body,
                            });
                            continue;
                        }
                    }
                    if let ast::Expr::Select(name, index) = lv {
                        if env
                            .net(name)
                            .map(|entry| entry.array.is_some())
                            .unwrap_or(false)
                            && self.const_eval(index, env.params, env.module_name).is_err()
                        {
                            if *delay != 0 {
                                return Err(
                                    "delayed runtime indexed continuous-assignment lvalue is not supported"
                                        .to_string(),
                                );
                            }
                            let element_width = env
                                .net(name)?
                                .array
                                .as_ref()
                                .map(|array| array.element_width)
                                .ok_or_else(|| format!("'{name}' is not an unpacked array"))?;
                            let body = self.elab_stmt(
                                &ast::Stmt::BL {
                                    lv: ast::Expr::ArrayIndexedPart {
                                        name: name.clone(),
                                        indexes: vec![index.as_ref().clone()],
                                        start: Box::new(constant_expr(0)),
                                        width: Box::new(constant_expr(element_width as i64)),
                                        ascending: true,
                                    },
                                    e: e.clone(),
                                },
                                &env,
                            )?;
                            self.design.processes.push(Process {
                                sens: SensIR::Comb,
                                body,
                            });
                            continue;
                        }
                    }
                    if let ast::Expr::ArraySelect(name, indexes) = lv {
                        if indexes.iter().any(|index| {
                            self.const_eval(index, env.params, env.module_name).is_err()
                        }) {
                            if *delay != 0 {
                                return Err(
                                    "delayed runtime indexed continuous-assignment lvalue is not supported"
                                        .to_string(),
                                );
                            }
                            let element_width = env
                                .net(name)?
                                .array
                                .as_ref()
                                .map(|array| array.element_width)
                                .ok_or_else(|| format!("'{name}' is not an unpacked array"))?;
                            let body = self.elab_stmt(
                                &ast::Stmt::BL {
                                    lv: ast::Expr::ArrayIndexedPart {
                                        name: name.clone(),
                                        indexes: indexes.clone(),
                                        start: Box::new(constant_expr(0)),
                                        width: Box::new(constant_expr(element_width as i64)),
                                        ascending: true,
                                    },
                                    e: e.clone(),
                                },
                                &env,
                            )?;
                            self.design.processes.push(Process {
                                sens: SensIR::Comb,
                                body,
                            });
                            continue;
                        }
                    }
                    if let ast::Expr::ArrayIndexedPart {
                        name,
                        indexes,
                        start,
                        width,
                        ascending,
                    } = lv
                    {
                        if indexes.iter().any(|index| {
                            self.const_eval(index, env.params, env.module_name).is_err()
                        }) {
                            if *delay != 0 {
                                return Err(
                                    "delayed runtime indexed continuous-assignment lvalue is not supported"
                                        .to_string(),
                                );
                            }
                            if self.const_eval(start, env.params, env.module_name).is_err() {
                                return Err(
                                    "runtime indexed part-select continuous-assignment lvalue is not supported"
                                        .to_string(),
                                );
                            }
                            let body = self.runtime_array_element_indexed_statement(
                                name, indexes, start, width, *ascending, e, false, &env,
                            )?;
                            self.design.processes.push(Process {
                                sens: SensIR::Comb,
                                body,
                            });
                            continue;
                        }
                        if self.const_eval(start, env.params, env.module_name).is_err() {
                            return Err(
                                "runtime indexed part-select continuous-assignment lvalue is not supported"
                                    .to_string(),
                            );
                        }
                        let (entry, offset, element_width) =
                            self.array_element_location(name, indexes, &env)?;
                        let (physical_lsb, width, reversed) = self.array_element_indexed_span(
                            &entry,
                            offset,
                            element_width,
                            start,
                            width,
                            *ascending,
                            &env,
                        )?;
                        let target = LValue {
                            net: entry.base,
                            decl_width: entry.width,
                            range: Range::Span {
                                msb: physical_lsb + width - 1,
                                lsb: physical_lsb,
                            },
                        };
                        let value = self.elab_expr(e, &env)?;
                        let value = Self::resize_indexed_value(value, width);
                        let physical_value = if reversed {
                            ExprIR::Concat(
                                (0..width)
                                    .map(|bit| ExprIR::Index {
                                        e: Box::new(value.clone()),
                                        msb: bit,
                                        lsb: bit,
                                    })
                                    .collect(),
                            )
                        } else {
                            value
                        };
                        self.design.assigns.push(Assign {
                            lv: target,
                            e: physical_value,
                            delay: *delay,
                        });
                        continue;
                    }
                    if let ast::Expr::IndexedPart {
                        name,
                        start,
                        width,
                        ascending,
                    } = lv
                    {
                        if let Ok(entry) = env.net(name) {
                            if entry.array.is_none() && entry.msb < entry.lsb {
                                if self.const_eval(start, env.params, env.module_name).is_err() {
                                    if *delay != 0 {
                                        return Err(
                                            "delayed runtime indexed continuous-assignment lvalue is not supported"
                                                .to_string(),
                                        );
                                    }
                                    let body = self.dynamic_indexed_part_bl_reversed(
                                        entry, start, width, *ascending, e, &env,
                                    )?;
                                    self.design.processes.push(Process {
                                        sens: SensIR::Comb,
                                        body,
                                    });
                                    continue;
                                }
                                let target = self.reversed_indexed_part_lvalue(
                                    entry, start, width, *ascending, &env,
                                )?;
                                let value = self.elab_expr(e, &env)?;
                                let value_width = target.range.width();
                                let value = Self::resize_indexed_value(value, value_width);
                                let physical_value = ExprIR::Concat(
                                    (0..value_width)
                                        .map(|bit| ExprIR::Index {
                                            e: Box::new(value.clone()),
                                            msb: bit,
                                            lsb: bit,
                                        })
                                        .collect(),
                                );
                                self.design.assigns.push(Assign {
                                    lv: target,
                                    e: physical_value,
                                    delay: *delay,
                                });
                                continue;
                            }
                            if entry.array.is_none()
                                && self.const_eval(start, env.params, env.module_name).is_err()
                            {
                                if *delay != 0 {
                                    return Err(
                                        "delayed runtime indexed continuous-assignment lvalue is not supported"
                                            .to_string(),
                                    );
                                }
                                let body = self.dynamic_indexed_part_bl(
                                    name, start, width, *ascending, e, &env,
                                )?;
                                self.design.processes.push(Process {
                                    sens: SensIR::Comb,
                                    body,
                                });
                                continue;
                            }
                        }
                    }
                    if let ast::Expr::Concat(parts) = lv {
                        let rhs = self.elab_expr(e, &env)?;
                        let total: usize = parts
                            .iter()
                            .map(|p| self.elab_lvalue(p, &env).map(|lv| lv.range.width()))
                            .collect::<Result<Vec<_>, _>>()?
                            .iter()
                            .sum();
                        let mut cursor = total;
                        for part in parts {
                            let lv = self.elab_lvalue(part, &env)?;
                            let width = lv.range.width();
                            cursor -= width;
                            let sliced = ExprIR::Index {
                                e: Box::new(rhs.clone()),
                                msb: cursor + width - 1,
                                lsb: cursor,
                            };
                            self.design.assigns.push(Assign {
                                lv,
                                e: sliced,
                                delay: *delay,
                            });
                        }
                        continue;
                    }
                    let lv = self.elab_lvalue(lv, &env)?;
                    let e = self.elab_expr(e, &env)?;
                    self.design.assigns.push(Assign {
                        lv,
                        e,
                        delay: *delay,
                    });
                }
                ast::Item::Always { sens, body } => {
                    let s = self.elab_sens(sens, &env)?;
                    let b = self.elab_stmt(body, &env)?;
                    self.design.processes.push(Process { sens: s, body: b });
                }
                ast::Item::AssertProperty {
                    kind,
                    label,
                    clock,
                    disable,
                    antecedent,
                    consequent,
                    composition,
                    overlapping,
                    delay,
                    repetition,
                    message,
                } => {
                    let sens = self.elab_sens(clock, &env)?;
                    if !matches!(sens, SensIR::Posedge(_) | SensIR::Negedge(_)) {
                        return Err(format!(
                            "module '{}': concurrent property requires a posedge or negedge clock",
                            module.name
                        ));
                    }
                    let antecedent = self.elab_expr(antecedent, &env)?;
                    let disable = self.elab_expr(disable, &env)?;
                    let mut consequent = self.elab_expr(consequent, &env)?;
                    let mut composed_right = None;
                    let mut composed_op = ast::PropertyCompositionOp::And;
                    let mut composition_delay_cycles = 0_usize;
                    if let Some(composition) = composition.as_deref() {
                        let right = self.elab_expr(&composition.right, &env)?;
                        match &composition.right_delay {
                            ast::PropertyDelay::Fixed(delay) => {
                                let delay = self.const_eval(delay, env.params, env.module_name)?;
                                if !(0..=256).contains(&delay) {
                                    return Err(format!(
                                        "module '{}': sequence composition delay must be between 0 and 256",
                                        module.name
                                    ));
                                }
                                composition_delay_cycles = delay as usize;
                            }
                            ast::PropertyDelay::Range(..) => {
                                return Err(format!(
                                    "module '{}': sequence composition supports fixed delay only",
                                    module.name
                                ));
                            }
                        }
                        composed_op = composition.op;
                        if composition_delay_cycles == 0 {
                            consequent = ExprIR::Bin {
                                op: match composition.op {
                                    ast::PropertyCompositionOp::And => BinOp::LogAnd,
                                    ast::PropertyCompositionOp::Or => BinOp::LogOr,
                                },
                                a: Box::new(consequent),
                                b: Box::new(right),
                            };
                        } else {
                            composed_right = Some(right);
                        }
                    }
                    let (delay_cycles, delay_range) = match delay.as_ref() {
                        ast::PropertyDelay::Fixed(delay) => {
                            let explicit_delay =
                                self.const_eval(delay, env.params, env.module_name)?;
                            if !(0..=256).contains(&explicit_delay) {
                                return Err(format!(
                                    "module '{}': property sequence delay must be between 0 and 256",
                                    module.name
                                ));
                            }
                            let cycles = if *overlapping {
                                explicit_delay as usize
                            } else {
                                1
                            };
                            (Some(cycles), None)
                        }
                        ast::PropertyDelay::Range(start, end) => {
                            let start = self.const_eval(start, env.params, env.module_name)?;
                            let end = self.const_eval(end, env.params, env.module_name)?;
                            if !(0..=256).contains(&start) || !(0..=256).contains(&end) {
                                return Err(format!(
                                    "module '{}': property sequence delay range must stay within 0..=256",
                                    module.name
                                ));
                            }
                            if start > end {
                                return Err(format!(
                                    "module '{}': property sequence delay range start exceeds end",
                                    module.name
                                ));
                            }
                            (None, Some((start as usize, end as usize)))
                        }
                    };
                    let property_index = self.design.processes.len();
                    let property_state_base = self.design.net_decls.len();
                    let mut composition_stages = Vec::with_capacity(composition_delay_cycles);
                    for stage in 0..composition_delay_cycles {
                        let stage_name = format!("__property_composed_{property_index}_{stage}");
                        composition_stages.push(self.declare_net(
                            &stage_name,
                            prefix,
                            &None,
                            &HashMap::new(),
                            true,
                            false,
                            None,
                            None,
                        )?);
                    }
                    let composition_stage_references = composition_stages
                        .iter()
                        .map(|pending| ExprIR::Ref {
                            net: pending.base,
                            decl_width: pending.width,
                            range: full_range(pending.width),
                            signed: false,
                        })
                        .collect::<Vec<_>>();
                    let cover_label = label
                        .clone()
                        .unwrap_or_else(|| format!("property_{property_index}"));
                    let make_direct_check = |consequent: ExprIR| -> StmtIR {
                        match kind {
                            ast::PropertyKind::Assert => StmtIR::Assert {
                                cond: consequent,
                                message: message.clone(),
                            },
                            ast::PropertyKind::Assume => StmtIR::Assume {
                                cond: consequent,
                                message: message.clone(),
                            },
                            ast::PropertyKind::Cover => StmtIR::Cover {
                                label: cover_label.clone(),
                                cond: consequent,
                            },
                        }
                    };
                    let make_check = |left: ExprIR| -> StmtIR {
                        if composed_right.is_none() {
                            return make_direct_check(left);
                        };
                        let Some(first_stage) = composition_stage_references.first() else {
                            return make_direct_check(left);
                        };
                        let ExprIR::Ref {
                            net: first_stage_net,
                            decl_width: first_stage_width,
                            ..
                        } = first_stage
                        else {
                            return make_direct_check(left);
                        };
                        let first_stage_lvalue = LValue {
                            net: *first_stage_net,
                            decl_width: *first_stage_width,
                            range: full_range(*first_stage_width),
                        };
                        match composed_op {
                            ast::PropertyCompositionOp::And => StmtIR::If {
                                c: left,
                                t: Box::new(StmtIR::NBl {
                                    lv: first_stage_lvalue,
                                    e: ExprIR::Const(LogicWord::from_u64(1, 1)),
                                }),
                                f: Some(Box::new(make_direct_check(ExprIR::Const(
                                    LogicWord::from_u64(1, 0),
                                )))),
                            },
                            ast::PropertyCompositionOp::Or => StmtIR::If {
                                c: left,
                                t: Box::new(make_direct_check(ExprIR::Const(LogicWord::from_u64(
                                    1, 1,
                                )))),
                                f: Some(Box::new(StmtIR::NBl {
                                    lv: first_stage_lvalue,
                                    e: ExprIR::Const(LogicWord::from_u64(1, 1)),
                                })),
                            },
                        }
                    };
                    let body = if let Some((goto_start, goto_end)) = match repetition.as_deref() {
                        Some(ast::PropertyRepetition::Goto(count)) => Some((count, None)),
                        Some(ast::PropertyRepetition::GotoRange(start, end)) => {
                            Some((start, Some(end)))
                        }
                        Some(ast::PropertyRepetition::Nonconsecutive(count)) => Some((count, None)),
                        Some(ast::PropertyRepetition::NonconsecutiveRange(start, end)) => {
                            Some((start, Some(end)))
                        }
                        _ => None,
                    } {
                        let count = self
                            .const_eval(goto_start, env.params, env.module_name)
                            .map_err(|error| {
                                format!(
                                    "module '{}': goto repetition range must be constant: {error}",
                                    module.name
                                )
                            })?;
                        if let Some(end) = goto_end {
                            let end = self.const_eval(end, env.params, env.module_name)?;
                            if !(1..=256).contains(&end) {
                                return Err(format!(
                                    "module '{}': goto repetition range must stay within 1..=256",
                                    module.name
                                ));
                            }
                            if count > end {
                                return Err(format!(
                                    "module '{}': goto repetition range start exceeds end",
                                    module.name
                                ));
                            }
                        }
                        if !(1..=256).contains(&count) {
                            return Err(format!(
                                "module '{}': goto repetition count must be between 1 and 256",
                                module.name
                            ));
                        }
                        if !*overlapping {
                            return Err(format!(
                                "module '{}': goto repetition requires '|->'",
                                module.name
                            ));
                        }
                        match delay.as_ref() {
                            ast::PropertyDelay::Fixed(delay) => {
                                let delay_value =
                                    self.const_eval(delay, env.params, env.module_name)?;
                                if delay_value != 0 {
                                    return Err(format!(
                                        "module '{}': goto repetition does not yet compose with sequence delay",
                                        module.name
                                    ));
                                }
                            }
                            ast::PropertyDelay::Range(..) => {
                                return Err(format!(
                                    "module '{}': goto repetition does not yet compose with delay ranges",
                                    module.name
                                ));
                            }
                        }

                        let count = count as usize;
                        if count == 1 {
                            StmtIR::If {
                                c: antecedent,
                                t: Box::new(make_check(consequent)),
                                f: None,
                            }
                        } else {
                            let mut stages = Vec::with_capacity(count - 1);
                            for seen in 1..count {
                                let state_name = format!("__property_goto_{property_index}_{seen}");
                                stages.push(self.declare_net(
                                    &state_name,
                                    prefix,
                                    &None,
                                    &HashMap::new(),
                                    true,
                                    false,
                                    None,
                                    None,
                                )?);
                            }
                            let references = stages
                                .iter()
                                .map(|state| ExprIR::Ref {
                                    net: state.base,
                                    decl_width: state.width,
                                    range: full_range(state.width),
                                    signed: false,
                                })
                                .collect::<Vec<_>>();
                            let mut statements = vec![StmtIR::If {
                                c: ExprIR::Bin {
                                    op: BinOp::And,
                                    a: Box::new(references[count - 2].clone()),
                                    b: Box::new(consequent.clone()),
                                },
                                t: Box::new(make_check(consequent.clone())),
                                f: None,
                            }];

                            for seen in (2..count).rev() {
                                let retain = ExprIR::Bin {
                                    op: BinOp::And,
                                    a: Box::new(references[seen - 1].clone()),
                                    b: Box::new(ExprIR::Un {
                                        op: UnOp::Not,
                                        a: Box::new(consequent.clone()),
                                    }),
                                };
                                let advance = ExprIR::Bin {
                                    op: BinOp::And,
                                    a: Box::new(references[seen - 2].clone()),
                                    b: Box::new(consequent.clone()),
                                };
                                let next = ExprIR::Bin {
                                    op: BinOp::Or,
                                    a: Box::new(retain),
                                    b: Box::new(advance),
                                };
                                statements.push(StmtIR::Bl {
                                    lv: LValue {
                                        net: stages[seen - 1].base,
                                        decl_width: stages[seen - 1].width,
                                        range: full_range(stages[seen - 1].width),
                                    },
                                    e: next,
                                });
                            }
                            let retain_first = ExprIR::Bin {
                                op: BinOp::And,
                                a: Box::new(references[0].clone()),
                                b: Box::new(ExprIR::Un {
                                    op: UnOp::Not,
                                    a: Box::new(consequent.clone()),
                                }),
                            };
                            let start = ExprIR::Bin {
                                op: BinOp::And,
                                a: Box::new(antecedent),
                                b: Box::new(consequent),
                            };
                            let first_next = ExprIR::Bin {
                                op: BinOp::Or,
                                a: Box::new(retain_first),
                                b: Box::new(start),
                            };
                            statements.push(StmtIR::Bl {
                                lv: LValue {
                                    net: stages[0].base,
                                    decl_width: stages[0].width,
                                    range: full_range(stages[0].width),
                                },
                                e: first_next,
                            });
                            StmtIR::Block(statements)
                        }
                    } else if let Some(repetition) = repetition.as_deref() {
                        let (minimum, maximum) = match repetition {
                            ast::PropertyRepetition::Count(count) => {
                                let count = self
                                    .const_eval(count, env.params, env.module_name)
                                    .map_err(|error| {
                                        format!(
                                            "module '{}': property repetition count must be constant: {error}",
                                            module.name
                                        )
                                    })?;
                                (count, count)
                            }
                            ast::PropertyRepetition::Range(start, end) => {
                                let start = self
                                    .const_eval(start, env.params, env.module_name)
                                    .map_err(|error| {
                                        format!(
                                            "module '{}': property repetition range must be constant: {error}",
                                            module.name
                                        )
                                    })?;
                                let end = self
                                    .const_eval(end, env.params, env.module_name)
                                    .map_err(|error| {
                                        format!(
                                            "module '{}': property repetition range must be constant: {error}",
                                            module.name
                                        )
                                    })?;
                                (start, end)
                            }
                            ast::PropertyRepetition::Goto(_) => {
                                unreachable!("goto repetition is handled separately")
                            }
                            ast::PropertyRepetition::GotoRange(..) => {
                                unreachable!("goto repetition range is handled separately")
                            }
                            ast::PropertyRepetition::Nonconsecutive(_) => {
                                unreachable!("nonconsecutive repetition is handled separately")
                            }
                            ast::PropertyRepetition::NonconsecutiveRange(..) => {
                                unreachable!(
                                    "nonconsecutive repetition range is handled separately"
                                )
                            }
                        };
                        if !(1..=256).contains(&minimum) || !(1..=256).contains(&maximum) {
                            return Err(format!(
                                "module '{}': property repetition range must stay within 1..=256",
                                module.name
                            ));
                        }
                        if minimum > maximum {
                            return Err(format!(
                                "module '{}': property repetition range start exceeds end",
                                module.name
                            ));
                        }
                        if !*overlapping {
                            return Err(format!(
                                "module '{}': consecutive repetition requires '|->'",
                                module.name
                            ));
                        }
                        match delay.as_ref() {
                            ast::PropertyDelay::Fixed(delay) => {
                                let delay_value =
                                    self.const_eval(delay, env.params, env.module_name)?;
                                if delay_value != 0 {
                                    return Err(format!(
                                        "module '{}': consecutive repetition does not yet compose with sequence delay",
                                        module.name
                                    ));
                                }
                            }
                            ast::PropertyDelay::Range(..) => {
                                return Err(format!(
                                    "module '{}': consecutive repetition does not yet compose with delay ranges",
                                    module.name
                                ));
                            }
                        }

                        let count = minimum as usize;
                        if count == 1 {
                            StmtIR::If {
                                c: antecedent,
                                t: Box::new(make_check(consequent)),
                                f: None,
                            }
                        } else {
                            let mut stages = Vec::with_capacity(count - 1);
                            for age in 1..count {
                                let pending_name =
                                    format!("__property_repeat_{property_index}_{age}");
                                stages.push(self.declare_net(
                                    &pending_name,
                                    prefix,
                                    &None,
                                    &HashMap::new(),
                                    true,
                                    false,
                                    None,
                                    None,
                                )?);
                            }
                            let references = stages
                                .iter()
                                .map(|pending| ExprIR::Ref {
                                    net: pending.base,
                                    decl_width: pending.width,
                                    range: full_range(pending.width),
                                    signed: false,
                                })
                                .collect::<Vec<_>>();
                            let mut statements = Vec::new();
                            if *kind == ast::PropertyKind::Cover {
                                statements.push(StmtIR::If {
                                    c: ExprIR::Bin {
                                        op: BinOp::And,
                                        a: Box::new(references[count - 2].clone()),
                                        b: Box::new(consequent.clone()),
                                    },
                                    t: Box::new(StmtIR::Cover {
                                        label: cover_label.clone(),
                                        cond: ExprIR::Const(LogicWord::from_u64(1, 1)),
                                    }),
                                    f: None,
                                });
                            } else {
                                let active_or_new = std::iter::once(antecedent.clone())
                                    .chain(references.iter().cloned())
                                    .reduce(|left, right| ExprIR::Bin {
                                        op: BinOp::Or,
                                        a: Box::new(left),
                                        b: Box::new(right),
                                    })
                                    .expect("repetition property has an antecedent");
                                statements.push(StmtIR::If {
                                    c: active_or_new,
                                    t: Box::new(make_check(consequent.clone())),
                                    f: None,
                                });
                            }

                            for age in (2..count).rev() {
                                statements.push(StmtIR::Bl {
                                    lv: LValue {
                                        net: stages[age - 1].base,
                                        decl_width: stages[age - 1].width,
                                        range: full_range(stages[age - 1].width),
                                    },
                                    e: ExprIR::Bin {
                                        op: BinOp::And,
                                        a: Box::new(references[age - 2].clone()),
                                        b: Box::new(consequent.clone()),
                                    },
                                });
                            }
                            statements.push(StmtIR::Bl {
                                lv: LValue {
                                    net: stages[0].base,
                                    decl_width: stages[0].width,
                                    range: full_range(stages[0].width),
                                },
                                e: ExprIR::Bin {
                                    op: BinOp::And,
                                    a: Box::new(antecedent),
                                    b: Box::new(consequent),
                                },
                            });
                            StmtIR::Block(statements)
                        }
                    } else if let Some((start, end)) = delay_range {
                        let mut stages = Vec::with_capacity(end);
                        for stage in 0..end {
                            let pending_name =
                                format!("__property_pending_{property_index}_{stage}");
                            stages.push(self.declare_net(
                                &pending_name,
                                prefix,
                                &None,
                                &HashMap::new(),
                                true,
                                false,
                                None,
                                None,
                            )?);
                        }
                        let references = stages
                            .iter()
                            .map(|pending| ExprIR::Ref {
                                net: pending.base,
                                decl_width: pending.width,
                                range: full_range(pending.width),
                                signed: false,
                            })
                            .collect::<Vec<_>>();
                        let candidate = |delay: usize| -> ExprIR {
                            if delay == 0 {
                                antecedent.clone()
                            } else {
                                references[delay - 1].clone()
                            }
                        };
                        let mut statements = Vec::new();

                        if *kind == ast::PropertyKind::Cover {
                            let any_candidate = (start..=end)
                                .map(&candidate)
                                .reduce(|left, right| ExprIR::Bin {
                                    op: BinOp::Or,
                                    a: Box::new(left),
                                    b: Box::new(right),
                                })
                                .expect("property delay range is nonempty");
                            statements.push(StmtIR::If {
                                c: ExprIR::Bin {
                                    op: BinOp::And,
                                    a: Box::new(any_candidate),
                                    b: Box::new(consequent.clone()),
                                },
                                t: Box::new(StmtIR::Cover {
                                    label: cover_label.clone(),
                                    cond: ExprIR::Const(LogicWord::from_u64(1, 1)),
                                }),
                                f: None,
                            });
                        }

                        for delay in start..=end {
                            if delay > 0 {
                                statements.push(StmtIR::If {
                                    c: ExprIR::Bin {
                                        op: BinOp::And,
                                        a: Box::new(references[delay - 1].clone()),
                                        b: Box::new(consequent.clone()),
                                    },
                                    t: Box::new(StmtIR::Bl {
                                        lv: LValue {
                                            net: stages[delay - 1].base,
                                            decl_width: stages[delay - 1].width,
                                            range: full_range(stages[delay - 1].width),
                                        },
                                        e: ExprIR::Const(LogicWord::zero(1)),
                                    }),
                                    f: None,
                                });
                            }
                        }

                        if *kind != ast::PropertyKind::Cover {
                            let expired = candidate(end);
                            let failure = make_check(consequent.clone());
                            if end > 0 {
                                statements.push(StmtIR::If {
                                    c: expired,
                                    t: Box::new(StmtIR::Block(vec![
                                        StmtIR::Bl {
                                            lv: LValue {
                                                net: stages[end - 1].base,
                                                decl_width: stages[end - 1].width,
                                                range: full_range(stages[end - 1].width),
                                            },
                                            e: ExprIR::Const(LogicWord::zero(1)),
                                        },
                                        failure,
                                    ])),
                                    f: None,
                                });
                            } else {
                                statements.push(StmtIR::If {
                                    c: expired,
                                    t: Box::new(failure),
                                    f: None,
                                });
                            }
                        }

                        for stage in (1..end).rev() {
                            statements.push(StmtIR::Bl {
                                lv: LValue {
                                    net: stages[stage].base,
                                    decl_width: stages[stage].width,
                                    range: full_range(stages[stage].width),
                                },
                                e: references[stage - 1].clone(),
                            });
                        }
                        if end > 0 {
                            let pending_value = ExprIR::Un {
                                op: UnOp::OrR,
                                a: Box::new(antecedent.clone()),
                            };
                            statements.push(StmtIR::Bl {
                                lv: LValue {
                                    net: stages[0].base,
                                    decl_width: stages[0].width,
                                    range: full_range(stages[0].width),
                                },
                                e: pending_value.clone(),
                            });
                            if start == 0 {
                                statements.push(StmtIR::If {
                                    c: ExprIR::Bin {
                                        op: BinOp::And,
                                        a: Box::new(pending_value),
                                        b: Box::new(consequent.clone()),
                                    },
                                    t: Box::new(StmtIR::Bl {
                                        lv: LValue {
                                            net: stages[0].base,
                                            decl_width: stages[0].width,
                                            range: full_range(stages[0].width),
                                        },
                                        e: ExprIR::Const(LogicWord::zero(1)),
                                    }),
                                    f: None,
                                });
                            }
                        }
                        StmtIR::Block(statements)
                    } else if delay_cycles == Some(0) {
                        StmtIR::If {
                            c: antecedent,
                            t: Box::new(make_check(consequent)),
                            f: None,
                        }
                    } else {
                        let Some(delay_cycles) = delay_cycles else {
                            unreachable!("property delay is either fixed or ranged")
                        };
                        let mut stages = Vec::with_capacity(delay_cycles);
                        for stage in 0..delay_cycles {
                            let pending_name =
                                format!("__property_pending_{property_index}_{stage}");
                            stages.push(self.declare_net(
                                &pending_name,
                                prefix,
                                &None,
                                &HashMap::new(),
                                true,
                                false,
                                None,
                                None,
                            )?);
                        }
                        let stage_references = stages
                            .iter()
                            .map(|pending| ExprIR::Ref {
                                net: pending.base,
                                decl_width: pending.width,
                                range: full_range(pending.width),
                                signed: false,
                            })
                            .collect::<Vec<_>>();
                        let trigger = stage_references[delay_cycles - 1].clone();
                        let pending_value = ExprIR::Un {
                            op: UnOp::OrR,
                            a: Box::new(antecedent),
                        };
                        let mut statements = vec![StmtIR::If {
                            c: trigger,
                            t: Box::new(make_check(consequent)),
                            f: None,
                        }];
                        for stage in (1..delay_cycles).rev() {
                            statements.push(StmtIR::NBl {
                                lv: LValue {
                                    net: stages[stage].base,
                                    decl_width: stages[stage].width,
                                    range: full_range(stages[stage].width),
                                },
                                e: stage_references[stage - 1].clone(),
                            });
                        }
                        statements.push(StmtIR::NBl {
                            lv: LValue {
                                net: stages[0].base,
                                decl_width: stages[0].width,
                                range: full_range(stages[0].width),
                            },
                            e: pending_value,
                        });
                        StmtIR::Block(statements)
                    };
                    let body = if composition_delay_cycles > 0 {
                        let right = composed_right
                            .clone()
                            .expect("delayed composition has a right expression");
                        let trigger =
                            composition_stage_references[composition_delay_cycles - 1].clone();
                        let mut composition_statements = vec![StmtIR::If {
                            c: trigger,
                            t: Box::new(make_direct_check(right)),
                            f: None,
                        }];
                        for stage in (1..composition_delay_cycles).rev() {
                            composition_statements.push(StmtIR::NBl {
                                lv: LValue {
                                    net: composition_stages[stage].base,
                                    decl_width: composition_stages[stage].width,
                                    range: full_range(composition_stages[stage].width),
                                },
                                e: composition_stage_references[stage - 1].clone(),
                            });
                        }
                        let first = &composition_stages[0];
                        composition_statements.push(StmtIR::NBl {
                            lv: LValue {
                                net: first.base,
                                decl_width: first.width,
                                range: full_range(first.width),
                            },
                            e: ExprIR::Const(LogicWord::zero(1)),
                        });
                        StmtIR::Block(vec![StmtIR::Block(composition_statements), body])
                    } else {
                        body
                    };
                    let pending_prefix = format!("{prefix}__property_pending_{property_index}_");
                    let composed_prefix = format!("{prefix}__property_composed_{property_index}_");
                    let repeat_prefix = format!("{prefix}__property_repeat_{property_index}_");
                    let goto_prefix = format!("{prefix}__property_goto_{property_index}_");
                    let clear_states = self.design.net_decls[property_state_base..]
                        .iter()
                        .filter(|declaration| {
                            declaration.name.starts_with(&pending_prefix)
                                || declaration.name.starts_with(&composed_prefix)
                                || declaration.name.starts_with(&repeat_prefix)
                                || declaration.name.starts_with(&goto_prefix)
                        })
                        .map(|declaration| StmtIR::Bl {
                            lv: LValue {
                                net: declaration.base,
                                decl_width: declaration.width,
                                range: full_range(declaration.width),
                            },
                            e: ExprIR::Const(LogicWord::zero(declaration.width)),
                        })
                        .collect::<Vec<_>>();
                    let body = StmtIR::If {
                        c: disable,
                        t: Box::new(StmtIR::Block(clear_states)),
                        f: Some(Box::new(body)),
                    };
                    self.design.processes.push(Process { sens, body });
                }
                ast::Item::Initial { body } => {
                    let b = self.elab_stmt(body, &env)?;
                    self.design.initials.push(b);
                }
                ast::Item::Gate { kind, name, args } => {
                    self.elab_gate(*kind, name, args, &env)?;
                }
                ast::Item::Inst {
                    module: sub,
                    name,
                    array_ranges,
                    params: overrides,
                    conns,
                } => {
                    self.elab_inst(
                        module,
                        sub,
                        name,
                        array_ranges,
                        overrides,
                        conns,
                        &env,
                        &mut interface_instances,
                    )?;
                }
                _ => {}
            }
        }
        if root_instance {
            let net_end = self.design.n_nets;
            let root = self
                .design
                .hierarchy
                .get_mut(root_index.expect("root hierarchy index"))
                .expect("root hierarchy record");
            root.net_end = net_end;
            root.ports = self
                .design
                .ports
                .iter()
                .map(|port| HierarchicalPort {
                    name: port.name.clone(),
                    direction: port.dir,
                    base: port.base,
                    width: port.width,
                    connections: vec![None; port.width],
                })
                .collect();
        }
        Ok(())
    }
}
