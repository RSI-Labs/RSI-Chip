//! Device, resistor-layer, statistical, optimization, and Pareto model parsing.

use super::core::{parse_directive, strip_comments};
use super::utility::{parse_number, parse_properties, split_tokens};
use super::writer::format_number;
use crate::types::*;

const OXIDE_PERMITTIVITY_F_PER_M: f64 = 3.9 * 8.854_187_812_8e-12;

pub(super) fn parse_model(
    circuit: &mut SpiceCircuit,
    parts: &[String],
    line_no: usize,
) -> Result<(), String> {
    if parts.len() < 2 {
        return Err(format!("line {}: malformed .model", line_no));
    }
    let model_name = parts[0].to_string();
    let joined = parts[1..].join(" ");
    let Some((model_type, body)) = joined.split_once('(') else {
        return Err(format!("line {}: .model needs TYPE(...)", line_no));
    };
    let body = body
        .trim()
        .strip_suffix(')')
        .ok_or_else(|| format!("line {}: unterminated .model properties", line_no))?;
    let properties = parse_properties(&split_tokens(body));
    match model_type.to_ascii_lowercase().as_str() {
        "d" | "diode" => {
            let model = DiodeModel {
                saturation_current_a: match properties.get("is") {
                    Some(value) => {
                        parse_number(value).map_err(|e| format!("line {}: {}", line_no, e))?
                    }
                    None => 1e-14,
                },
                emission_coefficient: match properties.get("n") {
                    Some(value) => {
                        parse_number(value).map_err(|e| format!("line {}: {}", line_no, e))?
                    }
                    None => 1.0,
                },
                series_resistance_ohm: match properties.get("rs") {
                    Some(value) => {
                        parse_number(value).map_err(|e| format!("line {}: {}", line_no, e))?
                    }
                    None => 0.0,
                },
                zero_bias_junction_cap_f: model_number(&properties, "cjo", 0.0, line_no)?,
                junction_potential_v: model_number(&properties, "vj", 0.7, line_no)?,
                grading_coefficient: model_number(&properties, "m", 0.5, line_no)?,
                forward_knee: model_number(&properties, "fc", 0.5, line_no)?,
                flicker_coefficient_a2: model_number(&properties, "kf", 0.0, line_no)?,
                flicker_current_exponent: model_number(&properties, "af", 1.0, line_no)?,
                flicker_frequency_exponent: model_number(&properties, "ef", 1.0, line_no)?,
                energy_gap_ev: model_number(&properties, "eg", 1.12, line_no)?,
                saturation_current_temperature_exponent: model_number(
                    &properties,
                    "xti",
                    3.0,
                    line_no,
                )?,
                transit_time_s: model_number(&properties, "tt", 0.0, line_no)?,
            };
            model
                .validate()
                .map_err(|e| format!("line {}: {}", line_no, e))?;
            circuit.diode_models.insert(model_name, model);
            Ok(())
        }
        "nmos" | "pmos" => {
            let default = MosfetModel::default();
            let polarity = MosfetPolarity::parse(model_type)
                .ok_or_else(|| format!("line {}: invalid MOS polarity", line_no))?;
            let level = model_number(&properties, "level", 1.0, line_no)?;
            let kind = MosfetModelKind::from_level(level)
                .ok_or_else(|| format!("line {}: unsupported MOS LEVEL={level}", line_no))?;
            let oxide_thickness =
                model_number(&properties, "tox", default.bsim3_oxide_thickness_m, line_no)?;
            let default_oxide_cap = match kind {
                MosfetModelKind::Level1 => default.gate_oxide_cap_per_m2,
                MosfetModelKind::Bsim3LongChannel => OXIDE_PERMITTIVITY_F_PER_M / oxide_thickness,
            };
            let threshold_voltage = match properties.get("vth0") {
                Some(value) => {
                    parse_number(value).map_err(|e| format!("line {}: {}", line_no, e))?
                }
                None => model_number(&properties, "vto", default.threshold_voltage_v, line_no)?,
            };
            let mobility_temperature_exponent = match properties.get("ute") {
                Some(value) => {
                    parse_number(value).map_err(|e| format!("line {}: {}", line_no, e))?
                }
                None => model_number(
                    &properties,
                    "mexp",
                    default.mobility_temperature_exponent,
                    line_no,
                )?,
            };
            let body_effect = match properties.get("k1") {
                Some(value) => {
                    parse_number(value).map_err(|e| format!("line {}: {}", line_no, e))?
                }
                None => model_number(
                    &properties,
                    "gamma",
                    default.body_effect_v_per_sqrt_v,
                    line_no,
                )?,
            };
            let geometry_bin = if ["wmin", "wmax", "lmin", "lmax"]
                .iter()
                .any(|key| properties.contains_key(*key))
            {
                Some(MosfetGeometryBin {
                    min_width_m: model_number(&properties, "wmin", 0.0, line_no)?,
                    max_width_m: model_number(&properties, "wmax", 0.0, line_no)?,
                    min_length_m: model_number(&properties, "lmin", 0.0, line_no)?,
                    max_length_m: model_number(&properties, "lmax", 0.0, line_no)?,
                })
            } else {
                None
            };
            let shared_junction_sidewall_cap = model_number(&properties, "cjsw", 0.0, line_no)?;
            let source_junction_sidewall_cap =
                model_number(&properties, "cjsws", shared_junction_sidewall_cap, line_no)?;
            let drain_junction_sidewall_cap =
                model_number(&properties, "cjswd", shared_junction_sidewall_cap, line_no)?;
            let model = MosfetModel {
                polarity,
                kind,
                transconductance_a_per_v2: model_number(
                    &properties,
                    "kp",
                    default.transconductance_a_per_v2,
                    line_no,
                )?,
                threshold_voltage_v: threshold_voltage,
                channel_length_modulation_per_v: model_number(
                    &properties,
                    "lambda",
                    default.channel_length_modulation_per_v,
                    line_no,
                )?,
                body_effect_v_per_sqrt_v: body_effect,
                surface_potential_v: model_number(
                    &properties,
                    "phi",
                    default.surface_potential_v,
                    line_no,
                )?,
                mobility_degradation_inverse_v: model_number(
                    &properties,
                    "theta",
                    default.mobility_degradation_inverse_v,
                    line_no,
                )?,
                velocity_saturation_voltage_v: match kind {
                    MosfetModelKind::Level1 => model_number(
                        &properties,
                        "vsat",
                        default.velocity_saturation_voltage_v,
                        line_no,
                    )?,
                    MosfetModelKind::Bsim3LongChannel => 0.0,
                },
                gate_source_overlap_cap_per_m: model_number(
                    &properties,
                    "cgso",
                    default.gate_source_overlap_cap_per_m,
                    line_no,
                )?,
                gate_drain_overlap_cap_per_m: model_number(
                    &properties,
                    "cgdo",
                    default.gate_drain_overlap_cap_per_m,
                    line_no,
                )?,
                gate_bulk_cap_per_m2: model_number(
                    &properties,
                    "cgb",
                    default.gate_bulk_cap_per_m2,
                    line_no,
                )?,
                gate_oxide_cap_per_m2: model_number(
                    &properties,
                    "cox",
                    default_oxide_cap,
                    line_no,
                )?,
                source_junction_sidewall_cap_per_m: source_junction_sidewall_cap,
                drain_junction_sidewall_cap_per_m: drain_junction_sidewall_cap,
                junction_potential_v: model_number(
                    &properties,
                    "pb",
                    default.junction_potential_v,
                    line_no,
                )?,
                junction_grading_coefficient: model_number(
                    &properties,
                    "mj",
                    default.junction_grading_coefficient,
                    line_no,
                )?,
                junction_forward_knee: model_number(
                    &properties,
                    "fc",
                    default.junction_forward_knee,
                    line_no,
                )?,
                saturation_charge_partition: model_number(
                    &properties,
                    "xpart",
                    default.saturation_charge_partition,
                    line_no,
                )?,
                flicker_coefficient_a2: model_number(
                    &properties,
                    "kf",
                    default.flicker_coefficient_a2,
                    line_no,
                )?,
                flicker_current_exponent: model_number(
                    &properties,
                    "af",
                    default.flicker_current_exponent,
                    line_no,
                )?,
                flicker_frequency_exponent: model_number(
                    &properties,
                    "ef",
                    default.flicker_frequency_exponent,
                    line_no,
                )?,
                mobility_temperature_exponent,
                self_heating_thermal_resistance_k_per_w: model_number(
                    &properties,
                    "rth",
                    default.self_heating_thermal_resistance_k_per_w,
                    line_no,
                )?,
                self_heating_thermal_capacitance_j_per_k: model_number(
                    &properties,
                    "cth",
                    default.self_heating_thermal_capacitance_j_per_k,
                    line_no,
                )?,
                bsim3_body_bias_threshold_coefficient: model_number(
                    &properties,
                    "k2",
                    default.bsim3_body_bias_threshold_coefficient,
                    line_no,
                )?,
                bsim3_low_field_mobility_m2_per_v_s: model_number(
                    &properties,
                    "u0",
                    default.bsim3_low_field_mobility_m2_per_v_s,
                    line_no,
                )?,
                bsim3_mobility_degradation_first_order_m_per_v: model_number(
                    &properties,
                    "ua",
                    default.bsim3_mobility_degradation_first_order_m_per_v,
                    line_no,
                )?,
                bsim3_mobility_degradation_second_order_m2_per_v2: model_number(
                    &properties,
                    "ub",
                    default.bsim3_mobility_degradation_second_order_m2_per_v2,
                    line_no,
                )?,
                bsim3_mobility_body_bias_coefficient_m2_per_v2: model_number(
                    &properties,
                    "uc",
                    default.bsim3_mobility_body_bias_coefficient_m2_per_v2,
                    line_no,
                )?,
                bsim3_oxide_thickness_m: oxide_thickness,
                bsim3_saturation_velocity_m_per_s: model_number(
                    &properties,
                    "vsat",
                    match kind {
                        MosfetModelKind::Level1 => 0.0,
                        MosfetModelKind::Bsim3LongChannel => {
                            default.bsim3_saturation_velocity_m_per_s
                        }
                    },
                    line_no,
                )?,
                bsim3_short_channel_threshold_amplitude: model_number(
                    &properties,
                    "dvt0",
                    default.bsim3_short_channel_threshold_amplitude,
                    line_no,
                )?,
                bsim3_short_channel_length_scale_m: model_number(
                    &properties,
                    "dvt1",
                    default.bsim3_short_channel_length_scale_m,
                    line_no,
                )?,
                bsim3_short_channel_drain_bias_coefficient_per_v: model_number(
                    &properties,
                    "dvt2",
                    default.bsim3_short_channel_drain_bias_coefficient_per_v,
                    line_no,
                )?,
                bsim3_narrow_width_threshold_amplitude_v: model_number(
                    &properties,
                    "dvt0w",
                    default.bsim3_narrow_width_threshold_amplitude_v,
                    line_no,
                )?,
                bsim3_narrow_width_exponent: model_number(
                    &properties,
                    "dvt1w",
                    default.bsim3_narrow_width_exponent,
                    line_no,
                )?,
                bsim3_reference_width_m: model_number(
                    &properties,
                    "w0",
                    default.bsim3_reference_width_m,
                    line_no,
                )?,
                bsim3_threshold_temperature_coefficient_v: model_number(
                    &properties,
                    "kt1",
                    default.bsim3_threshold_temperature_coefficient_v,
                    line_no,
                )?,
                bsim3_threshold_body_temperature_coefficient_v_per_v: model_number(
                    &properties,
                    "kt2",
                    default.bsim3_threshold_body_temperature_coefficient_v_per_v,
                    line_no,
                )?,
                bsim3_threshold_length_temperature_coefficient_v_m: model_number(
                    &properties,
                    "kt1l",
                    default.bsim3_threshold_length_temperature_coefficient_v_m,
                    line_no,
                )?,
                geometry_bin,
            };
            model
                .validate()
                .map_err(|e| format!("line {}: {}", line_no, e))?;
            circuit.mosfet_models.insert(model_name, model);
            Ok(())
        }
        "npn" | "pnp" => {
            let default = BjtModel::default();
            let polarity = BjtPolarity::parse(model_type)
                .ok_or_else(|| format!("line {}: invalid BJT polarity", line_no))?;
            let model = BjtModel {
                polarity,
                saturation_current_a: model_number(
                    &properties,
                    "is",
                    default.saturation_current_a,
                    line_no,
                )?,
                forward_beta: model_number(&properties, "bf", default.forward_beta, line_no)?,
                reverse_beta: model_number(&properties, "br", default.reverse_beta, line_no)?,
                forward_early_voltage_v: model_number(
                    &properties,
                    "vaf",
                    default.forward_early_voltage_v,
                    line_no,
                )?,
                reverse_early_voltage_v: model_number(
                    &properties,
                    "var",
                    default.reverse_early_voltage_v,
                    line_no,
                )?,
                base_emitter_zero_bias_cap_f: model_number(
                    &properties,
                    "cje",
                    default.base_emitter_zero_bias_cap_f,
                    line_no,
                )?,
                base_collector_zero_bias_cap_f: model_number(
                    &properties,
                    "cjc",
                    default.base_collector_zero_bias_cap_f,
                    line_no,
                )?,
                base_emitter_junction_potential_v: model_number(
                    &properties,
                    "vje",
                    default.base_emitter_junction_potential_v,
                    line_no,
                )?,
                base_collector_junction_potential_v: model_number(
                    &properties,
                    "vjc",
                    default.base_collector_junction_potential_v,
                    line_no,
                )?,
                base_emitter_grading_coefficient: model_number(
                    &properties,
                    "mje",
                    default.base_emitter_grading_coefficient,
                    line_no,
                )?,
                base_collector_grading_coefficient: model_number(
                    &properties,
                    "mjc",
                    default.base_collector_grading_coefficient,
                    line_no,
                )?,
                junction_forward_knee: model_number(
                    &properties,
                    "fc",
                    default.junction_forward_knee,
                    line_no,
                )?,
                flicker_coefficient_a2: model_number(
                    &properties,
                    "kf",
                    default.flicker_coefficient_a2,
                    line_no,
                )?,
                flicker_current_exponent: model_number(
                    &properties,
                    "af",
                    default.flicker_current_exponent,
                    line_no,
                )?,
                flicker_frequency_exponent: model_number(
                    &properties,
                    "ef",
                    default.flicker_frequency_exponent,
                    line_no,
                )?,
                energy_gap_ev: model_number(&properties, "eg", default.energy_gap_ev, line_no)?,
                saturation_current_temperature_exponent: model_number(
                    &properties,
                    "xti",
                    default.saturation_current_temperature_exponent,
                    line_no,
                )?,
                self_heating_thermal_resistance_k_per_w: model_number(
                    &properties,
                    "rth",
                    default.self_heating_thermal_resistance_k_per_w,
                    line_no,
                )?,
                self_heating_thermal_capacitance_j_per_k: model_number(
                    &properties,
                    "cth",
                    default.self_heating_thermal_capacitance_j_per_k,
                    line_no,
                )?,
                forward_transit_time_s: model_number(
                    &properties,
                    "tf",
                    default.forward_transit_time_s,
                    line_no,
                )?,
                reverse_transit_time_s: model_number(
                    &properties,
                    "tr",
                    default.reverse_transit_time_s,
                    line_no,
                )?,
            };
            model
                .validate()
                .map_err(|e| format!("line {}: {}", line_no, e))?;
            circuit.bjt_models.insert(model_name, model);
            Ok(())
        }
        _ => Err(format!(
            "line {}: unsupported model type '{}' (supported: D NMOS PMOS NPN PNP)",
            line_no, model_type
        )),
    }
}

