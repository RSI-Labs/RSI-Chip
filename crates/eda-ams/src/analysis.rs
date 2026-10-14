//! DC, AC, and transient circuit analysis.

use crate::numeric::{solve_complex, solve_real, Complex};
use crate::types::*;
use crate::FlowCancellation;
use std::collections::{BTreeMap, BTreeSet};

const MAX_SWEEP_POINTS: usize = 1_000_000;
const MAX_TRANSIENT_STEPS: usize = 1_000_000;
const MAX_NEWTON_ITERATIONS: usize = 100;
const GMIN: f64 = 1e-12;
const BOLTZMANN_CONSTANT_J_PER_K: f64 = 1.380_649e-23;
const NOMINAL_TEMPERATURE_K: f64 = SPICE_NOMINAL_TEMPERATURE_K;
const ELECTRON_CHARGE_C: f64 = 1.602_176_634e-19;
const OXIDE_PERMITTIVITY_F_PER_M: f64 = 3.9 * 8.854_187_812_8e-12;

fn thermal_voltage_at(temperature_k: f64, emission_coefficient: f64) -> f64 {
    BOLTZMANN_CONSTANT_J_PER_K * temperature_k / ELECTRON_CHARGE_C * emission_coefficient
}

fn temperature_scaled_saturation_current(
    nominal_current_a: f64,
    emission_coefficient: f64,
    energy_gap_ev: f64,
    temperature_exponent: f64,
    temperature_k: f64,
) -> f64 {
    let ratio = temperature_k / NOMINAL_TEMPERATURE_K;
    let nominal_thermal_voltage =
        BOLTZMANN_CONSTANT_J_PER_K * NOMINAL_TEMPERATURE_K / ELECTRON_CHARGE_C;
    let exponent = (ratio - 1.0) * energy_gap_ev / (emission_coefficient * nominal_thermal_voltage);
    nominal_current_a
        * ratio.powf(temperature_exponent / emission_coefficient)
        * exponent.clamp(-160.0, 160.0).exp()
}

fn temperature_scaled_mos_kp(
    nominal_kp: f64,
    mobility_temperature_exponent: f64,
    temperature_k: f64,
) -> f64 {
    nominal_kp * (NOMINAL_TEMPERATURE_K / temperature_k).powf(mobility_temperature_exponent)
}

fn mos_effective_kp(
    settings: &MosfetModel,
    overdrive: f64,
    body_voltage: f64,
    temperature_k: f64,
) -> f64 {
    match settings.kind {
        MosfetModelKind::Level1 => {
            let temperature_kp = temperature_scaled_mos_kp(
                settings.transconductance_a_per_v2,
                settings.mobility_temperature_exponent,
                temperature_k,
            );
            if settings.mobility_degradation_inverse_v > 0.0 && overdrive > 0.0 {
                temperature_kp / (1.0 + settings.mobility_degradation_inverse_v * overdrive)
            } else {
                temperature_kp
            }
        }
        MosfetModelKind::Bsim3LongChannel => {
            let oxide_cap = OXIDE_PERMITTIVITY_F_PER_M / settings.bsim3_oxide_thickness_m;
            let normalized_overdrive = overdrive / settings.bsim3_oxide_thickness_m;
            let mobility = temperature_scaled_mos_kp(
                settings.bsim3_low_field_mobility_m2_per_v_s,
                settings.mobility_temperature_exponent,
                temperature_k,
            ) / (1.0
                + (settings.bsim3_mobility_degradation_first_order_m_per_v
                    + settings.bsim3_mobility_body_bias_coefficient_m2_per_v2 * body_voltage)
                    * normalized_overdrive
                + settings.bsim3_mobility_degradation_second_order_m2_per_v2
                    * normalized_overdrive
                    * normalized_overdrive);
            mobility * oxide_cap
        }
    }
}

