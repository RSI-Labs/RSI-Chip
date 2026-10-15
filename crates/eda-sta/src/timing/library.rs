//! Core timing implementation module.

use crate::constraints;
use eda_core::netlist::*;

pub(super) fn clock_transition_bounds(
    constraints: Option<&constraints::TimingConstraints>,
    port: &str,
) -> (f64, f64) {
    let Some(constraints) = constraints else {
        return (0.0, 0.0);
    };
    let mut bounds = (0.0_f64, 0.0_f64);
    for clock in constraints
        .clocks
        .values()
        .filter(|clock| clock.ports.iter().any(|clock_port| clock_port == port))
    {
        bounds.0 = bounds.0.max(
            clock
                .min_rise_transition_ns
                .max(clock.min_fall_transition_ns),
        );
        bounds.1 = bounds
            .1
            .max(clock.rise_transition_ns.max(clock.fall_transition_ns));
    }
    bounds
}

pub(super) fn timing_derate_for_cell(
    nl: &Netlist,
    constraints: Option<&constraints::TimingConstraints>,
    cell_id: usize,
    check: bool,
) -> constraints::TimingDerate {
    let cell = &nl.cell(cell_id).name;
    let global = constraints
        .map(|c| {
            if check {
                c.check_timing_derate
            } else {
                c.timing_derate
            }
        })
        .unwrap_or_default();
    let exact = constraints
        .and_then(|c| {
            if check {
                c.cell_check_derates.get(cell).copied()
            } else {
                c.cell_timing_derates.get(cell).copied()
            }
        })
        .unwrap_or(global);
    constraints
        .and_then(|c| constraints::scoped_cell_timing_derate(nl, c, cell_id, check))
        .unwrap_or(exact)
}

pub(super) fn timing_derate_for_port(
    constraints: Option<&constraints::TimingConstraints>,
    port: &str,
) -> constraints::TimingDerate {
    let global = constraints.map(|c| c.timing_derate).unwrap_or_default();
    constraints
        .and_then(|c| c.port_timing_derates.get(port).copied())
        .unwrap_or(global)
}

pub(crate) fn input_pin_name(nl: &Netlist, cell_id: usize, pin: usize) -> Option<String> {
    let cell = nl.cell(cell_id);
    if pin >= cell.inputs.len() {
        return None;
    }
    match &cell.kind {
        CellKind::Lib(id) => nl
            .library()
            .and_then(|library| library.get(*id).inputs.get(pin).cloned()),
        _ => Some(pin.to_string()),
    }
}

pub(super) fn valid_input_pin_name(nl: &Netlist, cell_id: usize, name: &str) -> bool {
    let cell = nl.cell(cell_id);
    (0..cell.inputs.len())
        .any(|pin| input_pin_name(nl, cell_id, pin).is_some_and(|pin_name| pin_name == name))
}

pub(crate) fn output_pin_name(nl: &Netlist, cell_id: usize) -> Option<String> {
    match &nl.cell(cell_id).kind {
        CellKind::Lib(id) => nl.library().map(|library| library.get(*id).output.clone()),
        CellKind::Dff { .. } | CellKind::DffR { .. } => Some("q".into()),
        _ => Some("y".into()),
    }
}

pub(super) fn valid_timing_pin_name(nl: &Netlist, cell_id: usize, name: &str) -> bool {
    valid_input_pin_name(nl, cell_id, name)
        || output_pin_name(nl, cell_id).is_some_and(|output| output == name)
}

pub(super) fn timing_derate_for_output_pin(
    nl: &Netlist,
    constraints: Option<&constraints::TimingConstraints>,
    cell: &str,
    cell_id: usize,
) -> constraints::TimingDerate {
    let Some(pin_name) = output_pin_name(nl, cell_id) else {
        return timing_derate_for_cell(nl, constraints, cell_id, false);
    };
    if let Some(derate) = constraints
        .and_then(|c| constraints::scoped_pin_timing_derate(nl, c, cell_id, &pin_name, false))
    {
        return derate;
    }
    let cell_derate = timing_derate_for_cell(nl, constraints, cell_id, false);
    constraints
        .and_then(|c| {
            c.pin_timing_derates
                .get(&constraints::TimingPin::new(cell, pin_name))
                .copied()
        })
        .unwrap_or(cell_derate)
}

