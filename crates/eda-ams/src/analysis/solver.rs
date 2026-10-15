//! Nonlinear MNA, DC/transient Newton solves, AC solves, and device stamps.

use super::dc_operating_point;
use super::devices::*;
use crate::numeric::{solve_complex, solve_real, Complex};
use crate::types::*;
use crate::FlowCancellation;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn solve_time(
    circuit: &SpiceCircuit,
    time_s: f64,
    step_s: Option<f64>,
    source_override: Option<(&str, f64)>,
    method: IntegrationMethod,
    previous: Option<&TransientSample>,
    cancellation: &FlowCancellation,
) -> Result<DcOperatingPoint, String> {
    if cancellation.is_cancelled() {
        return Err("DC/transient solve cancelled".into());
    }
    circuit.validate()?;
    let mut guess = vec![0.0; SystemModel::new(circuit)?.size];
    if let Some(previous) = previous {
        let system = SystemModel::new(circuit)?;
        for (name, value) in &previous.node_voltages {
            if let Some(index) = system.node_index(name) {
                guess[index] = *value;
            }
        }
        for (name, value) in &previous.branch_currents {
            if let Some(index) = system.branch_index(name) {
                guess[index] = *value;
            }
        }
    } else {
        let system = SystemModel::new(circuit)?;
        for element in &circuit.elements {
            if let SpiceElementKind::Inductor {
                initial_current, ..
            } = element.kind
            {
                if let Some(index) = system.branch_index(&element.name) {
                    guess[index] = initial_current;
                }
            }
        }
    }

    let initial_result = newton_for_circuit(
        circuit,
        time_s,
        step_s,
        source_override,
        method,
        previous,
        &guess,
        cancellation,
    );
    let (mut solution, mut iterations, mut converged) = if circuit.bjt_models.is_empty() {
        initial_result?
    } else {
        match initial_result {
            Ok(result) => result,
            Err(_) => (guess, 0, false),
        }
    };
    if !circuit.diode_models.is_empty() {
        // Shockley equations can have a poor initial Newton basin when a
        // forward-biased junction is initialized at zero volts. Continue from
        // a nearly linear junction (large N) to the requested model.
        let targets: Vec<(String, f64)> = circuit
            .diode_models
            .iter()
            .map(|(name, model)| (name.clone(), model.emission_coefficient))
            .collect();
        let mut continued = circuit.clone();
        for factor in [
            256.0, 181.0, 128.0, 90.5, 64.0, 45.3, 32.0, 22.6, 16.0, 11.3, 8.0, 5.7, 4.0, 2.8, 2.0,
            1.4, 1.0, 1.0,
        ] {
            for (name, target) in &targets {
                if let Some(model) = continued.diode_models.get_mut(name) {
                    model.emission_coefficient = target * factor;
                }
            }
            let stage_guess = if factor == 256.0 {
                vec![0.0; SystemModel::new(&continued)?.size]
            } else {
                solution.clone()
            };
            let (stage, stage_iterations, stage_converged) = newton_for_circuit(
                &continued,
                time_s,
                step_s,
                source_override,
                method,
                previous,
                &stage_guess,
                cancellation,
            )?;
            iterations += stage_iterations;
            converged = stage_converged;
            solution = stage;
            if !converged {
                break;
            }
        }
    }
    if !circuit.bjt_models.is_empty() {
        // Exponential junctions are easiest to solve by ramping independent
        // sources from zero while reusing each converged stage as the next
        // Newton seed.
        for factor in [
            0.0, 1e-7, 1e-6, 1e-5, 1e-4, 1e-3, 1e-2, 5e-2, 1e-1, 2.5e-1, 5e-1, 7.5e-1, 1.0,
        ] {
            let mut continued = circuit.clone();
            for element in &mut continued.elements {
                let waveform = match &mut element.kind {
                    SpiceElementKind::VoltageSource { waveform, .. }
                    | SpiceElementKind::CurrentSource { waveform, .. } => waveform,
                    _ => continue,
                };
                if let SourceWaveform::Dc(value) = waveform {
                    *value *= if factor == 0.0 { 0.0 } else { factor };
                }
            }
            let stage_guess = if factor == 0.0 {
                vec![0.0; SystemModel::new(&continued)?.size]
            } else {
                solution.clone()
            };
            let (stage, stage_iterations, stage_converged) = newton_for_circuit(
                &continued,
                time_s,
                step_s,
                source_override,
                method,
                previous,
                &stage_guess,
                cancellation,
            )?;
            iterations += stage_iterations;
            converged = stage_converged;
            solution = stage;
            if !converged {
                break;
            }
        }
    }
    if !converged {
        return Err("DC/transient Newton iteration did not converge".into());
    }
    let system = SystemModel::new(circuit)?;
    Ok(system.extract_operating_point(&solution, iterations))
}

#[allow(clippy::too_many_arguments)]
fn newton_for_circuit(
    circuit: &SpiceCircuit,
    time_s: f64,
    step_s: Option<f64>,
    source_override: Option<(&str, f64)>,
    method: IntegrationMethod,
    previous: Option<&TransientSample>,
    initial: &[f64],
    cancellation: &FlowCancellation,
) -> Result<(Vec<f64>, usize, bool), String> {
    let system = SystemModel::new(circuit)?;
    let mut solution = initial.to_vec();
    if solution.len() != system.size {
        solution.resize(system.size, 0.0);
    }
    let mut converged = false;
    let mut iterations = 0usize;
    for _ in 0..MAX_NEWTON_ITERATIONS {
        if cancellation.is_cancelled() {
            return Err("Newton iteration cancelled".into());
        }
        iterations += 1;
        let (matrix, rhs) = build_real_system(
            circuit,
            &system,
            time_s,
            step_s,
            source_override,
            method,
            previous,
            &solution,
        )?;
        let check_matrix = matrix.clone();
        let next = solve_real(matrix, rhs.clone())?;
        let delta = next
            .iter()
            .zip(solution.iter())
            .map(|(new, old)| (new - old).abs() / (1.0 + new.abs().max(old.abs())))
            .fold(0.0_f64, f64::max);
        let residual = check_matrix
            .iter()
            .enumerate()
            .map(|(row, coefficients)| {
                let product: f64 = coefficients
                    .iter()
                    .zip(next.iter())
                    .map(|(coefficient, value)| coefficient * value)
                    .sum();
                (product - rhs[row]).abs()
            })
            .fold(0.0_f64, f64::max);
        solution = next;
        if delta <= 1e-10 && residual <= 1e-8 {
            converged = true;
            break;
        }
    }
    Ok((solution, iterations, converged))
}

pub(crate) fn nonlinear_terminal_currents(
    circuit: &SpiceCircuit,
    element: &SpiceElement,
    terminal_voltages: &[f64],
) -> Result<[f64; 4], String> {
    nonlinear_terminal_currents_at_temperature(circuit, element, terminal_voltages, None)
}

