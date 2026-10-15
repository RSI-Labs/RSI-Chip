//! Formal property implementation module.

use crate::bmc::flop_pins;
use eda_assert::{BoolExpr, Property};
use eda_core::logic::{LogicValue, LogicWord as BoolTraceWord};
use eda_core::netlist::{Dir, Netlist};
use eda_waveform::{parse_vcd, VcdSignal};
use std::collections::HashMap;

use super::semantics::{
    disable_is_true, eval_formal_expr, property_violation_in_trace, pulse_changes,
};
use super::types::*;

/// Export a formal input-assignment trace as VCD.
///
/// Each SAT frame becomes one VCD timestamp. Only primary inputs are present
/// because those are the free choices selected by the solver.
pub fn trace_input_vcd(nl: &Netlist, trace: &[HashMap<String, u64>]) -> Result<String, String> {
    if trace.is_empty() {
        return Err("cannot export an empty formal trace".into());
    }
    let input_ports = nl
        .ports
        .iter()
        .filter(|port| port.dir == Dir::In)
        .collect::<Vec<_>>();
    if input_ports.is_empty() {
        return Err("netlist has no primary inputs to export".into());
    }

    let mut signals = Vec::with_capacity(input_ports.len());
    for port in input_ports {
        let mut previous = None;
        let mut changes = Vec::new();
        for (frame, values) in trace.iter().enumerate() {
            let value = values
                .get(&port.name)
                .ok_or_else(|| format!("trace frame {} has no input '{}'", frame, port.name))?;
            if previous != Some(*value) {
                changes.push((
                    frame as u64,
                    BoolTraceWord::from_u64(port.bits.len(), *value),
                ));
                previous = Some(*value);
            }
        }
        signals.push(VcdSignal {
            name: port.name.clone(),
            short_name: port.name.clone(),
            width: port.bits.len(),
            id: String::new(),
            changes,
        });
    }
    eda_waveform::write_vcd(&signals)
}

/// Export a formal trace containing every scalar net in the netlist.
///
/// Unlike [`trace_input_vcd`], this includes internal combinational nets,
/// sequential state outputs, and primary outputs reconstructed from the SAT
/// model.
pub fn trace_net_vcd(nl: &Netlist, trace: &[HashMap<String, u64>]) -> Result<String, String> {
    let signals = net_signals(nl, trace)?;
    eda_waveform::write_vcd(&signals)
}

pub(super) fn net_signals(
    nl: &Netlist,
    trace: &[HashMap<String, u64>],
) -> Result<Vec<VcdSignal>, String> {
    if trace.is_empty() {
        return Err("cannot export an empty formal trace".into());
    }
    let mut names = std::collections::BTreeSet::new();
    let mut signals = Vec::with_capacity(nl.net_count());
    for net_id in 0..nl.net_count() {
        let name = nl.net(net_id).name.clone();
        if !names.insert(name.clone()) {
            return Err(format!("duplicate netlist net name '{}'", name));
        }
        let mut previous = None;
        let mut changes = Vec::new();
        for (frame, values) in trace.iter().enumerate() {
            let value = values
                .get(&name)
                .ok_or_else(|| format!("trace frame {} has no net '{}'", frame, name))?;
            if previous != Some(*value) {
                changes.push((frame as u64, BoolTraceWord::from_u64(1, *value)));
                previous = Some(*value);
            }
        }
        signals.push(VcdSignal {
            name: name.clone(),
            short_name: name.clone(),
            width: 1,
            id: String::new(),
            changes,
        });
    }
    Ok(signals)
}

/// Export a temporal-property counterexample with debug marker signals.
///
/// Markers are not design nets. They identify the SAT-selected antecedent
/// cycle, each missed consequence cycle, and the cycle at which the property
/// is considered violated.
pub fn trace_property_vcd(nl: &Netlist, result: &PropertyBmcResult) -> Result<String, String> {
    if result.status != PropertyBmcStatus::Violated {
        return Err("property markers are only defined for violated assertions".into());
    }
    let trace = result
        .net_trace
        .as_ref()
        .ok_or_else(|| "property result has no counterexample net trace".to_string())?;
    let mut signals = net_signals(nl, trace)?;
    let net_names = signals
        .iter()
        .map(|signal| signal.name.clone())
        .collect::<Vec<_>>();

    let (trigger_frames, miss_frames, violation_frames) = property_markers(
        &result.property,
        result.disable.as_ref(),
        trace,
        result.trigger_frame,
        result.repeat_count,
        result.end_frame,
    )?;

    for (name, active) in [
        ("formal_assertion_trigger", trigger_frames),
        ("formal_assertion_miss", miss_frames),
        ("formal_assertion_violation", violation_frames),
    ] {
        if net_names.iter().any(|net_name| net_name == name) {
            return Err(format!(
                "marker signal '{}' collides with a design net",
                name
            ));
        }
        signals.push(VcdSignal {
            name: name.to_string(),
            short_name: name.to_string(),
            width: 1,
            id: String::new(),
            changes: pulse_changes(trace.len(), &active),
        });
    }
    eda_waveform::write_vcd(&signals)
}

