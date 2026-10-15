//! Global Sobol/variance sensitivity analysis.

use super::super::random::*;
use super::parameters::*;
use crate::types::*;
use crate::FlowCancellation;

pub fn global_sensitivity(
    circuit: &SpiceCircuit,
    analysis: &Analysis,
) -> Result<GlobalSensitivityResult, String> {
    global_sensitivity_cancellable(circuit, analysis, &FlowCancellation::new())
}

/// Estimate global sensitivity with cooperative checks at every model sample.
pub fn global_sensitivity_cancellable(
    circuit: &SpiceCircuit,
    analysis: &Analysis,
    cancellation: &FlowCancellation,
) -> Result<GlobalSensitivityResult, String> {
    if cancellation.is_cancelled() {
        return Err("global sensitivity analysis cancelled".into());
    }
    let Analysis::GlobalSensitivity(options) = analysis else {
        return Err("global sensitivity called with a non-global analysis".into());
    };
    let local_options = SensitivityOptions {
        target: options.target,
        output_node: options.output_node.clone(),
        frequency_hz: options.frequency_hz,
        relative_step: 0.001,
    };
    let nominal_output = sensitivity_output(circuit, &local_options)?;
    let parameters: Vec<_> = sensitivity_parameters(circuit)
        .into_iter()
        .filter(|parameter| parameter.value.abs() > f64::EPSILON)
        .collect();
    if parameters.is_empty() {
        return Err("global sensitivity has no nonzero variable parameters".into());
    }

    let mut rng = McRng::new(options.seed);
    let correlation_matrix = global_correlation_matrix(&parameters, &options.correlations)?;
    let matrix_a = if options.correlations.is_empty() {
        random_factor_matrix(parameters.len(), options.samples, &mut rng)
    } else {
        correlated_factor_matrix(options.samples, &correlation_matrix, &mut rng)?
    };
    let mut unconditional = Vec::with_capacity(options.samples);
    for sample_row in &matrix_a {
        if cancellation.is_cancelled() {
            return Err("global sensitivity analysis cancelled".into());
        }
        let deltas: Vec<f64> = parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| parameter.value * options.tolerance * sample_row[index])
            .collect();
        let variant = global_variant(circuit, &parameters, &deltas)?;
        unconditional.push(sensitivity_output(&variant, &local_options)?);
    }
    let output_variance = sample_variance(&unconditional);
    if !(output_variance.is_finite() && output_variance > f64::EPSILON) {
        return Err("global sensitivity output has zero or non-finite variance".into());
    }

    let mut gradients = Vec::with_capacity(parameters.len());
    for parameter in &parameters {
        if cancellation.is_cancelled() {
            return Err("global sensitivity analysis cancelled".into());
        }
        let delta = sensitivity_perturbation(parameter, 0.001);
        let low = sensitivity_output(
            &perturbed_for_sensitivity(circuit, parameter, -delta)?,
            &local_options,
        )?;
        let high = sensitivity_output(
            &perturbed_for_sensitivity(circuit, parameter, delta)?,
            &local_options,
        )?;
        gradients.push((high - low) / (2.0 * delta));
    }
    let sigmas: Vec<f64> = parameters
        .iter()
        .map(|parameter| parameter.value.abs() * options.tolerance)
        .collect();
    let mut linearized_variance = 0.0_f64;
    let mut linear_contributions = vec![0.0; parameters.len()];
    for (left, left_gradient) in gradients.iter().enumerate() {
        for (right, right_gradient) in gradients.iter().enumerate() {
            let covariance = sigmas[left] * sigmas[right] * correlation_matrix[left][right];
            let term = left_gradient * right_gradient * covariance;
            linearized_variance += term;
            if left == right {
                linear_contributions[left] += term;
            } else if right > left {
                let shared = term;
                linear_contributions[left] += shared;
                linear_contributions[right] += shared;
            }
        }
    }

    let mut entries = Vec::with_capacity(parameters.len());
    for (parameter_index, parameter) in parameters.iter().enumerate() {
        if cancellation.is_cancelled() {
            return Err("global sensitivity analysis cancelled".into());
        }
        let mut only_this = Vec::with_capacity(options.samples);
        let mut without_this = Vec::with_capacity(options.samples);
        for sample_row in &matrix_a {
            if cancellation.is_cancelled() {
                return Err("global sensitivity analysis cancelled".into());
            }
            let only_deltas: Vec<f64> = parameters
                .iter()
                .enumerate()
                .map(|(index, candidate)| {
                    if index == parameter_index {
                        candidate.value * options.tolerance * sample_row[index]
                    } else {
                        0.0
                    }
                })
                .collect();
            only_this.push(sensitivity_output(
                &global_variant(circuit, &parameters, &only_deltas)?,
                &local_options,
            )?);

            let without_deltas: Vec<f64> = parameters
                .iter()
                .enumerate()
                .map(|(index, candidate)| {
                    if index == parameter_index {
                        0.0
                    } else {
                        candidate.value * options.tolerance * sample_row[index]
                    }
                })
                .collect();
            without_this.push(sensitivity_output(
                &global_variant(circuit, &parameters, &without_deltas)?,
                &local_options,
            )?);
        }

        let (total_order_index, first_order_index) = if options.correlations.is_empty() {
            (
                sample_variance(&only_this) / output_variance,
                1.0 - sample_variance(&without_this) / output_variance,
            )
        } else if linearized_variance.abs() > f64::EPSILON {
            (
                linear_contributions[parameter_index] / linearized_variance,
                gradients[parameter_index].powi(2) * sigmas[parameter_index].powi(2)
                    / linearized_variance,
            )
        } else {
            (0.0, 0.0)
        };
        if !first_order_index.is_finite() || !total_order_index.is_finite() {
            return Err(format!(
                "global sensitivity index for '{}' is non-finite",
                parameter.name
            ));
        }
        entries.push(GlobalSensitivityEntry {
            parameter: parameter.name.clone(),
            object: parameter.object.clone(),
            nominal_parameter: parameter.value,
            first_order_index,
            total_order_index,
            linear_variance_contribution: linear_contributions[parameter_index],
        });
    }

    Ok(GlobalSensitivityResult {
        target: options.target,
        output_node: options.output_node.clone(),
        nominal_output,
        output_variance,
        linearized_variance,
        entries,
    })
}
