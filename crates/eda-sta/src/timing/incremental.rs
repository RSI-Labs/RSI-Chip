//! Core timing implementation module.

use super::api::*;
use super::library::*;
use super::types::*;
use crate::constraints;
use eda_core::logic::LogicValue;
use eda_core::netlist::*;
use std::collections::*;

pub(super) fn nl_is_sequential(nl: &Netlist, cell_id: usize) -> bool {
    nl.cell_is_sequential(&nl.cell(cell_id).kind)
}

pub(super) fn remap_previous_timing(
    old: &Netlist,
    previous: &StaResult,
    new: &Netlist,
    diff: &TimingTopologyDiff,
    constraints: &constraints::TimingConstraints,
) -> Result<StaResult, String> {
    if old.net_count() != previous.arrivals.len()
        || old.net_count() != previous.min_arrivals.len()
        || old.net_count() != previous.transitions.len()
        || old.net_count() != previous.late_path_inputs.len()
        || old.net_count() != previous.early_path_inputs.len()
    {
        return Err("previous STA result does not match the old netlist".into());
    }

    let old_nets = (0..old.net_count())
        .map(|net| (old.net(net).name.clone(), net))
        .collect::<BTreeMap<_, _>>();
    let new_nets = (0..new.net_count())
        .map(|net| (new.net(net).name.clone(), net))
        .collect::<BTreeMap<_, _>>();
    let changed_or_added = diff
        .changed_nets
        .iter()
        .chain(&diff.added_nets)
        .cloned()
        .collect::<BTreeSet<_>>();

    let mut arrivals = vec![f64::NEG_INFINITY; new.net_count()];
    let mut min_arrivals = vec![f64::INFINITY; new.net_count()];
    let mut transitions = vec![f64::NAN; new.net_count()];
    let mut min_transitions = vec![f64::NAN; new.net_count()];
    let mut late_path_inputs = vec![None; new.net_count()];
    let mut early_path_inputs = vec![None; new.net_count()];
    let mut case_analysis_nets = BTreeMap::new();

    for (name, &new_net) in &new_nets {
        if changed_or_added.contains(name) {
            continue;
        }
        let Some(&old_net) = old_nets.get(name) else {
            continue;
        };
        arrivals[new_net] = previous.arrivals[old_net];
        min_arrivals[new_net] = previous.min_arrivals[old_net];
        transitions[new_net] = previous.transitions[old_net];
        min_transitions[new_net] = previous.min_transitions[old_net];
        if let Some(old_input) = previous.late_path_inputs[old_net] {
            let input_name = old.net(old_input).name.clone();
            if let Some(&mapped_input) = new_nets.get(&input_name) {
                late_path_inputs[new_net] = Some(mapped_input);
            }
        }
        if let Some(old_input) = previous.early_path_inputs[old_net] {
            let input_name = old.net(old_input).name.clone();
            if let Some(&mapped_input) = new_nets.get(&input_name) {
                early_path_inputs[new_net] = Some(mapped_input);
            }
        }
        if let Some(value) = previous.case_analysis_nets.get(name) {
            case_analysis_nets.insert(name.clone(), *value);
        }
    }

    Ok(StaResult {
        critical_path_delay: 0.0,
        reg2reg_critical: 0.0,
        in2reg_critical: 0.0,
        reg2reg_hold_critical: 0.0,
        in2reg_hold_critical: 0.0,
        disabled_timing_arcs: count_disabled_timing(new, constraints),
        case_analysis_nets,
        arrivals,
        min_arrivals,
        transitions,
        min_transitions,
        late_path_inputs,
        early_path_inputs,
        critical_cells: Vec::new(),
        setup_endpoints: Vec::new(),
        hold_endpoints: Vec::new(),
        reset_endpoints: Vec::new(),
    })
}

