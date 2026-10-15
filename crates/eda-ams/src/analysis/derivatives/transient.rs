//! Transient final-value first and second derivatives.

use super::super::devices::*;
use super::super::solver::*;
use super::super::transient;
use super::parameters::*;
use crate::numeric::solve_real;
use crate::types::*;

pub(super) fn transient_final_derivative_analysis(
    circuit: &SpiceCircuit,
    options: &DerivativeOptions,
) -> Result<DerivativeResult, String> {
    let stop_s = circuit
        .analyses
        .iter()
        .find_map(|analysis| match analysis {
            Analysis::Transient { stop_s, .. } => Some(*stop_s),
            _ => None,
        })
        .ok_or_else(|| "transient derivative target lacks .tran".to_string())?;

    let resistors: Vec<_> = circuit
        .elements
        .iter()
        .filter(|element| matches!(element.kind, SpiceElementKind::Resistor { .. }))
        .collect();
    let capacitors: Vec<_> = circuit
        .elements
        .iter()
        .filter(|element| matches!(element.kind, SpiceElementKind::Capacitor { .. }))
        .collect();
    let sources: Vec<_> = circuit
        .elements
        .iter()
        .filter(|element| {
            matches!(
                &element.kind,
                SpiceElementKind::VoltageSource {
                    waveform: SourceWaveform::Dc(_),
                    ..
                }
            )
        })
        .collect();
    if resistors.len() != 1
        || capacitors.len() != 1
        || sources.len() != 1
        || circuit.elements.len() != 3
    {
        return general_linear_transient_derivative_analysis(circuit, options);
    }

    let resistor = resistors[0];
    let capacitor = capacitors[0];
    let source = sources[0];
    let SpiceElementKind::Resistor { resistance, .. } = &resistor.kind else {
        unreachable!()
    };
    let SpiceElementKind::Capacitor {
        capacitance,
        initial_voltage,
    } = &capacitor.kind
    else {
        unreachable!()
    };
    let SpiceElementKind::VoltageSource {
        waveform: SourceWaveform::Dc(source_value),
        ..
    } = &source.kind
    else {
        unreachable!()
    };

    let input = source.nodes[0].clone();
    let ground = source.nodes[1].clone();
    let output = capacitor.nodes[0].clone();
    let capacitor_ground = capacitor.nodes[1].clone();
    if !GROUND_ALIASES.contains(&ground.as_str())
        || !GROUND_ALIASES.contains(&capacitor_ground.as_str())
        || resistor.nodes != [input.as_str(), output.as_str()]
        || output != options.output_node
    {
        return Err(
            "analytic transient derivatives require Vin--R--V(out),C(out--ground) topology".into(),
        );
    }

    let time_constant_s = stop_s / (resistance * capacitance);
    let decay = (-time_constant_s).exp();
    let initial_difference = initial_voltage - source_value;
    let nominal_output = source_value + initial_difference * decay;
    let mut entries = Vec::with_capacity(options.parameters.len());

    for name in &options.parameters {
        let (parameter, value, gradient) = if name
            .eq_ignore_ascii_case(&format!("{}.R", resistor.name))
        {
            (
                format!("{}.R", resistor.name),
                *resistance,
                -initial_difference * decay * time_constant_s / resistance,
            )
        } else if name.eq_ignore_ascii_case(&format!("{}.C", capacitor.name)) {
            (
                format!("{}.C", capacitor.name),
                *capacitance,
                -initial_difference * decay * time_constant_s / capacitance,
            )
        } else if name.eq_ignore_ascii_case(&format!("{}.DC", source.name)) {
            (format!("{}.DC", source.name), *source_value, 1.0 - decay)
        } else {
            return Err(format!(
                "transient derivative parameter '{name}' must be the series R, C, or source DC value"
            ));
        };
        entries.push(DerivativeEntry {
            parameter,
            nominal_parameter: value,
            gradient,
            hessian_row: Vec::with_capacity(options.parameters.len()),
        });
    }

    let second_common = initial_difference * decay * time_constant_s * (time_constant_s - 2.0);
    let second_rc =
        initial_difference * decay * time_constant_s * time_constant_s * (time_constant_s - 1.0)
            / stop_s;
    let hessians = [
        [
            second_common / (resistance * resistance),
            second_rc,
            -decay * time_constant_s / resistance,
        ],
        [
            second_rc,
            second_common / (capacitance * capacitance),
            -decay * time_constant_s / capacitance,
        ],
        [
            -decay * time_constant_s / resistance,
            -decay * time_constant_s / capacitance,
            0.0,
        ],
    ];
    for entry in &mut entries {
        let row = if entry
            .parameter
            .eq_ignore_ascii_case(&format!("{}.R", resistor.name))
        {
            0
        } else if entry
            .parameter
            .eq_ignore_ascii_case(&format!("{}.C", capacitor.name))
        {
            1
        } else {
            2
        };
        for column in 0..options.parameters.len() {
            let parameter = &options.parameters[column];
            let column = if parameter.eq_ignore_ascii_case(&format!("{}.R", resistor.name)) {
                0
            } else if parameter.eq_ignore_ascii_case(&format!("{}.C", capacitor.name)) {
                1
            } else {
                2
            };
            entry.hessian_row.push(hessians[row][column]);
        }
    }

    Ok(DerivativeResult {
        target: DerivativeTarget::TransientFinal,
        frequency_hz: options.frequency_hz,
        output_node: options.output_node.clone(),
        nominal_output,
        entries,
    })
}

