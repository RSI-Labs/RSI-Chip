//! Hierarchy flattening and IR offset-remapping helpers.

use super::{declaration_name, AstLoweredSymbol, InterfaceModportSymbols};
use crate::ast_util::{expression_symbol, string_field};
use eda_core::ir::Design;

const MAX_INSTANCE_ARRAY_ELEMENTS: usize = 4096;

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

fn instance_array_indexes(member: &serde_json::Value) -> Result<Vec<i64>, String> {
    let range = string_field(member, "range").unwrap_or_default();
    let range = range.trim().trim_start_matches('[').trim_end_matches(']');
    let Some((left, right)) = range.split_once(':') else {
        return Err(format!("instance array has invalid range '{range}'"));
    };
    let left = left
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("instance array has invalid left bound '{left}'"))?;
    let right = right
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("instance array has invalid right bound '{right}'"))?;
    if left <= right {
        Ok((left..=right).collect())
    } else {
        Ok((right..=left).rev().collect())
    }
}

fn count_instance_array_members(
    member: &serde_json::Value,
    count: &mut usize,
) -> Result<(), String> {
    let indexes = instance_array_indexes(member)?;
    let elements = member
        .get("members")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "slang InstanceArray has no members".to_string())?;
    if indexes.len() != elements.len() {
        return Err(format!(
            "instance array range has {} indexes but {} elaborated elements",
            indexes.len(),
            elements.len()
        ));
    }
    for element in elements {
        if string_field(element, "kind").as_deref() == Some("InstanceArray") {
            count_instance_array_members(element, count)?;
        } else {
            *count = count
                .checked_add(1)
                .ok_or_else(|| "instance array element count overflows".to_string())?;
        }
    }
    Ok(())
}

fn symbol_address(raw: &str) -> Result<u64, String> {
    raw.split_whitespace()
        .next()
        .and_then(|address| address.parse::<u64>().ok())
        .ok_or_else(|| format!("invalid slang symbol reference '{raw}'"))
}

fn interface_modport_key(interface_instance: &str, modport: &str) -> Result<String, String> {
    Ok(format!(
        "{}:{}",
        symbol_address(interface_instance)?,
        symbol_address(modport)?
    ))
}

fn connection_interface_modport_key(
    interface_modport_symbols: &std::collections::BTreeMap<String, InterfaceModportSymbols>,
    interface_modport_names: &std::collections::BTreeMap<String, String>,
    interface_instance: &str,
    modport: &str,
) -> Result<String, String> {
    let exact = interface_modport_key(interface_instance, modport)?;
    if interface_modport_symbols.contains_key(&exact) {
        return Ok(exact);
    }
    let instance_address = symbol_address(interface_instance)?;
    let modport_name = modport
        .split_whitespace()
        .next_back()
        .ok_or_else(|| format!("invalid modport reference '{modport}'"))?;
    let prefix = format!("{instance_address}:");
    interface_modport_names
        .iter()
        .find(|(key, name)| key.starts_with(&prefix) && name.as_str() == modport_name)
        .map(|(key, _)| key.clone())
        .ok_or(exact)
}

struct InterfaceSymbols {
    modports: std::collections::BTreeMap<String, InterfaceModportSymbols>,
    modport_names: std::collections::BTreeMap<String, String>,
    signals: std::collections::BTreeMap<String, AstLoweredSymbol>,
}