pub(super) fn timing_derate_for_input_pin(
    nl: &Netlist,
    constraints: Option<&constraints::TimingConstraints>,
    cell: &str,
    cell_id: usize,
    pin: usize,
) -> constraints::TimingDerate {
    let Some(pin_name) = input_pin_name(nl, cell_id, pin) else {
        return timing_derate_for_cell(nl, constraints, cell_id, false);
    };
    if let Some(derate) = constraints
        .and_then(|c| constraints::scoped_pin_timing_derate(nl, c, cell_id, &pin_name, false))
    {
        return derate;
    }
    let input_key = constraints::TimingPin::new(cell, pin_name);
    if let Some(derate) = constraints.and_then(|c| c.pin_timing_derates.get(&input_key).copied()) {
        return derate;
    }
    timing_derate_for_output_pin(nl, constraints, cell, cell_id)
}

/// True when a finite arrival path traced backwards from `net` reaches a
/// sequential launch edge. The direct-driver check used to miss logic between
/// two registers.
pub(crate) fn driven_from_sequential(nl: &Netlist, start: NetId) -> bool {
    let mut stack = vec![start];
    let mut seen = std::collections::HashSet::new();
    while let Some(net) = stack.pop() {
        if !seen.insert(net) {
            continue;
        }
        for &ci in &nl.net(net).drivers {
            let cell = nl.cell(ci);
            if nl.cell_is_sequential(&cell.kind) {
                return true;
            }
            for &input in &cell.inputs {
                stack.push(input);
            }
        }
    }
    false
}

pub(crate) fn seq_clk_to_q(nl: &Netlist, kind: &CellKind) -> Option<f64> {
    match kind {
        CellKind::Lib(id) => nl
            .library()
            .and_then(|l| l.get(*id).seq.as_ref().map(|s| s.clk_to_q)),
        _ => None,
    }
}

pub(crate) fn seq_setup(nl: &Netlist, kind: &CellKind) -> Option<f64> {
    match kind {
        CellKind::Lib(id) => nl
            .library()
            .and_then(|l| l.get(*id).seq.as_ref().map(|s| s.setup)),
        _ => None,
    }
}

/// Setup constraint at a concrete sequential cell and transition pair.
///
/// Indexed Liberty constraints use constrained-pin transition as index 1 and
/// related clock-pin transition as index 2. Missing transitions default to
/// zero, matching the scalar-delay launch model used by unconstrained STA.
pub(crate) fn seq_setup_at(
    nl: &Netlist,
    cell_id: usize,
    data_transition_ns: Option<f64>,
    clock_transition_ns: Option<f64>,
) -> Result<f64, String> {
    let cell = nl.cell(cell_id);
    let fallback = seq_setup(nl, &cell.kind).unwrap_or(0.0);
    let CellKind::Lib(id) = &cell.kind else {
        return Ok(fallback);
    };
    let Some(seq) = nl.library().map(|library| library.get(*id).seq.clone()) else {
        return Ok(fallback);
    };
    let Some(tables) = seq.as_ref().and_then(|seq| seq.setup_tables.as_ref()) else {
        return Ok(fallback);
    };
    let data_transition = data_transition_ns.unwrap_or(0.0);
    let clock_transition = clock_transition_ns.unwrap_or(0.0);
    Ok(tables
        .evaluate_worst(data_transition, clock_transition)?
        .unwrap_or(fallback)
        .max(0.0))
}

pub(crate) fn seq_hold(nl: &Netlist, kind: &CellKind) -> Option<f64> {
    match kind {
        CellKind::Lib(id) => nl
            .library()
            .and_then(|l| l.get(*id).seq.as_ref().map(|s| s.hold)),
        _ => None,
    }
}

/// Hold constraint at a concrete sequential cell and transition pair.
pub(crate) fn seq_hold_at(
    nl: &Netlist,
    cell_id: usize,
    data_transition_ns: Option<f64>,
    clock_transition_ns: Option<f64>,
) -> Result<f64, String> {
    let cell = nl.cell(cell_id);
    let fallback = seq_hold(nl, &cell.kind).unwrap_or(0.0);
    let CellKind::Lib(id) = &cell.kind else {
        return Ok(fallback);
    };
    let Some(seq) = nl.library().map(|library| library.get(*id).seq.clone()) else {
        return Ok(fallback);
    };
    let Some(tables) = seq.as_ref().and_then(|seq| seq.hold_tables.as_ref()) else {
        return Ok(fallback);
    };
    let data_transition = data_transition_ns.unwrap_or(0.0);
    let clock_transition = clock_transition_ns.unwrap_or(0.0);
    Ok(tables
        .evaluate_worst(data_transition, clock_transition)?
        .unwrap_or(fallback)
        .max(0.0))
}

pub(crate) fn seq_min_pulse(nl: &Netlist, kind: &CellKind) -> Option<(f64, f64)> {
    match kind {
        CellKind::Lib(id) => nl.library().and_then(|l| {
            l.get(*id)
                .seq
                .as_ref()
                .map(|s| (s.min_pulse_high, s.min_pulse_low))
        }),
        _ => None,
    }
}