fn general_linear_transient_derivative_analysis(
    circuit: &SpiceCircuit,
    options: &DerivativeOptions,
) -> Result<DerivativeResult, String> {
    let analysis = circuit
        .analyses
        .iter()
        .find(|analysis| matches!(analysis, Analysis::Transient { .. }))
        .ok_or_else(|| "transient derivative target lacks .tran".to_string())?;
    let Analysis::Transient { step_s, method, .. } = analysis else {
        unreachable!()
    };
    let available = sensitivity_parameters(circuit);
    let mut parameters = Vec::with_capacity(options.parameters.len());
    for name in &options.parameters {
        let parameter = available
            .iter()
            .find(|parameter| parameter.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("unknown transient derivative parameter '{name}'"))?;
        if !matches!(
            parameter.kind,
            SensitivityParameterKind::Resistance
                | SensitivityParameterKind::Capacitance
                | SensitivityParameterKind::Inductance
                | SensitivityParameterKind::DcSource
        ) {
            return Err(format!(
                "transient derivative parameter '{name}' must be R, C, L, or a DC source value"
            ));
        }
        parameters.push(parameter.clone());
    }

    let result = transient(circuit, analysis)?;
    let samples = &result.samples;
    let system = SystemModel::new(circuit)?;
    let size = system.size;
    let capacitors: Vec<_> = circuit
        .elements
        .iter()
        .filter(|element| matches!(element.kind, SpiceElementKind::Capacitor { .. }))
        .map(|element| {
            let SpiceElementKind::Capacitor { capacitance, .. } = &element.kind else {
                unreachable!()
            };
            (
                element.name.clone(),
                *capacitance,
                element.nodes[0].clone(),
                element.nodes[1].clone(),
            )
        })
        .collect();
    let augmented_size = size + capacitors.len();
    let dimensions = parameters.len();
    let mut previous_state: Option<Vec<f64>> = None;
    let mut previous_first: Option<Vec<Vec<f64>>> = None;
    let mut previous_second: Option<Vec<Vec<Vec<f64>>>> = None;
    let mut final_state = Vec::new();
    let mut final_first = Vec::new();
    let mut final_second = Vec::new();

    for (index, sample) in samples.iter().enumerate() {
        let previous = if index == 0 {
            None
        } else {
            Some(&samples[index - 1])
        };
        let mut state = vec![0.0; size];
        for (node, voltage) in &sample.node_voltages {
            if let Some(row) = system.node_index(node) {
                state[row] = *voltage;
            }
        }
        for (name, current) in &sample.branch_currents {
            if let Some(row) = system.branch_index(name) {
                state[row] = *current;
            }
        }
        let (matrix, rhs) = build_real_system(
            circuit,
            &system,
            sample.time_s,
            Some(*step_s),
            None,
            *method,
            previous,
            &state,
        )?;
        // Re-solve the linear step so the derivative recurrence uses exactly
        // the same matrix state as the reported transient sample.
        let state = solve_real(matrix.clone(), rhs)?;
        let mut first = vec![vec![0.0; size]; dimensions];
        let mut second = vec![vec![vec![0.0; size]; dimensions]; dimensions];
        let mut derivative_systems = Vec::with_capacity(dimensions);

        for parameter in &parameters {
            derivative_systems.push(transient_derivative_system(
                circuit,
                &system,
                *step_s,
                *method,
                previous,
                parameter,
                size,
                augmented_size,
            )?);
        }
        for (left, left_system) in derivative_systems.iter().enumerate() {
            let mut tangent_rhs = left_system.1.clone();
            if let (Some(prev_state), Some(prev_first)) =
                (previous_state.as_ref(), previous_first.as_ref())
            {
                add_matrix_vector(&mut tangent_rhs, &left_system.2, &prev_first[left]);
                add_scaled_vector(&mut tangent_rhs, &left_system.3, prev_state, 1.0);
            }
            add_matrix_vector_scaled_subtract(&mut tangent_rhs, &left_system.0, &state, 1.0);
            first[left] = solve_real(matrix.clone(), tangent_rhs)?;
        }
        for (left, left_system) in derivative_systems.iter().enumerate() {
            for (right, right_system) in derivative_systems.iter().enumerate() {
                let mut hessian_rhs = vec![0.0; size];
                add_matrix_vector_scaled(&mut hessian_rhs, &left_system.0, &first[right], -1.0);
                add_matrix_vector_scaled(&mut hessian_rhs, &right_system.0, &first[left], -1.0);
                if left == right {
                    add_matrix_vector_scaled(&mut hessian_rhs, &left_system.4, &state, -1.0);
                }
                if let (Some(prev_first), Some(_)) =
                    (previous_first.as_ref(), previous_second.as_ref())
                {
                    if let Some(prev_second) = previous_second.as_ref() {
                        add_matrix_vector(
                            &mut hessian_rhs,
                            &left_system.2,
                            &prev_second[left][right],
                        );
                    }
                    add_matrix_vector_scaled(
                        &mut hessian_rhs,
                        &left_system.3,
                        &prev_first[right],
                        1.0,
                    );
                    add_matrix_vector_scaled(
                        &mut hessian_rhs,
                        &right_system.3,
                        &prev_first[left],
                        1.0,
                    );
                }
                second[left][right] = solve_real(matrix.clone(), hessian_rhs)?;
            }
        }
        let (augmented_state, augmented_first, augmented_second) =
            augment_transient_derivative_state(
                &system,
                *method,
                *step_s,
                &capacitors,
                &parameters,
                &state,
                &first,
                &second,
                previous_state.as_ref(),
                previous_first.as_ref(),
                previous_second.as_ref(),
            );
        previous_state = Some(augmented_state);
        previous_first = Some(augmented_first);
        previous_second = Some(augmented_second);
        final_state = state;
        final_first = first;
        final_second = second;
    }

    let output_index = system
        .node_index(&options.output_node)
        .ok_or_else(|| format!("unknown derivative node '{}'", options.output_node))?;
    let mut entries = Vec::with_capacity(dimensions);
    for (index, parameter) in parameters.iter().enumerate() {
        entries.push(DerivativeEntry {
            parameter: parameter.name.clone(),
            nominal_parameter: parameter.value,
            gradient: final_first[index][output_index],
            hessian_row: final_second[index]
                .iter()
                .map(|values| values[output_index])
                .collect(),
        });
    }
    Ok(DerivativeResult {
        target: DerivativeTarget::TransientFinal,
        frequency_hz: options.frequency_hz,
        output_node: options.output_node.clone(),
        nominal_output: final_state[output_index],
        entries,
    })
}

