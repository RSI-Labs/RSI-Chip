//! Physical timing, post-route power/IR, fault grading, and PDK signoff RPCs.

use crate::{optional_top, required_string, required_value, Json};
use eda_api::{
    analyze_physical_timing_power_ir, analyze_physical_timing_power_ir_from_waveform,
    analyze_physical_timing_power_ir_power_modes_from_waveform_with_upf,
    analyze_physical_timing_power_ir_power_modes_with_upf, analyze_rtl_faults_cancellable,
    default_power_vectors, fault_report, parse_pdk, parse_power_intent, parse_timing_scenarios,
    parse_waveform, physical_timing_flow_report, post_route_power_ir_report,
    post_route_power_scenarios_from_timing_scenarios, report_pdk_signoff, run_flow_cancellable,
    run_pdk_signoff_cancellable, run_physical_timing_flow_cancellable, upf_power_mode_ir_report,
    write_delay_file, write_parasitics, Flow, FlowCancellation, IrOptions,
    MultilayerPowerMeshOptions, PatternOptions, PhysicalTimingFlowOptions, PowerOptions,
    RtlSimulationVector, ScanOptions,
};
use eda_core::logic::LogicWord;
use std::collections::{BTreeMap, HashMap};

fn parse_physical_timing_options(params: &Json) -> Result<PhysicalTimingFlowOptions, String> {
    let Some(Json::Object(fields)) = params
        .as_object()
        .map(|fields| fields.get("physicalTimingOptions"))
        .and_then(|value| value.cloned())
    else {
        return Ok(PhysicalTimingFlowOptions::default());
    };
    let mut options = PhysicalTimingFlowOptions::default();
    for (json_name, target) in [
        ("tracksPerChannel", &mut options.route.tracks_per_channel),
        ("iterations", &mut options.iterations),
        ("placementPasses", &mut options.placement_passes),
        ("routingPasses", &mut options.routing_passes),
        (
            "antennaMaxDiodesPerNet",
            &mut options.antenna_max_diodes_per_net,
        ),
    ] {
        if let Some(value) = fields.get(json_name) {
            let Some(value) = value.as_number() else {
                return Err(format!(
                    "physical timing option '{json_name}' must be a positive integer"
                ));
            };
            if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
                return Err(format!(
                    "physical timing option '{json_name}' must be a positive integer"
                ));
            }
            *target = *value as usize;
        }
    }
    if let Some(value) = fields.get("repairAntennas") {
        match value {
            Json::Bool(value) => options.repair_antennas = *value,
            _ => return Err("physical timing option 'repairAntennas' must be a boolean".into()),
        }
    }

    let interconnect_names = [
        (
            "wireResistanceOhmPerSite",
            &mut options.interconnect.wire_resistance_ohm_per_site,
        ),
        (
            "wireCapacitancePfPerSite",
            &mut options.interconnect.wire_capacitance_pf_per_site,
        ),
        (
            "couplingCapacitancePfPerSite",
            &mut options.interconnect.coupling_capacitance_pf_per_site,
        ),
        (
            "viaResistanceOhm",
            &mut options.interconnect.via_resistance_ohm,
        ),
        (
            "viaCapacitancePf",
            &mut options.interconnect.via_capacitance_pf,
        ),
    ];
    for (json_name, target) in interconnect_names {
        if let Some(value) = fields.get(json_name) {
            let Some(value) = value.as_number() else {
                return Err(format!(
                    "physical timing interconnect option '{json_name}' must be a number"
                ));
            };
            *target = *value;
        }
    }
    Ok(options)
}