fn mos_threshold(
    settings: &MosfetModel,
    body_voltage: f64,
    length_m: f64,
    width_m: f64,
    drain_voltage: f64,
    temperature_k: f64,
) -> f64 {
    let surface = settings.surface_potential_v.max(1e-6);
    let body = if settings.body_effect_v_per_sqrt_v > 0.0 {
        (surface - body_voltage).max(0.0).sqrt() - surface.sqrt()
    } else {
        0.0
    };
    match settings.kind {
        MosfetModelKind::Level1 => {
            settings.threshold_voltage_v.abs() + settings.body_effect_v_per_sqrt_v * body
        }
        MosfetModelKind::Bsim3LongChannel => {
            let short_channel = -(settings.bsim3_short_channel_threshold_amplitude
                * (-(length_m / settings.bsim3_short_channel_length_scale_m).clamp(-80.0, 80.0))
                    .exp())
                * (1.0 + settings.bsim3_short_channel_drain_bias_coefficient_per_v * drain_voltage);
            let narrow_width = settings.bsim3_narrow_width_threshold_amplitude_v
                * ((settings.bsim3_reference_width_m / width_m)
                    .powf(settings.bsim3_narrow_width_exponent)
                    - 1.0);
            let temperature_shift = (settings.bsim3_threshold_temperature_coefficient_v
                + settings.bsim3_threshold_body_temperature_coefficient_v_per_v * body_voltage
                - settings.bsim3_threshold_length_temperature_coefficient_v_m / length_m)
                * (temperature_k / NOMINAL_TEMPERATURE_K - 1.0);
            settings.threshold_voltage_v + settings.body_effect_v_per_sqrt_v * body
                - settings.bsim3_body_bias_threshold_coefficient * body_voltage
                + short_channel
                + narrow_width
                + temperature_shift
        }
    }
}

fn mos_saturation_drain_voltage(
    settings: &MosfetModel,
    overdrive: f64,
    effective_kp: f64,
    length_m: f64,
) -> f64 {
    match settings.kind {
        MosfetModelKind::Level1 => {
            if settings.velocity_saturation_voltage_v > 0.0 {
                overdrive / (1.0 + overdrive / settings.velocity_saturation_voltage_v)
            } else {
                overdrive
            }
        }
        MosfetModelKind::Bsim3LongChannel => {
            let mobility =
                effective_kp / (OXIDE_PERMITTIVITY_F_PER_M / settings.bsim3_oxide_thickness_m);
            overdrive
                / (1.0
                    + overdrive * mobility
                        / (2.0 * settings.bsim3_saturation_velocity_m_per_s * length_m))
        }
    }
}

fn effective_resistance(
    resistance: f64,
    first_temperature_coefficient_per_k: f64,
    second_temperature_coefficient_per_k2: f64,
    temperature_k: f64,
) -> f64 {
    let delta = temperature_k - NOMINAL_TEMPERATURE_K;
    resistance
        * (1.0
            + first_temperature_coefficient_per_k * delta
            + second_temperature_coefficient_per_k2 * delta * delta)
}

