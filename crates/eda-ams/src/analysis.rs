//! DC, AC, and transient circuit analysis.

use crate::types::*;
use crate::FlowCancellation;
use std::collections::BTreeMap;

mod derivatives;
mod devices;
mod optimization;
mod random;
mod solver;
mod statistical;

pub use derivatives::*;
use devices::*;
pub use optimization::*;
use solver::*;
#[cfg(test)]
pub(crate) use solver::{dynamic_capacitors, nonlinear_terminal_currents};
pub use statistical::*;

pub fn dc_operating_point(circuit: &SpiceCircuit) -> Result<DcOperatingPoint, String> {
    dc_operating_point_cancellable(circuit, &FlowCancellation::new())
}

/// Solve one DC operating point with cancellation at every Newton iteration.
pub fn dc_operating_point_cancellable(
    circuit: &SpiceCircuit,
    cancellation: &FlowCancellation,
) -> Result<DcOperatingPoint, String> {
    solve_time(
        circuit,
        0.0,
        None,
        None,
        IntegrationMethod::BackwardEuler,
        None,
        cancellation,
    )
}

pub fn dc_sweep(circuit: &SpiceCircuit, analysis: &Analysis) -> Result<DcSweepResult, String> {
    dc_sweep_cancellable(circuit, analysis, &FlowCancellation::new())
}

/// Run a DC sweep with cooperative cancellation at every point and Newton pass.
pub fn dc_sweep_cancellable(
    circuit: &SpiceCircuit,
    analysis: &Analysis,
    cancellation: &FlowCancellation,
) -> Result<DcSweepResult, String> {
    let Analysis::DcSweep {
        source,
        start,
        stop,
        step,
    } = analysis
    else {
        return Err("DC sweep called with a non-DC analysis".into());
    };
    match circuit.element(source) {
        Some(element)
            if matches!(
                element.kind,
                SpiceElementKind::VoltageSource { .. } | SpiceElementKind::CurrentSource { .. }
            ) => {}
        _ => return Err(format!("DC sweep source '{}' is missing", source)),
    }
    let count = sweep_point_count(*start, *stop, *step)?;
    let mut points = Vec::with_capacity(count);
    for idx in 0..count {
        if cancellation.is_cancelled() {
            return Err("DC sweep cancelled".into());
        }
        let value = if count == 1 {
            *start
        } else {
            start + (*stop - *start) * idx as f64 / (count - 1) as f64
        };
        let operating_point = solve_time(
            circuit,
            0.0,
            None,
            Some((source, value)),
            IntegrationMethod::BackwardEuler,
            None,
            cancellation,
        )?;
        points.push(DcSweepPoint {
            source_value: value,
            operating_point,
        });
    }
    Ok(DcSweepResult {
        source: source.clone(),
        points,
    })
}

pub fn transient(circuit: &SpiceCircuit, analysis: &Analysis) -> Result<TransientResult, String> {
    transient_cancellable(circuit, analysis, &FlowCancellation::new())
}

fn transistor_thermal_parameters(
    circuit: &SpiceCircuit,
    element: &SpiceElement,
) -> Result<Option<(f64, f64)>, String> {
    match &element.kind {
        SpiceElementKind::Mosfet {
            model,
            width_m,
            length_m,
            ..
        } => {
            let settings = circuit.mosfet_model_for(model, *width_m, *length_m)?;
            Ok(Some((
                settings.self_heating_thermal_resistance_k_per_w,
                settings.self_heating_thermal_capacitance_j_per_k,
            )))
        }
        SpiceElementKind::BipolarJunctionTransistor { model, .. } => {
            let settings = circuit
                .bjt_models
                .get(model)
                .ok_or_else(|| format!("unknown BJT model '{}'", model))?;
            Ok(Some((
                settings.self_heating_thermal_resistance_k_per_w,
                settings.self_heating_thermal_capacitance_j_per_k,
            )))
        }
        _ => Ok(None),
    }
}

fn sample_terminal_voltages(sample: &TransientSample, element: &SpiceElement) -> Vec<f64> {
    let mut values = vec![0.0; element.nodes.len().max(4)];
    for (index, node) in element.nodes.iter().take(4).enumerate() {
        values[index] = sample.node_voltages.get(node).copied().unwrap_or(0.0);
    }
    values
}