pub(crate) fn rtl_physical_timing(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL physical timing analysis cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let options = parse_physical_timing_options(params)?;
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL physical timing analysis cancelled".into());
    }
    let result = run_physical_timing_flow_cancellable(&flow.netlist, &options, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL physical timing analysis cancelled".into());
    }
    let best = result.best();
    let spef = write_parasitics(&best.parasitics)?;
    let sdf = write_delay_file(&best.sdf)?;

    Ok(Json::object([
        (
            "baselineDelayNs",
            Json::number(result.baseline.timing.critical_path_delay)?,
        ),
        (
            "bestDelayNs",
            Json::number(best.timing.critical_path_delay)?,
        ),
        ("bestIndex", Json::number(result.best_index as f64)?),
        (
            "optimizedIterations",
            Json::number(result.optimized.len() as f64)?,
        ),
        (
            "wirelength",
            Json::number(best.routing.wirelength() as f64)?,
        ),
        (
            "rcNets",
            Json::number(best.parasitic_report.net_count as f64)?,
        ),
        ("drcErrors", Json::number(best.drc.error_count() as f64)?),
        ("spef", Json::string(spef)),
        ("sdf", Json::string(sdf)),
        (
            "mappedStats",
            Json::object([
                ("cells", Json::number(flow.netlist.cell_count() as f64)?),
                ("nets", Json::number(flow.netlist.net_count() as f64)?),
                ("libCells", Json::number(flow.stats.lib_cells as f64)?),
            ]),
        ),
        ("report", Json::string(physical_timing_flow_report(&result))),
    ]))
}

fn fault_json(fault: &eda_api::Fault) -> Result<Json, String> {
    Ok(Json::object([
        ("index", Json::number(fault.index as f64)?),
        ("net", Json::number(fault.net as f64)?),
        ("netName", Json::string(fault.net_name.clone())),
        (
            "stuckAt",
            Json::string(match fault.stuck_at {
                eda_api::StuckAt::Zero => "0",
                eda_api::StuckAt::One => "1",
            }),
        ),
    ]))
}

enum PostRoutePowerStimulus {
    Vectors(Vec<HashMap<String, LogicWord>>),
    Waveform(eda_api::Vcd),
}

impl PostRoutePowerStimulus {
    fn sample_count(&self) -> usize {
        match self {
            Self::Vectors(vectors) => vectors.len(),
            Self::Waveform(waveform) => waveform.time_points().len(),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Vectors(_) => "vectors",
            Self::Waveform(_) => "vcd",
        }
    }
}

