//! RTL PnR, antenna repair, simulation, power, and IR RPC handlers.

use crate::physical_timing::{parse_ir_options, parse_power_options, parse_rtl_simulation_vectors};
use crate::{optional_top, required_string, Json};
use eda_api::{
    analyze_rtl_ir_cancellable, analyze_rtl_power_cancellable, implement_rtl_pnr_cancellable,
    implement_rtl_pnr_with_drc_options_cancellable, ir_report, power_report,
    repair_rtl_antennas_cancellable, simulate_rtl_cancellable, FlowCancellation,
};
use std::collections::BTreeMap;

fn parse_drc_options(params: &Json) -> Result<eda_api::DrcOptions, String> {
    let Some(options) = params
        .as_object()
        .and_then(|fields| fields.get("drcOptions"))
    else {
        return Ok(eda_api::DrcOptions::default());
    };
    let fields = options
        .as_object()
        .ok_or_else(|| "'drcOptions' must be an object".to_string())?;
    let max_antenna_ratio = match fields.get("maxAntennaRatio") {
        None | Some(Json::Null) => eda_api::DrcOptions::default().max_antenna_ratio,
        Some(Json::Number(value)) => *value,
        Some(_) => return Err("DRC option 'maxAntennaRatio' must be a number".into()),
    };
    let options = eda_api::DrcOptions { max_antenna_ratio };
    options.validate()?;
    Ok(options)
}