fn static_transistor_temperature(
    circuit: &SpiceCircuit,
    element: &SpiceElement,
    terminal_voltages: &[f64],
) -> Result<Option<f64>, String> {
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
                return Err("MOSFET needs four thermal voltage slots".into());
            };
            Ok(Some(
                self_heated_mos_state(
                    settings,
                    *width_m,
                    *length_m,
                    *multiplier,
                    polarity * (gate - source),
                    polarity * (drain - source),
                    polarity * (bulk - source),
                    circuit.temperature_k,
                )
                .1,
            ))
        }
        SpiceElementKind::BipolarJunctionTransistor { model, area } => {
            let settings = circuit
                .bjt_models
                .get(model)
                .ok_or_else(|| format!("unknown BJT model '{}'", model))?;
            let [collector, base, emitter, _substrate] = terminal_voltages else {
                return Err("BJT needs four thermal voltage slots".into());
            };
            Ok(Some(
                self_heated_bjt_state(
                    settings,
                    *area,
                    *collector,
                    *base,
                    *emitter,
                    circuit.temperature_k,
                )
                .1,
            ))
        }
        _ => Ok(None),
    }
}

fn update_device_temperatures(
    circuit: &SpiceCircuit,
    sample: &mut TransientSample,
    previous: Option<&TransientSample>,
    step_s: f64,
) -> Result<(), String> {
    for element in &circuit.elements {
        let Some((thermal_resistance, thermal_capacitance)) =
            transistor_thermal_parameters(circuit, element)?
        else {
            continue;
        };
        let terminal_voltages = sample_terminal_voltages(sample, element);
        if thermal_capacitance == 0.0 {
            let temperature = static_transistor_temperature(circuit, element, &terminal_voltages)?
                .ok_or_else(|| format!("missing static temperature for '{}'", element.name))?;
            sample
                .device_temperatures
                .insert(element.name.clone(), temperature);
            continue;
        }

        let Some(previous_temperature) = previous
            .and_then(|previous| previous.device_temperatures.get(&element.name))
            .copied()
        else {
            sample
                .device_temperatures
                .insert(element.name.clone(), circuit.temperature_k);
            continue;
        };
        let currents = nonlinear_terminal_currents_at_temperature(
            circuit,
            element,
            &terminal_voltages,
            Some(previous_temperature),
        )?;
        let power = terminal_voltages
            .iter()
            .zip(currents.iter())
            .map(|(voltage, current)| voltage * current)
            .sum::<f64>();
        // Integrate Cth*dT/dt = P - (T-Tamb)/Rth with power held at the
        // previous electrical step. This explicit-power / implicit-temperature
        // companion is stable for the thermal pole and keeps the electrical
        // Jacobian independent of an extra thermal unknown.
        let thermal_conductance_w_per_k = 1.0 / thermal_resistance;
        let denominator = 1.0 + step_s * thermal_conductance_w_per_k / thermal_capacitance;
        let temperature = if denominator.is_finite() && denominator > 0.0 {
            (previous_temperature
                + step_s * (power + thermal_conductance_w_per_k * circuit.temperature_k)
                    / thermal_capacitance)
                / denominator
        } else {
            circuit.temperature_k
        };
        sample
            .device_temperatures
            .insert(element.name.clone(), temperature);
    }
    Ok(())
}

