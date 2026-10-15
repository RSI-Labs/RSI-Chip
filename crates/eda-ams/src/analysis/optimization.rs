//! Bounded optimization and Pareto-front searches.

use super::derivatives::*;
use super::random::*;
use crate::numeric::solve_real;
use crate::types::*;
use crate::FlowCancellation;
use std::collections::BTreeMap;

pub fn optimize(circuit: &SpiceCircuit, analysis: &Analysis) -> Result<OptimizationResult, String> {
    optimize_cancellable(circuit, analysis, &FlowCancellation::new())
}

/// Optimize with cooperative checks at population, generation, and polish loops.
pub fn optimize_cancellable(
    circuit: &SpiceCircuit,
    analysis: &Analysis,
    cancellation: &FlowCancellation,
) -> Result<OptimizationResult, String> {
    let Analysis::Optimization(options) = analysis else {
        return Err("optimization called with a non-optimization analysis".into());
    };
    if cancellation.is_cancelled() {
        return Err("optimization analysis cancelled".into());
    }
    let local_options = SensitivityOptions {
        target: options.target,
        output_node: options.output_node.clone(),
        frequency_hz: options.frequency_hz,
        relative_step: 0.001,
    };
    let available = sensitivity_parameters(circuit);
    let mut parameters = Vec::with_capacity(options.parameters.len());
    for specification in &options.parameters {
        let nominal = available
            .iter()
            .find(|parameter| {
                parameter
                    .name
                    .eq_ignore_ascii_case(&specification.parameter)
            })
            .ok_or_else(|| {
                format!(
                    "unknown optimization parameter '{}'",
                    specification.parameter
                )
            })?
            .clone();
        let initial = specification
            .initial
            .unwrap_or(nominal.value)
            .clamp(specification.lower, specification.upper);
        let initial = optimization_engineering_value(
            normalize_optimization_value(initial, specification),
            specification,
        );
        parameters.push((specification.clone(), nominal, initial));
    }

    let mut rng = McRng::new(options.seed);
    let dimensions = parameters.len();
    let mut population: Vec<Vec<f64>> = Vec::with_capacity(options.population);
    population.push(
        parameters
            .iter()
            .map(|triple| normalize_optimization_value(triple.2, &triple.0))
            .collect(),
    );
    // Bound corners seed constrained searches with deterministic feasible
    // extremes. This is especially important when a constraint boundary lies
    // near a box corner and random continuous samples have low feasibility.
    for mask in 0..(1usize << dimensions) {
        if population.len() == options.population {
            break;
        }
        let corner: Vec<_> = (0..dimensions)
            .map(|dimension| {
                if mask & (1 << dimension) == 0 {
                    0.0
                } else {
                    1.0
                }
            })
            .collect();
        if population.iter().any(|values| {
            values
                .iter()
                .zip(&corner)
                .all(|(a, b)| (*a - b).abs() < 1e-12)
        }) {
            continue;
        }
        population.push(corner);
    }
    while population.len() < options.population {
        population.push((0..dimensions).map(|_| rng.unit()).collect());
    }
    let initial_values: Vec<_> = parameters.iter().map(|triple| triple.2).collect();
    let initial_circuit = optimization_circuit(circuit, &parameters, &initial_values)?;
    let initial_output = sensitivity_output(&initial_circuit, &local_options)?;
    let initial_score = match options.objective {
        OptimizationObjective::Match => (initial_output - options.goal_value).abs(),
        OptimizationObjective::Minimize => initial_output,
        OptimizationObjective::Maximize => -initial_output,
    };

    let mut evaluations = 0usize;
    let evaluate = |values: &[f64]| -> Result<(SpiceCircuit, f64, f64), String> {
        let actual_values = optimization_engineering_values(values, &options.parameters);
        evaluate_optimization_candidate(
            circuit,
            &parameters,
            &actual_values,
            &local_options,
            options.objective,
            options.goal_value,
        )
    };

    let mut population_results = Vec::with_capacity(population.len());
    for values in &population {
        if cancellation.is_cancelled() {
            return Err("optimization analysis cancelled".into());
        }
        let result = evaluate(values)?;
        evaluations += 1;
        population_results.push((values.clone(), result));
    }
    let mut converged = false;

    for _ in 0..options.iterations {
        for target_index in 0..population.len() {
            if cancellation.is_cancelled() {
                return Err("optimization analysis cancelled".into());
            }
            let (a, b, c) = distinct_optimization_indices(target_index, population.len(), &mut rng);
            let forced = (rng.unit() * dimensions as f64).floor() as usize;
            let mut trial = vec![0.0; dimensions];
            for dimension in 0..dimensions {
                let mutant = (population[a][dimension]
                    + options.differential_weight
                        * (population[b][dimension] - population[c][dimension]))
                    .clamp(0.0, 1.0);
                trial[dimension] =
                    if dimension == forced || rng.unit() < options.crossover_probability {
                        mutant
                    } else {
                        population[target_index][dimension]
                    };
            }
            let trial_result = evaluate(&trial)?;
            evaluations += 1;
            if trial_result.2 <= population_results[target_index].1 .2 {
                population[target_index] = trial.clone();
                population_results[target_index] = (trial, trial_result);
            }
        }

        let best_score = population_results
            .iter()
            .map(|result| result.1 .2)
            .fold(f64::INFINITY, f64::min);
        if options.objective == OptimizationObjective::Match && best_score <= options.tolerance {
            converged = true;
            break;
        }
    }

    let best_index = population_results
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| {
            left.1
                 .2
                .partial_cmp(&right.1 .2)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(index, _)| index)
        .ok_or_else(|| "optimization population is empty".to_string())?;
    let (mut best_values, mut best_result) = population_results[best_index].clone();
    let has_quantum = parameters.iter().any(|triple| triple.0.quantum.is_some());
    if has_quantum {
        refine_quantized_optimum(
            circuit,
            &parameters,
            &local_options,
            options,
            &mut best_values,
            &mut best_result,
            &mut evaluations,
            cancellation,
        )?;
    } else if options.objective == OptimizationObjective::Match {
        // Differential evolution is global but can stagnate near an optimum.
        // For linear DC circuits, exact adjoint first/second derivatives give
        // the local polish a Newton direction without finite-difference noise.
        let analytic_derivatives = |values: &[f64]| -> Option<DerivativeResult> {
            let actual_values = optimization_engineering_values(values, &options.parameters);
            let variant = optimization_circuit(circuit, &parameters, &actual_values).ok()?;
            let derivative_options = DerivativeOptions {
                target: match options.target {
                    MonteCarloTarget::AcMagnitude => DerivativeTarget::AcMagnitude,
                    MonteCarloTarget::OperatingPoint | MonteCarloTarget::TransientFinal => {
                        DerivativeTarget::Dc
                    }
                },
                output_node: options.output_node.clone(),
                frequency_hz: options.frequency_hz,
                parameters: options
                    .parameters
                    .iter()
                    .map(|specification| specification.parameter.clone())
                    .collect(),
            };
            derivative_analysis(&variant, &Analysis::Derivative(derivative_options)).ok()
        };
        for _ in 0..16 {
            if cancellation.is_cancelled() {
                return Err("optimization analysis cancelled".into());
            }
            if best_result.2 <= options.tolerance {
                converged = true;
                break;
            }
            let mut candidate = best_values.clone();
            let mut step_norm;
            let analytic = analytic_derivatives(&best_values);
            if analytic.is_some() {
                evaluations += 1;
            }
            if let Some(derivative) = analytic {
                let spans: Vec<_> = options
                    .parameters
                    .iter()
                    .map(|specification| specification.upper - specification.lower)
                    .collect();
                let gradients: Vec<_> = derivative
                    .entries
                    .iter()
                    .zip(&spans)
                    .map(|(entry, span)| entry.gradient * span)
                    .collect();
                let hessian: Vec<_> = derivative
                    .hessian()
                    .iter()
                    .enumerate()
                    .map(|(row, values)| {
                        values
                            .iter()
                            .zip(&spans)
                            .map(|(value, column_span)| value * spans[row] * column_span)
                            .collect::<Vec<_>>()
                    })
                    .collect();
                let residual = best_result.1 - options.goal_value;
                let mut newton_matrix = hessian
                    .iter()
                    .map(|row| row.iter().map(|value| residual * value).collect::<Vec<_>>())
                    .collect::<Vec<_>>();
                for (row, row_values) in newton_matrix.iter_mut().enumerate().take(dimensions) {
                    for (column, value) in row_values.iter_mut().enumerate().take(dimensions) {
                        *value += gradients[row] * gradients[column];
                    }
                    row_values[row] += 1e-12;
                }
                let newton_direction = solve_real(newton_matrix, gradients.clone()).ok();
                if let Some(direction) = newton_direction {
                    step_norm = 0.0_f64;
                    for dimension in 0..dimensions {
                        let step = -residual * direction[dimension];
                        step_norm = step_norm.hypot(step);
                        candidate[dimension] = (candidate[dimension] + step).clamp(0.0, 1.0);
                    }
                    // A full residual Hessian can be indefinite for nonlinear AC
                    // magnitude transfer functions. If its Newton direction is not a
                    // descent direction, fall back to the positive semidefinite
                    // Gauss-Newton matrix rather than stepping away from the target.
                    let directional_derivative = residual
                        * gradients
                            .iter()
                            .zip(&candidate)
                            .zip(&best_values)
                            .map(|((gradient, value), best)| gradient * (value - best))
                            .sum::<f64>();
                    if directional_derivative >= 0.0 {
                        let mut gauss_newton_matrix = vec![vec![0.0; dimensions]; dimensions];
                        for (row, row_values) in gauss_newton_matrix.iter_mut().enumerate() {
                            for (column, value) in row_values.iter_mut().enumerate() {
                                *value = gradients[row] * gradients[column];
                            }
                            row_values[row] += 1e-12;
                        }
                        if let Ok(direction) = solve_real(gauss_newton_matrix, gradients.clone()) {
                            candidate = best_values
                                .iter()
                                .zip(direction)
                                .map(|(best, direction)| {
                                    (best - residual * direction).clamp(0.0, 1.0)
                                })
                                .collect();
                        }
                    }
                } else {
                    let denominator: f64 =
                        gradients.iter().map(|gradient| gradient * gradient).sum();
                    if denominator <= f64::EPSILON {
                        break;
                    }
                    step_norm = 0.0_f64;
                    for (dimension, gradient) in gradients.iter().enumerate() {
                        let step = -residual * gradient / denominator;
                        step_norm = step_norm.hypot(step);
                        candidate[dimension] = (candidate[dimension] + step).clamp(0.0, 1.0);
                    }
                }
            } else {
                let mut gradients = Vec::with_capacity(dimensions);
                for dimension in 0..dimensions {
                    let mut low_values = best_values.clone();
                    let mut high_values = best_values.clone();
                    low_values[dimension] = (low_values[dimension] - 1e-5).clamp(0.0, 1.0);
                    high_values[dimension] = (high_values[dimension] + 1e-5).clamp(0.0, 1.0);
                    let low_output = evaluate(&low_values)?.1;
                    let high_output = evaluate(&high_values)?.1;
                    evaluations += 2;
                    gradients.push((high_output - low_output) / 2e-5);
                }
                let residual = best_result.1 - options.goal_value;
                let denominator: f64 = gradients.iter().map(|gradient| gradient * gradient).sum();
                if denominator <= f64::EPSILON {
                    break;
                }
                step_norm = 0.0_f64;
                for (dimension, gradient) in gradients.iter().enumerate() {
                    let step = -residual * gradient / denominator;
                    step_norm = step_norm.hypot(step);
                    candidate[dimension] = (candidate[dimension] + step).clamp(0.0, 1.0);
                }
            }
            if step_norm > 0.25 {
                let scale = 0.25 / step_norm;
                for dimension in 0..dimensions {
                    candidate[dimension] = (best_values[dimension]
                        + (candidate[dimension] - best_values[dimension]) * scale)
                        .clamp(0.0, 1.0);
                }
            }
            let mut trial = candidate;
            let mut candidate_result = evaluate(&trial)?;
            evaluations += 1;
            let mut accepted = false;
            let mut scale = 1.0_f64;
            let full_step: Vec<_> = best_values
                .iter()
                .zip(&trial)
                .map(|(best, value)| value - best)
                .collect();
            loop {
                if cancellation.is_cancelled() {
                    return Err("optimization analysis cancelled".into());
                }
                if candidate_result.2 <= best_result.2 {
                    accepted = true;
                    break;
                }
                scale *= 0.5;
                if scale < 1e-6 {
                    break;
                }
                trial = best_values
                    .iter()
                    .zip(&full_step)
                    .map(|(best, step)| best + step * scale)
                    .collect();
                candidate_result = evaluate(&trial)?;
                evaluations += 1;
            }
            if accepted {
                best_values = trial;
                best_result = candidate_result;
            }
            if !accepted {
                break;
            }
        }
    }
    if options.objective == OptimizationObjective::Match && best_result.2 <= options.tolerance {
        converged = true;
    }
    let (best_circuit, final_output, final_score) = best_result;
    let final_values = optimization_engineering_values(&best_values, &options.parameters);
    let mut parameter_results = Vec::with_capacity(options.parameters.len());
    for (specification, final_value) in options.parameters.iter().zip(final_values) {
        parameter_results.push(OptimizationParameterResult {
            parameter: specification.parameter.clone(),
            initial: parameters
                .iter()
                .find(|triple| {
                    triple
                        .0
                        .parameter
                        .eq_ignore_ascii_case(&specification.parameter)
                })
                .map(|triple| triple.2)
                .ok_or_else(|| "internal optimization parameter mismatch".to_string())?,
            final_value,
            lower: specification.lower,
            upper: specification.upper,
            quantum: specification.quantum,
        });
    }
    Ok(OptimizationResult {
        objective: options.objective,
        target: options.target,
        output_node: options.output_node.clone(),
        goal_value: options.goal_value,
        initial_output,
        initial_score,
        final_output,
        final_score,
        converged,
        generations: options.iterations,
        evaluations,
        parameters: parameter_results,
        circuit: best_circuit,
    })
}