pub(super) fn model_number(
    properties: &std::collections::BTreeMap<String, String>,
    key: &str,
    default: f64,
    line_no: usize,
) -> Result<f64, String> {
    match properties.get(key) {
        Some(value) => parse_number(value).map_err(|e| format!("line {}: {}", line_no, e)),
        None => Ok(default),
    }
}

pub(super) fn parse_resistor_width_bins(
    source: &str,
    line_no: usize,
) -> Result<Vec<ResistorWidthBin>, String> {
    let mut out = Vec::new();
    for item in source
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let fields: Vec<_> = item.split(':').map(str::trim).collect();
        if !(2..=3).contains(&fields.len()) {
            return Err(format!(
                "line {}: resistor width bins need width:sheet[:woff]",
                line_no
            ));
        }
        out.push(ResistorWidthBin {
            width_m: parse_number(fields[0]).map_err(|e| format!("line {}: {}", line_no, e))?,
            sheet_resistance_ohm_per_square: parse_number(fields[1])
                .map_err(|e| format!("line {}: {}", line_no, e))?,
            width_offset_m: if fields.len() == 3 {
                parse_number(fields[2]).map_err(|e| format!("line {}: {}", line_no, e))?
            } else {
                0.0
            },
        });
    }
    Ok(out)
}

