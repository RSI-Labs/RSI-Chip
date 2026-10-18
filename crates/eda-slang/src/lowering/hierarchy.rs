//! Hierarchy flattening and IR offset-remapping helpers.

use super::declaration_name;
use crate::ast_util::{expression_symbol, string_field};
use eda_core::ir::Design;

pub(super) fn full_range(width: usize) -> eda_core::ir::Range {
    if width == 1 {
        eda_core::ir::Range::Bit(0)
    } else {
        eda_core::ir::Range::Span {
            msb: width.saturating_sub(1),
            lsb: 0,
        }
    }
}

pub(super) fn range_parent_bits(
    net: usize,
    range: &eda_core::ir::Range,
) -> Option<Vec<Option<usize>>> {
    match range {
        eda_core::ir::Range::Bit(index) => Some(vec![Some(net + index)]),
        eda_core::ir::Range::Span { msb, lsb } => {
            if msb < lsb {
                return None;
            }
            Some((*lsb..=*msb).map(|index| Some(net + index)).collect())
        }
    }
}

pub(super) fn rewrite_hierarchy_path(path: &str, prefix: &str) -> String {
    if path.is_empty() {
        return prefix.to_string();
    }
    if path.starts_with('.') {
        format!("{prefix}{path}")
    } else {
        format!("{prefix}.{path}")
    }
}

pub(super) fn expression_parent_bits(
    expression: &eda_core::ir::Expr,
) -> Option<Vec<Option<usize>>> {
    match expression {
        eda_core::ir::Expr::Ref { net, range, .. } => range_parent_bits(*net, range),
        _ => None,
    }
}

pub(super) fn lvalue_parent_bits(lvalue: &eda_core::ir::LValue) -> Option<Vec<Option<usize>>> {
    range_parent_bits(lvalue.net, &lvalue.range)
}

pub(super) fn remap_assignment(assignment: &mut eda_core::ir::Assign, offset: usize) {
    assignment.lv.net += offset;
    remap_expression(&mut assignment.e, offset);
}

pub(super) fn remap_expression(expression: &mut eda_core::ir::Expr, offset: usize) {
    use eda_core::ir::Expr;
    match expression {
        Expr::Const(_) => {}
        Expr::Ref { net, .. } => *net += offset,
        Expr::Un { a, .. } => remap_expression(a, offset),
        Expr::Bin { a, b, .. } => {
            remap_expression(a, offset);
            remap_expression(b, offset);
        }
        Expr::Cond { c, t, f } => {
            remap_expression(c, offset);
            remap_expression(t, offset);
            remap_expression(f, offset);
        }
        Expr::Concat(parts) => parts
            .iter_mut()
            .for_each(|part| remap_expression(part, offset)),
        Expr::Repeat { e, .. } | Expr::Signed { e, .. } => remap_expression(e, offset),
        Expr::MemoryRead { indexes, .. } => {
            indexes
                .iter_mut()
                .for_each(|index| remap_expression(index, offset));
        }
        Expr::MemoryReadPart { indexes, start, .. } => {
            indexes
                .iter_mut()
                .for_each(|index| remap_expression(index, offset));
            remap_expression(start, offset);
        }
        Expr::Index { e, .. } => remap_expression(e, offset),
        Expr::StringLength { e } => remap_expression(e, offset),
        Expr::StringConcat { left, right } => {
            remap_expression(left, offset);
            remap_expression(right, offset);
        }
        Expr::StringGetByte { e, index } => {
            remap_expression(e, offset);
            remap_expression(index, offset);
        }
        Expr::StringCase { e, .. } => remap_expression(e, offset),
        Expr::StringCompare { left, right, .. } => {
            remap_expression(left, offset);
            remap_expression(right, offset);
        }
        Expr::StringPutc { e, index, value } => {
            remap_expression(e, offset);
            remap_expression(index, offset);
            remap_expression(value, offset);
        }
        Expr::StringSubstr { e, left, right } => {
            remap_expression(e, offset);
            remap_expression(left, offset);
            remap_expression(right, offset);
        }
        Expr::StringToRadix { e, .. } => remap_expression(e, offset),
        Expr::StringFromRadix { value, .. } => remap_expression(value, offset),
    }
}

