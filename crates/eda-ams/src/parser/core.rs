//! Top-level SPICE directive parsing.

//! A practical SPICE netlist subset parser/writer.

use super::elements::parse_element;
use super::models::{
    model_number, parse_model, parse_optimization_parameters, parse_parameter_correlations,
    parse_parameter_covariances, parse_pareto_constraints, parse_resistor_width_bins,
    parse_statistical_parameters,
};
use super::utility::{parse_number, parse_properties, split_tokens};
use crate::types::*;

#[derive(Clone, Debug)]
struct Line {
    number: usize,
    text: String,
}

pub fn parse_spice(source: &str) -> Result<SpiceCircuit, String> {
    let logical = join_continuations(source)?;
    let mut circuit = SpiceCircuit::default();
    let mut title_seen = false;
    let mut ended = false;

    for line in logical {
        if ended {
            return Err(format!("line {}: text after .end", line.number));
        }
        let text = strip_comments(&line.text);
        if text.trim().is_empty() {
            continue;
        }
        if text.starts_with('.') {
            parse_directive(&mut circuit, &text, line.number)?;
            if text.to_ascii_lowercase().starts_with(".end") {
                ended = true;
            }
            continue;
        }
        if !title_seen {
            circuit.title = text.trim().to_string();
            title_seen = true;
            continue;
        }
        let element = parse_element(&mut circuit, &text, line.number)?;
        circuit.add_element(element)?;
    }

    if !title_seen && !circuit.elements.is_empty() {
        circuit.title = "untitled".into();
    }
    if circuit.analyses.is_empty() {
        circuit.analyses.push(Analysis::OperatingPoint);
    }
    circuit.validate()?;
    Ok(circuit)
}

fn join_continuations(source: &str) -> Result<Vec<Line>, String> {
    let mut out = Vec::new();
    let mut current: Option<Line> = None;
    for (idx, raw) in source.lines().enumerate() {
        let trimmed_start = raw.trim_start();
        if trimmed_start.starts_with('+') && current.is_some() {
            let mut line = current.take().unwrap();
            line.text.push(' ');
            line.text.push_str(trimmed_start[1..].trim());
            current = Some(line);
            continue;
        }
        if let Some(line) = current.take() {
            out.push(line);
        }
        current = Some(Line {
            number: idx + 1,
            text: raw.to_string(),
        });
    }
    if let Some(line) = current {
        out.push(line);
    }
    Ok(out)
}

pub(super) fn strip_comments(line: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    for ch in line.chars() {
        if let Some(q) = quote {
            out.push(ch);
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            out.push(ch);
        } else if ch == '$' || ch == ';' {
            break;
        } else {
            out.push(ch);
        }
    }
    out
}

