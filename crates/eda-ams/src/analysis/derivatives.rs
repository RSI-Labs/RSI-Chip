//! Local sensitivity, derivatives, and global variance analysis.

use super::devices::*;
use super::random::*;
use super::solver::*;
use super::{ac_sweep, dc_operating_point, transient};
use crate::numeric::{solve_complex, solve_real, Complex};
use crate::types::*;
use crate::FlowCancellation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SensitivityParameterKind {
    Resistance,
    Capacitance,
    Inductance,
    DcSource,
    AcSource,
    DiodeIs,
    MosVto,
    MosKp,
    BjtBeta,
    BjtIs,
    BjtArea,
    Bsim3Vth0,
    Bsim3K1,
    Bsim3U0,
    Bsim3Tox,
    Bsim3Vsat,
    Bsim3Dvt0,
    Bsim3Dvt1,
    Bsim3Dvt2,
    Bsim3Dvt0w,
    Bsim3Dvt1w,
    Bsim3W0,
    Bsim3Kt1,
    Bsim3Kt2,
    Bsim3Kt1l,
}

#[derive(Clone, Debug)]
pub(super) struct SensitivityParameter {
    pub(super) name: String,
    pub(super) object: String,
    pub(super) value: f64,
    pub(super) kind: SensitivityParameterKind,
}

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