fn collect_interface_modport_symbols(
    instance: &serde_json::Value,
    body: &serde_json::Value,
    child_symbols: &std::collections::BTreeMap<String, AstLoweredSymbol>,
    child_offset: usize,
    memory_offset: usize,
) -> Result<InterfaceSymbols, String> {
    let mut result = InterfaceSymbols {
        modports: std::collections::BTreeMap::new(),
        modport_names: std::collections::BTreeMap::new(),
        signals: std::collections::BTreeMap::new(),
    };
    let empty_members = Vec::new();
    let members = body
        .get("members")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty_members);
    if !members
        .iter()
        .any(|member| string_field(member, "kind").as_deref() == Some("Modport"))
    {
        return Ok(result);
    }
    let instance_address = instance
        .get("addr")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "interface instance has no symbol address".to_string())?;
    for member in members {
        if !matches!(
            string_field(member, "kind").as_deref(),
            Some("Variable" | "Net" | "Port")
        ) {
            continue;
        }
        let member_name = string_field(member, "name").unwrap_or_default();
        let Some(symbol) = child_symbols.get(&member_name) else {
            continue;
        };
        let Some(address) = member.get("addr").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let base_instance_address = instance
            .get("eda_base_instance_address")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(instance_address);
        let synthesized_address = address
            .checked_add(instance_address)
            .and_then(|value| value.checked_sub(base_instance_address))
            .unwrap_or(address);
        let mut absolute = symbol.clone();
        absolute.base += child_offset;
        if let Some(memory) = absolute.memory.as_mut() {
            *memory += memory_offset;
        }
        result
            .signals
            .insert(format!("{address} {member_name}"), absolute.clone());
        result
            .signals
            .insert(format!("{synthesized_address} {member_name}"), absolute);
    }
    if let Some(hierarchical_symbols) = instance
        .get("eda_hierarchical_symbols")
        .and_then(|value| value.as_object())
    {
        for (symbol_id, member_name) in hierarchical_symbols {
            let Some(member_name) = member_name.as_str() else {
                continue;
            };
            let Some(symbol) = child_symbols.get(member_name) else {
                continue;
            };
            let mut absolute = symbol.clone();
            absolute.base += child_offset;
            if let Some(memory) = absolute.memory.as_mut() {
                *memory += memory_offset;
            }
            result.signals.insert(symbol_id.clone(), absolute);
        }
    }
    for member in members {
        if string_field(member, "kind").as_deref() != Some("Modport") {
            continue;
        }
        let modport_name = string_field(member, "name").unwrap_or_default();
        let modport_address = member
            .get("addr")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| format!("modport '{modport_name}' has no symbol address"))?;
        let modport_key = format!("{instance_address}:{modport_address}");
        let mut symbols = InterfaceModportSymbols::new();
        let empty_modport_ports = Vec::new();
        for port in member
            .get("members")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&empty_modport_ports)
        {
            if string_field(port, "kind").as_deref() != Some("ModportPort") {
                continue;
            }
            let port_name = string_field(port, "name").unwrap_or_default();
            let internal = string_field(port, "internalSymbol")
                .ok_or_else(|| format!("modport port '{port_name}' has no internal symbol"))?;
            let internal_name = internal
                .split_whitespace()
                .next_back()
                .ok_or_else(|| format!("modport port '{port_name}' has invalid internal symbol"))?;
            let symbol = child_symbols
                .get(internal_name)
                .ok_or_else(|| format!("unknown interface signal '{internal_name}'"))?;
            let port_address = port
                .get("addr")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| format!("modport port '{port_name}' has no symbol address"))?;
            let direction = match string_field(port, "direction").as_deref() {
                Some("In") => eda_core::ir::Dir::In,
                Some("Out") => eda_core::ir::Dir::Out,
                other => {
                    return Err(format!(
                        "modport port '{port_name}' has unsupported direction {other:?}"
                    ))
                }
            };
            let mut absolute = symbol.clone();
            absolute.base += child_offset;
            if let Some(memory) = absolute.memory.as_mut() {
                *memory += memory_offset;
            }
            result.signals.insert(internal, absolute.clone());
            symbols.insert(
                format!("{port_address} {port_name}"),
                super::AstInterfaceModportSymbol {
                    direction,
                    symbol: absolute,
                },
            );
        }
        result.modports.insert(modport_key.clone(), symbols);
        result
            .modport_names
            .insert(modport_key, modport_name.clone());
    }
    Ok(result)
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
        Expr::SimulationTime { .. } => {}
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
        Expr::EventTriggered { net } => *net += offset,
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
        Stmt::Wait { cond, body } => {
            remap_expression(cond, offset);
            remap_statement(body, offset);
        }
        Stmt::EventWait { watches, body } => {
            watches.iter_mut().for_each(|watch| watch.net += offset);
            remap_statement(body, offset);
        }
        Stmt::WaitOrder {
            events,
            success,
            failure,
        } => {
            events.iter_mut().for_each(|net| *net += offset);
            if let Some(statement) = success {
                remap_statement(statement, offset);
            }
            if let Some(statement) = failure {
                remap_statement(statement, offset);
            }
        }
        Stmt::Fork { branches, .. } => branches
            .iter_mut()
            .for_each(|branch| remap_statement(branch, offset)),
        Stmt::While { cond, body, steps } => {
            remap_expression(cond, offset);
            remap_statement(body, offset);
            steps
                .iter_mut()
                .for_each(|step| remap_statement(step, offset));
        }
        Stmt::Break | Stmt::Continue => {}
        Stmt::Repeat { count, body } => {
            remap_expression(count, offset);
            remap_statement(body, offset);
        }
        Stmt::Force { lv, e } => {
            lv.net += offset;
            remap_expression(e, offset);
        }
        Stmt::Release { lv } => lv.net += offset,
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
        Stmt::TimeFormat { .. } | Stmt::Finish => {}
    }
}

pub(super) fn remap_process(process: &mut eda_core::ir::Process, offset: usize) {
    use eda_core::ir::Sens;
    pub(super) fn remap_sensitivity(sensitivity: &mut Sens, offset: usize) {
        match sensitivity {
            Sens::Comb | Sens::Timed => {}
            Sens::Any(net) | Sens::Posedge(net) | Sens::Negedge(net) => *net += offset,
            Sens::List(list) => list
                .iter_mut()
                .for_each(|item| remap_sensitivity(item, offset)),
        }
    }
    remap_sensitivity(&mut process.sens, offset);
    remap_statement(&mut process.body, offset);
}

