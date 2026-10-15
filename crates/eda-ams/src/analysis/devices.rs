//! Compact-device temperature, electrothermal, and effective-value helpers.

use crate::types::*;

pub(super) const MAX_SWEEP_POINTS: usize = 1_000_000;
pub(super) const MAX_TRANSIENT_STEPS: usize = 1_000_000;
pub(super) const MAX_NEWTON_ITERATIONS: usize = 100;
pub(super) const GMIN: f64 = 1e-12;
pub(super) const BOLTZMANN_CONSTANT_J_PER_K: f64 = 1.380_649e-23;
pub(super) const NOMINAL_TEMPERATURE_K: f64 = SPICE_NOMINAL_TEMPERATURE_K;
pub(super) const ELECTRON_CHARGE_C: f64 = 1.602_176_634e-19;
pub(super) const OXIDE_PERMITTIVITY_F_PER_M: f64 = 3.9 * 8.854_187_812_8e-12;

pub(super) fn thermal_voltage_at(temperature_k: f64, emission_coefficient: f64) -> f64 {
    BOLTZMANN_CONSTANT_J_PER_K * temperature_k / ELECTRON_CHARGE_C * emission_coefficient
}

pub(super) fn temperature_scaled_saturation_current(
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

pub(super) fn temperature_scaled_mos_kp(
    nominal_kp: f64,
    mobility_temperature_exponent: f64,
    temperature_k: f64,
) -> f64 {
    nominal_kp * (NOMINAL_TEMPERATURE_K / temperature_k).powf(mobility_temperature_exponent)
}

pub(super) fn mos_effective_kp(
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

pub(super) fn mos_threshold(
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

pub(super) fn mos_saturation_drain_voltage(
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

#[allow(clippy::too_many_arguments)]
pub(super) fn mos_drain_current_at_temperature(
    settings: &MosfetModel,
    width_m: f64,
    length_m: f64,
    multiplier: f64,
    v_gs: f64,
    v_ds: f64,
    v_bs: f64,
    temperature_k: f64,
) -> f64 {
    let threshold = mos_threshold(settings, v_bs, length_m, width_m, v_ds, temperature_k);
    let overdrive = v_gs - threshold;
    let effective_kp = mos_effective_kp(settings, overdrive, v_bs, temperature_k);
    let saturation_vds = mos_saturation_drain_voltage(settings, overdrive, effective_kp, length_m);
    let effective_vds = v_ds.min(saturation_vds.max(0.0));
    if overdrive <= 0.0 {
        0.0
    } else {
        effective_kp * width_m / length_m
            * multiplier
            * (overdrive * effective_vds - effective_vds * effective_vds / 2.0)
            * (1.0 + settings.channel_length_modulation_per_v * v_ds)
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn self_heated_mos_drain_current(
    settings: &MosfetModel,
    width_m: f64,
    length_m: f64,
    multiplier: f64,
    v_gs: f64,
    v_ds: f64,
    v_bs: f64,
    ambient_temperature_k: f64,
) -> f64 {
    self_heated_mos_state(
        settings,
        width_m,
        length_m,
        multiplier,
        v_gs,
        v_ds,
        v_bs,
        ambient_temperature_k,
    )
    .0
}

#[allow(clippy::too_many_arguments)]
pub(super) fn self_heated_mos_state(
    settings: &MosfetModel,
    width_m: f64,
    length_m: f64,
    multiplier: f64,
    v_gs: f64,
    v_ds: f64,
    v_bs: f64,
    ambient_temperature_k: f64,
) -> (f64, f64) {
    let mut local_temperature = ambient_temperature_k;
    let mut current = mos_drain_current_at_temperature(
        settings,
        width_m,
        length_m,
        multiplier,
        v_gs,
        v_ds,
        v_bs,
        local_temperature,
    );
    if settings.self_heating_thermal_resistance_k_per_w == 0.0 || v_ds == 0.0 {
        return (current, local_temperature);
    }
    for _ in 0..24 {
        let power = current.abs() * v_ds.abs();
        let target = (ambient_temperature_k
            + settings.self_heating_thermal_resistance_k_per_w * power)
            .min(ambient_temperature_k + 300.0);
        let next_temperature = local_temperature + 0.5 * (target - local_temperature);
        if !next_temperature.is_finite() || next_temperature <= 0.0 {
            break;
        }
        let next_current = mos_drain_current_at_temperature(
            settings,
            width_m,
            length_m,
            multiplier,
            v_gs,
            v_ds,
            v_bs,
            next_temperature,
        );
        let temperature_converged = (next_temperature - local_temperature).abs() < 1e-7;
        local_temperature = next_temperature;
        let previous_current = current;
        current = next_current;
        if temperature_converged
            && (current - previous_current).abs() <= 1e-12 * (1.0 + current.abs())
        {
            break;
        }
    }
    (current, local_temperature)
}

pub(super) fn bjt_terminal_currents_at_temperature(
    settings: &BjtModel,
    area: f64,
    collector_voltage: f64,
    base_voltage: f64,
    emitter_voltage: f64,
    temperature_k: f64,
) -> [f64; 4] {
    let polarity = match settings.polarity {
        BjtPolarity::Npn => 1.0,
        BjtPolarity::Pnp => -1.0,
    };
    let v_be = polarity * (base_voltage - emitter_voltage);
    let v_bc = polarity * (base_voltage - collector_voltage);
    let thermal_voltage = thermal_voltage_at(temperature_k, 1.0);
    let saturation_current = temperature_scaled_saturation_current(
        settings.saturation_current_a,
        1.0,
        settings.energy_gap_ev,
        settings.saturation_current_temperature_exponent,
        temperature_k,
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
    [
        polarity * collector_current,
        polarity * base_current,
        -polarity * emitter_current,
        0.0,
    ]
}

pub(super) fn self_heated_bjt_terminal_currents(
    settings: &BjtModel,
    area: f64,
    collector_voltage: f64,
    base_voltage: f64,
    emitter_voltage: f64,
    ambient_temperature_k: f64,
) -> [f64; 4] {
    self_heated_bjt_state(
        settings,
        area,
        collector_voltage,
        base_voltage,
        emitter_voltage,
        ambient_temperature_k,
    )
    .0
}

pub(super) fn self_heated_bjt_state(
    settings: &BjtModel,
    area: f64,
    collector_voltage: f64,
    base_voltage: f64,
    emitter_voltage: f64,
    ambient_temperature_k: f64,
) -> ([f64; 4], f64) {
    let mut local_temperature = ambient_temperature_k;
    let mut currents = bjt_terminal_currents_at_temperature(
        settings,
        area,
        collector_voltage,
        base_voltage,
        emitter_voltage,
        local_temperature,
    );
    if settings.self_heating_thermal_resistance_k_per_w == 0.0 {
        return (currents, local_temperature);
    }
    for _ in 0..24 {
        let power = (collector_voltage - emitter_voltage).abs() * currents[0].abs()
            + (base_voltage - emitter_voltage).abs() * currents[1].abs();
        let target = (ambient_temperature_k
            + settings.self_heating_thermal_resistance_k_per_w * power)
            .min(ambient_temperature_k + 300.0);
        let next_temperature = local_temperature + 0.25 * (target - local_temperature);
        if !next_temperature.is_finite() || next_temperature <= 0.0 {
            break;
        }
        let next_currents = bjt_terminal_currents_at_temperature(
            settings,
            area,
            collector_voltage,
            base_voltage,
            emitter_voltage,
            next_temperature,
        );
        let temperature_converged = (next_temperature - local_temperature).abs() < 1e-7;
        local_temperature = next_temperature;
        let old_currents = currents;
        currents = next_currents;
        if temperature_converged
            && old_currents
                .iter()
                .zip(currents.iter())
                .all(|(old, new)| (old - new).abs() <= 1e-12 * (1.0 + old.abs().max(new.abs())))
        {
            break;
        }
    }
    (currents, local_temperature)
}

pub(super) fn effective_resistance(
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

pub(super) fn element_effective_resistance(
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
