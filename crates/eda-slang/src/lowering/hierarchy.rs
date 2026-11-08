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
    // A child fragment stores descendant paths relative to its definition, as
    // in `middle.u`. When merged under `top.u`, SystemVerilog's instance path
    // must not repeat the module name: the result is `top.u.u`.
    let suffix = path
        .strip_prefix('.')
        .unwrap_or(path)
        .split_once('.')
        .map(|(_, suffix)| suffix)
        .unwrap_or("");
    if suffix.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}.{suffix}")
    }
}

/// Conservatively reject reusable unit fragments when a source string mentions
/// `%m`. Such a format bakes in the selected instance's hierarchical path, so
/// one cached Design cannot be shared by sibling instances.
fn value_contains_hierarchy_format(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(text) => text.contains("%m") || text.contains("%M"),
        serde_json::Value::Array(values) => values.iter().any(value_contains_hierarchy_format),
        serde_json::Value::Object(fields) => fields.values().any(value_contains_hierarchy_format),
        _ => false,
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
        Expr::Const(_) | Expr::RealConst(_) | Expr::ShortRealConst(_) | Expr::ProcessSelf => {}
        Expr::Ref { net, .. } => *net += offset,
        Expr::RealRef { net, .. } => *net += offset,
        Expr::ShortRealRef { net } => *net += offset,
        Expr::ProcessStatus(handle) => remap_expression(handle, offset),
        Expr::Past { value, gating, .. } => {
            remap_expression(value, offset);
            if let Some(gating) = gating {
                remap_expression(gating, offset);
            }
        }
        Expr::Sampled(value) => remap_expression(value, offset),
        Expr::SampledValue { value, .. } => remap_expression(value, offset),
        Expr::RealBinary { left, right, .. } => {
            remap_expression(left, offset);
            remap_expression(right, offset);
        }
        Expr::RealCompare { left, right, .. } => {
            remap_expression(left, offset);
            remap_expression(right, offset);
        }
        Expr::RealNeg(operand) => remap_expression(operand, offset),
        Expr::ShortRealBinary { left, right, .. } => {
            remap_expression(left, offset);
            remap_expression(right, offset);
        }
        Expr::ShortRealNeg(operand)
        | Expr::ShortRealToReal(operand)
        | Expr::RealToShortReal(operand)
        | Expr::BitsToReal(operand)
        | Expr::BitsToShortReal(operand) => remap_expression(operand, offset),
        Expr::RealToInt { value, .. }
        | Expr::RealTruncToInt { value, .. }
        | Expr::IntToReal { value, .. } => remap_expression(value, offset),
        Expr::RealTime { .. } => {}
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
        Expr::MemorySize { .. } => {}
        Expr::AssocRead { index, .. } | Expr::AssocExists { index, .. } => {
            remap_expression(index, offset)
        }
        Expr::Index { e, .. } => remap_expression(e, offset),
        Expr::StringLength { e } => remap_expression(e, offset),
        Expr::StringConcat { left, right } => {
            remap_expression(left, offset);
            remap_expression(right, offset);
        }
        Expr::StringFormat { args, .. } => {
            args.iter_mut()
                .for_each(|argument| remap_expression(argument, offset));
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
        Expr::MemoryPattern { memory, .. } => *memory += offset,
        Expr::EnumPattern { value, .. } => remap_expression(value, offset),
        Expr::StructPattern { value, .. } => remap_expression(value, offset),
        Expr::EventTriggered { net } => *net += offset,
        Expr::Random { args, .. } => args
            .iter_mut()
            .for_each(|argument| remap_expression(argument, offset)),
        Expr::CountBits { value, .. }
        | Expr::IsUnknown(value)
        | Expr::OneHot(value)
        | Expr::OneHot0(value) => remap_expression(value, offset),
        Expr::FileOpen { .. } => {}
        Expr::FileOpenDynamic { path, mode } => {
            remap_expression(path, offset);
            remap_expression(mode, offset);
        }
        Expr::FileGetc { fd } => remap_expression(fd, offset),
        Expr::FileUngetc { value, fd } => {
            remap_expression(value, offset);
            remap_expression(fd, offset);
        }
        Expr::FileEof { fd } => remap_expression(fd, offset),
        Expr::FileTell { fd } => remap_expression(fd, offset),
        Expr::FileSeek {
            fd,
            offset: seek_offset,
            whence,
        } => {
            remap_expression(fd, offset);
            remap_expression(seek_offset, offset);
            remap_expression(whence, offset);
        }
    }
}