#[derive(Clone, Copy)]
struct NetRewrite {
    from: usize,
    to: usize,
    width: usize,
}

fn rewrite_net(net: usize, rewrites: &[NetRewrite]) -> usize {
    rewrites
        .iter()
        .find(|rewrite| net >= rewrite.from && net < rewrite.from + rewrite.width)
        .map(|rewrite| rewrite.to + (net - rewrite.from))
        .unwrap_or(net)
}

fn rewrite_lvalue_nets(lvalue: &mut eda_core::ir::LValue, rewrites: &[NetRewrite]) {
    lvalue.net = rewrite_net(lvalue.net, rewrites);
}

fn rewrite_expression_nets(expression: &mut eda_core::ir::Expr, rewrites: &[NetRewrite]) {
    use eda_core::ir::Expr;
    match expression {
        Expr::Const(_) => {}
        Expr::Ref { net, .. } => *net = rewrite_net(*net, rewrites),
        Expr::Un { a, .. } => rewrite_expression_nets(a, rewrites),
        Expr::Bin { a, b, .. } => {
            rewrite_expression_nets(a, rewrites);
            rewrite_expression_nets(b, rewrites);
        }
        Expr::Cond { c, t, f } => {
            rewrite_expression_nets(c, rewrites);
            rewrite_expression_nets(t, rewrites);
            rewrite_expression_nets(f, rewrites);
        }
        Expr::Concat(parts) => parts
            .iter_mut()
            .for_each(|part| rewrite_expression_nets(part, rewrites)),
        Expr::Repeat { e, .. } | Expr::Signed { e, .. } => rewrite_expression_nets(e, rewrites),
        Expr::SimulationTime { .. } => {}
        Expr::MemoryRead { indexes, .. } => indexes
            .iter_mut()
            .for_each(|index| rewrite_expression_nets(index, rewrites)),
        Expr::MemoryReadPart { indexes, start, .. } => {
            indexes
                .iter_mut()
                .for_each(|index| rewrite_expression_nets(index, rewrites));
            rewrite_expression_nets(start, rewrites);
        }
        Expr::Index { e, .. } => rewrite_expression_nets(e, rewrites),
        Expr::StringLength { e } => rewrite_expression_nets(e, rewrites),
        Expr::StringConcat { left, right } => {
            rewrite_expression_nets(left, rewrites);
            rewrite_expression_nets(right, rewrites);
        }
        Expr::StringGetByte { e, index } => {
            rewrite_expression_nets(e, rewrites);
            rewrite_expression_nets(index, rewrites);
        }
        Expr::StringCase { e, .. } => rewrite_expression_nets(e, rewrites),
        Expr::StringCompare { left, right, .. } => {
            rewrite_expression_nets(left, rewrites);
            rewrite_expression_nets(right, rewrites);
        }
        Expr::StringPutc { e, index, value } => {
            rewrite_expression_nets(e, rewrites);
            rewrite_expression_nets(index, rewrites);
            rewrite_expression_nets(value, rewrites);
        }
        Expr::StringSubstr { e, left, right } => {
            rewrite_expression_nets(e, rewrites);
            rewrite_expression_nets(left, rewrites);
            rewrite_expression_nets(right, rewrites);
        }
        Expr::StringToRadix { e, .. } => rewrite_expression_nets(e, rewrites),
        Expr::StringFromRadix { value, .. } => rewrite_expression_nets(value, rewrites),
        Expr::EventTriggered { net } => *net = rewrite_net(*net, rewrites),
    }
}