pub(super) fn nonlinear_terminal_currents_at_temperature(
    circuit: &SpiceCircuit,
    element: &SpiceElement,
    terminal_voltages: &[f64],
    thermal_temperature_k: Option<f64>,
) -> Result<[f64; 4], String> {
    match &element.kind {
        SpiceElementKind::Mosfet {
            model,
            width_m,
            length_m,
            multiplier,
        } => {
            let settings = circuit.mosfet_model_for(model, *width_m, *length_m)?;
            let polarity = match settings.polarity {
                MosfetPolarity::Nmos => 1.0,
                MosfetPolarity::Pmos => -1.0,
            };
            let [drain, gate, source, bulk] = terminal_voltages else {
                return Err("MOSFET needs four terminal voltages".into());
            };
            let v_gs = polarity * (gate - source);
            let v_ds = polarity * (drain - source);
            let v_bs = polarity * (bulk - source);
            let current = match thermal_temperature_k {
                Some(temperature) => mos_drain_current_at_temperature(
                    settings,
                    *width_m,
                    *length_m,
                    *multiplier,
                    v_gs,
                    v_ds,
                    v_bs,
                    temperature,
                ),
                None => self_heated_mos_drain_current(
                    settings,
                    *width_m,
                    *length_m,
                    *multiplier,
                    v_gs,
                    v_ds,
                    v_bs,
                    circuit.temperature_k,
                ),
            } * polarity;
            Ok([current, 0.0, -current, 0.0])
        }
        SpiceElementKind::BipolarJunctionTransistor { model, area } => {
            let settings = circuit
                .bjt_models
                .get(model)
                .ok_or_else(|| format!("unknown BJT model '{}'", model))?;
            let [collector, base, emitter, _substrate] = terminal_voltages else {
                return Err("BJT needs four terminal-voltage slots".into());
            };
            Ok(match thermal_temperature_k {
                Some(temperature) => bjt_terminal_currents_at_temperature(
                    settings,
                    *area,
                    *collector,
                    *base,
                    *emitter,
                    temperature,
                ),
                None => self_heated_bjt_terminal_currents(
                    settings,
                    *area,
                    *collector,
                    *base,
                    *emitter,
                    circuit.temperature_k,
                ),
            })
        }
        _ => Err("element is not a compact nonlinear transistor".into()),
    }
}

fn terminal_voltages(
    system: &SystemModel,
    element: &SpiceElement,
    guess: &[f64],
) -> Result<Vec<f64>, String> {
    let mut values = vec![0.0; element.nodes.len().max(4)];
    for (idx, node) in element.nodes.iter().enumerate() {
        values[idx] = system
            .node_index(node)
            .map(|index| guess[index])
            .unwrap_or(0.0);
    }
    Ok(values)
}

type DynamicCapacitor = (String, String, String, f64);

pub(crate) fn dynamic_capacitors(
    circuit: &SpiceCircuit,
    element: &SpiceElement,
    voltages: &BTreeMap<String, f64>,
) -> Result<Vec<DynamicCapacitor>, String> {
    dynamic_capacitors_at_temperature(circuit, element, voltages, None)
}

pub(super) fn dynamic_capacitors_at_temperature(
    circuit: &SpiceCircuit,
    element: &SpiceElement,
    voltages: &BTreeMap<String, f64>,
    thermal_temperature_k: Option<f64>,
) -> Result<Vec<DynamicCapacitor>, String> {
    let mut out = Vec::new();
    match &element.kind {
        SpiceElementKind::Mosfet {
            model,
            width_m,
            length_m,
            multiplier,
        } => {
            let settings = circuit.mosfet_model_for(model, *width_m, *length_m)?;
            let gate = &element.nodes[1];
            let source = &element.nodes[2];
            let drain = &element.nodes[0];
            let bulk = &element.nodes[3];
            let polarity = match settings.polarity {
                MosfetPolarity::Nmos => 1.0,
                MosfetPolarity::Pmos => -1.0,
            };
            let vgs = polarity * node_difference(voltages, gate, source);
            let vds = polarity * node_difference(voltages, drain, source);
            let vbs = polarity * node_difference(voltages, bulk, source);
            let vbd = polarity * node_difference(voltages, bulk, drain);
            let local_temperature = match thermal_temperature_k {
                Some(temperature) => temperature,
                None => {
                    self_heated_mos_state(
                        settings,
                        *width_m,
                        *length_m,
                        *multiplier,
                        vgs,
                        vds,
                        vbs,
                        circuit.temperature_k,
                    )
                    .1
                }
            };
            let threshold =
                mos_threshold(settings, vbs, *length_m, *width_m, vds, local_temperature);
            let overdrive = vgs - threshold;
            let effective_kp = mos_effective_kp(settings, overdrive, vbs, local_temperature);
            let saturation_vds =
                mos_saturation_drain_voltage(settings, overdrive, effective_kp, *length_m);
            let gate_oxide_cap = settings.gate_oxide_cap_per_m2 * width_m * length_m * multiplier;
            let (intrinsic_gs, intrinsic_gd, intrinsic_gb) = if gate_oxide_cap > 0.0 {
                if overdrive <= 0.0 {
                    (0.0, 0.0, gate_oxide_cap)
                } else if overdrive < 0.1 {
                    let alpha = overdrive / 0.1;
                    if vds < saturation_vds {
                        (
                            alpha * gate_oxide_cap * 0.5,
                            alpha * gate_oxide_cap * 0.5,
                            (1.0 - alpha) * gate_oxide_cap,
                        )
                    } else {
                        let channel = alpha * (2.0 / 3.0) * gate_oxide_cap;
                        (
                            channel * (1.0 - settings.saturation_charge_partition),
                            channel * settings.saturation_charge_partition,
                            (1.0 - alpha) * gate_oxide_cap,
                        )
                    }
                } else if vds < saturation_vds {
                    (gate_oxide_cap * 0.5, gate_oxide_cap * 0.5, 0.0)
                } else {
                    let channel = (2.0 / 3.0) * gate_oxide_cap;
                    (
                        channel * (1.0 - settings.saturation_charge_partition),
                        channel * settings.saturation_charge_partition,
                        0.0,
                    )
                }
            } else {
                (0.0, 0.0, 0.0)
            };
            let cgs = settings.gate_source_overlap_cap_per_m * width_m * multiplier + intrinsic_gs;
            let cgd = settings.gate_drain_overlap_cap_per_m * width_m * multiplier + intrinsic_gd;
            let cgb =
                settings.gate_bulk_cap_per_m2 * width_m * length_m * multiplier + intrinsic_gb;
            if cgs > 0.0 {
                out.push((
                    format!("{}:cgs", element.name),
                    gate.clone(),
                    source.clone(),
                    cgs,
                ));
            }
            if cgd > 0.0 {
                out.push((
                    format!("{}:cgd", element.name),
                    gate.clone(),
                    drain.clone(),
                    cgd,
                ));
            }
            if cgb > 0.0 {
                out.push((
                    format!("{}:cgb", element.name),
                    gate.clone(),
                    bulk.clone(),
                    cgb,
                ));
            }
            let cbs = junction_capacitance(
                settings.source_junction_sidewall_cap_per_m * width_m * multiplier,
                settings.junction_potential_v,
                settings.junction_grading_coefficient,
                settings.junction_forward_knee,
                vbs,
            );
            if cbs > 0.0 {
                out.push((
                    format!("{}:cbs", element.name),
                    bulk.clone(),
                    source.clone(),
                    cbs,
                ));
            }
            let cbd = junction_capacitance(
                settings.drain_junction_sidewall_cap_per_m * width_m * multiplier,
                settings.junction_potential_v,
                settings.junction_grading_coefficient,
                settings.junction_forward_knee,
                vbd,
            );
            if cbd > 0.0 {
                out.push((
                    format!("{}:cbd", element.name),
                    bulk.clone(),
                    drain.clone(),
                    cbd,
                ));
            }
        }
        SpiceElementKind::BipolarJunctionTransistor { model, area } => {
            let settings = circuit
                .bjt_models
                .get(model)
                .ok_or_else(|| format!("unknown BJT model '{}'", model))?;
            let base = &element.nodes[1];
            let emitter = &element.nodes[2];
            let collector = &element.nodes[0];
            let polarity = match settings.polarity {
                BjtPolarity::Npn => 1.0,
                BjtPolarity::Pnp => -1.0,
            };
            let vbe = polarity * node_difference(voltages, base, emitter);
            let vbc = polarity * node_difference(voltages, base, collector);
            let local_temperature = match thermal_temperature_k {
                Some(temperature) => temperature,
                None => {
                    self_heated_bjt_state(
                        settings,
                        *area,
                        *voltages.get(collector).unwrap_or(&0.0),
                        *voltages.get(base).unwrap_or(&0.0),
                        *voltages.get(emitter).unwrap_or(&0.0),
                        circuit.temperature_k,
                    )
                    .1
                }
            };
            let cbe = junction_capacitance(
                settings.base_emitter_zero_bias_cap_f,
                settings.base_emitter_junction_potential_v,
                settings.base_emitter_grading_coefficient,
                settings.junction_forward_knee,
                vbe,
            ) * area;
            let bjt_saturation_current = temperature_scaled_saturation_current(
                settings.saturation_current_a,
                1.0,
                settings.energy_gap_ev,
                settings.saturation_current_temperature_exponent,
                local_temperature,
            ) * area;
            let bjt_thermal_voltage = thermal_voltage_at(local_temperature, 1.0);
            let forward_charge_capacitance = settings.forward_transit_time_s
                * bjt_saturation_current
                * (vbe / bjt_thermal_voltage).clamp(-80.0, 80.0).exp()
                / bjt_thermal_voltage;
            let reverse_charge_capacitance = settings.reverse_transit_time_s
                * bjt_saturation_current
                * (vbc / bjt_thermal_voltage).clamp(-80.0, 80.0).exp()
                / bjt_thermal_voltage;
            let cbc = junction_capacitance(
                settings.base_collector_zero_bias_cap_f,
                settings.base_collector_junction_potential_v,
                settings.base_collector_grading_coefficient,
                settings.junction_forward_knee,
                vbc,
            ) * area;
            let cbe = cbe + forward_charge_capacitance;
            let cbc = cbc + reverse_charge_capacitance;
            if cbe > 0.0 {
                out.push((
                    format!("{}:cbe", element.name),
                    base.clone(),
                    emitter.clone(),
                    cbe,
                ));
            }
            if cbc > 0.0 {
                out.push((
                    format!("{}:cbc", element.name),
                    base.clone(),
                    collector.clone(),
                    cbc,
                ));
            }
        }
        SpiceElementKind::Diode { model } => {
            let settings = circuit
                .diode_models
                .get(model)
                .ok_or_else(|| format!("unknown diode model '{}'", model))?;
            let voltage = node_difference(voltages, &element.nodes[0], &element.nodes[1]);
            let capacitance = junction_capacitance(
                settings.zero_bias_junction_cap_f,
                settings.junction_potential_v,
                settings.grading_coefficient,
                settings.forward_knee,
                voltage,
            );
            let diode_saturation_current = temperature_scaled_saturation_current(
                settings.saturation_current_a,
                settings.emission_coefficient,
                settings.energy_gap_ev,
                settings.saturation_current_temperature_exponent,
                circuit.temperature_k,
            );
            let diode_thermal_voltage =
                thermal_voltage_at(circuit.temperature_k, settings.emission_coefficient);
            let diffusion_capacitance = settings.transit_time_s
                * diode_saturation_current
                * (voltage / diode_thermal_voltage).clamp(-80.0, 80.0).exp()
                / diode_thermal_voltage;
            let capacitance = capacitance + diffusion_capacitance;
            if capacitance > 0.0 {
                out.push((
                    format!("{}:cj", element.name),
                    element.nodes[0].clone(),
                    element.nodes[1].clone(),
                    capacitance,
                ));
            }
        }
        _ => {}
    }
    Ok(out)
}