/// Integrate a transient analysis with cooperative checks at every time step.
pub fn transient_cancellable(
    circuit: &SpiceCircuit,
    analysis: &Analysis,
    cancellation: &FlowCancellation,
) -> Result<TransientResult, String> {
    let Analysis::Transient {
        step_s,
        stop_s,
        method,
    } = analysis
    else {
        return Err("transient called with a non-transient analysis".into());
    };
    let steps = ((stop_s / step_s).ceil() as usize).saturating_add(1);
    if steps > MAX_TRANSIENT_STEPS {
        return Err(format!(
            "transient requires more than {} time steps",
            MAX_TRANSIENT_STEPS
        ));
    }
    let mut samples = Vec::with_capacity(steps);
    let mut previous: Option<TransientSample> = None;
    let mut convergence_failures = 0usize;
    for idx in 0..steps {
        if cancellation.is_cancelled() {
            return Err("transient analysis cancelled".into());
        }
        let time = (idx as f64 * step_s).min(*stop_s);
        let op = solve_time(
            circuit,
            time,
            Some(*step_s),
            None,
            *method,
            previous.as_ref(),
            cancellation,
        )?;
        if !op.converged {
            convergence_failures += 1;
        }
        let mut sample = TransientSample {
            time_s: time,
            node_voltages: op.node_voltages.clone(),
            branch_currents: op.branch_currents.clone(),
            capacitor_currents: BTreeMap::new(),
            device_temperatures: BTreeMap::new(),
        };
        update_device_temperatures(circuit, &mut sample, previous.as_ref(), *step_s)?;
        if let Some(prev) = &previous {
            for element in &circuit.elements {
                if let SpiceElementKind::Capacitor { capacitance, .. } = element.kind {
                    let v_now =
                        node_difference(&op.node_voltages, &element.nodes[0], &element.nodes[1]);
                    let v_prev =
                        node_difference(&prev.node_voltages, &element.nodes[0], &element.nodes[1]);
                    let current = match method {
                        IntegrationMethod::BackwardEuler => capacitance * (v_now - v_prev) / step_s,
                        IntegrationMethod::Trapezoidal => {
                            2.0 * capacitance * (v_now - v_prev) / step_s
                                - prev
                                    .capacitor_currents
                                    .get(&element.name)
                                    .copied()
                                    .unwrap_or(0.0)
                        }
                    };
                    sample
                        .capacitor_currents
                        .insert(element.name.clone(), current);
                }
            }
            for element in &circuit.elements {
                let thermal_temperature = sample.device_temperatures.get(&element.name).copied();
                for (name, plus, minus, capacitance) in dynamic_capacitors_at_temperature(
                    circuit,
                    element,
                    &prev.node_voltages,
                    thermal_temperature,
                )? {
                    let v_now = node_difference(&op.node_voltages, &plus, &minus);
                    let v_prev = node_difference(&prev.node_voltages, &plus, &minus);
                    let current = match method {
                        IntegrationMethod::BackwardEuler => capacitance * (v_now - v_prev) / step_s,
                        IntegrationMethod::Trapezoidal => {
                            2.0 * capacitance * (v_now - v_prev) / step_s
                                - prev.capacitor_currents.get(&name).copied().unwrap_or(0.0)
                        }
                    };
                    sample.capacitor_currents.insert(name, current);
                }
            }
        }
        samples.push(sample.clone());
        previous = Some(sample);
    }
    Ok(TransientResult {
        method: *method,
        step_s: *step_s,
        stop_s: *stop_s,
        samples,
        convergence_failures,
    })
}

pub fn ac_sweep(circuit: &SpiceCircuit, analysis: &Analysis) -> Result<AcResult, String> {
    let Analysis::AcSweep {
        kind,
        points,
        start_hz,
        stop_hz,
    } = analysis
    else {
        return Err("AC sweep called with a non-AC analysis".into());
    };
    let frequencies = frequencies(*kind, *points, *start_hz, *stop_hz)?;
    let op = dc_operating_point(circuit)?;
    let mut out = Vec::with_capacity(frequencies.len());
    for frequency in frequencies {
        let phasors = solve_ac(circuit, frequency, &op)?;
        out.push(AcPoint {
            frequency_hz: frequency,
            node_phasors: phasors,
        });
    }
    Ok(AcResult {
        kind: *kind,
        points: out,
    })
}

pub fn periodic_steady_state(
    circuit: &SpiceCircuit,
    analysis: &Analysis,
) -> Result<PssResult, String> {
    periodic_steady_state_cancellable(circuit, analysis, &FlowCancellation::new())
}