/// Incrementally update timing after a flat-netlist ECO.
///
/// Stable names identify unchanged nets and cells. Unchanged branches are
/// remapped into new numeric IDs and retained; added/changed cells and their
/// transitive combinational fanout are recomputed.
pub fn update_timing_for_topology(
    old: &Netlist,
    previous: &StaResult,
    new: &Netlist,
    constraints: &constraints::TimingConstraints,
) -> Result<TopologyTimingResult, String> {
    let diff = diff_timing_topology(old, new);
    let remapped = remap_previous_timing(old, previous, new, &diff, constraints)?;

    let new_nets = (0..new.net_count())
        .map(|net| (new.net(net).name.clone(), net))
        .collect::<BTreeMap<_, _>>();
    let mut changed_nets = BTreeSet::new();
    for name in diff.changed_nets.iter().chain(&diff.added_nets) {
        if let Some(&net) = new_nets.get(name) {
            changed_nets.insert(net);
        }
    }

    let mut seed_cells = BTreeSet::new();
    for name in diff.added_cells.iter().chain(&diff.changed_cells) {
        let Some(cell_id) = new.cells().iter().position(|cell| &cell.name == name) else {
            continue;
        };
        seed_cells.insert(cell_id);
        let output = new.cell(cell_id).output;
        changed_nets.insert(output);
    }

    let mut changed_ports = BTreeSet::new();
    for port in &diff.changed_ports {
        if new
            .ports
            .iter()
            .any(|candidate| candidate.name == *port && candidate.dir == eda_core::netlist::Dir::In)
        {
            changed_ports.insert(port.clone());
        }
    }

    let incremental = update_timing_incremental(
        new,
        &remapped,
        constraints,
        changed_ports,
        changed_nets,
        seed_cells,
    )?;
    Ok(TopologyTimingResult {
        timing: incremental.timing,
        diff,
        recomputed_nets: incremental.recomputed_nets,
        recomputed_cells: incremental.recomputed_cells,
        total_cells: incremental.total_cells,
    })
}

