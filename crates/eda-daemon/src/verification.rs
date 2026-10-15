//! Formal property, DFT, CDC, LVS, UPF, and safety RPC handlers.

use crate::{
    optional_positive_usize, optional_top, physical_timing::parse_scan_options, required_string,
    Json,
};
use eda_api::{
    analyze_rtl_cdc_cancellable, cdc_report, check_rtl_properties_cancellable,
    insert_rtl_scan_cancellable, proof_report, prove_safety_cancellable, run_flow_cancellable,
    FlowCancellation, ProofStatus, PropertyBmcStatus, Severity as CdcSeverity,
};

fn formal_trace_json(trace: Option<&[std::collections::HashMap<String, u64>]>) -> Json {
    let Some(frames) = trace else {
        return Json::Null;
    };
    Json::Array(
        frames
            .iter()
            .map(|frame| {
                Json::Object(
                    frame
                        .iter()
                        .map(|(name, value)| (name.clone(), Json::from(*value)))
                        .collect(),
                )
            })
            .collect(),
    )
}

pub(crate) fn rtl_bmc(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let properties = required_string(params, "properties")?;
    let depth = optional_positive_usize(params, "depth", 10)?;
    let result = check_rtl_properties_cancellable(&source, top, &properties, depth, cancellation)?;
    let property_results = result
        .results
        .iter()
        .map(|property| {
            Ok(Json::object([
                ("assertion", Json::string(property.assertion.clone())),
                (
                    "kind",
                    Json::string(match property.kind {
                        eda_api::AssertionKind::Assert => "assert",
                        eda_api::AssertionKind::Assume => "assume",
                        eda_api::AssertionKind::Cover => "cover",
                    }),
                ),
                (
                    "clock",
                    property.clock.clone().map_or(Json::Null, Json::string),
                ),
                (
                    "status",
                    Json::string(match property.status {
                        PropertyBmcStatus::BoundedPass => "boundedPass",
                        PropertyBmcStatus::Violated => "violated",
                        PropertyBmcStatus::Satisfied => "satisfied",
                        PropertyBmcStatus::NotSatisfied => "notSatisfied",
                        PropertyBmcStatus::AssumptionsUnsat => "assumptionsUnsat",
                    }),
                ),
                ("violated", Json::Bool(property.violated())),
                ("covered", Json::Bool(property.covered())),
                ("depthChecked", Json::number(property.depth_checked as f64)?),
                (
                    "framesChecked",
                    Json::number(property.frames_checked as f64)?,
                ),
                (
                    "triggerFrame",
                    property
                        .trigger_frame
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "repeatCount",
                    property
                        .repeat_count
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "endFrame",
                    property
                        .end_frame
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                ("inputTrace", formal_trace_json(property.trace.as_deref())),
                ("netTrace", formal_trace_json(property.net_trace.as_deref())),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("depth", Json::number(depth as f64)?),
        ("properties", Json::number(result.results.len() as f64)?),
        (
            "assumptions",
            Json::Array(result.assumptions.iter().map(Json::string).collect()),
        ),
        (
            "violations",
            Json::number(
                result
                    .results
                    .iter()
                    .filter(|property| property.violated())
                    .count() as f64,
            )?,
        ),
        ("results", Json::Array(property_results)),
        (
            "mappedStats",
            Json::object([
                ("nets", Json::number(result.mapped_stats.nets as f64)?),
                ("cells", Json::number(result.mapped_stats.cells as f64)?),
                ("comb", Json::number(result.mapped_stats.comb as f64)?),
                ("seq", Json::number(result.mapped_stats.seq as f64)?),
                ("consts", Json::number(result.mapped_stats.consts as f64)?),
                (
                    "libCells",
                    Json::number(result.mapped_stats.lib_cells as f64)?,
                ),
            ]),
        ),
        ("report", Json::string(result.report)),
    ]))
}

pub(crate) fn rtl_dft(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let options = parse_scan_options(params)?;
    let result = insert_rtl_scan_cancellable(&source, top, &options, cancellation)?;
    let chain = result
        .scan
        .chain
        .iter()
        .map(|cell| {
            Ok(Json::object([
                ("cell", Json::number(cell.cell as f64)?),
                ("name", Json::string(cell.name.clone())),
                ("muxCell", Json::number(cell.mux_cell as f64)?),
                ("qNet", Json::number(cell.q_net as f64)?),
                (
                    "functionalDNet",
                    Json::number(cell.functional_d_net as f64)?,
                ),
                ("scanDNet", Json::number(cell.scan_d_net as f64)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        (
            "scan",
            Json::object([
                ("chainName", Json::string(options.chain_name.clone())),
                (
                    "scanEnPort",
                    Json::string(result.scan.netlist.net(result.scan.scan_en).name.clone()),
                ),
                (
                    "scanInPort",
                    Json::string(result.scan.netlist.net(result.scan.scan_in).name.clone()),
                ),
                (
                    "scanOutPort",
                    Json::string(result.scan.netlist.net(result.scan.scan_out).name.clone()),
                ),
                ("chain", Json::Array(chain)),
            ]),
        ),
        (
            "dft",
            Json::object([
                (
                    "sequentialCells",
                    Json::number(result.dft.sequential_cells as f64)?,
                ),
                ("scanCells", Json::number(result.dft.scan_cells as f64)?),
                ("scanChains", Json::number(result.dft.scan_chains as f64)?),
                (
                    "combinationalCells",
                    Json::number(result.dft.combinational_cells as f64)?,
                ),
                (
                    "unscannedSequential",
                    Json::Array(
                        result
                            .dft
                            .unscanned_sequential
                            .iter()
                            .map(Json::string)
                            .collect(),
                    ),
                ),
            ]),
        ),
        (
            "originalStats",
            Json::object([
                ("cells", Json::number(result.original_stats.cells as f64)?),
                ("seq", Json::number(result.original_stats.seq as f64)?),
            ]),
        ),
        (
            "scannedStats",
            Json::object([
                ("cells", Json::number(result.scanned_stats.cells as f64)?),
                ("seq", Json::number(result.scanned_stats.seq as f64)?),
                (
                    "libCells",
                    Json::number(result.scanned_stats.lib_cells as f64)?,
                ),
            ]),
        ),
        ("report", Json::string(result.report)),
    ]))
}

fn cdc_path_json(path: &eda_api::CdcPath) -> Result<Json, String> {
    Ok(Json::object([
        (
            "sourceCell",
            path.source_cell
                .map_or(Json::Null, |value| Json::from(value as i64)),
        ),
        ("sourceName", Json::string(path.source_name.clone())),
        (
            "sourceHierarchy",
            path.source_hierarchy
                .clone()
                .map_or(Json::Null, Json::string),
        ),
        ("sourceClock", Json::string(path.source_clock.clone())),
        ("destinationCell", Json::from(path.destination_cell as i64)),
        (
            "destinationName",
            Json::string(path.destination_name.clone()),
        ),
        (
            "destinationHierarchy",
            path.destination_hierarchy
                .clone()
                .map_or(Json::Null, Json::string),
        ),
        (
            "destinationClock",
            Json::string(path.destination_clock.clone()),
        ),
        ("destinationPin", Json::string(path.destination_pin.clone())),
        (
            "combinationalCells",
            Json::Array(
                path.combinational_cells
                    .iter()
                    .map(|cell| Json::from(*cell as i64))
                    .collect(),
            ),
        ),
        (
            "hierarchyInstances",
            Json::Array(path.hierarchy_instances.iter().map(Json::string).collect()),
        ),
        ("synchronized", Json::Bool(path.synchronized)),
    ]))
}

pub(crate) fn rtl_cdc(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let result = analyze_rtl_cdc_cancellable(&source, top, cancellation)?;
    let clocks = result
        .cdc
        .clocks
        .iter()
        .map(|clock| {
            Ok(Json::object([
                ("name", Json::string(clock.name.clone())),
                ("rootNet", Json::number(clock.root_net as f64)?),
                ("rootName", Json::string(clock.root_name.clone())),
                (
                    "clockNets",
                    Json::Array(
                        clock
                            .clock_nets
                            .iter()
                            .map(|net| Json::from(*net as i64))
                            .collect(),
                    ),
                ),
                (
                    "sequentialCells",
                    Json::Array(
                        clock
                            .sequential_cells
                            .iter()
                            .map(|cell| Json::from(*cell as i64))
                            .collect(),
                    ),
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let crossings = result
        .cdc
        .crossings
        .iter()
        .map(cdc_path_json)
        .collect::<Result<Vec<_>, String>>()?;
    let findings = result
        .cdc
        .findings
        .iter()
        .map(|finding| {
            Ok(Json::object([
                ("kind", Json::string(finding.kind.clone())),
                (
                    "severity",
                    Json::string(match finding.severity {
                        CdcSeverity::Info => "info",
                        CdcSeverity::Warning => "warning",
                        CdcSeverity::Error => "error",
                    }),
                ),
                ("path", cdc_path_json(&finding.path)?),
                ("message", Json::string(finding.message.clone())),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("clocks", Json::Array(clocks)),
        ("crossings", Json::Array(crossings)),
        ("findings", Json::Array(findings)),
        ("hasErrors", Json::Bool(result.cdc.has_errors())),
        ("clean", Json::Bool(result.cdc.clean())),
        (
            "mappedStats",
            Json::object([
                ("nets", Json::number(result.mapped_stats.nets as f64)?),
                ("cells", Json::number(result.mapped_stats.cells as f64)?),
                ("seq", Json::number(result.mapped_stats.seq as f64)?),
                (
                    "libCells",
                    Json::number(result.mapped_stats.lib_cells as f64)?,
                ),
            ]),
        ),
        ("report", Json::string(cdc_report(&result.cdc))),
    ]))
}

pub(crate) fn optional_top_field<'a>(
    params: &'a Json,
    name: &str,
) -> Result<Option<&'a str>, String> {
    match params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?
        .get(name)
    {
        None | Some(Json::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| format!("RPC parameter '{name}' must be a string")),
    }
}

pub(crate) fn rtl_lvs(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL LVS cancelled".into());
    }
    let reference_source = required_string(params, "reference")?;
    let layout_source = required_string(params, "layout")?;
    let shared_top = optional_top(params)?;
    let reference_top = optional_top_field(params, "referenceTop")?.or(shared_top);
    let layout_top = optional_top_field(params, "layoutTop")?.or(shared_top);

    let reference_flow = run_flow_cancellable(&reference_source, reference_top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL LVS cancelled".into());
    }
    let layout_flow = run_flow_cancellable(&layout_source, layout_top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL LVS cancelled".into());
    }
    let result = eda_api::check_layout_vs_schematic(&reference_flow.netlist, &layout_flow.netlist)?;

    let cell_matches = result
        .cell_matches
        .iter()
        .map(|match_item| {
            Json::object([
                (
                    "referenceCell",
                    Json::string(match_item.reference_cell.clone()),
                ),
                ("layoutCell", Json::string(match_item.layout_cell.clone())),
                ("deviceType", Json::string(match_item.device_type.clone())),
            ])
        })
        .collect::<Vec<_>>();
    let net_matches = result
        .net_matches
        .iter()
        .map(|match_item| {
            Json::object([
                (
                    "referenceNet",
                    Json::string(match_item.reference_net.clone()),
                ),
                ("layoutNet", Json::string(match_item.layout_net.clone())),
            ])
        })
        .collect::<Vec<_>>();

    Ok(Json::object([
        ("matches", Json::Bool(result.matches)),
        ("cellMatches", Json::Array(cell_matches)),
        ("netMatches", Json::Array(net_matches)),
        (
            "shortedNets",
            Json::Array(
                result
                    .shorted_nets
                    .iter()
                    .map(|name| Json::string(name.clone()))
                    .collect(),
            ),
        ),
        (
            "openNets",
            Json::Array(
                result
                    .open_nets
                    .iter()
                    .map(|name| Json::string(name.clone()))
                    .collect(),
            ),
        ),
        (
            "notes",
            Json::Array(
                result
                    .notes
                    .iter()
                    .map(|note| Json::string(note.clone()))
                    .collect(),
            ),
        ),
        (
            "referenceStats",
            Json::object([
                (
                    "cells",
                    Json::number(reference_flow.netlist.cell_count() as f64)?,
                ),
                (
                    "nets",
                    Json::number(reference_flow.netlist.net_count() as f64)?,
                ),
            ]),
        ),
        (
            "layoutStats",
            Json::object([
                (
                    "cells",
                    Json::number(layout_flow.netlist.cell_count() as f64)?,
                ),
                (
                    "nets",
                    Json::number(layout_flow.netlist.net_count() as f64)?,
                ),
            ]),
        ),
        ("report", Json::string(eda_api::lvs_report(&result))),
    ]))
}

pub(crate) fn rtl_power_intent(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL power-intent check cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let upf_source = required_string(params, "upf")?;
    let design = eda_api::compile(&source, top)?;
    if cancellation.is_cancelled() {
        return Err("RTL power-intent check cancelled".into());
    }
    let upf = eda_api::parse_power_intent(&upf_source)?;
    let result = eda_api::power_intent_check(&design, &upf);
    if cancellation.is_cancelled() {
        return Err("RTL power-intent check cancelled".into());
    }

    let findings = result
        .findings
        .iter()
        .map(|finding| {
            Json::object([
                ("code", Json::string(finding.code.clone())),
                (
                    "severity",
                    Json::string(match finding.severity {
                        eda_api::UpfSeverity::Warning => "warning",
                        eda_api::UpfSeverity::Error => "error",
                    }),
                ),
                ("message", Json::string(finding.message.clone())),
            ])
        })
        .collect::<Vec<_>>();
    Ok(Json::object([
        ("domainCount", Json::number(result.domain_count as f64)?),
        (
            "supplyNetCount",
            Json::number(result.supply_net_count as f64)?,
        ),
        ("switchCount", Json::number(result.switch_count as f64)?),
        (
            "powerModeCount",
            Json::number(result.power_mode_count as f64)?,
        ),
        (
            "isolationCount",
            Json::number(result.isolation_count as f64)?,
        ),
        (
            "retentionCount",
            Json::number(result.retention_count as f64)?,
        ),
        (
            "levelShifterCount",
            Json::number(result.level_shifter_count as f64)?,
        ),
        ("findings", Json::Array(findings)),
        ("hasErrors", Json::Bool(result.has_errors())),
        ("report", Json::string(eda_api::upf_report(&result))),
    ]))
}

pub(crate) fn rtl_safety(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL safety proof cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let bad_net = required_string(params, "badNet")?;
    let Some(fields) = params.as_object() else {
        return Err("params must be an object".into());
    };
    let depth = match fields.get("depth") {
        None => 8,
        Some(Json::Number(value)) => {
            if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
                return Err("RPC parameter 'depth' must be a positive integer".into());
            }
            *value as usize
        }
        Some(_) => return Err("RPC parameter 'depth' must be a positive integer".into()),
    };

    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL safety proof cancelled".into());
    }
    let proof = prove_safety_cancellable(&flow.netlist, &bad_net, depth, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL safety proof cancelled".into());
    }

    Ok(Json::object([
        (
            "status",
            Json::string(match proof.status {
                ProofStatus::Proved => "proved",
                ProofStatus::Refuted => "refuted",
                ProofStatus::Unknown => "unknown",
            }),
        ),
        ("depthChecked", Json::number(proof.depth_checked as f64)?),
        ("inductive", Json::Bool(proof.inductive)),
        (
            "hasCounterexample",
            Json::Bool(proof.counterexample.is_some()),
        ),
        (
            "failedInvariant",
            proof
                .failed_invariant
                .clone()
                .map_or(Json::Null, Json::string),
        ),
        (
            "mappedStats",
            Json::object([
                ("cells", Json::number(flow.netlist.cell_count() as f64)?),
                ("nets", Json::number(flow.netlist.net_count() as f64)?),
                ("libCells", Json::number(flow.stats.lib_cells as f64)?),
            ]),
        ),
        ("report", Json::string(proof_report(&proof))),
    ]))
}