pub(super) fn format_resistor_width_bins(bins: &[ResistorWidthBin]) -> String {
    bins.iter()
        .map(|bin| {
            format!(
                "{}:{}:{}",
                format_number(bin.width_m),
                format_number(bin.sheet_resistance_ohm_per_square),
                format_number(bin.width_offset_m)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn format_resistor_layer_model(name: &str, model: &ResistorLayerModel) -> String {
    format!(
        ".rmodel {} SHEET={} TC1={} TC2={} WOFF={} MINW={} MAXW={} BINS={}\n",
        name,
        format_number(model.sheet_resistance_ohm_per_square),
        format_number(model.first_temperature_coefficient_per_k),
        format_number(model.second_temperature_coefficient_per_k2),
        format_number(model.width_offset_m),
        format_number(model.minimum_width_m),
        format_number(model.maximum_width_m),
        format_resistor_width_bins(&model.width_bins)
    )
}

/// Parse a generic foundry-style resistor layer table containing `.rmodel`
/// directives. The file is independent of a particular netlist and can be
/// attached to many SPICE circuits through the typed API or CLI.
pub fn parse_resistor_layer_models(
    source: &str,
) -> Result<std::collections::BTreeMap<String, ResistorLayerModel>, String> {
    let mut circuit = SpiceCircuit::default();
    for (index, raw) in source.lines().enumerate() {
        let line = strip_comments(raw).trim().to_string();
        if line.is_empty() {
            continue;
        }
        let line_no = index + 1;
        let name = split_tokens(&line)
            .first()
            .map(String::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches!(name.as_str(), ".rmodel" | ".resistor-model") {
            return Err(format!(
                "line {line_no}: resistor layer table supports only .rmodel directives"
            ));
        }
        parse_directive(&mut circuit, &line, line_no)?;
    }
    if circuit.resistor_layer_models.is_empty() {
        return Err("resistor layer table has no .rmodel directives".into());
    }
    Ok(circuit.resistor_layer_models)
}

pub fn write_resistor_layer_models(
    models: &std::collections::BTreeMap<String, ResistorLayerModel>,
) -> Result<String, String> {
    if models.is_empty() {
        return Err("resistor layer table has no .rmodel directives".into());
    }
    for model in models.values() {
        model.validate()?;
    }
    Ok(models
        .iter()
        .map(|(name, model)| format_resistor_layer_model(name, model))
        .collect())
}

pub(super) fn parse_parameter_correlations(
    source: &str,
    line_no: usize,
) -> Result<Vec<ParameterCorrelation>, String> {
    let mut out = Vec::new();
    for item in source
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let fields: Vec<_> = item.split('|').map(str::trim).collect();
        if fields.len() != 3 {
            return Err(format!(
                "line {}: corr needs parameter|parameter|coefficient entries",
                line_no
            ));
        }
        out.push(ParameterCorrelation {
            left: fields[0].to_string(),
            right: fields[1].to_string(),
            coefficient: parse_number(fields[2]).map_err(|e| format!("line {}: {}", line_no, e))?,
        });
    }
    Ok(out)
}

pub(super) fn parse_parameter_covariances(
    source: &str,
    line_no: usize,
) -> Result<Vec<ParameterCovariance>, String> {
    let mut out = Vec::new();
    for item in split_outside_parentheses(source) {
        let Some((parameters, value)) = item.split_once(':') else {
            return Err(format!(
                "line {}: covariance needs parameter:value or left|right:value, got '{item}'",
                line_no,
            ));
        };
        let covariance = parse_number(value).map_err(|e| format!("line {}: {}", line_no, e))?;
        if let Some((left, right)) = parameters.split_once('|') {
            out.push(ParameterCovariance {
                left: left.trim().to_string(),
                right: right.trim().to_string(),
                covariance,
            });
        } else {
            let parameter = parameters.trim().to_string();
            out.push(ParameterCovariance {
                left: parameter.clone(),
                right: parameter,
                covariance,
            });
        }
    }
    Ok(out)
}

pub(super) fn parse_statistical_parameters(
    source: &str,
    line_no: usize,
) -> Result<Vec<StatisticalParameterModel>, String> {
    let mut out = Vec::new();
    for item in source
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let fields: Vec<_> = item.split(':').map(str::trim).collect();
        if fields.len() != 4 {
            return Err(format!(
                "line {}: statistical parameters need name:distribution:relative:sigma",
                line_no
            ));
        }
        let distribution = MonteCarloDistribution::parse(fields[1]).ok_or_else(|| {
            format!(
                "line {}: unknown statistical model distribution '{}'",
                line_no, fields[1]
            )
        })?;
        let relative_sigma = matches!(
            fields[2].to_ascii_lowercase().as_str(),
            "rel" | "relative" | "true"
        );
        if !relative_sigma
            && !matches!(
                fields[2].to_ascii_lowercase().as_str(),
                "abs" | "absolute" | "false"
            )
        {
            return Err(format!(
                "line {}: statistical sigma mode must be relative or absolute",
                line_no
            ));
        }
        out.push(StatisticalParameterModel {
            parameter: fields[0].to_string(),
            distribution,
            relative_sigma,
            sigma: parse_number(fields[3]).map_err(|e| format!("line {}: {}", line_no, e))?,
        });
    }
    Ok(out)
}

pub(super) fn format_statistical_parameters(parameters: &[StatisticalParameterModel]) -> String {
    parameters
        .iter()
        .map(|parameter| {
            format!(
                "{}:{}:{}:{}",
                parameter.parameter,
                parameter.distribution.as_str(),
                if parameter.relative_sigma {
                    "relative"
                } else {
                    "absolute"
                },
                format_number(parameter.sigma)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Parse a dependency-free vendor statistical model file.
///
/// Format:
/// ```text
/// model name
/// param R1.R dist=normal relative=true sigma=0.05
/// corr R1.R R2.R -0.5
/// ```
pub fn parse_statistical_model(source: &str) -> Result<StatisticalModel, String> {
    let mut name: Option<String> = None;
    let mut parameters = Vec::new();
    let mut correlations = Vec::new();
    for (index, raw) in source.lines().enumerate() {
        let line = strip_comments(raw).trim().to_string();
        if line.is_empty() {
            continue;
        }
        let line_no = index + 1;
        let tokens: Vec<_> = line.split_whitespace().collect();
        match tokens[0].to_ascii_lowercase().as_str() {
            "model" => {
                if tokens.len() < 2 {
                    return Err(format!("line {line_no}: model needs one name"));
                }
                name = Some(tokens[1].to_string());
            }
            "param" | "parameter" => {
                if tokens.len() < 2 {
                    return Err(format!("line {line_no}: param needs name and properties"));
                }
                let properties: Vec<String> = tokens[2..]
                    .iter()
                    .map(|token: &&str| token.to_string())
                    .collect();
                let properties = parse_properties(&properties);
                let distribution = match properties
                    .get("dist")
                    .or_else(|| properties.get("distribution"))
                {
                    Some(value) => MonteCarloDistribution::parse(value)
                        .ok_or_else(|| format!("line {line_no}: unknown distribution"))?,
                    None => MonteCarloDistribution::Normal,
                };
                let relative_sigma = match properties.get("relative") {
                    Some(value) => value == "1" || value.eq_ignore_ascii_case("true"),
                    None => true,
                };
                let sigma = match properties.get("sigma") {
                    Some(value) => {
                        parse_number(value).map_err(|e| format!("line {line_no}: {}", e))?
                    }
                    None => return Err(format!("line {line_no}: param needs sigma")),
                };
                parameters.push(StatisticalParameterModel {
                    parameter: tokens[1].to_string(),
                    distribution,
                    relative_sigma,
                    sigma,
                });
            }
            "corr" | "correlation" => {
                if tokens.len() != 4 {
                    return Err(format!("line {line_no}: corr needs left right coefficient"));
                }
                correlations.push(ParameterCorrelation {
                    left: tokens[1].to_string(),
                    right: tokens[2].to_string(),
                    coefficient: parse_number(tokens[3])
                        .map_err(|e| format!("line {line_no}: {}", e))?,
                });
            }
            _ => {
                return Err(format!(
                    "line {line_no}: unknown statistical model directive"
                ))
            }
        }
    }
    let name = name.ok_or_else(|| "statistical model needs a model line".to_string())?;
    let model = StatisticalModel {
        name,
        parameters,
        correlations,
    };
    model.validate()?;
    Ok(model)
}

pub fn write_statistical_model(model: &StatisticalModel) -> Result<String, String> {
    model.validate()?;
    let mut out = format!("model {}\n", model.name);
    for parameter in &model.parameters {
        out.push_str(&format!(
            "param {} dist={} relative={} sigma={}\n",
            parameter.parameter,
            parameter.distribution.as_str(),
            parameter.relative_sigma,
            format_number(parameter.sigma)
        ));
    }
    for correlation in &model.correlations {
        out.push_str(&format!(
            "corr {} {} {}\n",
            correlation.left,
            correlation.right,
            format_number(correlation.coefficient)
        ));
    }
    Ok(out)
}

pub(super) fn parse_optimization_parameters(
    source: &str,
    line_no: usize,
) -> Result<Vec<OptimizationParameter>, String> {
    let mut out = Vec::new();
    for item in source
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let fields: Vec<_> = item.split(':').map(str::trim).collect();
        if !(3..=5).contains(&fields.len()) {
            return Err(format!(
                "line {}: optimization params need name:lower:upper[:initial[:quantum]]",
                line_no
            ));
        }
        out.push(OptimizationParameter {
            parameter: fields[0].to_string(),
            lower: parse_number(fields[1]).map_err(|e| format!("line {}: {}", line_no, e))?,
            upper: parse_number(fields[2]).map_err(|e| format!("line {}: {}", line_no, e))?,
            initial: if fields.len() >= 4 && !fields[3].is_empty() {
                Some(parse_number(fields[3]).map_err(|e| format!("line {}: {}", line_no, e))?)
            } else {
                None
            },
            quantum: if fields.len() == 5 {
                Some(parse_number(fields[4]).map_err(|e| format!("line {}: {}", line_no, e))?)
            } else {
                None
            },
        });
    }
    Ok(out)
}

pub(super) fn parse_pareto_constraints(
    source: &str,
    line_no: usize,
) -> Result<Vec<ParetoConstraint>, String> {
    let mut out = Vec::new();
    for item in split_outside_parentheses(source) {
        let (expression_token, sense, bound_token) =
            if let Some((left, right)) = item.split_once("==") {
                (left, ConstraintSense::Equal, right)
            } else if let Some((left, right)) = item.split_once("<=") {
                (left, ConstraintSense::LessEqual, right)
            } else if let Some((node, bound)) = item.split_once(">=") {
                (node, ConstraintSense::GreaterEqual, bound)
            } else {
                return Err(format!(
                    "line {}: Pareto constraints need expression<=bound, >=bound, or ==bound",
                    line_no
                ));
            };
        let expression = parse_constraint_expression(expression_token, line_no)?;
        let (node, expression) = match expression {
            ConstraintExpression::Node(node) => (node, None),
            expression => (String::new(), Some(expression)),
        };
        out.push(ParetoConstraint {
            node,
            sense,
            bound: parse_number(bound_token).map_err(|e| format!("line {}: {}", line_no, e))?,
            expression,
        });
    }
    Ok(out)
}

fn split_outside_parentheses(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for character in source.chars() {
        match character {
            '(' => {
                depth += 1;
                current.push(character);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(character);
            }
            ',' if depth == 0 => {
                let item = current.trim().to_string();
                if !item.is_empty() {
                    out.push(item);
                }
                current.clear();
            }
            _ => current.push(character),
        }
    }
    let current = current.trim().to_string();
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
enum ConstraintToken {
    Number(f64),
    Node(String),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Comma,
    Function(ConstraintFunction),
    LeftParen,
    RightParen,
}

fn tokenize_constraint_expression(
    source: &str,
    line_no: usize,
) -> Result<Vec<ConstraintToken>, String> {
    let characters: Vec<_> = source.chars().collect();
    let mut out = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if character.is_whitespace() {
            index += 1;
        } else if character == '(' {
            out.push(ConstraintToken::LeftParen);
            index += 1;
        } else if character == ')' {
            out.push(ConstraintToken::RightParen);
            index += 1;
        } else if character == '+' {
            out.push(ConstraintToken::Plus);
            index += 1;
        } else if character == '-' {
            out.push(ConstraintToken::Minus);
            index += 1;
        } else if character == '*' {
            if index + 1 < characters.len() && characters[index + 1] == '*' {
                out.push(ConstraintToken::Caret);
                index += 2;
            } else {
                out.push(ConstraintToken::Star);
                index += 1;
            }
        } else if character == '/' {
            out.push(ConstraintToken::Slash);
            index += 1;
        } else if character == '^' {
            out.push(ConstraintToken::Caret);
            index += 1;
        } else if character == ',' {
            out.push(ConstraintToken::Comma);
            index += 1;
        } else if character == 'v' || character == 'V' {
            if index + 1 >= characters.len() || characters[index + 1] != '(' {
                return Err(format!(
                    "line {}: constraint expression node needs V(name)",
                    line_no
                ));
            }
            let end = (index + 2..characters.len())
                .find(|position| characters[*position] == ')')
                .ok_or_else(|| {
                    format!(
                        "line {}: constraint expression node is missing ')'",
                        line_no
                    )
                })?;
            let node = characters[index + 2..end].iter().collect::<String>();
            if node.trim().is_empty() {
                return Err(format!(
                    "line {}: constraint expression node cannot be empty",
                    line_no
                ));
            }
            out.push(ConstraintToken::Node(node.trim().to_string()));
            index = end + 1;
        } else if character.is_ascii_alphabetic() || character == '_' {
            let start = index;
            while index < characters.len()
                && (characters[index].is_ascii_alphanumeric() || characters[index] == '_')
            {
                index += 1;
            }
            let name = characters[start..index].iter().collect::<String>();
            let function = ConstraintFunction::parse(&name).ok_or_else(|| {
                format!(
                    "line {}: unknown constraint expression function '{name}'",
                    line_no
                )
            })?;
            out.push(ConstraintToken::Function(function));
        } else if character.is_ascii_digit() || character == '.' {
            let start = index;
            index += 1;
            while index < characters.len()
                && (characters[index].is_ascii_digit()
                    || characters[index] == '.'
                    || ((characters[index] == 'e' || characters[index] == 'E')
                        && matches!(characters.get(index + 1), Some('+') | Some('-') | Some(_))))
            {
                if matches!(characters[index], 'e' | 'E') {
                    index += 1;
                    if matches!(characters.get(index), Some('+') | Some('-')) {
                        index += 1;
                    }
                } else {
                    index += 1;
                }
            }
            let text = characters[start..index].iter().collect::<String>();
            out.push(ConstraintToken::Number(
                parse_number(&text).map_err(|e| format!("line {}: {}", line_no, e))?,
            ));
        } else {
            return Err(format!(
                "line {}: unsupported character '{}' in constraint expression",
                line_no, character
            ));
        }
    }
    Ok(out)
}

#[derive(Debug)]
struct ConstraintExpressionParser {
    tokens: Vec<ConstraintToken>,
    position: usize,
    line_no: usize,
}

impl ConstraintExpressionParser {
    fn peek(&self) -> Option<&ConstraintToken> {
        self.tokens.get(self.position)
    }

    fn next(&mut self) -> Option<ConstraintToken> {
        let token = self.tokens.get(self.position).cloned();
        if token.is_some() {
            self.position += 1;
        }
        token
    }

    fn expression(&mut self) -> Result<ConstraintExpression, String> {
        let mut left = self.term()?;
        while matches!(
            self.peek(),
            Some(ConstraintToken::Plus | ConstraintToken::Minus)
        ) {
            let operator = self.next().unwrap();
            let right = self.term()?;
            left = match operator {
                ConstraintToken::Plus => ConstraintExpression::Add(left.into(), right.into()),
                ConstraintToken::Minus => ConstraintExpression::Subtract(left.into(), right.into()),
                _ => unreachable!(),
            };
        }
        Ok(left)
    }

    fn term(&mut self) -> Result<ConstraintExpression, String> {
        let mut left = self.power()?;
        while matches!(
            self.peek(),
            Some(ConstraintToken::Star | ConstraintToken::Slash)
        ) {
            let operator = self.next().unwrap();
            let right = self.power()?;
            left = match operator {
                ConstraintToken::Star => ConstraintExpression::Multiply(left.into(), right.into()),
                ConstraintToken::Slash => ConstraintExpression::Divide(left.into(), right.into()),
                _ => unreachable!(),
            };
        }
        Ok(left)
    }

    fn power(&mut self) -> Result<ConstraintExpression, String> {
        let base = self.factor()?;
        if matches!(self.peek(), Some(ConstraintToken::Caret)) {
            self.next();
            let exponent = self.power()?;
            Ok(ConstraintExpression::Power(base.into(), exponent.into()))
        } else {
            Ok(base)
        }
    }

    fn factor(&mut self) -> Result<ConstraintExpression, String> {
        match self.next() {
            Some(ConstraintToken::Number(value)) => Ok(ConstraintExpression::Constant(value)),
            Some(ConstraintToken::Node(node)) => Ok(ConstraintExpression::Node(node)),
            Some(ConstraintToken::Function(function)) => self.call(function),
            Some(ConstraintToken::LeftParen) => {
                let expression = self.expression()?;
                if self.next() != Some(ConstraintToken::RightParen) {
                    return Err(format!(
                        "line {}: constraint expression is missing ')'",
                        self.line_no
                    ));
                }
                Ok(expression)
            }
            Some(ConstraintToken::Minus) => Ok(ConstraintExpression::Subtract(
                ConstraintExpression::Constant(0.0).into(),
                self.factor()?.into(),
            )),
            Some(ConstraintToken::Plus) => self.factor(),
            _ => Err(format!(
                "line {}: incomplete constraint expression",
                self.line_no
            )),
        }
    }

    fn call(&mut self, function: ConstraintFunction) -> Result<ConstraintExpression, String> {
        if self.next() != Some(ConstraintToken::LeftParen) {
            return Err(format!(
                "line {}: constraint function '{}' needs '('",
                self.line_no,
                function.name()
            ));
        }
        let mut arguments = vec![self.expression()?];
        while matches!(self.peek(), Some(ConstraintToken::Comma)) {
            self.next();
            arguments.push(self.expression()?);
        }
        if self.next() != Some(ConstraintToken::RightParen) {
            return Err(format!(
                "line {}: constraint function is missing ')'",
                self.line_no
            ));
        }
        if arguments.len() != function.arity() {
            return Err(format!(
                "line {}: constraint function '{}' needs {} argument(s)",
                self.line_no,
                function.name(),
                function.arity()
            ));
        }
        Ok(ConstraintExpression::Function(function, arguments))
    }
}

fn parse_constraint_expression(
    source: &str,
    line_no: usize,
) -> Result<ConstraintExpression, String> {
    let tokens = tokenize_constraint_expression(source, line_no)?;
    if tokens.is_empty() {
        return Err(format!(
            "line {}: Pareto constraint expression cannot be empty",
            line_no
        ));
    }
    let mut parser = ConstraintExpressionParser {
        tokens,
        position: 0,
        line_no,
    };
    let expression = parser.expression()?;
    if parser.position != parser.tokens.len() {
        return Err(format!(
            "line {}: trailing tokens in constraint expression",
            line_no
        ));
    }
    Ok(expression)
}

pub(super) fn format_pareto_constraints(constraints: &[ParetoConstraint]) -> String {
    constraints
        .iter()
        .map(|constraint| {
            format!(
                "{}{}{}",
                constraint
                    .expression
                    .as_ref()
                    .map(|expression| expression.format())
                    .unwrap_or_else(|| format!("V({})", constraint.node)),
                constraint.sense.as_str(),
                format_number(constraint.bound)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn format_parameter_correlations(correlations: &[ParameterCorrelation]) -> String {
    correlations
        .iter()
        .map(|correlation| {
            format!(
                "{}|{}|{}",
                correlation.left,
                correlation.right,
                format_number(correlation.coefficient)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn format_parameter_covariances(covariances: &[ParameterCovariance]) -> String {
    covariances
        .iter()
        .map(|covariance| {
            if covariance.left.eq_ignore_ascii_case(&covariance.right) {
                format!(
                    "{}:{}",
                    covariance.left,
                    format_number(covariance.covariance)
                )
            } else {
                format!(
                    "{}|{}:{}",
                    covariance.left,
                    covariance.right,
                    format_number(covariance.covariance)
                )
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn format_optimization_parameters(parameters: &[OptimizationParameter]) -> String {
    parameters
        .iter()
        .map(|parameter| {
            let base = format!(
                "{}:{}:{}",
                parameter.parameter,
                format_number(parameter.lower),
                format_number(parameter.upper)
            );
            if let Some(quantum) = parameter.quantum {
                let initial = parameter.initial.map(format_number).unwrap_or_default();
                format!("{}:{}:{}", base, initial, format_number(quantum))
            } else if let Some(initial) = parameter.initial {
                format!("{}:{}", base, format_number(initial))
            } else {
                base
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}