fn rewrite_statement_nets(statement: &mut eda_core::ir::Stmt, rewrites: &[NetRewrite]) {
    use eda_core::ir::Stmt;
    match statement {
        Stmt::Block(statements) => statements
            .iter_mut()
            .for_each(|statement| rewrite_statement_nets(statement, rewrites)),
        Stmt::Forever(body) => rewrite_statement_nets(body, rewrites),
        Stmt::NBl { lv, e } | Stmt::Bl { lv, e } => {
            rewrite_lvalue_nets(lv, rewrites);
            rewrite_expression_nets(e, rewrites);
        }
        Stmt::MemoryBl { indexes, e, .. } | Stmt::MemoryNBl { indexes, e, .. } => {
            indexes
                .iter_mut()
                .for_each(|index| rewrite_expression_nets(index, rewrites));
            rewrite_expression_nets(e, rewrites);
        }
        Stmt::MemoryBlPart {
            indexes, start, e, ..
        }
        | Stmt::MemoryNBlPart {
            indexes, start, e, ..
        } => {
            indexes
                .iter_mut()
                .for_each(|index| rewrite_expression_nets(index, rewrites));
            rewrite_expression_nets(start, rewrites);
            rewrite_expression_nets(e, rewrites);
        }
        Stmt::BlIndexed { net, start, e, .. } | Stmt::NBlIndexed { net, start, e, .. } => {
            *net = rewrite_net(*net, rewrites);
            rewrite_expression_nets(start, rewrites);
            rewrite_expression_nets(e, rewrites);
        }
        Stmt::If { c, t, f } => {
            rewrite_expression_nets(c, rewrites);
            rewrite_statement_nets(t, rewrites);
            if let Some(statement) = f {
                rewrite_statement_nets(statement, rewrites);
            }
        }
        Stmt::Case { e, arms, dflt } => {
            rewrite_expression_nets(e, rewrites);
            for (expression, _) in arms {
                rewrite_expression_nets(expression, rewrites);
            }
            if let Some(statement) = dflt {
                rewrite_statement_nets(statement, rewrites);
            }
        }
        Stmt::Delay { body, .. } => rewrite_statement_nets(body, rewrites),
        Stmt::Wait { cond, body } => {
            rewrite_expression_nets(cond, rewrites);
            rewrite_statement_nets(body, rewrites);
        }
        Stmt::EventWait { watches, body } => {
            watches
                .iter_mut()
                .for_each(|watch| watch.net = rewrite_net(watch.net, rewrites));
            rewrite_statement_nets(body, rewrites);
        }
        Stmt::WaitOrder {
            events,
            success,
            failure,
        } => {
            events
                .iter_mut()
                .for_each(|net| *net = rewrite_net(*net, rewrites));
            if let Some(statement) = success {
                rewrite_statement_nets(statement, rewrites);
            }
            if let Some(statement) = failure {
                rewrite_statement_nets(statement, rewrites);
            }
        }
        Stmt::Fork { branches, .. } => branches
            .iter_mut()
            .for_each(|branch| rewrite_statement_nets(branch, rewrites)),
        Stmt::While { cond, body, steps } => {
            rewrite_expression_nets(cond, rewrites);
            rewrite_statement_nets(body, rewrites);
            steps
                .iter_mut()
                .for_each(|step| rewrite_statement_nets(step, rewrites));
        }
        Stmt::Break | Stmt::Continue => {}
        Stmt::Repeat { count, body } => {
            rewrite_expression_nets(count, rewrites);
            rewrite_statement_nets(body, rewrites);
        }
        Stmt::Force { lv, e } => {
            rewrite_lvalue_nets(lv, rewrites);
            rewrite_expression_nets(e, rewrites);
        }
        Stmt::Release { lv } => rewrite_lvalue_nets(lv, rewrites),
        Stmt::Display { args, .. } => args
            .iter_mut()
            .for_each(|argument| rewrite_expression_nets(argument, rewrites)),
        Stmt::Assert { cond, .. } | Stmt::Assume { cond, .. } | Stmt::Cover { cond, .. } => {
            rewrite_expression_nets(cond, rewrites)
        }
        Stmt::WriteMem { start, finish, .. } => {
            if let Some(start) = start {
                rewrite_expression_nets(start, rewrites);
            }
            if let Some(finish) = finish {
                rewrite_expression_nets(finish, rewrites);
            }
        }
        Stmt::TimeFormat { .. } | Stmt::Finish => {}
    }
}

fn rewrite_process_nets(process: &mut eda_core::ir::Process, rewrites: &[NetRewrite]) {
    use eda_core::ir::Sens;
    fn rewrite_sensitivity(sensitivity: &mut Sens, rewrites: &[NetRewrite]) {
        match sensitivity {
            Sens::Comb | Sens::Timed => {}
            Sens::Any(net) | Sens::Posedge(net) | Sens::Negedge(net) => {
                *net = rewrite_net(*net, rewrites)
            }
            Sens::List(list) => list
                .iter_mut()
                .for_each(|item| rewrite_sensitivity(item, rewrites)),
        }
    }
    rewrite_sensitivity(&mut process.sens, rewrites);
    rewrite_statement_nets(&mut process.body, rewrites);
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
        Expr::EventTriggered { .. } | Expr::SimulationTime { .. } => {}
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
        Stmt::Wait { cond, body } => {
            remap_expression_memory_ids(cond, offset);
            remap_statement_memory_ids(body, offset);
        }
        Stmt::EventWait { body, .. } => remap_statement_memory_ids(body, offset),
        Stmt::WaitOrder {
            success, failure, ..
        } => {
            if let Some(statement) = success {
                remap_statement_memory_ids(statement, offset);
            }
            if let Some(statement) = failure {
                remap_statement_memory_ids(statement, offset);
            }
        }
        Stmt::Fork { branches, .. } => branches
            .iter_mut()
            .for_each(|branch| remap_statement_memory_ids(branch, offset)),
        Stmt::While { cond, body, steps } => {
            remap_expression_memory_ids(cond, offset);
            remap_statement_memory_ids(body, offset);
            steps
                .iter_mut()
                .for_each(|step| remap_statement_memory_ids(step, offset));
        }
        Stmt::Break | Stmt::Continue => {}
        Stmt::Repeat { count, body } => {
            remap_expression_memory_ids(count, offset);
            remap_statement_memory_ids(body, offset);
        }
        Stmt::Force { e, .. } => remap_expression_memory_ids(e, offset),
        Stmt::Release { .. } => {}
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
        Stmt::TimeFormat { .. } | Stmt::Finish => {}
    }
}

