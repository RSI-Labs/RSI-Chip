//! Bounded checking of the temporal assertion and cover-property subset shared
//! with `eda-assert`.
//!
//! Unlike dynamic VCD checking, these queries use the same sequential SAT
//! transition relation as BMC. A closed window with no counterexample is
//! reported as `BoundedPass`, not as a proof for unbounded time.

use crate::bmc::{
    encode_frame, flop_pins, net_trace, shift_lit, target_lit, BmcOptions, BmcTarget,
};
use crate::sat::{solve_cancellable, SatResult};
use eda_assert::{Assertion, AssertionKind, BoolExpr, Edge, Property};
use eda_core::cancellation::FlowCancellation;
use eda_core::logic::{LogicValue, LogicWord as BoolTraceWord};
use eda_core::netlist::{CellKind, Dir, NetId, Netlist};
use eda_waveform::{parse_vcd, VcdSignal};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyBmcStatus {
    /// No counterexample exists in the bounded horizon.
    BoundedPass,
    /// A complete violating temporal window was found.
    Violated,
    /// A cover property was satisfied within the bounded horizon.
    Satisfied,
    /// A cover property had no complete satisfying witness in the horizon.
    NotSatisfied,
    /// The SVA assumptions are inconsistent, so the target result is vacuous.
    AssumptionsUnsat,
}

#[derive(Clone, Debug)]
pub struct PropertyBmcResult {
    pub assertion: String,
    pub kind: AssertionKind,
    pub property: Property,
    pub disable: Option<BoolExpr>,
    pub clock: Option<String>,
    pub status: PropertyBmcStatus,
    /// Per-frame primary-input assignments.
    pub trace: Option<Vec<HashMap<String, u64>>>,
    /// Per-frame values for every scalar net, including state and outputs.
    pub net_trace: Option<Vec<HashMap<String, u64>>>,
    /// Number of antecedent cycles considered.
    pub depth_checked: usize,
    /// Total SAT frames, including consequence lookahead.
    pub frames_checked: usize,
    /// Frame containing a true antecedent whose window failed.
    pub trigger_frame: Option<usize>,
    /// Number of consecutive antecedent samples selected for a repeated
    /// antecedent witness or counterexample.
    pub repeat_count: Option<usize>,
    /// Consequence frame selected for a repeated or nonconsecutive witness.
    pub end_frame: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PropertyReplayResult {
    pub frames_verified: usize,
    pub combinational_cells_checked: usize,
    pub transitions_verified: usize,
    pub property_violation_reproduced: bool,
}

impl PropertyBmcResult {
    pub fn violated(&self) -> bool {
        self.status == PropertyBmcStatus::Violated
    }