pub(super) fn parse_directive(
    circuit: &mut SpiceCircuit,
    text: &str,
    line_no: usize,
) -> Result<(), String> {
    let mut parts = split_tokens(text);
    let name = parts.remove(0).to_ascii_lowercase();
    match name.as_str() {
        ".end" | ".ends" => Ok(()),
        ".op" => {
            circuit.analyses.push(Analysis::OperatingPoint);
            Ok(())
        }
        ".temp" | ".temperature" => {
            if parts.len() != 1 {
                return Err(format!("line {}: .temp needs one Celsius value", line_no));
            }
            let celsius =
                parse_number(&parts[0]).map_err(|e| format!("line {}: {}", line_no, e))?;
            circuit
                .set_temperature_celsius(celsius)
                .map_err(|e| format!("line {}: {}", line_no, e))
        }
        ".location" | ".position" => {
            if parts.len() != 3 {
                return Err(format!("line {}: .location needs element x y", line_no));
            }
            let x = parse_number(&parts[1]).map_err(|e| format!("line {}: {}", line_no, e))?;
            let y = parse_number(&parts[2]).map_err(|e| format!("line {}: {}", line_no, e))?;
            if circuit
                .locations
                .insert(parts[0].clone(), ElementLocation { x_m: x, y_m: y })
                .is_some()
            {
                return Err(format!(
                    "line {}: duplicate location for '{}'",
                    line_no, parts[0]
                ));
            }
            Ok(())
        }
        ".rmodel" | ".resistor-model" => {
            if parts.len() < 2 {
                return Err(format!(
                    "line {}: .rmodel needs layer and properties",
                    line_no
                ));
            }
            let properties = parse_properties(&parts[1..]);
            let model = ResistorLayerModel {
                sheet_resistance_ohm_per_square: model_number(&properties, "sheet", 0.0, line_no)?,
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
                width_offset_m: model_number(&properties, "woff", 0.0, line_no)?,
                minimum_width_m: model_number(&properties, "minw", 0.0, line_no)?,
                maximum_width_m: model_number(&properties, "maxw", 0.0, line_no)?,
                width_bins: match properties.get("bins") {
                    Some(value) => parse_resistor_width_bins(value, line_no)?,
                    None => Vec::new(),
                },
            };
            model
                .validate()
                .map_err(|e| format!("line {}: {}", line_no, e))?;
            if circuit
                .resistor_layer_models
                .insert(parts[0].clone(), model)
                .is_some()
            {
                return Err(format!("line {}: duplicate resistor layer model", line_no));
            }
            Ok(())
        }
        ".stats" | ".statistics" => {
            let properties = parse_properties(&parts);
            let name = properties
                .get("name")
                .cloned()
                .ok_or_else(|| format!("line {}: .stats needs name", line_no))?;
            let parameters = match properties.get("params").or_else(|| properties.get("param")) {
                Some(value) => parse_statistical_parameters(value, line_no)?,
                None => return Err(format!("line {}: .stats needs params", line_no)),
            };
            let correlations = match properties.get("corr") {
                Some(value) => parse_parameter_correlations(value, line_no)?,
                None => Vec::new(),
            };
            let model = StatisticalModel {
                name,
                parameters,
                correlations,
            };
            model
                .validate()
                .map_err(|e| format!("line {}: {}", line_no, e))?;
            if circuit
                .statistical_models
                .insert(model.name.clone(), model)
                .is_some()
            {
                return Err(format!("line {}: duplicate statistical model", line_no));
            }
            Ok(())
        }
        ".option" | ".options" | ".global" | ".include" | ".inc" | ".lib" | ".param"
        | ".parameters" | ".probe" | ".save" => Ok(()),
        ".dc" => {
            if parts.len() != 4 {
                return Err(format!(
                    "line {}: .dc needs source start stop step",
                    line_no
                ));
            }
            let analysis = Analysis::DcSweep {
                source: parts[0].clone(),
                start: parse_number(&parts[1]).map_err(|e| format!("line {}: {}", line_no, e))?,
                stop: parse_number(&parts[2]).map_err(|e| format!("line {}: {}", line_no, e))?,
                step: parse_number(&parts[3]).map_err(|e| format!("line {}: {}", line_no, e))?,
            };
            circuit.analyses.push(analysis);
            Ok(())
        }
        ".tran" => {
            if parts.len() < 2 {
                return Err(format!("line {}: .tran needs step and stop", line_no));
            }
            let mut method = IntegrationMethod::Trapezoidal;
            for token in &parts[2..] {
                let lower = token.to_ascii_lowercase();
                if let Some((key, value)) = lower.split_once('=') {
                    if key == "method" {
                        method = IntegrationMethod::parse(value).ok_or_else(|| {
                            format!("line {}: unknown integration method '{}'", line_no, value)
                        })?;
                    }
                }
            }
            let analysis = Analysis::Transient {
                step_s: parse_number(&parts[0]).map_err(|e| format!("line {}: {}", line_no, e))?,
                stop_s: parse_number(&parts[1]).map_err(|e| format!("line {}: {}", line_no, e))?,
                method,
            };
            circuit.analyses.push(analysis);
            Ok(())
        }
        ".ac" => {
            if parts.len() != 4 {
                return Err(format!(
                    "line {}: .ac needs lin|dec|oct points start stop",
                    line_no
                ));
            }
            let kind = AcSweepKind::parse(&parts[0])
                .ok_or_else(|| format!("line {}: unknown AC sweep kind", line_no))?;
            let analysis = Analysis::AcSweep {
                kind,
                points: parts[1]
                    .parse()
                    .map_err(|_| format!("line {}: invalid AC point count", line_no))?,
                start_hz: parse_number(&parts[2])
                    .map_err(|e| format!("line {}: {}", line_no, e))?,
                stop_hz: parse_number(&parts[3]).map_err(|e| format!("line {}: {}", line_no, e))?,
            };
            circuit.analyses.push(analysis);
            Ok(())
        }
        ".model" => parse_model(circuit, &parts, line_no),
        ".noise" => {
            if parts.len() != 6 {
                return Err(format!(
                    "line {}: .noise needs V(output) source lin|dec|oct points start stop",
                    line_no
                ));
            }
            let output = parts[0]
                .trim()
                .trim_start_matches(['v', 'V'])
                .trim_matches(|ch| ch == '(' || ch == ')')
                .to_string();
            if output.is_empty() {
                return Err(format!("line {}: .noise output is empty", line_no));
            }
            let kind = AcSweepKind::parse(&parts[2])
                .ok_or_else(|| format!("line {}: unknown noise sweep kind", line_no))?;
            let analysis = Analysis::NoiseSweep {
                output_node: output,
                input_source: parts[1].clone(),
                kind,
                points: parts[3]
                    .parse()
                    .map_err(|_| format!("line {}: invalid noise point count", line_no))?,
                start_hz: parse_number(&parts[4])
                    .map_err(|e| format!("line {}: {}", line_no, e))?,
                stop_hz: parse_number(&parts[5]).map_err(|e| format!("line {}: {}", line_no, e))?,
            };
            circuit.analyses.push(analysis);
            Ok(())
        }
        ".mc" | ".montecarlo" => {
            if parts.len() < 3 {
                return Err(format!(
                    "line {}: .mc needs runs op|tran|ac V(output) [properties]",
                    line_no
                ));
            }
            let default = MonteCarloOptions::default();
            let target = MonteCarloTarget::parse(&parts[1])
                .ok_or_else(|| format!("line {}: unknown Monte Carlo target", line_no))?;
            let output = parts[2]
                .trim()
                .trim_start_matches(['v', 'V'])
                .trim_matches(['(', ')'])
                .to_string();
            let properties = parse_properties(&parts[3..]);
            let all_correlation = model_number(&properties, "corr", 0.0, line_no)?;
            let options = MonteCarloOptions {
                runs: parts[0]
                    .parse()
                    .map_err(|_| format!("line {}: invalid Monte Carlo run count", line_no))?,
                seed: match properties.get("seed") {
                    Some(value) => value
                        .parse()
                        .map_err(|_| format!("line {}: invalid Monte Carlo seed", line_no))?,
                    None => default.seed,
                },
                target,
                output_node: output,
                frequency_hz: model_number(&properties, "freq", default.frequency_hz, line_no)?,
                resistor_tolerance: model_number(
                    &properties,
                    "rtol",
                    default.resistor_tolerance,
                    line_no,
                )?,
                capacitor_tolerance: model_number(
                    &properties,
                    "ctol",
                    default.capacitor_tolerance,
                    line_no,
                )?,
                inductor_tolerance: model_number(
                    &properties,
                    "ltol",
                    default.inductor_tolerance,
                    line_no,
                )?,
                diode_is_decades: model_number(
                    &properties,
                    "disdec",
                    default.diode_is_decades,
                    line_no,
                )?,
                mos_vto_tolerance: model_number(
                    &properties,
                    "vtotol",
                    default.mos_vto_tolerance,
                    line_no,
                )?,
                mos_kp_tolerance: model_number(
                    &properties,
                    "kptol",
                    default.mos_kp_tolerance,
                    line_no,
                )?,
                bjt_beta_tolerance: model_number(
                    &properties,
                    "bftol",
                    default.bjt_beta_tolerance,
                    line_no,
                )?,
                bjt_is_decades: model_number(
                    &properties,
                    "bisdec",
                    default.bjt_is_decades,
                    line_no,
                )?,
                resistor_correlation: model_number(&properties, "rcorr", all_correlation, line_no)?,
                capacitor_correlation: model_number(
                    &properties,
                    "ccorr",
                    all_correlation,
                    line_no,
                )?,
                inductor_correlation: model_number(&properties, "lcorr", all_correlation, line_no)?,
                diode_is_correlation: model_number(
                    &properties,
                    "dscorr",
                    all_correlation,
                    line_no,
                )?,
                mos_vto_correlation: model_number(
                    &properties,
                    "vtocorr",
                    all_correlation,
                    line_no,
                )?,
                mos_kp_correlation: model_number(&properties, "kpcorr", all_correlation, line_no)?,
                bjt_beta_correlation: model_number(
                    &properties,
                    "bfcorr",
                    all_correlation,
                    line_no,
                )?,
                bjt_is_correlation: model_number(&properties, "biscorr", all_correlation, line_no)?,
                spatial_correlation: model_number(
                    &properties,
                    "spatialcorr",
                    default.spatial_correlation,
                    line_no,
                )?,
                spatial_correlation_length_m: model_number(
                    &properties,
                    "spatialscale",
                    default.spatial_correlation_length_m,
                    line_no,
                )?,
                distribution: match properties
                    .get("dist")
                    .or_else(|| properties.get("marginal"))
                {
                    Some(value) => MonteCarloDistribution::parse(value).ok_or_else(|| {
                        format!("line {}: unknown Monte Carlo distribution", line_no)
                    })?,
                    None => default.distribution,
                },
                statistical_model: properties
                    .get("stats")
                    .or_else(|| properties.get("statistical-model"))
                    .filter(|value| !value.trim().is_empty())
                    .cloned(),
                correlations: match properties.get("matrix").or_else(|| properties.get("mcorr")) {
                    Some(value) => parse_parameter_correlations(value, line_no)?,
                    None => Vec::new(),
                },
                covariances: match properties.get("cov").or_else(|| properties.get("covar")) {
                    Some(value) => parse_parameter_covariances(value, line_no)?,
                    None => Vec::new(),
                },
            };
            circuit.analyses.push(Analysis::MonteCarlo(options));
            Ok(())
        }
        ".sens" | ".sensitivity" => {
            if parts.is_empty() {
                return Err(format!("line {}: .sens needs V(output)", line_no));
            }
            let default = SensitivityOptions::default();
            let (target, output_token, property_start) =
                if let Some(target) = MonteCarloTarget::parse(&parts[0]) {
                    if parts.len() < 2 {
                        return Err(format!("line {}: .sens target needs V(output)", line_no));
                    }
                    (target, parts[1].clone(), 2)
                } else {
                    (MonteCarloTarget::OperatingPoint, parts[0].clone(), 1)
                };
            let output = output_token
                .trim()
                .trim_start_matches(['v', 'V'])
                .trim_matches(['(', ')'])
                .to_string();
            if output.is_empty() {
                return Err(format!("line {}: .sens output is empty", line_no));
            }
            let properties = parse_properties(&parts[property_start..]);
            let options = SensitivityOptions {
                target,
                output_node: output,
                frequency_hz: model_number(&properties, "freq", default.frequency_hz, line_no)?,
                relative_step: model_number(&properties, "step", default.relative_step, line_no)?,
            };
            circuit.analyses.push(Analysis::Sensitivity(options));
            Ok(())
        }
        ".deriv" | ".derivatives" | ".grad" => {
            if parts.is_empty() {
                return Err(format!(
                    "line {}: .deriv needs V(output) params=...",
                    line_no
                ));
            }
            let default = DerivativeOptions::default();
            let (target, output_token, property_start) =
                if let Some(target) = DerivativeTarget::parse(&parts[0]) {
                    if parts.len() < 2 {
                        return Err(format!("line {}: .deriv target needs V(output)", line_no));
                    }
                    (target, parts[1].clone(), 2)
                } else {
                    (DerivativeTarget::Dc, parts[0].clone(), 1)
                };
            let output = output_token
                .trim()
                .trim_start_matches(['v', 'V'])
                .trim_matches(['(', ')'])
                .to_string();
            if output.is_empty() {
                return Err(format!("line {}: derivative output is empty", line_no));
            }
            let properties = parse_properties(&parts[property_start..]);
            let parameters = properties
                .get("params")
                .or_else(|| properties.get("param"))
                .map(|value| {
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|item| !item.is_empty())
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .ok_or_else(|| format!("line {}: .deriv needs params", line_no))?;
            let options = DerivativeOptions {
                target,
                output_node: output,
                frequency_hz: model_number(&properties, "freq", default.frequency_hz, line_no)?,
                parameters,
            };
            options
                .validate()
                .map_err(|e| format!("line {}: {}", line_no, e))?;
            circuit.analyses.push(Analysis::Derivative(options));
            Ok(())
        }
        ".sobol" | ".globalsens" => {
            if parts.is_empty() {
                return Err(format!("line {}: .sobol needs V(output)", line_no));
            }
            let default = GlobalSensitivityOptions::default();
            let (target, output_token, property_start) =
                if let Some(target) = MonteCarloTarget::parse(&parts[0]) {
                    if parts.len() < 2 {
                        return Err(format!("line {}: .sobol target needs V(output)", line_no));
                    }
                    (target, parts[1].clone(), 2)
                } else {
                    (MonteCarloTarget::OperatingPoint, parts[0].clone(), 1)
                };
            let output = output_token
                .trim()
                .trim_start_matches(['v', 'V'])
                .trim_matches(['(', ')'])
                .to_string();
            if output.is_empty() {
                return Err(format!("line {}: .sobol output is empty", line_no));
            }
            let properties = parse_properties(&parts[property_start..]);
            let options = GlobalSensitivityOptions {
                target,
                output_node: output,
                frequency_hz: model_number(&properties, "freq", default.frequency_hz, line_no)?,
                samples: match properties.get("samples") {
                    Some(value) => value
                        .parse()
                        .map_err(|_| format!("line {}: invalid Sobol sample count", line_no))?,
                    None => default.samples,
                },
                seed: match properties.get("seed") {
                    Some(value) => value
                        .parse()
                        .map_err(|_| format!("line {}: invalid Sobol seed", line_no))?,
                    None => default.seed,
                },
                tolerance: model_number(&properties, "tol", default.tolerance, line_no)?,
                correlations: match properties.get("corr") {
                    Some(value) => parse_parameter_correlations(value, line_no)?,
                    None => Vec::new(),
                },
            };
            circuit.analyses.push(Analysis::GlobalSensitivity(options));
            Ok(())
        }
        ".optim" | ".optimize" => {
            if parts.len() < 3 {
                return Err(format!(
                    "line {}: .optim needs objective V(output) params=...",
                    line_no
                ));
            }
            let default = OptimizationOptions::default();
            let objective = OptimizationObjective::parse(&parts[0])
                .ok_or_else(|| format!("line {}: unknown optimization objective", line_no))?;
            let (target, output_token, property_start) =
                if let Some(target) = MonteCarloTarget::parse(&parts[1]) {
                    if parts.len() < 3 {
                        return Err(format!(
                            "line {}: optimization target needs V(output)",
                            line_no
                        ));
                    }
                    (target, parts[2].clone(), 3)
                } else {
                    (MonteCarloTarget::OperatingPoint, parts[1].clone(), 2)
                };
            let output = output_token
                .trim()
                .trim_start_matches(['v', 'V'])
                .trim_matches(['(', ')'])
                .to_string();
            if output.is_empty() {
                return Err(format!("line {}: optimization output is empty", line_no));
            }
            let properties = parse_properties(&parts[property_start..]);
            let parameters_source = properties
                .get("params")
                .or_else(|| properties.get("param"))
                .ok_or_else(|| format!("line {}: .optim needs params", line_no))?;
            let parameters = parse_optimization_parameters(parameters_source, line_no)?;
            let mut parameters = parameters;
            if let Some(quantum_source) =
                properties.get("quantum").or_else(|| properties.get("step"))
            {
                let quantum =
                    parse_number(quantum_source).map_err(|e| format!("line {}: {}", line_no, e))?;
                for parameter in &mut parameters {
                    if parameter.quantum.is_none() {
                        parameter.quantum = Some(quantum);
                    }
                }
            }
            let options = OptimizationOptions {
                objective,
                target,
                output_node: output,
                goal_value: model_number(&properties, "goal", default.goal_value, line_no)?,
                frequency_hz: model_number(&properties, "freq", default.frequency_hz, line_no)?,
                parameters,
                iterations: match properties.get("iterations") {
                    Some(value) => value.parse().map_err(|_| {
                        format!("line {}: invalid optimization iteration count", line_no)
                    })?,
                    None => default.iterations,
                },
                population: match properties.get("population") {
                    Some(value) => value.parse().map_err(|_| {
                        format!("line {}: invalid optimization population", line_no)
                    })?,
                    None => default.population,
                },
                seed: match properties.get("seed") {
                    Some(value) => value
                        .parse()
                        .map_err(|_| format!("line {}: invalid optimization seed", line_no))?,
                    None => default.seed,
                },
                tolerance: model_number(&properties, "tol", default.tolerance, line_no)?,
                differential_weight: model_number(
                    &properties,
                    "f",
                    default.differential_weight,
                    line_no,
                )?,
                crossover_probability: model_number(
                    &properties,
                    "cr",
                    default.crossover_probability,
                    line_no,
                )?,
            };
            circuit.analyses.push(Analysis::Optimization(options));
            Ok(())
        }
        ".pareto" | ".pareto_opt" => {
            if parts.len() < 3 {
                return Err(format!(
                    "line {}: .pareto needs [target] V(output) ... params=...",
                    line_no
                ));
            }
            let default = ParetoOptions::default();
            let output_start = if MonteCarloTarget::parse(&parts[0]).is_some() {
                1
            } else {
                0
            };
            let property_start = (output_start..parts.len())
                .find(|index| parts[*index].contains('='))
                .ok_or_else(|| format!("line {}: .pareto needs params", line_no))?;
            if property_start.saturating_sub(output_start) < 2 {
                return Err(format!(
                    "line {}: .pareto needs at least two output nodes",
                    line_no
                ));
            }
            let target = if output_start == 0 {
                MonteCarloTarget::OperatingPoint
            } else {
                MonteCarloTarget::parse(&parts[0]).ok_or_else(|| {
                    format!("line {}: unknown Pareto optimization target", line_no)
                })?
            };
            let output_nodes: Vec<_> = parts[output_start..property_start]
                .iter()
                .map(|token| {
                    token
                        .trim()
                        .trim_start_matches(['v', 'V'])
                        .trim_matches(['(', ')'])
                        .to_string()
                })
                .collect();
            if output_nodes.iter().any(String::is_empty) {
                return Err(format!("line {}: .pareto output cannot be empty", line_no));
            }
            let properties = parse_properties(&parts[property_start..]);
            let parameters_source = properties
                .get("params")
                .or_else(|| properties.get("param"))
                .ok_or_else(|| format!("line {}: .pareto needs params", line_no))?;
            let mut parameters = parse_optimization_parameters(parameters_source, line_no)?;
            if let Some(quantum_source) =
                properties.get("quantum").or_else(|| properties.get("step"))
            {
                if !quantum_source.trim().is_empty() {
                    let quantum = parse_number(quantum_source)
                        .map_err(|e| format!("line {}: {}", line_no, e))?;
                    for parameter in &mut parameters {
                        if parameter.quantum.is_none() {
                            parameter.quantum = Some(quantum);
                        }
                    }
                }
            }
            let senses = match properties.get("sense").or_else(|| properties.get("senses")) {
                Some(value) => value
                    .split(',')
                    .map(ParetoSense::parse)
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| format!("line {}: invalid Pareto objective sense", line_no))?,
                None => vec![ParetoSense::Minimize; output_nodes.len()],
            };
            let options = ParetoOptions {
                target,
                output_nodes,
                senses,
                frequency_hz: model_number(&properties, "freq", default.frequency_hz, line_no)?,
                parameters,
                population: match properties.get("population") {
                    Some(value) => value
                        .parse()
                        .map_err(|_| format!("line {}: invalid Pareto population", line_no))?,
                    None => default.population,
                },
                iterations: match properties.get("iterations") {
                    Some(value) => value
                        .parse()
                        .map_err(|_| format!("line {}: invalid Pareto iteration count", line_no))?,
                    None => default.iterations,
                },
                seed: match properties.get("seed") {
                    Some(value) => value
                        .parse()
                        .map_err(|_| format!("line {}: invalid Pareto seed", line_no))?,
                    None => default.seed,
                },
                differential_weight: model_number(
                    &properties,
                    "f",
                    default.differential_weight,
                    line_no,
                )?,
                crossover_probability: model_number(
                    &properties,
                    "cr",
                    default.crossover_probability,
                    line_no,
                )?,
                maximum_points: match properties.get("points") {
                    Some(value) => value
                        .parse()
                        .map_err(|_| format!("line {}: invalid Pareto point limit", line_no))?,
                    None => default.maximum_points,
                },
                front_method: match properties.get("front").or_else(|| properties.get("method")) {
                    Some(value) => ParetoFrontMethod::parse(value)
                        .ok_or_else(|| format!("line {}: invalid Pareto front method", line_no))?,
                    None => default.front_method,
                },
                reference_divisions: match properties.get("divisions") {
                    Some(value) => value.parse().map_err(|_| {
                        format!("line {}: invalid Pareto reference division count", line_no)
                    })?,
                    None => default.reference_divisions,
                },
                adaptive_references: match properties.get("adaptive") {
                    Some(value) => value == "1" || value.eq_ignore_ascii_case("true"),
                    None => default.adaptive_references,
                },
                constraints: match properties
                    .get("constraints")
                    .or_else(|| properties.get("constraint"))
                {
                    Some(value) => parse_pareto_constraints(value, line_no)?,
                    None => Vec::new(),
                },
            };
            circuit.analyses.push(Analysis::Pareto(options));
            Ok(())
        }
        ".pss" => {
            if parts.is_empty() {
                return Err(format!("line {}: .pss needs V(output)", line_no));
            }
            let default = PssOptions::default();
            let output = parts[0]
                .trim()
                .trim_start_matches(['v', 'V'])
                .trim_matches(['(', ')'])
                .to_string();
            if output.is_empty() {
                return Err(format!("line {}: .pss output is empty", line_no));
            }
            let properties = parse_properties(&parts[1..]);
            let automatic_fundamental = parts
                .iter()
                .skip(1)
                .any(|token| token.eq_ignore_ascii_case("auto"))
                || properties
                    .get("auto")
                    .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
            let threshold = match properties.get("threshold") {
                Some(value) => {
                    Some(parse_number(value).map_err(|e| format!("line {}: {}", line_no, e))?)
                }
                None => None,
            };
            let options = PssOptions {
                output_node: output,
                fundamental_hz: model_number(&properties, "fund", default.fundamental_hz, line_no)?,
                automatic_fundamental,
                threshold,
                minimum_cycles: match properties.get("min") {
                    Some(value) => value
                        .parse()
                        .map_err(|_| format!("line {}: invalid PSS minimum cycles", line_no))?,
                    None => default.minimum_cycles,
                },
                maximum_cycles: match properties.get("max") {
                    Some(value) => value
                        .parse()
                        .map_err(|_| format!("line {}: invalid PSS maximum cycles", line_no))?,
                    None => default.maximum_cycles,
                },
                samples_per_cycle: match properties.get("samples") {
                    Some(value) => value
                        .parse()
                        .map_err(|_| format!("line {}: invalid PSS sample count", line_no))?,
                    None => default.samples_per_cycle,
                },
                harmonics: match properties.get("harmonics") {
                    Some(value) => value
                        .parse()
                        .map_err(|_| format!("line {}: invalid PSS harmonic count", line_no))?,
                    None => default.harmonics,
                },
                tolerance: model_number(&properties, "tol", default.tolerance, line_no)?,
            };
            circuit
                .analyses
                .push(Analysis::PeriodicSteadyState(options));
            Ok(())
        }
        ".env" | ".envelope" => {
            if parts.is_empty() {
                return Err(format!("line {}: .env needs V(output)", line_no));
            }
            let default = EnvelopeOptions::default();
            let output = parts[0]
                .trim()
                .trim_start_matches(['v', 'V'])
                .trim_matches(['(', ')'])
                .to_string();
            if output.is_empty() {
                return Err(format!("line {}: .env output is empty", line_no));
            }
            let properties = parse_properties(&parts[1..]);
            let options = EnvelopeOptions {
                output_node: output,
                carrier_hz: model_number(&properties, "carrier", default.carrier_hz, line_no)?,
                carrier_cycles: model_number(
                    &properties,
                    "cycles",
                    default.carrier_cycles,
                    line_no,
                )?,
                points: match properties.get("points") {
                    Some(value) => value
                        .parse()
                        .map_err(|_| format!("line {}: invalid envelope point count", line_no))?,
                    None => default.points,
                },
            };
            circuit.analyses.push(Analysis::EnvelopeFollow(options));
            Ok(())
        }
        ".aging" | ".age" => {
            if parts.len() < 2 {
                return Err(format!(
                    "line {}: .aging needs [target] V(output) [properties]",
                    line_no
                ));
            }
            let default = AgingOptions::default();
            let (target, output_token, property_start) =
                if let Some(target) = MonteCarloTarget::parse(&parts[0]) {
                    if parts.len() < 2 {
                        return Err(format!("line {}: aging target needs V(output)", line_no));
                    }
                    (target, parts[1].clone(), 2)
                } else {
                    (MonteCarloTarget::OperatingPoint, parts[0].clone(), 1)
                };
            let output = output_token
                .trim()
                .trim_start_matches(['v', 'V'])
                .trim_matches(['(', ')'])
                .to_string();
            if output.is_empty() {
                return Err(format!("line {}: aging output is empty", line_no));
            }
            let properties = parse_properties(&parts[property_start..]);
            let options = AgingOptions {
                target,
                output_node: output,
                frequency_hz: model_number(&properties, "freq", default.frequency_hz, line_no)?,
                age_years: model_number(&properties, "years", default.age_years, line_no)?,
                time_exponent: model_number(&properties, "exp", default.time_exponent, line_no)?,
                resistance_increase_per_year: model_number(
                    &properties,
                    "rinc",
                    default.resistance_increase_per_year,
                    line_no,
                )?,
                mos_threshold_shift_v_per_year: model_number(
                    &properties,
                    "vtshift",
                    default.mos_threshold_shift_v_per_year,
                    line_no,
                )?,
                mos_kp_degradation_per_year: model_number(
                    &properties,
                    "kploss",
                    default.mos_kp_degradation_per_year,
                    line_no,
                )?,
            };
            circuit.analyses.push(Analysis::Aging(options));
            Ok(())
        }
        _ => Err(format!(
            "line {}: unsupported directive '{}'",
            line_no, name
        )),
    }
}