type MarkerFrames = (
    std::collections::BTreeSet<usize>,
    std::collections::BTreeSet<usize>,
    std::collections::BTreeSet<usize>,
);

#[allow(clippy::needless_range_loop)]
pub(super) fn property_markers(
    property: &Property,
    disable: Option<&BoolExpr>,
    trace: &[HashMap<String, u64>],
    trigger_frame: Option<usize>,
    repeat_count: Option<usize>,
    end_frame: Option<usize>,
) -> Result<MarkerFrames, String> {
    let mut trigger_frames = std::collections::BTreeSet::new();
    let mut miss_frames = std::collections::BTreeSet::new();
    let mut violation_frames = std::collections::BTreeSet::new();
    match property {
        Property::Immediate(expr) | Property::Always(expr) => {
            for (frame, values) in trace.iter().enumerate() {
                if disable_is_true(disable, values)? {
                    continue;
                }
                if !eval_formal_expr(expr, values)? {
                    trigger_frames.insert(frame);
                    violation_frames.insert(frame);
                }
            }
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
                return Err("temporal trigger frame does not satisfy antecedent".into());
            }
            if disable_is_true(disable, &trace[trigger])? {
                return Err("temporal trigger frame is disabled".into());
            }
            trigger_frames.insert(trigger);
            violation_frames.insert(trigger);
            for offset in *min_cycle..=*max_cycle {
                let frame = trigger + offset;
                if frame >= trace.len() {
                    return Err(format!(
                        "temporal consequence frame {} is outside trace length {}",
                        frame,
                        trace.len()
                    ));
                }
                if !eval_formal_expr(consequence, &trace[frame])? {
                    if disable_is_true(disable, &trace[frame])? {
                        return Err("temporal consequence miss is disabled".into());
                    }
                    miss_frames.insert(frame);
                }
            }
            if miss_frames.len() != max_cycle - min_cycle + 1 {
                return Err("temporal counterexample does not miss the full window".into());
            }
        }
        Property::RepeatedImplication {
            antecedent,
            min_repeat,
            max_repeat,
            unbounded,
            overlapped,
            consequence,
        } => {
            let trigger = trigger_frame
                .ok_or_else(|| "violated repeated property has no trigger frame".to_string())?;
            let repeat = repeat_count.ok_or_else(|| {
                "violated repeated property has no selected repetition count".to_string()
            })?;
            if repeat < *min_repeat || (!*unbounded && repeat > *max_repeat) {
                return Err("repeated property repetition count is outside its range".into());
            }
            let end = trigger + usize::from(!*overlapped) + repeat.saturating_sub(1);
            if end >= trace.len() {
                return Err(format!(
                    "repeated consequence frame {} is outside trace length {}",
                    end,
                    trace.len()
                ));
            }
            for frame in trigger..trigger + repeat {
                if !eval_formal_expr(antecedent, &trace[frame])? {
                    return Err("repeated antecedent frame does not satisfy expression".into());
                }
                if disable_is_true(disable, &trace[frame])? {
                    return Err("repeated antecedent frame is disabled".into());
                }
            }
            if disable_is_true(disable, &trace[end])? {
                return Err("repeated consequence frame is disabled".into());
            }
            if eval_formal_expr(consequence, &trace[end])? {
                return Err("repeated counterexample consequence unexpectedly holds".into());
            }
            trigger_frames.insert(trigger);
            violation_frames.insert(trigger);
            miss_frames.insert(end);
        }
        Property::NonconsecutiveImplication {
            antecedent,
            min_count,
            max_count,
            overlapped,
            consequence,
        } => {
            let trigger = trigger_frame.ok_or_else(|| {
                "violated nonconsecutive property has no trigger frame".to_string()
            })?;
            let count = repeat_count.ok_or_else(|| {
                "violated nonconsecutive property has no selected count".to_string()
            })?;
            let sequence_end = end_frame.ok_or_else(|| {
                "violated nonconsecutive property has no selected endpoint".to_string()
            })?;
            if count < *min_count || count > *max_count || sequence_end < trigger {
                return Err("nonconsecutive counterexample has an invalid range".into());
            }
            let mut hits = 0usize;
            for frame in trigger..=sequence_end {
                if disable_is_true(disable, &trace[frame])? {
                    return Err("nonconsecutive sequence frame is disabled".into());
                }
                if eval_formal_expr(antecedent, &trace[frame])? {
                    hits += 1;
                }
            }
            if hits != count {
                return Err(format!(
                    "nonconsecutive counterexample has {} hits, expected {}",
                    hits, count
                ));
            }
            let consequence_frame = sequence_end + usize::from(!*overlapped);
            if consequence_frame >= trace.len() {
                return Err("nonconsecutive consequence frame is outside trace".into());
            }
            if disable_is_true(disable, &trace[consequence_frame])? {
                return Err("nonconsecutive consequence frame is disabled".into());
            }
            if eval_formal_expr(consequence, &trace[consequence_frame])? {
                return Err("nonconsecutive consequence unexpectedly holds".into());
            }
            trigger_frames.insert(trigger);
            violation_frames.insert(trigger);
            miss_frames.insert(consequence_frame);
        }
        Property::SequenceImplication {
            first,
            second,
            min_delay,
            max_delay,
            overlapped,
            consequence,
        } => {
            let trigger = trigger_frame
                .ok_or_else(|| "violated sequence property has no trigger frame".to_string())?;
            let delay = repeat_count
                .ok_or_else(|| "violated sequence property has no selected delay".to_string())?;
            let end = end_frame
                .ok_or_else(|| "violated sequence property has no endpoint".to_string())?;
            if delay < *min_delay || delay > *max_delay {
                return Err("sequence property delay is outside its range".into());
            }
            let second_frame = trigger + delay;
            if second_frame >= trace.len() || end != second_frame + usize::from(!*overlapped) {
                return Err("sequence counterexample endpoint is inconsistent".into());
            }
            for (frame, expression) in [(trigger, first), (second_frame, second)] {
                if disable_is_true(disable, &trace[frame])? {
                    return Err("sequence antecedent frame is disabled".into());
                }
                if !eval_formal_expr(expression, &trace[frame])? {
                    return Err("sequence antecedent frame does not hold".into());
                }
            }
            if disable_is_true(disable, &trace[end])? {
                return Err("sequence consequence frame is disabled".into());
            }
            if eval_formal_expr(consequence, &trace[end])? {
                return Err("sequence counterexample consequence unexpectedly holds".into());
            }
            trigger_frames.insert(trigger);
            violation_frames.insert(trigger);
            miss_frames.insert(end);
        }
    }
    Ok((trigger_frames, miss_frames, violation_frames))
}