#[derive(Clone)]
struct ParetoCandidate {
    outputs: Vec<f64>,
    constraint_values: Vec<f64>,
    constraint_violation: f64,
    values: Vec<f64>,
    circuit: SpiceCircuit,
}

pub fn pareto_optimize(
    circuit: &SpiceCircuit,
    analysis: &Analysis,
) -> Result<ParetoResult, String> {
    pareto_optimize_cancellable(circuit, analysis, &FlowCancellation::new())
}

/// Optimize a Pareto front with cooperative checks at every candidate loop.
pub fn pareto_optimize_cancellable(
    circuit: &SpiceCircuit,
    analysis: &Analysis,
    cancellation: &FlowCancellation,
) -> Result<ParetoResult, String> {
    let Analysis::Pareto(options) = analysis else {
        return Err("Pareto optimization called with a non-Pareto analysis".into());
    };
    if cancellation.is_cancelled() {
        return Err("Pareto optimization cancelled".into());
    }
    circuit.validate()?;
    let available = sensitivity_parameters(circuit);
    let mut parameters = Vec::with_capacity(options.parameters.len());
    for specification in &options.parameters {
        let nominal = available
            .iter()
            .find(|parameter| {
                parameter
                    .name
                    .eq_ignore_ascii_case(&specification.parameter)
            })
            .ok_or_else(|| {
                format!(
                    "unknown Pareto optimization parameter '{}'",
                    specification.parameter
                )
            })?
            .clone();
        let initial = specification
            .initial
            .unwrap_or(nominal.value)
            .clamp(specification.lower, specification.upper);
        let initial = optimization_engineering_value(
            normalize_optimization_value(initial, specification),
            specification,
        );
        parameters.push((specification.clone(), nominal, initial));
    }

    let mut rng = McRng::new(options.seed);
    let dimensions = parameters.len();
    let mut population: Vec<Vec<f64>> = Vec::with_capacity(options.population);
    population.push(
        parameters
            .iter()
            .map(|triple| normalize_optimization_value(triple.2, &triple.0))
            .collect(),
    );
    while population.len() < options.population {
        population.push((0..dimensions).map(|_| rng.unit()).collect());
    }

    let mut evaluations = 0usize;
    let mut archive: Vec<ParetoCandidate> = Vec::new();
    let evaluate = |values: &[f64], archive: &mut Vec<ParetoCandidate>| -> Result<usize, String> {
        let actual_values = optimization_engineering_values(values, &options.parameters);
        let variant = optimization_circuit(circuit, &parameters, &actual_values)?;
        let mut outputs = Vec::with_capacity(options.output_nodes.len());
        for output_node in &options.output_nodes {
            let local = SensitivityOptions {
                target: options.target,
                output_node: output_node.clone(),
                frequency_hz: options.frequency_hz,
                relative_step: 0.001,
            };
            let output = sensitivity_output(&variant, &local)?;
            if !output.is_finite() {
                return Err(format!(
                    "Pareto optimization output '{}' is non-finite",
                    output_node
                ));
            }
            outputs.push(output);
        }
        let mut constraint_values = Vec::with_capacity(options.constraints.len());
        let mut constraint_violation = 0.0;
        for constraint in &options.constraints {
            let mut variables = BTreeMap::new();
            for variable in constraint.variable_names() {
                let local = SensitivityOptions {
                    target: options.target,
                    output_node: variable.clone(),
                    frequency_hz: options.frequency_hz,
                    relative_step: 0.001,
                };
                let value = sensitivity_output(&variant, &local)?;
                if !value.is_finite() {
                    return Err(format!(
                        "Pareto optimization constraint node '{variable}' is non-finite"
                    ));
                }
                variables.insert(variable, value);
            }
            let value = constraint.evaluate(&variables)?;
            constraint_violation +=
                crate::types::constraint_violation(value, constraint.sense, constraint.bound);
            constraint_values.push(value);
        }
        archive.push(ParetoCandidate {
            outputs,
            constraint_values,
            constraint_violation,
            values: actual_values,
            circuit: variant,
        });
        Ok(archive.len() - 1)
    };

    let mut population_results = Vec::with_capacity(population.len());
    for values in &population {
        if cancellation.is_cancelled() {
            return Err("Pareto optimization cancelled".into());
        }
        let index = evaluate(values, &mut archive)?;
        evaluations += 1;
        population_results.push((values.clone(), index));
    }

    for _ in 0..options.iterations {
        for target_index in 0..population.len() {
            if cancellation.is_cancelled() {
                return Err("Pareto optimization cancelled".into());
            }
            let (a, b, c) = distinct_optimization_indices(target_index, population.len(), &mut rng);
            let forced = (rng.unit() * dimensions as f64).floor() as usize;
            let mut trial = vec![0.0; dimensions];
            for dimension in 0..dimensions {
                let mutant = (population[a][dimension]
                    + options.differential_weight
                        * (population[b][dimension] - population[c][dimension]))
                    .clamp(0.0, 1.0);
                trial[dimension] =
                    if dimension == forced || rng.unit() < options.crossover_probability {
                        mutant
                    } else {
                        population[target_index][dimension]
                    };
            }
            let trial_index = evaluate(&trial, &mut archive)?;
            evaluations += 1;
            let target_index_in_archive = population_results[target_index].1;
            let trial_outputs = &archive[trial_index].outputs;
            let target_outputs = &archive[target_index_in_archive].outputs;
            let replace = if crate::types::constrained_pareto_dominates(
                trial_outputs,
                archive[trial_index].constraint_violation,
                target_outputs,
                archive[target_index_in_archive].constraint_violation,
                &options.senses,
            ) {
                true
            } else if crate::types::constrained_pareto_dominates(
                target_outputs,
                archive[target_index_in_archive].constraint_violation,
                trial_outputs,
                archive[trial_index].constraint_violation,
                &options.senses,
            ) {
                false
            } else {
                rng.unit() < 0.25
            };
            if replace {
                population[target_index] = trial.clone();
                population_results[target_index] = (trial, trial_index);
            }
        }
    }

    let mut front_indices: Vec<usize> = (0..archive.len())
        .filter(|left| {
            (0..archive.len()).all(|right| {
                right == *left
                    || !crate::types::constrained_pareto_dominates(
                        &archive[right].outputs,
                        archive[right].constraint_violation,
                        &archive[*left].outputs,
                        archive[*left].constraint_violation,
                        &options.senses,
                    )
            })
        })
        .collect();
    sort_pareto_front_indices(&archive, &mut front_indices, &options.senses);
    front_indices.dedup_by(|left, right| {
        archive[*left]
            .outputs
            .iter()
            .zip(&archive[*right].outputs)
            .all(|(a, b)| (a - b).abs() <= 1e-10)
    });
    let mut reference_directions: BTreeMap<usize, Vec<f64>> = BTreeMap::new();
    if front_indices.len() > options.maximum_points {
        match options.front_method {
            ParetoFrontMethod::CrowdingDistance => {
                let distances =
                    pareto_crowding_distances(&archive, &front_indices, &options.senses);
                let mut ranked: Vec<_> = front_indices.into_iter().zip(distances).collect();
                ranked.sort_by(|left, right| {
                    right
                        .1
                        .partial_cmp(&left.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                front_indices = ranked
                    .into_iter()
                    .take(options.maximum_points)
                    .map(|(index, _)| index)
                    .collect();
            }
            ParetoFrontMethod::ReferenceDirections => {
                let (selected, associations) = select_pareto_reference_directions(
                    &archive,
                    &front_indices,
                    &options.senses,
                    options.maximum_points,
                    options.reference_divisions,
                    options.adaptive_references,
                );
                front_indices = selected;
                reference_directions = associations;
            }
        }
        sort_pareto_front_indices(&archive, &mut front_indices, &options.senses);
    }
    let final_crowding_distances =
        pareto_crowding_distances(&archive, &front_indices, &options.senses);

    let points = front_indices
        .into_iter()
        .zip(final_crowding_distances)
        .map(|(index, crowding_distance)| {
            let candidate = archive[index].clone();
            let outputs = candidate.outputs;
            let constraint_values = candidate.constraint_values;
            let constraint_violation = candidate.constraint_violation;
            let reference_direction = reference_directions.get(&index).cloned();
            let actual_values = candidate.values;
            let parameter_results = options
                .parameters
                .iter()
                .zip(actual_values)
                .map(|(specification, final_value)| OptimizationParameterResult {
                    parameter: specification.parameter.clone(),
                    initial: parameters
                        .iter()
                        .find(|triple| {
                            triple
                                .0
                                .parameter
                                .eq_ignore_ascii_case(&specification.parameter)
                        })
                        .map(|triple| triple.2)
                        .unwrap_or(final_value),
                    final_value,
                    lower: specification.lower,
                    upper: specification.upper,
                    quantum: specification.quantum,
                })
                .collect();
            ParetoPoint {
                outputs,
                crowding_distance,
                reference_direction,
                constraint_values,
                constraint_violation,
                constraints_satisfied: constraint_violation <= 1e-10,
                parameters: parameter_results,
                circuit: candidate.circuit,
            }
        })
        .collect();

    Ok(ParetoResult {
        target: options.target,
        output_nodes: options.output_nodes.clone(),
        senses: options.senses.clone(),
        front_method: options.front_method,
        adaptive_references: options.adaptive_references,
        constraints: options.constraints.clone(),
        points,
        generations: options.iterations,
        evaluations,
    })
}

fn sort_pareto_front_indices(
    archive: &[ParetoCandidate],
    indices: &mut [usize],
    senses: &[ParetoSense],
) {
    indices.sort_by(|left, right| {
        let mut ordering = std::cmp::Ordering::Equal;
        for (dimension, sense) in senses.iter().enumerate() {
            let a = &archive[*left].outputs[dimension];
            let b = &archive[*right].outputs[dimension];
            ordering = match sense {
                ParetoSense::Minimize => a.partial_cmp(b),
                ParetoSense::Maximize => b.partial_cmp(a),
            }
            .unwrap_or(std::cmp::Ordering::Equal);
            if ordering != std::cmp::Ordering::Equal {
                break;
            }
        }
        ordering.then_with(|| {
            archive[*left].values[0]
                .partial_cmp(&archive[*right].values[0])
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
}

fn pareto_crowding_distances(
    archive: &[ParetoCandidate],
    indices: &[usize],
    senses: &[ParetoSense],
) -> Vec<f64> {
    let dimensions = senses.len();
    let mut coordinates = vec![vec![0.0; indices.len()]; dimensions];
    for dimension in 0..dimensions {
        let values: Vec<_> = indices
            .iter()
            .map(|index| match senses[dimension] {
                ParetoSense::Minimize => archive[*index].outputs[dimension],
                ParetoSense::Maximize => -archive[*index].outputs[dimension],
            })
            .collect();
        let minimum = values.iter().copied().fold(f64::INFINITY, f64::min);
        let maximum = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let span = maximum - minimum;
        for (row, value) in values.into_iter().enumerate() {
            coordinates[dimension][row] = if span > 1e-12 {
                (value - minimum) / span
            } else {
                0.5
            };
        }
    }

    let mut distances = vec![0.0; indices.len()];
    for normalized_dimension in &coordinates {
        let mut order: Vec<_> = (0..normalized_dimension.len()).collect();
        order.sort_by(|left, right| {
            normalized_dimension[*left]
                .partial_cmp(&normalized_dimension[*right])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        distances[order[0]] = f64::INFINITY;
        distances[order[order.len() - 1]] = f64::INFINITY;
        for interior in 1..order.len().saturating_sub(1) {
            let span = normalized_dimension[order[interior + 1]]
                - normalized_dimension[order[interior - 1]];
            distances[order[interior]] += span.abs();
        }
    }
    distances
}

fn simplex_reference_directions(dimensions: usize, divisions: usize) -> Vec<Vec<f64>> {
    let mut out = Vec::new();
    let mut prefix = Vec::with_capacity(dimensions);
    build_simplex_directions(divisions, dimensions, &mut prefix, &mut out);
    for direction in &mut out {
        let norm: f64 = direction
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt();
        if norm > 0.0 {
            for value in direction {
                *value /= norm;
            }
        }
    }
    out
}

fn build_simplex_directions(
    remaining_sum: usize,
    remaining_dimensions: usize,
    prefix: &mut Vec<f64>,
    out: &mut Vec<Vec<f64>>,
) {
    if remaining_dimensions == 1 {
        let mut direction = prefix.clone();
        direction.push(remaining_sum as f64);
        out.push(direction);
        return;
    }
    for component in 0..=remaining_sum {
        prefix.push(component as f64);
        build_simplex_directions(
            remaining_sum - component,
            remaining_dimensions - 1,
            prefix,
            out,
        );
        prefix.pop();
    }
}

#[allow(clippy::type_complexity)]
fn select_pareto_reference_directions(
    archive: &[ParetoCandidate],
    indices: &[usize],
    senses: &[ParetoSense],
    maximum_points: usize,
    divisions: usize,
    adaptive_references: bool,
) -> (Vec<usize>, BTreeMap<usize, Vec<f64>>) {
    let dimensions = senses.len();
    let mut coordinates = vec![vec![0.0; indices.len()]; dimensions];
    for dimension in 0..dimensions {
        let values: Vec<_> = indices
            .iter()
            .map(|index| match senses[dimension] {
                ParetoSense::Minimize => archive[*index].outputs[dimension],
                ParetoSense::Maximize => -archive[*index].outputs[dimension],
            })
            .collect();
        let minimum = values.iter().copied().fold(f64::INFINITY, f64::min);
        let maximum = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let span = maximum - minimum;
        for (row, value) in values.into_iter().enumerate() {
            coordinates[dimension][row] = if span > 1e-12 {
                (value - minimum) / span
            } else {
                0.5
            };
        }
    }

    let mut directions = simplex_reference_directions(dimensions, divisions);
    if adaptive_references {
        let mut layered = directions;
        let candidates = (0..indices.len()).map(|row| {
            (0..dimensions)
                .map(|dimension| coordinates[dimension][row])
                .collect::<Vec<_>>()
        });
        for candidate in candidates {
            let norm: f64 = candidate
                .iter()
                .map(|value| value * value)
                .sum::<f64>()
                .sqrt();
            if norm > 1e-12 {
                layered.push(candidate.into_iter().map(|value| value / norm).collect());
            }
        }
        directions = select_diverse_reference_directions(layered, maximum_points);
    } else if directions.len() > maximum_points {
        let denominator = (maximum_points - 1) as f64;
        directions = (0..maximum_points)
            .map(|index| {
                let rank =
                    (index as f64 * (directions.len() - 1) as f64 / denominator).round() as usize;
                directions[rank.min(directions.len() - 1)].clone()
            })
            .collect();
        directions.dedup_by(|left, right| {
            left.iter()
                .zip(right)
                .all(|(a, b)| (*a - *b).abs() < f64::EPSILON)
        });
    }

    let mut selected: BTreeMap<usize, (f64, Vec<f64>)> = BTreeMap::new();
    for direction in &directions {
        let mut best: Option<(usize, f64)> = None;
        for (row, index) in indices.iter().enumerate() {
            let norm_squared: f64 = (0..dimensions)
                .map(|dimension| coordinates[dimension][row].powi(2))
                .sum();
            let dot: f64 = (0..dimensions)
                .map(|dimension| coordinates[dimension][row] * direction[dimension])
                .sum();
            let distance = (norm_squared - dot * dot).max(0.0);
            if best.map(|(_, current)| distance < current).unwrap_or(true) {
                best = Some((*index, distance));
            }
        }
        if let Some((index, distance)) = best {
            let replace = selected
                .get(&index)
                .map(|(current, _)| distance < *current)
                .unwrap_or(true);
            if replace {
                selected.insert(index, (distance, direction.clone()));
            }
        }
    }

    while selected.len() < maximum_points {
        let chosen: Vec<_> = selected.keys().copied().collect();
        let mut best_remaining: Option<(usize, f64)> = None;
        for (row, index) in indices.iter().enumerate() {
            if selected.contains_key(index) {
                continue;
            }
            let mut nearest_chosen = f64::INFINITY;
            for other in &chosen {
                let other_row = indices.iter().position(|candidate| candidate == other);
                if let Some(other_row) = other_row {
                    let distance: f64 = (0..dimensions)
                        .map(|dimension| {
                            (coordinates[dimension][row] - coordinates[dimension][other_row])
                                .powi(2)
                        })
                        .sum::<f64>()
                        .sqrt();
                    nearest_chosen = nearest_chosen.min(distance);
                }
            }
            if best_remaining
                .map(|(_, current)| nearest_chosen > current)
                .unwrap_or(true)
            {
                best_remaining = Some((*index, nearest_chosen));
            }
        }
        let Some((index, _)) = best_remaining else {
            break;
        };
        let row = indices.iter().position(|candidate| *candidate == index);
        let Some(row) = row else {
            break;
        };
        let norm_squared: f64 = (0..dimensions)
            .map(|dimension| coordinates[dimension][row].powi(2))
            .sum();
        let mut nearest_direction = directions[0].clone();
        let mut nearest_distance = f64::INFINITY;
        for direction in &directions {
            let dot: f64 = (0..dimensions)
                .map(|dimension| coordinates[dimension][row] * direction[dimension])
                .sum();
            let distance = (norm_squared - dot * dot).max(0.0);
            if distance < nearest_distance {
                nearest_distance = distance;
                nearest_direction = direction.clone();
            }
        }
        selected.insert(index, (nearest_distance, nearest_direction));
    }

    let selected_indices = selected.keys().copied().collect::<Vec<_>>();
    let associations = selected
        .into_iter()
        .map(|(index, (_, direction))| (index, direction))
        .collect();
    (selected_indices, associations)
}

fn select_diverse_reference_directions(directions: Vec<Vec<f64>>, count: usize) -> Vec<Vec<f64>> {
    if directions.len() <= count {
        return directions;
    }
    let dimensions = directions.first().map(Vec::len).unwrap_or(0);
    let mut selected: Vec<Vec<f64>> = Vec::with_capacity(count);
    for dimension in 0..dimensions {
        let mut basis = vec![0.0; dimensions];
        basis[dimension] = 1.0;
        let candidates = directions.iter().enumerate().filter(|(_, candidate)| {
            !selected.iter().any(|existing| {
                existing
                    .iter()
                    .zip(candidate.iter())
                    .all(|(left, right)| (*left - *right).abs() < 1e-12)
            })
        });
        if let Some((_, best)) = candidates.min_by(|(_, left), (_, right)| {
            let left_distance: f64 = left
                .iter()
                .zip(&basis)
                .map(|(value, target)| (value - target).powi(2))
                .sum();
            let right_distance: f64 = right
                .iter()
                .zip(&basis)
                .map(|(value, target)| (value - target).powi(2))
                .sum();
            left_distance
                .partial_cmp(&right_distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            selected.push(best.clone());
        }
    }

    while selected.len() < count {
        let mut best: Option<(usize, f64)> = None;
        for (index, candidate) in directions.iter().enumerate() {
            if selected.iter().any(|existing| {
                existing
                    .iter()
                    .zip(candidate.iter())
                    .all(|(left, right)| (*left - *right).abs() < 1e-12)
            }) {
                continue;
            }
            let nearest = selected
                .iter()
                .map(|existing| {
                    existing
                        .iter()
                        .zip(candidate.iter())
                        .map(|(left, right)| (left - right).powi(2))
                        .sum::<f64>()
                        .sqrt()
                })
                .fold(f64::INFINITY, f64::min);
            if best.map(|(_, current)| nearest > current).unwrap_or(true) {
                best = Some((index, nearest));
            }
        }
        let Some((index, _)) = best else {
            break;
        };
        selected.push(directions[index].clone());
    }
    selected
}

fn normalize_optimization_value(value: f64, specification: &OptimizationParameter) -> f64 {
    ((value - specification.lower) / (specification.upper - specification.lower)).clamp(0.0, 1.0)
}

pub(super) fn optimization_engineering_value(
    normalized: f64,
    specification: &OptimizationParameter,
) -> f64 {
    let span = specification.upper - specification.lower;
    let Some(quantum) = specification.quantum else {
        return specification.lower + normalized.clamp(0.0, 1.0) * span;
    };
    let steps = ((span / quantum) + 1e-10).floor();
    let step = (normalized.clamp(0.0, 1.0) * steps)
        .round()
        .clamp(0.0, steps);
    specification.lower + step * quantum
}

fn optimization_engineering_values(
    values: &[f64],
    specifications: &[OptimizationParameter],
) -> Vec<f64> {
    specifications
        .iter()
        .enumerate()
        .map(|(index, specification)| optimization_engineering_value(values[index], specification))
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn refine_quantized_optimum(
    circuit: &SpiceCircuit,
    parameters: &[(OptimizationParameter, SensitivityParameter, f64)],
    local_options: &SensitivityOptions,
    options: &OptimizationOptions,
    best_values: &mut [f64],
    best_result: &mut (SpiceCircuit, f64, f64),
    evaluations: &mut usize,
    cancellation: &FlowCancellation,
) -> Result<(), String> {
    if cancellation.is_cancelled() {
        return Err("optimization analysis cancelled".into());
    }
    let evaluate = |values: &[f64]| -> Result<(SpiceCircuit, f64, f64), String> {
        let actual_values = optimization_engineering_values(values, &options.parameters);
        let result = evaluate_optimization_candidate(
            circuit,
            parameters,
            &actual_values,
            local_options,
            options.objective,
            options.goal_value,
        )?;
        Ok(result)
    };

    // Differential evolution explores normalized coordinates, so a quantized
    // population can finish a few grid points away from the engineering
    // optimum. A deterministic axis scan finishes that population using only
    // feasible grid values. Small axes are exhaustive; large axes use binary
    // steps first and then refine to one quantum.
    for _ in 0..16 {
        if cancellation.is_cancelled() {
            return Err("optimization analysis cancelled".into());
        }
        let mut improved = false;
        for dimension in 0..best_values.len() {
            if cancellation.is_cancelled() {
                return Err("optimization analysis cancelled".into());
            }
            let specification = &options.parameters[dimension];
            let Some(quantum) = specification.quantum else {
                continue;
            };
            let steps = (((specification.upper - specification.lower) / quantum) + 1e-10).floor();
            if !(steps.is_finite() && steps >= 1.0) {
                continue;
            }
            let current_step = (best_values[dimension] * steps).round().clamp(0.0, steps);
            let mut candidate_step = current_step;
            let mut candidate_result = best_result.clone();
            let mut candidate_score = best_result.2;

            let try_step = |step: f64,
                            candidate_step: &mut f64,
                            candidate_result: &mut (SpiceCircuit, f64, f64),
                            candidate_score: &mut f64,
                            evaluations: &mut usize|
             -> Result<(), String> {
                if cancellation.is_cancelled() {
                    return Err("optimization analysis cancelled".into());
                }
                if !(0.0..=steps).contains(&step) || step == current_step {
                    return Ok(());
                }
                let mut values = best_values.to_vec();
                values[dimension] = step / steps;
                let result = evaluate(&values)?;
                *evaluations += 1;
                if result.2 < *candidate_score - 1e-15 {
                    *candidate_step = step;
                    *candidate_score = result.2;
                    *candidate_result = result;
                }
                Ok(())
            };

            let integer_steps = steps as usize;
            if integer_steps <= 8192 {
                for step in 0..=integer_steps {
                    try_step(
                        step as f64,
                        &mut candidate_step,
                        &mut candidate_result,
                        &mut candidate_score,
                        evaluations,
                    )?;
                }
            } else {
                let mut radius = 8192.0_f64;
                while radius >= 1.0 {
                    for offset in [radius, -radius] {
                        try_step(
                            current_step + offset,
                            &mut candidate_step,
                            &mut candidate_result,
                            &mut candidate_score,
                            evaluations,
                        )?;
                    }
                    radius /= 2.0;
                }
            }

            if candidate_score < best_result.2 - 1e-15 {
                best_values[dimension] = candidate_step / steps;
                *best_result = candidate_result;
                improved = true;
            }
        }
        if !improved {
            break;
        }
    }
    Ok(())
}

fn optimization_circuit(
    circuit: &SpiceCircuit,
    parameters: &[(OptimizationParameter, SensitivityParameter, f64)],
    values: &[f64],
) -> Result<SpiceCircuit, String> {
    let mut out = circuit.clone();
    for ((specification, nominal, _), target) in parameters.iter().zip(values) {
        let clamped = target.clamp(specification.lower, specification.upper);
        out = perturbed_for_sensitivity(&out, nominal, clamped - nominal.value)?;
    }
    Ok(out)
}

fn evaluate_optimization_candidate(
    circuit: &SpiceCircuit,
    parameters: &[(OptimizationParameter, SensitivityParameter, f64)],
    values: &[f64],
    local_options: &SensitivityOptions,
    objective: OptimizationObjective,
    goal_value: f64,
) -> Result<(SpiceCircuit, f64, f64), String> {
    let variant = optimization_circuit(circuit, parameters, values)?;
    let output = sensitivity_output(&variant, local_options)?;
    let score = match objective {
        OptimizationObjective::Match => (output - goal_value).abs(),
        OptimizationObjective::Minimize => output,
        OptimizationObjective::Maximize => -output,
    };
    Ok((variant, output, score))
}

fn distinct_optimization_indices(
    target: usize,
    len: usize,
    rng: &mut McRng,
) -> (usize, usize, usize) {
    let mut pick = || {
        let mut index = (rng.unit() * len as f64).floor() as usize;
        while index == target {
            index = (rng.unit() * len as f64).floor() as usize;
        }
        index.min(len - 1)
    };
    let a = pick();
    let mut b = pick();
    while b == a || b == target {
        b = pick();
    }
    let mut c = pick();
    while c == a || c == b || c == target {
        c = pick();
    }
    (a, b, c)
}
