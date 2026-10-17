//! Elaborator module-domain implementation.

use super::super::*;

impl<'a> Elaborator<'a> {
    pub(super) fn elab_gate(
        &mut self,
        kind: ast::GateKind,
        name: &str,
        args: &[ast::Expr],
        env: &Env,
    ) -> Result<(), String> {
        if args.len() < 2 {
            return Err(format!("gate '{}' needs at least 2 connections", name));
        }
        let n_out = match kind {
            ast::GateKind::Buf | ast::GateKind::Not => args.len() - 1,
            _ => 1,
        };
        let mut ins = Vec::new();
        for i in &args[n_out..] {
            ins.push(self.elab_expr(i, env)?);
        }
        let combined = match kind {
            ast::GateKind::And => reduce(&ins, BinOp::And),
            ast::GateKind::Nand => ExprIR::Un {
                op: UnOp::Not,
                a: Box::new(reduce(&ins, BinOp::And)),
            },
            ast::GateKind::Or => reduce(&ins, BinOp::Or),
            ast::GateKind::Nor => ExprIR::Un {
                op: UnOp::Not,
                a: Box::new(reduce(&ins, BinOp::Or)),
            },
            ast::GateKind::Xor => reduce(&ins, BinOp::Xor),
            ast::GateKind::Xnor => ExprIR::Un {
                op: UnOp::Not,
                a: Box::new(reduce(&ins, BinOp::Xor)),
            },
            ast::GateKind::Buf => ins[0].clone(),
            ast::GateKind::Not => ExprIR::Un {
                op: UnOp::Not,
                a: Box::new(ins[0].clone()),
            },
        };
        for out in &args[..n_out] {
            let lv = self.elab_lvalue(out, env)?;
            self.design.assigns.push(Assign {
                lv,
                e: combined.clone(),
                delay: 0,
            });
        }
        Ok(())
    }