type TransientCapacitor = (String, f64, String, String);
type AugmentedTransientDerivatives = (Vec<f64>, Vec<Vec<f64>>, Vec<Vec<Vec<f64>>>);

#[allow(clippy::too_many_arguments)]
fn augment_transient_derivative_state(
    system: &SystemModel,
    method: IntegrationMethod,
    step_s: f64,
    capacitors: &[TransientCapacitor],
    parameters: &[SensitivityParameter],
    state: &[f64],
    first: &[Vec<f64>],
    second: &[Vec<Vec<f64>>],
    previous_state: Option<&Vec<f64>>,
    previous_first: Option<&Vec<Vec<f64>>>,
    previous_second: Option<&Vec<Vec<Vec<f64>>>>,
) -> AugmentedTransientDerivatives {
    let size = system.size;
    let augmented_size = size + capacitors.len();
    let dimensions = parameters.len();
    let mut augmented_state = vec![0.0; augmented_size];
    augmented_state[..size].copy_from_slice(state);
    let mut augmented_first = vec![vec![0.0; augmented_size]; dimensions];
    let mut augmented_second = vec![vec![vec![0.0; augmented_size]; dimensions]; dimensions];
    for (index, vector) in first.iter().enumerate() {
        augmented_first[index][..size].copy_from_slice(vector);
    }
    for (left, vectors) in second.iter().enumerate() {
        for (right, vector) in vectors.iter().enumerate() {
            augmented_second[left][right][..size].copy_from_slice(vector);
        }
    }

    let Some(previous_state) = previous_state else {
        return (augmented_state, augmented_first, augmented_second);
    };
    let (Some(previous_first), Some(previous_second)) = (previous_first, previous_second) else {
        return (augmented_state, augmented_first, augmented_second);
    };
    let integration_factor = transient_integration_factor(method);
    let trapezoidal = method == IntegrationMethod::Trapezoidal;

    for (capacitor_index, capacitor) in capacitors.iter().enumerate() {
        let output_column = size + capacitor_index;
        let plus_index = system.node_index(&capacitor.2);
        let minus_index = system.node_index(&capacitor.3);
        let difference = |vector: &[f64]| {
            plus_index.map(|index| vector[index]).unwrap_or(0.0)
                - minus_index.map(|index| vector[index]).unwrap_or(0.0)
        };
        let conductance = capacitor.1 * integration_factor / step_s;
        let delta = difference(state) - difference(previous_state);
        let previous_current = if trapezoidal {
            previous_state[output_column]
        } else {
            0.0
        };
        augmented_state[output_column] = conductance * delta - previous_current;

        for left in 0..dimensions {
            let conductance_derivative =
                if parameters[left].object.eq_ignore_ascii_case(&capacitor.0)
                    && parameters[left].kind == SensitivityParameterKind::Capacitance
                {
                    integration_factor / step_s
                } else {
                    0.0
                };
            let delta_first = difference(&first[left]) - difference(&previous_first[left]);
            let previous_current_first = if trapezoidal {
                previous_first[left][output_column]
            } else {
                0.0
            };
            augmented_first[left][output_column] =
                conductance_derivative * delta + conductance * delta_first - previous_current_first;

            for right in 0..dimensions {
                let right_conductance_derivative =
                    if parameters[right].object.eq_ignore_ascii_case(&capacitor.0)
                        && parameters[right].kind == SensitivityParameterKind::Capacitance
                    {
                        integration_factor / step_s
                    } else {
                        0.0
                    };
                let delta_second =
                    difference(&second[left][right]) - difference(&previous_second[left][right]);
                let previous_current_second = if trapezoidal {
                    previous_second[left][right][output_column]
                } else {
                    0.0
                };
                augmented_second[left][right][output_column] = conductance_derivative
                    * (difference(&first[right]) - difference(&previous_first[right]))
                    + right_conductance_derivative * delta_first
                    + conductance * delta_second
                    - previous_current_second;
            }
        }
    }
    (augmented_state, augmented_first, augmented_second)
}

