//! Local finite-difference and analytic DC derivative analysis.

use super::super::dc_operating_point;
use super::super::devices::*;
use super::super::solver::*;
use super::ac::ac_derivative_analysis;
use super::parameters::*;
use super::transient::{subtract_nonlinear_voltage_hessian, transient_final_derivative_analysis};
use crate::numeric::solve_real;
use crate::types::*;

pub fn sensitivity(
    circuit: &SpiceCircuit,
    analysis: &Analysis,
) -> Result<SensitivityResult, String> {
    let Analysis::Sensitivity(options) = analysis else {
        return Err("sensitivity called with a non-sensitivity analysis".into());
    };
    let nominal_output = sensitivity_output(circuit, options)?;
    let parameters = sensitivity_parameters(circuit);
    let mut entries = Vec::new();

    for parameter in parameters {
        let perturbation = sensitivity_perturbation(&parameter, options.relative_step);
        let low_circuit = perturbed_for_sensitivity(circuit, &parameter, -perturbation)?;
        let high_circuit = perturbed_for_sensitivity(circuit, &parameter, perturbation)?;
        let low_output = sensitivity_output(&low_circuit, options)?;
        let high_output = sensitivity_output(&high_circuit, options)?;
        let absolute_sensitivity = (high_output - low_output) / (2.0 * perturbation);
        let normalized_sensitivity = if nominal_output.abs() > f64::EPSILON {
            absolute_sensitivity * parameter.value / nominal_output
        } else {
            0.0
        };
        if !absolute_sensitivity.is_finite() || !normalized_sensitivity.is_finite() {
            return Err(format!("sensitivity of '{}' is non-finite", parameter.name));
        }
        entries.push(SensitivityEntry {
            parameter: parameter.name.clone(),
            object: parameter.object.clone(),
            nominal_parameter: parameter.value,
            perturbation,
            low_output,
            high_output,
            absolute_sensitivity,
            normalized_sensitivity,
        });
    }

    Ok(SensitivityResult {
        target: options.target,
        output_node: options.output_node.clone(),
        nominal_output,
        entries,
    })
}

