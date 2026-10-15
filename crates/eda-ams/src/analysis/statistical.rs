//! Monte Carlo sampling and correlated statistical variants.

use super::derivatives::{sensitivity_parameters, SensitivityParameter, SensitivityParameterKind};
use super::random::*;
use super::{ac_sweep, dc_operating_point, transient_cancellable};
use crate::types::*;
use crate::FlowCancellation;
use std::collections::BTreeMap;

struct MonteCarloCovarianceModel {
    parameters: Vec<SensitivityParameter>,
    nominal_values: Vec<f64>,
    lower: Vec<Vec<f64>>,
    lognormal: bool,
}

#[derive(Clone, Debug)]
struct StatisticalModelSampler {
    parameters: Vec<SensitivityParameter>,
    specifications: Vec<StatisticalParameterModel>,
    lower: Vec<Vec<f64>>,
}

pub fn monte_carlo(
    circuit: &SpiceCircuit,
    analysis: &Analysis,
) -> Result<MonteCarloResult, String> {
    monte_carlo_cancellable(circuit, analysis, &FlowCancellation::new())
}

/// Run Monte Carlo with cooperative checks at every sample.
pub fn monte_carlo_cancellable(
    circuit: &SpiceCircuit,
    analysis: &Analysis,
    cancellation: &FlowCancellation,
) -> Result<MonteCarloResult, String> {
    let Analysis::MonteCarlo(options) = analysis else {
        return Err("Monte Carlo called with a non-Monte-Carlo analysis".into());
    };
    circuit.validate()?;
    if cancellation.is_cancelled() {
        return Err("Monte Carlo analysis cancelled".into());
    }
    let mut rng = McRng::new(options.seed);
    let mut samples = Vec::with_capacity(options.runs);
    let statistical_model = monte_carlo_statistical_model(circuit, options)?;
    let covariance_model = monte_carlo_covariance_model(circuit, options)?;
    let correlations = monte_carlo_correlations(circuit, options);
    let correlated_parameters = if correlations.is_empty() {
        None
    } else {
        let parameters = sensitivity_parameters(circuit);
        let correlation = global_correlation_matrix(&parameters, &correlations)?;
        let lower = cholesky_correlation(&correlation)?;
        Some((parameters, lower))
    };

    for index in 0..options.runs {
        if cancellation.is_cancelled() {
            return Err("Monte Carlo analysis cancelled".into());
        }
        let (variant, randomized_objects) = if let Some(model) = statistical_model.as_ref() {
            let factors = correlated_factor_row(&model.lower, &mut rng);
            let deltas = model
                .parameters
                .iter()
                .zip(&model.specifications)
                .zip(&factors)
                .map(|((parameter, specification), factor)| {
                    statistical_parameter_delta(parameter, specification, *factor)
                })
                .collect::<Vec<_>>();
            let randomized_objects = deltas.iter().filter(|delta| delta.abs() > 1e-15).count();
            (
                global_variant(circuit, &model.parameters, &deltas)?,
                randomized_objects,
            )
        } else if let Some(model) = covariance_model.as_ref() {
            {
                let deltas = monte_carlo_covariance_deltas(model, &mut rng);
                let randomized_objects = deltas.iter().filter(|delta| delta.abs() > 1e-15).count();
                (
                    global_variant(circuit, &model.parameters, &deltas)?,
                    randomized_objects,
                )
            }
        } else {
            match &correlated_parameters {
                Some((parameters, lower)) => {
                    let factors = correlated_factor_row(lower, &mut rng);
                    let deltas: Vec<_> = parameters
                        .iter()
                        .zip(&factors)
                        .map(|(parameter, factor)| {
                            monte_carlo_parameter_delta(parameter, *factor, options)
                        })
                        .collect();
                    let randomized_objects = parameters
                        .iter()
                        .zip(&deltas)
                        .filter(|(parameter, delta)| {
                            monte_carlo_parameter_delta(parameter, 1.0, options).abs()
                                > f64::EPSILON
                                && delta.abs() > f64::EPSILON
                        })
                        .count();
                    (
                        global_variant(circuit, parameters, &deltas)?,
                        randomized_objects,
                    )
                }
                None => randomized_circuit(circuit, options, &mut rng)?,
            }
        };
        let value = match options.target {
            MonteCarloTarget::OperatingPoint => dc_operating_point(&variant)?
                .node_voltages
                .get(&options.output_node)
                .copied()
                .ok_or_else(|| format!("unknown Monte Carlo node '{}'", options.output_node))?,
            MonteCarloTarget::TransientFinal => {
                let base = circuit
                    .analyses
                    .iter()
                    .find_map(|analysis| match analysis {
                        Analysis::Transient { .. } => Some(analysis.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| "Monte Carlo transient target lacks .tran".to_string())?;
                transient_cancellable(&variant, &base, cancellation)?
                    .final_sample()
                    .and_then(|sample| sample.node_voltages.get(&options.output_node))
                    .copied()
                    .ok_or_else(|| format!("unknown Monte Carlo node '{}'", options.output_node))?
            }
            MonteCarloTarget::AcMagnitude => {
                let base = circuit
                    .analyses
                    .iter()
                    .find_map(|analysis| match analysis {
                        Analysis::AcSweep { .. } => Some(analysis.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| "Monte Carlo AC target lacks .ac".to_string())?;
                let result = ac_sweep(&variant, &base)?;
                result
                    .points
                    .iter()
                    .min_by(|a, b| {
                        (a.frequency_hz - options.frequency_hz)
                            .abs()
                            .partial_cmp(&(b.frequency_hz - options.frequency_hz).abs())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .and_then(|point| point.node_phasors.get(&options.output_node))
                    .map(Phasor::magnitude)
                    .ok_or_else(|| format!("unknown Monte Carlo node '{}'", options.output_node))?
            }
        };
        if !value.is_finite() {
            return Err(format!("Monte Carlo sample {index} is non-finite"));
        }
        samples.push(MonteCarloSample {
            index,
            value,
            randomized_objects,
        });
    }

    let statistics = monte_carlo_statistics(&samples);
    Ok(MonteCarloResult {
        target: options.target,
        output_node: options.output_node.clone(),
        samples,
        statistics,
    })
}

fn monte_carlo_statistical_model(
    circuit: &SpiceCircuit,
    options: &MonteCarloOptions,
) -> Result<Option<StatisticalModelSampler>, String> {
    let Some(model_name) = &options.statistical_model else {
        return Ok(None);
    };
    let model = circuit
        .statistical_models
        .get(model_name)
        .ok_or_else(|| format!("Monte Carlo statistical model '{model_name}' does not exist"))?;
    let available = sensitivity_parameters(circuit);
    let mut parameters = Vec::with_capacity(model.parameters.len());
    let mut specifications = Vec::with_capacity(model.parameters.len());
    for specification in &model.parameters {
        let parameter = available
            .iter()
            .find(|candidate| {
                candidate
                    .name
                    .eq_ignore_ascii_case(&specification.parameter)
            })
            .ok_or_else(|| {
                format!(
                    "statistical model parameter '{}' does not exist",
                    specification.parameter
                )
            })?
            .clone();
        parameters.push(parameter);
        specifications.push(specification.clone());
    }
    let correlation = global_correlation_matrix(&parameters, &model.correlations)?;
    let lower = cholesky_correlation(&correlation)?;
    Ok(Some(StatisticalModelSampler {
        parameters,
        specifications,
        lower,
    }))
}

fn statistical_parameter_delta(
    parameter: &SensitivityParameter,
    specification: &StatisticalParameterModel,
    uniform: f64,
) -> f64 {
    let scale = if specification.relative_sigma {
        parameter.value
    } else {
        1.0
    };
    match specification.distribution {
        MonteCarloDistribution::Uniform => scale * specification.sigma * uniform,
        MonteCarloDistribution::Normal | MonteCarloDistribution::Lognormal => {
            let standard = monte_carlo_standard_factor(uniform, specification.distribution);
            match specification.distribution {
                MonteCarloDistribution::Normal => scale * specification.sigma * standard,
                MonteCarloDistribution::Lognormal => {
                    parameter.value * (specification.sigma * standard).exp_m1()
                }
                MonteCarloDistribution::Uniform => unreachable!(),
            }
        }
    }
}

fn monte_carlo_covariance_model(
    circuit: &SpiceCircuit,
    options: &MonteCarloOptions,
) -> Result<Option<MonteCarloCovarianceModel>, String> {
    if options.covariances.is_empty() {
        return Ok(None);
    }
    let all_parameters = sensitivity_parameters(circuit);
    let parameter_index = |name: &str| {
        all_parameters
            .iter()
            .position(|parameter| parameter.name.eq_ignore_ascii_case(name))
    };
    let mut diagonal = BTreeMap::<usize, f64>::new();
    let mut off_diagonal: Vec<(usize, usize, f64)> = Vec::new();
    for covariance in &options.covariances {
        let left = parameter_index(&covariance.left).ok_or_else(|| {
            format!(
                "unknown Monte Carlo covariance parameter '{}'",
                covariance.left
            )
        })?;
        let right = parameter_index(&covariance.right).ok_or_else(|| {
            format!(
                "unknown Monte Carlo covariance parameter '{}'",
                covariance.right
            )
        })?;
        if left == right {
            if let Some(existing) = diagonal.get(&left) {
                if (*existing - covariance.covariance).abs() > 1e-12 {
                    return Err(format!(
                        "conflicting diagonal covariance for '{}'",
                        covariance.left
                    ));
                }
            }
            diagonal.insert(left, covariance.covariance);
        } else {
            if !diagonal.contains_key(&left) || !diagonal.contains_key(&right) {
                return Err(format!(
                    "off-diagonal covariance for '{}|{}' lacks both diagonal entries",
                    covariance.left, covariance.right
                ));
            }
            let conflicting = off_diagonal
                .iter()
                .any(|(existing_left, existing_right, value)| {
                    ((*existing_left == left && *existing_right == right)
                        || (*existing_left == right && *existing_right == left))
                        && (*value - covariance.covariance).abs() > 1e-12
                });
            if conflicting {
                return Err(format!(
                    "conflicting covariance for '{}|{}'",
                    covariance.left, covariance.right
                ));
            }
            off_diagonal.push((left, right, covariance.covariance));
        }
    }
    if diagonal.is_empty() {
        return Err("Monte Carlo covariance input has no diagonal entries".into());
    }
    let indices: Vec<_> = diagonal.keys().copied().collect();
    let parameters: Vec<_> = indices
        .iter()
        .map(|index| all_parameters[*index].clone())
        .collect();
    let nominal_values = parameters.iter().map(|parameter| parameter.value).collect();
    let mut natural = vec![vec![0.0; indices.len()]; indices.len()];
    for (row, left) in indices.iter().enumerate() {
        natural[row][row] = diagonal[left];
    }
    for (left, right, value) in off_diagonal {
        let row = indices
            .iter()
            .position(|index| *index == left)
            .ok_or_else(|| "internal covariance index mismatch".to_string())?;
        let column = indices
            .iter()
            .position(|index| *index == right)
            .ok_or_else(|| "internal covariance index mismatch".to_string())?;
        natural[row][column] = value;
        natural[column][row] = value;
    }
    let matrix = if matches!(options.distribution, MonteCarloDistribution::Lognormal) {
        let mut logarithmic = vec![vec![0.0; indices.len()]; indices.len()];
        for row in 0..natural.len() {
            for column in 0..natural.len() {
                let denominator =
                    all_parameters[indices[row]].value * all_parameters[indices[column]].value;
                let ratio = 1.0 + natural[row][column] / denominator;
                if ratio <= 0.0 {
                    return Err("Monte Carlo covariance is invalid for lognormal sampling".into());
                }
                logarithmic[row][column] = ratio.ln();
            }
        }
        logarithmic
    } else {
        natural
    };
    let lower = cholesky_covariance(&matrix)?;
    Ok(Some(MonteCarloCovarianceModel {
        parameters,
        nominal_values,
        lower,
        lognormal: matches!(options.distribution, MonteCarloDistribution::Lognormal),
    }))
}

fn monte_carlo_covariance_deltas(model: &MonteCarloCovarianceModel, rng: &mut McRng) -> Vec<f64> {
    let independent: Vec<_> = (0..model.lower.len())
        .map(|_| standard_normal(rng))
        .collect();
    (0..model.lower.len())
        .map(|row| {
            let correlated: f64 = (0..=row)
                .map(|source| model.lower[row][source] * independent[source])
                .sum();
            if model.lognormal {
                model.nominal_values[row] * correlated.exp_m1()
            } else {
                correlated
            }
        })
        .collect()
}

fn monte_carlo_correlations(
    circuit: &SpiceCircuit,
    options: &MonteCarloOptions,
) -> Vec<ParameterCorrelation> {
    let mut out = options.correlations.clone();
    if options.spatial_correlation <= 0.0 {
        return out;
    }
    let parameters = sensitivity_parameters(circuit);
    for (left_index, left) in parameters.iter().enumerate() {
        let Some(left_location) = circuit.locations.get(&left.object) else {
            continue;
        };
        for right in parameters.iter().skip(left_index + 1) {
            let Some(right_location) = circuit.locations.get(&right.object) else {
                continue;
            };
            let distance_squared = (left_location.x_m - right_location.x_m).powi(2)
                + (left_location.y_m - right_location.y_m).powi(2);
            let coefficient = options.spatial_correlation
                * (-distance_squared / (2.0 * options.spatial_correlation_length_m.powi(2))).exp();
            let already_specified = out.iter().any(|correlation| {
                (correlation.left.eq_ignore_ascii_case(&left.name)
                    && correlation.right.eq_ignore_ascii_case(&right.name))
                    || (correlation.left.eq_ignore_ascii_case(&right.name)
                        && correlation.right.eq_ignore_ascii_case(&left.name))
            });
            if !already_specified {
                out.push(ParameterCorrelation {
                    left: left.name.clone(),
                    right: right.name.clone(),
                    coefficient,
                });
            }
        }
    }
    out
}

fn monte_carlo_parameter_delta(
    parameter: &SensitivityParameter,
    factor: f64,
    options: &MonteCarloOptions,
) -> f64 {
    let standard_factor = monte_carlo_standard_factor(factor, options.distribution);
    match parameter.kind {
        SensitivityParameterKind::Resistance => {
            parameter.value
                * monte_carlo_relative_multiplier(
                    factor,
                    options.resistor_tolerance,
                    options.distribution,
                )
        }
        SensitivityParameterKind::Capacitance => {
            parameter.value
                * monte_carlo_relative_multiplier(
                    factor,
                    options.capacitor_tolerance,
                    options.distribution,
                )
        }
        SensitivityParameterKind::Inductance => {
            parameter.value
                * monte_carlo_relative_multiplier(
                    factor,
                    options.inductor_tolerance,
                    options.distribution,
                )
        }
        SensitivityParameterKind::DiodeIs => {
            parameter.value * (10f64.powf(options.diode_is_decades * standard_factor) - 1.0)
        }
        SensitivityParameterKind::MosVto => options.mos_vto_tolerance * standard_factor,
        SensitivityParameterKind::MosKp => {
            parameter.value
                * monte_carlo_relative_multiplier(
                    factor,
                    options.mos_kp_tolerance,
                    options.distribution,
                )
        }
        SensitivityParameterKind::BjtBeta => {
            parameter.value
                * monte_carlo_relative_multiplier(
                    factor,
                    options.bjt_beta_tolerance,
                    options.distribution,
                )
        }
        SensitivityParameterKind::BjtIs => {
            parameter.value * (10f64.powf(options.bjt_is_decades * standard_factor) - 1.0)
        }
        SensitivityParameterKind::DcSource
        | SensitivityParameterKind::AcSource
        | SensitivityParameterKind::BjtArea
        | SensitivityParameterKind::Bsim3Vth0
        | SensitivityParameterKind::Bsim3K1
        | SensitivityParameterKind::Bsim3U0
        | SensitivityParameterKind::Bsim3Tox
        | SensitivityParameterKind::Bsim3Vsat
        | SensitivityParameterKind::Bsim3Dvt0
        | SensitivityParameterKind::Bsim3Dvt1
        | SensitivityParameterKind::Bsim3Dvt2
        | SensitivityParameterKind::Bsim3Dvt0w
        | SensitivityParameterKind::Bsim3Dvt1w
        | SensitivityParameterKind::Bsim3W0
        | SensitivityParameterKind::Bsim3Kt1
        | SensitivityParameterKind::Bsim3Kt2
        | SensitivityParameterKind::Bsim3Kt1l => 0.0,
    }
}

pub(super) fn monte_carlo_standard_factor(
    uniform: f64,
    distribution: MonteCarloDistribution,
) -> f64 {
    match distribution {
        MonteCarloDistribution::Uniform => uniform,
        MonteCarloDistribution::Normal | MonteCarloDistribution::Lognormal => {
            let probability = ((uniform.clamp(-1.0, 1.0) + 1.0) / 2.0).clamp(1e-12, 1.0 - 1e-12);
            normal_inverse_cdf(probability).clamp(-3.0, 3.0)
        }
    }
}

pub(super) fn monte_carlo_relative_multiplier(
    uniform: f64,
    tolerance: f64,
    distribution: MonteCarloDistribution,
) -> f64 {
    match distribution {
        MonteCarloDistribution::Uniform => tolerance * uniform,
        MonteCarloDistribution::Normal => {
            tolerance * monte_carlo_standard_factor(uniform, distribution)
        }
        MonteCarloDistribution::Lognormal => {
            (tolerance * monte_carlo_standard_factor(uniform, distribution)).exp_m1()
        }
    }
}