/// Independently replay a SAT property counterexample at netlist level.
///
/// This checks every combinational cell in every frame and every flip-flop
/// transition between frames, then re-evaluates the original property from
/// reconstructed net values. It is intentionally independent of the SAT model
/// that produced the trace.
pub fn verify_property_trace(
    nl: &Netlist,
    result: &PropertyBmcResult,
) -> Result<PropertyReplayResult, String> {
    if result.status != PropertyBmcStatus::Violated {
        return Err("only a violated property has a counterexample to replay".into());
    }
    let named_trace = result
        .net_trace
        .as_ref()
        .ok_or_else(|| "property result has no net trace".to_string())?;
    if named_trace.is_empty() {
        return Err("property counterexample has no frames".into());
    }

    let mut frames = Vec::with_capacity(named_trace.len());
    for (frame_index, named_values) in named_trace.iter().enumerate() {
        let mut values = HashMap::with_capacity(nl.net_count());
        for net_id in 0..nl.net_count() {
            let value = named_values.get(&nl.net(net_id).name).ok_or_else(|| {
                format!(
                    "trace frame {} has no net '{}'",
                    frame_index,
                    nl.net(net_id).name
                )
            })?;
            values.insert(
                net_id,
                if *value == 0 {
                    LogicValue::Zero
                } else {
                    LogicValue::One
                },
            );
        }
        frames.push(values);
    }

    let mut replay = PropertyReplayResult {
        frames_verified: frames.len(),
        ..Default::default()
    };
    for (frame_index, values) in frames.iter().enumerate() {
        for cell_id in 0..nl.cell_count() {
            let cell = nl.cell(cell_id);
            if nl.cell_is_sequential(&cell.kind) {
                continue;
            }
            let expected = nl.eval_cell(cell_id, values);
            let actual = values[&cell.output];
            if expected != actual {
                return Err(format!(
                    "frame {}: cell '{}' expected {} but trace has {}",
                    frame_index,
                    cell.name,
                    expected.to_char(),
                    actual.to_char()
                ));
            }
            replay.combinational_cells_checked += 1;
        }
    }

    let flops = flop_pins(nl)?;
    for frame_index in 1..frames.len() {
        let previous = &frames[frame_index - 1];
        let current = &frames[frame_index];
        for flop in &flops {
            let q_now = current[&flop.q];
            let d_previous = previous[&flop.d];
            let reset_active_previous = flop.reset.map(|(reset, reset_high)| {
                let value = previous[&reset];
                if reset_high {
                    value == LogicValue::One
                } else {
                    value == LogicValue::Zero
                }
            });
            let expected = if reset_active_previous == Some(true) {
                LogicValue::Zero
            } else {
                d_previous
            };
            if q_now != expected {
                return Err(format!(
                    "frame {}: sequential transition expects q={} but trace has {}",
                    frame_index,
                    expected.to_char(),
                    q_now.to_char()
                ));
            }
            if let Some((reset_now, reset_high)) = flop.reset {
                let active = if reset_high {
                    current[&reset_now] == LogicValue::One
                } else {
                    current[&reset_now] == LogicValue::Zero
                };
                if active && q_now != LogicValue::Zero {
                    return Err(format!(
                        "frame {}: asynchronous reset does not dominate q",
                        frame_index
                    ));
                }
            }
            replay.transitions_verified += 1;
        }
    }

    replay.property_violation_reproduced = property_violation_in_trace(
        &result.property,
        result.disable.as_ref(),
        named_trace,
        result.trigger_frame,
        result.repeat_count,
        result.end_frame,
    )?;
    if !replay.property_violation_reproduced {
        return Err("trace does not reproduce the property violation".into());
    }
    Ok(replay)
}