fn junction_capacitance(
    zero_bias_capacitance: f64,
    junction_potential: f64,
    grading_coefficient: f64,
    forward_knee: f64,
    voltage: f64,
) -> f64 {
    if zero_bias_capacitance <= 0.0 {
        return 0.0;
    }
    let knee_voltage = forward_knee * junction_potential;
    if voltage <= knee_voltage {
        let denominator = (1.0 - voltage / junction_potential).max(0.01);
        zero_bias_capacitance / denominator.powf(grading_coefficient)
    } else {
        let knee_factor = 1.0 - forward_knee;
        let knee_cap = zero_bias_capacitance / knee_factor.powf(grading_coefficient);
        let derivative = knee_cap * grading_coefficient / (junction_potential * knee_factor);
        // SPICE uses a linear continuation above FC to keep the capacitance
        // finite while preserving C(V) continuity at the knee.
        knee_cap + derivative * (voltage - knee_voltage) / knee_factor
    }
}

#[allow(clippy::too_many_arguments)]
fn stamp_internal_capacitor(
    a: &mut [Vec<f64>],
    b: &mut [f64],
    system: &SystemModel,
    plus: &str,
    minus: &str,
    capacitance: f64,
    step_s: f64,
    method: IntegrationMethod,
    previous: &TransientSample,
    name: &str,
) {
    let vprev = node_difference(&previous.node_voltages, plus, minus);
    let conductance = match method {
        IntegrationMethod::BackwardEuler => capacitance / step_s,
        IntegrationMethod::Trapezoidal => 2.0 * capacitance / step_s,
    };
    let equivalent_current = match method {
        IntegrationMethod::BackwardEuler => conductance * vprev,
        IntegrationMethod::Trapezoidal => {
            conductance * vprev
                + previous
                    .capacitor_currents
                    .get(name)
                    .copied()
                    .unwrap_or(0.0)
        }
    };
    stamp_conductance(a, system, plus, minus, conductance);
    if let Some(row) = system.node_index(plus) {
        b[row] += equivalent_current;
    }
    if let Some(row) = system.node_index(minus) {
        b[row] -= equivalent_current;
    }
}

