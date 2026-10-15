//! SPICE element parsing.

use super::models::model_number;
use super::utility::{expect_arg_count, parse_number, parse_properties, split_tokens};
use crate::types::*;

pub(super) fn parse_element(
    circuit: &mut SpiceCircuit,
    text: &str,
    line_no: usize,
) -> Result<SpiceElement, String> {
    let mut tokens = split_tokens(text);
    let name = tokens.remove(0);
    if name.len() < 2 {
        return Err(format!("line {}: element name is too short", line_no));
    }
    let kind_char = name.as_bytes()[0].to_ascii_uppercase() as char;
    match kind_char {
        'R' | 'C' | 'L' => parse_passive(circuit, &name, kind_char, tokens, line_no),
        'V' | 'I' => parse_source(&name, kind_char, tokens, line_no),
        'E' => {
            expect_arg_count(tokens.len(), 5, line_no, &name)?;
            Ok(SpiceElement {
                name,
                nodes: normalized_nodes(&tokens[..4]),
                kind: SpiceElementKind::VoltageControlledVoltageSource {
                    gain: parse_number(&tokens[4])
                        .map_err(|e| format!("line {}: {}", line_no, e))?,
                },
            })
        }
        'G' => {
            expect_arg_count(tokens.len(), 5, line_no, &name)?;
            Ok(SpiceElement {
                name,
                nodes: normalized_nodes(&tokens[..4]),
                kind: SpiceElementKind::VoltageControlledCurrentSource {
                    transconductance: parse_number(&tokens[4])
                        .map_err(|e| format!("line {}: {}", line_no, e))?,
                },
            })
        }
        'F' => {
            expect_arg_count(tokens.len(), 3, line_no, &name)?;
            Ok(SpiceElement {
                name,
                nodes: normalized_nodes(&tokens[..2]),
                kind: SpiceElementKind::CurrentControlledCurrentSource {
                    controlling_source: tokens[0].clone(),
                    gain: parse_number(&tokens[2])
                        .map_err(|e| format!("line {}: {}", line_no, e))?,
                },
            })
        }
        'H' => {
            expect_arg_count(tokens.len(), 3, line_no, &name)?;
            Ok(SpiceElement {
                name,
                nodes: normalized_nodes(&tokens[..2]),
                kind: SpiceElementKind::CurrentControlledVoltageSource {
                    controlling_source: tokens[0].clone(),
                    transresistance: parse_number(&tokens[2])
                        .map_err(|e| format!("line {}: {}", line_no, e))?,
                },
            })
        }
        'D' => {
            expect_arg_count(tokens.len(), 3, line_no, &name)?;
            Ok(SpiceElement {
                name,
                nodes: normalized_nodes(&tokens[..2]),
                kind: SpiceElementKind::Diode {
                    model: tokens[2].clone(),
                },
            })
        }
        'M' => {
            if tokens.len() < 5 {
                return Err(format!(
                    "line {}: MOSFET '{}' needs D G S B and a model",
                    line_no, name
                ));
            }
            let properties = parse_properties(&tokens[5..]);
            Ok(SpiceElement {
                name,
                nodes: normalized_nodes(&tokens[..4]),
                kind: SpiceElementKind::Mosfet {
                    model: tokens[4].clone(),
                    width_m: model_number(&properties, "w", 1e-6, line_no)?,
                    length_m: model_number(&properties, "l", 180e-9, line_no)?,
                    multiplier: model_number(&properties, "m", 1.0, line_no)?,
                },
            })
        }
        'Q' => {
            if tokens.len() < 4 {
                return Err(format!(
                    "line {}: BJT '{}' needs C B E and a model",
                    line_no, name
                ));
            }
            let fifth_is_property = tokens.get(4).is_some_and(|token| token.contains('='));
            let model_index = if tokens.len() == 4 || (tokens.len() > 4 && fifth_is_property) {
                3
            } else {
                4
            };
            let node_count = model_index;
            let properties = parse_properties(&tokens[model_index + 1..]);
            Ok(SpiceElement {
                name,
                nodes: normalized_nodes(&tokens[..node_count]),
                kind: SpiceElementKind::BipolarJunctionTransistor {
                    model: tokens[model_index].clone(),
                    area: model_number(&properties, "area", 1.0, line_no)?,
                },
            })
        }
        _ => Err(format!(
            "line {}: unsupported element '{}' (supported: R C L V I E G F H D M Q)",
            line_no, name
        )),
    }
}