type TransientDerivativeSystem = (
    Vec<Vec<f64>>,
    Vec<f64>,
    Vec<Vec<f64>>,
    Vec<Vec<f64>>,
    Vec<Vec<f64>>,
);

#[allow(clippy::too_many_arguments)]
fn transient_derivative_system(
    circuit: &SpiceCircuit,
    system: &SystemModel,
    step_s: f64,
    method: IntegrationMethod,
    previous: Option<&TransientSample>,
    parameter: &SensitivityParameter,
    size: usize,
    augmented_size: usize,
) -> Result<TransientDerivativeSystem, String> {
    let mut matrix = vec![vec![0.0; size]; size];
    let mut second_matrix = vec![vec![0.0; size]; size];
    let mut rhs = vec![0.0; size];
    let mut companion = vec![vec![0.0; augmented_size]; size];
    let mut companion_derivative = vec![vec![0.0; augmented_size]; size];
    let element = circuit.element(&parameter.object).ok_or_else(|| {
        format!(
            "missing transient derivative element '{}'",
            parameter.object
        )
    })?;

    if previous.is_some() {
        let integration_factor = transient_integration_factor(method);
        let mut capacitor_index = 0usize;
        for dynamic in &circuit.elements {
            match &dynamic.kind {
                SpiceElementKind::Capacitor { capacitance, .. } => {
                    let conductance = capacitance * integration_factor / step_s;
                    for (row_node, sign) in [(&dynamic.nodes[0], 1.0), (&dynamic.nodes[1], -1.0)] {
                        if let Some(row) = system.node_index(row_node) {
                            if let Some(column) = system.node_index(&dynamic.nodes[0]) {
                                companion[row][column] += sign * conductance;
                            }
                            if let Some(column) = system.node_index(&dynamic.nodes[1]) {
                                companion[row][column] -= sign * conductance;
                            }
                        }
                    }
                    if method == IntegrationMethod::Trapezoidal {
                        let history = size + capacitor_index;
                        if let Some(row) = system.node_index(&dynamic.nodes[0]) {
                            companion[row][history] += 1.0;
                        }
                        if let Some(row) = system.node_index(&dynamic.nodes[1]) {
                            companion[row][history] -= 1.0;
                        }
                    }
                    capacitor_index += 1;
                }
                SpiceElementKind::Inductor { inductance, .. } => {
                    let branch = system
                        .branch_index(&dynamic.name)
                        .ok_or_else(|| format!("missing branch for inductor '{}'", dynamic.name))?;
                    let resistance = inductance * integration_factor / step_s;
                    companion[branch][branch] -= resistance;
                    if method == IntegrationMethod::Trapezoidal {
                        if let Some(column) = system.node_index(&dynamic.nodes[0]) {
                            companion[branch][column] -= 1.0;
                        }
                        if let Some(column) = system.node_index(&dynamic.nodes[1]) {
                            companion[branch][column] += 1.0;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    match parameter.kind {
        SensitivityParameterKind::Resistance => {
            let SpiceElementKind::Resistor { resistance, .. } = &element.kind else {
                return Err(format!("'{}' is not a resistor", element.name));
            };
            let effective = element_effective_resistance(circuit, element)?;
            let scale = effective / resistance;
            stamp_conductance(
                &mut matrix,
                system,
                &element.nodes[0],
                &element.nodes[1],
                -scale / (effective * effective),
            );
            stamp_conductance(
                &mut second_matrix,
                system,
                &element.nodes[0],
                &element.nodes[1],
                2.0 * scale * scale / effective.powi(3),
            );
        }
        SensitivityParameterKind::Capacitance => {
            if previous.is_some() {
                let SpiceElementKind::Capacitor { .. } = &element.kind else {
                    return Err(format!("'{}' is not a capacitor", element.name));
                };
                stamp_conductance(
                    &mut matrix,
                    system,
                    &element.nodes[0],
                    &element.nodes[1],
                    transient_integration_factor(method) / step_s,
                );
                for (row_node, sign) in [(&element.nodes[0], 1.0), (&element.nodes[1], -1.0)] {
                    if let Some(row) = system.node_index(row_node) {
                        if let Some(column) = system.node_index(&element.nodes[0]) {
                            companion_derivative[row][column] +=
                                sign * transient_integration_factor(method) / step_s;
                        }
                        if let Some(column) = system.node_index(&element.nodes[1]) {
                            companion_derivative[row][column] -=
                                sign * transient_integration_factor(method) / step_s;
                        }
                    }
                }
            }
        }
        SensitivityParameterKind::Inductance => {
            if previous.is_some() {
                let SpiceElementKind::Inductor { .. } = &element.kind else {
                    return Err(format!("'{}' is not an inductor", element.name));
                };
                let branch = system
                    .branch_index(&element.name)
                    .ok_or_else(|| format!("missing branch for inductor '{}'", element.name))?;
                let factor = transient_integration_factor(method);
                matrix[branch][branch] -= factor / step_s;
                companion_derivative[branch][branch] -= factor / step_s;
            }
        }
        SensitivityParameterKind::DcSource => match &element.kind {
            SpiceElementKind::VoltageSource { waveform, .. } => {
                if !matches!(waveform, SourceWaveform::Dc(_)) {
                    return Err(format!("'{}' is not a DC source", element.name));
                }
                let branch = system
                    .branch_index(&element.name)
                    .ok_or_else(|| format!("missing branch for source '{}'", element.name))?;
                rhs[branch] = 1.0;
            }
            SpiceElementKind::CurrentSource { waveform, .. } => {
                if !matches!(waveform, SourceWaveform::Dc(_)) {
                    return Err(format!("'{}' is not a DC source", element.name));
                }
                stamp_current_source(&mut rhs, system, &element.nodes[0], &element.nodes[1], 1.0);
            }
            _ => return Err(format!("'{}' is not a source", element.name)),
        },
        _ => return Err("unsupported transient derivative parameter kind".into()),
    }
    Ok((matrix, rhs, companion, companion_derivative, second_matrix))
}

fn transient_integration_factor(method: IntegrationMethod) -> f64 {
    match method {
        IntegrationMethod::BackwardEuler => 1.0,
        IntegrationMethod::Trapezoidal => 2.0,
    }
}

fn add_matrix_vector(rhs: &mut [f64], matrix: &[Vec<f64>], vector: &[f64]) {
    for (row, value) in rhs.iter_mut().enumerate() {
        *value += matrix[row]
            .iter()
            .zip(vector)
            .map(|(coefficient, value)| coefficient * value)
            .sum::<f64>();
    }
}

fn add_scaled_vector(rhs: &mut [f64], matrix: &[Vec<f64>], vector: &[f64], scale: f64) {
    for (row, value) in rhs.iter_mut().enumerate() {
        *value += scale
            * matrix[row]
                .iter()
                .zip(vector)
                .map(|(coefficient, value)| coefficient * value)
                .sum::<f64>();
    }
}

fn add_matrix_vector_scaled(rhs: &mut [f64], matrix: &[Vec<f64>], vector: &[f64], scale: f64) {
    add_scaled_vector(rhs, matrix, vector, scale);
}

fn add_matrix_vector_scaled_subtract(
    rhs: &mut [f64],
    matrix: &[Vec<f64>],
    vector: &[f64],
    scale: f64,
) {
    add_scaled_vector(rhs, matrix, vector, -scale);
}

pub(super) fn subtract_nonlinear_voltage_hessian(
    circuit: &SpiceCircuit,
    system: &SystemModel,
    solution: &[f64],
    left_derivative: &[f64],
    right_derivative: &[f64],
    rhs: &mut [f64],
) -> Result<(), String> {
    for element in &circuit.elements {
        let SpiceElementKind::Diode { model: model_name } = &element.kind else {
            continue;
        };
        let model = circuit
            .diode_models
            .get(model_name)
            .ok_or_else(|| format!("unknown diode model '{model_name}'"))?;
        let anode = if model.series_resistance_ohm > 0.0 {
            format!("diode:{}:internal", element.name)
        } else {
            element.nodes[0].clone()
        };
        let cathode = element.nodes[1].clone();
        let anode_index = system
            .node_index(&anode)
            .ok_or_else(|| format!("diode '{}' has no anode unknown", element.name))?;
        let cathode_index = system.node_index(&cathode);
        let left_voltage = left_derivative[anode_index]
            - cathode_index
                .map(|index| left_derivative[index])
                .unwrap_or(0.0);
        let voltage =
            solution[anode_index] - cathode_index.map(|index| solution[index]).unwrap_or(0.0);
        let right_voltage = right_derivative[anode_index]
            - cathode_index
                .map(|index| right_derivative[index])
                .unwrap_or(0.0);
        let thermal_voltage = thermal_voltage_at(circuit.temperature_k, model.emission_coefficient);
        let effective_saturation_current = temperature_scaled_saturation_current(
            model.saturation_current_a,
            model.emission_coefficient,
            model.energy_gap_ev,
            model.saturation_current_temperature_exponent,
            circuit.temperature_k,
        );
        // A diode contributes I(V) to KCL. Its second voltage derivative is
        // required by the implicit-function Hessian; linear circuits have a
        // zero tensor and therefore do not call this path.
        let second_current = effective_saturation_current
            * ((voltage / thermal_voltage).clamp(-80.0, 80.0).exp())
            * left_voltage
            * right_voltage
            / (thermal_voltage * thermal_voltage);
        if let Some(row) = system.node_index(&anode) {
            rhs[row] -= second_current;
        }
        if let Some(row) = system.node_index(&cathode) {
            rhs[row] += second_current;
        }
    }
    Ok(())
}