pub fn derivative_analysis(
    circuit: &SpiceCircuit,
    analysis: &Analysis,
) -> Result<DerivativeResult, String> {
    let Analysis::Derivative(options) = analysis else {
        return Err("derivative analysis called with a non-derivative analysis".into());
    };
    circuit.validate()?;
    if options.target == DerivativeTarget::AcMagnitude {
        return ac_derivative_analysis(circuit, options);
    } else if options.target == DerivativeTarget::TransientFinal {
        return transient_final_derivative_analysis(circuit, options);
    } else if circuit
        .elements
        .iter()
        .any(|element| matches!(element.kind, SpiceElementKind::Mosfet { .. }))
    {
        return mos_diode_connected_dc_derivative_analysis(circuit, options);
    } else if circuit.elements.iter().any(|element| {
        matches!(
            element.kind,
            SpiceElementKind::BipolarJunctionTransistor { .. }
        )
    }) {
        return bjt_diode_connected_dc_derivative_analysis(circuit, options);
    }
    let operating_point = dc_operating_point(circuit)?;
    let nominal_output = operating_point
        .node_voltages
        .get(&options.output_node)
        .copied()
        .ok_or_else(|| format!("unknown derivative node '{}'", options.output_node))?;
    let system = SystemModel::new(circuit)?;
    let mut solution = vec![0.0; system.size];
    for (node, voltage) in &operating_point.node_voltages {
        if let Some(index) = system.node_index(node) {
            solution[index] = *voltage;
        }
    }
    for (name, current) in &operating_point.branch_currents {
        if let Some(index) = system.branch_index(name) {
            solution[index] = *current;
        }
    }
    let (matrix, _) = build_real_system(
        circuit,
        &system,
        0.0,
        None,
        None,
        IntegrationMethod::BackwardEuler,
        None,
        &solution,
    )?;

    let available = sensitivity_parameters(circuit);
    let mut first_derivatives = Vec::with_capacity(options.parameters.len());
    let mut entries = Vec::with_capacity(options.parameters.len());

    for name in &options.parameters {
        let parameter = available
            .iter()
            .find(|parameter| parameter.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("unknown derivative parameter '{name}'"))?;
        let size = system.size;
        let mut derivative_matrix = vec![vec![0.0; size]; size];
        let mut derivative_rhs = vec![0.0; size];
        let mut second_matrix = vec![vec![0.0; size]; size];

        match parameter.kind {
            SensitivityParameterKind::Resistance => {
                let element = circuit
                    .element(&parameter.object)
                    .ok_or_else(|| format!("missing resistor '{}'", parameter.object))?;
                let SpiceElementKind::Resistor { resistance, .. } = &element.kind else {
                    return Err(format!("'{}' is not a resistor", parameter.object));
                };
                let effective = element_effective_resistance(circuit, element)?;
                let scale = effective / resistance;
                let conductance_derivative = -scale / (effective * effective);
                let conductance_second_derivative =
                    2.0 * scale * scale / (effective * effective * effective);
                stamp_conductance(
                    &mut derivative_matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    conductance_derivative,
                );
                stamp_conductance(
                    &mut second_matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    conductance_second_derivative,
                );
            }
            SensitivityParameterKind::DcSource => {
                let element = circuit
                    .element(&parameter.object)
                    .ok_or_else(|| format!("missing source '{}'", parameter.object))?;
                match &element.kind {
                    SpiceElementKind::VoltageSource {
                        waveform: SourceWaveform::Dc(_),
                        ..
                    } => {
                        let branch = system.branch_index(&element.name).ok_or_else(|| {
                            format!("missing branch for source '{}'", element.name)
                        })?;
                        derivative_rhs[branch] = 1.0;
                    }
                    SpiceElementKind::CurrentSource {
                        waveform: SourceWaveform::Dc(_),
                        ..
                    } => {
                        stamp_current_source(
                            &mut derivative_rhs,
                            &system,
                            &element.nodes[0],
                            &element.nodes[1],
                            1.0,
                        );
                    }
                    _ => return Err(format!("'{}' is not a DC source", parameter.object)),
                }
            }
            SensitivityParameterKind::DiodeIs => {
                let Some(model_name) = parameter.object.strip_prefix("model:") else {
                    return Err(format!("'{}' is not a diode model", parameter.object));
                };
                let model = circuit
                    .diode_models
                    .get(model_name)
                    .ok_or_else(|| format!("missing diode model '{model_name}'"))?;
                let effective_saturation_current = temperature_scaled_saturation_current(
                    model.saturation_current_a,
                    model.emission_coefficient,
                    model.energy_gap_ev,
                    model.saturation_current_temperature_exponent,
                    circuit.temperature_k,
                );
                let temperature_scale = effective_saturation_current / model.saturation_current_a;

                for element in &circuit.elements {
                    let SpiceElementKind::Diode {
                        model: element_model,
                    } = &element.kind
                    else {
                        continue;
                    };
                    if !element_model.eq_ignore_ascii_case(model_name) {
                        continue;
                    }
                    let anode = if model.series_resistance_ohm > 0.0 {
                        format!("diode:{}:internal", element.name)
                    } else {
                        element.nodes[0].clone()
                    };
                    let cathode = element.nodes[1].clone();
                    let anode_index = system
                        .node_index(&anode)
                        .ok_or_else(|| format!("diode '{}' has no anode unknown", element.name))?;
                    let voltage = solution[anode_index]
                        - system
                            .node_index(&cathode)
                            .map(|index| solution[index])
                            .unwrap_or(0.0);
                    let thermal_voltage =
                        thermal_voltage_at(circuit.temperature_k, model.emission_coefficient);
                    let exponent = (voltage / thermal_voltage).clamp(-80.0, 80.0);
                    let normalized_current = exponent.exp_m1();
                    let normalized_conductance = exponent.exp() / thermal_voltage;
                    let scaled_conductance = temperature_scale * normalized_conductance;
                    let stamped_conductance = scaled_conductance.max(GMIN);
                    let linearized_current_derivative =
                        temperature_scale * normalized_current - stamped_conductance * voltage;

                    if scaled_conductance > GMIN {
                        stamp_conductance(
                            &mut derivative_matrix,
                            &system,
                            &anode,
                            &cathode,
                            scaled_conductance,
                        );
                    }
                    if let Some(row) = system.node_index(&anode) {
                        derivative_rhs[row] -= linearized_current_derivative;
                    }
                    if let Some(row) = system.node_index(&cathode) {
                        derivative_rhs[row] += linearized_current_derivative;
                    }
                }
            }
            _ => {
                return Err(format!(
                    "analytic derivative parameter '{name}' must be a resistor R or DC source"
                ))
            }
        }

        let mut rhs = derivative_rhs.clone();
        for row in 0..system.size {
            rhs[row] -= derivative_matrix[row]
                .iter()
                .zip(&solution)
                .map(|(coefficient, value)| coefficient * value)
                .sum::<f64>();
        }
        let solution_derivative = solve_real(matrix.clone(), rhs)?;
        let output_index = system
            .node_index(&options.output_node)
            .ok_or_else(|| format!("unknown derivative node '{}'", options.output_node))?;
        let gradient = solution_derivative[output_index];
        first_derivatives.push((derivative_matrix, second_matrix, solution_derivative));
        entries.push(DerivativeEntry {
            parameter: parameter.name.clone(),
            nominal_parameter: parameter.value,
            gradient,
            hessian_row: Vec::with_capacity(options.parameters.len()),
        });
    }

    for (left, left_derivative) in first_derivatives.iter().enumerate() {
        for (right, right_derivative) in first_derivatives.iter().enumerate() {
            let mut rhs = vec![0.0; system.size];
            for (row, rhs_value) in rhs.iter_mut().enumerate().take(system.size) {
                let left_term = left_derivative.0[row]
                    .iter()
                    .zip(&right_derivative.2)
                    .map(|(coefficient, value)| coefficient * value)
                    .sum::<f64>();
                let right_term = right_derivative.0[row]
                    .iter()
                    .zip(&left_derivative.2)
                    .map(|(coefficient, value)| coefficient * value)
                    .sum::<f64>();
                *rhs_value -= left_term + right_term;
                if left == right {
                    *rhs_value -= left_derivative.1[row]
                        .iter()
                        .zip(&solution)
                        .map(|(coefficient, value)| coefficient * value)
                        .sum::<f64>();
                }
            }
            subtract_nonlinear_voltage_hessian(
                circuit,
                &system,
                &solution,
                &left_derivative.2,
                &right_derivative.2,
                &mut rhs,
            )?;
            let second_derivative = solve_real(matrix.clone(), rhs)?;
            let output_index = system
                .node_index(&options.output_node)
                .ok_or_else(|| format!("unknown derivative node '{}'", options.output_node))?;
            entries[left]
                .hessian_row
                .push(second_derivative[output_index]);
        }
    }

    if entries.iter().any(|entry| {
        !entry.gradient.is_finite() || entry.hessian_row.iter().any(|value| !value.is_finite())
    }) {
        return Err("derivative analysis produced a non-finite derivative".into());
    }
    Ok(DerivativeResult {
        target: DerivativeTarget::Dc,
        frequency_hz: options.frequency_hz,
        output_node: options.output_node.clone(),
        nominal_output,
        entries,
    })
}