/// Shared same-topology incremental propagation engine.
///
/// Callers provide already validated changed nets and seed cells. The engine
/// recomputes their combinational fanout, rebuilds compact sequential endpoint
/// summaries, and retains unrelated late/early arrivals from `previous`.
#[allow(clippy::too_many_arguments)]
pub(super) fn update_timing_incremental(
    nl: &Netlist,
    previous: &StaResult,
    constraints: &constraints::TimingConstraints,
    changed_port_names: BTreeSet<String>,
    changed_nets: BTreeSet<NetId>,
    seed_cells: BTreeSet<usize>,
) -> Result<IncrementalTimingResult, String> {
    if nl.net_count() != previous.arrivals.len()
        || nl.net_count() != previous.min_arrivals.len()
        || nl.net_count() != previous.transitions.len()
        || nl.net_count() != previous.min_transitions.len()
        || nl.net_count() != previous.late_path_inputs.len()
        || nl.net_count() != previous.early_path_inputs.len()
    {
        return Err("incremental STA requires an unchanged netlist topology".into());
    }

    let (constant_nets, case_analysis_nets) = resolve_case_analyses(nl, Some(constraints))?;
    let mut constant_nets = constant_nets;
    let mut case_analysis_nets = case_analysis_nets;
    for (name, value) in &previous.case_analysis_nets {
        let Some(net) = nl.net_by_name(name) else {
            continue;
        };
        if changed_nets.contains(&net) {
            continue;
        }
        if constant_nets[net].is_none() {
            constant_nets[net] = Some(*value);
        }
        case_analysis_nets.entry(name.clone()).or_insert(*value);
    }
    let mut arrival = previous.arrivals.clone();
    let mut min_arrival = previous.min_arrivals.clone();
    let mut transition = previous.transitions.clone();
    let mut min_transition = previous.min_transitions.clone();
    let mut late_path_inputs = previous.late_path_inputs.clone();
    let mut early_path_inputs = previous.early_path_inputs.clone();
    let disabled_timing_arcs = previous.disabled_timing_arcs;

    // Recompute the explicitly changed launch points. Their downstream cone is
    // collected below, while all unrelated old arrivals remain in place.
    for name in &changed_port_names {
        let Some(port) = nl.ports.iter().find(|port| &port.name == name) else {
            unreachable!("input port validated above");
        };
        let input_delay = constraints.input_delays.get(name).map(|delay| delay.delay);
        let derate = timing_derate_for_port(Some(constraints), name);
        for &net in &port.bits {
            if constant_nets[net].is_some() {
                arrival[net] = f64::NEG_INFINITY;
                min_arrival[net] = f64::INFINITY;
                transition[net] = 0.0;
                min_transition[net] = 0.0;
            } else {
                arrival[net] = input_delay.unwrap_or(0.0) * derate.late;
                min_arrival[net] = input_delay.unwrap_or(0.0) * derate.early;
                transition[net] = 0.0;
                min_transition[net] = 0.0;
                let (minimum, maximum) = clock_transition_bounds(Some(constraints), name);
                transition[net] = maximum;
                min_transition[net] = minimum;
            }
        }
    }

    // A changed sequential-cell timing arc relaunches its q output. Data and
    // reset input changes stop at the sequential boundary instead.
    for &cell_id in &seed_cells {
        let cell = nl.cell(cell_id);
        if let Some(c2q) = seq_clk_to_q(nl, &cell.kind) {
            if constant_nets[cell.output].is_some()
                || constraints::timing_arc_disabled(nl, constraints, cell_id, Some(0), None)
            {
                arrival[cell.output] = f64::NEG_INFINITY;
                min_arrival[cell.output] = f64::INFINITY;
                transition[cell.output] = 0.0;
                min_transition[cell.output] = 0.0;
                continue;
            }
            let derate = timing_derate_for_output_pin(nl, Some(constraints), &cell.name, cell_id);
            arrival[cell.output] = c2q * derate.late;
            min_arrival[cell.output] = c2q * derate.early;
            transition[cell.output] = c2q;
            min_transition[cell.output] = c2q;
        }
    }

    // Find the combinational fanout of the changed seeds. Sequential cells are
    // timing boundaries: their inputs are checked, but their q outputs do not
    // automatically become changing data-launch points.
    let mut affected_cells = BTreeSet::new();
    let mut affected_nets = changed_nets.clone();
    let mut stack = changed_nets.iter().copied().collect::<Vec<_>>();
    while let Some(net) = stack.pop() {
        for &cell_id in &nl.net(net).sinks {
            let cell = nl.cell(cell_id);
            if nl.cell_is_sequential(&cell.kind) {
                continue;
            }
            if !affected_cells.insert(cell_id) {
                continue;
            }
            if affected_nets.insert(cell.output) {
                stack.push(cell.output);
            }
        }
    }
    affected_cells.extend(seed_cells.iter().copied());

    let order = nl
        .topo_cells()
        .ok_or_else(|| "netlist has a combinational loop".to_string())?;
    for ci in order {
        if !affected_cells.contains(&ci) {
            continue;
        }
        let cell = nl.cell(ci);
        if nl.cell_is_sequential(&cell.kind) {
            continue;
        }
        let out = cell.output;
        if constant_nets[out].is_some() {
            arrival[out] = f64::NEG_INFINITY;
            min_arrival[out] = f64::INFINITY;
            transition[out] = 0.0;
            min_transition[out] = 0.0;
            late_path_inputs[out] = None;
            early_path_inputs[out] = None;
            continue;
        }

        let mut latest = f64::NEG_INFINITY;
        let mut latest_input = None;
        let mut latest_transition = 0.0_f64;
        for (pin, &input) in cell.inputs.iter().enumerate() {
            if constraints::timing_arc_disabled(nl, constraints, ci, Some(pin), None) {
                continue;
            }
            let input_time = arrival[input];
            if input_time.is_finite() {
                let derate =
                    timing_derate_for_input_pin(nl, Some(constraints), &cell.name, ci, pin);
                let arc_delay =
                    input_arc_delay_with_transition(nl, ci, pin, derate.late, transition[input]);
                let candidate = input_time + arc_delay;
                if candidate > latest {
                    latest = candidate;
                    latest_input = Some(input);
                    latest_transition = output_arc_transition(
                        nl,
                        ci,
                        pin,
                        derate.late,
                        transition[input],
                        arc_delay,
                    )
                    .max(0.0);
                }
            }
        }
        if latest.is_finite() {
            arrival[out] = latest;
            late_path_inputs[out] = latest_input;
            transition[out] = latest_transition;
        } else {
            arrival[out] = f64::NEG_INFINITY;
            late_path_inputs[out] = None;
            transition[out] = f64::NAN;
        }

        let mut earliest = f64::INFINITY;
        let mut earliest_input = None;
        let mut earliest_transition = f64::INFINITY;
        for (pin, &input) in cell.inputs.iter().enumerate() {
            if constraints::timing_arc_disabled(nl, constraints, ci, Some(pin), None) {
                continue;
            }
            let input_time = min_arrival[input];
            if input_time.is_finite() {
                let derate =
                    timing_derate_for_input_pin(nl, Some(constraints), &cell.name, ci, pin);
                let arc_delay = input_arc_delay_with_transition(
                    nl,
                    ci,
                    pin,
                    derate.early,
                    min_transition[input],
                );
                let candidate = input_time + arc_delay;
                if candidate < earliest {
                    earliest = candidate;
                    earliest_input = Some(input);
                    earliest_transition = output_arc_transition(
                        nl,
                        ci,
                        pin,
                        derate.early,
                        min_transition[input],
                        arc_delay,
                    )
                    .max(0.0);
                }
            }
        }
        if earliest.is_finite() {
            min_arrival[out] = earliest;
            early_path_inputs[out] = earliest_input;
            min_transition[out] = if earliest_transition.is_finite() {
                earliest_transition
            } else {
                f64::NAN
            };
        } else {
            min_arrival[out] = f64::INFINITY;
            early_path_inputs[out] = None;
            min_transition[out] = f64::NAN;
        }
    }

    // Sequential checks are compact endpoint summaries and are rebuilt without
    // propagating through the flops.
    let mut reg2reg = 0.0f64;
    let mut in2reg = 0.0f64;
    let mut reg2reg_hold = f64::INFINITY;
    let mut in2reg_hold = f64::INFINITY;
    let mut setup_endpoints = Vec::new();
    let mut hold_endpoints = Vec::new();
    let mut reset_endpoints = Vec::new();
    let mut best_end = (f64::NEG_INFINITY, None::<NetId>);

    for (ci, cell) in nl.cells().iter().enumerate() {
        if seq_setup(nl, &cell.kind).is_none() {
            continue;
        }
        if cell
            .inputs
            .first()
            .and_then(|&clock_net| constant_nets[clock_net])
            .is_some()
        {
            continue;
        }
        let data_pin = 1;
        if constraints::timing_check_disabled(nl, constraints, ci, data_pin) {
            continue;
        }
        let Some(&dnet) = cell.inputs.get(data_pin) else {
            continue;
        };
        let clock_net = cell.inputs.first().copied();
        let setup = seq_setup_at(
            nl,
            ci,
            transition.get(dnet).copied(),
            clock_net.and_then(|net| transition.get(net).copied()),
        )?;
        let a = arrival[dnet];
        let min_a = min_arrival[dnet];
        if a.is_finite() {
            let check_derate = constraints::pin_check_derate(nl, constraints, ci, 1);
            let path = a + setup * check_derate.late;
            if driven_from_sequential(nl, dnet) {
                reg2reg = reg2reg.max(path);
            } else {
                in2reg = in2reg.max(path);
            }
            setup_endpoints.push((ci, a));
            if a > best_end.0 {
                best_end = (a, Some(dnet));
            }
        }
        if min_a.is_finite() {
            let from_sequential = driven_from_sequential(nl, dnet);
            if from_sequential {
                reg2reg_hold = reg2reg_hold.min(min_a);
            } else {
                in2reg_hold = in2reg_hold.min(min_a);
            }
            hold_endpoints.push(HoldEndpoint {
                cell: ci,
                data_arrival: min_a,
                launched_from_sequential: from_sequential,
            });
        }

        if let Some((reset_pin, _, _)) = seq_reset(nl, &cell.kind) {
            if let Some(&reset_net) = cell.inputs.get(reset_pin) {
                if constraints::timing_check_disabled(nl, constraints, ci, reset_pin) {
                    continue;
                }
                let reset_arrival = arrival[reset_net];
                let reset_min_arrival = min_arrival[reset_net];
                if reset_arrival.is_finite() && reset_min_arrival.is_finite() {
                    reset_endpoints.push(ResetEndpoint {
                        cell: ci,
                        reset_arrival,
                        reset_min_arrival,
                    });
                }
            }
        }
    }

    for (net, &time) in arrival.iter().enumerate() {
        if time.is_finite() && time > best_end.0 {
            best_end = (time, Some(net));
        }
    }
    let critical_path_delay = arrival
        .iter()
        .copied()
        .filter(|time| time.is_finite())
        .fold(0.0f64, f64::max);
    let critical_cells = match best_end.1 {
        Some(net) => backtrack(nl, &arrival, net),
        None => Vec::new(),
    };

    let timing = StaResult {
        critical_path_delay: critical_path_delay.max(0.0),
        reg2reg_critical: reg2reg.max(0.0),
        in2reg_critical: in2reg.max(0.0),
        reg2reg_hold_critical: if reg2reg_hold.is_finite() {
            reg2reg_hold
        } else {
            0.0
        },
        in2reg_hold_critical: if in2reg_hold.is_finite() {
            in2reg_hold
        } else {
            0.0
        },
        disabled_timing_arcs,
        case_analysis_nets,
        arrivals: arrival,
        min_arrivals: min_arrival,
        transitions: transition,
        min_transitions: min_transition,
        late_path_inputs,
        early_path_inputs,
        critical_cells,
        setup_endpoints,
        hold_endpoints,
        reset_endpoints,
    };

    let recomputed_nets = affected_nets
        .iter()
        .map(|&net| nl.net(net).name.clone())
        .collect::<BTreeSet<_>>();
    let recomputed_cells = affected_cells
        .iter()
        .map(|&cell| nl.cell(cell).name.clone())
        .collect::<BTreeSet<_>>();
    Ok(IncrementalTimingResult {
        timing,
        changed_ports: changed_port_names,
        recomputed_nets,
        recomputed_cells,
        total_cells: nl.cell_count(),
    })
}