fn parse_passive(
    circuit: &SpiceCircuit,
    name: &str,
    kind: char,
    tokens: Vec<String>,
    line_no: usize,
) -> Result<SpiceElement, String> {
    if tokens.len() < 3 {
        return Err(format!(
            "line {}: '{}' needs two nodes and a value",
            line_no, name
        ));
    }
    let nodes = normalized_nodes(&tokens[..2]);
    let physical = kind == 'R' && tokens[2].contains('=');
    let property_start = if physical { 2 } else { 3 };
    let value = if physical {
        1.0
    } else {
        parse_number(&tokens[2]).map_err(|e| format!("line {}: {}", line_no, e))?
    };
    let properties = parse_properties(&tokens[property_start..]);
    let kind = match kind {
        'R' => {
            let layer = properties
                .get("layer")
                .or_else(|| properties.get("rmodel"))
                .cloned();
            let sheet_resistance = model_number(&properties, "sheet", 0.0, line_no)?;
            let length_m = model_number(&properties, "l", 0.0, line_no)?;
            let width_m = model_number(&properties, "w", 0.0, line_no)?;
            let multiplier = model_number(&properties, "m", value, line_no)?;
            if layer.is_some() && sheet_resistance > 0.0 {
                return Err(format!(
                    "line {}: resistor '{}' cannot use both LAYER and SHEET",
                    line_no, name
                ));
            }
            let resistance = if sheet_resistance > 0.0 {
                if length_m <= 0.0 || width_m <= 0.0 {
                    return Err(format!(
                        "line {}: physical resistor '{}' needs positive L and W with SHEET",
                        line_no, name
                    ));
                }
                value * sheet_resistance * length_m / width_m
            } else if physical || layer.is_some() {
                let Some(layer_name) = &layer else {
                    return Err(format!(
                        "line {}: physical resistor '{}' needs SHEET or LAYER",
                        line_no, name
                    ));
                };
                let Some(model) = circuit
                    .resistor_layer_models
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(layer_name))
                    .map(|(_, model)| model)
                else {
                    return Err(format!(
                        "line {}: unknown resistor layer model '{layer_name}'",
                        line_no
                    ));
                };
                if length_m <= 0.0 || width_m <= 0.0 {
                    return Err(format!(
                        "line {}: physical resistor '{}' needs positive L and W with LAYER",
                        line_no, name
                    ));
                }
                let (sheet, width_offset) = model
                    .effective_sheet(width_m)
                    .map_err(|e| format!("line {}: {}", line_no, e))?;
                multiplier * sheet * length_m / (width_m + width_offset)
            } else {
                value
            };
            SpiceElementKind::Resistor {
                resistance,
                first_temperature_coefficient_per_k: model_number(
                    &properties,
                    "tc1",
                    0.0,
                    line_no,
                )?,
                second_temperature_coefficient_per_k2: model_number(
                    &properties,
                    "tc2",
                    0.0,
                    line_no,
                )?,
                sheet_resistance_ohm_per_square: sheet_resistance,
                length_m,
                width_m,
                multiplier,
                layer,
            }
        }
        'C' => {
            let initial_voltage = match properties.get("ic") {
                Some(value) => {
                    parse_number(value).map_err(|e| format!("line {}: {}", line_no, e))?
                }
                None => 0.0,
            };
            SpiceElementKind::Capacitor {
                capacitance: value,
                initial_voltage,
            }
        }
        _ => {
            let initial_current = match properties.get("ic") {
                Some(value) => {
                    parse_number(value).map_err(|e| format!("line {}: {}", line_no, e))?
                }
                None => 0.0,
            };
            SpiceElementKind::Inductor {
                inductance: value,
                initial_current,
            }
        }
    };
    Ok(SpiceElement {
        name: name.to_string(),
        nodes,
        kind,
    })
}