pub(super) fn remap_statement(statement: &mut eda_core::ir::Stmt, offset: usize) {
    use eda_core::ir::Stmt;
    match statement {
        Stmt::Block(statements) => statements
            .iter_mut()
            .for_each(|statement| remap_statement(statement, offset)),
        Stmt::Forever(body) => remap_statement(body, offset),
        Stmt::NBl { lv, e } | Stmt::Bl { lv, e } => {
            lv.net += offset;
            remap_expression(e, offset);
        }
        Stmt::MemoryBl { indexes, e, .. } | Stmt::MemoryNBl { indexes, e, .. } => {
            indexes
                .iter_mut()
                .for_each(|index| remap_expression(index, offset));
            remap_expression(e, offset);
        }
        Stmt::MemoryBlPart {
            indexes, start, e, ..
        }
        | Stmt::MemoryNBlPart {
            indexes, start, e, ..
        } => {
            indexes
                .iter_mut()
                .for_each(|index| remap_expression(index, offset));
            remap_expression(start, offset);
            remap_expression(e, offset);
        }
        Stmt::BlIndexed { net, start, e, .. } | Stmt::NBlIndexed { net, start, e, .. } => {
            *net += offset;
            remap_expression(start, offset);
            remap_expression(e, offset);
        }
        Stmt::If { c, t, f } => {
            remap_expression(c, offset);
            remap_statement(t, offset);
            if let Some(f) = f {
                remap_statement(f, offset);
            }
        }
        Stmt::Case { e, arms, dflt } => {
            remap_expression(e, offset);
            for (expression, _) in arms {
                remap_expression(expression, offset);
            }
            if let Some(statement) = dflt {
                remap_statement(statement, offset);
            }
        }
        Stmt::Delay { body, .. } => remap_statement(body, offset),
        Stmt::Display { args, .. } => {
            args.iter_mut()
                .for_each(|argument| remap_expression(argument, offset));
        }
        Stmt::Assert { cond, .. } | Stmt::Assume { cond, .. } | Stmt::Cover { cond, .. } => {
            remap_expression(cond, offset)
        }
        Stmt::WriteMem { start, finish, .. } => {
            if let Some(start) = start {
                remap_expression(start, offset);
            }
            if let Some(finish) = finish {
                remap_expression(finish, offset);
            }
        }
        Stmt::Finish => {}
    }
}

pub(super) fn remap_process(process: &mut eda_core::ir::Process, offset: usize) {
    use eda_core::ir::Sens;
    pub(super) fn remap_sensitivity(sensitivity: &mut Sens, offset: usize) {
        match sensitivity {
            Sens::Comb => {}
            Sens::Any(net) | Sens::Posedge(net) | Sens::Negedge(net) => *net += offset,
            Sens::List(list) => list
                .iter_mut()
                .for_each(|item| remap_sensitivity(item, offset)),
        }
    }
    remap_sensitivity(&mut process.sens, offset);
    remap_statement(&mut process.body, offset);
}