pub(super) fn remap_statement(statement: &mut eda_core::ir::Stmt, offset: usize) {
    use eda_core::ir::Stmt;
    match statement {
        Stmt::Block(statements) => statements
            .iter_mut()
            .for_each(|statement| remap_statement(statement, offset)),
        Stmt::Forever(body) => remap_statement(body, offset),
        Stmt::NamedBlock { body, .. } => remap_statement(body, offset),
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
        Stmt::MemoryCopy { .. } => {}
        Stmt::MemoryNew { size, .. } => remap_expression(size, offset),
        Stmt::MemoryDelete { .. } => {}
        Stmt::AssocTraversal {
            key, destination, ..
        } => {
            key.net += offset;
            destination.net += offset;
        }
        Stmt::QueuePush { value, .. } => remap_expression(value, offset),
        Stmt::QueuePop { destination, .. } => destination.net += offset,
        Stmt::QueueInsert { index, value, .. } => {
            remap_expression(index, offset);
            remap_expression(value, offset);
        }
        Stmt::QueueDelete { index, .. } => remap_expression(index, offset),
        Stmt::AssocWrite { index, value, .. } => {
            remap_expression(index, offset);
            remap_expression(value, offset);
        }
        Stmt::AssocDelete { index, .. } => {
            if let Some(index) = index {
                remap_expression(index, offset);
            }
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
            for (expression, statement) in arms {
                remap_expression(expression, offset);
                remap_statement(statement, offset);
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
        Stmt::Break | Stmt::Continue | Stmt::ReturnBarrier | Stmt::Return => {}
        Stmt::Scope(body) => remap_statement(body, offset),
        Stmt::Repeat { count, body } => {
            remap_expression(count, offset);
            remap_statement(body, offset);
        }
        Stmt::Force { lv, e } => {
            lv.net += offset;
            remap_expression(e, offset);
        }
        Stmt::Release { lv } => lv.net += offset,
        Stmt::ProceduralAssign { lv, e } => {
            lv.net += offset;
            remap_expression(e, offset);
        }
        Stmt::ProceduralDeassign { lv } => lv.net += offset,
        Stmt::DisableFork | Stmt::WaitFork | Stmt::NamedBlockEnd => {}
        Stmt::DisableNamedBlock { flag, .. } => *flag += offset,
        Stmt::ProcessKill(handle)
        | Stmt::ProcessAwait(handle)
        | Stmt::ProcessSuspend(handle)
        | Stmt::ProcessResume(handle) => remap_expression(handle, offset),
        Stmt::Display { args, .. } => {
            args.iter_mut()
                .for_each(|argument| remap_expression(argument, offset));
        }
        Stmt::FileClose { fd } => remap_expression(fd, offset),
        Stmt::FileFlush { fd } => remap_expression(fd, offset),
        Stmt::FileGets { fd, string, status } => {
            remap_expression(fd, offset);
            string.net += offset;
            status.net += offset;
        }
        Stmt::FileError {
            fd,
            message,
            status,
        } => {
            remap_expression(fd, offset);
            message.net += offset;
            status.net += offset;
        }
        Stmt::FileRead { fd, target, status } => {
            remap_expression(fd, offset);
            target.net += offset;
            status.net += offset;
        }
        Stmt::FileReadMemory {
            fd,
            start,
            count,
            status,
            ..
        } => {
            remap_expression(fd, offset);
            if let Some(start) = start {
                remap_expression(start, offset);
            }
            if let Some(count) = count {
                remap_expression(count, offset);
            }
            status.net += offset;
        }
        Stmt::FileScan {
            fd,
            targets,
            status,
            ..
        } => {
            remap_expression(fd, offset);
            for target in targets {
                target.net += offset;
            }
            status.net += offset;
        }
        Stmt::StringScan {
            source,
            targets,
            status,
            ..
        } => {
            remap_expression(source, offset);
            for target in targets {
                target.net += offset;
            }
            status.net += offset;
        }
        Stmt::ImmediateAssertion {
            cond,
            pass_action,
            fail_action,
            ..
        } => {
            remap_expression(cond, offset);
            for action in pass_action.iter_mut().chain(fail_action.iter_mut()) {
                remap_statement(action, offset);
            }
        }
        Stmt::FileDisplay { fd, args, .. } => {
            remap_expression(fd, offset);
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
        Stmt::RandomSeeded { seed, destination } => {
            seed.net += offset;
            destination.net += offset;
        }
        Stmt::TimeFormat { .. }
        | Stmt::AssertControl(_)
        | Stmt::Severity(_)
        | Stmt::Finish
        | Stmt::Stop => {}
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
        Expr::Const(_) | Expr::RealConst(_) | Expr::ShortRealConst(_) | Expr::ProcessSelf => {}
        Expr::Ref { net, .. } => *net = rewrite_net(*net, rewrites),
        Expr::RealRef { net, .. } => *net = rewrite_net(*net, rewrites),
        Expr::ShortRealRef { net } => *net = rewrite_net(*net, rewrites),
        Expr::ProcessStatus(handle) => rewrite_expression_nets(handle, rewrites),
        Expr::Past { value, gating, .. } => {
            rewrite_expression_nets(value, rewrites);
            if let Some(gating) = gating {
                rewrite_expression_nets(gating, rewrites);
            }
        }
        Expr::Sampled(value) => rewrite_expression_nets(value, rewrites),
        Expr::SampledValue { value, .. } => rewrite_expression_nets(value, rewrites),
        Expr::RealBinary { left, right, .. } => {
            rewrite_expression_nets(left, rewrites);
            rewrite_expression_nets(right, rewrites);
        }
        Expr::RealCompare { left, right, .. } => {
            rewrite_expression_nets(left, rewrites);
            rewrite_expression_nets(right, rewrites);
        }
        Expr::RealNeg(operand) => rewrite_expression_nets(operand, rewrites),
        Expr::ShortRealBinary { left, right, .. } => {
            rewrite_expression_nets(left, rewrites);
            rewrite_expression_nets(right, rewrites);
        }
        Expr::ShortRealNeg(operand)
        | Expr::ShortRealToReal(operand)
        | Expr::RealToShortReal(operand)
        | Expr::BitsToReal(operand)
        | Expr::BitsToShortReal(operand) => rewrite_expression_nets(operand, rewrites),
        Expr::RealToInt { value, .. }
        | Expr::RealTruncToInt { value, .. }
        | Expr::IntToReal { value, .. } => rewrite_expression_nets(value, rewrites),
        Expr::RealTime { .. } => {}
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
        Expr::MemorySize { .. } => {}
        Expr::AssocRead { index, .. } | Expr::AssocExists { index, .. } => {
            rewrite_expression_nets(index, rewrites)
        }
        Expr::Index { e, .. } => rewrite_expression_nets(e, rewrites),
        Expr::StringLength { e } => rewrite_expression_nets(e, rewrites),
        Expr::StringConcat { left, right } => {
            rewrite_expression_nets(left, rewrites);
            rewrite_expression_nets(right, rewrites);
        }
        Expr::StringFormat { args, .. } => {
            args.iter_mut()
                .for_each(|argument| rewrite_expression_nets(argument, rewrites));
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
        Expr::MemoryPattern { .. } => {}
        Expr::EnumPattern { value, .. } => rewrite_expression_nets(value, rewrites),
        Expr::StructPattern { value, .. } => rewrite_expression_nets(value, rewrites),
        Expr::EventTriggered { net } => *net = rewrite_net(*net, rewrites),
        Expr::Random { args, .. } => args
            .iter_mut()
            .for_each(|argument| rewrite_expression_nets(argument, rewrites)),
        Expr::CountBits { value, .. }
        | Expr::IsUnknown(value)
        | Expr::OneHot(value)
        | Expr::OneHot0(value) => rewrite_expression_nets(value, rewrites),
        Expr::FileOpen { .. } => {}
        Expr::FileOpenDynamic { path, mode } => {
            rewrite_expression_nets(path, rewrites);
            rewrite_expression_nets(mode, rewrites);
        }
        Expr::FileGetc { fd } => rewrite_expression_nets(fd, rewrites),
        Expr::FileUngetc { value, fd } => {
            rewrite_expression_nets(value, rewrites);
            rewrite_expression_nets(fd, rewrites);
        }
        Expr::FileEof { fd } => rewrite_expression_nets(fd, rewrites),
        Expr::FileTell { fd } => rewrite_expression_nets(fd, rewrites),
        Expr::FileSeek {
            fd,
            offset: seek_offset,
            whence,
        } => {
            rewrite_expression_nets(fd, rewrites);
            rewrite_expression_nets(seek_offset, rewrites);
            rewrite_expression_nets(whence, rewrites);
        }
    }
}

fn rewrite_statement_nets(statement: &mut eda_core::ir::Stmt, rewrites: &[NetRewrite]) {
    use eda_core::ir::Stmt;
    match statement {
        Stmt::Block(statements) => statements
            .iter_mut()
            .for_each(|statement| rewrite_statement_nets(statement, rewrites)),
        Stmt::Forever(body) => rewrite_statement_nets(body, rewrites),
        Stmt::NamedBlock { body, .. } => rewrite_statement_nets(body, rewrites),
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
        Stmt::MemoryCopy { .. } => {}
        Stmt::MemoryNew { size, .. } => rewrite_expression_nets(size, rewrites),
        Stmt::MemoryDelete { .. } => {}
        Stmt::AssocTraversal {
            key, destination, ..
        } => {
            rewrite_lvalue_nets(key, rewrites);
            rewrite_lvalue_nets(destination, rewrites);
        }
        Stmt::QueuePush { value, .. } => rewrite_expression_nets(value, rewrites),
        Stmt::QueuePop { destination, .. } => rewrite_lvalue_nets(destination, rewrites),
        Stmt::QueueInsert { index, value, .. } => {
            rewrite_expression_nets(index, rewrites);
            rewrite_expression_nets(value, rewrites);
        }
        Stmt::QueueDelete { index, .. } => rewrite_expression_nets(index, rewrites),
        Stmt::AssocWrite { index, value, .. } => {
            rewrite_expression_nets(index, rewrites);
            rewrite_expression_nets(value, rewrites);
        }
        Stmt::AssocDelete { index, .. } => {
            if let Some(index) = index {
                rewrite_expression_nets(index, rewrites);
            }
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
            for (expression, statement) in arms {
                rewrite_expression_nets(expression, rewrites);
                rewrite_statement_nets(statement, rewrites);
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
        Stmt::Break | Stmt::Continue | Stmt::ReturnBarrier | Stmt::Return => {}
        Stmt::Scope(body) => rewrite_statement_nets(body, rewrites),
        Stmt::Repeat { count, body } => {
            rewrite_expression_nets(count, rewrites);
            rewrite_statement_nets(body, rewrites);
        }
        Stmt::Force { lv, e } => {
            rewrite_lvalue_nets(lv, rewrites);
            rewrite_expression_nets(e, rewrites);
        }
        Stmt::Release { lv } => rewrite_lvalue_nets(lv, rewrites),
        Stmt::ProceduralAssign { lv, e } => {
            rewrite_lvalue_nets(lv, rewrites);
            rewrite_expression_nets(e, rewrites);
        }
        Stmt::ProceduralDeassign { lv } => rewrite_lvalue_nets(lv, rewrites),
        Stmt::DisableFork | Stmt::WaitFork | Stmt::NamedBlockEnd => {}
        Stmt::DisableNamedBlock { flag, .. } => *flag = rewrite_net(*flag, rewrites),
        Stmt::ProcessKill(handle)
        | Stmt::ProcessAwait(handle)
        | Stmt::ProcessSuspend(handle)
        | Stmt::ProcessResume(handle) => rewrite_expression_nets(handle, rewrites),
        Stmt::Display { args, .. } => args
            .iter_mut()
            .for_each(|argument| rewrite_expression_nets(argument, rewrites)),
        Stmt::FileClose { fd } => rewrite_expression_nets(fd, rewrites),
        Stmt::FileFlush { fd } => rewrite_expression_nets(fd, rewrites),
        Stmt::FileGets { fd, string, status } => {
            rewrite_expression_nets(fd, rewrites);
            rewrite_lvalue_nets(string, rewrites);
            rewrite_lvalue_nets(status, rewrites);
        }
        Stmt::FileError {
            fd,
            message,
            status,
        } => {
            rewrite_expression_nets(fd, rewrites);
            rewrite_lvalue_nets(message, rewrites);
            rewrite_lvalue_nets(status, rewrites);
        }
        Stmt::FileRead { fd, target, status } => {
            rewrite_expression_nets(fd, rewrites);
            rewrite_lvalue_nets(target, rewrites);
            rewrite_lvalue_nets(status, rewrites);
        }
        Stmt::FileReadMemory {
            fd,
            start,
            count,
            status,
            ..
        } => {
            rewrite_expression_nets(fd, rewrites);
            if let Some(start) = start {
                rewrite_expression_nets(start, rewrites);
            }
            if let Some(count) = count {
                rewrite_expression_nets(count, rewrites);
            }
            rewrite_lvalue_nets(status, rewrites);
        }
        Stmt::FileScan {
            fd,
            targets,
            status,
            ..
        } => {
            rewrite_expression_nets(fd, rewrites);
            for target in targets {
                rewrite_lvalue_nets(target, rewrites);
            }
            rewrite_lvalue_nets(status, rewrites);
        }
        Stmt::StringScan {
            source,
            targets,
            status,
            ..
        } => {
            rewrite_expression_nets(source, rewrites);
            for target in targets {
                rewrite_lvalue_nets(target, rewrites);
            }
            rewrite_lvalue_nets(status, rewrites);
        }
        Stmt::ImmediateAssertion {
            cond,
            pass_action,
            fail_action,
            ..
        } => {
            rewrite_expression_nets(cond, rewrites);
            for action in pass_action.iter_mut().chain(fail_action.iter_mut()) {
                rewrite_statement_nets(action, rewrites);
            }
        }
        Stmt::FileDisplay { fd, args, .. } => {
            rewrite_expression_nets(fd, rewrites);
            args.iter_mut()
                .for_each(|argument| rewrite_expression_nets(argument, rewrites));
        }
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
        Stmt::RandomSeeded { seed, destination } => {
            rewrite_lvalue_nets(seed, rewrites);
            rewrite_lvalue_nets(destination, rewrites);
        }
        Stmt::TimeFormat { .. }
        | Stmt::AssertControl(_)
        | Stmt::Severity(_)
        | Stmt::Finish
        | Stmt::Stop => {}
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
        Expr::Const(_)
        | Expr::RealConst(_)
        | Expr::ShortRealConst(_)
        | Expr::Ref { .. }
        | Expr::RealRef { .. }
        | Expr::ShortRealRef { .. }
        | Expr::ProcessSelf => {}
        Expr::ProcessStatus(handle) => remap_expression_memory_ids(handle, offset),
        Expr::Past { value, gating, .. } => {
            remap_expression_memory_ids(value, offset);
            if let Some(gating) = gating {
                remap_expression_memory_ids(gating, offset);
            }
        }
        Expr::Sampled(value) => remap_expression_memory_ids(value, offset),
        Expr::SampledValue { value, .. } => remap_expression_memory_ids(value, offset),
        Expr::RealBinary { left, right, .. } => {
            remap_expression_memory_ids(left, offset);
            remap_expression_memory_ids(right, offset);
        }
        Expr::RealCompare { left, right, .. } => {
            remap_expression_memory_ids(left, offset);
            remap_expression_memory_ids(right, offset);
        }
        Expr::RealNeg(operand) => remap_expression_memory_ids(operand, offset),
        Expr::ShortRealBinary { left, right, .. } => {
            remap_expression_memory_ids(left, offset);
            remap_expression_memory_ids(right, offset);
        }
        Expr::ShortRealNeg(operand)
        | Expr::ShortRealToReal(operand)
        | Expr::RealToShortReal(operand)
        | Expr::BitsToReal(operand)
        | Expr::BitsToShortReal(operand) => remap_expression_memory_ids(operand, offset),
        Expr::RealToInt { value, .. }
        | Expr::RealTruncToInt { value, .. }
        | Expr::IntToReal { value, .. } => remap_expression_memory_ids(value, offset),
        Expr::RealTime { .. } => {}
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
        Expr::MemorySize { memory, .. } => *memory += offset,
        Expr::AssocRead { memory, index, .. } | Expr::AssocExists { memory, index } => {
            *memory += offset;
            remap_expression_memory_ids(index, offset);
        }
        Expr::Index { e, .. } => remap_expression_memory_ids(e, offset),
        Expr::StringLength { e } => remap_expression_memory_ids(e, offset),
        Expr::StringConcat { left, right } => {
            remap_expression_memory_ids(left, offset);
            remap_expression_memory_ids(right, offset);
        }
        Expr::StringFormat { args, .. } => {
            args.iter_mut()
                .for_each(|argument| remap_expression_memory_ids(argument, offset));
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
        Expr::MemoryPattern { memory, .. } => *memory += offset,
        Expr::EnumPattern { value, .. } => remap_expression_memory_ids(value, offset),
        Expr::StructPattern { value, .. } => remap_expression_memory_ids(value, offset),
        Expr::EventTriggered { .. } | Expr::SimulationTime { .. } => {}
        Expr::Random { args, .. } => args
            .iter_mut()
            .for_each(|argument| remap_expression_memory_ids(argument, offset)),
        Expr::CountBits { value, .. }
        | Expr::IsUnknown(value)
        | Expr::OneHot(value)
        | Expr::OneHot0(value) => remap_expression_memory_ids(value, offset),
        Expr::FileOpen { .. } => {}
        Expr::FileOpenDynamic { path, mode } => {
            remap_expression_memory_ids(path, offset);
            remap_expression_memory_ids(mode, offset);
        }
        Expr::FileGetc { fd } => remap_expression_memory_ids(fd, offset),
        Expr::FileUngetc { value, fd } => {
            remap_expression_memory_ids(value, offset);
            remap_expression_memory_ids(fd, offset);
        }
        Expr::FileEof { fd } => remap_expression_memory_ids(fd, offset),
        Expr::FileTell { fd } => remap_expression_memory_ids(fd, offset),
        Expr::FileSeek {
            fd,
            offset: seek_offset,
            whence,
        } => {
            remap_expression_memory_ids(fd, offset);
            remap_expression_memory_ids(seek_offset, offset);
            remap_expression_memory_ids(whence, offset);
        }
    }
}

pub(super) fn remap_statement_memory_ids(statement: &mut eda_core::ir::Stmt, offset: usize) {
    use eda_core::ir::Stmt;
    match statement {
        Stmt::Block(statements) => statements
            .iter_mut()
            .for_each(|statement| remap_statement_memory_ids(statement, offset)),
        Stmt::Forever(body) => remap_statement_memory_ids(body, offset),
        Stmt::NamedBlock { body, .. } => remap_statement_memory_ids(body, offset),
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
        Stmt::MemoryCopy {
            source,
            destination,
            ..
        } => {
            *source += offset;
            *destination += offset;
        }
        Stmt::MemoryNew { memory, size, .. } => {
            *memory += offset;
            remap_expression_memory_ids(size, offset);
        }
        Stmt::MemoryDelete { memory } => *memory += offset,
        Stmt::AssocTraversal { memory, .. } => *memory += offset,
        Stmt::QueuePush { memory, value, .. } => {
            *memory += offset;
            remap_expression_memory_ids(value, offset);
        }
        Stmt::QueuePop { memory, .. } => *memory += offset,
        Stmt::QueueInsert {
            memory,
            index,
            value,
        } => {
            *memory += offset;
            remap_expression_memory_ids(index, offset);
            remap_expression_memory_ids(value, offset);
        }
        Stmt::QueueDelete { memory, index } => {
            *memory += offset;
            remap_expression_memory_ids(index, offset);
        }
        Stmt::AssocWrite {
            memory,
            index,
            value,
            ..
        } => {
            *memory += offset;
            remap_expression_memory_ids(index, offset);
            remap_expression_memory_ids(value, offset);
        }
        Stmt::AssocDelete { memory, index } => {
            *memory += offset;
            if let Some(index) = index {
                remap_expression_memory_ids(index, offset);
            }
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
            for (expression, statement) in arms {
                remap_expression_memory_ids(expression, offset);
                remap_statement_memory_ids(statement, offset);
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
        Stmt::Break | Stmt::Continue | Stmt::ReturnBarrier | Stmt::Return => {}
        Stmt::Scope(body) => remap_statement_memory_ids(body, offset),
        Stmt::Repeat { count, body } => {
            remap_expression_memory_ids(count, offset);
            remap_statement_memory_ids(body, offset);
        }
        Stmt::Force { e, .. } => remap_expression_memory_ids(e, offset),
        Stmt::Release { .. } => {}
        Stmt::ProceduralAssign { e, .. } => remap_expression_memory_ids(e, offset),
        Stmt::ProceduralDeassign { .. } => {}
        Stmt::DisableFork | Stmt::WaitFork | Stmt::NamedBlockEnd => {}
        Stmt::DisableNamedBlock { .. } => {}
        Stmt::ProcessKill(handle)
        | Stmt::ProcessAwait(handle)
        | Stmt::ProcessSuspend(handle)
        | Stmt::ProcessResume(handle) => remap_expression_memory_ids(handle, offset),
        Stmt::Display { args, .. } => {
            args.iter_mut()
                .for_each(|argument| remap_expression_memory_ids(argument, offset));
        }
        Stmt::FileClose { fd } => remap_expression_memory_ids(fd, offset),
        Stmt::FileFlush { fd } => remap_expression_memory_ids(fd, offset),
        Stmt::FileGets { fd, .. } | Stmt::FileError { fd, .. } | Stmt::FileRead { fd, .. } => {
            remap_expression_memory_ids(fd, offset)
        }
        Stmt::FileReadMemory {
            fd,
            memory,
            start,
            count,
            ..
        } => {
            remap_expression_memory_ids(fd, offset);
            *memory += offset;
            if let Some(start) = start {
                remap_expression_memory_ids(start, offset);
            }
            if let Some(count) = count {
                remap_expression_memory_ids(count, offset);
            }
        }
        Stmt::FileScan { fd, .. } => remap_expression_memory_ids(fd, offset),
        Stmt::StringScan { source, .. } => remap_expression_memory_ids(source, offset),
        Stmt::ImmediateAssertion {
            cond,
            pass_action,
            fail_action,
            ..
        } => {
            remap_expression_memory_ids(cond, offset);
            for action in pass_action.iter_mut().chain(fail_action.iter_mut()) {
                remap_statement_memory_ids(action, offset);
            }
        }
        Stmt::FileDisplay { fd, args, .. } => {
            remap_expression_memory_ids(fd, offset);
            args.iter_mut().for_each(|argument| {
                remap_expression_memory_ids(argument, offset);
            });
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
        Stmt::RandomSeeded { .. } => {}
        Stmt::TimeFormat { .. }
        | Stmt::AssertControl(_)
        | Stmt::Severity(_)
        | Stmt::Finish
        | Stmt::Stop => {}
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
        let parent_scope_path = self.current_format_hierarchy_path();
        let child_scope_path = if parent_scope_path.is_empty() {
            instance_name.clone()
        } else {
            format!("{parent_scope_path}.{instance_name}")
        };
        let body = member
            .get("body")
            .ok_or_else(|| "slang AST Instance has no body".to_string())?;
        let definition_name = string_field(body, "name").unwrap_or_default();
        let format_library_scope = self
            .library_scopes
            .get(&definition_name)
            .cloned()
            .unwrap_or_else(|| definition_name.clone());
        let child_offset = self.design.n_nets;
        let memory_offset = self.design.memories.len();
        let mut interface_proxy_connections = Vec::new();
        let mut interface_memory_connections = Vec::new();
        let mut child = Self {
            design: Design::default(),
            unit_ir_cache: self.unit_ir_cache.clone(),
            symbols: std::collections::BTreeMap::new(),
            local_symbols: std::collections::BTreeMap::new(),
            external_symbols: std::collections::BTreeMap::new(),
            external_symbol_directions: std::collections::BTreeMap::new(),
            local_initializers: std::collections::BTreeMap::new(),
            local_blocks: std::collections::BTreeMap::new(),
            local_static_symbols: std::collections::BTreeSet::new(),
            unbounded_parameters: std::collections::BTreeSet::new(),
            constants: std::collections::BTreeMap::new(),
            subroutines: std::collections::BTreeMap::new(),
            type_aliases: self.package_type_aliases.clone(),
            type_alias_type_names: self.package_type_alias_type_names.clone(),
            package_type_aliases: self.package_type_aliases.clone(),
            package_type_alias_type_names: self.package_type_alias_type_names.clone(),
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
            uvm_report_controls: std::cell::RefCell::new(super::AstUvmReportControls::default()),
            uvm_report_catcher: std::cell::RefCell::new(Vec::new()),
            uvm_callbacks: std::cell::RefCell::new(Vec::new()),
            uvm_analysis_connections: std::cell::RefCell::new(Vec::new()),
            uvm_tlm_fifos: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            uvm_tlm_endpoint_connections: std::cell::RefCell::new(Vec::new()),
            uvm_sequencers: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            uvm_sequence_contexts: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            uvm_sequence_items: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            uvm_sequence_response_types: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            uvm_sequence_transaction_ids: std::cell::RefCell::new(std::collections::BTreeMap::new()),
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
            plusargs: std::collections::BTreeMap::new(),
            plusarg_values: std::collections::BTreeMap::new(),
            time_scale: super::body_time_scale(body),
            global_time_precision: self.global_time_precision,
            active_named_blocks: std::cell::RefCell::new(Vec::new()),
            active_assertion_scopes: std::cell::RefCell::new(Vec::new()),
            known_assertion_scopes: std::collections::BTreeSet::new(),
            manual_covergroups: std::collections::BTreeMap::new(),
            covergroup_instances: std::collections::BTreeMap::new(),
            constructed_covergroups: std::cell::RefCell::new(std::collections::BTreeSet::new()),
            generate_path: Vec::new(),
            scope_path: child_scope_path,
            library_scopes: self.library_scopes.clone(),
            format_library_scope,
            inline_stack: std::cell::RefCell::new(Vec::new()),
            past_sampling_allowed: false,
            past_sampling_event: None,
            recursive_property_depth: self.recursive_property_depth,
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
        child.design.name = definition_name.clone();
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
        let cache_key = body
            .get("eda_unit_ir_cache_key")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let has_interface_connection = lowered_connections
            .iter()
            .any(|connection| connection.get("ifaceInstance").is_some());
        let uses_hierarchy_format = value_contains_hierarchy_format(body);
        let reusable_unit = cache_key.is_some()
            && !is_interface
            && !has_interface_connection
            && self.global_time_precision == -9
            && !uses_hierarchy_format;
        let mut cached_unit = false;
        if let (Some(cache_key), Some(cache)) = (cache_key.clone(), self.unit_ir_cache.clone()) {
            if reusable_unit {
                if let Ok(mut unit_cache) = cache.lock() {
                    if let Some(design) = unit_cache.get(&cache_key) {
                        child.design = (*design).clone();
                        cached_unit = true;
                    }
                }
            }
        }
        if !cached_unit {
            child.prepare_plusargs(body)?;
            child.lower_body(body)?;
            if let (Some(cache_key), Some(cache)) = (cache_key, self.unit_ir_cache.clone()) {
                // Class-static initializer counts live in the lowerer rather
                // than Design. Do not cache a fragment until those statements
                // have a design-native ownership marker.
                if reusable_unit && child.class_static_initializer_count == 0 {
                    if let Ok(mut unit_cache) = cache.lock() {
                        unit_cache.insert(cache_key, std::sync::Arc::new(child.design.clone()));
                    }
                }
            }
        }

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
        let child_port_signedness = child_ports
            .iter()
            .map(|port| {
                let signed = child
                    .design
                    .net_decls
                    .iter()
                    .find(|declaration| {
                        declaration.base == port.base && declaration.width == port.width
                    })
                    .is_some_and(|declaration| declaration.is_signed);
                (port.base, signed)
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let child_memories = child.design.memories.clone();
        let mut static_net_rewrites = Vec::new();
        let mut plusarg_net_rewrites = Vec::new();
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
        for mut connection in child.design.array_connections {
            connection.child += memory_offset;
            connection.parent += memory_offset;
            self.design.array_connections.push(connection);
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
        for (name, net) in &child.design.plusargs {
            let remapped = net + offset;
            if let Some(existing) = self.design.plusargs.get(name) {
                if *existing != remapped {
                    plusarg_net_rewrites.push(NetRewrite {
                        from: remapped,
                        to: *existing,
                        width: 1,
                    });
                }
            } else {
                self.design.plusargs.insert(name.clone(), remapped);
            }
        }
        for (name, net) in &child.design.plusarg_values {
            let remapped = net + offset;
            if let Some(existing) = self.design.plusarg_values.get(name) {
                if *existing != remapped {
                    plusarg_net_rewrites.push(NetRewrite {
                        from: remapped,
                        to: *existing,
                        width: 512,
                    });
                }
            } else {
                self.design.plusarg_values.insert(name.clone(), remapped);
            }
        }
        static_net_rewrites.extend(plusarg_net_rewrites);
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
        let empty_argument = serde_json::json!({ "kind": "EmptyArgument" });
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
            let expression = connection.get("expr").unwrap_or(&empty_argument);
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
                    let child_lvalue = eda_core::ir::LValue {
                        net: port.base + offset,
                        decl_width: port.width,
                        range: full_range(port.width),
                    };
                    self.record_implicit_type_mismatch(&child_lvalue, expression)?;
                    let rhs = self.lower_expression(expression)?;
                    if let Some(parent_bits) = expression_parent_bits(&rhs) {
                        connections.insert(port_name.to_string(), parent_bits);
                    }
                    self.design.assigns.push(eda_core::ir::Assign {
                        lv: child_lvalue,
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
                    if let Some(child_signed) = child_port_signedness.get(&port.base) {
                        let parent_bit = match target.range {
                            eda_core::ir::Range::Bit(bit) => target.net + bit,
                            eda_core::ir::Range::Span { msb, lsb } => target.net + lsb.min(msb),
                        };
                        let parent_signed = self
                            .design
                            .net_decls
                            .iter()
                            .find(|declaration| {
                                declaration.base == parent_bit
                                    || (parent_bit > declaration.base
                                        && parent_bit < declaration.base + declaration.width)
                            })
                            .is_some_and(|declaration| declaration.is_signed);
                        if parent_signed != *child_signed {
                            let bit_name = self.design.net_name(parent_bit);
                            let net = bit_name.split('[').next().unwrap_or(&bit_name).to_string();
                            self.record_type_mismatch(eda_core::ir::LintIssue {
                                kind: eda_core::ir::LintIssueKind::SignednessMismatch,
                                net,
                                count: 1,
                            });
                        }
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
