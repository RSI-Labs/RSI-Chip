//! Core timing implementation module.

use super::engine::*;
use super::incremental::*;
use super::library::*;
use super::types::*;
use crate::constraints;
use eda_core::netlist::*;
use std::collections::*;

pub fn analyze(nl: &Netlist) -> Result<StaResult, String> {
    analyze_impl(nl, None)
}

/// Analyze with SDC input delays applied as primary-input arrival offsets.
pub fn analyze_with_constraints(
    nl: &Netlist,
    constraints: &constraints::TimingConstraints,
) -> Result<StaResult, String> {
    analyze_impl(nl, Some(constraints))
}

/// Incrementally update timing after input-delay changes on one flat netlist.
///
/// The topology, library, case analysis, exceptions, and derates must remain
/// unchanged. Only arrival propagation through the transitive fanout of the
/// listed input ports is recomputed; unrelated branches retain their previous
/// late/early arrivals and selected path inputs.
pub fn update_timing_for_input_delays<I, S>(
    nl: &Netlist,
    previous: &StaResult,
    constraints: &constraints::TimingConstraints,
    changed_ports: I,
) -> Result<IncrementalTimingResult, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut changed_port_names = BTreeSet::new();
    for port in changed_ports {
        if !changed_port_names.insert(port.as_ref().to_string()) {
            return Err("duplicate incremental STA input port".into());
        }
    }
    if changed_port_names.is_empty() {
        return Err("incremental STA requires at least one changed input port".into());
    }

    let mut changed_nets = BTreeSet::new();
    for name in &changed_port_names {
        let Some(port) = nl.ports.iter().find(|port| &port.name == name) else {
            return Err(format!("incremental STA input port '{}' not found", name));
        };
        if port.dir != eda_core::netlist::Dir::In {
            return Err(format!("incremental STA port '{}' is not an input", name));
        }
        changed_nets.extend(port.bits.iter().copied());
    }
    update_timing_incremental(
        nl,
        previous,
        constraints,
        changed_port_names,
        changed_nets,
        BTreeSet::new(),
    )
}

/// Incrementally update timing after cell/pin delay-derate or SDF arc changes.
///
/// The caller must list every changed cell or pin and keep all other timing
/// inputs unchanged. Cell and pin seeds recompute that cell's output arc and
/// its transitive combinational fanout; unrelated branches are retained.
pub fn update_timing_for_timing_derates<C, S>(
    nl: &Netlist,
    previous: &StaResult,
    constraints: &constraints::TimingConstraints,
    changed_cells: C,
    changed_pins: &[constraints::TimingPin],
) -> Result<IncrementalTimingResult, String>
where
    C: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut changed_cell_names = BTreeSet::new();
    for cell in changed_cells {
        if !changed_cell_names.insert(cell.as_ref().to_string()) {
            return Err("duplicate incremental STA cell".into());
        }
    }
    if changed_cell_names.is_empty() && changed_pins.is_empty() {
        return Err("incremental STA requires at least one changed cell or pin".into());
    }

    let mut seed_cells = BTreeSet::new();
    for name in &changed_cell_names {
        let Some(cell_id) = nl.cells().iter().position(|cell| &cell.name == name) else {
            return Err(format!("incremental STA cell '{}' not found", name));
        };
        seed_cells.insert(cell_id);
    }
    for pin in changed_pins {
        let Some(cell_id) = nl.cells().iter().position(|cell| cell.name == pin.cell) else {
            return Err(format!("incremental STA cell '{}' not found", pin.cell));
        };
        if !valid_timing_pin_name(nl, cell_id, &pin.pin) {
            return Err(format!(
                "incremental STA pin '{}/{}' not found",
                pin.cell, pin.pin
            ));
        }
        seed_cells.insert(cell_id);
    }

    let changed_nets = seed_cells
        .iter()
        .map(|&cell_id| nl.cell(cell_id).output)
        .collect::<BTreeSet<_>>();
    update_timing_incremental(
        nl,
        previous,
        constraints,
        BTreeSet::new(),
        changed_nets,
        seed_cells,
    )
}

