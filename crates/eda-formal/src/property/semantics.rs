//! Formal property implementation module.

use eda_assert::{Assertion, BoolExpr, Edge, Property};
use eda_core::logic::{LogicValue, LogicWord as BoolTraceWord};
use eda_core::netlist::{CellKind, Dir, NetId, Netlist};
use std::collections::HashMap;

pub(super) fn eval_formal_expr(
    expr: &BoolExpr,
    values: &HashMap<String, u64>,
) -> Result<bool, String> {
    match expr {
        BoolExpr::True => Ok(true),
        BoolExpr::False => Ok(false),
        BoolExpr::Signal(name) => values
            .get(name)
            .map(|value| *value != 0)
            .ok_or_else(|| format!("formal trace has no signal '{}'", name)),
        BoolExpr::Not(inner) => Ok(!eval_formal_expr(inner, values)?),
        BoolExpr::And(a, b) => Ok(eval_formal_expr(a, values)? && eval_formal_expr(b, values)?),
        BoolExpr::Or(a, b) => Ok(eval_formal_expr(a, values)? || eval_formal_expr(b, values)?),
        BoolExpr::Implication(a, b) => {
            Ok(!eval_formal_expr(a, values)? || eval_formal_expr(b, values)?)
        }
    }
}

#[allow(clippy::needless_range_loop)]
pub(super) fn property_violation_in_trace(
    property: &Property,
    disable: Option<&BoolExpr>,
    trace: &[HashMap<String, u64>],
    trigger_frame: Option<usize>,
    repeat_count: Option<usize>,
    end_frame: Option<usize>,
) -> Result<bool, String> {
    match property {
        Property::Immediate(expr) | Property::Always(expr) => {
            for values in trace {
                if disable_is_true(disable, values)? {
                    continue;
                }
                if !eval_formal_expr(expr, values)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Property::Implication {
            antecedent,
            consequence,
            min_cycle,
            max_cycle,
        } => {
            let trigger = trigger_frame
                .ok_or_else(|| "violated temporal property has no trigger frame".to_string())?;
            if !eval_formal_expr(antecedent, &trace[trigger])? {
                return Ok(false);
            }
            if disable_is_true(disable, &trace[trigger])? {
                return Ok(false);
            }
            for offset in *min_cycle..=*max_cycle {
                let frame = trigger + offset;
                if frame >= trace.len() {
                    return Ok(false);
                }
                if disable_is_true(disable, &trace[frame])? {
                    return Ok(false);
                }
                if eval_formal_expr(consequence, &trace[frame])? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Property::RepeatedImplication {
            antecedent,
            min_repeat,
            max_repeat,
            unbounded,
            overlapped,
            consequence,
        } => {
            let Some(trigger) = trigger_frame else {
                return Ok(false);
            };
            let Some(repeat) = repeat_count else {
                return Ok(false);
            };
            if repeat < *min_repeat || (!*unbounded && repeat > *max_repeat) {
                return Ok(false);
            }
            let end = trigger + usize::from(!*overlapped) + repeat.saturating_sub(1);
            if end >= trace.len() {
                return Ok(false);
            }
            for frame in trigger..trigger + repeat {
                if disable_is_true(disable, &trace[frame])?
                    || !eval_formal_expr(antecedent, &trace[frame])?
                {
                    return Ok(false);
                }
            }
            if disable_is_true(disable, &trace[end])? || eval_formal_expr(consequence, &trace[end])?
            {
                return Ok(false);
            }
            Ok(true)
        }
        Property::NonconsecutiveImplication {
            antecedent,
            min_count,
            max_count,
            overlapped,
            consequence,
        } => {
            let (Some(trigger), Some(count), Some(sequence_end)) =
                (trigger_frame, repeat_count, end_frame)
            else {
                return Ok(false);
            };
            if count < *min_count
                || count > *max_count
                || sequence_end < trigger
                || sequence_end >= trace.len()
            {
                return Ok(false);
            }
            let mut hits = 0usize;
            for frame in trigger..=sequence_end {
                if disable_is_true(disable, &trace[frame])? {
                    return Ok(false);
                }
                if eval_formal_expr(antecedent, &trace[frame])? {
                    hits += 1;
                }
            }
            if hits != count {
                return Ok(false);
            }
            let consequence_frame = sequence_end + usize::from(!*overlapped);
            if consequence_frame >= trace.len()
                || disable_is_true(disable, &trace[consequence_frame])?
                || eval_formal_expr(consequence, &trace[consequence_frame])?
            {
                return Ok(false);
            }
            Ok(true)
        }
        Property::SequenceImplication {
            first,
            second,
            min_delay,
            max_delay,
            overlapped,
            consequence,
        } => {
            let (Some(trigger), Some(delay), Some(end)) = (trigger_frame, repeat_count, end_frame)
            else {
                return Ok(false);
            };
            if delay < *min_delay || delay > *max_delay {
                return Ok(false);
            }
            let second_frame = trigger + delay;
            if second_frame >= trace.len()
                || end != second_frame + usize::from(!*overlapped)
                || end >= trace.len()
            {
                return Ok(false);
            }
            if disable_is_true(disable, &trace[trigger])?
                || !eval_formal_expr(first, &trace[trigger])?
                || disable_is_true(disable, &trace[second_frame])?
                || !eval_formal_expr(second, &trace[second_frame])?
                || disable_is_true(disable, &trace[end])?
                || eval_formal_expr(consequence, &trace[end])?
            {
                return Ok(false);
            }
            Ok(true)
        }
    }
}

pub(super) fn disable_is_true(
    disable: Option<&BoolExpr>,
    values: &HashMap<String, u64>,
) -> Result<bool, String> {
    disable
        .map(|expr| eval_formal_expr(expr, values))
        .transpose()
        .map(|disabled| disabled.unwrap_or(false))
}

pub(super) fn pulse_changes(
    frame_count: usize,
    active: &std::collections::BTreeSet<usize>,
) -> Vec<(u64, BoolTraceWord)> {
    let mut changes = Vec::new();
    let mut previous = None;
    for frame in 0..frame_count {
        let current = active.contains(&frame);
        if previous != Some(current) {
            changes.push((frame as u64, BoolTraceWord::from_u64(1, u64::from(current))));
            previous = Some(current);
        }
    }
    changes
}
pub(super) fn validate_assertion(nl: &Netlist, assertion: &Assertion) -> Result<(), String> {
    if let Some(clock) = &assertion.clock {
        let net = nl
            .net_by_name(clock)
            .ok_or_else(|| format!("assertion clock '{}' not found", clock))?;
        let is_scalar_input = nl
            .ports
            .iter()
            .any(|port| port.dir == Dir::In && port.bits.len() == 1 && port.bits[0] == net);
        if !is_scalar_input {
            return Err(format!(
                "assertion clock '{}' must be a scalar input",
                clock
            ));
        }
    }
    let sequential_clocks = sequential_clock_edges(nl)?;
    if sequential_clocks.len() > 1 {
        return Err("multi-clock temporal BMC is not yet supported".into());
    }
    if let Some((design_clock, design_posedge)) = sequential_clocks.first() {
        if let Some(edge) = assertion.edge {
            let assertion_posedge = edge == Edge::Pos;
            if assertion_posedge != *design_posedge {
                return Err(format!(
                    "assertion {} edge does not match design {} edge",
                    if assertion_posedge {
                        "posedge"
                    } else {
                        "negedge"
                    },
                    if *design_posedge {
                        "posedge"
                    } else {
                        "negedge"
                    }
                ));
            }
        }
        if let Some(assertion_clock) = &assertion.clock {
            let expected = nl
                .net_by_name(assertion_clock)
                .ok_or_else(|| format!("assertion clock '{}' not found", assertion_clock))?;
            if expected != *design_clock {
                return Err(format!(
                    "assertion clock '{}' does not drive the design clock '{}'",
                    assertion_clock,
                    nl.net(*design_clock).name
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn sequential_clock_edges(nl: &Netlist) -> Result<Vec<(NetId, bool)>, String> {
    let mut clocks = Vec::new();
    for cell in nl.cells() {
        let (clock_pin, posedge) = match &cell.kind {
            CellKind::Dff { posedge } => (0, *posedge),
            CellKind::DffR { posedge, .. } => (0, *posedge),
            CellKind::Lib(id) => {
                let Some(library) = nl.library() else {
                    return Err(format!("cell '{}' has no attached library", cell.name));
                };
                let Some(seq) = &library.get(*id).seq else {
                    continue;
                };
                (seq.clk_pin, true)
            }
            _ => continue,
        };
        if let Some(&clock) = cell.inputs.get(clock_pin) {
            let effective = effective_clock(nl, clock, posedge);
            if !clocks.contains(&effective) {
                clocks.push(effective);
            }
        }
    }
    Ok(clocks)
}

/// Trace simple clock buffers/inverters back to their root clock.
///
/// Library mapping represents a negedge flop as a posedge flop preceded by
/// an inverter. Recognizing that transform lets temporal BMC preserve the
/// source-clock edge from the mapped gate-level netlist.
pub(super) fn effective_clock(nl: &Netlist, mut clock: NetId, mut posedge: bool) -> (NetId, bool) {
    loop {
        let drivers = nl.net(clock).drivers.clone();
        if drivers.len() != 1 {
            return (clock, posedge);
        }
        let cell = nl.cell(drivers[0]);
        if nl.cell_is_sequential(&cell.kind) || cell.inputs.len() != 1 {
            return (clock, posedge);
        }
        match &cell.kind {
            CellKind::Not => posedge = !posedge,
            CellKind::Buf => {}
            CellKind::Lib(id) => {
                let Some(library) = nl.library() else {
                    return (clock, posedge);
                };
                let definition = library.get(*id);
                if definition.seq.is_some() {
                    return (clock, posedge);
                }
                let input = [LogicValue::One];
                let one = library.eval(*id, &input);
                let input = [LogicValue::Zero];
                let zero = library.eval(*id, &input);
                if one == LogicValue::Zero && zero == LogicValue::One {
                    posedge = !posedge;
                } else if one == LogicValue::One && zero == LogicValue::Zero {
                    // A non-inverting buffer leaves the edge unchanged.
                } else {
                    return (clock, posedge);
                }
            }
            _ => return (clock, posedge),
        }
        clock = cell.inputs[0];
    }
}