/// Search for periodic steady state with cooperative cancellation.
pub fn periodic_steady_state_cancellable(
    circuit: &SpiceCircuit,
    analysis: &Analysis,
    cancellation: &FlowCancellation,
) -> Result<PssResult, String> {
    let Analysis::PeriodicSteadyState(options) = analysis else {
        return Err("PSS called with a non-PSS analysis".into());
    };
    if cancellation.is_cancelled() {
        return Err("PSS analysis cancelled".into());
    }
    let base = circuit
        .analyses
        .iter()
        .find_map(|analysis| match analysis {
            Analysis::Transient { .. } => Some(analysis.clone()),
            _ => None,
        })
        .ok_or_else(|| "PSS requires a .tran analysis".to_string())?;
    let Analysis::Transient { method, .. } = base else {
        return Err("PSS could not recover transient method".into());
    };
    let mut fundamental_hz = options.fundamental_hz;
    let mut automatic_fundamental = false;
    if options.automatic_fundamental {
        if cancellation.is_cancelled() {
            return Err("PSS analysis cancelled".into());
        }
        let startup = transient_cancellable(circuit, &base, cancellation)?;
        let startup_times = startup.time();
        let startup_values = startup.node_values(&options.output_node)?;
        if startup_times.len() != startup_values.len() || startup_times.len() < 3 {
            return Err("automatic PSS needs a longer transient waveform".into());
        }
        let min = startup_values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = startup_values
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let threshold = options.threshold.unwrap_or((min + max) / 2.0);
        let mut rising_times = Vec::new();
        for (time_pair, value_pair) in startup_times.windows(2).zip(startup_values.windows(2)) {
            let left_time = time_pair[0];
            let right_time = time_pair[1];
            let left_value = value_pair[0];
            let right_value = value_pair[1];
            if left_value < threshold && right_value >= threshold && right_value != left_value {
                let alpha = (threshold - left_value) / (right_value - left_value);
                rising_times.push(left_time + alpha * (right_time - left_time));
            }
        }
        let intervals: Vec<_> = rising_times
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .filter(|interval| *interval > 0.0)
            .collect();
        if intervals.len() < 2 {
            return Err("automatic PSS found fewer than three rising threshold crossings".into());
        }
        let use_count = intervals.len().min(8);
        let mut stable: Vec<_> = intervals[intervals.len() - use_count..].to_vec();
        stable.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
        let median_interval = stable[stable.len() / 2];
        if !median_interval.is_finite() || median_interval <= 0.0 {
            return Err("automatic PSS estimated a non-positive period".into());
        }
        fundamental_hz = 1.0 / median_interval;
        automatic_fundamental = true;
    }
    let period = 1.0 / fundamental_hz;
    let step = period / options.samples_per_cycle as f64;
    let transient_analysis = Analysis::Transient {
        step_s: step,
        stop_s: period * options.maximum_cycles as f64,
        method,
    };
    if cancellation.is_cancelled() {
        return Err("PSS analysis cancelled".into());
    }
    let waveform = transient_cancellable(circuit, &transient_analysis, cancellation)?;
    let times = waveform.time();
    let values = waveform.node_values(&options.output_node)?;

    let value_at = |time_s: f64| -> Result<f64, String> {
        if time_s < times.first().copied().unwrap_or(0.0) - step {
            return Err("PSS interpolation requested time before transient start".into());
        }
        if time_s > times.last().copied().unwrap_or(0.0) + step {
            return Err("PSS interpolation requested time after transient end".into());
        }
        let idx = times
            .partition_point(|time| *time <= time_s)
            .saturating_sub(1);
        let idx = idx.min(times.len().saturating_sub(2));
        let left = times[idx];
        let right = times.get(idx + 1).copied().unwrap_or(left + step);
        if (right - left).abs() <= f64::EPSILON {
            return Ok(values[idx]);
        }
        let alpha = ((time_s - left) / (right - left)).clamp(0.0, 1.0);
        Ok(values[idx] + (values[idx + 1] - values[idx]) * alpha)
    };

    let cycle_waveform = |cycle_index: usize| -> Result<Vec<f64>, String> {
        (0..options.samples_per_cycle)
            .map(|sample| {
                value_at(
                    period * cycle_index as f64
                        + period * sample as f64 / options.samples_per_cycle as f64,
                )
            })
            .collect()
    };

    let mut cycles = options.minimum_cycles;
    let mut converged = false;
    let mut residual = f64::INFINITY;
    let mut steady = cycle_waveform(options.minimum_cycles)?;
    for cycle_index in options.minimum_cycles..=options.maximum_cycles {
        if cancellation.is_cancelled() {
            return Err("PSS analysis cancelled".into());
        }
        if cycle_index < 2 {
            continue;
        }
        let current = cycle_waveform(cycle_index)?;
        let previous = cycle_waveform(cycle_index - 1)?;
        let error = current
            .iter()
            .zip(previous.iter())
            .map(|(next, old)| (next - old).abs())
            .fold(0.0_f64, f64::max);
        steady = current;
        cycles = cycle_index;
        residual = error;
        if error <= options.tolerance {
            converged = true;
            break;
        }
    }
    if !residual.is_finite() {
        return Err("PSS residual is non-finite".into());
    }

    let samples = steady
        .iter()
        .enumerate()
        .map(|(index, value)| PssSample {
            phase: std::f64::consts::TAU * index as f64 / options.samples_per_cycle as f64,
            time_s: period * index as f64 / options.samples_per_cycle as f64,
            value: *value,
        })
        .collect();
    let mut harmonics = Vec::with_capacity(options.harmonics);
    for harmonic_index in 0..options.harmonics {
        if cancellation.is_cancelled() {
            return Err("PSS analysis cancelled".into());
        }
        let mut phasor = crate::numeric::Complex::zero();
        for (sample_index, value) in steady.iter().enumerate() {
            let angle = -std::f64::consts::TAU * harmonic_index as f64 * sample_index as f64
                / options.samples_per_cycle as f64;
            phasor = phasor.add(crate::numeric::Complex::polar(*value, angle).scale(
                if harmonic_index == 0 { 1.0 } else { 2.0 } / options.samples_per_cycle as f64,
            ));
        }
        harmonics.push(PssHarmonic {
            index: harmonic_index,
            frequency_hz: fundamental_hz * harmonic_index as f64,
            phasor: Phasor {
                real: phasor.re,
                imaginary: phasor.im,
            },
        });
    }

    Ok(PssResult {
        output_node: options.output_node.clone(),
        fundamental_hz,
        automatic_fundamental,
        cycles,
        converged,
        residual,
        samples,
        harmonics,
    })
}