    pub(super) fn interface_fields(
        &self,
        interface: &ast::Module,
        prefix: &str,
        decl_base: usize,
        inst_name: &str,
    ) -> Result<HashMap<String, NetEntry>, String> {
        let mut fields = HashMap::new();
        for item in &interface.items {
            if let ast::Item::Net { names, .. } = item {
                for (field, _) in names {
                    let full = format!("{}{}", prefix, field);
                    let Some(declaration) = self.design.net_decls[decl_base..]
                        .iter()
                        .find(|d| d.name == full)
                    else {
                        return Err(format!(
                            "interface instance '{}': missing signal '{}'",
                            inst_name, field
                        ));
                    };
                    fields.insert(
                        field.clone(),
                        NetEntry {
                            base: declaration.base,
                            width: declaration.width,
                            msb: declaration.width as i64 - 1,
                            lsb: 0,
                            type_name: None,
                            is_signed: false,
                            array: None,
                            memory: None,
                        },
                    );
                }
            }
        }
        Ok(fields)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn elab_inst(
        &mut self,
        parent: &ast::Module,
        sub_name: &str,
        inst_name: &str,
        array_ranges: &[(Box<ast::Expr>, Box<ast::Expr>)],
        overrides: &[(Option<String>, ast::Expr)],
        conns: &[ast::Conn],
        env: &Env,
        interface_instances: &mut HashMap<String, InterfaceInstanceBinding>,
    ) -> Result<(), String> {
        let sub = match self.mods.get(sub_name) {
            Some(m) => *m,
            None => {
                return Err(format!(
                    "module '{}': instance '{}' of unknown module '{}'",
                    parent.name, inst_name, sub_name
                ))
            }
        };
        validate_instance_bindings(sub, inst_name, overrides, conns)?;
        // Child parameter defaults, then overrides.
        let mut child_params: HashMap<String, i64> = HashMap::new();
        let mut declared_order: Vec<String> = Vec::new();
        for item in &sub.items {
            if let ast::Item::Params(list) = item {
                for (n, v) in list {
                    let val = self.const_eval(v, &child_params, &sub.name)?;
                    child_params.insert(n.clone(), val);
                    declared_order.push(n.clone());
                }
            }
        }
        for (i, (name, v)) in overrides.iter().enumerate() {
            let val = self.const_eval(v, env.params, &parent.name)?;
            match name {
                Some(n) => {
                    child_params.insert(n.clone(), val);
                }
                None => {
                    if i < declared_order.len() {
                        child_params.insert(declared_order[i].clone(), val);
                    }
                }
            }
        }

        let child_prefix = format!("{}{}.", env.prefix, inst_name);
        let child_net_start = self.design.n_nets;
        let decl_base = self.design.net_decls.len();
        let parent_path = if env.prefix.is_empty() {
            Some(self.design.name.clone())
        } else {
            Some(format!(
                "{}.{}",
                self.design.name,
                env.prefix.trim_end_matches('.')
            ))
        };

        if sub.is_interface {
            if !conns.is_empty() {
                return Err(format!(
                    "instance '{}': interface '{}' does not support port connections",
                    inst_name, sub_name
                ));
            }
            let binding = if !array_ranges.is_empty() {
                let mut dimensions = Vec::with_capacity(array_ranges.len());
                let mut total_elements = 1_usize;
                for range in array_ranges {
                    let (left, right) = range;
                    let base = self.const_eval(left, env.params, &parent.name)?;
                    let last = self.const_eval(right, env.params, &parent.name)?;
                    let step = if base <= last { 1 } else { -1 };
                    let length = (base - last).unsigned_abs() as usize + 1;
                    if length == 0 || length > 256 {
                        return Err(format!(
                            "interface instance '{}': each array length must be between 1 and 256",
                            inst_name
                        ));
                    }
                    total_elements = total_elements
                        .checked_mul(length)
                        .filter(|total| *total <= 4096)
                        .ok_or_else(|| {
                            format!(
                                "interface instance '{}': total array elements exceed 4096",
                                inst_name
                            )
                        })?;
                    dimensions.push(InterfaceArrayDimension { base, step, length });
                }

                let mut index_sets = vec![Vec::<i64>::new()];
                for dimension in &dimensions {
                    let mut next_index_sets = Vec::new();
                    for index_set in &index_sets {
                        for element in 0..dimension.length {
                            let mut next = index_set.clone();
                            next.push(dimension.base + dimension.step * element as i64);
                            next_index_sets.push(next);
                        }
                    }
                    index_sets = next_index_sets;
                }

                let mut elements = Vec::with_capacity(total_elements);
                for index_set in &index_sets {
                    let index_text = index_set
                        .iter()
                        .map(|index| index.to_string())
                        .collect::<Vec<_>>()
                        .join("_");
                    let element_prefix = format!("{}{}_{index_text}.", env.prefix, inst_name);
                    let element_net_start = self.design.n_nets;
                    let element_decl_base = self.design.net_decls.len();
                    self.elab_module(sub_name, &element_prefix, &child_params, &HashMap::new())?;
                    let element_net_end = self.design.n_nets;
                    elements.push(self.interface_fields(
                        sub,
                        &element_prefix,
                        element_decl_base,
                        inst_name,
                    )?);
                    let hierarchy_indexes = index_set
                        .iter()
                        .map(|index| format!("[{index}]"))
                        .collect::<String>();
                    self.design.hierarchy.push(HierarchicalInstance {
                        path: format!(
                            "{}.{}{hierarchy_indexes}",
                            parent_path.clone().unwrap_or_default(),
                            inst_name
                        ),
                        module: sub.name.clone(),
                        parent: parent_path.clone(),
                        depth: env.prefix.matches('.').count() + 1,
                        net_start: element_net_start,
                        net_end: element_net_end,
                        ports: Vec::new(),
                    });
                }
                InterfaceInstanceBinding::Array {
                    dimensions,
                    elements,
                }
            } else {
                self.elab_module(sub_name, &child_prefix, &child_params, &HashMap::new())?;
                let child_net_end = self.design.n_nets;
                let fields = self.interface_fields(sub, &child_prefix, decl_base, inst_name)?;
                self.design.hierarchy.push(HierarchicalInstance {
                    path: format!("{}.{}", parent_path.clone().unwrap_or_default(), inst_name),
                    module: sub.name.clone(),
                    parent: parent_path,
                    depth: env.prefix.matches('.').count() + 1,
                    net_start: child_net_start,
                    net_end: child_net_end,
                    ports: Vec::new(),
                });
                InterfaceInstanceBinding::Single(fields)
            };
            if interface_instances
                .insert(inst_name.to_string(), binding)
                .is_some()
            {
                return Err(format!("duplicate interface instance '{}'", inst_name));
            }
            return Ok(());
        }
        if !array_ranges.is_empty() {
            return Err(format!(
                "instance '{}': module instance arrays are not yet supported",
                inst_name
            ));
        }

        // Child port list in declaration order.
        let sub_ports: Vec<(String, ast::DirSrc)> = {
            let mut dirs: HashMap<String, ast::DirSrc> = HashMap::new();
            let mut order: Vec<String> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for item in &sub.items {
                if let ast::Item::Port {
                    dir,
                    interface_name: None,
                    names,
                    ..
                } = item
                {
                    for n in names {
                        dirs.insert(n.clone(), *dir);
                        if seen.insert(n.clone()) {
                            order.push(n.clone());
                        }
                    }
                }
            }
            order.into_iter().map(|n| (n.clone(), dirs[&n])).collect()
        };

        let mut child_interfaces = HashMap::new();
        let mut interface_port_index = 0_usize;
        for item in &sub.items {
            if let ast::Item::Port {
                interface_name: Some(definition),
                names,
                ..
            } = item
            {
                for local_name in names {
                    let expression = find_conn(conns, local_name, interface_port_index)
                        .ok_or_else(|| {
                            format!(
                                "instance '{}': missing interface port '{}'",
                                inst_name, local_name
                            )
                        })?;
                    let fields: HashMap<String, NetEntry> = match expression {
                        ast::Expr::Ident(object_name) => interface_instances
                            .get(object_name)
                            .ok_or_else(|| {
                                format!(
                                    "instance '{}': '{}' is not an interface object",
                                    inst_name, object_name
                                )
                            })?
                            .select(&[])?
                            .clone(),
                        ast::Expr::Select(object_name, index) => {
                            let index = self.const_eval(index, env.params, &parent.name)?;
                            interface_instances
                                .get(object_name)
                                .ok_or_else(|| {
                                    format!(
                                        "instance '{}': '{}' is not an interface object",
                                        inst_name, object_name
                                    )
                                })?
                                .select(&[index])?
                                .clone()
                        }
                        ast::Expr::ArraySelect(object_name, indexes) => {
                            let mut resolved = Vec::with_capacity(indexes.len());
                            for index in indexes {
                                resolved.push(self.const_eval(index, env.params, &parent.name)?);
                            }
                            interface_instances
                                .get(object_name)
                                .ok_or_else(|| {
                                    format!(
                                        "instance '{}': '{}' is not an interface object",
                                        inst_name, object_name
                                    )
                                })?
                                .select(&resolved)?
                                .clone()
                        }
                        other => {
                            return Err(format!(
                                "instance '{}': interface port '{}' must connect an interface object or array element, found {:?}",
                                inst_name, local_name, other
                            ));
                        }
                    };
                    let interface = self.mods.get(definition).ok_or_else(|| {
                        format!(
                            "instance '{}': unknown interface '{}'",
                            inst_name, definition
                        )
                    })?;
                    if !interface.is_interface {
                        return Err(format!("'{}' is not an interface", definition));
                    }
                    child_interfaces.insert(local_name.clone(), fields);
                }
                interface_port_index += names.len();
            }
        }

        // Elaborate the child in place (flat design, prefixed names).
        self.elab_module(sub_name, &child_prefix, &child_params, &child_interfaces)?;
        let child_net_end = self.design.n_nets;

        // Locate child port nets.
        let mut port_nets: HashMap<String, (usize, usize)> = HashMap::new();
        for (n, _) in &sub_ports {
            let full = format!("{}{}", child_prefix, n);
            for d in &self.design.net_decls[decl_base..] {
                if d.name == full {
                    port_nets.insert(n.clone(), (d.base, d.width));
                    break;
                }
            }
        }

        let ports = sub_ports
            .iter()
            .filter_map(|(name, direction)| {
                let (base, width) = port_nets.get(name).copied()?;
                Some(HierarchicalPort {
                    name: name.clone(),
                    direction: match direction {
                        ast::DirSrc::In => Dir::In,
                        ast::DirSrc::Out => Dir::Out,
                        ast::DirSrc::InOut => Dir::In,
                    },
                    base,
                    width,
                    connections: vec![None; width],
                })
            })
            .collect::<Vec<_>>();
        let hierarchy_index = self.design.hierarchy.len();
        self.design.hierarchy.push(HierarchicalInstance {
            path: format!("{}.{}", parent_path.clone().unwrap_or_default(), inst_name),
            module: sub.name.clone(),
            parent: parent_path,
            depth: env.prefix.matches('.').count() + 1,
            net_start: child_net_start,
            net_end: child_net_end,
            ports,
        });
        let mut port_connections: HashMap<String, Vec<Option<usize>>> = sub_ports
            .iter()
            .filter_map(|(name, _)| {
                port_nets
                    .get(name)
                    .map(|(_, width)| (name.clone(), vec![None; *width]))
            })
            .collect();

        // First-class sparse `inout` unpacked-array ports share the parent
        // memory object instead of flattening every element into port nets.
        let mut connected_memory_ports = std::collections::HashSet::new();
        for (i, (pname, pdir)) in sub_ports.iter().enumerate() {
            if *pdir != ast::DirSrc::InOut {
                continue;
            }
            let Some(ast::Expr::Ident(parent_name)) = find_conn(conns, pname, i) else {
                continue;
            };
            let full_child_name = format!("{child_prefix}{pname}");
            let Some(child_memory) = self
                .design
                .memories
                .iter()
                .position(|memory| memory.name == full_child_name)
            else {
                continue;
            };
            let parent_entry = env.net(parent_name)?;
            let Some(parent_memory) = parent_entry.memory else {
                return Err(format!(
                    "instance '{}': inout memory port '{}' must connect to a sparse memory object",
                    inst_name, pname
                ));
            };
            let child = &self.design.memories[child_memory];
            let parent = &self.design.memories[parent_memory];
            if child.element_width != parent.element_width
                || child.element_msb != parent.element_msb
                || child.element_lsb != parent.element_lsb
                || child.dimensions != parent.dimensions
                || child.flat_length != parent.flat_length
            {
                return Err(format!(
                    "instance '{}': memory port '{}' shape does not match parent memory '{}'",
                    inst_name, pname, parent_name
                ));
            }
            self.design.memory_connections.push(MemoryConnection {
                child: child_memory,
                parent: parent_memory,
            });
            connected_memory_ports.insert(pname.clone());
        }

        // Connect ports.
        for (i, (pname, pdir)) in sub_ports.iter().enumerate() {
            if connected_memory_ports.contains(pname) {
                continue;
            }
            let (base, pwidth) = match port_nets.get(pname) {
                Some(v) => *v,
                None => continue,
            };
            let lv = LValue {
                net: base,
                decl_width: pwidth,
                range: full_range(pwidth),
            };
            if let Some(expr) = find_conn(conns, pname, i) {
                match pdir {
                    ast::DirSrc::In => {
                        let e = self.elab_expr(expr, env)?;
                        if let ExprIR::Ref { net, range, .. } = &e {
                            let parent_bits = range_bits(net, range);
                            if let Some(connections) = port_connections.get_mut(pname) {
                                for (connection, parent_bit) in
                                    connections.iter_mut().zip(parent_bits)
                                {
                                    *connection = Some(parent_bit);
                                }
                            }
                        }
                        self.design.assigns.push(Assign { lv, e, delay: 0 });
                    }
                    ast::DirSrc::Out => {
                        let target = self.elab_lvalue(expr, env).map_err(|_| {
                            format!(
                                "instance '{}': output port '{}' must connect to a net",
                                inst_name, pname
                            )
                        })?;
                        let parent_bits = range_bits(&target.net, &target.range);
                        if let Some(connections) = port_connections.get_mut(pname) {
                            for (connection, parent_bit) in connections.iter_mut().zip(parent_bits)
                            {
                                *connection = Some(parent_bit);
                            }
                        }
                        let e = ExprIR::Ref {
                            net: base,
                            decl_width: pwidth,
                            range: full_range(pwidth),
                            signed: false,
                        };
                        self.design.assigns.push(Assign {
                            lv: target,
                            e,
                            delay: 0,
                        });
                    }
                    ast::DirSrc::InOut => {
                        return Err(format!(
                            "instance '{}': inout ports are not supported",
                            inst_name
                        ))
                    }
                }
            }
        }
        for port in &mut self.design.hierarchy[hierarchy_index].ports {
            if let Some(connections) = port_connections.remove(&port.name) {
                port.connections = connections;
            }
        }
        Ok(())
    }
}

fn validate_instance_bindings(
    child: &ast::Module,
    instance: &str,
    overrides: &[(Option<String>, ast::Expr)],
    connections: &[ast::Conn],
) -> Result<(), String> {
    let mut declared_parameters = std::collections::HashSet::new();
    let mut declared_parameter_order = Vec::new();
    for item in &child.items {
        if let ast::Item::Params(parameters) = item {
            for (name, _) in parameters {
                if !declared_parameters.insert(name.clone()) {
                    return Err(format!(
                        "module '{}': duplicate parameter '{}'",
                        child.name, name
                    ));
                }
                declared_parameter_order.push(name.clone());
            }
        }
    }

    let mut named_parameters = std::collections::HashSet::new();
    let mut positional_parameters = 0_usize;
    for (name, _) in overrides {
        match name {
            Some(parameter) => {
                if !declared_parameters.contains(parameter) {
                    return Err(format!(
                        "instance '{}': module '{}' has no parameter '{}'",
                        instance, child.name, parameter
                    ));
                }
                if !named_parameters.insert(parameter.clone()) {
                    return Err(format!(
                        "instance '{}': duplicate parameter connection '{}'",
                        instance, parameter
                    ));
                }
            }
            None => positional_parameters += 1,
        }
    }
    if positional_parameters != 0 && !named_parameters.is_empty() {
        return Err(format!(
            "instance '{}': mixing ordered and named parameter connections is not allowed",
            instance
        ));
    }
    if positional_parameters > declared_parameter_order.len() {
        return Err(format!(
            "instance '{}': too many parameter connections given for '{}' ({} given, {} expected)",
            instance,
            child.name,
            positional_parameters,
            declared_parameter_order.len()
        ));
    }

    let mut declared_ports = std::collections::HashSet::new();
    let mut port_count = 0_usize;
    for item in &child.items {
        if let ast::Item::Port { names, .. } = item {
            for name in names {
                if declared_ports.insert(name.clone()) {
                    port_count += 1;
                }
            }
        }
    }

    let mut named_ports = std::collections::HashSet::new();
    let mut positional_ports = 0_usize;
    for connection in connections {
        match connection {
            ast::Conn::Named(name, _) => {
                if !declared_ports.contains(name) {
                    return Err(format!(
                        "instance '{}': port '{}' does not exist in '{}'",
                        instance, name, child.name
                    ));
                }
                if !named_ports.insert(name.clone()) {
                    return Err(format!(
                        "instance '{}': duplicate connection for port '{}'",
                        instance, name
                    ));
                }
            }
            ast::Conn::Pos(_) => positional_ports += 1,
        }
    }
    if positional_ports != 0 && !named_ports.is_empty() {
        return Err(format!(
            "instance '{}': mixing ordered and named port connections is not allowed",
            instance
        ));
    }
    if positional_ports > port_count {
        return Err(format!(
            "instance '{}': too many port connections given for '{}' ({} given, {} expected)",
            instance, child.name, positional_ports, port_count
        ));
    }

    Ok(())
}