fn parse_source(
    name: &str,
    kind: char,
    tokens: Vec<String>,
    line_no: usize,
) -> Result<SpiceElement, String> {
    if tokens.len() < 2 {
        return Err(format!("line {}: source '{}' has no nodes", line_no, name));
    }
    let nodes = normalized_nodes(&tokens[..2]);
    let mut waveform = SourceWaveform::Dc(0.0);
    let mut saw_value = false;
    let mut ac_magnitude = None;
    let mut ac_phase = 0.0;
    let mut i = 2;
    while i < tokens.len() {
        let lower = tokens[i].to_ascii_lowercase();
        if let Some(args) = parenthesized_args(&lower) {
            waveform = parse_waveform(&args, line_no)?;
            saw_value = true;
            i += 1;
            continue;
        }
        match lower.as_str() {
            "dc" => {
                i += 1;
                let value = source_number(&tokens, &mut i, line_no, "DC value")?;
                waveform = SourceWaveform::Dc(value);
                saw_value = true;
            }
            "ac" => {
                i += 1;
                ac_magnitude = Some(source_number(&tokens, &mut i, line_no, "AC magnitude")?);
                if i < tokens.len() {
                    if let Ok(phase) = parse_number(&tokens[i]) {
                        ac_phase = phase;
                        i += 1;
                    }
                }
            }
            "sine" | "pulse" | "pwl" => {
                let mut args = Vec::new();
                i += 1;
                while i < tokens.len() && !is_source_keyword(&tokens[i]) {
                    args.push(tokens[i].clone());
                    i += 1;
                }
                waveform = parse_waveform(&(lower, args), line_no)?;
                saw_value = true;
            }
            _ => {
                let value =
                    parse_number(&tokens[i]).map_err(|e| format!("line {}: {}", line_no, e))?;
                waveform = SourceWaveform::Dc(value);
                saw_value = true;
                i += 1;
            }
        }
    }
    if !saw_value && ac_magnitude.is_none() {
        return Err(format!(
            "line {}: source '{}' has no DC/AC value",
            line_no, name
        ));
    }
    let kind = if kind == 'V' {
        SpiceElementKind::VoltageSource {
            waveform,
            ac_magnitude,
            ac_phase_deg: ac_phase,
        }
    } else {
        SpiceElementKind::CurrentSource {
            waveform,
            ac_magnitude,
            ac_phase_deg: ac_phase,
        }
    };
    Ok(SpiceElement {
        name: name.to_string(),
        nodes,
        kind,
    })
}

fn parse_waveform(spec: &(String, Vec<String>), line_no: usize) -> Result<SourceWaveform, String> {
    let (kind, args) = spec;
    let number = |idx: usize, default: Option<f64>| -> Result<f64, String> {
        if idx < args.len() {
            parse_number(&args[idx]).map_err(|e| format!("line {}: {}", line_no, e))
        } else if let Some(value) = default {
            Ok(value)
        } else {
            Err(format!(
                "line {}: waveform argument {} is missing",
                line_no, idx
            ))
        }
    };
    match kind.as_str() {
        "sin" | "sine" => Ok(SourceWaveform::Sin {
            offset: number(0, Some(0.0))?,
            amplitude: number(1, None)?,
            frequency_hz: number(2, Some(1.0))?,
            delay_s: number(3, Some(0.0))?,
            damping_per_s: number(4, Some(0.0))?,
        }),
        "am" => Ok(SourceWaveform::Am {
            offset: number(0, Some(0.0))?,
            amplitude: number(1, None)?,
            carrier_hz: number(2, None)?,
            modulation_hz: number(3, None)?,
            depth: number(4, Some(1.0))?,
        }),
        "pulse" => Ok(SourceWaveform::Pulse {
            initial: number(0, Some(0.0))?,
            pulsed: number(1, None)?,
            delay_s: number(2, Some(0.0))?,
            rise_s: number(3, Some(0.0))?,
            fall_s: number(4, Some(0.0))?,
            width_s: number(5, Some(f64::INFINITY))?,
            period_s: number(6, Some(f64::INFINITY))?,
        }),
        "pwl" => {
            if args.is_empty() || args.len() % 2 != 0 {
                return Err(format!("line {}: PWL needs time/value pairs", line_no));
            }
            let mut points = Vec::new();
            for pair in args.chunks(2) {
                points.push((
                    parse_number(&pair[0]).map_err(|e| format!("line {}: {}", line_no, e))?,
                    parse_number(&pair[1]).map_err(|e| format!("line {}: {}", line_no, e))?,
                ));
            }
            if points.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
                return Err(format!("line {}: PWL times must increase", line_no));
            }
            Ok(SourceWaveform::Pwl(points))
        }
        _ => Err(format!("line {}: unknown waveform '{}'", line_no, kind)),
    }
}

fn parenthesized_args(token: &str) -> Option<(String, Vec<String>)> {
    if !token.ends_with(')') || !token.contains('(') {
        return None;
    }
    let (kind, rest) = token.split_once('(')?;
    let body = rest.strip_suffix(')')?;
    let args = split_tokens(body);
    Some((kind.to_string(), args))
}

fn source_number(
    tokens: &[String],
    i: &mut usize,
    line_no: usize,
    what: &str,
) -> Result<f64, String> {
    let value = tokens
        .get(*i)
        .ok_or_else(|| format!("line {}: missing {}", line_no, what))?;
    *i += 1;
    parse_number(value).map_err(|e| format!("line {}: {}", line_no, e))
}

fn is_source_keyword(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "dc" | "ac" | "sin" | "sine" | "pulse" | "pwl" | "am"
    ) || token.to_ascii_lowercase().ends_with(')')
}

fn normalized_nodes(tokens: &[String]) -> Vec<String> {
    tokens
        .iter()
        .map(|token| {
            let value = token.trim();
            if GROUND_ALIASES.contains(&value) {
                "0".to_string()
            } else {
                value.to_string()
            }
        })
        .collect()
}