pub(super) fn remap_expression_memory_ids(expression: &mut eda_core::ir::Expr, offset: usize) {
    use eda_core::ir::Expr;
    match expression {
        Expr::Const(_) => {}
        Expr::Ref { .. } => {}
        Expr::Un { a, .. } => remap_expression_memory_ids(a, offset),
        Expr::Bin { a, b, .. } => {
            remap_expression_memory_ids(a, offset);
            remap_expression_memory_ids(b, offset);
        }
        Expr::Cond { c, t, f } => {
            remap_expression_memory_ids(c, offset);
            remap_expression_memory_ids(t, offset);
            remap_expression_memory_ids(f, offset);
        }
        Expr::Concat(parts) => parts
            .iter_mut()
            .for_each(|part| remap_expression_memory_ids(part, offset)),
        Expr::Repeat { e, .. } | Expr::Signed { e, .. } => remap_expression_memory_ids(e, offset),
        Expr::MemoryRead {
            memory, indexes, ..
        } => {
            *memory += offset;
            indexes
                .iter_mut()
                .for_each(|index| remap_expression_memory_ids(index, offset));
        }
        Expr::MemoryReadPart {
            memory,
            indexes,
            start,
            ..
        } => {
            *memory += offset;
            indexes
                .iter_mut()
                .for_each(|index| remap_expression_memory_ids(index, offset));
            remap_expression_memory_ids(start, offset);
        }
        Expr::Index { e, .. } => remap_expression_memory_ids(e, offset),
        Expr::StringLength { e } => remap_expression_memory_ids(e, offset),
        Expr::StringConcat { left, right } => {
            remap_expression_memory_ids(left, offset);
            remap_expression_memory_ids(right, offset);
        }
        Expr::StringGetByte { e, index } => {
            remap_expression_memory_ids(e, offset);
            remap_expression_memory_ids(index, offset);
        }
        Expr::StringCase { e, .. } => remap_expression_memory_ids(e, offset),
        Expr::StringCompare { left, right, .. } => {
            remap_expression_memory_ids(left, offset);
            remap_expression_memory_ids(right, offset);
        }
        Expr::StringPutc { e, index, value } => {
            remap_expression_memory_ids(e, offset);
            remap_expression_memory_ids(index, offset);
            remap_expression_memory_ids(value, offset);
        }
        Expr::StringSubstr { e, left, right } => {
            remap_expression_memory_ids(e, offset);
            remap_expression_memory_ids(left, offset);
            remap_expression_memory_ids(right, offset);
        }
        Expr::StringToRadix { e, .. } => remap_expression_memory_ids(e, offset),
        Expr::StringFromRadix { value, .. } => remap_expression_memory_ids(value, offset),
    }
}

pub(super) fn remap_statement_memory_ids(statement: &mut eda_core::ir::Stmt, offset: usize) {
    use eda_core::ir::Stmt;
    match statement {
        Stmt::Block(statements) => statements
            .iter_mut()
            .for_each(|statement| remap_statement_memory_ids(statement, offset)),
        Stmt::Forever(body) => remap_statement_memory_ids(body, offset),
        Stmt::NBl { e, .. } | Stmt::Bl { e, .. } => remap_expression_memory_ids(e, offset),
        Stmt::MemoryBl {
            memory, indexes, e, ..
        }
        | Stmt::MemoryNBl {
            memory, indexes, e, ..
        } => {
            *memory += offset;
            indexes
                .iter_mut()
                .for_each(|index| remap_expression_memory_ids(index, offset));
            remap_expression_memory_ids(e, offset);
        }
        Stmt::MemoryBlPart {
            memory,
            indexes,
            start,
            e,
            ..
        }
        | Stmt::MemoryNBlPart {
            memory,
            indexes,
            start,
            e,
            ..
        } => {
            *memory += offset;
            indexes
                .iter_mut()
                .for_each(|index| remap_expression_memory_ids(index, offset));
            remap_expression_memory_ids(start, offset);
            remap_expression_memory_ids(e, offset);
        }
        Stmt::BlIndexed { start, e, .. } | Stmt::NBlIndexed { start, e, .. } => {
            remap_expression_memory_ids(start, offset);
            remap_expression_memory_ids(e, offset);
        }
        Stmt::If { c, t, f } => {
            remap_expression_memory_ids(c, offset);
            remap_statement_memory_ids(t, offset);
            if let Some(f) = f {
                remap_statement_memory_ids(f, offset);
            }
        }
        Stmt::Case { e, arms, dflt } => {
            remap_expression_memory_ids(e, offset);
            for (expression, _) in arms {
                remap_expression_memory_ids(expression, offset);
            }
            if let Some(statement) = dflt {
                remap_statement_memory_ids(statement, offset);
            }
        }
        Stmt::Delay { body, .. } => remap_statement_memory_ids(body, offset),
        Stmt::Display { args, .. } => {
            args.iter_mut()
                .for_each(|argument| remap_expression_memory_ids(argument, offset));
        }
        Stmt::Assert { cond, .. } | Stmt::Assume { cond, .. } | Stmt::Cover { cond, .. } => {
            remap_expression_memory_ids(cond, offset)
        }
        Stmt::WriteMem {
            memory,
            start,
            finish,
            ..
        } => {
            *memory += offset;
            if let Some(start) = start {
                remap_expression_memory_ids(start, offset);
            }
            if let Some(finish) = finish {
                remap_expression_memory_ids(finish, offset);
            }
        }
        Stmt::Finish => {}
    }
}