fn bjt_diode_connected_dc_derivative_analysis(
    circuit: &SpiceCircuit,
    options: &DerivativeOptions,
) -> Result<DerivativeResult, String> {
    let resistor = circuit
        .elements
        .iter()
        .find(|element| matches!(element.kind, SpiceElementKind::Resistor { .. }))
        .ok_or_else(|| "BJT DC derivative needs a load resistor".to_string())?;
    let transistor = circuit
        .elements
        .iter()
        .find(|element| {
            matches!(
                element.kind,
                SpiceElementKind::BipolarJunctionTransistor { .. }
            )
        })
        .ok_or_else(|| "BJT DC derivative needs a BJT".to_string())?;
    let source = circuit
        .elements
        .iter()
        .find(|element| {
            matches!(
                &element.kind,
                SpiceElementKind::VoltageSource {
                    waveform: SourceWaveform::Dc(_),
                    ..
                }
            )
        })
        .ok_or_else(|| "BJT DC derivative needs a DC source".to_string())?;
    if circuit.elements.len() != 3 {
        return Err("BJT DC derivatives support exactly V, R, and one BJT".into());
    }
    let SpiceElementKind::Resistor {
        resistance,
        first_temperature_coefficient_per_k,
        second_temperature_coefficient_per_k2,
        sheet_resistance_ohm_per_square,
        layer,
        ..
    } = &resistor.kind
    else {
        unreachable!()
    };
    if layer.is_some()
        || *sheet_resistance_ohm_per_square > 0.0
        || *first_temperature_coefficient_per_k != 0.0
        || *second_temperature_coefficient_per_k2 != 0.0
    {
        return Err("BJT DC R derivatives require an ideal resistor".into());
    }
    let SpiceElementKind::BipolarJunctionTransistor {
        model: model_name,
        area,
    } = &transistor.kind
    else {
        unreachable!()
    };
    let model = circuit
        .bjt_models
        .get(model_name)
        .ok_or_else(|| format!("unknown BJT model '{model_name}'"))?;
    if model.polarity != BjtPolarity::Npn {
        return Err("BJT DC derivatives require an NPN transistor".into());
    }
    let SpiceElementKind::VoltageSource {
        waveform: SourceWaveform::Dc(supply_value),
        ..
    } = &source.kind
    else {
        unreachable!()
    };
    let supply = &source.nodes[0];
    let output = &transistor.nodes[0];
    if !GROUND_ALIASES.contains(&source.nodes[1].as_str())
        || !GROUND_ALIASES.contains(&transistor.nodes[2].as_str())
        || resistor.nodes != [supply.as_str(), output.as_str()]
        || transistor.nodes[..3] != [output.as_str(), output.as_str(), "0"]
        || output != &options.output_node
    {
        return Err("BJT DC derivatives require V--R--C/B(out),NPN(emitter=ground)".into());
    }

    let operating_point = dc_operating_point(circuit)?;
    let voltage = operating_point
        .node_voltages
        .get(output)
        .copied()
        .ok_or_else(|| format!("missing BJT DC derivative node '{output}'"))?;
    let thermal_voltage = thermal_voltage_at(circuit.temperature_k, 1.0);
    let exponent = (voltage / thermal_voltage).clamp(-80.0, 80.0);
    let normalized_current = exponent.exp_m1();
    if normalized_current <= 0.0 {
        return Err("BJT DC derivatives require forward conduction".into());
    }
    let temperature_scale = temperature_scaled_saturation_current(
        model.saturation_current_a,
        1.0,
        model.energy_gap_ev,
        model.saturation_current_temperature_exponent,
        circuit.temperature_k,
    ) / model.saturation_current_a;
    let saturation_scale = temperature_scale * area;
    let current_scale = saturation_scale * (1.0 + 1.0 / model.forward_beta);

    // F(V,p) = (VIN-V)/R - IS_eff*(1+1/BF)*expm1(V/VT).
    // Parameter order is R, IS, BF, VIN, and instance AREA.
    let current_first = normalized_current / thermal_voltage;
    let current_second = exponent.exp() / (thermal_voltage * thermal_voltage);
    let f_voltage = -1.0 / resistance - model.saturation_current_a * current_scale * current_first;
    let beta_derivative = -1.0 / (model.forward_beta * model.forward_beta);
    let first_rhs = [
        -(*supply_value - voltage) / (resistance * resistance),
        -saturation_scale * (1.0 + 1.0 / model.forward_beta) * normalized_current,
        -model.saturation_current_a * saturation_scale * beta_derivative * normalized_current,
        1.0 / resistance,
        -model.saturation_current_a
            * temperature_scale
            * (1.0 + 1.0 / model.forward_beta)
            * normalized_current,
    ];
    let voltage_first = first_rhs.map(|value| -value / f_voltage);
    let voltage_voltage_derivative = [
        1.0 / (resistance * resistance),
        -saturation_scale * (1.0 + 1.0 / model.forward_beta) * current_first,
        -model.saturation_current_a * saturation_scale * beta_derivative * current_first,
        0.0,
        -model.saturation_current_a
            * temperature_scale
            * (1.0 + 1.0 / model.forward_beta)
            * current_first,
    ];
    let voltage_voltage_second = -model.saturation_current_a * current_scale * current_second;
    let mut direct_second = [[0.0; 5]; 5];
    direct_second[0][0] = 2.0 * (*supply_value - voltage) / resistance.powi(3);
    direct_second[0][3] = -1.0 / (resistance * resistance);
    direct_second[3][0] = direct_second[0][3];
    direct_second[1][4] =
        -temperature_scale * (1.0 + 1.0 / model.forward_beta) * normalized_current;
    direct_second[4][1] = direct_second[1][4];
    direct_second[2][4] =
        -model.saturation_current_a * temperature_scale * beta_derivative * normalized_current;
    direct_second[4][2] = direct_second[2][4];
    let mut voltage_second = [[0.0; 5]; 5];
    for left in 0..5 {
        for right in 0..5 {
            let mixed = voltage_voltage_derivative[left] * voltage_first[right]
                + voltage_voltage_derivative[right] * voltage_first[left]
                + voltage_voltage_second * voltage_first[left] * voltage_first[right];
            voltage_second[left][right] = -(direct_second[left][right] + mixed) / f_voltage;
        }
    }

    let parameter_info = |name: &str| -> Option<(usize, f64, String)> {
        if name.eq_ignore_ascii_case(&format!("{}.R", resistor.name)) {
            Some((0, *resistance, format!("{}.R", resistor.name)))
        } else if name.eq_ignore_ascii_case(&format!("{model_name}.IS")) {
            Some((1, model.saturation_current_a, format!("{model_name}.IS")))
        } else if name.eq_ignore_ascii_case(&format!("{model_name}.BF")) {
            Some((2, model.forward_beta, format!("{model_name}.BF")))
        } else if name.eq_ignore_ascii_case(&format!("{}.DC", source.name)) {
            Some((3, *supply_value, format!("{}.DC", source.name)))
        } else if name.eq_ignore_ascii_case(&format!("{}.AREA", transistor.name)) {
            Some((4, *area, format!("{}.AREA", transistor.name)))
        } else {
            None
        }
    };
    let mut entries = Vec::with_capacity(options.parameters.len());
    for name in &options.parameters {
        let Some((index, nominal, parameter)) = parameter_info(name) else {
            return Err(format!(
                "BJT DC derivative parameter '{name}' must be R, IS, BF, instance AREA, or source DC"
            ));
        };
        entries.push(DerivativeEntry {
            parameter,
            nominal_parameter: nominal,
            gradient: voltage_first[index],
            hessian_row: Vec::with_capacity(options.parameters.len()),
        });
    }
    for entry in &mut entries {
        let Some((left, _, _)) = parameter_info(&entry.parameter) else {
            unreachable!("BJT derivative parameter was validated above");
        };
        for name in &options.parameters {
            let Some((right, _, _)) = parameter_info(name) else {
                unreachable!("BJT derivative parameter was validated above");
            };
            entry.hessian_row.push(voltage_second[left][right]);
        }
    }

    Ok(DerivativeResult {
        target: DerivativeTarget::Dc,
        frequency_hz: options.frequency_hz,
        output_node: options.output_node.clone(),
        nominal_output: voltage,
        entries,
    })
}

