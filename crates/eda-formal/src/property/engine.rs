//! Formal property implementation module.

use crate::bmc::{net_trace, target_lit, BmcOptions, BmcTarget};
use crate::sat::{solve_cancellable, SatResult};
use eda_assert::{Assertion, AssertionKind, BoolExpr, Property};
use eda_core::cancellation::FlowCancellation;
use eda_core::netlist::{NetId, Netlist};
use std::collections::HashMap;

use super::sat::{and_lits, exactly_true_lits, input_trace, model_value, or_lits, unroll};
use super::semantics::validate_assertion;
use super::types::*;

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
pub(super) fn bmc_assertion_with_assumptions(
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
pub(super) fn encode_assumptions(
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