pub fn envelope_follow(
    circuit: &SpiceCircuit,
    analysis: &Analysis,
) -> Result<EnvelopeResult, String> {
    let Analysis::EnvelopeFollow(options) = analysis else {
        return Err("envelope following called with a non-envelope analysis".into());
    };
    let base = circuit
        .analyses
        .iter()
        .find_map(|analysis| match analysis {
            Analysis::Transient { .. } => Some(analysis.clone()),
            _ => None,
        })
        .ok_or_else(|| "envelope following requires a .tran analysis".to_string())?;
    let waveform = transient(circuit, &base)?;
    let times = waveform.time();
    let values = waveform.node_values(&options.output_node)?;
    let window_s = options.carrier_cycles / options.carrier_hz;
    let first = times.first().copied().unwrap_or(0.0);
    let last = times.last().copied().unwrap_or(0.0);
    if last - first < 2.0 * window_s {
        return Err("envelope transient must span at least two averaging windows".into());
    }
    let transient_step = if times.len() > 1 {
        (last - first) / (times.len() - 1) as f64
    } else {
        f64::INFINITY
    };
    if transient_step > 0.25 / options.carrier_hz {
        return Err("envelope transient step is too coarse for the carrier".into());
    }

    let start = first + window_s / 2.0;
    let stop = last - window_s / 2.0;
    let mut samples = Vec::with_capacity(options.points);
    for point_index in 0..options.points {
        let center = start + (stop - start) * point_index as f64 / (options.points - 1) as f64;
        let lower = center - window_s / 2.0;
        let upper = center + window_s / 2.0;
        let begin = times.partition_point(|time| *time < lower);
        let end = times.partition_point(|time| *time <= upper);
        let count = end.saturating_sub(begin);
        if count == 0 {
            return Err("envelope averaging window contains no transient samples".into());
        }
        let mut in_phase = 0.0_f64;
        let mut quadrature = 0.0_f64;
        for index in begin..end {
            let angle = std::f64::consts::TAU * options.carrier_hz * times[index];
            in_phase += 2.0 * values[index] * angle.cos();
            quadrature += -2.0 * values[index] * angle.sin();
        }
        in_phase /= count as f64;
        quadrature /= count as f64;
        samples.push(EnvelopeSample {
            time_s: center,
            envelope: in_phase.hypot(quadrature),
            phase_rad: quadrature.atan2(in_phase),
        });
    }
    let envelopes: Vec<_> = samples.iter().map(|sample| sample.envelope).collect();
    let minimum = envelopes.iter().copied().fold(f64::INFINITY, f64::min);
    let maximum = envelopes.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let average = envelopes.iter().sum::<f64>() / envelopes.len() as f64;
    if !minimum.is_finite() || !maximum.is_finite() || !average.is_finite() {
        return Err("envelope following produced a non-finite value".into());
    }
    Ok(EnvelopeResult {
        output_node: options.output_node.clone(),
        carrier_hz: options.carrier_hz,
        window_s,
        minimum,
        maximum,
        average,
        samples,
    })
}

