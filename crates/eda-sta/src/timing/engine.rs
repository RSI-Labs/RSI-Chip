//! Core timing implementation module.

use super::incremental::*;
use super::library::*;
use super::types::*;
use crate::constraints;
use eda_core::netlist::*;

pub(super) fn analyze_impl(
    nl: &Netlist,
    constraints: Option<&constraints::TimingConstraints>,
) -> Result<StaResult, String> {
    let order = analysis_order(nl).ok_or_else(|| "netlist has a combinational loop".to_string())?;
    let n_nets = nl.net_count();
    if let Some(constraints) = constraints {
        let cell_names: std::collections::BTreeSet<_> =
            nl.cells().iter().map(|cell| cell.name.as_str()).collect();
        for cell in constraints.cell_timing_derates.keys() {
            if !cell_names.contains(cell.as_str()) {
                return Err(format!("timing derate cell '{}' not found", cell));
            }
        }
        for cell in constraints.cell_check_derates.keys() {
            if !cell_names.contains(cell.as_str()) {
                return Err(format!("check derate cell '{}' not found", cell));
            }
        }
        let port_names: std::collections::BTreeSet<_> =
            nl.ports.iter().map(|port| port.name.as_str()).collect();
        for port in constraints.port_timing_derates.keys() {
            if !port_names.contains(port.as_str()) {
                return Err(format!("timing derate port '{}' not found", port));
            }
        }
        for pin in constraints.pin_timing_derates.keys() {
            let Some((cell_id, _cell)) = nl
                .cells()
                .iter()
                .enumerate()
                .find(|(_, cell)| cell.name == pin.cell)
            else {
                return Err(format!("timing derate cell '{}' not found", pin.cell));
            };
            if !valid_timing_pin_name(nl, cell_id, &pin.pin) {
                return Err(format!(
                    "timing derate pin '{}/{}' not found",
                    pin.cell, pin.pin
                ));
            }
        }
        for pin in constraints.pin_check_derates.keys() {
            let Some((cell_id, _cell)) = nl
                .cells()
                .iter()
                .enumerate()
                .find(|(_, cell)| cell.name == pin.cell)
            else {
                return Err(format!("check derate cell '{}' not found", pin.cell));
            };
            if !valid_input_pin_name(nl, cell_id, &pin.pin) {
                return Err(format!(
                    "check derate pin '{}/{}' not found",
                    pin.cell, pin.pin
                ));
            }
        }
    }
    let (mut constant_nets, mut case_analysis_nets) = resolve_case_analyses(nl, constraints)?;
    let mut arrival = vec![f64::NEG_INFINITY; n_nets];
    let mut min_arrival = vec![f64::INFINITY; n_nets];
    let mut transition = vec![f64::NAN; n_nets];
    let mut min_transition = vec![f64::NAN; n_nets];
    let mut late_path_inputs = vec![None; n_nets];
    let mut early_path_inputs = vec![None; n_nets];
    let mut disabled_timing_arcs = 0usize;
    // Primary inputs launch at 0, or at their SDC input delay.
    for p in &nl.ports {
        if p.dir == eda_core::netlist::Dir::In {
            let input_delay = constraints
                .and_then(|c| c.input_delays.get(&p.name))
                .map(|d| d.delay);
            if let Some(delay) = input_delay {
                if !delay.is_finite() {
                    return Err(format!("input delay for '{}' is not finite", p.name));
                }
            }
            for &b in &p.bits {
                let derate = timing_derate_for_port(constraints, &p.name);
                arrival[b] = input_delay.unwrap_or(0.0) * derate.late;
                min_arrival[b] = input_delay.unwrap_or(0.0) * derate.early;
                transition[b] = 0.0;
                min_transition[b] = 0.0;
                if let Some(constraints) = constraints {
                    let (minimum, maximum) = clock_transition_bounds(Some(constraints), &p.name);
                    transition[b] = maximum;
                    min_transition[b] = minimum;
                }
            }
        }
    }
    for (net, value) in constant_nets.iter().enumerate() {
        if value.is_some() {
            arrival[net] = f64::NEG_INFINITY;
            min_arrival[net] = f64::INFINITY;
            transition[net] = 0.0;
            min_transition[net] = 0.0;
        }
    }

    // Sequential outputs launch at clk_to_q (topological order is safe:
    // flops break combinational cycles).
    for (cell_id, cell) in nl.cells().iter().enumerate() {
        if let Some(c2q) = seq_clk_to_q(nl, &cell.kind) {
            if cell
                .inputs
                .first()
                .and_then(|&clock_net| constant_nets[clock_net])
                .is_some()
            {
                continue;
            }
            if constant_nets[cell.output].is_some() {
                continue;
            }
            if constraints.is_some_and(|constraints| {
                constraints::timing_arc_disabled(nl, constraints, cell_id, Some(0), None)
            }) {
                disabled_timing_arcs += 1;
                continue;
            }
            let derate = timing_derate_for_output_pin(nl, constraints, &cell.name, cell_id);
            arrival[cell.output] = c2q * derate.late;
            min_arrival[cell.output] = c2q * derate.early;
            transition[cell.output] = c2q;
            min_transition[cell.output] = c2q;
        }
    }

    let mut reg2reg = 0.0f64;
    let mut in2reg = 0.0f64;
    let mut reg2reg_hold = f64::INFINITY;
    let mut in2reg_hold = f64::INFINITY;
    let mut setup_endpoints: Vec<(usize, f64)> = Vec::new();
    let mut hold_endpoints: Vec<HoldEndpoint> = Vec::new();
    let mut reset_endpoints: Vec<ResetEndpoint> = Vec::new();
    let mut best_end = (f64::NEG_INFINITY, None::<NetId>);

    for &ci in &order {
        let cell = nl.cell(ci);
        if seq_setup(nl, &cell.kind).is_some() {
            if cell
                .inputs
                .first()
                .and_then(|&clock_net| constant_nets[clock_net])
                .is_some()
            {
                continue;
            }
            // Setup check on the data pin (pin 1 of DFF-like cells).
            let data_pin = 1;
            let data_check_disabled = constraints.is_some_and(|constraints| {
                constraints::timing_check_disabled(nl, constraints, ci, data_pin)
            });
            if data_check_disabled {
                disabled_timing_arcs += 1;
            }
            if !data_check_disabled && data_pin < cell.inputs.len() {
                let dnet = cell.inputs[data_pin];
                let a = arrival[dnet];
                let min_a = min_arrival[dnet];
                let clock_net = cell.inputs.first().copied();
                let setup = seq_setup_at(
                    nl,
                    ci,
                    transition.get(dnet).copied(),
                    clock_net.and_then(|net| transition.get(net).copied()),
                )?;
                if a.is_finite() {
                    let path = a + setup;
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
            }
            if let Some((reset_pin, _, _)) = seq_reset(nl, &cell.kind) {
                if let Some(&reset_net) = cell.inputs.get(reset_pin) {
                    if constraints.is_some_and(|constraints| {
                        constraints::timing_check_disabled(nl, constraints, ci, reset_pin)
                    }) {
                        disabled_timing_arcs += 1;
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
            continue;
        }
        // Combinational cell: pair each input arrival with its own arc so
        // pin-specific derates cannot leak onto unrelated input arcs.
        let out = cell.output;
        if constant_nets[out].is_some() {
            continue;
        }
        if let Some(value) = evaluated_constant(nl, ci, &constant_nets) {
            constant_nets[out] = Some(value);
            case_analysis_nets.insert(nl.net(out).name.clone(), value);
            arrival[out] = f64::NEG_INFINITY;
            min_arrival[out] = f64::INFINITY;
            transition[out] = 0.0;
            min_transition[out] = 0.0;
            late_path_inputs[out] = None;
            early_path_inputs[out] = None;
            continue;
        }
        let mut t = f64::NEG_INFINITY;
        let mut late_input = None;
        let mut late_transition = 0.0_f64;
        for (pin, &input) in cell.inputs.iter().enumerate() {
            if constraints.is_some_and(|constraints| {
                constraints::timing_arc_disabled(nl, constraints, ci, Some(pin), None)
            }) {
                disabled_timing_arcs += 1;
                continue;
            }
            let input_time = arrival[input];
            if input_time.is_finite() {
                let derate = timing_derate_for_input_pin(nl, constraints, &cell.name, ci, pin);
                let arc_delay =
                    input_arc_delay_with_transition(nl, ci, pin, derate.late, transition[input]);
                let candidate = input_time + arc_delay;
                if candidate > t {
                    t = candidate;
                    late_input = Some(input);
                    late_transition = output_arc_transition(
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
        if !t.is_finite() {
            continue; // unreachable from any source
        }
        if t > arrival[out] {
            arrival[out] = t;
            late_path_inputs[out] = late_input;
            transition[out] = late_transition;
        }
        let mut earliest = f64::INFINITY;
        let mut early_input = None;
        let mut early_transition = f64::INFINITY;
        for (pin, &input) in cell.inputs.iter().enumerate() {
            let input_time = min_arrival[input];
            if input_time.is_finite() {
                let derate = timing_derate_for_input_pin(nl, constraints, &cell.name, ci, pin);
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
                    early_input = Some(input);
                    early_transition = output_arc_transition(
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
        if earliest.is_finite() && earliest < min_arrival[out] {
            min_arrival[out] = earliest;
            early_path_inputs[out] = early_input;
            min_transition[out] = early_transition;
        }
        if t > best_end.0 {
            best_end = (t, Some(out));
        }
    }

    let critical_path_delay = arrival
        .iter()
        .copied()
        .filter(|t| t.is_finite())
        .fold(0.0f64, f64::max);

    let critical_cells = match best_end.1 {
        Some(net) => backtrack(nl, &arrival, net),
        None => Vec::new(),
    };

    Ok(StaResult {
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
    })
}