/// Parse and independently replay an exported formal property VCD.
///
/// The design nets are reconstructed from VCD value changes, marker pulses
/// are checked against the original property, and the resulting trace is run
/// through the same combinational / sequential replay verifier as the internal
/// SAT trace.
pub fn replay_property_vcd(
    nl: &Netlist,
    result: &PropertyBmcResult,
    source: &str,
) -> Result<PropertyReplayResult, String> {
    if result.status != PropertyBmcStatus::Violated {
        return Err("only a violated property has a VCD counterexample to replay".into());
    }
    if result.frames_checked == 0 {
        return Err("property counterexample has no frames".into());
    }
    let waveform = parse_vcd(source)?;
    let frame_count = result.frames_checked;
    let mut replay_trace = Vec::with_capacity(frame_count);
    for frame in 0..frame_count {
        let mut values = HashMap::with_capacity(nl.net_count());
        for net_id in 0..nl.net_count() {
            let name = &nl.net(net_id).name;
            let signal = waveform
                .signal(name)
                .ok_or_else(|| format!("counterexample VCD has no net '{}'", name))?;
            if signal.width != 1 {
                return Err(format!("counterexample VCD net '{}' must be scalar", name));
            }
            let value = signal
                .value_at(frame as u64)
                .and_then(|value| value.to_u64())
                .ok_or_else(|| {
                    format!(
                        "counterexample VCD net '{}' is unknown at frame {}",
                        name, frame
                    )
                })?;
            values.insert(name.clone(), value);
        }
        replay_trace.push(values);
    }

    let (expected_trigger, expected_miss, expected_violation) = property_markers(
        &result.property,
        result.disable.as_ref(),
        &replay_trace,
        result.trigger_frame,
        result.repeat_count,
        result.end_frame,
    )?;
    for (marker_name, expected) in [
        ("formal_assertion_trigger", &expected_trigger),
        ("formal_assertion_miss", &expected_miss),
        ("formal_assertion_violation", &expected_violation),
    ] {
        let signal = waveform
            .signal(marker_name)
            .ok_or_else(|| format!("counterexample VCD has no marker '{}'", marker_name))?;
        if signal.width != 1 {
            return Err(format!(
                "counterexample marker '{}' must be scalar",
                marker_name
            ));
        }
        for frame in 0..frame_count {
            let actual = signal
                .value_at(frame as u64)
                .and_then(|value| value.to_u64())
                .ok_or_else(|| {
                    format!(
                        "counterexample marker '{}' is unknown at frame {}",
                        marker_name, frame
                    )
                })?;
            let expected_value = u64::from(expected.contains(&frame));
            if actual != expected_value {
                return Err(format!(
                    "marker '{}' is {} at frame {}; expected {}",
                    marker_name, actual, frame, expected_value
                ));
            }
        }
    }

    let replayed_result = PropertyBmcResult {
        net_trace: Some(replay_trace),
        trigger_frame: expected_trigger.iter().next().copied(),
        ..result.clone()
    };
    verify_property_trace(nl, &replayed_result)
}