fn element_effective_resistance(
    circuit: &SpiceCircuit,
    element: &SpiceElement,
) -> Result<f64, String> {
    let SpiceElementKind::Resistor {
        resistance,
        first_temperature_coefficient_per_k,
        second_temperature_coefficient_per_k2,
        layer,
        ..
    } = &element.kind
    else {
        return Err(format!("element '{}' is not a resistor", element.name));
    };
    if let Some(layer_name) = layer {
        let model = circuit
            .resistor_layer_models
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(layer_name))
            .map(|(_, model)| model)
            .ok_or_else(|| format!("unknown resistor layer model '{layer_name}'"))?;
        return Ok(effective_resistance(
            *resistance,
            model.first_temperature_coefficient_per_k,
            model.second_temperature_coefficient_per_k2,
            circuit.temperature_k,
        ));
    }
    Ok(effective_resistance(
        *resistance,
        *first_temperature_coefficient_per_k,
        *second_temperature_coefficient_per_k2,
        circuit.temperature_k,
    ))
}

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
        };
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
                for (name, plus, minus, capacitance) in
                    dynamic_capacitors(circuit, element, &prev.node_voltages)?
                {
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

#[derive(Clone, Debug)]
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

fn monte_carlo_standard_factor(uniform: f64, distribution: MonteCarloDistribution) -> f64 {
    match distribution {
        MonteCarloDistribution::Uniform => uniform,
        MonteCarloDistribution::Normal | MonteCarloDistribution::Lognormal => {
            let probability = ((uniform.clamp(-1.0, 1.0) + 1.0) / 2.0).clamp(1e-12, 1.0 - 1e-12);
            normal_inverse_cdf(probability).clamp(-3.0, 3.0)
        }
    }
}

fn monte_carlo_relative_multiplier(
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SensitivityParameterKind {
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
struct SensitivityParameter {
    name: String,
    object: String,
    value: f64,
    kind: SensitivityParameterKind,
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

fn sensitivity_output(circuit: &SpiceCircuit, options: &SensitivityOptions) -> Result<f64, String> {
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

fn sensitivity_parameters(circuit: &SpiceCircuit) -> Vec<SensitivityParameter> {
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

fn sensitivity_perturbation(parameter: &SensitivityParameter, relative_step: f64) -> f64 {
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

fn perturbed_for_sensitivity(
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

fn optimization_engineering_value(normalized: f64, specification: &OptimizationParameter) -> f64 {
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
fn refine_quantized_optimum(
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

fn random_factor_matrix(columns: usize, rows: usize, rng: &mut McRng) -> Vec<Vec<f64>> {
    vec![vec![0.0; columns]; rows]
        .into_iter()
        .map(|row| row.into_iter().map(|_| rng.symmetric()).collect())
        .collect()
}

fn global_correlation_matrix(
    parameters: &[SensitivityParameter],
    correlations: &[ParameterCorrelation],
) -> Result<Vec<Vec<f64>>, String> {
    let len = parameters.len();
    let mut matrix = vec![vec![0.0; len]; len];
    for (row, values) in matrix.iter_mut().enumerate() {
        values[row] = 1.0;
    }
    let index = |name: &str| {
        parameters
            .iter()
            .position(|parameter| parameter.name.eq_ignore_ascii_case(name))
    };
    for correlation in correlations {
        let left = index(&correlation.left)
            .ok_or_else(|| format!("unknown Sobol parameter '{}'", correlation.left))?;
        let right = index(&correlation.right)
            .ok_or_else(|| format!("unknown Sobol parameter '{}'", correlation.right))?;
        if left == right {
            return Err("a Sobol parameter cannot be correlated with itself".into());
        }
        if matrix[left][right] != 0.0
            && (matrix[left][right] - correlation.coefficient).abs() > f64::EPSILON
        {
            return Err(format!(
                "conflicting Sobol correlations for '{}' and '{}'",
                correlation.left, correlation.right
            ));
        }
        matrix[left][right] = correlation.coefficient;
        matrix[right][left] = correlation.coefficient;
    }
    cholesky_correlation(&matrix)?;
    Ok(matrix)
}

fn cholesky_correlation(matrix: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, String> {
    let len = matrix.len();
    let mut lower = vec![vec![0.0; len]; len];
    for row in 0..len {
        for column in 0..=row {
            let mut sum = matrix[row][column];
            for (row_value, column_value) in
                lower[row].iter().zip(lower[column].iter()).take(column)
            {
                sum -= row_value * column_value;
            }
            if row == column {
                if sum < -1e-12 {
                    return Err("Sobol correlation matrix is not positive definite".into());
                }
                lower[row][column] = sum.max(0.0).sqrt();
            } else {
                let pivot = lower[column][column];
                if pivot <= 1e-12 {
                    if sum.abs() > 1e-12 {
                        return Err("Sobol correlation matrix is not positive definite".into());
                    }
                    lower[row][column] = 0.0;
                } else {
                    lower[row][column] = sum / pivot;
                }
            }
        }
    }
    Ok(lower)
}

fn cholesky_covariance(matrix: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, String> {
    let len = matrix.len();
    let mut lower = vec![vec![0.0; len]; len];
    for row in 0..len {
        for column in 0..=row {
            let mut sum = matrix[row][column];
            for (row_value, column_value) in
                lower[row].iter().zip(lower[column].iter()).take(column)
            {
                sum -= row_value * column_value;
            }
            if row == column {
                if sum <= 1e-15 {
                    return Err("Monte Carlo covariance matrix is not positive definite".into());
                }
                lower[row][column] = sum.sqrt();
            } else {
                let pivot = lower[column][column];
                if pivot <= 1e-15 {
                    return Err("Monte Carlo covariance matrix is not positive definite".into());
                }
                lower[row][column] = sum / pivot;
            }
        }
    }
    Ok(lower)
}

fn correlated_factor_matrix(
    rows: usize,
    correlation: &[Vec<f64>],
    rng: &mut McRng,
) -> Result<Vec<Vec<f64>>, String> {
    let lower = cholesky_correlation(correlation)?;
    let mut out = Vec::with_capacity(rows);
    for _ in 0..rows {
        out.push(correlated_factor_row(&lower, rng));
    }
    Ok(out)
}

fn correlated_factor_row(lower: &[Vec<f64>], rng: &mut McRng) -> Vec<f64> {
    let columns = lower.len();
    let independent: Vec<f64> = (0..columns).map(|_| standard_normal(rng)).collect();
    (0..columns)
        .map(|column| {
            (0..=column)
                .map(|source| lower[column][source] * independent[source])
                .sum::<f64>()
        })
        .map(|value| 2.0 * normal_cdf(value) - 1.0)
        .collect()
}

fn standard_normal(rng: &mut McRng) -> f64 {
    let u1 = rng.unit().max(f64::MIN_POSITIVE);
    let u2 = rng.unit();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

fn normal_cdf(value: f64) -> f64 {
    0.5 * (1.0 + erf_approx(value / std::f64::consts::SQRT_2))
}

fn normal_inverse_cdf(probability: f64) -> f64 {
    if !(0.0..=1.0).contains(&probability) || !probability.is_finite() {
        return f64::NAN;
    }
    if probability == 0.0 {
        return f64::NEG_INFINITY;
    }
    if probability == 1.0 {
        return f64::INFINITY;
    }
    let mut low = -8.0;
    let mut high = 8.0;
    for _ in 0..64 {
        let middle = (low + high) / 2.0;
        if normal_cdf(middle) < probability {
            low = middle;
        } else {
            high = middle;
        }
    }
    (low + high) / 2.0
}

fn erf_approx(value: f64) -> f64 {
    // Abramowitz-Stegun 7.1.26 provides a compact, dependency-free erf
    // approximation with approximately 1.5e-7 absolute error.
    let absolute = value.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * absolute);
    let polynomial = 0.254_829_592 * t - 0.284_496_736 * t.powi(2) + 1.421_413_741 * t.powi(3)
        - 1.453_152_027 * t.powi(4)
        + 1.061_405_429 * t.powi(5);
    let approximation = 1.0 - polynomial * (-absolute * absolute).exp();
    if value >= 0.0 {
        approximation
    } else {
        -approximation
    }
}

fn global_variant(
    circuit: &SpiceCircuit,
    parameters: &[SensitivityParameter],
    deltas: &[f64],
) -> Result<SpiceCircuit, String> {
    let mut out = circuit.clone();
    for (parameter, delta) in parameters.iter().zip(deltas) {
        if delta.abs() > f64::EPSILON {
            out = perturbed_for_sensitivity(&out, parameter, *delta)?;
        }
    }
    Ok(out)
}

fn sample_variance(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    values
        .iter()
        .map(|value| {
            let delta = value - mean;
            delta * delta
        })
        .sum::<f64>()
        / values.len() as f64
}

struct McRng {
    state: u64,
}

impl McRng {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_mul(6364136223846793005).max(1),
        }
    }

    fn unit(&mut self) -> f64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        let value = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (value >> 11) as f64 / ((1u64 << 53) as f64)
    }

    fn symmetric(&mut self) -> f64 {
        2.0 * self.unit() - 1.0
    }
}

fn randomized_circuit(
    circuit: &SpiceCircuit,
    options: &MonteCarloOptions,
    rng: &mut McRng,
) -> Result<(SpiceCircuit, usize), String> {
    let mut out = circuit.clone();
    let mut count = 0usize;
    let resistor_global = if options.resistor_tolerance > 0.0 {
        rng.symmetric()
    } else {
        0.0
    };
    let capacitor_global = if options.capacitor_tolerance > 0.0 {
        rng.symmetric()
    } else {
        0.0
    };
    let inductor_global = if options.inductor_tolerance > 0.0 {
        rng.symmetric()
    } else {
        0.0
    };
    let diode_is_global = if options.diode_is_decades > 0.0 {
        rng.symmetric()
    } else {
        0.0
    };
    let mos_vto_global = if options.mos_vto_tolerance > 0.0 {
        rng.symmetric()
    } else {
        0.0
    };
    let mos_kp_global = if options.mos_kp_tolerance > 0.0 {
        rng.symmetric()
    } else {
        0.0
    };
    let bjt_beta_global = if options.bjt_beta_tolerance > 0.0 {
        rng.symmetric()
    } else {
        0.0
    };
    let bjt_is_global = if options.bjt_is_decades > 0.0 {
        rng.symmetric()
    } else {
        0.0
    };
    for element in &mut out.elements {
        match &mut element.kind {
            SpiceElementKind::Resistor { resistance, .. } => {
                if options.resistor_tolerance > 0.0 {
                    let sample =
                        correlated_sample(resistor_global, options.resistor_correlation, rng);
                    *resistance *= 1.0
                        + monte_carlo_relative_multiplier(
                            sample,
                            options.resistor_tolerance,
                            options.distribution,
                        );
                    count += 1;
                }
            }
            SpiceElementKind::Capacitor { capacitance, .. } => {
                if options.capacitor_tolerance > 0.0 {
                    let sample =
                        correlated_sample(capacitor_global, options.capacitor_correlation, rng);
                    *capacitance *= 1.0
                        + monte_carlo_relative_multiplier(
                            sample,
                            options.capacitor_tolerance,
                            options.distribution,
                        );
                    count += 1;
                }
            }
            SpiceElementKind::Inductor { inductance, .. } => {
                if options.inductor_tolerance > 0.0 {
                    let sample =
                        correlated_sample(inductor_global, options.inductor_correlation, rng);
                    *inductance *= 1.0
                        + monte_carlo_relative_multiplier(
                            sample,
                            options.inductor_tolerance,
                            options.distribution,
                        );
                    count += 1;
                }
            }
            _ => {}
        }
    }
    for model in out.diode_models.values_mut() {
        if options.diode_is_decades > 0.0 {
            let sample = correlated_sample(diode_is_global, options.diode_is_correlation, rng);
            let exponent = model.saturation_current_a.log10()
                + options.diode_is_decades
                    * monte_carlo_standard_factor(sample, options.distribution);
            model.saturation_current_a = 10f64.powf(exponent);
            count += 1;
        }
    }
    for model in out.mosfet_models.values_mut() {
        if options.mos_vto_tolerance > 0.0 {
            let sample = correlated_sample(mos_vto_global, options.mos_vto_correlation, rng);
            model.threshold_voltage_v += options.mos_vto_tolerance
                * monte_carlo_standard_factor(sample, options.distribution);
            count += 1;
        }
        if options.mos_kp_tolerance > 0.0 {
            let sample = correlated_sample(mos_kp_global, options.mos_kp_correlation, rng);
            model.transconductance_a_per_v2 *= 1.0
                + monte_carlo_relative_multiplier(
                    sample,
                    options.mos_kp_tolerance,
                    options.distribution,
                );
            count += 1;
        }
    }
    for model in out.bjt_models.values_mut() {
        if options.bjt_beta_tolerance > 0.0 {
            let sample = correlated_sample(bjt_beta_global, options.bjt_beta_correlation, rng);
            model.forward_beta *= 1.0
                + monte_carlo_relative_multiplier(
                    sample,
                    options.bjt_beta_tolerance,
                    options.distribution,
                );
            count += 1;
        }
        if options.bjt_is_decades > 0.0 {
            let sample = correlated_sample(bjt_is_global, options.bjt_is_correlation, rng);
            let exponent = model.saturation_current_a.log10()
                + options.bjt_is_decades
                    * monte_carlo_standard_factor(sample, options.distribution);
            model.saturation_current_a = 10f64.powf(exponent);
            count += 1;
        }
    }
    out.validate()?;
    Ok((out, count))
}

fn correlated_sample(global: f64, correlation: f64, rng: &mut McRng) -> f64 {
    let local_weight = (1.0 - correlation * correlation).max(0.0).sqrt();
    correlation * global + local_weight * rng.symmetric()
}

fn monte_carlo_statistics(samples: &[MonteCarloSample]) -> MonteCarloStatistics {
    if samples.is_empty() {
        return MonteCarloStatistics::default();
    }
    let mut minimum = f64::INFINITY;
    let mut maximum = f64::NEG_INFINITY;
    let mut sum = 0.0;
    for sample in samples {
        minimum = minimum.min(sample.value);
        maximum = maximum.max(sample.value);
        sum += sample.value;
    }
    let mean = sum / samples.len() as f64;
    let variance = samples
        .iter()
        .map(|sample| {
            let delta = sample.value - mean;
            delta * delta
        })
        .sum::<f64>()
        / samples.len() as f64;
    MonteCarloStatistics {
        minimum,
        maximum,
        mean,
        standard_deviation: variance.sqrt(),
    }
}

fn solve_time(
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
            let threshold = mos_threshold(
                settings,
                v_bs,
                *length_m,
                *width_m,
                v_ds,
                circuit.temperature_k,
            );
            let overdrive = v_gs - threshold;
            let effective_kp = mos_effective_kp(settings, overdrive, v_bs, circuit.temperature_k);
            let saturation_vds =
                mos_saturation_drain_voltage(settings, overdrive, effective_kp, *length_m);
            let effective_vds = v_ds.min(saturation_vds.max(0.0));
            let current = if overdrive <= 0.0 {
                0.0
            } else {
                effective_kp * width_m / length_m
                    * multiplier
                    * (overdrive * effective_vds - effective_vds * effective_vds / 2.0)
                    * (1.0 + settings.channel_length_modulation_per_v * v_ds)
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
            let polarity = match settings.polarity {
                BjtPolarity::Npn => 1.0,
                BjtPolarity::Pnp => -1.0,
            };
            let v_be = polarity * (base - emitter);
            let v_bc = polarity * (base - collector);
            let thermal_voltage = thermal_voltage_at(circuit.temperature_k, 1.0);
            let saturation_current = temperature_scaled_saturation_current(
                settings.saturation_current_a,
                1.0,
                settings.energy_gap_ev,
                settings.saturation_current_temperature_exponent,
                circuit.temperature_k,
            );
            let forward_early_factor = if settings.forward_early_voltage_v > 0.0 {
                (1.0 - v_bc / settings.forward_early_voltage_v).max(1.0e-3)
            } else {
                1.0
            };
            let reverse_early_factor = if settings.reverse_early_voltage_v > 0.0 {
                (1.0 - v_be / settings.reverse_early_voltage_v).max(1.0e-3)
            } else {
                1.0
            };
            let forward = saturation_current
                * area
                * ((v_be / thermal_voltage).clamp(-80.0, 80.0)).exp_m1()
                * forward_early_factor;
            let reverse = saturation_current
                * area
                * ((v_bc / thermal_voltage).clamp(-80.0, 80.0)).exp_m1()
                * reverse_early_factor;
            let transport = forward - reverse;
            let collector_current = transport - reverse / settings.reverse_beta;
            let base_current = forward / settings.forward_beta + reverse / settings.reverse_beta;
            let emitter_current = forward / settings.forward_beta + transport;
            Ok([
                polarity * collector_current,
                polarity * base_current,
                -polarity * emitter_current,
                0.0,
            ])
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

fn dynamic_capacitors(
    circuit: &SpiceCircuit,
    element: &SpiceElement,
    voltages: &BTreeMap<String, f64>,
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
            let threshold = mos_threshold(
                settings,
                vbs,
                *length_m,
                *width_m,
                vds,
                circuit.temperature_k,
            );
            let overdrive = vgs - threshold;
            let effective_kp = mos_effective_kp(settings, overdrive, vbs, circuit.temperature_k);
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
                circuit.temperature_k,
            ) * area;
            let bjt_thermal_voltage = thermal_voltage_at(circuit.temperature_k, 1.0);
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
) -> Result<(), String> {
    let voltages = terminal_voltages(system, element, guess)?;
    let currents = nonlinear_terminal_currents(circuit, element, &voltages)?;
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
fn build_real_system(
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
                linearized_transistor_stamp(circuit, system, &mut a, &mut b, element, guess)?;
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
        }
    }
    Ok((a, b))
}

fn solve_ac(
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

fn noise_contributions(
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

fn injected_noise_response(
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

fn input_source_gain(
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

struct SystemModel<'a> {
    circuit: &'a SpiceCircuit,
    node_index: BTreeMap<String, usize>,
    branch_index: BTreeMap<String, usize>,
    size: usize,
}

impl<'a> SystemModel<'a> {
    fn new(circuit: &'a SpiceCircuit) -> Result<Self, String> {
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

    fn node_index(&self, name: &str) -> Option<usize> {
        if GROUND_ALIASES.contains(&name) {
            None
        } else {
            self.node_index.get(name).copied()
        }
    }

    fn branch_index(&self, name: &str) -> Option<usize> {
        self.branch_index.get(name).copied()
    }

    fn extract_operating_point(&self, x: &[f64], iterations: usize) -> DcOperatingPoint {
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

fn stamp_conductance(
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

fn stamp_current_source(
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

fn stamp_node_coefficient(
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

fn stamp_complex_conductance(
    a: &mut [Vec<Complex>],
    system: &SystemModel,
    plus: &str,
    minus: &str,
    conductance: Complex,
) {
    stamp_complex_conductance_pair(a, system, [plus, minus, plus, minus], conductance);
}

fn stamp_complex_conductance_pair(
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

fn stamp_complex_current(
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

fn stamp_complex_branch_current(
    a: &mut [Vec<Complex>],
    system: &SystemModel,
    branch: usize,
    plus: &str,
    minus: &str,
) {
    stamp_complex_branch_scaled(a, system, branch, plus, minus, Complex::new(1.0, 0.0));
}

fn stamp_complex_branch_scaled(
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

fn stamp_complex_node(
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

fn node_difference(nodes: &BTreeMap<String, f64>, plus: &str, minus: &str) -> f64 {
    nodes.get(plus).copied().unwrap_or(0.0) - nodes.get(minus).copied().unwrap_or(0.0)
}

fn sweep_point_count(start: f64, stop: f64, step: f64) -> Result<usize, String> {
    let count = ((stop - start) / step).abs().floor() as usize + 1;
    if count == 0 || count > MAX_SWEEP_POINTS {
        return Err(format!("DC sweep produces {} points", count));
    }
    Ok(count)
}

fn frequencies(
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