pub fn aging_analysis(circuit: &SpiceCircuit, analysis: &Analysis) -> Result<AgingResult, String> {
    let Analysis::Aging(options) = analysis else {
        return Err("aging called with a non-aging analysis".into());
    };
    circuit.validate()?;
    let local_options = SensitivityOptions {
        target: options.target,
        output_node: options.output_node.clone(),
        frequency_hz: options.frequency_hz,
        relative_step: 0.001,
    };
    let nominal_output = sensitivity_output(circuit, &local_options)?;
    let stress = options.age_years.powf(options.time_exponent);
    let mut aged = circuit.clone();
    let resistance_factor = 1.0 + options.resistance_increase_per_year * stress;
    let threshold_shift = options.mos_threshold_shift_v_per_year * stress;
    let kp_factor = 1.0 - options.mos_kp_degradation_per_year * stress;

    for element in &mut aged.elements {
        if let SpiceElementKind::Resistor { resistance, .. } = &mut element.kind {
            *resistance *= resistance_factor;
        }
    }
    for model in aged.mosfet_models.values_mut() {
        let direction = match model.polarity {
            MosfetPolarity::Nmos => 1.0,
            MosfetPolarity::Pmos => -1.0,
        };
        model.threshold_voltage_v += direction * threshold_shift;
        model.transconductance_a_per_v2 *= kp_factor;
    }
    aged.validate()?;
    let aged_output = sensitivity_output(&aged, &local_options)?;
    if !nominal_output.is_finite() || !aged_output.is_finite() || !stress.is_finite() {
        return Err("aging analysis produced a non-finite output".into());
    }
    Ok(AgingResult {
        target: options.target,
        output_node: options.output_node.clone(),
        age_years: options.age_years,
        time_exponent: options.time_exponent,
        stress,
        nominal_output,
        aged_output,
        circuit: aged,
    })
}

pub fn noise_sweep(circuit: &SpiceCircuit, analysis: &Analysis) -> Result<NoiseResult, String> {
    let Analysis::NoiseSweep {
        output_node,
        input_source,
        kind,
        points,
        start_hz,
        stop_hz,
    } = analysis
    else {
        return Err("noise sweep called with a non-noise analysis".into());
    };
    let frequencies = frequencies(*kind, *points, *start_hz, *stop_hz)?;
    let operating_point = dc_operating_point(circuit)?;
    let mut out = Vec::with_capacity(frequencies.len());

    for frequency in frequencies {
        let contributions = noise_contributions(circuit, &operating_point, frequency)?;
        let mut total_noise_squared = 0.0_f64;
        let mut contribution_density = BTreeMap::new();
        for (name, plus, minus, current_density) in &contributions {
            let response = injected_noise_response(circuit, output_node, frequency, plus, minus)?;
            let output_density = current_density * response.abs();
            total_noise_squared += output_density * output_density;
            contribution_density.insert(name.clone(), output_density);
        }
        let output_noise = total_noise_squared.sqrt();
        let gain = input_source_gain(circuit, input_source, output_node, frequency)?;
        if !gain.is_finite() || gain <= f64::EPSILON {
            return Err(format!(
                "noise input source '{}' has zero transfer to V({}) at {} Hz",
                input_source, output_node, frequency
            ));
        }
        out.push(NoisePoint {
            frequency_hz: frequency,
            output_v_per_sqrt_hz: output_noise,
            input_referred_per_sqrt_hz: output_noise / gain,
            contributions: contribution_density,
        });
    }

    Ok(NoiseResult {
        output_node: output_node.clone(),
        input_source: input_source.clone(),
        points: out,
    })
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;

    #[test]
    fn quantized_refine_loop_honors_cancellation() {
        let circuit = crate::parse_spice(
            "quantized cancel\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim match V(out) goal=1.2 params=R2.R:100:5000:1000:100 population=5 iterations=2 seed=7\n.end",
        )
        .unwrap();
        let Analysis::Optimization(options) = &circuit.analyses[0] else {
            panic!("expected optimization analysis");
        };
        let local_options = SensitivityOptions {
            target: options.target,
            output_node: options.output_node.clone(),
            frequency_hz: options.frequency_hz,
            relative_step: 0.001,
        };
        let specification = options.parameters[0].clone();
        let nominal = sensitivity_parameters(&circuit)
            .into_iter()
            .find(|parameter| {
                parameter
                    .name
                    .eq_ignore_ascii_case(&specification.parameter)
            })
            .unwrap();
        let initial = optimization_engineering_value(0.5, &specification);
        let mut values = vec![0.5];
        let mut result = (circuit.clone(), 0.0, 0.0);
        let mut evaluations = 0;
        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            refine_quantized_optimum(
                &circuit,
                &[(specification, nominal, initial)],
                &local_options,
                options,
                &mut values,
                &mut result,
                &mut evaluations,
                &cancellation,
            )
            .unwrap_err(),
            "optimization analysis cancelled"
        );
    }
}