pub(super) fn case_analysis_matches_net(
    nl: &Netlist,
    object: &constraints::TimingObjectRef,
    net: NetId,
) -> bool {
    match object.kind {
        constraints::TimingObjectKind::Port => nl
            .ports
            .iter()
            .any(|port| object.matches_name(&port.name) && port.bits.contains(&net)),
        constraints::TimingObjectKind::Net => object.matches_name(&nl.net(net).name),
        constraints::TimingObjectKind::Pin => {
            nl.net(net).drivers.iter().any(|&cell_id| {
                output_pin_name(nl, cell_id).is_some_and(|pin| {
                    object.matches_name(&format!("{}/{}", nl.cell(cell_id).name, pin))
                })
            }) || nl.net(net).sinks.iter().any(|&cell_id| {
                nl.cell(cell_id)
                    .inputs
                    .iter()
                    .enumerate()
                    .any(|(pin, &input)| {
                        input == net
                            && input_pin_name(nl, cell_id, pin).is_some_and(|pin_name| {
                                object.matches_name(&format!(
                                    "{}/{}",
                                    nl.cell(cell_id).name,
                                    pin_name
                                ))
                            })
                    })
            })
        }
        _ => false,
    }
}

pub(super) fn net_hierarchy_matches(nl: &Netlist, net: NetId, pattern: &str) -> bool {
    nl.owning_hierarchy_instance(net).is_some_and(|instance| {
        constraints::TimingObjectPattern::new(pattern).matches(&instance.path)
    })
}

