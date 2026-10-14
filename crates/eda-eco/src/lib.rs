//! # eda-eco — functional engineering-change-order patch engine.
//!
//! ECO operations are intentionally small and structural: replace a mapped or
//! generic combinational cell, reconnect one input, insert a buffer before a
//! pin, or tie a pin to a constant. Every patch is applied transactionally in
//! order and the resulting netlist is checked for topology and output-driver
//! validity.

use eda_core::logic::LogicValue;
use eda_core::netlist::{Cell, CellKind, Netlist};
use eda_pnr::Placement;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EcoCellTarget {
    Buffer,
    Inverter,
    And,
    Nand,
    Or,
    Nor,
    Xor,
    Xnor,
    Mux,
    Library(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EcoOperation {
    ReplaceCell {
        cell: String,
        target: EcoCellTarget,
    },
    ConnectInput {
        cell: String,
        pin: usize,
        net: String,
    },
    InsertBuffer {
        cell: String,
        pin: usize,
        new_cell: String,
        new_net: String,
    },
    TieInput {
        cell: String,
        pin: usize,
        value: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EcoChange {
    pub operation: usize,
    pub description: String,
}

#[derive(Clone, Debug)]
pub struct EcoResult {
    pub patch_name: String,
    /// Optional stable hierarchy-instance scope applied to every operation.
    pub hierarchy_scope: Option<String>,
    pub netlist: Netlist,
    /// Legal placement when an ECO transformation is invoked with a physical
    /// input placement. Logical-only ECO leaves this field unset.
    pub physical_placement: Option<Placement>,
    pub changes: Vec<EcoChange>,
    pub operations_applied: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EcoPatch {
    pub name: String,
    /// Optional wildcard hierarchy scope for all cell selectors.
    pub scope: Option<String>,
    pub operations: Vec<EcoOperation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FanoutRepairOptions {
    pub max_fanout: usize,
    pub patch_name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SequentialPinRoles {
    input_count: usize,
    clock_pin: usize,
    reset_pin: Option<usize>,
}

fn sequential_pin_roles(nl: &Netlist, kind: &CellKind) -> Option<SequentialPinRoles> {
    match kind {
        CellKind::Dff { .. } => Some(SequentialPinRoles {
            input_count: 2,
            clock_pin: 0,
            reset_pin: None,
        }),
        CellKind::DffR { .. } => Some(SequentialPinRoles {
            input_count: 3,
            clock_pin: 0,
            reset_pin: Some(2),
        }),
        CellKind::Lib(id) => {
            let library = nl.library()?;
            let definition = library.get(*id);
            let sequential = definition.seq.as_ref()?;
            Some(SequentialPinRoles {
                input_count: definition.inputs.len(),
                clock_pin: sequential.clk_pin,
                reset_pin: sequential.reset_pin,
            })
        }
        _ => None,
    }
}

impl Default for FanoutRepairOptions {
    fn default() -> Self {
        Self {
            max_fanout: 8,
            patch_name: "fanout_eco".into(),
        }
    }
}

/// Automatically split high-fanout nets with functionally transparent buffers.
///
/// Each original sink is assigned to one buffer output. The original net then
/// drives only the newly inserted buffer inputs, while each buffer drives at
/// most `max_fanout` sinks. This preserves every primary-output function while
/// reducing fanout and, with Liberty input-pin capacitance, usually reducing
/// the driver's lumped load.
pub fn repair_fanout(nl: &Netlist, options: &FanoutRepairOptions) -> Result<EcoResult, String> {
    repair_fanout_impl(nl, None, options, None)
}

/// Repair high fanout and legalize the locations of all inserted buffers.
pub fn repair_fanout_with_placement(
    nl: &Netlist,
    placement: &Placement,
    options: &FanoutRepairOptions,
) -> Result<EcoResult, String> {
    repair_fanout_impl(nl, Some(placement), options, None)
}

/// Repair high-fanout nets driven inside one hierarchy namespace.
pub fn repair_fanout_in_hierarchy(
    nl: &Netlist,
    options: &FanoutRepairOptions,
    hierarchy_pattern: &str,
) -> Result<EcoResult, String> {
    repair_fanout_impl(nl, None, options, Some(hierarchy_pattern))
}

/// Repair and physically legalize high-fanout nets in one hierarchy namespace.
pub fn repair_fanout_with_placement_in_hierarchy(
    nl: &Netlist,
    placement: &Placement,
    options: &FanoutRepairOptions,
    hierarchy_pattern: &str,
) -> Result<EcoResult, String> {
    repair_fanout_impl(nl, Some(placement), options, Some(hierarchy_pattern))
}

fn repair_fanout_impl(
    nl: &Netlist,
    placement: Option<&Placement>,
    options: &FanoutRepairOptions,
    hierarchy_scope: Option<&str>,
) -> Result<EcoResult, String> {
    if options.max_fanout == 0 {
        return Err("fanout repair requires a positive max fanout".into());
    }
    if options.patch_name.is_empty() {
        return Err("fanout repair patch name cannot be empty".into());
    }
    if hierarchy_scope.is_some_and(str::is_empty) {
        return Err("fanout repair hierarchy scope cannot be empty".into());
    }

    let mut out = nl.clone();
    let mut changes = Vec::new();
    let mut operation = 0usize;
    for net_id in 0..nl.net_count() {
        let sink_ids = {
            let mut unique = out.net(net_id).sinks.clone();
            unique.sort_unstable();
            unique.dedup();
            unique
        };
        if sink_ids.len() <= options.max_fanout {
            continue;
        }
        let drivers = out.net(net_id).drivers.clone();
        let source_hierarchy = if let Some(scope) = hierarchy_scope {
            let matching: Vec<_> = drivers
                .iter()
                .copied()
                .filter(|&cell| {
                    out.cell_hierarchy_instance(cell)
                        .is_some_and(|instance| hierarchy_pattern_matches(scope, &instance.path))
                })
                .collect();
            if matching.is_empty() {
                continue;
            }
            if matching.len() > 1 {
                return Err(format!(
                    "fanout repair net '{}' has multiple drivers in hierarchy scope '{}'",
                    nl.net(net_id).name,
                    scope
                ));
            }
            cell_hierarchy_path(&out, matching[0])
        } else {
            None
        };

        operation += 1;
        let source_name = nl.net(net_id).name.clone();
        let safe_source = source_name.replace_nonalnum();
        let cells_before = out.cell_count();
        let mut buffer_index = 0usize;
        insert_fanout_tree(
            &mut out,
            net_id,
            net_id,
            &sink_ids,
            options.max_fanout,
            &safe_source,
            &mut buffer_index,
            source_hierarchy.as_deref(),
        )?;
        let buffers_inserted = out.cell_count() - cells_before;

        let new_fanout = out.net(net_id).sinks.len();
        if new_fanout > options.max_fanout {
            return Err(format!(
                "fanout repair left net '{}' with fanout {} > {}",
                source_name, new_fanout, options.max_fanout
            ));
        }
        let description = format!(
            "split net '{}' with {} buffer(s); original fanout now {}",
            source_name, buffers_inserted, new_fanout
        );
        changes.push(EcoChange {
            operation,
            description: with_hierarchy(description, source_hierarchy.as_deref()),
        });
    }

    validate(&out).map_err(|error| format!("fanout repair validation failed: {}", error))?;
    let physical_placement = placement
        .map(|original| eda_pnr::extend_placement(&out, original))
        .transpose()?;
    let operations_applied = changes.len();
    Ok(EcoResult {
        patch_name: options.patch_name.clone(),
        hierarchy_scope: hierarchy_scope.map(str::to_string),
        netlist: out,
        physical_placement,
        changes,
        operations_applied,
    })
}

#[allow(clippy::too_many_arguments)]
fn insert_fanout_tree(
    out: &mut Netlist,
    input: usize,
    original_input: usize,
    sinks: &[usize],
    max_fanout: usize,
    source_name: &str,
    buffer_index: &mut usize,
    hierarchy: Option<&str>,
) -> Result<(), String> {
    let buffer_net_name = unique_name(out, &format!("{}_eco_buf{}_net", source_name, buffer_index));
    let buffer_cell_name = unique_name(out, &format!("{}_eco_buf{}", source_name, buffer_index));
    *buffer_index += 1;
    let buffer_net = out.add_net(buffer_net_name);
    attach_net_to_hierarchy(out, hierarchy, buffer_net)?;
    out.add_cell(Cell {
        name: buffer_cell_name,
        kind: buffer_kind(out)?,
        inputs: vec![input],
        output: buffer_net,
    });

    if sinks.len() <= max_fanout {
        for &sink in sinks {
            let mut patched = out.cell(sink).clone();
            let mut reconnects = 0usize;
            for pin in &mut patched.inputs {
                if *pin == original_input {
                    *pin = buffer_net;
                    reconnects += 1;
                }
            }
            if reconnects == 0 {
                return Err(format!(
                    "fanout repair lost sink '{}' on net '{}'",
                    out.cell(sink).name,
                    out.net(input).name
                ));
            }
            out.replace_cell(sink, patched);
        }
        return Ok(());
    }

    // Keep the number of immediate children bounded while distributing the
    // remaining sinks as evenly as possible.
    let group_size = sinks.len().div_ceil(max_fanout);
    for group in sinks.chunks(group_size) {
        insert_fanout_tree(
            out,
            buffer_net,
            original_input,
            group,
            max_fanout,
            source_name,
            buffer_index,
            hierarchy,
        )?;
    }
    Ok(())
}

/// Parse a compact semicolon-delimited ECO language.
///
/// Supported commands:
/// - `replace_cell <instance> <LIB|gate>;`
/// - `connect_input <instance> <pin> <net>;`
/// - `insert_buffer <instance> <pin> <new-instance> <new-net>;`
/// - `tie_input <instance> <pin> <0|1>;`
/// - `scope <hierarchy-pattern>;` after the optional `eco` header
pub fn parse_eco(source: &str) -> Result<EcoPatch, String> {
    let tokens = lex(source)?;
    let mut patch = EcoPatch::default();
    let mut pos = 0;
    if let Some(Token::Word(word, _)) = tokens.first() {
        if word == "eco" {
            patch.name = identifier(&tokens, 1, tokens[0].line())?;
            expect_punct(&tokens, 2, ";", tokens[0].line())?;
            pos = 3;
        }
    }
    if let Some(Token::Word(word, _)) = tokens.get(pos) {
        if word == "scope" {
            let line = tokens[pos].line();
            patch.scope = Some(identifier(&tokens, pos + 1, line)?);
            expect_punct(&tokens, pos + 2, ";", line)?;
            pos += 3;
        }
    }
    if let Some(Token::Word(word, line)) = tokens.get(pos) {
        if word == "scope" {
            return Err(format!("line {}: duplicate or misplaced ECO scope", line));
        }
    }
    while pos < tokens.len() {
        let line = tokens[pos].line();
        let command = identifier(&tokens, pos, line)?;
        let operation = match command.as_str() {
            "replace_cell" => {
                let cell = identifier(&tokens, pos + 1, line)?;
                let target_name = identifier(&tokens, pos + 2, line)?;
                EcoOperation::ReplaceCell {
                    cell,
                    target: parse_target(&target_name)?,
                }
            }
            "connect_input" => {
                let cell = identifier(&tokens, pos + 1, line)?;
                let pin = number(&tokens, pos + 2, line)?;
                let net = identifier(&tokens, pos + 3, line)?;
                EcoOperation::ConnectInput { cell, pin, net }
            }
            "insert_buffer" => {
                let cell = identifier(&tokens, pos + 1, line)?;
                let pin = number(&tokens, pos + 2, line)?;
                let new_cell = identifier(&tokens, pos + 3, line)?;
                let new_net = identifier(&tokens, pos + 4, line)?;
                EcoOperation::InsertBuffer {
                    cell,
                    pin,
                    new_cell,
                    new_net,
                }
            }
            "tie_input" => {
                let cell = identifier(&tokens, pos + 1, line)?;
                let pin = number(&tokens, pos + 2, line)?;
                let raw_value = number(&tokens, pos + 3, line)?;
                if raw_value > 1 {
                    return Err(format!("line {}: tie_input value must be 0 or 1", line));
                }
                let value = raw_value == 1;
                EcoOperation::TieInput { cell, pin, value }
            }
            other => return Err(format!("line {}: unknown ECO command '{}'", line, other)),
        };
        let end = match &operation {
            EcoOperation::ReplaceCell { .. } => pos + 3,
            EcoOperation::ConnectInput { .. } | EcoOperation::TieInput { .. } => pos + 4,
            EcoOperation::InsertBuffer { .. } => pos + 5,
        };
        expect_punct(&tokens, end, ";", line)?;
        patch.operations.push(operation);
        pos = end + 1;
    }
    if patch.operations.is_empty() {
        return Err("ECO patch contains no operations".into());
    }
    Ok(patch)
}

fn parse_target(name: &str) -> Result<EcoCellTarget, String> {
    match name.to_ascii_uppercase().as_str() {
        "BUF" | "BUFFER" => Ok(EcoCellTarget::Buffer),
        "INV" | "INVERTER" => Ok(EcoCellTarget::Inverter),
        "AND" => Ok(EcoCellTarget::And),
        "NAND" => Ok(EcoCellTarget::Nand),
        "OR" => Ok(EcoCellTarget::Or),
        "NOR" => Ok(EcoCellTarget::Nor),
        "XOR" => Ok(EcoCellTarget::Xor),
        "XNOR" => Ok(EcoCellTarget::Xnor),
        "MUX" => Ok(EcoCellTarget::Mux),
        other => Ok(EcoCellTarget::Library(other.to_string())),
    }
}

/// Apply a patch in declaration order. If any operation fails, the original
/// netlist is returned in the error path and no partial ECO is exposed.
pub fn apply_eco(nl: &Netlist, patch: &EcoPatch) -> Result<EcoResult, String> {
    if let Some(scope) = &patch.scope {
        if scope.trim().is_empty() {
            return Err("ECO hierarchy scope cannot be empty".into());
        }
    }
    let mut out = nl.clone();
    let mut changes = Vec::new();
    for (index, operation) in patch.operations.iter().enumerate() {
        match operation {
            EcoOperation::ReplaceCell { cell, target } => {
                let ci = find_operation_cell(&out, cell, patch.scope.as_deref(), index)?;
                let hierarchy = cell_hierarchy_path(&out, ci);
                let old = out.cell(ci);
                let kind = target_kind(&out, target, old.inputs.len())?;
                let old_roles = sequential_pin_roles(&out, &old.kind);
                let new_roles = sequential_pin_roles(&out, &kind);
                match (old_roles, new_roles) {
                    (Some(_), None) | (None, Some(_)) => {
                        return Err(format!(
                            "operation {}: sequential/combinational replacement changes cell class",
                            index
                        ));
                    }
                    (Some(old_roles), Some(new_roles)) => {
                        if old_roles.input_count != old.inputs.len()
                            || old_roles != new_roles
                            || new_roles.input_count != old.inputs.len()
                        {
                            return Err(format!(
                                "operation {}: sequential replacement requires identical clock/reset pin roles",
                                index
                            ));
                        }
                    }
                    (None, None) => {}
                }
                let new = Cell {
                    name: old.name.clone(),
                    kind,
                    inputs: old.inputs.clone(),
                    output: old.output,
                };
                out.replace_cell(ci, new);
                changes.push(EcoChange {
                    operation: index,
                    description: with_hierarchy(
                        format!("replaced '{}' with {:?}", cell, target),
                        hierarchy.as_deref(),
                    ),
                });
            }
            EcoOperation::ConnectInput { cell, pin, net } => {
                let ci = find_operation_cell(&out, cell, patch.scope.as_deref(), index)?;
                let hierarchy = cell_hierarchy_path(&out, ci);
                let net_id = find_net(&out, net)?;
                let mut new = out.cell(ci).clone();
                *new.inputs.get_mut(*pin).ok_or_else(|| {
                    format!("operation {}: cell '{}' has no pin {}", index, cell, pin)
                })? = net_id;
                out.replace_cell(ci, new);
                changes.push(EcoChange {
                    operation: index,
                    description: with_hierarchy(
                        format!("connected '{}':{} to '{}'", cell, pin, net),
                        hierarchy.as_deref(),
                    ),
                });
            }
            EcoOperation::InsertBuffer {
                cell,
                pin,
                new_cell,
                new_net,
            } => {
                let ci = find_operation_cell(&out, cell, patch.scope.as_deref(), index)?;
                let hierarchy = cell_hierarchy_path(&out, ci);
                let source = *out.cell(ci).inputs.get(*pin).ok_or_else(|| {
                    format!("operation {}: cell '{}' has no pin {}", index, cell, pin)
                })?;
                if out.net_by_name(new_net).is_some() || find_cell(&out, new_cell).is_ok() {
                    return Err(format!(
                        "operation {}: ECO name '{}' is already used",
                        index, new_cell
                    ));
                }
                let buffer_out = out.add_net(new_net.clone());
                attach_net_to_hierarchy(&mut out, hierarchy.as_deref(), buffer_out)?;
                let kind = target_kind(&out, &EcoCellTarget::Buffer, 1)?;
                out.add_cell(Cell {
                    name: new_cell.clone(),
                    kind,
                    inputs: vec![source],
                    output: buffer_out,
                });
                let mut patched = out.cell(ci).clone();
                patched.inputs[*pin] = buffer_out;
                out.replace_cell(ci, patched);
                changes.push(EcoChange {
                    operation: index,
                    description: with_hierarchy(
                        format!(
                            "inserted buffer '{}' before '{}':{} on net '{}'",
                            new_cell, cell, pin, new_net
                        ),
                        hierarchy.as_deref(),
                    ),
                });
            }
            EcoOperation::TieInput { cell, pin, value } => {
                let ci = find_operation_cell(&out, cell, patch.scope.as_deref(), index)?;
                let hierarchy = cell_hierarchy_path(&out, ci);
                let tie_name = format!("eco_tie_{}_{}", cell.replace_nonalnum(), pin);
                let tie_net_name = format!("{}_net", tie_name);
                if out.net_by_name(&tie_net_name).is_some() {
                    return Err(format!(
                        "operation {}: tie net '{}' already exists",
                        index, tie_net_name
                    ));
                }
                let tie_net = out.add_net(tie_net_name);
                attach_net_to_hierarchy(&mut out, hierarchy.as_deref(), tie_net)?;
                out.add_cell(Cell {
                    name: tie_name,
                    kind: CellKind::Const(logic_value(*value)),
                    inputs: Vec::new(),
                    output: tie_net,
                });
                let mut patched = out.cell(ci).clone();
                *patched.inputs.get_mut(*pin).ok_or_else(|| {
                    format!("operation {}: cell '{}' has no pin {}", index, cell, pin)
                })? = tie_net;
                out.replace_cell(ci, patched);
                changes.push(EcoChange {
                    operation: index,
                    description: with_hierarchy(
                        format!("tied '{}':{} to {}", cell, pin, u8::from(*value)),
                        hierarchy.as_deref(),
                    ),
                });
            }
        }
    }
    validate(&out).map_err(|error| format!("patch validation failed: {}", error))?;
    let operations_applied = changes.len();
    Ok(EcoResult {
        patch_name: patch.name.clone(),
        hierarchy_scope: patch.scope.clone(),
        netlist: out,
        physical_placement: None,
        changes,
        operations_applied,
    })
}

/// Apply a parsed patch to one stable hierarchy-instance namespace.
///
/// Cell selectors remain exact post-synthesis cell names; the supplied scope
/// selects which identically named cell is patched when hierarchy metadata is
/// retained. This entry point is useful when scope is supplied by a higher
/// level flow rather than serialized in the ECO text.
pub fn apply_eco_in_hierarchy(
    nl: &Netlist,
    patch: &EcoPatch,
    hierarchy_pattern: &str,
) -> Result<EcoResult, String> {
    if hierarchy_pattern.trim().is_empty() {
        return Err("ECO hierarchy scope cannot be empty".into());
    }
    let mut scoped = patch.clone();
    if let Some(existing) = &scoped.scope {
        if existing != hierarchy_pattern {
            return Err(format!(
                "ECO patch scope '{}' does not match requested hierarchy scope '{}'",
                existing, hierarchy_pattern
            ));
        }
    }
    scoped.scope = Some(hierarchy_pattern.to_string());
    apply_eco(nl, &scoped)
}

fn find_operation_cell(
    nl: &Netlist,
    name: &str,
    scope: Option<&str>,
    operation: usize,
) -> Result<usize, String> {
    let Some(scope) = scope else {
        return find_cell(nl, name);
    };
    let mut candidates = (0..nl.cell_count()).filter(|&ci| {
        nl.cell(ci).name == name
            && nl
                .cell_hierarchy_instance(ci)
                .is_some_and(|instance| hierarchy_pattern_matches(scope, &instance.path))
    });
    let Some(ci) = candidates.next() else {
        return Err(format!(
            "operation {}: cell '{}' not found in hierarchy scope '{}'",
            operation, name, scope
        ));
    };
    if candidates.next().is_some() {
        return Err(format!(
            "operation {}: cell '{}' matches multiple instances in hierarchy scope '{}'",
            operation, name, scope
        ));
    }
    Ok(ci)
}

fn cell_hierarchy_path(nl: &Netlist, cell: usize) -> Option<String> {
    nl.cell_hierarchy_instance(cell)
        .map(|instance| instance.path.clone())
}

fn with_hierarchy(description: String, hierarchy: Option<&str>) -> String {
    match hierarchy {
        Some(path) => format!("{} at '{}'", description, path),
        None => description,
    }
}

fn attach_net_to_hierarchy(
    nl: &mut Netlist,
    hierarchy: Option<&str>,
    net: usize,
) -> Result<(), String> {
    let Some(path) = hierarchy else {
        return Ok(());
    };
    let instance = nl
        .hierarchy
        .iter_mut()
        .find(|instance| instance.path == path)
        .ok_or_else(|| format!("ECO hierarchy instance '{}' disappeared", path))?;
    if !instance.nets.contains(&net) {
        instance.nets.push(net);
        instance.nets.sort_unstable();
    }
    Ok(())
}

fn hierarchy_pattern_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('.').collect();
    let value: Vec<&str> = value.split('.').collect();
    hierarchy_segments_match(&pattern, &value)
}

fn hierarchy_segments_match(pattern: &[&str], value: &[&str]) -> bool {
    match (pattern.first(), value.first()) {
        (None, None) => true,
        (Some(&"**"), _) => {
            hierarchy_segments_match(&pattern[1..], value)
                || (!value.is_empty() && hierarchy_segments_match(pattern, &value[1..]))
        }
        (Some(pattern_segment), Some(value_segment)) => {
            path_segment_matches(pattern_segment, value_segment)
                && hierarchy_segments_match(&pattern[1..], &value[1..])
        }
        _ => false,
    }
}

fn path_segment_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    path_bytes_match(pattern, value)
}

fn path_bytes_match(pattern: &[u8], value: &[u8]) -> bool {
    match (pattern.first(), value.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            path_bytes_match(&pattern[1..], value)
                || (!value.is_empty() && path_bytes_match(pattern, &value[1..]))
        }
        (Some(b'?'), Some(_)) => path_bytes_match(&pattern[1..], &value[1..]),
        (Some(&expected), Some(&actual)) => {
            expected == actual && path_bytes_match(&pattern[1..], &value[1..])
        }
        _ => false,
    }
}

fn target_kind(
    nl: &Netlist,
    target: &EcoCellTarget,
    input_count: usize,
) -> Result<CellKind, String> {
    match target {
        EcoCellTarget::Library(name) => {
            let id = nl
                .library()
                .and_then(|lib| lib.find(name))
                .ok_or_else(|| format!("library has no cell '{}'", name))?;
            Ok(CellKind::Lib(id))
        }
        EcoCellTarget::Buffer => Ok(CellKind::Buf),
        EcoCellTarget::Inverter => Ok(CellKind::Not),
        EcoCellTarget::And => Ok(CellKind::And),
        EcoCellTarget::Nand => Ok(CellKind::Nand),
        EcoCellTarget::Or => Ok(CellKind::Or),
        EcoCellTarget::Nor => Ok(CellKind::Nor),
        EcoCellTarget::Xor => Ok(CellKind::Xor),
        EcoCellTarget::Xnor => Ok(CellKind::Xnor),
        EcoCellTarget::Mux => {
            if input_count != 3 {
                return Err("MUX replacement requires exactly three inputs".into());
            }
            Ok(CellKind::Mux)
        }
    }
}

fn validate(nl: &Netlist) -> Result<(), String> {
    nl.topo_cells()
        .ok_or_else(|| "ECO created a combinational loop".to_string())?;
    for port in &nl.ports {
        if port.dir == eda_core::netlist::Dir::Out {
            for (bit, net) in port.bits.iter().enumerate() {
                if nl.net(*net).drivers.is_empty() {
                    return Err(format!("output '{}[{}]' is undriven", port.name, bit));
                }
            }
        }
    }
    Ok(())
}

fn find_cell(nl: &Netlist, name: &str) -> Result<usize, String> {
    (0..nl.cell_count())
        .find(|&ci| nl.cell(ci).name == name)
        .ok_or_else(|| format!("cell '{}' not found", name))
}

fn find_net(nl: &Netlist, name: &str) -> Result<usize, String> {
    nl.net_by_name(name)
        .ok_or_else(|| format!("net '{}' not found", name))
}

fn unique_name(nl: &Netlist, base: &str) -> String {
    let mut name = base.to_string();
    let mut index = 1usize;
    while nl.net_by_name(&name).is_some() || find_cell(nl, &name).is_ok() {
        name = format!("{}_{}", base, index);
        index += 1;
    }
    name
}

fn buffer_kind(nl: &Netlist) -> Result<CellKind, String> {
    if let Some(library) = nl.library() {
        if let Some(id) = library.find("BUF") {
            return Ok(CellKind::Lib(id));
        }
    }
    Ok(CellKind::Buf)
}

fn logic_value(value: bool) -> LogicValue {
    if value {
        LogicValue::One
    } else {
        LogicValue::Zero
    }
}

trait ReplaceNonAlnum {
    fn replace_nonalnum(&self) -> String;
}

impl ReplaceNonAlnum for str {
    fn replace_nonalnum(&self) -> String {
        self.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }
}

pub fn report(result: &EcoResult) -> String {
    let mut out = format!(
        "ECO report '{}': {} operation(s) applied\n",
        result.patch_name, result.operations_applied
    );
    if let Some(placement) = &result.physical_placement {
        out.push_str(&format!("  Physical placement: {} rows\n", placement.rows));
    }
    if let Some(scope) = &result.hierarchy_scope {
        out.push_str(&format!("  Hierarchy scope: {}\n", scope));
    }
    for change in &result.changes {
        out.push_str(&format!(
            "  {:>3}: {}\n",
            change.operation, change.description
        ));
    }
    out
}

#[derive(Clone, Debug)]
enum Token {
    Word(String, usize),
    Number(usize, usize),
    Punct(&'static str, usize),
}

impl Token {
    fn line(&self) -> usize {
        match self {
            Token::Word(_, line) | Token::Number(_, line) | Token::Punct(_, line) => *line,
        }
    }
}

fn lex(source: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let line = match raw.find("//") {
            Some(position) => &raw[..position],
            None => raw,
        };
        let mut word = String::new();
        for character in line.chars() {
            if character.is_whitespace() {
                if !word.is_empty() {
                    push_word(&mut out, &word, line_number)?;
                    word.clear();
                }
            } else if character == ';' {
                if !word.is_empty() {
                    push_word(&mut out, &word, line_number)?;
                    word.clear();
                }
                out.push(Token::Punct(";", line_number));
            } else {
                if !matches!(
                    character,
                    'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '\\' | '.' | '*' | '?'
                ) {
                    return Err(format!(
                        "line {}: illegal ECO character '{}'",
                        line_number, character
                    ));
                }
                word.push(character);
            }
        }
        if !word.is_empty() {
            push_word(&mut out, &word, line_number)?;
        }
    }
    Ok(out)
}

fn push_word(out: &mut Vec<Token>, word: &str, line: usize) -> Result<(), String> {
    if let Ok(number) = word.parse::<usize>() {
        out.push(Token::Number(number, line));
    } else {
        out.push(Token::Word(word.to_string(), line));
    }
    Ok(())
}

fn identifier(tokens: &[Token], pos: usize, line: usize) -> Result<String, String> {
    match tokens.get(pos) {
        Some(Token::Word(word, _)) => Ok(word.clone()),
        Some(token) => Err(format!("line {}: expected identifier", token.line())),
        None => Err(format!("line {}: expected identifier", line)),
    }
}

fn number(tokens: &[Token], pos: usize, line: usize) -> Result<usize, String> {
    match tokens.get(pos) {
        Some(Token::Number(value, _)) => Ok(*value),
        Some(token) => Err(format!("line {}: expected number", token.line())),
        None => Err(format!("line {}: expected number", line)),
    }
}

fn expect_punct(tokens: &[Token], pos: usize, punct: &str, line: usize) -> Result<(), String> {
    match tokens.get(pos) {
        Some(Token::Punct(actual, _)) if *actual == punct => Ok(()),
        Some(token) => Err(format!("line {}: expected '{}'", token.line(), punct)),
        None => Err(format!("line {}: expected '{}'", line, punct)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::library::CellLibrary;
    use eda_core::netlist::Dir;
    use std::sync::Arc;

    #[test]
    fn rejects_empty_patch() {
        assert!(parse_eco("// empty\n").is_err());
    }

    #[test]
    fn fanout_repair_splits_high_fanout_nets() {
        let library = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("fanout");
        nl.set_library(library.clone());
        let a = nl.add_net("a");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "driver".into(),
            kind: CellKind::Lib(library.find("BUF").unwrap()),
            inputs: vec![a],
            output: y,
        });
        let inv = library.find("INV").unwrap();
        for index in 0..5 {
            let out = nl.add_net(format!("o{}", index));
            nl.add_cell(Cell {
                name: format!("sink{}", index),
                kind: CellKind::Lib(inv),
                inputs: vec![y],
                output: out,
            });
            nl.add_port(format!("o{}", index), Dir::Out, vec![out]);
        }
        nl.add_port("a", Dir::In, vec![a]);

        let before = nl.cell_count();
        let result = repair_fanout(
            &nl,
            &FanoutRepairOptions {
                max_fanout: 2,
                patch_name: "fanout_fix".into(),
            },
        )
        .unwrap();
        assert_eq!(result.netlist.cell_count(), before + 5);
        assert_eq!(result.netlist.net_by_name("y").unwrap(), y);
        assert_eq!(result.netlist.net(y).sinks.len(), 1);
        for sink in &result.netlist.net(y).sinks {
            let buffer_output = result.netlist.cell(*sink).output;
            let fanout = result.netlist.net(buffer_output).sinks.len();
            assert!(fanout <= 2, "buffer fanout {}", fanout);
        }
        assert_eq!(result.operations_applied, 1);
    }

    #[test]
    fn replaces_generic_dff_with_pin_compatible_library_dff() {
        let library = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("sequential_eco");
        nl.set_library(library.clone());
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let q = nl.add_net("q");
        nl.add_cell(Cell {
            name: "u_dff".into(),
            kind: CellKind::Dff { posedge: true },
            inputs: vec![clk, d],
            output: q,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("q", Dir::Out, vec![q]);

        let patch = parse_eco("eco dff_swap; replace_cell u_dff DFF;").unwrap();
        let result = apply_eco(&nl, &patch).unwrap();
        let expected = library.find("DFF").unwrap();
        assert_eq!(result.netlist.cell(0).kind, CellKind::Lib(expected));
        assert_eq!(result.netlist.cell(0).inputs, vec![clk, d]);
        assert_eq!(result.netlist.cell(0).output, q);
        assert_eq!(result.operations_applied, 1);
        assert!(result.changes[0].description.contains("replaced"));
    }

    #[test]
    fn replaces_generic_dffr_with_pin_compatible_library_dffr() {
        let library = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("sequential_reset_eco");
        nl.set_library(library.clone());
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let rst = nl.add_net("rst");
        let q = nl.add_net("q");
        nl.add_cell(Cell {
            name: "u_dffr".into(),
            kind: CellKind::DffR {
                posedge: true,
                reset_high: true,
            },
            inputs: vec![clk, d, rst],
            output: q,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("rst", Dir::In, vec![rst]);
        nl.add_port("q", Dir::Out, vec![q]);

        let patch = parse_eco("eco dffr_swap; replace_cell u_dffr DFFR;").unwrap();
        let result = apply_eco(&nl, &patch).unwrap();
        let expected = library.find("DFFR").unwrap();
        assert_eq!(result.netlist.cell(0).kind, CellKind::Lib(expected));
        assert_eq!(result.netlist.cell(0).inputs, vec![clk, d, rst]);
        assert_eq!(result.operations_applied, 1);
    }

    #[test]
    fn sequential_eco_rejects_changed_pin_roles() {
        let library = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("bad_sequential_eco");
        nl.set_library(library);
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let rst = nl.add_net("rst");
        let q = nl.add_net("q");
        nl.add_cell(Cell {
            name: "u_dff".into(),
            kind: CellKind::Dff { posedge: true },
            inputs: vec![clk, d],
            output: q,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("rst", Dir::In, vec![rst]);
        nl.add_port("q", Dir::Out, vec![q]);

        let patch = parse_eco("eco bad_swap; replace_cell u_dff DFFR;").unwrap();
        let error = apply_eco(&nl, &patch).unwrap_err();
        assert!(error.contains("identical clock/reset pin roles"), "{error}");
    }

    #[test]
    fn sequential_eco_rejects_cell_class_changes() {
        let library = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("class_eco");
        nl.set_library(library);
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let q = nl.add_net("q");
        nl.add_cell(Cell {
            name: "u_dff".into(),
            kind: CellKind::Dff { posedge: true },
            inputs: vec![clk, d],
            output: q,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("q", Dir::Out, vec![q]);

        let patch = parse_eco("eco class_swap; replace_cell u_dff AND;").unwrap();
        let error = apply_eco(&nl, &patch).unwrap_err();
        assert!(error.contains("changes cell class"), "{error}");
    }
}