fn mos_diode_connected_dc_derivative_analysis(
    circuit: &SpiceCircuit,
    options: &DerivativeOptions,
) -> Result<DerivativeResult, String> {
    let resistor = circuit
        .elements
        .iter()
        .find(|element| matches!(element.kind, SpiceElementKind::Resistor { .. }))
        .ok_or_else(|| "MOS DC derivative needs a load resistor".to_string())?;
    let mosfet = circuit
        .elements
        .iter()
        .find(|element| matches!(element.kind, SpiceElementKind::Mosfet { .. }))
        .ok_or_else(|| "MOS DC derivative needs a MOSFET".to_string())?;
    let source = circuit
        .elements
        .iter()
        .find(|element| {
            matches!(
                &element.kind,
                SpiceElementKind::VoltageSource {
                    waveform: SourceWaveform::Dc(_),
                    ..
                }
            )
        })
        .ok_or_else(|| "MOS DC derivative needs a DC source".to_string())?;
    if circuit.elements.len() != 3 {
        return Err("MOS DC derivatives support exactly V, R, and one MOSFET".into());
    }
    let SpiceElementKind::Resistor {
        resistance,
        first_temperature_coefficient_per_k,
        second_temperature_coefficient_per_k2,
        sheet_resistance_ohm_per_square,
        layer,
        ..
    } = &resistor.kind
    else {
        unreachable!()
    };
    if layer.is_some()
        || *sheet_resistance_ohm_per_square > 0.0
        || *first_temperature_coefficient_per_k != 0.0
        || *second_temperature_coefficient_per_k2 != 0.0
    {
        return Err("MOS DC R derivatives require an ideal resistor".into());
    }
    let SpiceElementKind::Mosfet {
        model: model_name,
        width_m,
        length_m,
        multiplier,
    } = &mosfet.kind
    else {
        unreachable!()
    };
    let model = circuit.mosfet_model_for(model_name, *width_m, *length_m)?;
    if model.polarity != MosfetPolarity::Nmos
        || model.channel_length_modulation_per_v != 0.0
        || model.body_effect_v_per_sqrt_v != 0.0
        || model.mobility_degradation_inverse_v != 0.0
        || model.velocity_saturation_voltage_v != 0.0
    {
        return Err(
            "MOS DC derivatives require an ideal NMOS with LAMBDA/GAMMA/THETA/VSAT=0".into(),
        );
    }
    let SpiceElementKind::VoltageSource {
        waveform: SourceWaveform::Dc(supply_value),
        ..
    } = &source.kind
    else {
        unreachable!()
    };
    let supply = &source.nodes[0];
    let output = &mosfet.nodes[0];
    let source_ground = GROUND_ALIASES.contains(&source.nodes[1].as_str());
    let bulk_ground = GROUND_ALIASES.contains(&mosfet.nodes[3].as_str());
    let mos_source_ground = GROUND_ALIASES.contains(&mosfet.nodes[2].as_str());
    if !source_ground
        || !bulk_ground
        || !mos_source_ground
        || resistor.nodes != [supply.as_str(), output.as_str()]
        || mosfet.nodes[..4] != [output.as_str(), output.as_str(), "0", "0"]
        || output != &options.output_node
    {
        return Err("MOS DC derivatives require V--R--D/G(out),NMOS(source=bulk=ground)".into());
    }

    let operating_point = dc_operating_point(circuit)?;
    let voltage = operating_point
        .node_voltages
        .get(output)
        .copied()
        .ok_or_else(|| format!("missing MOS DC derivative node '{output}'"))?;
    let overdrive = voltage - model.threshold_voltage_v;
    if overdrive <= 0.0 || voltage <= overdrive {
        return Err("MOS DC derivatives require forward saturation".into());
    }
    let temperature_scale = temperature_scaled_mos_kp(
        model.transconductance_a_per_v2,
        model.mobility_temperature_exponent,
        circuit.temperature_k,
    ) / model.transconductance_a_per_v2;
    let geometry = width_m / length_m * multiplier;
    let transconductance = model.transconductance_a_per_v2 * temperature_scale * geometry;

    // F(V,p) = (VIN-V)/R - K/2*(V-VTO)^2, with K linear in model KP.
    let f_voltage = -1.0 / resistance - transconductance * overdrive;
    let first_rhs = [
        -(*supply_value - voltage) / (resistance * resistance),
        transconductance * overdrive,
        -temperature_scale * geometry * overdrive * overdrive / 2.0,
        1.0 / resistance,
    ];
    let voltage_first = first_rhs.map(|value| -value / f_voltage);
    let voltage_voltage_derivative = [
        1.0 / (resistance * resistance),
        transconductance,
        -overdrive,
        0.0,
    ];
    let mut direct_second = [[0.0; 4]; 4];
    direct_second[0][0] = 2.0 * (*supply_value - voltage) / resistance.powi(3);
    direct_second[0][3] = -1.0 / (resistance * resistance);
    direct_second[3][0] = direct_second[0][3];
    let mut voltage_second = [[0.0; 4]; 4];
    for left in 0..4 {
        for right in 0..4 {
            let mixed = voltage_voltage_derivative[left] * voltage_first[right]
                + voltage_voltage_derivative[right] * voltage_first[left];
            voltage_second[left][right] = -(direct_second[left][right] + mixed) / f_voltage;
        }
    }

    let parameter_info = |name: &str| -> Option<(usize, f64, String)> {
        if name.eq_ignore_ascii_case(&format!("{}.R", resistor.name)) {
            Some((0, *resistance, format!("{}.R", resistor.name)))
        } else if name.eq_ignore_ascii_case(&format!("{model_name}.VTO")) {
            Some((1, model.threshold_voltage_v, format!("{model_name}.VTO")))
        } else if name.eq_ignore_ascii_case(&format!("{model_name}.KP")) {
            Some((
                2,
                model.transconductance_a_per_v2,
                format!("{model_name}.KP"),
            ))
        } else if name.eq_ignore_ascii_case(&format!("{}.DC", source.name)) {
            Some((3, *supply_value, format!("{}.DC", source.name)))
        } else {
            None
        }
    };
    let mut entries = Vec::with_capacity(options.parameters.len());
    for name in &options.parameters {
        let Some((index, nominal, parameter)) = parameter_info(name) else {
            return Err(format!(
                "MOS DC derivative parameter '{name}' must be R, VTO, KP, or source DC"
            ));
        };
        entries.push(DerivativeEntry {
            parameter,
            nominal_parameter: nominal,
            gradient: voltage_first[index],
            hessian_row: Vec::with_capacity(options.parameters.len()),
        });
    }
    for entry in &mut entries {
        let Some((left, _, _)) = parameter_info(&entry.parameter) else {
            unreachable!("MOS derivative parameter was validated above");
        };
        for name in &options.parameters {
            let Some((right, _, _)) = parameter_info(name) else {
                unreachable!("MOS derivative parameter was validated above");
            };
            entry.hessian_row.push(voltage_second[left][right]);
        }
    }

    Ok(DerivativeResult {
        target: DerivativeTarget::Dc,
        frequency_hz: options.frequency_hz,
        output_node: options.output_node.clone(),
        nominal_output: voltage,
        entries,
    })
}