impl super::SlangAstLowerer {
    fn inject_interface_type_specialization(
        &self,
        child: &mut Self,
        interface_instance: &str,
    ) -> Result<(), String> {
        let instance_address = symbol_address(interface_instance)?;
        let prefix = format!("__interface_instance_{instance_address}__.");
        for (name, definition) in &self.type_aliases {
            let Some(visible_name) = name.strip_prefix(&prefix) else {
                continue;
            };
            if let Some(existing) = child.type_aliases.get(visible_name) {
                if *existing != *definition {
                    return Err(format!(
                        "child module uses conflicting interface type '{visible_name}'"
                    ));
                }
            } else {
                child
                    .type_aliases
                    .insert(visible_name.to_string(), definition.clone());
            }
        }
        for (name, definition) in &self.type_enums {
            let Some(visible_name) = name.strip_prefix(&prefix) else {
                continue;
            };
            if let Some(existing) = child.type_enums.get(visible_name) {
                if *existing != *definition {
                    return Err(format!(
                        "child module uses conflicting interface enum '{visible_name}'"
                    ));
                }
            } else {
                child
                    .type_enums
                    .insert(visible_name.to_string(), definition.clone());
            }
        }
        Ok(())
    }

    pub(super) fn lower_instance_array(
        &mut self,
        member: &serde_json::Value,
    ) -> Result<(), String> {
        let array_name = declaration_name(member, "InstanceArray")?;
        let mut count = 0_usize;
        count_instance_array_members(member, &mut count)?;
        if count > MAX_INSTANCE_ARRAY_ELEMENTS {
            return Err(format!(
                "instance array '{array_name}' has {count} elements; the direct path supports at most {MAX_INSTANCE_ARRAY_ELEMENTS}"
            ));
        }
        self.lower_instance_array_elements(member, &array_name, None)
    }