/// Incrementally update base and constrained timing after SDC changes.
///
/// The netlist topology must be unchanged. Input-delay and cell/pin delay-derate
/// changes use local propagation. Case analysis, disable timing, and global
/// delay-derate changes conservatively rebuild the base graph. Constraint-only
/// changes reuse base arrivals and rebuild the compact constrained summary.
pub fn update_timing_for_constraints(
    nl: &Netlist,
    previous: &StaResult,
    old_constraints: &constraints::TimingConstraints,
    new_constraints: &constraints::TimingConstraints,
) -> Result<ConstraintTimingResult, String> {
    let diff = constraints::diff_timing_constraints(old_constraints, new_constraints);
    let (timing, recomputed_nets, recomputed_cells, full_base_recompute) =
        if diff.full_base_recompute_required {
            let constrained = constraints::analyze_with_sdc(nl, new_constraints)?;
            let all_nets = (0..nl.net_count())
                .map(|net| nl.net(net).name.clone())
                .collect::<BTreeSet<_>>();
            let all_cells = nl
                .cells()
                .iter()
                .map(|cell| cell.name.clone())
                .collect::<BTreeSet<_>>();
            (constrained.base.clone(), all_nets, all_cells, true)
        } else if diff.base_recompute_required {
            let mut current = previous.clone();
            let mut changed_ports = diff.changed_input_ports.clone();
            changed_ports.extend(diff.changed_port_derates.iter().cloned());
            let mut recomputed_nets = BTreeSet::new();
            let mut recomputed_cells = BTreeSet::new();

            if !changed_ports.is_empty() {
                let update = update_timing_for_input_delays(
                    nl,
                    &current,
                    new_constraints,
                    changed_ports.iter().cloned(),
                )?;
                current = update.timing;
                recomputed_nets.extend(update.recomputed_nets);
                recomputed_cells.extend(update.recomputed_cells);
            }

            if !diff.changed_cell_derates.is_empty() || !diff.changed_pin_derates.is_empty() {
                let changed_pins = diff.changed_pin_derates.iter().cloned().collect::<Vec<_>>();
                let update = update_timing_for_timing_derates(
                    nl,
                    &current,
                    new_constraints,
                    diff.changed_cell_derates.iter().cloned(),
                    &changed_pins,
                )?;
                current = update.timing;
                recomputed_nets.extend(update.recomputed_nets);
                recomputed_cells.extend(update.recomputed_cells);
            }
            (current, recomputed_nets, recomputed_cells, false)
        } else {
            (previous.clone(), BTreeSet::new(), BTreeSet::new(), false)
        };

    let constrained = constraints::analyze_constrained(nl, timing.clone(), new_constraints)?;
    Ok(ConstraintTimingResult {
        timing,
        constrained,
        diff,
        recomputed_nets,
        recomputed_cells,
        total_cells: nl.cell_count(),
        full_base_recompute,
    })
}

pub(super) fn timing_cell_signature(nl: &Netlist, cell: &eda_core::netlist::Cell) -> String {
    let library_cell = match &cell.kind {
        CellKind::Lib(id) => nl
            .library()
            .map(|library| format!("{}:{}", library.name, library.get(*id).name)),
        _ => None,
    };
    let inputs = cell
        .inputs
        .iter()
        .map(|&net| nl.net(net).name.clone())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{}|{}|{}",
        library_cell.unwrap_or_else(|| format!("{:?}", cell.kind)),
        inputs,
        nl.net(cell.output).name
    )
}

pub(super) fn timing_net_drivers(nl: &Netlist, net: NetId) -> BTreeSet<String> {
    nl.net(net)
        .drivers
        .iter()
        .map(|&cell| nl.cell(cell).name.clone())
        .collect()
}