type CaseAnalysisResolution = (Vec<Option<bool>>, BTreeMap<String, bool>);

pub(super) fn resolve_case_analyses(
    nl: &Netlist,
    constraints: Option<&constraints::TimingConstraints>,
) -> Result<CaseAnalysisResolution, String> {
    let mut constants = vec![None; nl.net_count()];
    let mut named = BTreeMap::new();
    let Some(constraints) = constraints else {
        return Ok((constants, named));
    };
    for analysis in &constraints.case_analyses {
        for (net, constant) in constants.iter_mut().enumerate() {
            if !case_analysis_matches_net(nl, &analysis.object, net) {
                continue;
            }
            if let Some(pattern) = &analysis.instance_pattern {
                if !net_hierarchy_matches(nl, net, pattern) {
                    continue;
                }
            }
            if let Some(old) = *constant {
                if old != analysis.value {
                    return Err(format!(
                        "conflicting set_case_analysis values for net '{}'",
                        nl.net(net).name
                    ));
                }
            }
            *constant = Some(analysis.value);
            named.insert(nl.net(net).name.clone(), analysis.value);
        }
    }
    Ok((constants, named))
}

pub(super) fn evaluated_constant(
    nl: &Netlist,
    cell_id: usize,
    constants: &[Option<bool>],
) -> Option<bool> {
    let cell = nl.cell(cell_id);
    let inputs = cell
        .inputs
        .iter()
        .map(|&net| constants[net].map(LogicValue::bit).unwrap_or(LogicValue::X))
        .collect::<Vec<_>>();
    let value = match &cell.kind {
        CellKind::Lib(id) => {
            let library = nl.library()?;
            let definition = library.get(*id);
            if definition.seq.is_some() {
                return None;
            }
            definition.func.eval(&inputs)
        }
        other => other.eval(&inputs),
    };
    value.as_bool()
}

/// Return a timing evaluation order with every combinational stage before its
/// sequential sinks.
///
/// `Netlist::topo_cells` treats sequential cells as cut points and therefore
/// does not require a flop to follow combinational logic on its data, clock, or
/// reset pins. STA consumes those arrival values when it visits the flop, so it
/// deliberately partitions the generic order into combinational and sequential
/// phases while preserving each phase's relative deterministic order.
pub(super) fn analysis_order(nl: &Netlist) -> Option<Vec<usize>> {
    let order = nl.topo_cells()?;
    let mut timing_order = order
        .iter()
        .copied()
        .filter(|&cell| !nl.cell_is_sequential(&nl.cell(cell).kind))
        .collect::<Vec<_>>();
    timing_order.extend(
        order
            .into_iter()
            .filter(|cell| nl.cell_is_sequential(&nl.cell(*cell).kind)),
    );
    Some(timing_order)
}