    fn lower_instance_array_elements(
        &mut self,
        member: &serde_json::Value,
        prefix: &str,
        base_instance_address: Option<u64>,
    ) -> Result<(), String> {
        let indexes = instance_array_indexes(member)?;
        let elements = member
            .get("members")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "slang InstanceArray has no members".to_string())?;
        if indexes.len() != elements.len() {
            return Err(format!(
                "instance array '{prefix}' range has {} indexes but {} elaborated elements",
                indexes.len(),
                elements.len()
            ));
        }
        let global_base = base_instance_address.or_else(|| Self::first_instance_address(member));
        for (index, element) in indexes.into_iter().zip(elements) {
            let element_name = format!("{prefix}[{index}]");
            match string_field(element, "kind").as_deref() {
                Some("Instance") => {
                    let mut instance = element.clone();
                    if let Some(name) = instance.get_mut("name") {
                        *name = serde_json::Value::String(element_name);
                    }
                    if let Some(fields) = instance.as_object_mut() {
                        fields.insert(
                            "eda_base_instance_address".into(),
                            serde_json::Value::from(global_base.unwrap_or_default()),
                        );
                    }
                    self.lower_instance(&instance)?;
                }
                Some("InstanceArray") => {
                    self.lower_instance_array_elements(element, &element_name, global_base)?;
                }
                other => {
                    return Err(format!(
                        "instance array '{prefix}' contains unsupported member kind {other:?}"
                    ))
                }
            }
        }
        Ok(())
    }

    fn first_instance_address(member: &serde_json::Value) -> Option<u64> {
        match string_field(member, "kind").as_deref() {
            Some("Instance") => member.get("addr").and_then(serde_json::Value::as_u64),
            Some("InstanceArray") => member
                .get("members")
                .and_then(serde_json::Value::as_array)?
                .iter()
                .find_map(Self::first_instance_address),
            _ => None,
        }
    }

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
        let child_offset = self.design.n_nets;
        let memory_offset = self.design.memories.len();
        let mut interface_proxy_connections = Vec::new();
        let mut interface_memory_connections = Vec::new();
        let mut child = Self {
            design: Design::default(),
            symbols: std::collections::BTreeMap::new(),
            local_symbols: std::collections::BTreeMap::new(),
            external_symbols: std::collections::BTreeMap::new(),
            external_symbol_directions: std::collections::BTreeMap::new(),
            local_initializers: std::collections::BTreeMap::new(),
            local_blocks: std::collections::BTreeMap::new(),
            constants: std::collections::BTreeMap::new(),
            subroutines: std::collections::BTreeMap::new(),
            type_aliases: std::collections::BTreeMap::new(),
            type_enums: std::collections::BTreeMap::new(),
            ambiguous_type_aliases: std::collections::BTreeSet::new(),
            ambiguous_type_enums: std::collections::BTreeSet::new(),
            class_definitions: self.class_definitions.clone(),
            class_objects: std::collections::BTreeMap::new(),
            class_object_types: std::collections::BTreeMap::new(),
            dynamic_class_sources: std::collections::BTreeMap::new(),
            runtime_dynamic_class_sources: std::cell::RefCell::new(
                std::collections::BTreeMap::new(),
            ),
            class_object_parent_keys: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            class_object_children: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            class_object_declared_names: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            class_object_declared_full_names: std::cell::RefCell::new(
                std::collections::BTreeMap::new(),
            ),
            initialized_class_objects: std::collections::BTreeSet::new(),
            constructed_class_objects: std::collections::BTreeSet::new(),
            class_object_tags: std::collections::BTreeMap::new(),
            class_object_ids: std::collections::BTreeMap::new(),
            class_object_names: std::collections::BTreeMap::new(),
            class_object_full_names: std::collections::BTreeMap::new(),
            class_object_parent_ids: std::collections::BTreeMap::new(),
            class_object_serial: None,
            class_static_methods: std::collections::BTreeMap::new(),
            class_static_properties: std::collections::BTreeMap::new(),
            class_static_initializer_count: 0,
            class_object_arrays: std::collections::BTreeMap::new(),
            class_array_constructor_temps: std::collections::BTreeMap::new(),
            class_property_owners: std::collections::BTreeMap::new(),
            class_property_temps: std::collections::BTreeMap::new(),
            class_property_initializers: std::collections::BTreeMap::new(),
            run_test_objects: std::collections::BTreeMap::new(),
            uvm_factory_type_aliases: Vec::new(),
            uvm_factory_instance_aliases: Vec::new(),
            uvm_factory_overrides: Vec::new(),
            uvm_factory_instance_overrides: Vec::new(),
            uvm_config_entries: std::cell::RefCell::new(Vec::new()),
            uvm_config_waits: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            active_uvm_phase: std::cell::RefCell::new(None),
            class_composition_children: std::collections::BTreeMap::new(),
            class_composition_parents: std::collections::BTreeMap::new(),
            active_composition_classes: std::collections::BTreeSet::new(),
            active_class_objects: std::cell::RefCell::new(Vec::new()),
            next_synthetic_class_handle: u64::MAX / 2,
            enum_constants: std::collections::BTreeMap::new(),
            named_block_flags: std::collections::BTreeMap::new(),
            named_disable_seen: false,
            clocking_events: std::collections::BTreeMap::new(),
            default_clocking_event: None,
            clocking_variables: std::collections::BTreeMap::new(),
            interface_modport_symbols: std::collections::BTreeMap::new(),
            interface_modport_names: std::collections::BTreeMap::new(),
            time_scale: super::body_time_scale(body),
            global_time_precision: self.global_time_precision,
            active_named_blocks: std::cell::RefCell::new(Vec::new()),
            generate_path: Vec::new(),
            inline_stack: std::cell::RefCell::new(Vec::new()),
        };
        let empty_connections = Vec::new();
        let mut lowered_connections = Vec::new();
        for connection in member
            .get("connections")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&empty_connections)
        {
            if let Some(elements) = connection
                .get("eda_interface_array_connections")
                .and_then(serde_json::Value::as_array)
            {
                lowered_connections.extend(elements.iter().cloned());
            } else {
                lowered_connections.push(connection.clone());
            }
        }
        for connection in &lowered_connections {
            let (Some(interface_instance), Some(modport)) = (
                connection
                    .get("ifaceInstance")
                    .and_then(serde_json::Value::as_str),
                connection
                    .get("modport")
                    .and_then(serde_json::Value::as_str),
            ) else {
                continue;
            };
            self.inject_interface_type_specialization(&mut child, interface_instance)?;
            let key = connection_interface_modport_key(
                &self.interface_modport_symbols,
                &self.interface_modport_names,
                interface_instance,
                modport,
            )?;
            let Some(modport_symbols) = self.interface_modport_symbols.get(&key) else {
                return Err(format!(
                    "instance '{scoped_instance_name}' connects unknown interface/modport {interface_instance} {modport}"
                ));
            };
            for (symbol_id, modport_symbol) in modport_symbols {
                let port_name = symbol_id
                    .split_whitespace()
                    .next_back()
                    .ok_or_else(|| "interface modport symbol has no name".to_string())?;
                let external_symbol_id = connection
                    .get("eda_external_symbols")
                    .and_then(|symbols| symbols.get(symbol_id))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(symbol_id)
                    .to_string();
                let proxy_name = connection
                    .get("eda_proxy_suffix")
                    .and_then(serde_json::Value::as_str)
                    .map(|suffix| format!("__interface_{suffix}_{port_name}"))
                    .unwrap_or_else(|| format!("__interface_{port_name}"));
                if let Some(interface_memory_index) = modport_symbol.symbol.memory {
                    let interface_memory = self.design.memories[interface_memory_index].clone();
                    let proxy_base = child.design.n_nets;
                    child.design.n_nets += 1;
                    child.design.net_decls.push(eda_core::ir::NetDecl {
                        name: proxy_name.clone(),
                        width: 1,
                        is_reg: true,
                        is_signed: modport_symbol.symbol.signed,
                        base: proxy_base,
                    });
                    let mut proxy_memory = interface_memory;
                    proxy_memory.name = proxy_name;
                    let proxy_memory_index = child.design.memories.len();
                    child.design.memories.push(proxy_memory);
                    let mut proxy = modport_symbol.symbol.clone();
                    proxy.base = proxy_base;
                    proxy.memory = Some(proxy_memory_index);
                    child
                        .external_symbols
                        .insert(external_symbol_id.clone(), proxy);
                    child
                        .external_symbol_directions
                        .insert(external_symbol_id, modport_symbol.direction);
                    interface_memory_connections.push((interface_memory_index, proxy_memory_index));
                    continue;
                }
                let proxy_base = child.design.n_nets;
                child.design.n_nets += modport_symbol.symbol.width;
                child.design.net_decls.push(eda_core::ir::NetDecl {
                    name: proxy_name,
                    width: modport_symbol.symbol.width,
                    is_reg: true,
                    is_signed: modport_symbol.symbol.signed,
                    base: proxy_base,
                });
                let mut proxy = modport_symbol.symbol.clone();
                proxy.base = proxy_base;
                child
                    .external_symbols
                    .insert(external_symbol_id.clone(), proxy.clone());
                child
                    .external_symbol_directions
                    .insert(external_symbol_id, modport_symbol.direction);
                interface_proxy_connections.push((
                    modport_symbol.direction,
                    modport_symbol.symbol.clone(),
                    proxy,
                ));
            }
        }
        let empty_body_members = Vec::new();
        let is_interface = body
            .get("members")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&empty_body_members)
            .iter()
            .any(|member| string_field(member, "kind").as_deref() == Some("Modport"));
        if !is_interface {
            for (name, definition) in &self.type_aliases {
                if name.contains('.') && !name.starts_with("__interface_instance_") {
                    child.type_aliases.insert(name.clone(), definition.clone());
                }
            }
            for (name, definition) in &self.type_enums {
                if name.contains('.') && !name.starts_with("__interface_instance_") {
                    child.type_enums.insert(name.clone(), definition.clone());
                }
            }
        }
        child.lower_body(body)?;

        if is_interface {
            let instance_address = member
                .get("addr")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| "interface instance has no address".to_string())?;
            let specialization_prefix = format!("__interface_instance_{instance_address}__.");
            for (name, definition) in &child.type_aliases {
                let specialized_name = format!("{specialization_prefix}{definition_name}.{name}");
                self.type_aliases
                    .insert(specialized_name, definition.clone());

                let scoped_name = format!("{definition_name}.{name}");
                if self.ambiguous_type_aliases.contains(&scoped_name) {
                    continue;
                }
                if let Some(existing) = self.type_aliases.get(&scoped_name) {
                    if *existing != *definition {
                        self.type_aliases.remove(&scoped_name);
                        self.ambiguous_type_aliases.insert(scoped_name);
                    }
                } else {
                    self.type_aliases.insert(scoped_name, definition.clone());
                }
            }
            for (name, definition) in &child.type_enums {
                let specialized_name = format!("{specialization_prefix}{definition_name}.{name}");
                self.type_enums.insert(specialized_name, definition.clone());

                let scoped_name = format!("{definition_name}.{name}");
                if self.ambiguous_type_enums.contains(&scoped_name) {
                    continue;
                }
                if let Some(existing) = self.type_enums.get(&scoped_name) {
                    if *existing != *definition {
                        self.type_enums.remove(&scoped_name);
                        self.ambiguous_type_enums.insert(scoped_name);
                    }
                } else {
                    self.type_enums.insert(scoped_name, definition.clone());
                }
            }
        }

        let interface_symbols = collect_interface_modport_symbols(
            member,
            body,
            &child.symbols,
            child_offset,
            memory_offset,
        )?;
        self.interface_modport_symbols
            .extend(interface_symbols.modports);
        self.interface_modport_names
            .extend(interface_symbols.modport_names);
        for (symbol_id, symbol) in interface_symbols.signals {
            self.external_symbols.entry(symbol_id).or_insert(symbol);
        }

        let offset = child_offset;
        let child_net_start = offset;
        let child_ports = child.design.ports.clone();
        let child_memories = child.design.memories.clone();
        let mut static_net_rewrites = Vec::new();
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
        for (parent, child) in interface_memory_connections {
            self.design
                .memory_connections
                .push(eda_core::ir::MemoryConnection {
                    parent,
                    child: child + memory_offset,
                });
        }
        for declaration in &child.design.net_decls {
            if declaration.name.starts_with("__class_static_") {
                let Some(parent_symbol) = self.class_static_properties.get(&declaration.name)
                else {
                    return Err(format!(
                        "hierarchy-global class static '{}' is missing from the root scope",
                        declaration.name
                    ));
                };
                if parent_symbol.width != declaration.width
                    || parent_symbol.signed != declaration.is_signed
                {
                    return Err(format!(
                        "hierarchy-global class static '{}' has conflicting layouts",
                        declaration.name
                    ));
                }
                static_net_rewrites.push(NetRewrite {
                    from: declaration.base + offset,
                    to: parent_symbol.base,
                    width: declaration.width,
                });
                continue;
            }
            if declaration.name.contains(".__class_static_") {
                continue;
            }
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
            rewrite_lvalue_nets(&mut assignment.lv, &static_net_rewrites);
            rewrite_expression_nets(&mut assignment.e, &static_net_rewrites);
            remap_expression_memory_ids(&mut assignment.e, memory_offset);
            self.design.assigns.push(assignment);
        }
        for (initializer_index, mut statement) in child.design.initials.into_iter().enumerate() {
            if initializer_index < child.class_static_initializer_count {
                continue;
            }
            remap_statement(&mut statement, offset);
            rewrite_statement_nets(&mut statement, &static_net_rewrites);
            remap_statement_memory_ids(&mut statement, memory_offset);
            self.design.initials.push(statement);
        }
        for mut statement in child.design.final_blocks {
            remap_statement(&mut statement, offset);
            rewrite_statement_nets(&mut statement, &static_net_rewrites);
            remap_statement_memory_ids(&mut statement, memory_offset);
            self.design.final_blocks.push(statement);
        }
        for mut process in child.design.processes {
            remap_process(&mut process, offset);
            rewrite_process_nets(&mut process, &static_net_rewrites);
            remap_statement_memory_ids(&mut process.body, memory_offset);
            self.design.processes.push(process);
        }
        for (direction, interface_symbol, proxy) in interface_proxy_connections {
            let mut proxy = proxy;
            proxy.base += offset;
            let (target, source) = match direction {
                eda_core::ir::Dir::Out => (interface_symbol, proxy),
                eda_core::ir::Dir::In => (proxy, interface_symbol),
            };
            let target_lvalue = eda_core::ir::LValue {
                net: target.base,
                decl_width: target.width,
                range: full_range(target.width),
            };
            let source_expression = eda_core::ir::Expr::Ref {
                net: source.base,
                decl_width: source.width,
                range: full_range(source.width),
                signed: source.signed,
            };
            self.design.assigns.push(eda_core::ir::Assign {
                lv: target_lvalue,
                e: source_expression,
                delay: 0,
            });
        }
        for mut clocking_block in child.design.clocking_blocks {
            clocking_block.name = format!("{scoped_instance_name}.{}", clocking_block.name);
            clocking_block.event.net += offset;
            for input in &mut clocking_block.inputs {
                input.target.net += offset;
                input.sample.net += offset;
            }
            self.design.clocking_blocks.push(clocking_block);
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
                let is_memory = matches!(
                    string_field(port, "direction").as_deref(),
                    Some("In" | "Out" | "InOut")
                ) && self
                    .declared_memory_type(&string_field(port, "type").unwrap_or_default())
                    .is_some();
                if !is_memory {
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
            if connection.get("ifaceInstance").is_some() {
                continue;
            }
            let Some(port) = child_ports.iter().find(|port| port.name == port_name) else {
                return Err(format!(
                    "instance '{scoped_instance_name}' connects unknown child port '{port_name}'"
                ));
            };
            let expression = connection
                .get("expr")
                .ok_or_else(|| "instance connection has no expression".to_string())?;
            if let Some(local_memory) = memory_ports.get(&port_name).copied() {
                let parent_target =
                    if string_field(expression, "kind").as_deref() == Some("Assignment") {
                        expression
                            .get("left")
                            .ok_or_else(|| "memory connection has no parent target".to_string())?
                    } else {
                        expression
                    };
                let parent_name = expression_symbol(parent_target).ok_or_else(|| {
                    format!("memory connection '{port_name}' has no parent symbol")
                })?;
                let parent_symbol = self
                    .symbols
                    .get(&parent_name)
                    .ok_or_else(|| format!("unknown memory parent '{parent_name}'"))?;
                let parent_memory = parent_symbol.memory.ok_or_else(|| {
                    format!("memory port '{port_name}' must connect to a sparse memory")
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
                        "instance '{scoped_instance_name}' memory port '{port_name}' shape does not match parent memory '{parent_name}'"
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