pub(crate) fn seq_reset(nl: &Netlist, kind: &CellKind) -> Option<(usize, f64, f64)> {
    match kind {
        CellKind::Lib(id) => nl.library().and_then(|l| {
            l.get(*id)
                .seq
                .as_ref()
                .and_then(|s| s.reset_pin.map(|pin| (pin, s.recovery, s.removal)))
        }),
        _ => None,
    }
}

/// Recovery constraint at a concrete sequential cell and transition pair.
///
/// Indexed Liberty constraints use the asynchronous reset-pin transition as
/// index 1 and the related clock-pin transition as index 2.
pub(crate) fn seq_recovery_at(
    nl: &Netlist,
    cell_id: usize,
    reset_transition_ns: Option<f64>,
    clock_transition_ns: Option<f64>,
) -> Result<f64, String> {
    let cell = nl.cell(cell_id);
    let fallback = seq_reset(nl, &cell.kind)
        .map(|(_, recovery, _)| recovery)
        .unwrap_or(0.0);
    let CellKind::Lib(id) = &cell.kind else {
        return Ok(fallback);
    };
    let Some(seq) = nl.library().map(|library| library.get(*id).seq.clone()) else {
        return Ok(fallback);
    };
    let Some(tables) = seq.as_ref().and_then(|seq| seq.recovery_tables.as_ref()) else {
        return Ok(fallback);
    };
    let reset_transition = reset_transition_ns.unwrap_or(0.0);
    let clock_transition = clock_transition_ns.unwrap_or(0.0);
    Ok(tables
        .evaluate_worst(reset_transition, clock_transition)?
        .unwrap_or(fallback)
        .max(0.0))
}

/// Removal constraint at a concrete sequential cell and transition pair.
pub(crate) fn seq_removal_at(
    nl: &Netlist,
    cell_id: usize,
    reset_transition_ns: Option<f64>,
    clock_transition_ns: Option<f64>,
) -> Result<f64, String> {
    let cell = nl.cell(cell_id);
    let fallback = seq_reset(nl, &cell.kind)
        .map(|(_, _, removal)| removal)
        .unwrap_or(0.0);
    let CellKind::Lib(id) = &cell.kind else {
        return Ok(fallback);
    };
    let Some(seq) = nl.library().map(|library| library.get(*id).seq.clone()) else {
        return Ok(fallback);
    };
    let Some(tables) = seq.as_ref().and_then(|seq| seq.removal_tables.as_ref()) else {
        return Ok(fallback);
    };
    let reset_transition = reset_transition_ns.unwrap_or(0.0);
    let clock_transition = clock_transition_ns.unwrap_or(0.0);
    Ok(tables
        .evaluate_worst(reset_transition, clock_transition)?
        .unwrap_or(fallback)
        .max(0.0))
}

pub(super) fn output_capacitance_pf(nl: &Netlist, output: NetId) -> f64 {
    let mut capacitance_pf = 0.0;
    for &sink in &nl.net(output).sinks {
        let cell = nl.cell(sink);
        let CellKind::Lib(id) = &cell.kind else {
            continue;
        };
        let Some(library) = nl.library() else {
            continue;
        };
        let definition = library.get(*id);
        let metadata = library.metadata(&definition.name);
        for (position, &input) in cell.inputs.iter().enumerate() {
            if input != output {
                continue;
            }
            if let Some(pin) = definition.inputs.get(position) {
                capacitance_pf += metadata
                    .input_capacitance_pf
                    .get(pin)
                    .copied()
                    .unwrap_or(0.0);
            }
        }
    }
    capacitance_pf
}

pub(super) fn indexed_arc_delay(
    nl: &Netlist,
    ci: usize,
    input_pin: usize,
    input_transition_ns: f64,
) -> Option<f64> {
    let cell = nl.cell(ci);
    let CellKind::Lib(id) = &cell.kind else {
        return None;
    };
    let library = nl.library()?;
    let definition = library.get(*id);
    let tables = definition
        .timing_tables
        .iter()
        .find(|tables| tables.input_pin == input_pin)?;
    let output_cap_pf = output_capacitance_pf(nl, cell.output);
    let rise = tables
        .rise
        .as_ref()
        .and_then(|lut| lut.evaluate(input_transition_ns, output_cap_pf).ok());
    let fall = tables
        .fall
        .as_ref()
        .and_then(|lut| lut.evaluate(input_transition_ns, output_cap_pf).ok());
    Some(rise?.max(fall.unwrap_or(f64::NEG_INFINITY)))
}

