//! Shared sensitivity parameter discovery and perturbation.

use super::super::devices::*;
use super::super::{ac_sweep, dc_operating_point, transient};
use crate::types::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analysis) enum SensitivityParameterKind {
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
pub(in crate::analysis) struct SensitivityParameter {
    pub(in crate::analysis) name: String,
    pub(in crate::analysis) object: String,
    pub(in crate::analysis) value: f64,
    pub(in crate::analysis) kind: SensitivityParameterKind,
}

pub(in crate::analysis) fn sensitivity_output(
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

pub(in crate::analysis) fn sensitivity_parameters(
    circuit: &SpiceCircuit,
) -> Vec<SensitivityParameter> {
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

pub(in crate::analysis) fn sensitivity_perturbation(
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

pub(in crate::analysis) fn perturbed_for_sensitivity(
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
