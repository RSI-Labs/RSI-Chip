//! A practical SPICE netlist subset parser/writer.

use crate::types::*;
use std::collections::BTreeMap;

const OXIDE_PERMITTIVITY_F_PER_M: f64 = 3.9 * 8.854_187_812_8e-12;

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

fn strip_comments(line: &str) -> String {
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

fn parse_directive(circuit: &mut SpiceCircuit, text: &str, line_no: usize) -> Result<(), String> {
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

fn parse_model(circuit: &mut SpiceCircuit, parts: &[String], line_no: usize) -> Result<(), String> {
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

fn model_number(
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

fn parse_resistor_width_bins(
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

fn format_resistor_width_bins(bins: &[ResistorWidthBin]) -> String {
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

fn format_resistor_layer_model(name: &str, model: &ResistorLayerModel) -> String {
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

fn parse_parameter_correlations(
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

fn parse_parameter_covariances(
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

fn parse_statistical_parameters(
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

fn format_statistical_parameters(parameters: &[StatisticalParameterModel]) -> String {
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
                let properties: Vec<_> =
                    tokens[2..].iter().map(|token| token.to_string()).collect();
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

fn parse_optimization_parameters(
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

fn parse_pareto_constraints(source: &str, line_no: usize) -> Result<Vec<ParetoConstraint>, String> {
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

fn format_pareto_constraints(constraints: &[ParetoConstraint]) -> String {
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

fn format_parameter_correlations(correlations: &[ParameterCorrelation]) -> String {
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

fn format_parameter_covariances(covariances: &[ParameterCovariance]) -> String {
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

fn format_optimization_parameters(parameters: &[OptimizationParameter]) -> String {
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

fn parse_element(
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

fn parse_properties(tokens: &[String]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut i = 0;
    while i < tokens.len() {
        let lower = tokens[i].to_ascii_lowercase();
        if let Some((key, value)) = lower.split_once('=') {
            out.insert(key.to_string(), value.to_string());
            i += 1;
        } else if matches!(lower.as_str(), "ic" | "temp") && i + 1 < tokens.len() {
            out.insert(lower, tokens[i + 1].clone());
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

fn split_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut paren_depth = 0usize;
    for ch in text.chars() {
        match ch {
            '(' => {
                paren_depth += 1;
                current.push(ch);
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                current.push(ch);
            }
            c if c.is_whitespace() && paren_depth == 0 => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

pub fn parse_number(text: &str) -> Result<f64, String> {
    let cleaned: String = text
        .trim()
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '_')
        .collect();
    if cleaned.is_empty() {
        return Err(format!("invalid number '{}'", text));
    }
    let lower = cleaned.to_ascii_lowercase();
    let suffixes = [
        ("meg", 1e6),
        ("mil", 25.4e-6),
        ("t", 1e12),
        ("g", 1e9),
        ("k", 1e3),
        ("m", 1e-3),
        ("u", 1e-6),
        ("n", 1e-9),
        ("p", 1e-12),
        ("f", 1e-15),
    ];
    for (suffix, scale) in suffixes {
        if let Some(prefix) = lower.strip_suffix(suffix) {
            let mantissa: f64 = prefix
                .parse()
                .map_err(|_| format!("invalid number '{}'", text))?;
            return Ok(mantissa * scale);
        }
    }
    cleaned
        .parse()
        .map_err(|_| format!("invalid number '{}'", text))
}

fn expect_arg_count(got: usize, expected: usize, line_no: usize, name: &str) -> Result<(), String> {
    if got == expected {
        Ok(())
    } else {
        Err(format!(
            "line {}: '{}' expects {} arguments, found {}",
            line_no, name, expected, got
        ))
    }
}

pub fn write_spice(circuit: &SpiceCircuit) -> Result<String, String> {
    circuit.validate()?;
    let mut out = String::new();
    out.push_str(if circuit.title.trim().is_empty() {
        "untitled circuit"
    } else {
        &circuit.title
    });
    out.push('\n');
    if (circuit.temperature_k - 300.15).abs() > 1e-9 {
        out.push_str(&format!(
            ".temp {}\n",
            format_number(circuit.temperature_k - 273.15)
        ));
    }
    for (name, model) in &circuit.resistor_layer_models {
        out.push_str(&format_resistor_layer_model(name, model));
    }
    for (name, location) in &circuit.locations {
        out.push_str(&format!(
            ".location {} {} {}\n",
            name,
            format_number(location.x_m),
            format_number(location.y_m)
        ));
    }
    for model in circuit.statistical_models.values() {
        out.push_str(&format!(
            ".stats name={} params={} corr={}\n",
            model.name,
            format_statistical_parameters(&model.parameters),
            format_parameter_correlations(&model.correlations)
        ));
    }
    for (name, model) in &circuit.diode_models {
        out.push_str(&format!(
            ".model {} D(IS={} N={} RS={} CJO={} VJ={} M={} FC={} KF={} AF={} EF={} EG={} XTI={} TT={})\n",
            name,
            format_number(model.saturation_current_a),
            format_number(model.emission_coefficient),
            format_number(model.series_resistance_ohm),
            format_number(model.zero_bias_junction_cap_f),
            format_number(model.junction_potential_v),
            format_number(model.grading_coefficient),
            format_number(model.forward_knee),
            format_number(model.flicker_coefficient_a2),
            format_number(model.flicker_current_exponent),
            format_number(model.flicker_frequency_exponent),
            format_number(model.energy_gap_ev),
            format_number(model.saturation_current_temperature_exponent),
            format_number(model.transit_time_s)
        ));
    }
    for (name, model) in &circuit.mosfet_models {
        let polarity = match model.polarity {
            MosfetPolarity::Nmos => "NMOS",
            MosfetPolarity::Pmos => "PMOS",
        };
        let body = match model.kind {
            MosfetModelKind::Level1 => format!(
                "KP={} VTO={} LAMBDA={} GAMMA={} PHI={} THETA={} VSAT={} CGSO={} CGDO={} CGB={} COX={} CJSWS={} CJSWD={} PB={} MJ={} FC={} XPART={} KF={} AF={} EF={} MEXP={} RTH={} CTH={}",
                format_number(model.transconductance_a_per_v2),
                format_number(model.threshold_voltage_v),
                format_number(model.channel_length_modulation_per_v),
                format_number(model.body_effect_v_per_sqrt_v),
                format_number(model.surface_potential_v),
                format_number(model.mobility_degradation_inverse_v),
                format_number(model.velocity_saturation_voltage_v),
                format_number(model.gate_source_overlap_cap_per_m),
                format_number(model.gate_drain_overlap_cap_per_m),
                format_number(model.gate_bulk_cap_per_m2),
                format_number(model.gate_oxide_cap_per_m2),
                format_number(model.source_junction_sidewall_cap_per_m),
                format_number(model.drain_junction_sidewall_cap_per_m),
                format_number(model.junction_potential_v),
                format_number(model.junction_grading_coefficient),
                format_number(model.junction_forward_knee),
                format_number(model.saturation_charge_partition),
                format_number(model.flicker_coefficient_a2),
                format_number(model.flicker_current_exponent),
                format_number(model.flicker_frequency_exponent),
                format_number(model.mobility_temperature_exponent),
                format_number(model.self_heating_thermal_resistance_k_per_w),
                format_number(model.self_heating_thermal_capacitance_j_per_k)
            ),
            MosfetModelKind::Bsim3LongChannel => format!(
                "LEVEL=8 VTH0={} K1={} K2={} DVT0={} DVT1={} DVT2={} DVT0W={} DVT1W={} W0={} KT1={} KT2={} KT1L={} LAMBDA={} PHI={} TOX={} U0={} UA={} UB={} UC={} VSAT={} CGSO={} CGDO={} CGB={} COX={} CJSWS={} CJSWD={} PB={} MJ={} FC={} XPART={} KF={} AF={} EF={} UTE={} RTH={} CTH={}",
                format_number(model.threshold_voltage_v),
                format_number(model.body_effect_v_per_sqrt_v),
                format_number(model.bsim3_body_bias_threshold_coefficient),
                format_number(model.bsim3_short_channel_threshold_amplitude),
                format_number(model.bsim3_short_channel_length_scale_m),
                format_number(model.bsim3_short_channel_drain_bias_coefficient_per_v),
                format_number(model.bsim3_narrow_width_threshold_amplitude_v),
                format_number(model.bsim3_narrow_width_exponent),
                format_number(model.bsim3_reference_width_m),
                format_number(model.bsim3_threshold_temperature_coefficient_v),
                format_number(model.bsim3_threshold_body_temperature_coefficient_v_per_v),
                format_number(model.bsim3_threshold_length_temperature_coefficient_v_m),
                format_number(model.channel_length_modulation_per_v),
                format_number(model.surface_potential_v),
                format_number(model.bsim3_oxide_thickness_m),
                format_number(model.bsim3_low_field_mobility_m2_per_v_s),
                format_number(model.bsim3_mobility_degradation_first_order_m_per_v),
                format_number(model.bsim3_mobility_degradation_second_order_m2_per_v2),
                format_number(model.bsim3_mobility_body_bias_coefficient_m2_per_v2),
                format_number(model.bsim3_saturation_velocity_m_per_s),
                format_number(model.gate_source_overlap_cap_per_m),
                format_number(model.gate_drain_overlap_cap_per_m),
                format_number(model.gate_bulk_cap_per_m2),
                format_number(model.gate_oxide_cap_per_m2),
                format_number(model.source_junction_sidewall_cap_per_m),
                format_number(model.drain_junction_sidewall_cap_per_m),
                format_number(model.junction_potential_v),
                format_number(model.junction_grading_coefficient),
                format_number(model.junction_forward_knee),
                format_number(model.saturation_charge_partition),
                format_number(model.flicker_coefficient_a2),
                format_number(model.flicker_current_exponent),
                format_number(model.flicker_frequency_exponent),
                format_number(model.mobility_temperature_exponent),
                format_number(model.self_heating_thermal_resistance_k_per_w),
                format_number(model.self_heating_thermal_capacitance_j_per_k)
            ),
        };
        let body = if let Some(bin) = model.geometry_bin {
            format!(
                "{body} WMIN={} WMAX={} LMIN={} LMAX={}",
                format_number(bin.min_width_m),
                format_number(bin.max_width_m),
                format_number(bin.min_length_m),
                format_number(bin.max_length_m)
            )
        } else {
            body
        };
        out.push_str(&format!(".model {} {}({body})\n", name, polarity));
    }
    for (name, model) in &circuit.bjt_models {
        out.push_str(&format!(
            ".model {} {}(IS={} BF={} BR={} VAF={} VAR={} CJE={} CJC={} VJE={} VJC={} MJE={} MJC={} FC={} KF={} AF={} EF={} EG={} XTI={} RTH={} CTH={} TF={} TR={})\n",
            name,
            match model.polarity {
                BjtPolarity::Npn => "NPN",
                BjtPolarity::Pnp => "PNP",
            },
            format_number(model.saturation_current_a),
            format_number(model.forward_beta),
            format_number(model.reverse_beta),
            format_number(model.forward_early_voltage_v),
            format_number(model.reverse_early_voltage_v),
            format_number(model.base_emitter_zero_bias_cap_f),
            format_number(model.base_collector_zero_bias_cap_f),
            format_number(model.base_emitter_junction_potential_v),
            format_number(model.base_collector_junction_potential_v),
            format_number(model.base_emitter_grading_coefficient),
            format_number(model.base_collector_grading_coefficient),
            format_number(model.junction_forward_knee),
            format_number(model.flicker_coefficient_a2),
            format_number(model.flicker_current_exponent),
            format_number(model.flicker_frequency_exponent),
            format_number(model.energy_gap_ev),
            format_number(model.saturation_current_temperature_exponent),
            format_number(model.self_heating_thermal_resistance_k_per_w),
            format_number(model.self_heating_thermal_capacitance_j_per_k),
            format_number(model.forward_transit_time_s),
            format_number(model.reverse_transit_time_s)
        ));
    }
    for element in &circuit.elements {
        write_element(&mut out, element);
    }
    for analysis in &circuit.analyses {
        match analysis {
            Analysis::OperatingPoint => out.push_str(".op\n"),
            Analysis::DcSweep {
                source,
                start,
                stop,
                step,
            } => out.push_str(&format!(
                ".dc {} {} {} {}\n",
                source,
                format_number(*start),
                format_number(*stop),
                format_number(*step)
            )),
            Analysis::Transient {
                step_s,
                stop_s,
                method,
            } => out.push_str(&format!(
                ".tran {} {} method={}\n",
                format_number(*step_s),
                format_number(*stop_s),
                method.as_str()
            )),
            Analysis::AcSweep {
                kind,
                points,
                start_hz,
                stop_hz,
            } => out.push_str(&format!(
                ".ac {} {} {} {}\n",
                match kind {
                    AcSweepKind::Linear => "lin",
                    AcSweepKind::Decade => "dec",
                    AcSweepKind::Octave => "oct",
                },
                points,
                format_number(*start_hz),
                format_number(*stop_hz)
            )),
        Analysis::NoiseSweep {
                output_node,
                input_source,
                kind,
                points,
                start_hz,
                stop_hz,
            } => out.push_str(&format!(
                ".noise V({}) {} {} {} {} {}\n",
                output_node,
                input_source,
                match kind {
                    AcSweepKind::Linear => "lin",
                    AcSweepKind::Decade => "dec",
                    AcSweepKind::Octave => "oct",
                },
                points,
                format_number(*start_hz),
            format_number(*stop_hz)
        )),
        Analysis::MonteCarlo(options) => out.push_str(&format!(
            ".mc {} {} V({}) seed={} freq={} rtol={} ctol={} ltol={} disdec={} vtotol={} kptol={} bftol={} bisdec={} rcorr={} ccorr={} lcorr={} dscorr={} vtocorr={} kpcorr={} bfcorr={} biscorr={} spatialcorr={} spatialscale={} dist={} stats={} matrix={} cov={}\n",
            options.runs,
            options.target.as_str(),
            options.output_node,
            options.seed,
            format_number(options.frequency_hz),
            format_number(options.resistor_tolerance),
            format_number(options.capacitor_tolerance),
            format_number(options.inductor_tolerance),
            format_number(options.diode_is_decades),
            format_number(options.mos_vto_tolerance),
            format_number(options.mos_kp_tolerance),
            format_number(options.bjt_beta_tolerance),
            format_number(options.bjt_is_decades),
            format_number(options.resistor_correlation),
            format_number(options.capacitor_correlation),
            format_number(options.inductor_correlation),
            format_number(options.diode_is_correlation),
            format_number(options.mos_vto_correlation),
            format_number(options.mos_kp_correlation),
            format_number(options.bjt_beta_correlation),
            format_number(options.bjt_is_correlation),
            format_number(options.spatial_correlation),
            format_number(options.spatial_correlation_length_m),
            options.distribution.as_str(),
            options.statistical_model.as_deref().unwrap_or_default(),
            format_parameter_correlations(&options.correlations),
            format_parameter_covariances(&options.covariances)
        )),
        Analysis::Sensitivity(options) => out.push_str(&format!(
            ".sens {} V({}) freq={} step={}\n",
            options.target.as_str(),
            options.output_node,
            format_number(options.frequency_hz),
            format_number(options.relative_step)
        )),
        Analysis::GlobalSensitivity(options) => out.push_str(&format!(
            ".sobol {} V({}) freq={} samples={} seed={} tol={} corr={}\n",
            options.target.as_str(),
            options.output_node,
            format_number(options.frequency_hz),
            options.samples,
            options.seed,
            format_number(options.tolerance),
            format_parameter_correlations(&options.correlations)
        )),
        Analysis::Derivative(options) => {
            let prefix = match options.target {
                DerivativeTarget::AcMagnitude => format!(
                    ".deriv ac V({}) freq={}",
                    options.output_node,
                    format_number(options.frequency_hz)
                ),
                DerivativeTarget::TransientFinal => {
                    format!(".deriv tran V({})", options.output_node)
                }
                DerivativeTarget::Dc => format!(".deriv V({})", options.output_node),
            };
            out.push_str(&format!(
                "{prefix} params={}\n",
                options.parameters.join(",")
            ))
        }
        Analysis::Optimization(options) => out.push_str(&format!(
            ".optim {} {} V({}) goal={} freq={} params={} iterations={} population={} seed={} tol={} f={} cr={}\n",
            options.objective.as_str(),
            options.target.as_str(),
            options.output_node,
            format_number(options.goal_value),
            format_number(options.frequency_hz),
            format_optimization_parameters(&options.parameters),
            options.iterations,
            options.population,
            options.seed,
            format_number(options.tolerance),
            format_number(options.differential_weight),
            format_number(options.crossover_probability)
        )),
        Analysis::Pareto(options) => {
            let outputs = options
                .output_nodes
                .iter()
                .map(|node| format!("V({node})"))
                .collect::<Vec<_>>()
                .join(" ");
            let senses = options
                .senses
                .iter()
                .map(|sense| sense.as_str())
                .collect::<Vec<_>>()
                .join(",");
            out.push_str(&format!(
            ".pareto {} {} sense={} freq={} params={} iterations={} population={} seed={} points={} front={} divisions={} adaptive={} f={} cr={} quantum={} constraints={}\n",
            options.target.as_str(),
            outputs,
            senses,
            format_number(options.frequency_hz),
            format_optimization_parameters(&options.parameters),
            options.iterations,
            options.population,
            options.seed,
            options.maximum_points,
            options.front_method.as_str(),
            options.reference_divisions,
            options.adaptive_references,
            format_number(options.differential_weight),
            format_number(options.crossover_probability),
            options
                .parameters
                .iter()
                .filter_map(|parameter| parameter.quantum)
                .next()
                .map(format_number)
                .unwrap_or_default(),
            format_pareto_constraints(&options.constraints)
            ))
        }
        Analysis::PeriodicSteadyState(options) => {
            let mut line = format!(
                ".pss V({}) fund={} auto={}",
                options.output_node,
                format_number(options.fundamental_hz),
                options.automatic_fundamental
            );
            if let Some(threshold) = options.threshold {
                line.push_str(&format!(" threshold={}", format_number(threshold)));
            }
            line.push_str(&format!(
                " min={} max={} samples={} harmonics={} tol={}\n",
                options.minimum_cycles,
                options.maximum_cycles,
                options.samples_per_cycle,
                options.harmonics,
                format_number(options.tolerance)
            ));
            out.push_str(&line);
        }
        Analysis::EnvelopeFollow(options) => out.push_str(&format!(
            ".env V({}) carrier={} cycles={} points={}\n",
            options.output_node,
            format_number(options.carrier_hz),
            format_number(options.carrier_cycles),
            options.points
        )),
        Analysis::Aging(options) => out.push_str(&format!(
            ".aging {} V({}) freq={} years={} exp={} rinc={} vtshift={} kploss={}\n",
            options.target.as_str(),
            options.output_node,
            format_number(options.frequency_hz),
            format_number(options.age_years),
            format_number(options.time_exponent),
            format_number(options.resistance_increase_per_year),
            format_number(options.mos_threshold_shift_v_per_year),
            format_number(options.mos_kp_degradation_per_year)
        )),
    }
    }
    out.push_str(".end\n");
    Ok(out)
}

fn write_element(out: &mut String, element: &SpiceElement) {
    let mut line = element.name.clone();
    for node in &element.nodes {
        line.push(' ');
        line.push_str(node);
    }
    match &element.kind {
        SpiceElementKind::Resistor {
            resistance,
            first_temperature_coefficient_per_k,
            second_temperature_coefficient_per_k2,
            sheet_resistance_ohm_per_square,
            length_m,
            width_m,
            multiplier,
            layer,
        } => {
            line.push_str(&format!(
                " {} TC1={} TC2={}",
                format_number(
                    if layer.is_some() || *sheet_resistance_ohm_per_square > 0.0 {
                        *multiplier
                    } else {
                        *resistance
                    }
                ),
                format_number(*first_temperature_coefficient_per_k),
                format_number(*second_temperature_coefficient_per_k2),
            ));
            if let Some(layer) = layer {
                line.push_str(&format!(
                    " LAYER={} L={} W={}",
                    layer,
                    format_number(*length_m),
                    format_number(*width_m)
                ));
            } else if *sheet_resistance_ohm_per_square > 0.0 {
                line.push_str(&format!(
                    " SHEET={} L={} W={}",
                    format_number(*sheet_resistance_ohm_per_square),
                    format_number(*length_m),
                    format_number(*width_m)
                ));
            }
        }
        SpiceElementKind::Capacitor {
            capacitance,
            initial_voltage,
        } => {
            line.push_str(&format!(
                " {} IC={}",
                format_number(*capacitance),
                format_number(*initial_voltage)
            ));
        }
        SpiceElementKind::Inductor {
            inductance,
            initial_current,
        } => {
            line.push_str(&format!(
                " {} IC={}",
                format_number(*inductance),
                format_number(*initial_current)
            ));
        }
        SpiceElementKind::VoltageSource {
            waveform,
            ac_magnitude,
            ac_phase_deg,
        } => {
            line.push_str(&format_source(waveform, *ac_magnitude, *ac_phase_deg));
        }
        SpiceElementKind::CurrentSource {
            waveform,
            ac_magnitude,
            ac_phase_deg,
        } => {
            line.push_str(&format_source(waveform, *ac_magnitude, *ac_phase_deg));
        }
        SpiceElementKind::VoltageControlledVoltageSource { gain }
        | SpiceElementKind::VoltageControlledCurrentSource {
            transconductance: gain,
        } => {
            line.push_str(&format!(" {}", format_number(*gain)));
        }
        SpiceElementKind::CurrentControlledVoltageSource {
            controlling_source,
            transresistance,
        } => {
            line.push_str(&format!(
                " {} {}",
                controlling_source,
                format_number(*transresistance)
            ));
        }
        SpiceElementKind::CurrentControlledCurrentSource {
            controlling_source,
            gain,
        } => {
            line.push_str(&format!(" {} {}", controlling_source, format_number(*gain)));
        }
        SpiceElementKind::Diode { model } => line.push_str(&format!(" {}", model)),
        SpiceElementKind::Mosfet {
            model,
            width_m,
            length_m,
            multiplier,
        } => line.push_str(&format!(
            " {} W={} L={} M={}",
            model,
            format_number(*width_m),
            format_number(*length_m),
            format_number(*multiplier)
        )),
        SpiceElementKind::BipolarJunctionTransistor { model, area } => {
            line.push_str(&format!(" {} AREA={}", model, format_number(*area)));
        }
    }
    out.push_str(&line);
    out.push('\n');
}

fn format_source(waveform: &SourceWaveform, ac_magnitude: Option<f64>, ac_phase: f64) -> String {
    let mut out = String::new();
    if let Some(magnitude) = ac_magnitude {
        out.push_str(&format!(
            " AC {} {}",
            format_number(magnitude),
            format_number(ac_phase)
        ));
    }
    match waveform {
        SourceWaveform::Dc(value) => {
            if ac_magnitude.is_some() {
                out.push_str(&format!(" DC {}", format_number(*value)));
            } else {
                out.push_str(&format!(" {}", format_number(*value)));
            }
        }
        SourceWaveform::Sin {
            offset,
            amplitude,
            frequency_hz,
            delay_s,
            damping_per_s,
        } => out.push_str(&format!(
            " SIN({} {} {} {} {})",
            format_number(*offset),
            format_number(*amplitude),
            format_number(*frequency_hz),
            format_number(*delay_s),
            format_number(*damping_per_s)
        )),
        SourceWaveform::Am {
            offset,
            amplitude,
            carrier_hz,
            modulation_hz,
            depth,
        } => out.push_str(&format!(
            " AM({} {} {} {} {})",
            format_number(*offset),
            format_number(*amplitude),
            format_number(*carrier_hz),
            format_number(*modulation_hz),
            format_number(*depth)
        )),
        SourceWaveform::Pulse {
            initial,
            pulsed,
            delay_s,
            rise_s,
            fall_s,
            width_s,
            period_s,
        } => out.push_str(&format!(
            " PULSE({} {} {} {} {} {} {})",
            format_number(*initial),
            format_number(*pulsed),
            format_number(*delay_s),
            format_number(*rise_s),
            format_number(*fall_s),
            format_number(*width_s),
            format_number(*period_s)
        )),
        SourceWaveform::Pwl(points) => {
            out.push_str(" PWL(");
            for (idx, (time, value)) in points.iter().enumerate() {
                if idx > 0 {
                    out.push(' ');
                }
                out.push_str(&format!(
                    "{} {}",
                    format_number(*time),
                    format_number(*value)
                ));
            }
            out.push(')');
        }
    }
    out
}

pub fn format_number(value: f64) -> String {
    if !value.is_finite() {
        if value.is_infinite() && value.is_sign_positive() {
            return "inf".into();
        }
        if value.is_infinite() {
            return "-inf".into();
        }
        return "nan".into();
    }
    if value == 0.0 {
        return "0".into();
    }
    let abs = value.abs();
    if (1e-15..1e12).contains(&abs) {
        format!("{}", value)
    } else {
        format!("{:e}", value)
    }
}