pub(super) fn input_arc_transition(
    nl: &Netlist,
    ci: usize,
    input_pin: usize,
    input_transition_ns: f64,
) -> Option<f64> {
    let cell = nl.cell(ci);
    let CellKind::Lib(id) = &cell.kind else {
        return None;
    };
    let library = nl.library()?;
    let definition = library.get(*id);
    let tables = definition
        .timing_tables
        .iter()
        .find(|tables| tables.input_pin == input_pin)?;
    let output_cap_pf = output_capacitance_pf(nl, cell.output);
    let rise = tables
        .rise_transition
        .as_ref()
        .and_then(|lut| lut.evaluate(input_transition_ns, output_cap_pf).ok());
    let fall = tables
        .fall_transition
        .as_ref()
        .and_then(|lut| lut.evaluate(input_transition_ns, output_cap_pf).ok());
    Some(rise?.max(fall.unwrap_or(f64::NEG_INFINITY)))
}

/// Transition produced by one input→output arc.
///
/// Liberty transition tables take precedence. Arcs without a transition table
/// use the same conservative scalar approximation as quality analysis: the
/// incoming transition plus the arc delay.
pub(super) fn output_arc_transition(
    nl: &Netlist,
    ci: usize,
    input_pin: usize,
    derate: f64,
    input_transition_ns: f64,
    arc_delay_ns: f64,
) -> f64 {
    input_arc_transition(nl, ci, input_pin, input_transition_ns)
        .map(|transition| transition * derate)
        .unwrap_or(input_transition_ns + arc_delay_ns)
        .max(0.0)
}

/// Per-cell worst input→output arc delay in ns.
pub(super) fn arc_delay(nl: &Netlist, ci: usize, derate: f64) -> f64 {
    let cell = nl.cell(ci);
    match &cell.kind {
        CellKind::Lib(id) => {
            if let Some(lib) = nl.library() {
                let def = lib.get(*id);
                if def.seq.is_none() {
                    let indexed = def
                        .timing_tables
                        .iter()
                        .filter_map(|tables| indexed_arc_delay(nl, ci, tables.input_pin, 0.0))
                        .fold(f64::NEG_INFINITY, f64::max);
                    return if indexed.is_finite() {
                        indexed
                    } else {
                        def.arcs.iter().map(|(_, d)| *d).fold(0.0f64, f64::max)
                    } * derate;
                }
            }
            0.0
        }
        _ => 0.5 * derate, // nominal generic primitive delay
    }
}

/// Delay for one input→output arc at a specific input transition.
///
/// Generic cells use the same nominal delay as setup analysis.
pub(super) fn input_arc_delay_with_transition(
    nl: &Netlist,
    ci: usize,
    input: usize,
    derate: f64,
    input_transition_ns: f64,
) -> f64 {
    let cell = nl.cell(ci);
    match &cell.kind {
        CellKind::Lib(id) => {
            if let Some(lib) = nl.library() {
                let def = lib.get(*id);
                if def.seq.is_none() {
                    let scalar = def
                        .arcs
                        .iter()
                        .find(|&&(pin, _)| pin == input)
                        .map(|&(_, delay)| delay * derate)
                        .unwrap_or(0.0);
                    return indexed_arc_delay(nl, ci, input, input_transition_ns)
                        .map_or(scalar, |delay| delay * derate);
                }
            }
            0.0
        }
        _ => 0.5 * derate,
    }
}

/// Walk backwards from `net` collecting the cells on the max-arrival path.
pub(super) fn backtrack(nl: &Netlist, arrival: &[f64], mut net: NetId) -> Vec<usize> {
    let mut path = Vec::new();
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > 10_000 {
            break;
        }
        let drivers = nl.net(net).drivers.clone();
        let mut best: Option<(usize, f64, NetId)> = None; // (cell, pin time, pin net)
        for d in drivers {
            let cell = nl.cell(d);
            if nl.cell_is_sequential(&cell.kind) {
                if best.is_none() {
                    best = Some((d, f64::NEG_INFINITY, usize::MAX));
                }
                continue;
            }
            let mut pin_time = f64::NEG_INFINITY;
            let mut pin_net = usize::MAX;
            for &i in &cell.inputs {
                if arrival[i].is_finite() && arrival[i] > pin_time {
                    pin_time = arrival[i];
                    pin_net = i;
                }
            }
            if best.is_none_or(|(_, bt, _)| pin_time > bt) {
                best = Some((d, pin_time, pin_net));
            }
        }
        match best {
            None => break,
            Some((cell, _t, prev)) => {
                path.push(cell);
                if prev == usize::MAX {
                    break; // launched by a flop
                }
                net = prev;
                let is_input = nl
                    .ports
                    .iter()
                    .any(|p| p.dir == eda_core::netlist::Dir::In && p.bits.contains(&net));
                if is_input {
                    break;
                }
            }
        }
    }
    path.reverse();
    path
}