    pub fn covered(&self) -> bool {
        self.status == PropertyBmcStatus::Satisfied
    }
}

pub fn bmc_assertions(
    nl: &Netlist,
    assertions: &[Assertion],
    opts: &BmcOptions,
) -> Result<Vec<PropertyBmcResult>, String> {
    bmc_assertions_cancellable(nl, assertions, opts, &FlowCancellation::new())
}

pub fn bmc_assertions_cancellable(
    nl: &Netlist,
    assertions: &[Assertion],
    opts: &BmcOptions,
    cancellation: &FlowCancellation,
) -> Result<Vec<PropertyBmcResult>, String> {
    if cancellation.is_cancelled() {
        return Err("temporal property BMC cancelled".into());
    }
    assertions
        .iter()
        .map(|assertion| {
            if cancellation.is_cancelled() {
                return Err("temporal property BMC cancelled".into());
            }
            bmc_assertion_cancellable(nl, assertion, opts, cancellation)
        })
        .collect()
}

pub fn bmc_assertions_with_assumptions(
    nl: &Netlist,
    assertions: &[Assertion],
    assumptions: &[Assertion],
    opts: &BmcOptions,
) -> Result<Vec<PropertyBmcResult>, String> {
    bmc_assertions_with_assumptions_cancellable(
        nl,
        assertions,
        assumptions,
        opts,
        &FlowCancellation::new(),
    )
}

pub fn bmc_assertions_with_assumptions_cancellable(
    nl: &Netlist,
    assertions: &[Assertion],
    assumptions: &[Assertion],
    opts: &BmcOptions,
    cancellation: &FlowCancellation,
) -> Result<Vec<PropertyBmcResult>, String> {
    if cancellation.is_cancelled() {
        return Err("temporal property BMC cancelled".into());
    }
    if assertions.is_empty() {
        return Err("at least one assert or cover property is required".into());
    }
    if assertions
        .iter()
        .any(|property| property.kind == AssertionKind::Assume)
    {
        return Err("assumptions must be passed in the assumptions slice".into());
    }
    assertions
        .iter()
        .map(|assertion| {
            if cancellation.is_cancelled() {
                return Err("temporal property BMC cancelled".into());
            }
            bmc_assertion_with_assumptions(nl, assertion, assumptions, opts, cancellation)
        })
        .collect()
}

pub fn bmc_assertion(
    nl: &Netlist,
    assertion: &Assertion,
    opts: &BmcOptions,
) -> Result<PropertyBmcResult, String> {
    bmc_assertion_cancellable(nl, assertion, opts, &FlowCancellation::new())
}

pub fn bmc_assertion_cancellable(
    nl: &Netlist,
    assertion: &Assertion,
    opts: &BmcOptions,
    cancellation: &FlowCancellation,
) -> Result<PropertyBmcResult, String> {
    bmc_assertion_with_assumptions(nl, assertion, &[], opts, cancellation)
}

#[allow(clippy::needless_range_loop)]
fn bmc_assertion_with_assumptions(
    nl: &Netlist,
    assertion: &Assertion,
    assumptions: &[Assertion],
    opts: &BmcOptions,
    cancellation: &FlowCancellation,
) -> Result<PropertyBmcResult, String> {
    if cancellation.is_cancelled() {
        return Err("temporal property BMC cancelled".into());
    }
    validate_assertion(nl, assertion)?;
    for assumption in assumptions {
        if assumption.kind != AssertionKind::Assume {
            return Err(format!(
                "property '{}' cannot be used as a formal assumption",
                assumption.name
            ));
        }
        validate_assertion(nl, assumption)?;
    }
    let depth = opts.depth.max(1);
    let lookahead = match &assertion.property {
        Property::Implication { max_cycle, .. } => *max_cycle,
        Property::RepeatedImplication {
            max_repeat,
            unbounded,
            overlapped,
            ..
        } => {
            if *unbounded {
                depth + usize::from(!*overlapped)
            } else {
                max_repeat.saturating_sub(1) + usize::from(!*overlapped)
            }
        }
        Property::NonconsecutiveImplication {
            max_count,
            overlapped,
            ..
        } => *max_count + usize::from(!*overlapped),
        Property::SequenceImplication {
            max_delay,
            overlapped,
            ..
        } => *max_delay + usize::from(!*overlapped),
        Property::Immediate(_) | Property::Always(_) => 0,
    };
    let assumption_lookahead = assumptions
        .iter()
        .map(|assumption| match &assumption.property {
            Property::Implication { max_cycle, .. } => *max_cycle,
            Property::RepeatedImplication {
                max_repeat,
                unbounded,
                overlapped,
                ..
            } => {
                if *unbounded {
                    depth + usize::from(!*overlapped)
                } else {
                    max_repeat.saturating_sub(1) + usize::from(!*overlapped)
                }
            }
            Property::NonconsecutiveImplication {
                max_count,
                overlapped,
                ..
            } => *max_count + usize::from(!*overlapped),
            Property::SequenceImplication {
                max_delay,
                overlapped,
                ..
            } => *max_delay + usize::from(!*overlapped),
            Property::Immediate(_) | Property::Always(_) => 0,
        })
        .max()
        .unwrap_or(0);
    let total_depth = depth + lookahead + assumption_lookahead;
    let (mut cnf, frames) = unroll(nl, opts, total_depth, cancellation)?;

    encode_assumptions(nl, &mut cnf, &frames, assumptions, depth, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("temporal property BMC cancelled".into());
    }
    let assumptions_sat = !assumptions.is_empty()
        && matches!(solve_cancellable(&cnf, cancellation)?, SatResult::Sat(_));
    if !assumptions.is_empty() && !assumptions_sat {
        return Ok(PropertyBmcResult {
            assertion: assertion.name.clone(),
            kind: assertion.kind,
            property: assertion.property.clone(),
            disable: assertion.disable.clone(),
            clock: assertion.clock.clone(),
            status: PropertyBmcStatus::AssumptionsUnsat,
            trace: None,
            net_trace: None,
            depth_checked: depth,
            frames_checked: frames.len(),
            trigger_frame: None,
            repeat_count: None,
            end_frame: None,
        });
    }

    let cover = assertion.kind == AssertionKind::Cover;
    let mut query_lits = Vec::new();
    let mut temporal_windows = Vec::new();
    let mut repeat_choices = Vec::new();
    match (&assertion.property, cover) {
        (Property::Immediate(expr) | Property::Always(expr), false) => {
            for frame in &frames {
                let property = target_lit(nl, &mut cnf, frame, &BmcTarget::Expr(expr.clone()))?;
                let mut violation_terms = vec![-property];
                if let Some(disable) = &assertion.disable {
                    let enabled = target_lit(
                        nl,
                        &mut cnf,
                        frame,
                        &BmcTarget::Expr(BoolExpr::Not(Box::new(disable.clone()))),
                    )?;
                    violation_terms.push(enabled);
                }
                query_lits.push(and_lits(&mut cnf, &violation_terms));
            }
        }
        (Property::Immediate(expr) | Property::Always(expr), true) => {
            for (frame_index, frame) in frames.iter().enumerate() {
                let property = target_lit(nl, &mut cnf, frame, &BmcTarget::Expr(expr.clone()))?;
                let mut witness_terms = vec![property];
                if let Some(disable) = &assertion.disable {
                    let enabled = target_lit(
                        nl,
                        &mut cnf,
                        frame,
                        &BmcTarget::Expr(BoolExpr::Not(Box::new(disable.clone()))),
                    )?;
                    witness_terms.push(enabled);
                }
                let witness = and_lits(&mut cnf, &witness_terms);
                query_lits.push(witness);
                temporal_windows.push((frame_index, witness));
            }
        }
        (
            Property::Implication {
                antecedent,
                consequence,
                min_cycle,
                max_cycle,
            },
            false,
        ) => {
            if min_cycle > max_cycle {
                return Err("temporal property delay min is greater than max".into());
            }
            for start in 0..=depth {
                let antecedent = target_lit(
                    nl,
                    &mut cnf,
                    &frames[start],
                    &BmcTarget::Expr(antecedent.clone()),
                )?;
                let mut all_false = vec![antecedent];
                if let Some(disable) = &assertion.disable {
                    let enabled = target_lit(
                        nl,
                        &mut cnf,
                        &frames[start],
                        &BmcTarget::Expr(BoolExpr::Not(Box::new(disable.clone()))),
                    )?;
                    all_false.push(enabled);
                }
                for offset in *min_cycle..=*max_cycle {
                    let missing = target_lit(
                        nl,
                        &mut cnf,
                        &frames[start + offset],
                        &BmcTarget::Expr(BoolExpr::Not(Box::new(consequence.clone()))),
                    )?;
                    all_false.push(missing);
                    if let Some(disable) = &assertion.disable {
                        let enabled = target_lit(
                            nl,
                            &mut cnf,
                            &frames[start + offset],
                            &BmcTarget::Expr(BoolExpr::Not(Box::new(disable.clone()))),
                        )?;
                        all_false.push(enabled);
                    }
                }
                let violation = and_lits(&mut cnf, &all_false);
                temporal_windows.push((start, violation));
                query_lits.push(violation);
            }
        }
        (
            Property::Implication {
                antecedent,
                consequence,
                min_cycle,
                max_cycle,
            },
            true,
        ) => {
            if min_cycle > max_cycle {
                return Err("temporal property delay min is greater than max".into());
            }
            for start in 0..=depth {
                let antecedent_lit = target_lit(
                    nl,
                    &mut cnf,
                    &frames[start],
                    &BmcTarget::Expr(antecedent.clone()),
                )?;
                let mut any_match_lits = Vec::new();
                for offset in *min_cycle..=*max_cycle {
                    let mut witness_terms = vec![antecedent_lit];
                    if let Some(disable) = &assertion.disable {
                        let start_enabled = target_lit(
                            nl,
                            &mut cnf,
                            &frames[start],
                            &BmcTarget::Expr(BoolExpr::Not(Box::new(disable.clone()))),
                        )?;
                        witness_terms.push(start_enabled);
                    }
                    let consequence_lit = target_lit(
                        nl,
                        &mut cnf,
                        &frames[start + offset],
                        &BmcTarget::Expr(consequence.clone()),
                    )?;
                    witness_terms.push(consequence_lit);
                    if let Some(disable) = &assertion.disable {
                        let consequence_enabled = target_lit(
                            nl,
                            &mut cnf,
                            &frames[start + offset],
                            &BmcTarget::Expr(BoolExpr::Not(Box::new(disable.clone()))),
                        )?;
                        witness_terms.push(consequence_enabled);
                    }
                    any_match_lits.push(and_lits(&mut cnf, &witness_terms));
                }
                // A temporal cover is hit when at least one consequence in
                // the bounded window matches its enabled antecedent.
                let witness = or_lits(&mut cnf, &any_match_lits);
                temporal_windows.push((start, witness));
                query_lits.push(witness);
            }
        }
        (
            Property::RepeatedImplication {
                antecedent,
                min_repeat,
                max_repeat,
                unbounded,
                overlapped,
                consequence,
            },
            false,
        ) => {
            if !*unbounded && (*min_repeat == 0 || *min_repeat > *max_repeat) {
                return Err("invalid consecutive repetition range".into());
            }
            for start in 0..=depth {
                let repeat_max = if *unbounded {
                    depth - start + 1
                } else {
                    *max_repeat
                };
                for repeat in *min_repeat..=repeat_max {
                    let end = start + usize::from(!*overlapped) + repeat.saturating_sub(1);
                    let mut violation_terms = Vec::new();
                    for offset in 0..repeat {
                        violation_terms.push(target_lit(
                            nl,
                            &mut cnf,
                            &frames[start + offset],
                            &BmcTarget::Expr(antecedent.clone()),
                        )?);
                    }
                    if let Some(disable) = &assertion.disable {
                        for frame in &frames[start..=end] {
                            violation_terms.push(target_lit(
                                nl,
                                &mut cnf,
                                frame,
                                &BmcTarget::Expr(BoolExpr::Not(Box::new(disable.clone()))),
                            )?);
                        }
                    }
                    violation_terms.push(-target_lit(
                        nl,
                        &mut cnf,
                        &frames[end],
                        &BmcTarget::Expr(consequence.clone()),
                    )?);
                    let violation = and_lits(&mut cnf, &violation_terms);
                    temporal_windows.push((start, violation));
                    repeat_choices.push((start, end, repeat, violation));
                    query_lits.push(violation);
                }
            }
        }
        (
            Property::RepeatedImplication {
                antecedent,
                min_repeat,
                max_repeat,
                unbounded,
                overlapped,
                consequence,
            },
            true,
        ) => {
            if !*unbounded && (*min_repeat == 0 || *min_repeat > *max_repeat) {
                return Err("invalid consecutive repetition range".into());
            }
            for start in 0..=depth {
                let repeat_max = if *unbounded {
                    depth - start + 1
                } else {
                    *max_repeat
                };
                for repeat in *min_repeat..=repeat_max {
                    let end = start + usize::from(!*overlapped) + repeat.saturating_sub(1);
                    let mut witness_terms = Vec::new();
                    for offset in 0..repeat {
                        witness_terms.push(target_lit(
                            nl,
                            &mut cnf,
                            &frames[start + offset],
                            &BmcTarget::Expr(antecedent.clone()),
                        )?);
                    }
                    if let Some(disable) = &assertion.disable {
                        for frame in &frames[start..=end] {
                            witness_terms.push(target_lit(
                                nl,
                                &mut cnf,
                                frame,
                                &BmcTarget::Expr(BoolExpr::Not(Box::new(disable.clone()))),
                            )?);
                        }
                    }
                    witness_terms.push(target_lit(
                        nl,
                        &mut cnf,
                        &frames[end],
                        &BmcTarget::Expr(consequence.clone()),
                    )?);
                    let witness = and_lits(&mut cnf, &witness_terms);
                    temporal_windows.push((start, witness));
                    repeat_choices.push((start, end, repeat, witness));
                    query_lits.push(witness);
                }
            }
        }
        (
            Property::NonconsecutiveImplication {
                antecedent,
                min_count,
                max_count,
                overlapped,
                consequence,
            },
            false,
        ) => {
            if *min_count == 0 || min_count > max_count {
                return Err("invalid nonconsecutive repetition range".into());
            }
            let last_frame = depth + max_count + usize::from(!*overlapped);
            for start in 0..=depth {
                for count in *min_count..=*max_count {
                    for end in start + count - 1..=last_frame {
                        let mut ant_lits = Vec::with_capacity(end - start + 1);
                        for frame in start..=end {
                            ant_lits.push(target_lit(
                                nl,
                                &mut cnf,
                                &frames[frame],
                                &BmcTarget::Expr(antecedent.clone()),
                            )?);
                        }
                        let pattern = exactly_true_lits(&mut cnf, &ant_lits, count)?;
                        let mut violation_terms = vec![
                            pattern,
                            -target_lit(
                                nl,
                                &mut cnf,
                                &frames[end + usize::from(!*overlapped)],
                                &BmcTarget::Expr(consequence.clone()),
                            )?,
                        ];
                        if let Some(disable) = &assertion.disable {
                            for frame in start..=end {
                                violation_terms.push(target_lit(
                                    nl,
                                    &mut cnf,
                                    &frames[frame],
                                    &BmcTarget::Expr(BoolExpr::Not(Box::new(disable.clone()))),
                                )?);
                            }
                        }
                        let violation = and_lits(&mut cnf, &violation_terms);
                        temporal_windows.push((start, violation));
                        repeat_choices.push((start, end, count, violation));
                        query_lits.push(violation);
                    }
                }
            }
        }
        (
            Property::NonconsecutiveImplication {
                antecedent,
                min_count,
                max_count,
                overlapped,
                consequence,
            },
            true,
        ) => {
            if *min_count == 0 || min_count > max_count {
                return Err("invalid nonconsecutive repetition range".into());
            }
            let last_frame = depth + max_count + usize::from(!*overlapped);
            for start in 0..=depth {
                for count in *min_count..=*max_count {
                    for end in start + count - 1..=last_frame {
                        let mut ant_lits = Vec::with_capacity(end - start + 1);
                        for frame in start..=end {
                            ant_lits.push(target_lit(
                                nl,
                                &mut cnf,
                                &frames[frame],
                                &BmcTarget::Expr(antecedent.clone()),
                            )?);
                        }
                        let pattern = exactly_true_lits(&mut cnf, &ant_lits, count)?;
                        let mut witness_terms = vec![
                            pattern,
                            target_lit(
                                nl,
                                &mut cnf,
                                &frames[end + usize::from(!*overlapped)],
                                &BmcTarget::Expr(consequence.clone()),
                            )?,
                        ];
                        if let Some(disable) = &assertion.disable {
                            for frame in start..=end {
                                witness_terms.push(target_lit(
                                    nl,
                                    &mut cnf,
                                    &frames[frame],
                                    &BmcTarget::Expr(BoolExpr::Not(Box::new(disable.clone()))),
                                )?);
                            }
                        }
                        let witness = and_lits(&mut cnf, &witness_terms);
                        temporal_windows.push((start, witness));
                        repeat_choices.push((start, end, count, witness));
                        query_lits.push(witness);
                    }
                }
            }
        }
        (
            Property::SequenceImplication {
                first,
                second,
                min_delay,
                max_delay,
                overlapped,
                consequence,
            },
            false,
        ) => {
            if min_delay > max_delay {
                return Err("sequence delay min is greater than max".into());
            }
            for start in 0..=depth {
                for delay in *min_delay..=*max_delay {
                    let second_frame = start + delay;
                    let end = second_frame + usize::from(!*overlapped);
                    let mut violation_terms = vec![
                        target_lit(
                            nl,
                            &mut cnf,
                            &frames[start],
                            &BmcTarget::Expr(first.clone()),
                        )?,
                        target_lit(
                            nl,
                            &mut cnf,
                            &frames[second_frame],
                            &BmcTarget::Expr(second.clone()),
                        )?,
                        -target_lit(
                            nl,
                            &mut cnf,
                            &frames[end],
                            &BmcTarget::Expr(consequence.clone()),
                        )?,
                    ];
                    if let Some(disable) = &assertion.disable {
                        for frame in [start, second_frame, end] {
                            violation_terms.push(target_lit(
                                nl,
                                &mut cnf,
                                &frames[frame],
                                &BmcTarget::Expr(BoolExpr::Not(Box::new(disable.clone()))),
                            )?);
                        }
                    }
                    let violation = and_lits(&mut cnf, &violation_terms);
                    temporal_windows.push((start, violation));
                    repeat_choices.push((start, end, delay, violation));
                    query_lits.push(violation);
                }
            }
        }
        (
            Property::SequenceImplication {
                first,
                second,
                min_delay,
                max_delay,
                overlapped,
                consequence,
            },
            true,
        ) => {
            if min_delay > max_delay {
                return Err("sequence delay min is greater than max".into());
            }
            for start in 0..=depth {
                for delay in *min_delay..=*max_delay {
                    let second_frame = start + delay;
                    let end = second_frame + usize::from(!*overlapped);
                    let mut witness_terms = vec![
                        target_lit(
                            nl,
                            &mut cnf,
                            &frames[start],
                            &BmcTarget::Expr(first.clone()),
                        )?,
                        target_lit(
                            nl,
                            &mut cnf,
                            &frames[second_frame],
                            &BmcTarget::Expr(second.clone()),
                        )?,
                        target_lit(
                            nl,
                            &mut cnf,
                            &frames[end],
                            &BmcTarget::Expr(consequence.clone()),
                        )?,
                    ];
                    if let Some(disable) = &assertion.disable {
                        for frame in [start, second_frame, end] {
                            witness_terms.push(target_lit(
                                nl,
                                &mut cnf,
                                &frames[frame],
                                &BmcTarget::Expr(BoolExpr::Not(Box::new(disable.clone()))),
                            )?);
                        }
                    }
                    let witness = and_lits(&mut cnf, &witness_terms);
                    temporal_windows.push((start, witness));
                    repeat_choices.push((start, end, delay, witness));
                    query_lits.push(witness);
                }
            }
        }
    }
    cnf.add_clause(query_lits);

    if cancellation.is_cancelled() {
        return Err("temporal property BMC cancelled".into());
    }

    match solve_cancellable(&cnf, cancellation)? {
        SatResult::Unsat => Ok(PropertyBmcResult {
            assertion: assertion.name.clone(),
            kind: assertion.kind,
            property: assertion.property.clone(),
            disable: assertion.disable.clone(),
            clock: assertion.clock.clone(),
            status: if cover {
                PropertyBmcStatus::NotSatisfied
            } else {
                PropertyBmcStatus::BoundedPass
            },
            trace: None,
            net_trace: None,
            depth_checked: depth,
            frames_checked: frames.len(),
            trigger_frame: None,
            repeat_count: None,
            end_frame: None,
        }),
        SatResult::Sat(model) => {
            let trigger_frame = if temporal_windows.is_empty() {
                None
            } else {
                temporal_windows
                    .iter()
                    .find(|(_, lit)| model_value(&model, *lit))
                    .map(|(start, _)| *start)
            };
            let repeat_count = repeat_choices
                .iter()
                .find(|(_, _, _, lit)| model_value(&model, *lit))
                .map(|(_, _, repeat, _)| *repeat);
            let end_frame = repeat_choices
                .iter()
                .find(|(_, _, _, lit)| model_value(&model, *lit))
                .map(|(_, end, _, _)| *end);
            Ok(PropertyBmcResult {
                assertion: assertion.name.clone(),
                kind: assertion.kind,
                property: assertion.property.clone(),
                disable: assertion.disable.clone(),
                clock: assertion.clock.clone(),
                status: if cover {
                    PropertyBmcStatus::Satisfied
                } else {
                    PropertyBmcStatus::Violated
                },
                trace: Some(input_trace(nl, &frames, &model)),
                net_trace: Some(net_trace(nl, &frames, &model)),
                depth_checked: depth,
                frames_checked: frames.len(),
                trigger_frame,
                repeat_count,
                end_frame,
            })
        }
    }
}

#[allow(clippy::needless_range_loop)]
fn encode_assumptions(
    nl: &Netlist,
    cnf: &mut crate::sat::Cnf,
    frames: &[HashMap<NetId, i32>],
    assumptions: &[Assertion],
    depth: usize,
    cancellation: &FlowCancellation,
) -> Result<(), String> {
    for assumption in assumptions {
        if cancellation.is_cancelled() {
            return Err("temporal property BMC cancelled".into());
        }
        match &assumption.property {
            Property::Immediate(expr) | Property::Always(expr) => {
                for frame in frames {
                    let property = target_lit(nl, cnf, frame, &BmcTarget::Expr(expr.clone()))?;
                    if let Some(disable) = &assumption.disable {
                        let disabled =
                            target_lit(nl, cnf, frame, &BmcTarget::Expr(disable.clone()))?;
                        cnf.add_clause(vec![property, disabled]);
                    } else {
                        cnf.add_clause(vec![property]);
                    }
                }
            }
            Property::Implication {
                antecedent,
                consequence,
                min_cycle,
                max_cycle,
            } => {
                if min_cycle > max_cycle {
                    return Err("temporal assumption delay min is greater than max".into());
                }
                for start in 0..=depth {
                    if cancellation.is_cancelled() {
                        return Err("temporal property BMC cancelled".into());
                    }
                    let mut miss_terms = vec![target_lit(
                        nl,
                        cnf,
                        &frames[start],
                        &BmcTarget::Expr(antecedent.clone()),
                    )?];
                    if let Some(disable) = &assumption.disable {
                        miss_terms.push(target_lit(
                            nl,
                            cnf,
                            &frames[start],
                            &BmcTarget::Expr(disable.clone()),
                        )?);
                    }
                    for offset in *min_cycle..=*max_cycle {
                        miss_terms.push(-target_lit(
                            nl,
                            cnf,
                            &frames[start + offset],
                            &BmcTarget::Expr(consequence.clone()),
                        )?);
                        if let Some(disable) = &assumption.disable {
                            miss_terms.push(target_lit(
                                nl,
                                cnf,
                                &frames[start + offset],
                                &BmcTarget::Expr(disable.clone()),
                            )?);
                        }
                    }
                    let complete_miss = and_lits(cnf, &miss_terms);
                    cnf.add_clause(vec![-complete_miss]);
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
                if !*unbounded && (*min_repeat == 0 || *min_repeat > *max_repeat) {
                    return Err("invalid consecutive repetition range".into());
                }
                for start in 0..=depth {
                    let repeat_max = if *unbounded {
                        depth - start + 1
                    } else {
                        *max_repeat
                    };
                    for repeat in *min_repeat..=repeat_max {
                        let end = start + usize::from(!*overlapped) + repeat.saturating_sub(1);
                        let mut miss_terms = Vec::new();
                        for offset in 0..repeat {
                            miss_terms.push(target_lit(
                                nl,
                                cnf,
                                &frames[start + offset],
                                &BmcTarget::Expr(antecedent.clone()),
                            )?);
                            if let Some(disable) = &assumption.disable {
                                miss_terms.push(target_lit(
                                    nl,
                                    cnf,
                                    &frames[start + offset],
                                    &BmcTarget::Expr(disable.clone()),
                                )?);
                            }
                        }
                        miss_terms.push(-target_lit(
                            nl,
                            cnf,
                            &frames[end],
                            &BmcTarget::Expr(consequence.clone()),
                        )?);
                        if let Some(disable) = &assumption.disable {
                            miss_terms.push(target_lit(
                                nl,
                                cnf,
                                &frames[end],
                                &BmcTarget::Expr(disable.clone()),
                            )?);
                        }
                        let complete_miss = and_lits(cnf, &miss_terms);
                        cnf.add_clause(vec![-complete_miss]);
                    }
                }
            }
            Property::NonconsecutiveImplication {
                antecedent,
                min_count,
                max_count,
                overlapped,
                consequence,
            } => {
                if *min_count == 0 || *min_count > *max_count {
                    return Err("invalid nonconsecutive repetition range".into());
                }
                let last_frame = depth + max_count + usize::from(!*overlapped);
                for start in 0..=depth {
                    for count in *min_count..=*max_count {
                        for end in start + count - 1..=last_frame {
                            let mut ant_lits = Vec::with_capacity(end - start + 1);
                            for frame in start..=end {
                                ant_lits.push(target_lit(
                                    nl,
                                    cnf,
                                    &frames[frame],
                                    &BmcTarget::Expr(antecedent.clone()),
                                )?);
                            }
                            let mut match_terms = vec![exactly_true_lits(cnf, &ant_lits, count)?];
                            if let Some(disable) = &assumption.disable {
                                for frame in start..=end {
                                    match_terms.push(target_lit(
                                        nl,
                                        cnf,
                                        &frames[frame],
                                        &BmcTarget::Expr(disable.clone()),
                                    )?);
                                }
                            }
                            let pattern = and_lits(cnf, &match_terms);
                            let consequence_lit = target_lit(
                                nl,
                                cnf,
                                &frames[end + usize::from(!*overlapped)],
                                &BmcTarget::Expr(consequence.clone()),
                            )?;
                            cnf.add_clause(vec![-pattern, consequence_lit]);
                        }
                    }
                }
            }
            Property::SequenceImplication {
                first,
                second,
                min_delay,
                max_delay,
                overlapped,
                consequence,
            } => {
                if min_delay > max_delay {
                    return Err("sequence delay min is greater than max".into());
                }
                for start in 0..=depth {
                    for delay in *min_delay..=*max_delay {
                        let second_frame = start + delay;
                        let end = second_frame + usize::from(!*overlapped);
                        let mut miss_terms = vec![
                            target_lit(nl, cnf, &frames[start], &BmcTarget::Expr(first.clone()))?,
                            target_lit(
                                nl,
                                cnf,
                                &frames[second_frame],
                                &BmcTarget::Expr(second.clone()),
                            )?,
                            -target_lit(
                                nl,
                                cnf,
                                &frames[end],
                                &BmcTarget::Expr(consequence.clone()),
                            )?,
                        ];
                        if let Some(disable) = &assumption.disable {
                            for frame in [start, second_frame, end] {
                                miss_terms.push(target_lit(
                                    nl,
                                    cnf,
                                    &frames[frame],
                                    &BmcTarget::Expr(disable.clone()),
                                )?);
                            }
                        }
                        let complete_miss = and_lits(cnf, &miss_terms);
                        cnf.add_clause(vec![-complete_miss]);
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn property_bmc_report(result: &PropertyBmcResult) -> String {
    if result.kind == AssertionKind::Assume {
        return match result.status {
            PropertyBmcStatus::BoundedPass => format!(
                "Temporal BMC report '{}': ASSUMPTION HOLDS through {} cycle(s), {} frame(s)\n",
                result.assertion, result.depth_checked, result.frames_checked
            ),
            PropertyBmcStatus::Violated => format!(
                "Temporal BMC report '{}': ASSUMPTION VIOLATION at cycle {}, {} frame(s) checked\n",
                result.assertion,
                result
                    .trigger_frame
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".into()),
                result.frames_checked
            ),
            PropertyBmcStatus::AssumptionsUnsat => format!(
                "Temporal BMC report '{}': ASSUMPTIONS UNSAT (vacuous result)\n",
                result.assertion
            ),
            PropertyBmcStatus::Satisfied => format!(
                "Temporal BMC report '{}': ASSUMPTION WITNESS at cycle {}\n",
                result.assertion,
                result
                    .trigger_frame
                    .map(|v| v.to_string())
                    .unwrap_or_default()
            ),
            PropertyBmcStatus::NotSatisfied => format!(
                "Temporal BMC report '{}': NO ASSUMPTION WITNESS through {} cycle(s)\n",
                result.assertion, result.depth_checked
            ),
        };
    }
    let repeat = result
        .repeat_count
        .map(|count| format!(", repeat={}", count))
        .unwrap_or_default();
    match result.status {
        PropertyBmcStatus::BoundedPass => format!(
            "Temporal BMC report '{}': BOUNDED PASS through {} cycle(s), {} frame(s)\n",
            result.assertion, result.depth_checked, result.frames_checked
        ),
        PropertyBmcStatus::Violated => format!(
            "Temporal BMC report '{}': VIOLATED at trigger cycle {}{}, {} frame(s) checked\n",
            result.assertion,
            result
                .trigger_frame
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".into()),
            repeat,
            result.frames_checked
        ),
        PropertyBmcStatus::Satisfied => format!(
            "Temporal BMC report '{}': COVER HIT at cycle {}{}, {} frame(s) checked\n",
            result.assertion,
            result
                .trigger_frame
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".into()),
            repeat,
            result.frames_checked
        ),
        PropertyBmcStatus::NotSatisfied => format!(
            "Temporal BMC report '{}': COVER MISS through {} cycle(s), {} frame(s)\n",
            result.assertion, result.depth_checked, result.frames_checked
        ),
        PropertyBmcStatus::AssumptionsUnsat => format!(
            "Temporal BMC report '{}': ASSUMPTIONS UNSAT (vacuous result), {} frame(s)\n",
            result.assertion, result.frames_checked
        ),
    }
}

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

fn net_signals(nl: &Netlist, trace: &[HashMap<String, u64>]) -> Result<Vec<VcdSignal>, String> {
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
fn property_markers(
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
fn eval_formal_expr(expr: &BoolExpr, values: &HashMap<String, u64>) -> Result<bool, String> {
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
fn property_violation_in_trace(
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

fn disable_is_true(
    disable: Option<&BoolExpr>,
    values: &HashMap<String, u64>,
) -> Result<bool, String> {
    disable
        .map(|expr| eval_formal_expr(expr, values))
        .transpose()
        .map(|disabled| disabled.unwrap_or(false))
}

fn pulse_changes(
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
fn validate_assertion(nl: &Netlist, assertion: &Assertion) -> Result<(), String> {
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

fn sequential_clock_edges(nl: &Netlist) -> Result<Vec<(NetId, bool)>, String> {
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
fn effective_clock(nl: &Netlist, mut clock: NetId, mut posedge: bool) -> (NetId, bool) {
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

fn unroll(
    nl: &Netlist,
    opts: &BmcOptions,
    depth: usize,
    cancellation: &FlowCancellation,
) -> Result<(crate::sat::Cnf, Vec<HashMap<NetId, i32>>), String> {
    let mut cnf = crate::sat::Cnf::new();
    let mut frames: Vec<HashMap<NetId, i32>> = Vec::new();
    let flops = flop_pins(nl)?;
    for t in 0..=depth {
        if cancellation.is_cancelled() {
            return Err("temporal property BMC cancelled".into());
        }
        let (frame_cnf, frame_lit) = encode_frame(nl)?;
        let off = cnf.n_vars as i32;
        while cnf.n_vars < off as usize + frame_cnf.n_vars {
            cnf.add_var();
        }
        for clause in &frame_cnf.clauses {
            cnf.add_clause(clause.iter().map(|lit| shift_lit(*lit, off)).collect());
        }
        let lit = frame_lit
            .iter()
            .map(|(net, value)| (*net, shift_lit(*value, off)))
            .collect::<HashMap<_, _>>();
        if t > 0 {
            let previous = &frames[t - 1];
            for flop in &flops {
                if cancellation.is_cancelled() {
                    return Err("temporal property BMC cancelled".into());
                }
                let q_now = lit[&flop.q];
                let d_previous = previous[&flop.d];
                match flop.reset {
                    Some((reset_previous, reset_high)) => {
                        let reset_active = if reset_high {
                            previous[&reset_previous]
                        } else {
                            -previous[&reset_previous]
                        };
                        cnf.add_clause(vec![-q_now, -reset_active]);
                        cnf.add_clause(vec![-q_now, reset_active, d_previous]);
                        cnf.add_clause(vec![q_now, reset_active, -d_previous]);
                    }
                    None => {
                        cnf.add_clause(vec![-q_now, d_previous]);
                        cnf.add_clause(vec![q_now, -d_previous]);
                    }
                }
                if let Some((reset_now, reset_high)) = flop.reset {
                    let active_now = if reset_high {
                        lit[&reset_now]
                    } else {
                        -lit[&reset_now]
                    };
                    cnf.add_clause(vec![-q_now, -active_now]);
                }
            }
        }
        if t == 0 && opts.init_state_zero {
            for flop in &flops {
                cnf.add_clause(vec![-lit[&flop.q]]);
            }
        }
        frames.push(lit);
    }
    Ok((cnf, frames))
}

fn and_lits(cnf: &mut crate::sat::Cnf, lits: &[i32]) -> i32 {
    let output = cnf.add_var() as i32;
    for lit in lits {
        cnf.add_clause(vec![-output, *lit]);
    }
    cnf.add_clause({
        let mut clause = Vec::with_capacity(lits.len() + 1);
        clause.push(output);
        clause.extend(lits.iter().map(|lit| -lit));
        clause
    });
    output
}

fn constant_lit(cnf: &mut crate::sat::Cnf, value: bool) -> i32 {
    let lit = cnf.add_var() as i32;
    cnf.add_clause(vec![if value { lit } else { -lit }]);
    lit
}

/// Encode `exactly k` of the input literals with a sequential unary counter.
#[allow(clippy::needless_range_loop)]
fn exactly_true_lits(cnf: &mut crate::sat::Cnf, lits: &[i32], count: usize) -> Result<i32, String> {
    if count > lits.len() {
        return Ok(constant_lit(cnf, false));
    }
    let mut previous = vec![constant_lit(cnf, false); count + 1];
    previous[0] = constant_lit(cnf, true);
    for (index, input) in lits.iter().enumerate() {
        let processed = index + 1;
        let mut current = Vec::with_capacity(count + 1);
        for target in 0..=count.min(processed) {
            let mut sources = Vec::new();
            if target <= index {
                sources.push(and_lits(cnf, &[previous[target], -*input]));
            }
            if target > 0 && target - 1 <= index {
                sources.push(and_lits(cnf, &[previous[target - 1], *input]));
            }
            current.push(or_lits(cnf, &sources));
        }
        for target in count.min(processed) + 1..=count {
            current.push(previous[target]);
        }
        previous = current;
    }
    let result = previous[count];
    Ok(result)
}

fn or_lits(cnf: &mut crate::sat::Cnf, lits: &[i32]) -> i32 {
    let output = cnf.add_var() as i32;
    let mut completion = Vec::with_capacity(lits.len() + 1);
    completion.push(-output);
    for lit in lits {
        cnf.add_clause(vec![output, -*lit]);
        completion.push(*lit);
    }
    cnf.add_clause(completion);
    output
}

fn model_value(model: &[bool], lit: i32) -> bool {
    model.get(lit.unsigned_abs() as usize - 1).copied() == Some(lit > 0)
}

fn input_trace(
    nl: &Netlist,
    frames: &[HashMap<NetId, i32>],
    model: &[bool],
) -> Vec<HashMap<String, u64>> {
    frames
        .iter()
        .map(|frame| {
            let mut values = HashMap::new();
            for port in &nl.ports {
                if port.dir != Dir::In {
                    continue;
                }
                let mut value = 0u64;
                for (bit, net) in port.bits.iter().enumerate() {
                    if model_value(model, frame[net]) {
                        value |= 1 << bit;
                    }
                }
                values.insert(port.name.clone(), value);
            }
            values
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sat::solve;
    use eda_synth::{synthesize, SynthOptions};

    fn netlist(source: &str) -> Netlist {
        let design = eda_hdl::compile(source, None).unwrap();
        synthesize(
            &design,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )
        .unwrap()
        .netlist
    }

    #[test]
    fn immediate_property_counterexample_is_found() {
        let nl = netlist("module m(input a, output y); assign y = a; endmodule");
        let assertions = eda_assert::parse_assertions("assert property (!a);").unwrap();
        let result = bmc_assertion(&nl, &assertions[0], &BmcOptions::default()).unwrap();
        assert!(result.violated());
        assert!(result.trace.is_some());
    }

    #[test]
    fn temporal_bmc_checks_cancellation_during_encoding() {
        let nl = netlist(
            "module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule",
        );
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) d |-> q);").unwrap();
        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = bmc_assertion_cancellable(
            &nl,
            &assertions[0],
            &BmcOptions {
                depth: 64,
                init_state_zero: true,
            },
            &cancellation,
        )
        .unwrap_err();
        assert_eq!(error, "temporal property BMC cancelled");
    }

    #[test]
    fn sequential_exact_counter_encodes_exactly_two() {
        let mut cnf = crate::sat::Cnf::new();
        let lits: Vec<i32> = (0..4).map(|_| cnf.add_var() as i32).collect();
        let exact = exactly_true_lits(&mut cnf, &lits, 2).unwrap();
        cnf.add_clause(vec![exact]);
        match solve(&cnf) {
            crate::sat::SatResult::Sat(model) => {
                let count = lits.iter().filter(|lit| model_value(&model, **lit)).count();
                assert_eq!(count, 2);
            }
            crate::sat::SatResult::Unsat => panic!("exact-two encoding is UNSAT"),
        }
    }

    #[test]
    fn sequential_exact_counter_accepts_net_trace() {
        let nl = netlist(
            "module sparse(input clk, d, output reg q);
                always @(posedge clk) q <= d;
             endmodule",
        );
        let (mut cnf, frames) = unroll(
            &nl,
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
            6,
            &FlowCancellation::new(),
        )
        .unwrap();
        let lits: Vec<i32> = (0..=4)
            .map(|frame| {
                target_lit(
                    &nl,
                    &mut cnf,
                    &frames[frame],
                    &BmcTarget::Expr(BoolExpr::Signal("d".into())),
                )
                .unwrap()
            })
            .collect();
        let exact = exactly_true_lits(&mut cnf, &lits, 1).unwrap();
        cnf.add_clause(vec![exact]);
        assert!(solve(&cnf) != SatResult::Unsat);
    }

    #[test]
    fn immediate_cover_witness_is_found() {
        let nl = netlist("module m(input a, output y); assign y = a; endmodule");
        let assertions = eda_assert::parse_assertions("cover property (a);").unwrap();
        let result = bmc_assertion(&nl, &assertions[0], &BmcOptions::default()).unwrap();
        assert_eq!(result.status, PropertyBmcStatus::Satisfied);
        assert!(result.trigger_frame.is_some());
        assert!(result.trace.is_some());
        assert!(property_bmc_report(&result).contains("COVER HIT"));
    }

    #[test]
    fn immediate_assumption_constrains_bounded_assertion() {
        let nl = netlist(
            "module m(input clk, d, output reg q);
                always @(posedge clk) q <= d;
             endmodule",
        );
        let properties = eda_assert::parse_assertions(
            r#"
            assert property (@(posedge clk) !d);
            assume property (@(posedge clk) !d);
        "#,
        )
        .unwrap();
        let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
            .into_iter()
            .partition(|property| property.kind != AssertionKind::Assume);
        let unconstrained = bmc_assertion(
            &nl,
            &assertions[0],
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert!(unconstrained.violated());

        let constrained = bmc_assertions_with_assumptions(
            &nl,
            &assertions,
            &assumptions,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert_eq!(constrained[0].status, PropertyBmcStatus::BoundedPass);
    }

    #[test]
    fn temporal_assumption_constrains_bounded_assertion() {
        let nl = netlist(
            "module m(input clk, d, en, mode, output reg state, output q);
                always @(posedge clk) state <= d;
                assign q = mode & en;
             endmodule",
        );
        let properties = eda_assert::parse_assertions(
            r#"
            assert property (@(posedge clk) mode && d |-> ##[1:1] q);
            assume property (@(posedge clk) mode && d |-> ##[1:1] mode && en);
        "#,
        )
        .unwrap();
        let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
            .into_iter()
            .partition(|property| property.kind != AssertionKind::Assume);
        let results = bmc_assertions_with_assumptions(
            &nl,
            &assertions,
            &assumptions,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert_eq!(results[0].status, PropertyBmcStatus::BoundedPass);
        assert_eq!(results[0].frames_checked, 6);
    }

    #[test]
    fn inconsistent_assumptions_report_a_vacuous_result() {
        let nl = netlist(
            "module m(input clk, d, output reg q);
                always @(posedge clk) q <= d;
             endmodule",
        );
        let properties = eda_assert::parse_assertions(
            r#"
            assert property (@(posedge clk) d);
            assume property (@(posedge clk) d);
            assume property (@(posedge clk) !d);
        "#,
        )
        .unwrap();
        let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
            .into_iter()
            .partition(|property| property.kind != AssertionKind::Assume);
        let results =
            bmc_assertions_with_assumptions(&nl, &assertions, &assumptions, &BmcOptions::default())
                .unwrap();
        assert_eq!(results[0].status, PropertyBmcStatus::AssumptionsUnsat);
        assert!(property_bmc_report(&results[0]).contains("ASSUMPTIONS UNSAT"));
    }

    #[test]
    fn assumptions_constrain_cover_witnesses() {
        let nl = netlist(
            "module m(input clk, d, output reg q);
                always @(posedge clk) q <= d;
             endmodule",
        );
        let properties = eda_assert::parse_assertions(
            r#"
            cover property (@(posedge clk) d);
            assume property (@(posedge clk) !d);
        "#,
        )
        .unwrap();
        let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
            .into_iter()
            .partition(|property| property.kind != AssertionKind::Assume);
        let results = bmc_assertions_with_assumptions(
            &nl,
            &assertions,
            &assumptions,
            &BmcOptions {
                depth: 2,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert_eq!(results[0].status, PropertyBmcStatus::NotSatisfied);
    }

    #[test]
    fn temporal_cover_can_be_satisfied_or_missed() {
        let nl = netlist(
            "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule",
        );
        let hit = eda_assert::parse_assertions("cover property (@(posedge clk) d |-> ##[2:2] q2);")
            .unwrap();
        let result = bmc_assertion(
            &nl,
            &hit[0],
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert_eq!(result.status, PropertyBmcStatus::Satisfied);
        assert!(result.net_trace.is_some());

        let miss_nl = netlist(
            "module impossible(input clk, d, output done);
                assign done = 1'b0;
             endmodule",
        );
        let miss =
            eda_assert::parse_assertions("cover property (@(posedge clk) d |-> ##[1:1] done);")
                .unwrap();
        let result = bmc_assertion(
            &miss_nl,
            &miss[0],
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert_eq!(result.status, PropertyBmcStatus::NotSatisfied);
        assert!(result.trace.is_none());
        assert!(property_bmc_report(&result).contains("COVER MISS"));
    }

    #[test]
    fn repeated_antecedent_properties_are_checked() {
        let nl = netlist(
            "module shift(input clk, d, output reg q);
                always @(posedge clk) q <= d;
             endmodule",
        );
        let bad =
            eda_assert::parse_assertions("assert property (@(posedge clk) d[*2] |-> !q);").unwrap();
        let result = bmc_assertion(
            &nl,
            &bad[0],
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert!(result.violated());
        assert_eq!(result.repeat_count, Some(2));
        assert!(verify_property_trace(&nl, &result).is_ok());

        let good =
            eda_assert::parse_assertions("assert property (@(posedge clk) d[*2] |-> q);").unwrap();
        let result = bmc_assertion(
            &nl,
            &good[0],
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert_eq!(result.status, PropertyBmcStatus::BoundedPass);

        let cover =
            eda_assert::parse_assertions("cover property (@(posedge clk) d[*2] |-> q);").unwrap();
        let result = bmc_assertion(
            &nl,
            &cover[0],
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert_eq!(result.status, PropertyBmcStatus::Satisfied);
        assert_eq!(result.repeat_count, Some(2));
    }

    #[test]
    fn unbounded_repetition_is_bounded_by_bmc_depth() {
        let nl = netlist(
            "module shift(input clk, d, output reg q);
                always @(posedge clk) q <= d;
             endmodule",
        );
        let star =
            eda_assert::parse_assertions("assert property (@(posedge clk) d[*] |-> q);").unwrap();
        let result = bmc_assertion(
            &nl,
            &star[0],
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert!(result.violated());
        assert_eq!(result.repeat_count, Some(0));

        let plus =
            eda_assert::parse_assertions("assert property (@(posedge clk) d[+] |=> q);").unwrap();
        let result = bmc_assertion(
            &nl,
            &plus[0],
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert_eq!(result.status, PropertyBmcStatus::BoundedPass);

        let cover =
            eda_assert::parse_assertions("cover property (@(posedge clk) d[+] |-> q);").unwrap();
        let result = bmc_assertion(
            &nl,
            &cover[0],
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert_eq!(result.status, PropertyBmcStatus::Satisfied);
        assert_eq!(result.repeat_count, Some(2));
    }

    #[test]
    fn nonconsecutive_repetition_properties_are_checked() {
        let nl = netlist(
            "module sparse(input clk, d, output reg q);
                always @(posedge clk) q <= d;
             endmodule",
        );
        let bad =
            eda_assert::parse_assertions("assert property (@(posedge clk) d[=2] |-> q);").unwrap();
        let result = bmc_assertion(
            &nl,
            &bad[0],
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert!(result.violated());
        assert_eq!(result.repeat_count, Some(2));
        assert!(result.end_frame.is_some());
        assert!(verify_property_trace(&nl, &result).is_ok());

        let cover =
            eda_assert::parse_assertions("cover property (@(posedge clk) d[=2] |-> q);").unwrap();
        let result = bmc_assertion(
            &nl,
            &cover[0],
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert_eq!(result.status, PropertyBmcStatus::Satisfied);
        assert_eq!(result.repeat_count, Some(2));
    }

    #[test]
    fn sequence_antecedent_properties_are_checked() {
        let nl = netlist(
            "module sequence_m(input clk, a, b, output reg state, output q);
                always @(posedge clk) state <= a;
                assign q = b & state;
             endmodule",
        );
        let properties = eda_assert::parse_assertions(
            r#"
            assert property (@(posedge clk) a ##[1:2] b |-> !q);
            cover property (@(posedge clk) a ##1 b |-> q);
        "#,
        )
        .unwrap();
        let results = bmc_assertions(
            &nl,
            &properties,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )
        .unwrap();
        let replay = verify_property_trace(&nl, &results[0]).unwrap();
        assert!(results[0].violated());
        assert!(matches!(results[0].repeat_count, Some(1 | 2)));
        assert!(results[0].end_frame.is_some());
        assert!(replay.property_violation_reproduced);
        assert_eq!(results[1].status, PropertyBmcStatus::Satisfied);
        assert_eq!(results[1].repeat_count, Some(1));
    }

    #[test]
    fn bounded_temporal_delay_is_checked() {
        let nl = netlist(
            "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule",
        );
        let good =
            eda_assert::parse_assertions("assert property (@(posedge clk) d |-> ##[2:2] q2);")
                .unwrap();
        let result = bmc_assertion(
            &nl,
            &good[0],
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert_eq!(result.status, PropertyBmcStatus::BoundedPass);
        assert_eq!(result.frames_checked, 7);

        let bad =
            eda_assert::parse_assertions("assert property (@(posedge clk) d |-> ##[1:1] q2);")
                .unwrap();
        let result = bmc_assertion(
            &nl,
            &bad[0],
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert_eq!(result.status, PropertyBmcStatus::Violated);
        assert!(result.trigger_frame.is_some());
    }

    #[test]
    fn negedge_sequential_properties_are_checked() {
        let nl = netlist(
            "module shift(input clk, d, output reg q1, q2);
                always @(negedge clk) q1 <= d;
                always @(negedge clk) q2 <= q1;
             endmodule",
        );
        let good =
            eda_assert::parse_assertions("assert property (@(negedge clk) d |-> ##[2:2] q2);")
                .unwrap();
        let result = bmc_assertion(
            &nl,
            &good[0],
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert_eq!(result.status, PropertyBmcStatus::BoundedPass);

        let bad =
            eda_assert::parse_assertions("assert property (@(negedge clk) d |-> ##[1:1] q2);")
                .unwrap();
        let result = bmc_assertion(
            &nl,
            &bad[0],
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert!(result.violated());
    }

    #[test]
    fn disable_iff_suppresses_property_frames() {
        let nl = netlist(
            "module m(input clk, d, output reg q, output reset);
                always @(posedge clk) q <= d;
                assign reset = 1'b1;
             endmodule",
        );
        let assertions = eda_assert::parse_assertions(
            "assert property (@(posedge clk) disable iff (reset) !q);",
        )
        .unwrap();
        let result = bmc_assertion(
            &nl,
            &assertions[0],
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert_eq!(result.status, PropertyBmcStatus::BoundedPass);
        assert!(result.trace.is_none());
    }

    #[test]
    fn formal_input_trace_exports_round_trip_vcd() {
        let nl = netlist(
            "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule",
        );
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) d |-> ##[1:1] q2);")
                .unwrap();
        let result = bmc_assertion(
            &nl,
            &assertions[0],
            &BmcOptions {
                depth: 2,
                init_state_zero: true,
            },
        )
        .unwrap();
        let trace = result.trace.clone().expect("counterexample");
        let text = trace_input_vcd(&nl, &trace).unwrap();
        let waveform = eda_waveform::parse_vcd(&text).unwrap();
        let d = waveform.signal("top.d").unwrap();
        let clk = waveform.signal("top.clk").unwrap();
        let trigger = result.trigger_frame.unwrap() as u64;
        assert_eq!(trace[trigger as usize]["d"], 1);
        assert_eq!(
            d.value_at(trigger).unwrap().to_u64().unwrap(),
            trace[result.trigger_frame.unwrap()]["d"]
        );
        assert!(clk.initial().is_some());
        assert!(waveform.time_points().len() <= trace.len());

        let full_trace = result.net_trace.clone().expect("full counterexample");
        let full_text = trace_net_vcd(&nl, &full_trace).unwrap();
        let full_waveform = eda_waveform::parse_vcd(&full_text).unwrap();
        assert!(full_waveform.signal("top.q1").is_some());
        assert!(full_waveform.signal("top.q2").is_some());
        assert_eq!(full_trace[trigger as usize + 1]["q2"], 0);
        assert_eq!(
            full_waveform
                .signal("top.q2")
                .unwrap()
                .value_at(trigger + 1)
                .unwrap()
                .to_u64(),
            Some(0)
        );

        let marker_text = trace_property_vcd(&nl, &result).unwrap();
        let marker_waveform = eda_waveform::parse_vcd(&marker_text).unwrap();
        let trigger_marker = marker_waveform.signal("formal_assertion_trigger").unwrap();
        let miss_marker = marker_waveform.signal("formal_assertion_miss").unwrap();
        let violation_marker = marker_waveform
            .signal("formal_assertion_violation")
            .unwrap();
        assert_eq!(trigger_marker.value_at(trigger).unwrap().to_u64(), Some(1));
        assert_eq!(miss_marker.value_at(trigger + 1).unwrap().to_u64(), Some(1));
        assert_eq!(
            violation_marker.value_at(trigger).unwrap().to_u64(),
            Some(1)
        );
        let vcd_replay = replay_property_vcd(&nl, &result, &marker_text).unwrap();
        assert_eq!(vcd_replay.frames_verified, full_trace.len());
        assert!(vcd_replay.property_violation_reproduced);

        let q2_id = marker_text
            .lines()
            .find(|line| line.contains("$var wire 1 ") && line.contains(" q2 $end"))
            .and_then(|line| line.split_whitespace().nth(3))
            .unwrap()
            .to_string();
        let tampered_vcd = format!("{}#{}\n1{}\n", marker_text, trigger + 1, q2_id);
        assert!(replay_property_vcd(&nl, &result, &tampered_vcd).is_err());

        let replay = verify_property_trace(&nl, &result).unwrap();
        assert_eq!(replay.frames_verified, full_trace.len());
        assert_eq!(replay.transitions_verified, 2 * (full_trace.len() - 1));
        assert!(replay.property_violation_reproduced);

        let mut tampered = result.clone();
        let trigger_index = result.trigger_frame.unwrap();
        tampered.net_trace.as_mut().unwrap()[trigger_index + 1].insert("q2".to_string(), 1);
        assert!(verify_property_trace(&nl, &tampered).is_err());
    }
}