fn transient_final_derivative_analysis(
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

fn subtract_nonlinear_voltage_hessian(
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AcDerivativeKind {
    Resistance,
    Capacitance,
    Inductance,
    AcSource,
}

#[derive(Clone, Debug)]
struct AcDerivativeParameter {
    name: String,
    object: String,
    value: f64,
    kind: AcDerivativeKind,
}

fn conjugate(value: Complex) -> Complex {
    Complex::new(value.re, -value.im)
}

fn ac_derivative_parameters(circuit: &SpiceCircuit) -> Vec<AcDerivativeParameter> {
    let mut out = Vec::new();
    for element in &circuit.elements {
        match &element.kind {
            SpiceElementKind::Resistor { resistance, .. } => out.push(AcDerivativeParameter {
                name: format!("{}.R", element.name),
                object: element.name.clone(),
                value: *resistance,
                kind: AcDerivativeKind::Resistance,
            }),
            SpiceElementKind::Capacitor { capacitance, .. } => out.push(AcDerivativeParameter {
                name: format!("{}.C", element.name),
                object: element.name.clone(),
                value: *capacitance,
                kind: AcDerivativeKind::Capacitance,
            }),
            SpiceElementKind::Inductor { inductance, .. } => out.push(AcDerivativeParameter {
                name: format!("{}.L", element.name),
                object: element.name.clone(),
                value: *inductance,
                kind: AcDerivativeKind::Inductance,
            }),
            SpiceElementKind::VoltageSource {
                ac_magnitude: Some(magnitude),
                ..
            }
            | SpiceElementKind::CurrentSource {
                ac_magnitude: Some(magnitude),
                ..
            } => out.push(AcDerivativeParameter {
                name: format!("{}.AC", element.name),
                object: element.name.clone(),
                value: *magnitude,
                kind: AcDerivativeKind::AcSource,
            }),
            _ => {}
        }
    }
    out
}

fn ac_derivative_analysis(
    circuit: &SpiceCircuit,
    options: &DerivativeOptions,
) -> Result<DerivativeResult, String> {
    if circuit
        .elements
        .iter()
        .any(|element| matches!(element.kind, SpiceElementKind::Diode { .. }))
    {
        return nonlinear_diode_ac_derivative_analysis(circuit, options);
    }
    let system = SystemModel::new(circuit)?;
    let size = system.size;
    let mut matrix = vec![vec![Complex::zero(); size]; size];
    let mut rhs = vec![Complex::zero(); size];
    let omega = std::f64::consts::TAU * options.frequency_hz;
    let j = Complex::new(0.0, 1.0);

    for element in &circuit.elements {
        if matches!(
            element.kind,
            SpiceElementKind::Capacitor { .. } | SpiceElementKind::Inductor { .. }
        ) {
            let branch = system
                .branch_index(&element.name)
                .ok_or_else(|| format!("missing branch for element '{}'", element.name))?;
            matrix[branch][branch] = Complex::new(1.0, 0.0);
        }
    }

    for element in &circuit.elements {
        match &element.kind {
            SpiceElementKind::Resistor { .. } => {
                let effective = element_effective_resistance(circuit, element)?;
                stamp_complex_conductance(
                    &mut matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    Complex::new(1.0 / effective, 0.0),
                );
            }
            SpiceElementKind::Capacitor { capacitance, .. } => {
                stamp_complex_conductance(
                    &mut matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    j.scale(omega * capacitance),
                );
            }
            SpiceElementKind::Inductor { inductance, .. } => {
                let admittance = Complex::new(1.0, 0.0).div(j.scale(omega * inductance))?;
                stamp_complex_conductance(
                    &mut matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    admittance,
                );
            }
            SpiceElementKind::VoltageSource {
                ac_magnitude,
                ac_phase_deg,
                ..
            } => {
                let branch = system
                    .branch_index(&element.name)
                    .ok_or_else(|| format!("missing branch for source '{}'", element.name))?;
                stamp_complex_branch_current(
                    &mut matrix,
                    &system,
                    branch,
                    &element.nodes[0],
                    &element.nodes[1],
                );
                stamp_complex_node(
                    &mut matrix,
                    &system,
                    branch,
                    &element.nodes[0],
                    Complex::new(1.0, 0.0),
                );
                stamp_complex_node(
                    &mut matrix,
                    &system,
                    branch,
                    &element.nodes[1],
                    Complex::new(-1.0, 0.0),
                );
                if let Some(magnitude) = ac_magnitude {
                    rhs[branch] = Complex::polar(*magnitude, (*ac_phase_deg).to_radians());
                }
            }
            SpiceElementKind::CurrentSource {
                ac_magnitude,
                ac_phase_deg,
                ..
            } => {
                if let Some(magnitude) = ac_magnitude {
                    stamp_complex_current(
                        &mut rhs,
                        &system,
                        &element.nodes[0],
                        &element.nodes[1],
                        Complex::polar(*magnitude, (*ac_phase_deg).to_radians()),
                    );
                }
            }
            _ => return Err("AC analytic derivatives support linear RCL circuits only".into()),
        }
    }

    let solution = solve_complex(matrix.clone(), rhs)?;
    let output_index = system
        .node_index(&options.output_node)
        .ok_or_else(|| format!("unknown derivative node '{}'", options.output_node))?;
    let output = solution[output_index];
    let nominal_output = output.magnitude();
    if !nominal_output.is_finite() || nominal_output <= f64::EPSILON {
        return Err("AC derivative output magnitude is zero".into());
    }

    let available = ac_derivative_parameters(circuit);
    let mut first = Vec::with_capacity(options.parameters.len());
    let mut entries = Vec::with_capacity(options.parameters.len());
    for name in &options.parameters {
        let parameter = available
            .iter()
            .find(|parameter| parameter.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("unknown AC derivative parameter '{name}'"))?;
        let element = circuit
            .element(&parameter.object)
            .ok_or_else(|| format!("missing AC derivative element '{}'", parameter.object))?;
        let mut derivative_matrix = vec![vec![Complex::zero(); size]; size];
        let mut derivative_rhs = vec![Complex::zero(); size];
        let mut second_matrix = vec![vec![Complex::zero(); size]; size];

        match parameter.kind {
            AcDerivativeKind::Resistance => {
                let SpiceElementKind::Resistor { resistance, .. } = &element.kind else {
                    return Err(format!("'{}' is not a resistor", parameter.object));
                };
                let effective = element_effective_resistance(circuit, element)?;
                let scale = effective / resistance;
                stamp_complex_conductance(
                    &mut derivative_matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    Complex::new(-scale / (effective * effective), 0.0),
                );
                stamp_complex_conductance(
                    &mut second_matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    Complex::new(2.0 * scale * scale / effective.powi(3), 0.0),
                );
            }
            AcDerivativeKind::Capacitance => {
                stamp_complex_conductance(
                    &mut derivative_matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    j.scale(omega),
                );
            }
            AcDerivativeKind::Inductance => {
                let SpiceElementKind::Inductor { inductance, .. } = &element.kind else {
                    return Err(format!("'{}' is not an inductor", parameter.object));
                };
                let admittance = Complex::new(1.0, 0.0).div(j.scale(omega * inductance))?;
                stamp_complex_conductance(
                    &mut derivative_matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    admittance.scale(-1.0 / inductance),
                );
                stamp_complex_conductance(
                    &mut second_matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    admittance.scale(2.0 / (inductance * inductance)),
                );
            }
            AcDerivativeKind::AcSource => match &element.kind {
                SpiceElementKind::VoltageSource { ac_phase_deg, .. } => {
                    let branch = system
                        .branch_index(&element.name)
                        .ok_or_else(|| format!("missing branch for source '{}'", element.name))?;
                    derivative_rhs[branch] = Complex::polar(1.0, (*ac_phase_deg).to_radians());
                }
                SpiceElementKind::CurrentSource { ac_phase_deg, .. } => stamp_complex_current(
                    &mut derivative_rhs,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    Complex::polar(1.0, (*ac_phase_deg).to_radians()),
                ),
                _ => return Err(format!("'{}' is not an AC source", parameter.object)),
            },
        }

        let mut dx_rhs = derivative_rhs.clone();
        for row in 0..size {
            let product = derivative_matrix[row]
                .iter()
                .zip(&solution)
                .map(|(coefficient, value)| coefficient.mul(*value))
                .fold(Complex::zero(), |sum, value| sum.add(value));
            dx_rhs[row] = dx_rhs[row].sub(product);
        }
        let solution_derivative = solve_complex(matrix.clone(), dx_rhs)?;
        let gradient = conjugate(output).mul(solution_derivative[output_index]).re / nominal_output;
        first.push((derivative_matrix, second_matrix, solution_derivative));
        entries.push(DerivativeEntry {
            parameter: parameter.name.clone(),
            nominal_parameter: parameter.value,
            gradient,
            hessian_row: Vec::with_capacity(options.parameters.len()),
        });
    }

    for (left, left_derivative) in first.iter().enumerate() {
        for (right, right_derivative) in first.iter().enumerate() {
            let mut second_rhs = vec![Complex::zero(); size];
            for (row, second_value) in second_rhs.iter_mut().enumerate() {
                let left_term = left_derivative.0[row]
                    .iter()
                    .zip(&right_derivative.2)
                    .map(|(coefficient, value)| coefficient.mul(*value))
                    .fold(Complex::zero(), |sum, value| sum.add(value));
                let right_term = right_derivative.0[row]
                    .iter()
                    .zip(&left_derivative.2)
                    .map(|(coefficient, value)| coefficient.mul(*value))
                    .fold(Complex::zero(), |sum, value| sum.add(value));
                *second_value = left_term.add(right_term).scale(-1.0);
                if left == right {
                    let second_term = left_derivative.1[row]
                        .iter()
                        .zip(&solution)
                        .map(|(coefficient, value)| coefficient.mul(*value))
                        .fold(Complex::zero(), |sum, value| sum.add(value));
                    *second_value = second_value.sub(second_term);
                }
            }
            let second_solution = solve_complex(matrix.clone(), second_rhs)?;
            let cross = conjugate(left_derivative.2[output_index])
                .mul(right_derivative.2[output_index])
                .re
                / nominal_output;
            let direct = conjugate(output).mul(second_solution[output_index]).re / nominal_output;
            let value =
                direct + cross - entries[left].gradient * entries[right].gradient / nominal_output;
            entries[left].hessian_row.push(value);
        }
    }

    if entries.iter().any(|entry| {
        !entry.gradient.is_finite() || entry.hessian_row.iter().any(|value| !value.is_finite())
    }) {
        return Err("AC derivative analysis produced a non-finite derivative".into());
    }
    Ok(DerivativeResult {
        target: DerivativeTarget::AcMagnitude,
        frequency_hz: options.frequency_hz,
        output_node: options.output_node.clone(),
        nominal_output,
        entries,
    })
}

/// A scalar value carrying first and second derivatives with respect to four
/// independent variables. This small internal jet keeps nonlinear diode
/// depletion-capacitance derivatives algorithmic without external crates.
#[derive(Clone, Copy)]
struct SecondJet {
    value: f64,
    gradient: [f64; 4],
    hessian: [[f64; 4]; 4],
}

impl SecondJet {
    fn constant(value: f64) -> Self {
        Self {
            value,
            gradient: [0.0; 4],
            hessian: [[0.0; 4]; 4],
        }
    }

    fn variable(index: usize, value: f64) -> Self {
        let mut gradient = [0.0; 4];
        gradient[index] = 1.0;
        Self {
            value,
            gradient,
            hessian: [[0.0; 4]; 4],
        }
    }

    fn add(&self, other: &Self) -> Self {
        let mut gradient = [0.0; 4];
        for (destination, (left, right)) in gradient
            .iter_mut()
            .zip(self.gradient.iter().zip(&other.gradient))
        {
            *destination = left + right;
        }
        let mut out = Self {
            value: self.value + other.value,
            gradient,
            hessian: [[0.0; 4]; 4],
        };
        for left in 0..4 {
            for right in 0..4 {
                out.hessian[left][right] = self.hessian[left][right] + other.hessian[left][right];
            }
        }
        out
    }

    fn neg(&self) -> Self {
        Self {
            value: -self.value,
            gradient: self.gradient.map(|value| -value),
            hessian: self.hessian.map(|row| row.map(|value| -value)),
        }
    }

    fn sub(&self, other: &Self) -> Self {
        self.add(&other.neg())
    }

    fn mul(&self, other: &Self) -> Self {
        let mut out = Self {
            value: self.value * other.value,
            gradient: [0.0; 4],
            hessian: [[0.0; 4]; 4],
        };
        for index in 0..4 {
            out.gradient[index] =
                self.gradient[index] * other.value + other.gradient[index] * self.value;
            for other_index in 0..4 {
                out.hessian[index][other_index] = self.hessian[index][other_index] * other.value
                    + other.hessian[index][other_index] * self.value
                    + self.gradient[index] * other.gradient[other_index]
                    + other.gradient[index] * self.gradient[other_index];
            }
        }
        out
    }

    fn div(&self, other: &Self) -> Self {
        self.mul(&other.inverse())
    }

    fn inverse(&self) -> Self {
        let value = 1.0 / self.value;
        let mut out = Self {
            value,
            gradient: self.gradient.map(|derivative| -derivative * value * value),
            hessian: [[0.0; 4]; 4],
        };
        for left in 0..4 {
            for right in 0..4 {
                out.hessian[left][right] =
                    2.0 * self.gradient[left] * self.gradient[right] * value.powi(3)
                        - self.hessian[left][right] * value * value;
            }
        }
        out
    }

    fn ln(&self) -> Self {
        let inverse = 1.0 / self.value;
        let mut out = Self {
            value: self.value.ln(),
            gradient: self.gradient.map(|derivative| derivative * inverse),
            hessian: [[0.0; 4]; 4],
        };
        for left in 0..4 {
            for right in 0..4 {
                out.hessian[left][right] = self.hessian[left][right] * inverse
                    - self.gradient[left] * self.gradient[right] * inverse * inverse;
            }
        }
        out
    }

    fn exp(&self) -> Self {
        let value = self.value.exp();
        let mut out = Self {
            value,
            gradient: self.gradient.map(|derivative| derivative * value),
            hessian: [[0.0; 4]; 4],
        };
        for left in 0..4 {
            for right in 0..4 {
                out.hessian[left][right] = value
                    * (self.gradient[left] * self.gradient[right] + self.hessian[left][right]);
            }
        }
        out
    }
}

fn diode_depletion_shape_jet(voltage: f64, potential: f64, grading: f64, knee: f64) -> SecondJet {
    let voltage_jet = SecondJet::variable(0, voltage);
    let potential_jet = SecondJet::variable(1, potential);
    let grading_jet = SecondJet::variable(2, grading);
    let knee_jet = SecondJet::variable(3, knee);
    if voltage <= knee * potential {
        let denominator = SecondJet::constant(1.0).sub(&voltage_jet.div(&potential_jet));
        let clamped = if denominator.value < 0.01 {
            SecondJet::constant(0.01)
        } else {
            denominator
        };
        let exponent = clamped.ln().mul(&grading_jet.neg());
        exponent.exp()
    } else {
        let knee_factor = SecondJet::constant(1.0).sub(&knee_jet);
        let squared_factor = knee_factor.mul(&knee_factor);
        let knee_cap = knee_factor.ln().mul(&grading_jet.neg()).exp();
        let numerator = voltage_jet.sub(&knee_jet.mul(&potential_jet));
        let normalized_difference = numerator.div(&potential_jet.mul(&squared_factor));
        let bracket = SecondJet::constant(1.0).add(&grading_jet.mul(&normalized_difference));
        knee_cap.mul(&bracket)
    }
}

fn nonlinear_diode_ac_derivative_analysis(
    circuit: &SpiceCircuit,
    options: &DerivativeOptions,
) -> Result<DerivativeResult, String> {
    let resistor = circuit
        .elements
        .iter()
        .find(|element| matches!(element.kind, SpiceElementKind::Resistor { .. }))
        .ok_or_else(|| "nonlinear AC derivative needs a series resistor".to_string())?;
    let diode = circuit
        .elements
        .iter()
        .find(|element| matches!(element.kind, SpiceElementKind::Diode { .. }))
        .ok_or_else(|| "nonlinear AC derivative needs a diode".to_string())?;
    let source = circuit
        .elements
        .iter()
        .find(|element| {
            matches!(
                &element.kind,
                SpiceElementKind::VoltageSource {
                    waveform: SourceWaveform::Dc(_),
                    ac_magnitude: Some(_),
                    ..
                }
            )
        })
        .ok_or_else(|| "nonlinear AC derivative needs a DC/AC source".to_string())?;
    if circuit.elements.len() != 3 {
        return Err("nonlinear AC derivatives support exactly R, diode, and V".into());
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
        return Err(
            "nonlinear AC R derivatives support an ideal temperature-neutral resistor".into(),
        );
    }
    let SpiceElementKind::Diode { model: model_name } = &diode.kind else {
        unreachable!()
    };
    let model = circuit
        .diode_models
        .get(model_name)
        .ok_or_else(|| format!("unknown diode model '{model_name}'"))?;
    if model.series_resistance_ohm > 0.0 {
        return Err("nonlinear AC diode derivatives require RS=0".into());
    }
    let SpiceElementKind::VoltageSource {
        waveform: SourceWaveform::Dc(dc_value),
        ac_magnitude: Some(ac_magnitude),
        ..
    } = &source.kind
    else {
        unreachable!()
    };
    let input = &source.nodes[0];
    let output = &diode.nodes[0];
    if !GROUND_ALIASES.contains(&source.nodes[1].as_str())
        || !GROUND_ALIASES.contains(&diode.nodes[1].as_str())
        || resistor.nodes != [input.as_str(), output.as_str()]
        || output != &options.output_node
    {
        return Err("nonlinear AC derivatives require V--R--V(out),D(out--ground) topology".into());
    }

    let operating_point = dc_operating_point(circuit)?;
    let diode_voltage = operating_point
        .node_voltages
        .get(output)
        .copied()
        .ok_or_else(|| format!("missing nonlinear AC node '{output}'"))?;
    let current = (*dc_value - diode_voltage) / resistance;
    let thermal_voltage = thermal_voltage_at(circuit.temperature_k, model.emission_coefficient);
    let temperature_scale = temperature_scaled_saturation_current(
        model.saturation_current_a,
        model.emission_coefficient,
        model.energy_gap_ev,
        model.saturation_current_temperature_exponent,
        circuit.temperature_k,
    ) / model.saturation_current_a;
    let saturation_current = temperature_scale * model.saturation_current_a;
    let dynamic_conductance = (current + saturation_current) / thermal_voltage + GMIN;
    if dynamic_conductance <= GMIN {
        return Err("nonlinear AC derivative requires a forward-biased diode".into());
    }

    // Scalar implicit derivatives of F(I,p) = R*I + VT*ln((I+IS)/IS) - VDC.
    let f_current = resistance + thermal_voltage / current;
    let current_first_base = [
        -current / f_current,
        -temperature_scale
            * thermal_voltage
            * (1.0 / (current + saturation_current) - 1.0 / saturation_current)
            / f_current,
        1.0 / f_current,
    ];
    let current_second_current = -thermal_voltage / (current * current);
    let mut current_second_base = [[0.0; 3]; 3];
    for left in 0..3 {
        for right in 0..3 {
            let mixed_current = match (left, right) {
                (0, 0) => 2.0 * current_first_base[0],
                (0, 1) | (1, 0) => current_first_base[1],
                (0, 2) | (2, 0) => current_first_base[2],
                _ => 0.0,
            };
            let direct = if (left, right) == (1, 1) {
                temperature_scale
                    * temperature_scale
                    * thermal_voltage
                    * (1.0 / (saturation_current * saturation_current)
                        - 1.0 / ((current + saturation_current) * (current + saturation_current)))
            } else {
                0.0
            };
            current_second_base[left][right] = -(mixed_current
                + current_second_current * current_first_base[left] * current_first_base[right]
                + direct)
                / f_current;
        }
    }

    // Physical parameter order: R, IS, VDC, TT, CJO, VJ, M, FC.
    // AC source magnitude is linear and handled as parameter index 8.
    let charge_first = [
        current_first_base[0],
        current_first_base[1] + temperature_scale,
        current_first_base[2],
    ];
    let mut conductance_first = [0.0; 8];
    conductance_first[..3].copy_from_slice(&charge_first.map(|value| value / thermal_voltage));
    let mut conductance_second = [[0.0; 8]; 8];
    for left in 0..3 {
        for right in 0..3 {
            conductance_second[left][right] = current_second_base[left][right] / thermal_voltage;
        }
    }
    let diffusion_capacitance =
        model.transit_time_s * (current + saturation_current) / thermal_voltage;
    let mut capacitance_first = [0.0; 8];
    let mut capacitance_second = [[0.0; 8]; 8];
    for left in 0..3 {
        capacitance_first[left] = model.transit_time_s * charge_first[left] / thermal_voltage;
        for right in 0..3 {
            capacitance_second[left][right] =
                model.transit_time_s * current_second_base[left][right] / thermal_voltage;
        }
        capacitance_second[3][left] = charge_first[left] / thermal_voltage;
        capacitance_second[left][3] = charge_first[left] / thermal_voltage;
    }
    capacitance_first[3] = (current + saturation_current) / thermal_voltage;

    // Depletion capacitance derivatives. The terminal voltage is
    // Vd=VDC-R*I. VJ/M/FC do not affect the DC solution; CJO is a direct
    // multiplier. A second-order jet over [Vd,VJ,M,FC] avoids long manual
    // derivatives of the depletion equation and its FC knee continuation.
    let mut voltage_first = [0.0; 8];
    voltage_first[0] = -current - resistance * current_first_base[0];
    voltage_first[1] = -resistance * current_first_base[1];
    voltage_first[2] = 1.0 - resistance * current_first_base[2];
    let mut voltage_second = [[0.0; 8]; 8];
    voltage_second[0][0] = -2.0 * current_first_base[0] - resistance * current_second_base[0][0];
    voltage_second[0][1] = -current_first_base[1] - resistance * current_second_base[0][1];
    voltage_second[0][2] = -current_first_base[2] - resistance * current_second_base[0][2];
    voltage_second[1][0] = voltage_second[0][1];
    voltage_second[1][1] = -resistance * current_second_base[1][1];
    voltage_second[1][2] = -resistance * current_second_base[1][2];
    voltage_second[2][0] = voltage_second[0][2];
    voltage_second[2][1] = voltage_second[1][2];
    voltage_second[2][2] = -resistance * current_second_base[2][2];

    let shape = diode_depletion_shape_jet(
        diode_voltage,
        model.junction_potential_v,
        model.grading_coefficient,
        model.forward_knee,
    );
    let shape_gradient = [
        shape.gradient[0],
        shape.gradient[1],
        shape.gradient[2],
        shape.gradient[3],
    ];
    // T maps a physical-parameter perturbation to [Vd,VJ,M,FC].
    let mut composition = [[0.0; 4]; 8];
    for parameter in 0..3 {
        composition[parameter][0] = voltage_first[parameter];
    }
    composition[4] = [0.0, 0.0, 0.0, 0.0];
    composition[5] = [0.0, 1.0, 0.0, 0.0];
    composition[6] = [0.0, 0.0, 1.0, 0.0];
    composition[7] = [0.0, 0.0, 0.0, 1.0];
    let depletion_gradient = |parameter: usize| {
        let mut value = 0.0;
        for (index, coefficient) in composition[parameter].iter().enumerate() {
            value += coefficient * shape_gradient[index];
        }
        value
    };
    let depletion_second = |left: usize, right: usize| {
        let mut value = 0.0;
        for row in 0..4 {
            for column in 0..4 {
                value += composition[left][row]
                    * shape.hessian[row][column]
                    * composition[right][column];
            }
        }
        if left < 3 && right < 3 {
            value += shape_gradient[0] * voltage_second[left][right];
        }
        value
    };
    let depletion_capacitance = model.zero_bias_junction_cap_f * shape.value;
    for parameter in 0..8 {
        capacitance_first[parameter] += if parameter == 4 {
            shape.value
        } else {
            model.zero_bias_junction_cap_f * depletion_gradient(parameter)
        };
        for (other, second_value) in capacitance_second[parameter].iter_mut().enumerate() {
            let mut value = model.zero_bias_junction_cap_f * depletion_second(parameter, other);
            if parameter == 4 || other == 4 {
                let multiplier_parameter = if parameter == 4 { other } else { parameter };
                value += depletion_gradient(multiplier_parameter);
            }
            *second_value += value;
        }
    }

    // Source-R/diode small-signal transfer magnitude.
    let real_part = 1.0 + resistance * dynamic_conductance;
    let omega = std::f64::consts::TAU * options.frequency_hz;
    let total_capacitance = diffusion_capacitance + depletion_capacitance;
    let imaginary_part = omega * resistance * total_capacitance;
    let mut real_first = [0.0; 8];
    let mut imaginary_first = [0.0; 8];
    let mut real_second = [[0.0; 8]; 8];
    let mut imaginary_second = [[0.0; 8]; 8];
    for left in 0..8 {
        real_first[left] = resistance * conductance_first[left];
        imaginary_first[left] = omega * resistance * capacitance_first[left];
        if left == 0 {
            real_first[left] += dynamic_conductance;
            imaginary_first[left] += omega * total_capacitance;
        }
        for right in 0..8 {
            real_second[left][right] = resistance * conductance_second[left][right];
            imaginary_second[left][right] = omega * resistance * capacitance_second[left][right];
            if left == 0 {
                real_second[left][right] += conductance_first[right];
                imaginary_second[left][right] += omega * capacitance_first[right];
            }
            if right == 0 {
                real_second[left][right] += conductance_first[left];
                imaginary_second[left][right] += omega * capacitance_first[left];
            }
        }
    }

    let denominator = real_part * real_part + imaginary_part * imaginary_part;
    let unit_magnitude = 1.0 / denominator.sqrt();
    let mut numerator_first = [0.0; 8];
    for index in 0..8 {
        numerator_first[index] =
            real_part * real_first[index] + imaginary_part * imaginary_first[index];
    }
    let mut unit_first = [0.0; 8];
    for index in 0..8 {
        unit_first[index] = -numerator_first[index] * unit_magnitude / denominator;
    }
    let mut unit_second = [[0.0; 8]; 8];
    for left in 0..8 {
        for right in 0..8 {
            let numerator_second = real_first[left] * real_first[right]
                + imaginary_first[left] * imaginary_first[right]
                + real_part * real_second[left][right]
                + imaginary_part * imaginary_second[left][right];
            unit_second[left][right] = -numerator_second * unit_magnitude / denominator
                + 3.0 * numerator_first[left] * numerator_first[right] * unit_magnitude
                    / (denominator * denominator);
        }
    }

    let parameter_info = |name: &str| -> Option<(usize, f64, String)> {
        if name.eq_ignore_ascii_case(&format!("{}.R", resistor.name)) {
            Some((0, *resistance, format!("{}.R", resistor.name)))
        } else if name.eq_ignore_ascii_case(&format!("{model_name}.IS")) {
            Some((1, model.saturation_current_a, format!("{model_name}.IS")))
        } else if name.eq_ignore_ascii_case(&format!("{}.DC", source.name)) {
            Some((2, *dc_value, format!("{}.DC", source.name)))
        } else if name.eq_ignore_ascii_case(&format!("{model_name}.TT")) {
            Some((3, model.transit_time_s, format!("{model_name}.TT")))
        } else if name.eq_ignore_ascii_case(&format!("{model_name}.CJO")) {
            Some((
                4,
                model.zero_bias_junction_cap_f,
                format!("{model_name}.CJO"),
            ))
        } else if name.eq_ignore_ascii_case(&format!("{model_name}.VJ")) {
            Some((5, model.junction_potential_v, format!("{model_name}.VJ")))
        } else if name.eq_ignore_ascii_case(&format!("{model_name}.M")) {
            Some((6, model.grading_coefficient, format!("{model_name}.M")))
        } else if name.eq_ignore_ascii_case(&format!("{model_name}.FC")) {
            Some((7, model.forward_knee, format!("{model_name}.FC")))
        } else if name.eq_ignore_ascii_case(&format!("{}.AC", source.name)) {
            Some((8, *ac_magnitude, format!("{}.AC", source.name)))
        } else {
            None
        }
    };

    let mut entries = Vec::with_capacity(options.parameters.len());
    for name in &options.parameters {
        let Some((index, nominal, parameter)) = parameter_info(name) else {
            return Err(format!(
                "nonlinear AC derivative parameter '{name}' must be R, diode IS/TT/CJO/VJ/M/FC, DC, or AC magnitude"
            ));
        };
        let gradient = if index == 8 {
            unit_magnitude
        } else {
            ac_magnitude * unit_first[index]
        };
        entries.push(DerivativeEntry {
            parameter,
            nominal_parameter: nominal,
            gradient,
            hessian_row: Vec::with_capacity(options.parameters.len()),
        });
    }
    for entry in &mut entries {
        let Some((left, _, _)) = parameter_info(&entry.parameter) else {
            unreachable!("parameter was validated above");
        };
        for parameter in &options.parameters {
            let Some((right, _, _)) = parameter_info(parameter) else {
                unreachable!("parameter was validated above");
            };
            let value = if left == 8 && right == 8 {
                0.0
            } else if left == 8 {
                unit_first[right]
            } else if right == 8 {
                unit_first[left]
            } else {
                ac_magnitude * unit_second[left][right]
            };
            entry.hessian_row.push(value);
        }
    }

    Ok(DerivativeResult {
        target: DerivativeTarget::AcMagnitude,
        frequency_hz: options.frequency_hz,
        output_node: options.output_node.clone(),
        nominal_output: ac_magnitude * unit_magnitude,
        entries,
    })
}

pub(super) fn sensitivity_output(
    circuit: &SpiceCircuit,
    options: &SensitivityOptions,
) -> Result<f64, String> {
    match options.target {
        MonteCarloTarget::OperatingPoint => dc_operating_point(circuit)?
            .node_voltages
            .get(&options.output_node)
            .copied()
            .ok_or_else(|| format!("unknown sensitivity node '{}'", options.output_node)),
        MonteCarloTarget::TransientFinal => {
            let base = circuit
                .analyses
                .iter()
                .find_map(|analysis| match analysis {
                    Analysis::Transient { .. } => Some(analysis.clone()),
                    _ => None,
                })
                .ok_or_else(|| "sensitivity transient target lacks .tran".to_string())?;
            transient(circuit, &base)?
                .final_sample()
                .and_then(|sample| sample.node_voltages.get(&options.output_node))
                .copied()
                .ok_or_else(|| format!("unknown sensitivity node '{}'", options.output_node))
        }
        MonteCarloTarget::AcMagnitude => {
            let base = circuit
                .analyses
                .iter()
                .find_map(|analysis| match analysis {
                    Analysis::AcSweep { .. } => Some(analysis.clone()),
                    _ => None,
                })
                .ok_or_else(|| "sensitivity AC target lacks .ac".to_string())?;
            ac_sweep(circuit, &base)?
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
                .ok_or_else(|| format!("unknown sensitivity node '{}'", options.output_node))
        }
    }
}

pub(super) fn sensitivity_parameters(circuit: &SpiceCircuit) -> Vec<SensitivityParameter> {
    let mut out = Vec::new();
    for element in &circuit.elements {
        match &element.kind {
            SpiceElementKind::Resistor { resistance, .. } => out.push(SensitivityParameter {
                name: format!("{}.R", element.name),
                object: element.name.clone(),
                value: *resistance,
                kind: SensitivityParameterKind::Resistance,
            }),
            SpiceElementKind::Capacitor { capacitance, .. } => out.push(SensitivityParameter {
                name: format!("{}.C", element.name),
                object: element.name.clone(),
                value: *capacitance,
                kind: SensitivityParameterKind::Capacitance,
            }),
            SpiceElementKind::Inductor { inductance, .. } => out.push(SensitivityParameter {
                name: format!("{}.L", element.name),
                object: element.name.clone(),
                value: *inductance,
                kind: SensitivityParameterKind::Inductance,
            }),
            SpiceElementKind::BipolarJunctionTransistor { area, .. } => {
                out.push(SensitivityParameter {
                    name: format!("{}.AREA", element.name),
                    object: element.name.clone(),
                    value: *area,
                    kind: SensitivityParameterKind::BjtArea,
                })
            }
            SpiceElementKind::VoltageSource {
                waveform,
                ac_magnitude,
                ..
            }
            | SpiceElementKind::CurrentSource {
                waveform,
                ac_magnitude,
                ..
            } => {
                if let SourceWaveform::Dc(value) = waveform {
                    out.push(SensitivityParameter {
                        name: format!("{}.DC", element.name),
                        object: element.name.clone(),
                        value: *value,
                        kind: SensitivityParameterKind::DcSource,
                    });
                }
                if let Some(value) = ac_magnitude {
                    out.push(SensitivityParameter {
                        name: format!("{}.AC", element.name),
                        object: element.name.clone(),
                        value: *value,
                        kind: SensitivityParameterKind::AcSource,
                    });
                }
            }
            _ => {}
        }
    }
    for (name, model) in &circuit.diode_models {
        out.push(SensitivityParameter {
            name: format!("{name}.IS"),
            object: format!("model:{name}"),
            value: model.saturation_current_a,
            kind: SensitivityParameterKind::DiodeIs,
        });
    }
    for (name, model) in &circuit.mosfet_models {
        match model.kind {
            MosfetModelKind::Level1 => {
                out.push(SensitivityParameter {
                    name: format!("{name}.VTO"),
                    object: format!("model:{name}"),
                    value: model.threshold_voltage_v,
                    kind: SensitivityParameterKind::MosVto,
                });
                out.push(SensitivityParameter {
                    name: format!("{name}.KP"),
                    object: format!("model:{name}"),
                    value: model.transconductance_a_per_v2,
                    kind: SensitivityParameterKind::MosKp,
                });
            }
            MosfetModelKind::Bsim3LongChannel => {
                for (field, value, kind) in [
                    (
                        "VTH0",
                        model.threshold_voltage_v,
                        SensitivityParameterKind::Bsim3Vth0,
                    ),
                    (
                        "K1",
                        model.body_effect_v_per_sqrt_v,
                        SensitivityParameterKind::Bsim3K1,
                    ),
                    (
                        "U0",
                        model.bsim3_low_field_mobility_m2_per_v_s,
                        SensitivityParameterKind::Bsim3U0,
                    ),
                    (
                        "TOX",
                        model.bsim3_oxide_thickness_m,
                        SensitivityParameterKind::Bsim3Tox,
                    ),
                    (
                        "VSAT",
                        model.bsim3_saturation_velocity_m_per_s,
                        SensitivityParameterKind::Bsim3Vsat,
                    ),
                    (
                        "DVT0",
                        model.bsim3_short_channel_threshold_amplitude,
                        SensitivityParameterKind::Bsim3Dvt0,
                    ),
                    (
                        "DVT1",
                        model.bsim3_short_channel_length_scale_m,
                        SensitivityParameterKind::Bsim3Dvt1,
                    ),
                    (
                        "DVT2",
                        model.bsim3_short_channel_drain_bias_coefficient_per_v,
                        SensitivityParameterKind::Bsim3Dvt2,
                    ),
                    (
                        "DVT0W",
                        model.bsim3_narrow_width_threshold_amplitude_v,
                        SensitivityParameterKind::Bsim3Dvt0w,
                    ),
                    (
                        "DVT1W",
                        model.bsim3_narrow_width_exponent,
                        SensitivityParameterKind::Bsim3Dvt1w,
                    ),
                    (
                        "W0",
                        model.bsim3_reference_width_m,
                        SensitivityParameterKind::Bsim3W0,
                    ),
                    (
                        "KT1",
                        model.bsim3_threshold_temperature_coefficient_v,
                        SensitivityParameterKind::Bsim3Kt1,
                    ),
                    (
                        "KT2",
                        model.bsim3_threshold_body_temperature_coefficient_v_per_v,
                        SensitivityParameterKind::Bsim3Kt2,
                    ),
                    (
                        "KT1L",
                        model.bsim3_threshold_length_temperature_coefficient_v_m,
                        SensitivityParameterKind::Bsim3Kt1l,
                    ),
                ] {
                    if matches!(
                        kind,
                        SensitivityParameterKind::Bsim3K1
                            | SensitivityParameterKind::Bsim3Dvt0
                            | SensitivityParameterKind::Bsim3Dvt2
                            | SensitivityParameterKind::Bsim3Dvt0w
                    ) && value == 0.0
                    {
                        continue;
                    }
                    out.push(SensitivityParameter {
                        name: format!("{name}.{field}"),
                        object: format!("model:{name}"),
                        value,
                        kind,
                    });
                }
            }
        }
    }
    for (name, model) in &circuit.bjt_models {
        out.push(SensitivityParameter {
            name: format!("{name}.BF"),
            object: format!("model:{name}"),
            value: model.forward_beta,
            kind: SensitivityParameterKind::BjtBeta,
        });
        out.push(SensitivityParameter {
            name: format!("{name}.IS"),
            object: format!("model:{name}"),
            value: model.saturation_current_a,
            kind: SensitivityParameterKind::BjtIs,
        });
    }
    out
}

pub(super) fn sensitivity_perturbation(
    parameter: &SensitivityParameter,
    relative_step: f64,
) -> f64 {
    let floor = match parameter.kind {
        SensitivityParameterKind::Resistance => 1.0,
        SensitivityParameterKind::Capacitance => 1e-15,
        SensitivityParameterKind::Inductance => 1e-12,
        SensitivityParameterKind::DcSource
        | SensitivityParameterKind::AcSource
        | SensitivityParameterKind::MosVto => 1e-6,
        SensitivityParameterKind::BjtArea => 1e-6,
        SensitivityParameterKind::Bsim3Vth0
        | SensitivityParameterKind::Bsim3K1
        | SensitivityParameterKind::Bsim3Dvt0
        | SensitivityParameterKind::Bsim3Dvt2
        | SensitivityParameterKind::Bsim3Dvt0w => 1e-6,
        SensitivityParameterKind::Bsim3Kt1 | SensitivityParameterKind::Bsim3Kt2 => 1e-6,
        SensitivityParameterKind::Bsim3Tox => 1e-15,
        SensitivityParameterKind::Bsim3Dvt1 => 1e-12,
        SensitivityParameterKind::Bsim3W0 => 1e-12,
        SensitivityParameterKind::Bsim3Kt1l => 1e-12,
        SensitivityParameterKind::Bsim3Vsat => 1.0,
        SensitivityParameterKind::Bsim3Dvt1w => 1e-6,
        _ => 0.0,
    };
    (parameter.value.abs() * relative_step).max(floor)
}

pub(super) fn perturbed_for_sensitivity(
    circuit: &SpiceCircuit,
    parameter: &SensitivityParameter,
    delta: f64,
) -> Result<SpiceCircuit, String> {
    let mut out = circuit.clone();
    let element = out
        .elements
        .iter_mut()
        .find(|element| element.name.eq_ignore_ascii_case(&parameter.object));
    if let Some(element) = element {
        match (&mut element.kind, parameter.kind) {
            (
                SpiceElementKind::Resistor { resistance, .. },
                SensitivityParameterKind::Resistance,
            ) => *resistance += delta,
            (
                SpiceElementKind::Capacitor { capacitance, .. },
                SensitivityParameterKind::Capacitance,
            ) => *capacitance += delta,
            (
                SpiceElementKind::Inductor { inductance, .. },
                SensitivityParameterKind::Inductance,
            ) => *inductance += delta,
            (
                SpiceElementKind::BipolarJunctionTransistor { area, .. },
                SensitivityParameterKind::BjtArea,
            ) => *area += delta,
            (
                SpiceElementKind::VoltageSource { waveform, .. }
                | SpiceElementKind::CurrentSource { waveform, .. },
                SensitivityParameterKind::DcSource,
            ) => {
                if let SourceWaveform::Dc(value) = waveform {
                    *value += delta;
                }
            }
            (
                SpiceElementKind::VoltageSource {
                    ac_magnitude: Some(magnitude),
                    ..
                },
                SensitivityParameterKind::AcSource,
            ) => *magnitude += delta,
            (
                SpiceElementKind::CurrentSource {
                    ac_magnitude: Some(magnitude),
                    ..
                },
                SensitivityParameterKind::AcSource,
            ) => *magnitude += delta,
            _ => {}
        }
    } else if let Some(model_name) = parameter.object.strip_prefix("model:") {
        match parameter.kind {
            SensitivityParameterKind::DiodeIs => {
                if let Some(model) = out.diode_models.get_mut(model_name) {
                    model.saturation_current_a += delta;
                }
            }
            SensitivityParameterKind::MosVto => {
                if let Some(model) = out.mosfet_models.get_mut(model_name) {
                    model.threshold_voltage_v += delta;
                }
            }
            SensitivityParameterKind::MosKp => {
                if let Some(model) = out.mosfet_models.get_mut(model_name) {
                    model.transconductance_a_per_v2 += delta;
                }
            }
            SensitivityParameterKind::BjtBeta => {
                if let Some(model) = out.bjt_models.get_mut(model_name) {
                    model.forward_beta += delta;
                }
            }
            SensitivityParameterKind::BjtIs => {
                if let Some(model) = out.bjt_models.get_mut(model_name) {
                    model.saturation_current_a += delta;
                }
            }
            SensitivityParameterKind::Bsim3Vth0 => {
                if let Some(model) = out.mosfet_models.get_mut(model_name) {
                    model.threshold_voltage_v += delta;
                }
            }
            SensitivityParameterKind::Bsim3K1 => {
                if let Some(model) = out.mosfet_models.get_mut(model_name) {
                    model.body_effect_v_per_sqrt_v += delta;
                }
            }
            SensitivityParameterKind::Bsim3U0 => {
                if let Some(model) = out.mosfet_models.get_mut(model_name) {
                    model.bsim3_low_field_mobility_m2_per_v_s += delta;
                }
            }
            SensitivityParameterKind::Bsim3Tox => {
                if let Some(model) = out.mosfet_models.get_mut(model_name) {
                    model.bsim3_oxide_thickness_m += delta;
                    model.gate_oxide_cap_per_m2 =
                        OXIDE_PERMITTIVITY_F_PER_M / model.bsim3_oxide_thickness_m;
                }
            }
            SensitivityParameterKind::Bsim3Vsat => {
                if let Some(model) = out.mosfet_models.get_mut(model_name) {
                    model.bsim3_saturation_velocity_m_per_s += delta;
                }
            }
            SensitivityParameterKind::Bsim3Dvt0 => {
                if let Some(model) = out.mosfet_models.get_mut(model_name) {
                    model.bsim3_short_channel_threshold_amplitude += delta;
                }
            }
            SensitivityParameterKind::Bsim3Dvt1 => {
                if let Some(model) = out.mosfet_models.get_mut(model_name) {
                    model.bsim3_short_channel_length_scale_m += delta;
                }
            }
            SensitivityParameterKind::Bsim3Dvt2 => {
                if let Some(model) = out.mosfet_models.get_mut(model_name) {
                    model.bsim3_short_channel_drain_bias_coefficient_per_v += delta;
                }
            }
            SensitivityParameterKind::Bsim3Dvt0w => {
                if let Some(model) = out.mosfet_models.get_mut(model_name) {
                    model.bsim3_narrow_width_threshold_amplitude_v += delta;
                }
            }
            SensitivityParameterKind::Bsim3Dvt1w => {
                if let Some(model) = out.mosfet_models.get_mut(model_name) {
                    model.bsim3_narrow_width_exponent += delta;
                }
            }
            SensitivityParameterKind::Bsim3W0 => {
                if let Some(model) = out.mosfet_models.get_mut(model_name) {
                    model.bsim3_reference_width_m += delta;
                }
            }
            SensitivityParameterKind::Bsim3Kt1 => {
                if let Some(model) = out.mosfet_models.get_mut(model_name) {
                    model.bsim3_threshold_temperature_coefficient_v += delta;
                }
            }
            SensitivityParameterKind::Bsim3Kt2 => {
                if let Some(model) = out.mosfet_models.get_mut(model_name) {
                    model.bsim3_threshold_body_temperature_coefficient_v_per_v += delta;
                }
            }
            SensitivityParameterKind::Bsim3Kt1l => {
                if let Some(model) = out.mosfet_models.get_mut(model_name) {
                    model.bsim3_threshold_length_temperature_coefficient_v_m += delta;
                }
            }
            _ => {}
        }
    }
    out.validate()?;
    Ok(out)
}

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