/// Compare two flat netlists by stable cell/net/port names.
///
/// Numeric IDs are intentionally ignored. Library cells are compared by
/// library/definition name, so an SDF-annotated replacement is detected even
/// when its numeric `LibCellId` changes.
pub fn diff_timing_topology(old: &Netlist, new: &Netlist) -> TimingTopologyDiff {
    let mut diff = TimingTopologyDiff::default();
    let old_cells = old
        .cells()
        .iter()
        .map(|cell| (cell.name.clone(), cell))
        .collect::<BTreeMap<_, _>>();
    let new_cells = new
        .cells()
        .iter()
        .map(|cell| (cell.name.clone(), cell))
        .collect::<BTreeMap<_, _>>();
    for name in old_cells.keys() {
        if !new_cells.contains_key(name) {
            diff.removed_cells.insert(name.clone());
        }
    }
    for (name, cell) in &new_cells {
        if !old_cells.contains_key(name) {
            diff.added_cells.insert(name.clone());
            continue;
        }
        let old_cell = old_cells[name];
        if timing_cell_signature(old, old_cell) != timing_cell_signature(new, cell) {
            diff.changed_cells.insert(name.clone());
        }
    }

    let old_nets = (0..old.net_count())
        .map(|net| (old.net(net).name.clone(), net))
        .collect::<BTreeMap<_, _>>();
    let new_nets = (0..new.net_count())
        .map(|net| (new.net(net).name.clone(), net))
        .collect::<BTreeMap<_, _>>();
    for name in old_nets.keys() {
        if !new_nets.contains_key(name) {
            diff.removed_nets.insert(name.clone());
        }
    }
    for (name, &net) in &new_nets {
        if !old_nets.contains_key(name) {
            diff.added_nets.insert(name.clone());
            continue;
        }
        if timing_net_drivers(old, old_nets[name]) != timing_net_drivers(new, net) {
            diff.changed_nets.insert(name.clone());
        }
    }

    let old_ports = old
        .ports
        .iter()
        .map(|port| (port.name.clone(), port))
        .collect::<BTreeMap<_, _>>();
    let new_ports = new
        .ports
        .iter()
        .map(|port| (port.name.clone(), port))
        .collect::<BTreeMap<_, _>>();
    for name in old_ports.keys().chain(new_ports.keys()) {
        let (Some(old_port), Some(new_port)) = (old_ports.get(name), new_ports.get(name)) else {
            diff.changed_ports.insert(name.clone());
            continue;
        };
        let old_bits = old_port
            .bits
            .iter()
            .map(|&net| old.net(net).name.clone())
            .collect::<BTreeSet<_>>();
        let new_bits = new_port
            .bits
            .iter()
            .map(|&net| new.net(net).name.clone())
            .collect::<BTreeSet<_>>();
        if old_port.dir != new_port.dir || old_bits != new_bits {
            diff.changed_ports.insert(name.clone());
            if new_port.dir == eda_core::netlist::Dir::In {
                diff.changed_nets.extend(new_bits);
            }
        }
    }
    diff
}

pub(super) fn count_disabled_timing(
    new: &Netlist,
    constraints: &constraints::TimingConstraints,
) -> usize {
    let mut count = 0usize;
    for (cell_id, cell) in new.cells().iter().enumerate() {
        if nl_is_sequential(new, cell_id) {
            if constraints::timing_arc_disabled(new, constraints, cell_id, Some(0), None) {
                count += 1;
            }
            if constraints::timing_check_disabled(new, constraints, cell_id, 1) {
                count += 1;
            }
            if let Some((reset_pin, _, _)) = seq_reset(new, &cell.kind) {
                if constraints::timing_check_disabled(new, constraints, cell_id, reset_pin) {
                    count += 1;
                }
            }
        } else {
            for pin in 0..cell.inputs.len() {
                if constraints::timing_arc_disabled(new, constraints, cell_id, Some(pin), None) {
                    count += 1;
                }
            }
        }
    }
    count
}