impl super::SlangAstLowerer {
    pub(super) fn lower_instance(&mut self, member: &serde_json::Value) -> Result<(), String> {
        let instance_name = declaration_name(member, "Instance")?;
        let scoped_instance_name = self
            .generate_path
            .iter()
            .chain(std::iter::once(&instance_name))
            .cloned()
            .collect::<Vec<_>>()
            .join(".");
        let body = member
            .get("body")
            .ok_or_else(|| "slang AST Instance has no body".to_string())?;
        let definition_name = string_field(body, "name").unwrap_or_default();
        let mut child = Self {
            design: Design::default(),
            symbols: std::collections::BTreeMap::new(),
            constants: std::collections::BTreeMap::new(),
            subroutines: std::collections::BTreeMap::new(),
            type_aliases: std::collections::BTreeMap::new(),
            type_enums: std::collections::BTreeMap::new(),
            enum_constants: std::collections::BTreeMap::new(),
            generate_path: Vec::new(),
            inline_stack: std::cell::RefCell::new(Vec::new()),
        };
        child.lower_body(body)?;

        let offset = self.design.n_nets;
        let memory_offset = self.design.memories.len();
        let child_net_start = offset;
        let child_ports = child.design.ports.clone();
        let child_memories = child.design.memories.clone();
        for memory in &child.design.memories {
            let mut memory = memory.clone();
            memory.name = format!("{scoped_instance_name}.{}", memory.name);
            self.design.memories.push(memory);
        }
        for mut connection in child.design.memory_connections {
            connection.child += memory_offset;
            connection.parent += memory_offset;
            self.design.memory_connections.push(connection);
        }
        for declaration in &child.design.net_decls {
            self.design.net_decls.push(eda_core::ir::NetDecl {
                name: format!("{scoped_instance_name}.{}", declaration.name),
                width: declaration.width,
                is_reg: declaration.is_reg,
                is_signed: declaration.is_signed,
                base: declaration.base + offset,
            });
        }
        self.design.n_nets += child.design.n_nets;

        for mut assignment in child.design.assigns {
            remap_assignment(&mut assignment, offset);
            remap_expression_memory_ids(&mut assignment.e, memory_offset);
            self.design.assigns.push(assignment);
        }
        for mut statement in child.design.initials {
            remap_statement(&mut statement, offset);
            remap_statement_memory_ids(&mut statement, memory_offset);
            self.design.initials.push(statement);
        }
        for mut process in child.design.processes {
            remap_process(&mut process, offset);
            remap_statement_memory_ids(&mut process.body, memory_offset);
            self.design.processes.push(process);
        }

        let nested_hierarchy = child.design.hierarchy;

        let mut connections = std::collections::BTreeMap::new();
        let empty_connections = Vec::new();
        let memory_ports: std::collections::BTreeMap<String, usize> = body
            .get("members")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&empty_connections)
            .iter()
            .filter_map(|port| {
                let name = string_field(port, "name")?;
                let is_inout_memory = string_field(port, "direction").as_deref() == Some("InOut")
                    && self
                        .declared_memory_type(&string_field(port, "type").unwrap_or_default())
                        .is_some();
                if !is_inout_memory {
                    return None;
                }
                let local_index = child_memories
                    .iter()
                    .position(|memory| memory.name == name)?;
                Some((name, local_index))
            })
            .collect();
        for connection in member
            .get("connections")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&empty_connections)
        {
            let port_name = connection
                .get("port")
                .and_then(|port| string_field(port, "name"))
                .ok_or_else(|| "instance connection has no port".to_string())?;
            let Some(port) = child_ports.iter().find(|port| port.name == port_name) else {
                return Err(format!(
                    "instance '{scoped_instance_name}' connects unknown child port '{port_name}'"
                ));
            };
            let expression = connection
                .get("expr")
                .ok_or_else(|| "instance connection has no expression".to_string())?;
            if let Some(local_memory) = memory_ports.get(&port_name).copied() {
                let parent_target = expression
                    .get("left")
                    .ok_or_else(|| "inout memory connection has no parent target".to_string())?;
                let parent_name = expression_symbol(parent_target).ok_or_else(|| {
                    format!("inout memory connection '{port_name}' has no parent symbol")
                })?;
                let parent_symbol = self
                    .symbols
                    .get(&parent_name)
                    .ok_or_else(|| format!("unknown inout memory parent '{parent_name}'"))?;
                let parent_memory = parent_symbol.memory.ok_or_else(|| {
                    format!("inout memory port '{port_name}' must connect to a sparse memory")
                })?;
                let child_memory = memory_offset + local_memory;
                let child = &self.design.memories[child_memory];
                let parent = &self.design.memories[parent_memory];
                if child.element_width != parent.element_width
                    || child.element_msb != parent.element_msb
                    || child.element_lsb != parent.element_lsb
                    || child.dimensions != parent.dimensions
                    || child.flat_length != parent.flat_length
                {
                    return Err(format!(
                        "instance '{scoped_instance_name}' inout memory port '{port_name}' shape does not match parent memory '{parent_name}'"
                    ));
                }
                self.design
                    .memory_connections
                    .push(eda_core::ir::MemoryConnection {
                        child: child_memory,
                        parent: parent_memory,
                    });
                continue;
            }
            if string_field(expression, "kind").as_deref() == Some("EmptyArgument") {
                continue;
            }
            match port.dir {
                eda_core::ir::Dir::In => {
                    let rhs = self.lower_expression(expression)?;
                    if let Some(parent_bits) = expression_parent_bits(&rhs) {
                        connections.insert(port_name.to_string(), parent_bits);
                    }
                    self.design.assigns.push(eda_core::ir::Assign {
                        lv: eda_core::ir::LValue {
                            net: port.base + offset,
                            decl_width: port.width,
                            range: full_range(port.width),
                        },
                        e: rhs,
                        delay: 0,
                    });
                }
                eda_core::ir::Dir::Out => {
                    if string_field(expression, "kind").as_deref() != Some("Assignment") {
                        return Err(format!(
                            "instance '{scoped_instance_name}' output port '{port_name}' has no parent assignment"
                        ));
                    }
                    let parent_target = expression
                        .get("left")
                        .ok_or_else(|| "output connection has no parent target".to_string())?;
                    let target = self.lower_lvalue(parent_target)?;
                    if let Some(parent_bits) = lvalue_parent_bits(&target) {
                        connections.insert(port_name.to_string(), parent_bits);
                    }
                    self.design.assigns.push(eda_core::ir::Assign {
                        lv: target,
                        e: eda_core::ir::Expr::Ref {
                            net: port.base + offset,
                            decl_width: port.width,
                            range: full_range(port.width),
                            signed: false,
                        },
                        delay: 0,
                    });
                }
            }
        }

        let ports = child_ports
            .iter()
            .map(|port| eda_core::ir::HierarchicalPort {
                name: port.name.clone(),
                direction: port.dir,
                base: port.base + offset,
                width: port.width,
                connections: connections
                    .get(&port.name)
                    .cloned()
                    .unwrap_or_else(|| vec![None; port.width]),
            })
            .collect::<Vec<_>>();
        self.design
            .hierarchy
            .push(eda_core::ir::HierarchicalInstance {
                path: format!("{}.{}", self.design.name, scoped_instance_name),
                module: definition_name,
                parent: Some(self.design.name.clone()),
                depth: 1,
                net_start: child_net_start,
                net_end: self.design.n_nets,
                ports,
            });
        let child_path = format!("{}.{}", self.design.name, scoped_instance_name);
        for mut nested in nested_hierarchy {
            nested.path = rewrite_hierarchy_path(&nested.path, &child_path);
            if let Some(parent) = nested.parent.as_deref() {
                nested.parent = Some(rewrite_hierarchy_path(parent, &child_path));
            }
            nested.depth += 1;
            nested.net_start += offset;
            nested.net_end += offset;
            for port in &mut nested.ports {
                port.base += offset;
                port.connections = port
                    .connections
                    .iter()
                    .map(|connection| connection.map(|net| net + offset))
                    .collect();
            }
            self.design.hierarchy.push(nested);
        }
        Ok(())
    }
}