fn linearized_transistor_stamp(
    circuit: &SpiceCircuit,
    system: &SystemModel,
    a: &mut [Vec<f64>],
    b: &mut [f64],
    element: &SpiceElement,
    guess: &[f64],
    thermal_temperature_k: Option<f64>,
) -> Result<(), String> {
    let voltages = terminal_voltages(system, element, guess)?;
    let currents = nonlinear_terminal_currents_at_temperature(
        circuit,
        element,
        &voltages,
        thermal_temperature_k,
    )?;
    let mut jacobian = [[0.0_f64; 4]; 4];
    let epsilon = 1e-6_f64;
    for terminal in 0..element.nodes.len().min(4) {
        let mut increased = voltages.clone();
        let mut decreased = voltages.clone();
        increased[terminal] += epsilon;
        decreased[terminal] -= epsilon;
        let high = nonlinear_terminal_currents_at_temperature(
            circuit,
            element,
            &increased,
            thermal_temperature_k,
        )?;
        let low = nonlinear_terminal_currents_at_temperature(
            circuit,
            element,
            &decreased,
            thermal_temperature_k,
        )?;
        for row in 0..4 {
            jacobian[row][terminal] = (high[row] - low[row]) / (2.0 * epsilon);
        }
    }
    for (row_terminal, node) in element.nodes.iter().enumerate().take(4) {
        let Some(row) = system.node_index(node) else {
            continue;
        };
        a[row][row] += GMIN;
        let mut constant = currents[row_terminal];
        for (column_terminal, control) in element.nodes.iter().enumerate().take(4) {
            if let Some(column) = system.node_index(control) {
                a[row][column] += jacobian[row_terminal][column_terminal];
                constant -= jacobian[row_terminal][column_terminal] * voltages[column_terminal];
            }
        }
        b[row] -= constant;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_real_system(
    circuit: &SpiceCircuit,
    system: &SystemModel,
    time_s: f64,
    step_s: Option<f64>,
    source_override: Option<(&str, f64)>,
    method: IntegrationMethod,
    previous: Option<&TransientSample>,
    guess: &[f64],
) -> Result<(Vec<Vec<f64>>, Vec<f64>), String> {
    let size = system.size;
    let mut a = vec![vec![0.0; size]; size];
    let mut b = vec![0.0; size];
    let mut initial_capacitor_groups = BTreeSet::new();

    for element in &circuit.elements {
        match &element.kind {
            SpiceElementKind::Resistor { .. } => {
                let effective = element_effective_resistance(circuit, element)?;
                stamp_conductance(
                    &mut a,
                    system,
                    &element.nodes[0],
                    &element.nodes[1],
                    1.0 / effective,
                );
            }
            SpiceElementKind::Capacitor { .. } => {
                let branch = system
                    .branch_index(&element.name)
                    .ok_or_else(|| format!("missing branch for capacitor '{}'", element.name))?;
                stamp_branch_current(&mut a, system, branch, &element.nodes[0], &element.nodes[1]);
                if step_s.is_some() && previous.is_none() {
                    // UIC at t=0 fixes the capacitor voltage difference.
                    let SpiceElementKind::Capacitor {
                        initial_voltage, ..
                    } = element.kind
                    else {
                        unreachable!()
                    };
                    let key = if element.nodes[0] <= element.nodes[1] {
                        (element.nodes[0].clone(), element.nodes[1].clone())
                    } else {
                        (element.nodes[1].clone(), element.nodes[0].clone())
                    };
                    if initial_capacitor_groups.insert(key) {
                        stamp_node_coefficient(&mut a, system, branch, &element.nodes[0], 1.0);
                        stamp_node_coefficient(&mut a, system, branch, &element.nodes[1], -1.0);
                        b[branch] = initial_voltage;
                    } else {
                        // Parallel UIC capacitors share one voltage constraint.
                        // Extra branch-current unknowns are assigned zero; the
                        // first branch carries the aggregate initial impulse.
                        a[branch][branch] = 1.0;
                    }
                } else {
                    // The companion conductance eliminates the branch current
                    // from KCL; this row keeps the auxiliary unknown bounded.
                    a[branch][branch] = 1.0;
                }
                if let (Some(h), Some(prev)) = (step_s, previous) {
                    let SpiceElementKind::Capacitor { capacitance, .. } = element.kind else {
                        unreachable!()
                    };
                    let (conductance, equivalent_current) = match method {
                        IntegrationMethod::BackwardEuler => {
                            let g = capacitance / h;
                            let vprev = node_difference(
                                &prev.node_voltages,
                                &element.nodes[0],
                                &element.nodes[1],
                            );
                            (g, g * vprev)
                        }
                        IntegrationMethod::Trapezoidal => {
                            let g = 2.0 * capacitance / h;
                            let vprev = node_difference(
                                &prev.node_voltages,
                                &element.nodes[0],
                                &element.nodes[1],
                            );
                            let iprev = prev
                                .capacitor_currents
                                .get(&element.name)
                                .copied()
                                .unwrap_or(0.0);
                            (g, g * vprev + iprev)
                        }
                    };
                    stamp_conductance(
                        &mut a,
                        system,
                        &element.nodes[0],
                        &element.nodes[1],
                        conductance,
                    );
                    // The companion is i(v) = g*v - i_eq, so i_eq enters the
                    // RHS positively at the positive node.
                    if let Some(row) = system.node_index(&element.nodes[0]) {
                        b[row] += equivalent_current;
                    }
                    if let Some(row) = system.node_index(&element.nodes[1]) {
                        b[row] -= equivalent_current;
                    }
                }
            }
            SpiceElementKind::Inductor {
                inductance,
                initial_current,
            } => {
                let branch = system
                    .branch_index(&element.name)
                    .ok_or_else(|| format!("missing branch for inductor '{}'", element.name))?;
                stamp_branch_current(&mut a, system, branch, &element.nodes[0], &element.nodes[1]);
                let (resistance, rhs) = match (step_s, previous) {
                    (Some(h), Some(prev)) => {
                        let iprev = prev
                            .branch_currents
                            .get(&element.name)
                            .copied()
                            .unwrap_or(*initial_current);
                        match method {
                            IntegrationMethod::BackwardEuler => {
                                let r = inductance / h;
                                (r, -r * iprev)
                            }
                            IntegrationMethod::Trapezoidal => {
                                let r = 2.0 * inductance / h;
                                let vprev = node_difference(
                                    &prev.node_voltages,
                                    &element.nodes[0],
                                    &element.nodes[1],
                                );
                                (r, -(r * iprev + vprev))
                            }
                        }
                    }
                    _ => (0.0, 0.0),
                };
                if step_s.is_some() && previous.is_none() {
                    // UIC at t=0: fix the branch current to its declared IC.
                    stamp_branch_current(
                        &mut a,
                        system,
                        branch,
                        &element.nodes[0],
                        &element.nodes[1],
                    );
                    a[branch][branch] = 1.0;
                    b[branch] = *initial_current;
                } else {
                    stamp_node_coefficient(&mut a, system, branch, &element.nodes[0], 1.0);
                    stamp_node_coefficient(&mut a, system, branch, &element.nodes[1], -1.0);
                    a[branch][branch] = -resistance;
                    b[branch] = rhs;
                }
            }
            SpiceElementKind::VoltageSource { waveform, .. } => {
                let branch = system
                    .branch_index(&element.name)
                    .ok_or_else(|| format!("missing branch for source '{}'", element.name))?;
                stamp_branch_current(&mut a, system, branch, &element.nodes[0], &element.nodes[1]);
                stamp_node_coefficient(&mut a, system, branch, &element.nodes[0], 1.0);
                stamp_node_coefficient(&mut a, system, branch, &element.nodes[1], -1.0);
                let value = source_override
                    .filter(|(name, _)| name.eq_ignore_ascii_case(&element.name))
                    .map(|(_, value)| value)
                    .unwrap_or_else(|| waveform.value_at(time_s));
                b[branch] = value;
            }
            SpiceElementKind::CurrentSource { waveform, .. } => {
                let value = source_override
                    .filter(|(name, _)| name.eq_ignore_ascii_case(&element.name))
                    .map(|(_, value)| value)
                    .unwrap_or_else(|| waveform.value_at(time_s));
                stamp_current_source(&mut b, system, &element.nodes[0], &element.nodes[1], value);
            }
            SpiceElementKind::VoltageControlledVoltageSource { gain } => {
                let branch = system
                    .branch_index(&element.name)
                    .ok_or_else(|| format!("missing branch for VCVS '{}'", element.name))?;
                stamp_branch_current(&mut a, system, branch, &element.nodes[0], &element.nodes[1]);
                stamp_node_coefficient(&mut a, system, branch, &element.nodes[0], 1.0);
                stamp_node_coefficient(&mut a, system, branch, &element.nodes[1], -1.0);
                stamp_node_coefficient(&mut a, system, branch, &element.nodes[2], -*gain);
                stamp_node_coefficient(&mut a, system, branch, &element.nodes[3], *gain);
            }
            SpiceElementKind::VoltageControlledCurrentSource { transconductance } => {
                stamp_vccs(
                    &mut a,
                    system,
                    [
                        &element.nodes[0],
                        &element.nodes[1],
                        &element.nodes[2],
                        &element.nodes[3],
                    ],
                    *transconductance,
                );
            }
            SpiceElementKind::CurrentControlledVoltageSource {
                controlling_source,
                transresistance,
            } => {
                let branch = system
                    .branch_index(&element.name)
                    .ok_or_else(|| format!("missing branch for CCVS '{}'", element.name))?;
                let control = system.branch_index(controlling_source).ok_or_else(|| {
                    format!("missing controlling branch '{}'", controlling_source)
                })?;
                stamp_branch_current(&mut a, system, branch, &element.nodes[0], &element.nodes[1]);
                stamp_node_coefficient(&mut a, system, branch, &element.nodes[0], 1.0);
                stamp_node_coefficient(&mut a, system, branch, &element.nodes[1], -1.0);
                a[branch][control] -= *transresistance;
            }
            SpiceElementKind::CurrentControlledCurrentSource {
                controlling_source,
                gain,
            } => {
                let branch = system.branch_index(controlling_source).ok_or_else(|| {
                    format!("missing controlling branch '{}'", controlling_source)
                })?;
                stamp_branch_current_scaled(
                    &mut a,
                    system,
                    branch,
                    &element.nodes[0],
                    &element.nodes[1],
                    *gain,
                );
            }
            SpiceElementKind::Diode { model } => {
                stamp_diode_newton(circuit, system, &mut a, &mut b, element, model, guess)?;
                if let (Some(h), Some(previous)) = (step_s, previous) {
                    for (name, plus, minus, capacitance) in
                        dynamic_capacitors(circuit, element, &previous.node_voltages)?
                    {
                        stamp_internal_capacitor(
                            &mut a,
                            &mut b,
                            system,
                            &plus,
                            &minus,
                            capacitance,
                            h,
                            method,
                            previous,
                            &name,
                        );
                    }
                }
            }
            SpiceElementKind::Mosfet { .. }
            | SpiceElementKind::BipolarJunctionTransistor { .. } => {
                let thermal_temperature_k = previous
                    .and_then(|previous| previous.device_temperatures.get(&element.name))
                    .copied();
                linearized_transistor_stamp(
                    circuit,
                    system,
                    &mut a,
                    &mut b,
                    element,
                    guess,
                    thermal_temperature_k,
                )?;
                if let (Some(h), Some(previous)) = (step_s, previous) {
                    for (name, plus, minus, capacitance) in dynamic_capacitors_at_temperature(
                        circuit,
                        element,
                        &previous.node_voltages,
                        thermal_temperature_k,
                    )? {
                        stamp_internal_capacitor(
                            &mut a,
                            &mut b,
                            system,
                            &plus,
                            &minus,
                            capacitance,
                            h,
                            method,
                            previous,
                            &name,
                        );
                    }
                }
            }
        }
    }
    Ok((a, b))
}

pub(super) fn solve_ac(
    circuit: &SpiceCircuit,
    frequency_hz: f64,
    op: &DcOperatingPoint,
) -> Result<BTreeMap<String, Phasor>, String> {
    let system = SystemModel::new(circuit)?;
    let size = system.size;
    let mut a = vec![vec![Complex::zero(); size]; size];
    let mut b = vec![Complex::zero(); size];
    // Capacitor and inductor branch currents are eliminated by their phasor
    // admittances. Give only those auxiliary rows an identity equation; source
    // and controlled-source branch rows receive their own constitutive
    // equations and must not retain a hidden identity term.
    for element in &circuit.elements {
        if matches!(
            element.kind,
            SpiceElementKind::Capacitor { .. } | SpiceElementKind::Inductor { .. }
        ) {
            let branch = system
                .branch_index(&element.name)
                .ok_or_else(|| format!("missing branch for element '{}'", element.name))?;
            a[branch][branch] = Complex::new(1.0, 0.0);
        }
    }
    let omega = std::f64::consts::TAU * frequency_hz;
    let j = Complex::new(0.0, 1.0);

    for element in &circuit.elements {
        match &element.kind {
            SpiceElementKind::Resistor { .. } => {
                let effective = element_effective_resistance(circuit, element)?;
                stamp_complex_conductance(
                    &mut a,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    Complex::new(1.0 / effective, 0.0),
                );
            }
            SpiceElementKind::Capacitor { capacitance, .. } => {
                stamp_complex_conductance(
                    &mut a,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    j.scale(omega * capacitance),
                );
            }
            SpiceElementKind::Inductor { inductance, .. } => {
                let admittance = Complex::new(1.0, 0.0).div(j.scale(omega * inductance))?;
                stamp_complex_conductance(
                    &mut a,
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
                    &mut a,
                    &system,
                    branch,
                    &element.nodes[0],
                    &element.nodes[1],
                );
                stamp_complex_node(
                    &mut a,
                    &system,
                    branch,
                    &element.nodes[0],
                    Complex::new(1.0, 0.0),
                );
                stamp_complex_node(
                    &mut a,
                    &system,
                    branch,
                    &element.nodes[1],
                    Complex::new(-1.0, 0.0),
                );
                let value = ac_magnitude
                    .map(|magnitude| Complex::polar(magnitude, (*ac_phase_deg).to_radians()))
                    .unwrap_or_else(Complex::zero);
                b[branch] = value;
            }
            SpiceElementKind::CurrentSource {
                ac_magnitude,
                ac_phase_deg,
                ..
            } => {
                if let Some(magnitude) = ac_magnitude {
                    let value = Complex::polar(*magnitude, (*ac_phase_deg).to_radians());
                    stamp_complex_current(
                        &mut b,
                        &system,
                        &element.nodes[0],
                        &element.nodes[1],
                        value,
                    );
                }
            }
            SpiceElementKind::VoltageControlledVoltageSource { gain } => {
                let branch = system
                    .branch_index(&element.name)
                    .ok_or_else(|| format!("missing branch for VCVS '{}'", element.name))?;
                stamp_complex_branch_current(
                    &mut a,
                    &system,
                    branch,
                    &element.nodes[0],
                    &element.nodes[1],
                );
                stamp_complex_node(
                    &mut a,
                    &system,
                    branch,
                    &element.nodes[0],
                    Complex::new(1.0, 0.0),
                );
                stamp_complex_node(
                    &mut a,
                    &system,
                    branch,
                    &element.nodes[1],
                    Complex::new(-1.0, 0.0),
                );
                stamp_complex_node(
                    &mut a,
                    &system,
                    branch,
                    &element.nodes[2],
                    Complex::new(-*gain, 0.0),
                );
                stamp_complex_node(
                    &mut a,
                    &system,
                    branch,
                    &element.nodes[3],
                    Complex::new(*gain, 0.0),
                );
            }
            SpiceElementKind::VoltageControlledCurrentSource { transconductance } => {
                stamp_complex_conductance_pair(
                    &mut a,
                    &system,
                    [
                        &element.nodes[0],
                        &element.nodes[1],
                        &element.nodes[2],
                        &element.nodes[3],
                    ],
                    Complex::new(*transconductance, 0.0),
                );
            }
            SpiceElementKind::CurrentControlledVoltageSource {
                controlling_source,
                transresistance,
            } => {
                let branch = system
                    .branch_index(&element.name)
                    .ok_or_else(|| format!("missing branch for CCVS '{}'", element.name))?;
                let control = system.branch_index(controlling_source).ok_or_else(|| {
                    format!("missing controlling branch '{}'", controlling_source)
                })?;
                stamp_complex_branch_current(
                    &mut a,
                    &system,
                    branch,
                    &element.nodes[0],
                    &element.nodes[1],
                );
                stamp_complex_node(
                    &mut a,
                    &system,
                    branch,
                    &element.nodes[0],
                    Complex::new(1.0, 0.0),
                );
                stamp_complex_node(
                    &mut a,
                    &system,
                    branch,
                    &element.nodes[1],
                    Complex::new(-1.0, 0.0),
                );
                a[branch][control] = a[branch][control].sub(Complex::new(*transresistance, 0.0));
            }
            SpiceElementKind::CurrentControlledCurrentSource {
                controlling_source,
                gain,
            } => {
                let control = system.branch_index(controlling_source).ok_or_else(|| {
                    format!("missing controlling branch '{}'", controlling_source)
                })?;
                stamp_complex_branch_scaled(
                    &mut a,
                    &system,
                    control,
                    &element.nodes[0],
                    &element.nodes[1],
                    Complex::new(*gain, 0.0),
                );
            }
            SpiceElementKind::Diode { model } => {
                let settings = circuit
                    .diode_models
                    .get(model)
                    .ok_or_else(|| format!("unknown diode model '{}'", model))?;
                let voltage =
                    node_difference(&op.node_voltages, &element.nodes[0], &element.nodes[1]);
                let vt = thermal_voltage_at(circuit.temperature_k, settings.emission_coefficient);
                let saturation_current = temperature_scaled_saturation_current(
                    settings.saturation_current_a,
                    settings.emission_coefficient,
                    settings.energy_gap_ev,
                    settings.saturation_current_temperature_exponent,
                    circuit.temperature_k,
                );
                let current = saturation_current * (voltage / vt).clamp(-80.0, 80.0).exp_m1();
                let conductance = (current + saturation_current) / vt + GMIN;
                stamp_complex_conductance(
                    &mut a,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    Complex::new(conductance, 0.0),
                );
                for (_name, plus, minus, capacitance) in
                    dynamic_capacitors(circuit, element, &op.node_voltages)?
                {
                    stamp_complex_conductance(
                        &mut a,
                        &system,
                        &plus,
                        &minus,
                        j.scale(omega * capacitance),
                    );
                }
            }
            SpiceElementKind::Mosfet { .. }
            | SpiceElementKind::BipolarJunctionTransistor { .. } => {
                let voltages = terminal_voltages(&system, element, &{
                    let mut values = vec![0.0; system.size];
                    for (node, voltage) in &op.node_voltages {
                        if let Some(index) = system.node_index(node) {
                            values[index] = *voltage;
                        }
                    }
                    values
                })?;
                let mut jacobian = [[0.0_f64; 4]; 4];
                let epsilon = 1e-6_f64;
                for terminal in 0..element.nodes.len().min(4) {
                    let mut increased = voltages.clone();
                    let mut decreased = voltages.clone();
                    increased[terminal] += epsilon;
                    decreased[terminal] -= epsilon;
                    let high = nonlinear_terminal_currents(circuit, element, &increased)?;
                    let low = nonlinear_terminal_currents(circuit, element, &decreased)?;
                    for row in 0..4 {
                        jacobian[row][terminal] = (high[row] - low[row]) / (2.0 * epsilon);
                    }
                }
                for (row_terminal, row_node) in element.nodes.iter().enumerate().take(4) {
                    let Some(row) = system.node_index(row_node) else {
                        continue;
                    };
                    for (column_terminal, column_node) in element.nodes.iter().enumerate().take(4) {
                        if let Some(column) = system.node_index(column_node) {
                            let coefficient =
                                Complex::new(jacobian[row_terminal][column_terminal], 0.0);
                            a[row][column] = a[row][column].add(coefficient);
                        }
                    }
                }
                for (_name, plus, minus, capacitance) in
                    dynamic_capacitors(circuit, element, &op.node_voltages)?
                {
                    stamp_complex_conductance(
                        &mut a,
                        &system,
                        &plus,
                        &minus,
                        j.scale(omega * capacitance),
                    );
                }
            }
        }
    }
    let x = solve_complex(a, b)?;
    let mut out = BTreeMap::new();
    for node in circuit.node_names() {
        if let Some(index) = system.node_index(&node) {
            out.insert(
                node,
                Phasor {
                    real: x[index].re,
                    imaginary: x[index].im,
                },
            );
        }
    }
    Ok(out)
}

type NoiseContribution = (String, String, String, f64);

pub(super) fn noise_contributions(
    circuit: &SpiceCircuit,
    operating_point: &DcOperatingPoint,
    frequency_hz: f64,
) -> Result<Vec<NoiseContribution>, String> {
    let mut out = Vec::new();
    for element in &circuit.elements {
        match &element.kind {
            SpiceElementKind::Resistor { .. } => {
                let effective = element_effective_resistance(circuit, element)?;
                let density =
                    (4.0 * BOLTZMANN_CONSTANT_J_PER_K * circuit.temperature_k / effective).sqrt();
                out.push((
                    element.name.clone(),
                    element.nodes[0].clone(),
                    element.nodes[1].clone(),
                    density,
                ));
            }
            SpiceElementKind::Diode { model } => {
                let settings = circuit
                    .diode_models
                    .get(model)
                    .ok_or_else(|| format!("unknown diode model '{}'", model))?;
                let voltage = node_difference(
                    &operating_point.node_voltages,
                    &element.nodes[0],
                    &element.nodes[1],
                );
                let vt = thermal_voltage_at(circuit.temperature_k, settings.emission_coefficient);
                let saturation_current = temperature_scaled_saturation_current(
                    settings.saturation_current_a,
                    settings.emission_coefficient,
                    settings.energy_gap_ev,
                    settings.saturation_current_temperature_exponent,
                    circuit.temperature_k,
                );
                let current = saturation_current * ((voltage / vt).clamp(-80.0, 80.0)).exp_m1();
                let shot_squared = 2.0 * ELECTRON_CHARGE_C * current.abs();
                let flicker_squared = settings.flicker_coefficient_a2
                    * current.abs().powf(settings.flicker_current_exponent)
                    / frequency_hz.powf(settings.flicker_frequency_exponent);
                let density = (shot_squared + flicker_squared).sqrt();
                out.push((
                    element.name.clone(),
                    element.nodes[0].clone(),
                    element.nodes[1].clone(),
                    density,
                ));
            }
            SpiceElementKind::Mosfet { model, .. } => {
                let SpiceElementKind::Mosfet {
                    width_m, length_m, ..
                } = &element.kind
                else {
                    unreachable!("MOS noise branch checked element kind");
                };
                let settings = circuit.mosfet_model_for(model, *width_m, *length_m)?;
                let mut terminals = vec![0.0; 4];
                for (index, node) in element.nodes.iter().enumerate().take(4) {
                    terminals[index] = operating_point
                        .node_voltages
                        .get(node)
                        .copied()
                        .unwrap_or(0.0);
                }
                let baseline = nonlinear_terminal_currents(circuit, element, &terminals)?;
                let epsilon = 1e-6_f64;
                terminals[1] += epsilon;
                let perturbed = nonlinear_terminal_currents(circuit, element, &terminals)?;
                let transconductance = ((perturbed[0] - baseline[0]) / epsilon).max(0.0);
                let drain_current = baseline[0].abs();
                let thermal_squared = (8.0 / 3.0)
                    * BOLTZMANN_CONSTANT_J_PER_K
                    * circuit.temperature_k
                    * transconductance;
                let flicker_squared = settings.flicker_coefficient_a2
                    * drain_current.powf(settings.flicker_current_exponent)
                    / frequency_hz.powf(settings.flicker_frequency_exponent);
                let density = (thermal_squared + flicker_squared).sqrt();
                out.push((
                    element.name.clone(),
                    element.nodes[0].clone(),
                    element.nodes[2].clone(),
                    density,
                ));
            }
            SpiceElementKind::BipolarJunctionTransistor { model, area } => {
                let settings = circuit
                    .bjt_models
                    .get(model)
                    .ok_or_else(|| format!("unknown BJT model '{}'", model))?;
                let collector = operating_point
                    .node_voltages
                    .get(&element.nodes[0])
                    .copied()
                    .unwrap_or(0.0);
                let base = operating_point
                    .node_voltages
                    .get(&element.nodes[1])
                    .copied()
                    .unwrap_or(0.0);
                let emitter = operating_point
                    .node_voltages
                    .get(&element.nodes[2])
                    .copied()
                    .unwrap_or(0.0);
                let polarity = match settings.polarity {
                    BjtPolarity::Npn => 1.0,
                    BjtPolarity::Pnp => -1.0,
                };
                let thermal_voltage = thermal_voltage_at(circuit.temperature_k, 1.0);
                let saturation_current = temperature_scaled_saturation_current(
                    settings.saturation_current_a,
                    1.0,
                    settings.energy_gap_ev,
                    settings.saturation_current_temperature_exponent,
                    circuit.temperature_k,
                );
                let forward = saturation_current
                    * area
                    * ((polarity * (base - emitter) / thermal_voltage).clamp(-80.0, 80.0)).exp_m1();
                let reverse = saturation_current
                    * area
                    * ((polarity * (base - collector) / thermal_voltage).clamp(-80.0, 80.0))
                        .exp_m1();
                let base_current =
                    forward.abs() / settings.forward_beta + reverse.abs() / settings.reverse_beta;
                let be_shot_squared = 2.0 * ELECTRON_CHARGE_C * forward.abs();
                let be_flicker_squared = settings.flicker_coefficient_a2
                    * base_current.powf(settings.flicker_current_exponent)
                    / frequency_hz.powf(settings.flicker_frequency_exponent);
                let be_density = (be_shot_squared + be_flicker_squared).sqrt();
                let bc_density = (2.0 * ELECTRON_CHARGE_C * reverse.abs()).sqrt();
                out.push((
                    format!("{}:be", element.name),
                    element.nodes[1].clone(),
                    element.nodes[2].clone(),
                    be_density,
                ));
                out.push((
                    format!("{}:bc", element.name),
                    element.nodes[1].clone(),
                    element.nodes[0].clone(),
                    bc_density,
                ));
            }
            _ => {}
        }
    }
    Ok(out)
}

fn clear_ac_sources(circuit: &mut SpiceCircuit) {
    for element in &mut circuit.elements {
        match &mut element.kind {
            SpiceElementKind::VoltageSource { ac_magnitude, .. }
            | SpiceElementKind::CurrentSource { ac_magnitude, .. } => *ac_magnitude = None,
            _ => {}
        }
    }
}

pub(super) fn injected_noise_response(
    circuit: &SpiceCircuit,
    output_node: &str,
    frequency_hz: f64,
    plus: &str,
    minus: &str,
) -> Result<f64, String> {
    let mut noisy = circuit.clone();
    clear_ac_sources(&mut noisy);
    noisy.add_element(SpiceElement {
        name: "__noise_probe".into(),
        nodes: vec![plus.into(), minus.into()],
        kind: SpiceElementKind::CurrentSource {
            waveform: SourceWaveform::Dc(0.0),
            ac_magnitude: Some(1.0),
            ac_phase_deg: 0.0,
        },
    })?;
    let operating_point = dc_operating_point(&noisy)?;
    let phasors = solve_ac(&noisy, frequency_hz, &operating_point)?;
    phasors
        .get(output_node)
        .map(Phasor::magnitude)
        .ok_or_else(|| format!("noise response has no output node '{}'", output_node))
}

pub(super) fn input_source_gain(
    circuit: &SpiceCircuit,
    source: &str,
    output_node: &str,
    frequency_hz: f64,
) -> Result<f64, String> {
    let mut transfer = circuit.clone();
    clear_ac_sources(&mut transfer);
    for element in &mut transfer.elements {
        let selected = element.name.eq_ignore_ascii_case(source);
        match &mut element.kind {
            SpiceElementKind::VoltageSource {
                ac_magnitude,
                ac_phase_deg,
                ..
            }
            | SpiceElementKind::CurrentSource {
                ac_magnitude,
                ac_phase_deg,
                ..
            } => {
                *ac_magnitude = if selected { Some(1.0) } else { None };
                *ac_phase_deg = 0.0;
            }
            _ => {}
        }
    }
    let operating_point = dc_operating_point(&transfer)?;
    let phasors = solve_ac(&transfer, frequency_hz, &operating_point)?;
    phasors
        .get(output_node)
        .map(Phasor::magnitude)
        .ok_or_else(|| format!("noise transfer has no output node '{}'", output_node))
}

pub(super) struct SystemModel<'a> {
    circuit: &'a SpiceCircuit,
    node_index: BTreeMap<String, usize>,
    branch_index: BTreeMap<String, usize>,
    pub(super) size: usize,
}

impl<'a> SystemModel<'a> {
    pub(super) fn new(circuit: &'a SpiceCircuit) -> Result<Self, String> {
        let mut node_index = BTreeMap::new();
        for node in circuit.node_names() {
            node_index.insert(node, node_index.len());
        }
        for element in &circuit.elements {
            if let SpiceElementKind::Diode { model } = &element.kind {
                let settings = circuit
                    .diode_models
                    .get(model)
                    .ok_or_else(|| format!("unknown diode model '{}'", model))?;
                if settings.series_resistance_ohm > 0.0 {
                    let internal = format!("diode:{}:internal", element.name);
                    node_index.insert(internal, node_index.len());
                }
            }
        }
        let mut branch_index = BTreeMap::new();
        for element in &circuit.elements {
            let needs_branch = matches!(
                element.kind,
                SpiceElementKind::VoltageSource { .. }
                    | SpiceElementKind::Capacitor { .. }
                    | SpiceElementKind::Inductor { .. }
                    | SpiceElementKind::VoltageControlledVoltageSource { .. }
                    | SpiceElementKind::CurrentControlledVoltageSource { .. }
            );
            if needs_branch {
                branch_index.insert(element.name.clone(), node_index.len() + branch_index.len());
            }
        }
        let size = node_index.len() + branch_index.len();
        if size == 0 {
            return Err("circuit has no non-ground nodes or unknowns".into());
        }
        Ok(Self {
            circuit,
            node_index,
            branch_index,
            size,
        })
    }

    pub(super) fn node_index(&self, name: &str) -> Option<usize> {
        if GROUND_ALIASES.contains(&name) {
            None
        } else {
            self.node_index.get(name).copied()
        }
    }

    pub(super) fn branch_index(&self, name: &str) -> Option<usize> {
        self.branch_index.get(name).copied()
    }

    pub(super) fn extract_operating_point(&self, x: &[f64], iterations: usize) -> DcOperatingPoint {
        let mut node_voltages = BTreeMap::new();
        for node in self.circuit.node_names() {
            if let Some(index) = self.node_index(&node) {
                node_voltages.insert(node, x[index]);
            }
        }
        let mut branch_currents = BTreeMap::new();
        for name in self.branch_index.keys() {
            branch_currents.insert(name.clone(), x[self.branch_index[name]]);
        }
        DcOperatingPoint {
            node_voltages,
            branch_currents,
            iterations,
            converged: true,
        }
    }
}

pub(super) fn stamp_conductance(
    a: &mut [Vec<f64>],
    system: &SystemModel,
    plus: &str,
    minus: &str,
    conductance: f64,
) {
    for (node, sign) in [(plus, 1.0), (minus, -1.0)] {
        if let Some(row) = system.node_index(node) {
            if let Some(plus_col) = system.node_index(plus) {
                a[row][plus_col] += sign * conductance;
            }
            if let Some(minus_col) = system.node_index(minus) {
                a[row][minus_col] -= sign * conductance;
            }
        }
    }
}

pub(super) fn stamp_current_source(
    b: &mut [f64],
    system: &SystemModel,
    plus: &str,
    minus: &str,
    current: f64,
) {
    if let Some(row) = system.node_index(plus) {
        b[row] -= current;
    }
    if let Some(row) = system.node_index(minus) {
        b[row] += current;
    }
}

fn stamp_branch_current(
    a: &mut [Vec<f64>],
    system: &SystemModel,
    branch: usize,
    plus: &str,
    minus: &str,
) {
    if let Some(row) = system.node_index(plus) {
        a[row][branch] += 1.0;
    }
    if let Some(row) = system.node_index(minus) {
        a[row][branch] -= 1.0;
    }
}

fn stamp_branch_current_scaled(
    a: &mut [Vec<f64>],
    system: &SystemModel,
    branch: usize,
    plus: &str,
    minus: &str,
    scale: f64,
) {
    if let Some(row) = system.node_index(plus) {
        a[row][branch] += scale;
    }
    if let Some(row) = system.node_index(minus) {
        a[row][branch] -= scale;
    }
}

pub(super) fn stamp_node_coefficient(
    a: &mut [Vec<f64>],
    system: &SystemModel,
    row: usize,
    node: &str,
    coefficient: f64,
) {
    if let Some(col) = system.node_index(node) {
        a[row][col] += coefficient;
    }
}

fn stamp_vccs(a: &mut [Vec<f64>], system: &SystemModel, nodes: [&str; 4], transconductance: f64) {
    let [out_plus, out_minus, control_plus, control_minus] = nodes;
    for (node, sign) in [(out_plus, 1.0), (out_minus, -1.0)] {
        if let Some(row) = system.node_index(node) {
            if let Some(col) = system.node_index(control_plus) {
                a[row][col] += sign * transconductance;
            }
            if let Some(col) = system.node_index(control_minus) {
                a[row][col] -= sign * transconductance;
            }
        }
    }
}

fn stamp_diode_newton(
    circuit: &SpiceCircuit,
    system: &SystemModel,
    a: &mut [Vec<f64>],
    b: &mut [f64],
    element: &SpiceElement,
    model_name: &str,
    guess: &[f64],
) -> Result<(), String> {
    let model = circuit
        .diode_models
        .get(model_name)
        .ok_or_else(|| format!("unknown diode model '{}'", model_name))?;
    let (anode, cathode) = if model.series_resistance_ohm > 0.0 {
        (
            format!("diode:{}:internal", element.name),
            element.nodes[1].clone(),
        )
    } else {
        (element.nodes[0].clone(), element.nodes[1].clone())
    };
    if model.series_resistance_ohm > 0.0 {
        stamp_conductance(
            a,
            system,
            &element.nodes[0],
            &anode,
            1.0 / model.series_resistance_ohm,
        );
    }
    let anode_index = system
        .node_index(&anode)
        .ok_or_else(|| format!("diode '{}' has no anode unknown", element.name))?;
    let cathode_index = system.node_index(&cathode);
    let voltage = guess[anode_index] - cathode_index.map(|index| guess[index]).unwrap_or(0.0);
    let vt = thermal_voltage_at(circuit.temperature_k, model.emission_coefficient);
    let saturation_current = temperature_scaled_saturation_current(
        model.saturation_current_a,
        model.emission_coefficient,
        model.energy_gap_ev,
        model.saturation_current_temperature_exponent,
        circuit.temperature_k,
    );
    let exponent = (voltage / vt).clamp(-80.0, 80.0);
    let current = saturation_current * exponent.exp_m1();
    let derivative = saturation_current * exponent.exp() / vt;
    let conductance = derivative.max(GMIN);
    stamp_conductance(a, system, &anode, &cathode, conductance);
    let linearized_current = current - conductance * voltage;
    if let Some(row) = system.node_index(&anode) {
        b[row] -= linearized_current;
    }
    if let Some(row) = system.node_index(&cathode) {
        b[row] += linearized_current;
    }
    Ok(())
}

pub(super) fn stamp_complex_conductance(
    a: &mut [Vec<Complex>],
    system: &SystemModel,
    plus: &str,
    minus: &str,
    conductance: Complex,
) {
    stamp_complex_conductance_pair(a, system, [plus, minus, plus, minus], conductance);
}

pub(super) fn stamp_complex_conductance_pair(
    a: &mut [Vec<Complex>],
    system: &SystemModel,
    nodes: [&str; 4],
    transconductance: Complex,
) {
    let [out_plus, out_minus, control_plus, control_minus] = nodes;
    for (node, sign) in [(out_plus, 1.0), (out_minus, -1.0)] {
        if let Some(row) = system.node_index(node) {
            if let Some(col) = system.node_index(control_plus) {
                a[row][col] = a[row][col].add(transconductance.scale(sign));
            }
            if let Some(col) = system.node_index(control_minus) {
                a[row][col] = a[row][col].sub(transconductance.scale(sign));
            }
        }
    }
}

pub(super) fn stamp_complex_current(
    b: &mut [Complex],
    system: &SystemModel,
    plus: &str,
    minus: &str,
    current: Complex,
) {
    if let Some(row) = system.node_index(plus) {
        b[row] = b[row].sub(current);
    }
    if let Some(row) = system.node_index(minus) {
        b[row] = b[row].add(current);
    }
}

pub(super) fn stamp_complex_branch_current(
    a: &mut [Vec<Complex>],
    system: &SystemModel,
    branch: usize,
    plus: &str,
    minus: &str,
) {
    stamp_complex_branch_scaled(a, system, branch, plus, minus, Complex::new(1.0, 0.0));
}

pub(super) fn stamp_complex_branch_scaled(
    a: &mut [Vec<Complex>],
    system: &SystemModel,
    branch: usize,
    plus: &str,
    minus: &str,
    scale: Complex,
) {
    if let Some(row) = system.node_index(plus) {
        a[row][branch] = a[row][branch].add(scale);
    }
    if let Some(row) = system.node_index(minus) {
        a[row][branch] = a[row][branch].sub(scale);
    }
}

pub(super) fn stamp_complex_node(
    a: &mut [Vec<Complex>],
    system: &SystemModel,
    row: usize,
    node: &str,
    coefficient: Complex,
) {
    if let Some(col) = system.node_index(node) {
        a[row][col] = a[row][col].add(coefficient);
    }
}

pub(super) fn node_difference(nodes: &BTreeMap<String, f64>, plus: &str, minus: &str) -> f64 {
    nodes.get(plus).copied().unwrap_or(0.0) - nodes.get(minus).copied().unwrap_or(0.0)
}

pub(super) fn sweep_point_count(start: f64, stop: f64, step: f64) -> Result<usize, String> {
    let count = ((stop - start) / step).abs().floor() as usize + 1;
    if count == 0 || count > MAX_SWEEP_POINTS {
        return Err(format!("DC sweep produces {} points", count));
    }
    Ok(count)
}

pub(super) fn frequencies(
    kind: AcSweepKind,
    points: usize,
    start: f64,
    stop: f64,
) -> Result<Vec<f64>, String> {
    if start > stop {
        return Err("AC start frequency cannot exceed stop frequency".into());
    }
    let mut out = Vec::new();
    match kind {
        AcSweepKind::Linear => {
            for idx in 0..points {
                let alpha = if points == 1 {
                    0.0
                } else {
                    idx as f64 / (points - 1) as f64
                };
                out.push(start + (stop - start) * alpha);
            }
        }
        AcSweepKind::Decade => {
            let decades = (stop / start).log10();
            let total = ((decades * points as f64).ceil() as usize).saturating_add(1);
            if total > MAX_SWEEP_POINTS {
                return Err("AC decade sweep is too large".into());
            }
            for idx in 0..total {
                let decade = idx as f64 / points as f64;
                out.push((start * 10f64.powf(decade)).min(stop));
            }
        }
        AcSweepKind::Octave => {
            let octaves = (stop / start).log2();
            let total = ((octaves * points as f64).ceil() as usize).saturating_add(1);
            if total > MAX_SWEEP_POINTS {
                return Err("AC octave sweep is too large".into());
            }
            for idx in 0..total {
                let octave = idx as f64 / points as f64;
                out.push((start * 2f64.powf(octave)).min(stop));
            }
        }
    }
    if let Some(last) = out.last_mut() {
        *last = stop;
    }
    Ok(out)
}