pub(crate) fn rtl_pnr(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let drc_options = parse_drc_options(params)?;
    let result = if drc_options == eda_api::DrcOptions::default() {
        implement_rtl_pnr_cancellable(&source, top, cancellation)?
    } else {
        implement_rtl_pnr_with_drc_options_cancellable(&source, top, &drc_options, cancellation)?
    };
    let wire_length = result
        .routing
        .segments
        .iter()
        .map(|segment| segment.length())
        .sum::<usize>();
    let drc_violations = result
        .drc
        .violations
        .iter()
        .map(|violation| {
            Ok(Json::object([
                ("code", Json::string(violation.code.clone())),
                (
                    "severity",
                    Json::string(match violation.severity {
                        eda_pnr::DrcSeverity::Warning => "warning",
                        eda_pnr::DrcSeverity::Error => "error",
                    }),
                ),
                ("message", Json::string(violation.message.clone())),
                (
                    "cell",
                    violation
                        .cell
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "net",
                    violation
                        .net
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "row",
                    violation
                        .row
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "site",
                    violation
                        .site
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        (
            "placement",
            Json::object([
                ("rows", Json::number(result.placement.rows as f64)?),
                (
                    "placedCells",
                    Json::number(result.placement.cell_pos.len() as f64)?,
                ),
                (
                    "blockages",
                    Json::number(result.placement.blockages.len() as f64)?,
                ),
            ]),
        ),
        (
            "routing",
            Json::object([
                (
                    "segments",
                    Json::number(result.routing.segments.len() as f64)?,
                ),
                ("vias", Json::number(result.routing.vias as f64)?),
                (
                    "explicitViaLocations",
                    Json::number(result.routing.via_locations.len() as f64)?,
                ),
                (
                    "tracksPerChannel",
                    Json::number(result.routing.tracks_per_channel as f64)?,
                ),
                (
                    "congestedChannels",
                    Json::number(result.routing.congested_channels.len() as f64)?,
                ),
                ("wireLength", Json::number(wire_length as f64)?),
            ]),
        ),
        (
            "drc",
            Json::object([
                (
                    "errors",
                    Json::number(
                        result
                            .drc
                            .violations
                            .iter()
                            .filter(|violation| violation.severity == eda_pnr::DrcSeverity::Error)
                            .count() as f64,
                    )?,
                ),
                (
                    "warnings",
                    Json::number(
                        result
                            .drc
                            .violations
                            .iter()
                            .filter(|violation| violation.severity == eda_pnr::DrcSeverity::Warning)
                            .count() as f64,
                    )?,
                ),
                ("violations", Json::Array(drc_violations)),
            ]),
        ),
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
        ("lef", Json::string(result.lef)),
        ("def", Json::string(result.def)),
    ]))
}

fn parse_antenna_repair_options(params: &Json) -> Result<eda_api::AntennaRepairOptions, String> {
    let Some(options) = params
        .as_object()
        .and_then(|fields| fields.get("antennaOptions"))
    else {
        return Ok(eda_api::AntennaRepairOptions::default());
    };
    let fields = options
        .as_object()
        .ok_or_else(|| "'antennaOptions' must be an object".to_string())?;
    let patch_name = match fields.get("patchName") {
        None | Some(Json::Null) => "antenna_repair".to_string(),
        Some(Json::String(value)) => value.clone(),
        Some(_) => return Err("antenna option 'patchName' must be a string".into()),
    };
    let max_diodes_per_net = match fields.get("maxDiodesPerNet") {
        None | Some(Json::Null) => 4_usize,
        Some(Json::Number(value)) => {
            if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
                return Err("antenna option 'maxDiodesPerNet' must be a positive usize".into());
            }
            *value as usize
        }
        Some(_) => return Err("antenna option 'maxDiodesPerNet' must be a positive usize".into()),
    };
    let mut route = eda_api::RouteOptions::default();
    if let Some(route_fields) = fields
        .get("routeOptions")
        .map(|value| {
            value
                .as_object()
                .ok_or_else(|| "antenna option 'routeOptions' must be an object".to_string())
        })
        .transpose()?
    {
        if let Some(value) = route_fields.get("tracksPerChannel") {
            let Json::Number(value) = value else {
                return Err("route option 'tracksPerChannel' must be a positive usize".into());
            };
            if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
                return Err("route option 'tracksPerChannel' must be a positive usize".into());
            }
            route.tracks_per_channel = *value as usize;
        }
        if let Some(value) = route_fields.get("allowCongestion") {
            let Json::Bool(value) = value else {
                return Err("route option 'allowCongestion' must be a boolean".into());
            };
            route.allow_congestion = *value;
        }
    }
    let options = eda_api::AntennaRepairOptions {
        patch_name,
        max_diodes_per_net,
        route,
    };
    options.validate()?;
    Ok(options)
}

fn pnr_drc_json(drc: &eda_api::DrcResult) -> Result<Json, String> {
    let violations = drc
        .violations
        .iter()
        .map(|violation| {
            Ok(Json::object([
                ("code", Json::string(violation.code.clone())),
                (
                    "severity",
                    Json::string(match violation.severity {
                        eda_pnr::DrcSeverity::Warning => "warning",
                        eda_pnr::DrcSeverity::Error => "error",
                    }),
                ),
                ("message", Json::string(violation.message.clone())),
                (
                    "cell",
                    violation
                        .cell
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "net",
                    violation
                        .net
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "row",
                    violation
                        .row
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                (
                    "site",
                    violation
                        .site
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("errors", Json::from(drc.error_count() as u64)),
        ("warnings", Json::from(drc.warning_count() as u64)),
        ("violations", Json::Array(violations)),
    ]))
}

pub(crate) fn rtl_antenna_repair(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL antenna repair cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let options = parse_antenna_repair_options(params)?;
    let result = repair_rtl_antennas_cancellable(&source, top, &options, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL antenna repair cancelled".into());
    }
    let repairs = result
        .repair
        .repairs
        .iter()
        .map(|repair| {
            Ok(Json::object([
                ("net", Json::string(repair.net.clone())),
                ("oldRatio", Json::number(repair.old_ratio)?),
                ("newRatio", Json::number(repair.new_ratio)?),
                ("diodesInserted", Json::from(repair.diodes_inserted as u64)),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let wire_length = result
        .repair
        .routing
        .segments
        .iter()
        .map(|segment| segment.length())
        .sum::<usize>();
    Ok(Json::object([
        ("baselineDrc", pnr_drc_json(&result.baseline_drc)?),
        (
            "repairCount",
            Json::from(result.repair.repairs.len() as u64),
        ),
        (
            "diodesInserted",
            Json::from(result.repair.diodes_inserted as u64),
        ),
        (
            "antennaWarnings",
            Json::from(result.repair.antenna_warnings as u64),
        ),
        (
            "allAntennasRepaired",
            Json::Bool(result.repair.all_antennas_repaired),
        ),
        ("repairs", Json::Array(repairs)),
        (
            "placement",
            Json::object([
                (
                    "rows",
                    Json::number(result.repair.physical_placement.rows as f64)?,
                ),
                (
                    "placedCells",
                    Json::number(result.repair.physical_placement.cell_pos.len() as f64)?,
                ),
            ]),
        ),
        (
            "routing",
            Json::object([
                (
                    "segments",
                    Json::number(result.repair.routing.segments.len() as f64)?,
                ),
                ("vias", Json::number(result.repair.routing.vias as f64)?),
                (
                    "tracksPerChannel",
                    Json::number(result.repair.routing.tracks_per_channel as f64)?,
                ),
                ("wireLength", Json::number(wire_length as f64)?),
            ]),
        ),
        (
            "mappedStats",
            Json::object([
                ("nets", Json::number(result.final_mapped_stats.nets as f64)?),
                (
                    "cells",
                    Json::number(result.final_mapped_stats.cells as f64)?,
                ),
                (
                    "libCells",
                    Json::number(result.final_mapped_stats.lib_cells as f64)?,
                ),
            ]),
        ),
        (
            "report",
            Json::string(eda_api::antenna_repair_report(&result.repair)),
        ),
    ]))
}

pub(crate) fn rtl_sim(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let vectors = parse_rtl_simulation_vectors(params)?;
    let result = simulate_rtl_cancellable(&source, top, &vectors, cancellation)?;
    let samples = result
        .samples
        .iter()
        .map(|sample| {
            Ok(Json::object([
                ("timeNs", Json::number(sample.time_ns as f64)?),
                (
                    "outputs",
                    Json::Object(
                        sample
                            .outputs
                            .iter()
                            .map(|(name, value)| (name.clone(), Json::string(value.clone())))
                            .collect(),
                    ),
                ),
                (
                    "displays",
                    Json::Array(sample.displays.iter().map(Json::string).collect()),
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("samples", Json::Array(samples)),
        ("finished", Json::Bool(result.finished)),
        ("finalTimeNs", Json::number(result.final_time_ns as f64)?),
    ]))
}

pub(crate) fn rtl_power(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let vectors = parse_rtl_simulation_vectors(params)?;
    let options = parse_power_options(params)?;
    let result = analyze_rtl_power_cancellable(&source, top, &vectors, &options, cancellation)?;
    let activities = result
        .report
        .activity_by_net
        .iter()
        .map(|(net, activity)| {
            Ok((
                net.clone(),
                Json::object([
                    ("transitions", Json::number(activity.transitions as f64)?),
                    ("knownSamples", Json::number(activity.known_samples as f64)?),
                    ("oneSamples", Json::number(activity.one_samples as f64)?),
                    (
                        "toggleProbability",
                        Json::number(activity.toggle_probability(result.report.intervals as u64))?,
                    ),
                    (
                        "staticProbability",
                        Json::number(activity.static_probability())?,
                    ),
                ]),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let cells = result
        .report
        .cells
        .iter()
        .map(|cell| {
            Ok(Json::object([
                ("name", Json::string(cell.name.clone())),
                ("widthSites", Json::number(cell.width_sites as f64)?),
                ("activity", Json::number(cell.activity)?),
                ("dynamicPowerUw", Json::number(cell.dynamic_power_uw)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("frequencyMhz", Json::number(result.report.frequency_mhz)?),
        ("vectors", Json::number(result.report.vectors as f64)?),
        ("intervals", Json::number(result.report.intervals as f64)?),
        (
            "dynamicPowerUw",
            Json::number(result.report.dynamic_power_uw)?,
        ),
        (
            "staticPowerUw",
            Json::number(result.report.static_power_uw)?,
        ),
        ("totalPowerUw", Json::number(result.report.total_power_uw)?),
        ("activityByNet", Json::Object(activities)),
        ("cells", Json::Array(cells)),
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
        ("report", Json::string(power_report(&result.report))),
    ]))
}

pub(crate) fn rtl_ir(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let vectors = parse_rtl_simulation_vectors(params)?;
    let power_options = parse_power_options(params)?;
    let ir_options = parse_ir_options(params)?;
    let result = analyze_rtl_ir_cancellable(
        &source,
        top,
        &vectors,
        &power_options,
        &ir_options,
        cancellation,
    )?;
    let rows = result
        .ir
        .rows
        .iter()
        .map(|row| {
            Ok(Json::object([
                ("row", Json::number(row.row as f64)?),
                ("totalCurrentA", Json::number(row.total_current_a)?),
                ("vddWorstDropV", Json::number(row.vdd_worst_drop_v)?),
                ("gndWorstDropV", Json::number(row.gnd_worst_drop_v)?),
                ("totalDropV", Json::number(row.total_drop_v)?),
                ("emMargin", Json::number(row.em_margin)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let cells = result
        .ir
        .cells
        .iter()
        .map(|cell| {
            Ok(Json::object([
                ("cell", Json::number(cell.cell as f64)?),
                ("name", Json::string(cell.name.clone())),
                ("row", Json::number(cell.row as f64)?),
                ("siteStart", Json::number(cell.site_start as f64)?),
                ("siteEnd", Json::number(cell.site_end as f64)?),
                ("currentMa", Json::number(cell.current_ma)?),
                ("localDropV", Json::number(cell.local_drop_v)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
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
                    finding
                        .row
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                ("message", Json::string(finding.message.clone())),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        (
            "power",
            Json::object([
                ("vectors", Json::number(result.power.vectors as f64)?),
                (
                    "dynamicPowerUw",
                    Json::number(result.power.dynamic_power_uw)?,
                ),
                ("staticPowerUw", Json::number(result.power.static_power_uw)?),
                ("totalPowerUw", Json::number(result.power.total_power_uw)?),
            ]),
        ),
        (
            "ir",
            Json::object([
                ("voltageV", Json::number(result.ir.voltage_v)?),
                ("totalPowerUw", Json::number(result.ir.total_power_uw)?),
                ("totalCurrentA", Json::number(result.ir.total_current_a)?),
                ("worstDropV", Json::number(result.ir.worst_drop_v)?),
                (
                    "worstRow",
                    result
                        .ir
                        .worst_row
                        .map_or(Json::Null, |value| Json::from(value as i64)),
                ),
                ("rows", Json::Array(rows)),
                ("cells", Json::Array(cells)),
                ("findings", Json::Array(findings)),
                ("hasErrors", Json::Bool(result.ir.has_errors())),
            ]),
        ),
        (
            "placement",
            Json::object([
                ("rows", Json::number(result.placement.rows as f64)?),
                (
                    "placedCells",
                    Json::number(result.placement.cell_pos.len() as f64)?,
                ),
                (
                    "blockages",
                    Json::number(result.placement.blockages.len() as f64)?,
                ),
            ]),
        ),
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
        ("report", Json::string(ir_report(&result.ir))),
    ]))
}