fn parse_post_route_power_stimulus(
    params: &Json,
    flow: &Flow,
) -> Result<PostRoutePowerStimulus, String> {
    let fields = params.as_object();
    let has_vcd = fields.is_some_and(|fields| fields.contains_key("vcd"));
    let has_vectors = fields.is_some_and(|fields| fields.contains_key("vectors"));
    if has_vcd && has_vectors {
        return Err("RPC parameters 'vcd' and 'vectors' are mutually exclusive".into());
    }
    if has_vcd {
        let source = required_string(params, "vcd")?;
        return Ok(PostRoutePowerStimulus::Waveform(parse_waveform(&source)?));
    }

    let simulation_vectors = if params
        .as_object()
        .is_some_and(|fields| fields.contains_key("vectors"))
    {
        parse_rtl_simulation_vectors(params)?
    } else {
        Vec::new()
    };
    if simulation_vectors.is_empty() {
        return Ok(PostRoutePowerStimulus::Vectors(default_power_vectors(
            &flow.netlist,
        )));
    }

    let widths = flow
        .netlist
        .ports
        .iter()
        .filter(|port| port.dir == eda_core::netlist::Dir::In)
        .map(|port| (port.name.clone(), port.bits.len()))
        .collect::<BTreeMap<_, _>>();
    let inputs = simulation_vectors
        .iter()
        .map(|vector| {
            let mut inputs = HashMap::new();
            for (name, width) in &widths {
                let text = vector
                    .inputs
                    .get(name)
                    .ok_or_else(|| format!("post-route vector lacks input '{name}'"))?;
                let word = LogicWord::parse(*width, text)
                    .ok_or_else(|| format!("invalid post-route value for input '{name}'"))?;
                inputs.insert(name.clone(), word);
            }
            Ok(inputs)
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(PostRoutePowerStimulus::Vectors(inputs))
}

pub(crate) fn rtl_post_route_power_ir(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL post-route power/IR analysis cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let scenario_source = required_string(params, "scenarios")?;
    let timing_definitions = parse_timing_scenarios(&scenario_source)?;
    let scenarios = post_route_power_scenarios_from_timing_scenarios(&timing_definitions)?;
    if scenarios.is_empty() {
        return Err("RTL post-route power/IR requires at least one scenario".into());
    }
    let physical_options = parse_physical_timing_options(params)?;
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL post-route power/IR analysis cancelled".into());
    }
    let stimulus = parse_post_route_power_stimulus(params, &flow)?;
    let physical =
        run_physical_timing_flow_cancellable(&flow.netlist, &physical_options, cancellation)?;
    let iteration = physical.best();

    let mut results = Vec::with_capacity(scenarios.len());
    for scenario in &scenarios {
        if cancellation.is_cancelled() {
            return Err("RTL post-route power/IR analysis cancelled".into());
        }
        results.push(match &stimulus {
            PostRoutePowerStimulus::Vectors(vectors) => {
                analyze_physical_timing_power_ir(iteration, vectors, &scenario.options)?
            }
            PostRoutePowerStimulus::Waveform(waveform) => {
                analyze_physical_timing_power_ir_from_waveform(
                    iteration,
                    waveform,
                    &scenario.options,
                )?
            }
        });
    }
    if cancellation.is_cancelled() {
        return Err("RTL post-route power/IR analysis cancelled".into());
    }

    let mut worst_power_corner = 0;
    let mut worst_drop_corner = 0;
    let mut error_findings = 0usize;
    for (index, result) in results.iter().enumerate() {
        if result.power.total_power_uw > results[worst_power_corner].power.total_power_uw {
            worst_power_corner = index;
        }
        if result.ir.worst_drop_v > results[worst_drop_corner].ir.worst_drop_v {
            worst_drop_corner = index;
        }
        error_findings += result
            .ir
            .findings
            .iter()
            .filter(|finding| finding.severity == eda_api::IrSeverity::Error)
            .count();
    }

    let corners = scenarios
        .iter()
        .zip(results.iter())
        .map(|(scenario, result)| {
            let findings = result
                .ir
                .findings
                .iter()
                .map(|finding| {
                    Ok(Json::object([
                        ("code", Json::string(finding.code.clone())),
                        (
                            "severity",
                            Json::string(match finding.severity {
                                eda_api::IrSeverity::Warning => "warning",
                                eda_api::IrSeverity::Error => "error",
                            }),
                        ),
                        (
                            "row",
                            finding.row.map_or(Json::Null, |row| Json::from(row as i64)),
                        ),
                        ("message", Json::string(finding.message.clone())),
                    ]))
                })
                .collect::<Result<Vec<_>, String>>()?;
            let effective_capacitance_pf = result.effective_capacitance_pf.values().sum();
            Ok(Json::object([
                ("scenario", Json::string(scenario.name.clone())),
                ("voltageV", Json::number(result.ir.voltage_v)?),
                (
                    "dynamicPowerUw",
                    Json::number(result.power.dynamic_power_uw)?,
                ),
                ("staticPowerUw", Json::number(result.power.static_power_uw)?),
                ("totalPowerUw", Json::number(result.power.total_power_uw)?),
                (
                    "parasiticDynamicPowerUw",
                    Json::number(result.parasitic_dynamic_power_uw)?,
                ),
                (
                    "effectiveCapacitancePf",
                    Json::number(effective_capacitance_pf)?,
                ),
                (
                    "effectiveCapacitanceNets",
                    Json::from(result.effective_capacitance_pf.len() as u64),
                ),
                ("totalCurrentA", Json::number(result.ir.total_current_a)?),
                ("worstDropV", Json::number(result.ir.worst_drop_v)?),
                (
                    "worstRow",
                    result
                        .ir
                        .worst_row
                        .map_or(Json::Null, |row| Json::from(row as i64)),
                ),
                ("hasIrErrors", Json::Bool(result.ir.has_errors())),
                ("findings", Json::Array(findings)),
                ("report", Json::string(post_route_power_ir_report(result))),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(Json::object([
        ("scenarioCount", Json::from(scenarios.len() as u64)),
        ("vectorCount", Json::from(stimulus.sample_count() as u64)),
        ("stimulus", Json::string(stimulus.kind())),
        (
            "worstPowerScenario",
            Json::string(scenarios[worst_power_corner].name.clone()),
        ),
        (
            "worstDropScenario",
            Json::string(scenarios[worst_drop_corner].name.clone()),
        ),
        ("errorFindings", Json::from(error_findings as u64)),
        ("corners", Json::Array(corners)),
        (
            "physicalTimingReport",
            Json::string(physical_timing_flow_report(&physical)),
        ),
        (
            "mappedStats",
            Json::object([
                ("cells", Json::number(flow.netlist.cell_count() as f64)?),
                ("nets", Json::number(flow.netlist.net_count() as f64)?),
                ("libCells", Json::number(flow.stats.lib_cells as f64)?),
            ]),
        ),
    ]))
}

pub(crate) fn rtl_post_route_power_mode_ir(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL post-route power-mode IR analysis cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let scenario_source = required_string(params, "scenarios")?;
    let upf_source = required_string(params, "upf")?;
    let timing_definitions = parse_timing_scenarios(&scenario_source)?;
    let mut scenarios = post_route_power_scenarios_from_timing_scenarios(&timing_definitions)?;
    if scenarios.is_empty() {
        return Err("RTL post-route power-mode IR requires at least one scenario".into());
    }

    let mesh_enabled = params
        .as_object()
        .and_then(|fields| fields.get("powerModeOptions"))
        .and_then(Json::as_object)
        .and_then(|fields| fields.get("multilayerPowerMesh"))
        .is_some_and(|value| matches!(value, Json::Bool(enabled) if *enabled));
    let mesh_rows = params
        .as_object()
        .and_then(|fields| fields.get("powerModeOptions"))
        .and_then(Json::as_object)
        .and_then(|fields| fields.get("rows"))
        .and_then(Json::as_number)
        .map_or(3, |value| value.max(1.0) as usize);
    let mesh_columns = params
        .as_object()
        .and_then(|fields| fields.get("powerModeOptions"))
        .and_then(Json::as_object)
        .and_then(|fields| fields.get("columns"))
        .and_then(Json::as_number)
        .map_or(3, |value| value.max(1.0) as usize);
    if mesh_enabled {
        for scenario in &mut scenarios {
            let ir = scenario.options.ir.clone();
            scenario.options.multilayer_power_mesh = Some(MultilayerPowerMeshOptions {
                rows: mesh_rows,
                columns: mesh_columns,
                voltage_v: ir.voltage_v,
                max_total_drop_v: ir.max_total_drop_v,
                max_node_current_a: ir.max_rail_current_a,
                max_segment_current_a: ir.max_rail_current_a,
                ..MultilayerPowerMeshOptions::default()
            });
        }
    }

    let power_intent = parse_power_intent(&upf_source)?;
    let physical_options = parse_physical_timing_options(params)?;
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL post-route power-mode IR analysis cancelled".into());
    }
    let stimulus = parse_post_route_power_stimulus(params, &flow)?;
    let physical =
        run_physical_timing_flow_cancellable(&flow.netlist, &physical_options, cancellation)?;
    let iteration = physical.best();

    let mut results = Vec::with_capacity(scenarios.len());
    for scenario in &scenarios {
        if cancellation.is_cancelled() {
            return Err("RTL post-route power-mode IR analysis cancelled".into());
        }
        results.push(match &stimulus {
            PostRoutePowerStimulus::Vectors(vectors) => {
                analyze_physical_timing_power_ir_power_modes_with_upf(
                    iteration,
                    vectors,
                    &scenario.options,
                    &power_intent,
                )?
            }
            PostRoutePowerStimulus::Waveform(waveform) => {
                analyze_physical_timing_power_ir_power_modes_from_waveform_with_upf(
                    iteration,
                    waveform,
                    &scenario.options,
                    &power_intent,
                )?
            }
        });
    }
    if cancellation.is_cancelled() {
        return Err("RTL post-route power-mode IR analysis cancelled".into());
    }

    let mut worst_power = (0usize, 0usize);
    let mut worst_drop = (0usize, 0usize);
    let mut worst_em_lifetime = (0usize, 0usize);
    let mut worst_power_value = f64::NEG_INFINITY;
    let mut worst_drop_value = f64::NEG_INFINITY;
    let mut worst_em_value = None::<f64>;
    let mut error_findings = 0usize;
    for (corner_index, result) in results.iter().enumerate() {
        for (mode_index, mode) in result.modes.iter().enumerate() {
            if mode.result.total_power_uw > worst_power_value {
                worst_power_value = mode.result.total_power_uw;
                worst_power = (corner_index, mode_index);
            }
            if mode.result.worst_drop_v > worst_drop_value {
                worst_drop_value = mode.result.worst_drop_v;
                worst_drop = (corner_index, mode_index);
            }
            if let Some(lifetime) = mode
                .worst_em_lifetime_hours
                .filter(|value| value.is_finite())
            {
                if worst_em_value.is_none_or(|current| lifetime < current) {
                    worst_em_value = Some(lifetime);
                    worst_em_lifetime = (corner_index, mode_index);
                }
            }
        }
        error_findings += result.error_findings;
    }

    let corners = scenarios
        .iter()
        .zip(results.iter())
        .map(|(scenario, result)| {
            let modes = result
                .modes
                .iter()
                .map(|mode| {
                    let findings = mode
                        .result
                        .findings
                        .iter()
                        .map(|finding| {
                            Ok(Json::object([
                                ("code", Json::string(finding.code.clone())),
                                (
                                    "severity",
                                    Json::string(match finding.severity {
                                        eda_api::IrSeverity::Warning => "warning",
                                        eda_api::IrSeverity::Error => "error",
                                    }),
                                ),
                                (
                                    "row",
                                    finding.row.map_or(Json::Null, |row| Json::from(row as i64)),
                                ),
                                ("message", Json::string(finding.message.clone())),
                            ]))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    let domains = mode
                        .result
                        .domains
                        .iter()
                        .map(|domain| {
                            Ok(Json::object([
                                ("name", Json::string(domain.name.clone())),
                                ("powered", Json::Bool(domain.powered)),
                                ("voltageV", Json::number(domain.voltage_v)?),
                                ("cellCount", Json::from(domain.cell_count as u64)),
                                ("totalPowerUw", Json::number(domain.total_power_uw)?),
                                ("totalCurrentA", Json::number(domain.total_current_a)?),
                                ("worstDropV", Json::number(domain.worst_drop_v)?),
                            ]))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    Ok(Json::object([
                        ("powerMode", Json::string(mode.power_mode.clone())),
                        ("totalPowerUw", Json::number(mode.result.total_power_uw)?),
                        ("totalCurrentA", Json::number(mode.result.total_current_a)?),
                        (
                            "switchLeakagePowerUw",
                            Json::number(mode.result.switch_leakage_power_uw)?,
                        ),
                        (
                            "strategyPowerUw",
                            Json::number(mode.result.strategy_power_uw)?,
                        ),
                        ("worstDropV", Json::number(mode.result.worst_drop_v)?),
                        (
                            "worstPowerDomain",
                            mode.result
                                .worst_power_domain
                                .clone()
                                .map_or(Json::Null, Json::string),
                        ),
                        (
                            "worstRow",
                            mode.result
                                .worst_row
                                .map_or(Json::Null, |row| Json::from(row as i64)),
                        ),
                        (
                            "coupledMultilayerMesh",
                            Json::Bool(mode.coupled_multilayer_mesh),
                        ),
                        (
                            "worstEmLifetimeHours",
                            mode.worst_em_lifetime_hours
                                .filter(|lifetime| lifetime.is_finite())
                                .map_or(Ok(Json::Null), Json::number)?,
                        ),
                        (
                            "worstEmLifetimeUnbounded",
                            Json::Bool(
                                mode.worst_em_lifetime_hours
                                    .is_some_and(|lifetime| lifetime.is_infinite()),
                            ),
                        ),
                        ("domains", Json::Array(domains)),
                        ("hasErrors", Json::Bool(mode.result.has_errors())),
                        ("findings", Json::Array(findings)),
                    ]))
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(Json::object([
                ("scenario", Json::string(scenario.name.clone())),
                ("modeCount", Json::from(result.modes.len() as u64)),
                (
                    "worstPowerMode",
                    result
                        .modes
                        .get(result.worst_power_mode)
                        .map_or(Json::Null, |mode| Json::string(mode.power_mode.clone())),
                ),
                (
                    "worstDropMode",
                    result
                        .modes
                        .get(result.worst_drop_mode)
                        .map_or(Json::Null, |mode| Json::string(mode.power_mode.clone())),
                ),
                ("modes", Json::Array(modes)),
                ("report", Json::string(upf_power_mode_ir_report(result))),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(Json::object([
        ("scenarioCount", Json::from(scenarios.len() as u64)),
        ("vectorCount", Json::from(stimulus.sample_count() as u64)),
        ("stimulus", Json::string(stimulus.kind())),
        (
            "worstPowerScenario",
            Json::string(scenarios[worst_power.0].name.clone()),
        ),
        (
            "worstPowerMode",
            Json::string(
                results[worst_power.0].modes[worst_power.1]
                    .power_mode
                    .clone(),
            ),
        ),
        (
            "worstDropScenario",
            Json::string(scenarios[worst_drop.0].name.clone()),
        ),
        (
            "worstDropMode",
            Json::string(results[worst_drop.0].modes[worst_drop.1].power_mode.clone()),
        ),
        (
            "worstEmLifetimeScenario",
            Json::string(scenarios[worst_em_lifetime.0].name.clone()),
        ),
        (
            "worstEmLifetimeMode",
            Json::string(
                results[worst_em_lifetime.0].modes[worst_em_lifetime.1]
                    .power_mode
                    .clone(),
            ),
        ),
        (
            "worstEmLifetimeHours",
            worst_em_value.map_or(Ok(Json::Null), Json::number)?,
        ),
        ("errorFindings", Json::from(error_findings as u64)),
        ("corners", Json::Array(corners)),
        (
            "physicalTimingReport",
            Json::string(physical_timing_flow_report(&physical)),
        ),
        (
            "mappedStats",
            Json::object([
                ("cells", Json::number(flow.netlist.cell_count() as f64)?),
                ("nets", Json::number(flow.netlist.net_count() as f64)?),
                ("libCells", Json::number(flow.stats.lib_cells as f64)?),
            ]),
        ),
    ]))
}

pub(crate) fn rtl_fault(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let options = parse_pattern_options(params)?;
    let result = analyze_rtl_faults_cancellable(&source, top, &options, cancellation)?;
    let patterns = result
        .faults
        .patterns
        .iter()
        .map(|pattern| {
            Json::Object(
                pattern
                    .iter()
                    .map(|(name, value)| (name.clone(), Json::from(*value)))
                    .collect(),
            )
        })
        .collect::<Vec<_>>();
    let detections = result
        .faults
        .detections
        .iter()
        .map(|detection| {
            Ok(Json::object([
                ("fault", fault_json(&detection.fault)?),
                (
                    "detectingPatterns",
                    Json::Array(
                        detection
                            .detecting_patterns
                            .iter()
                            .map(|index| Json::from(*index as i64))
                            .collect(),
                    ),
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let undetected = result
        .faults
        .undetected_faults
        .iter()
        .map(fault_json)
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("patterns", Json::Array(patterns)),
        ("exhaustive", Json::Bool(result.faults.exhaustive)),
        (
            "totalFaults",
            Json::number(result.faults.total_faults as f64)?,
        ),
        (
            "detectedFaults",
            Json::number(result.faults.detected_faults as f64)?,
        ),
        ("undetectedFaults", Json::Array(undetected)),
        ("detections", Json::Array(detections)),
        (
            "coveragePercent",
            Json::number(result.faults.coverage_percent)?,
        ),
        (
            "mappedStats",
            Json::object([
                ("nets", Json::number(result.mapped_stats.nets as f64)?),
                ("cells", Json::number(result.mapped_stats.cells as f64)?),
                (
                    "libCells",
                    Json::number(result.mapped_stats.lib_cells as f64)?,
                ),
            ]),
        ),
        ("stil", Json::string(result.stil.clone())),
        ("report", Json::string(fault_report(&result.faults))),
    ]))
}

pub(crate) fn parse_rtl_simulation_vectors(
    params: &Json,
) -> Result<Vec<RtlSimulationVector>, String> {
    let vectors_value = required_value(params, "vectors")?;
    let Json::Array(vector_values) = vectors_value else {
        return Err("'vectors' must be an array".into());
    };
    let mut vectors = Vec::with_capacity(vector_values.len());
    for vector in &vector_values {
        let fields = vector
            .as_object()
            .ok_or_else(|| "each simulation vector must be an object".to_string())?;
        let time_ns = match fields.get("timeNs") {
            Some(Json::Number(value))
                if *value >= 0.0 && value.fract() == 0.0 && *value <= u64::MAX as f64 =>
            {
                *value as u64
            }
            _ => return Err("'timeNs' must be a non-negative integer".into()),
        };
        let Some(Json::Object(inputs)) = fields.get("inputs") else {
            return Err("simulation vector 'inputs' must be an object".into());
        };
        let mut parsed_inputs = BTreeMap::new();
        for (name, value) in inputs {
            let Some(value) = value.as_str() else {
                return Err(format!("simulation input '{name}' must be a string"));
            };
            parsed_inputs.insert(name.clone(), value.to_string());
        }
        vectors.push(RtlSimulationVector {
            time_ns,
            inputs: parsed_inputs,
        });
    }
    Ok(vectors)
}

pub(crate) fn parse_power_options(params: &Json) -> Result<PowerOptions, String> {
    let Some(Json::Object(fields)) = params
        .as_object()
        .map(|fields| fields.get("powerOptions"))
        .and_then(|value| value.cloned())
    else {
        return Ok(PowerOptions::default());
    };
    let mut options = PowerOptions::default();
    for (json_name, target) in [
        ("frequencyMhz", &mut options.frequency_mhz),
        ("baseCellEnergyPj", &mut options.base_cell_energy_pj),
        ("energyPerSitePj", &mut options.energy_per_site_pj),
        (
            "capacitiveEnergyPerPfPj",
            &mut options.capacitive_energy_per_pf_pj,
        ),
        ("leakagePerSiteNw", &mut options.leakage_per_site_nw),
    ] {
        if let Some(value) = fields.get(json_name) {
            let Some(value) = value.as_number() else {
                return Err(format!("power option '{json_name}' must be a number"));
            };
            *target = *value;
        }
    }
    Ok(options)
}

pub(crate) fn parse_ir_options(params: &Json) -> Result<IrOptions, String> {
    let Some(Json::Object(fields)) = params
        .as_object()
        .map(|fields| fields.get("irOptions"))
        .and_then(|value| value.cloned())
    else {
        return Ok(IrOptions::default());
    };
    let mut options = IrOptions::default();
    for (json_name, target) in [
        ("voltageV", &mut options.voltage_v),
        ("vddOhmPerSite", &mut options.vdd_ohm_per_site),
        ("gndOhmPerSite", &mut options.gnd_ohm_per_site),
        ("maxTotalDropV", &mut options.max_total_drop_v),
        ("maxRailCurrentA", &mut options.max_rail_current_a),
    ] {
        if let Some(value) = fields.get(json_name) {
            let Some(value) = value.as_number() else {
                return Err(format!("IR option '{json_name}' must be a number"));
            };
            *target = *value;
        }
    }
    Ok(options)
}

pub(crate) fn parse_scan_options(params: &Json) -> Result<ScanOptions, String> {
    let Some(Json::Object(fields)) = params
        .as_object()
        .map(|fields| fields.get("scanOptions"))
        .and_then(|value| value.cloned())
    else {
        return Ok(ScanOptions::default());
    };
    let mut options = ScanOptions::default();
    for (json_name, target) in [
        ("chainName", &mut options.chain_name),
        ("scanEnPort", &mut options.scan_en_port),
        ("scanInPort", &mut options.scan_in_port),
        ("scanOutPort", &mut options.scan_out_port),
    ] {
        if let Some(value) = fields.get(json_name) {
            let Some(value) = value.as_str() else {
                return Err(format!("scan option '{json_name}' must be a string"));
            };
            *target = value.to_string();
        }
    }
    Ok(options)
}

pub(crate) fn parse_pattern_options(params: &Json) -> Result<PatternOptions, String> {
    let Some(Json::Object(fields)) = params
        .as_object()
        .map(|fields| fields.get("patternOptions"))
        .and_then(|value| value.cloned())
    else {
        return Ok(PatternOptions::default());
    };
    let mut options = PatternOptions::default();
    if let Some(value) = fields.get("maxRandomPatterns") {
        let Json::Number(value) = value else {
            return Err("pattern option 'maxRandomPatterns' must be a positive integer".into());
        };
        if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
            return Err("pattern option 'maxRandomPatterns' must be a positive integer".into());
        }
        options.max_random_patterns = *value as usize;
    }
    if let Some(value) = fields.get("exhaustiveInputLimit") {
        let Json::Number(value) = value else {
            return Err(
                "pattern option 'exhaustiveInputLimit' must be an integer from 1 through 64".into(),
            );
        };
        if *value < 1.0 || value.fract() != 0.0 || *value > 64.0 {
            return Err(
                "pattern option 'exhaustiveInputLimit' must be an integer from 1 through 64".into(),
            );
        }
        options.exhaustive_input_limit = *value as usize;
    }
    if let Some(value) = fields.get("randomSeed") {
        let Json::Number(value) = value else {
            return Err("pattern option 'randomSeed' must be a non-negative integer".into());
        };
        if *value < 0.0 || value.fract() != 0.0 || *value > u64::MAX as f64 {
            return Err("pattern option 'randomSeed' must be a non-negative integer".into());
        }
        options.random_seed = *value as u64;
    }
    Ok(options)
}

pub(crate) fn pdk_signoff(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let manifest = required_string(params, "pdk")?;
    let fields = params.as_object().ok_or("params must be an object")?;
    let top = match fields.get("top") {
        None | Some(Json::Null) => None,
        Some(value) => Some(value.as_str().ok_or("'top' must be a string")?),
    };
    let pdk = parse_pdk(&manifest)?;
    let result = run_pdk_signoff_cancellable(&source, top, &pdk, cancellation)?;
    Ok(Json::object([
        ("passed", Json::Bool(result.passed)),
        ("pdk", Json::string(result.pdk_name.clone())),
        ("version", Json::string(result.pdk_version.clone())),
        ("design", Json::string(result.design_name.clone())),
        (
            "layoutRuleDeckSha256",
            result
                .layout_rule_deck_sha256
                .clone()
                .map_or(Json::Null, Json::string),
        ),
        (
            "layoutRuleDeckValidated",
            Json::Bool(result.layout_rule_deck_validated),
        ),
        (
            "layoutRuleDeckAntennaRatio",
            result
                .layout_rule_deck_antenna_ratio
                .map_or(Json::Null, |value| {
                    Json::number(value).unwrap_or(Json::Null)
                }),
        ),
        ("report", Json::string(report_pdk_signoff(&result))),
    ]))
}
