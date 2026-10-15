//! RTL implementation, equivalence, STA, CTS/ECO, and quality RPC handlers.

use crate::verification::optional_top_field;
use crate::{optional_top, required_string, Json};
use eda_api::{
    analyze_rtl_timing_cancellable, analyze_signal_quality, apply_eco,
    check_netlists_equivalent_cancellable, check_rtl_equivalence_cancellable,
    clock_tree_synthesis_with_placement, cts_report, eco_report, export_verilog, parse_eco,
    run_flow_cancellable, run_flow_with_library_cancellable, signal_quality_report,
    timing_driven_clock_tree_synthesis_with_placement, timing_driven_cts_report, CellLibrary, Flow,
    FlowCacheKey, FlowCancellation, QualityKind, QualityOptions, ScopedQualityLimit,
    SharedFlowCache, TimingDrivenCtsOptions, TimingObjectKind,
};

pub(crate) fn rtl_flow(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = match params
        .as_object()
        .ok_or("params must be an object")?
        .get("top")
    {
        None | Some(Json::Null) => None,
        Some(value) => Some(value.as_str().ok_or("'top' must be a string")?),
    };
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    flow_result_json(&flow)
}

pub(crate) fn rtl_flow_cached(
    params: &Json,
    cancellation: &FlowCancellation,
    flow_cache: &SharedFlowCache,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("cached RTL flow cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let library = CellLibrary::std();
    let key = FlowCacheKey::new(&source, top, &library);
    let cached = flow_cache.get_or_insert_with(&key, || {
        run_flow_with_library_cancellable(&source, top, &library, cancellation)
    })?;
    if cancellation.is_cancelled() {
        return Err("cached RTL flow cancelled".into());
    }
    let mut result = flow_result_json(&cached.flow)?;
    let Json::Object(fields) = &mut result else {
        return Err("internal error: RTL flow result is not an object".into());
    };
    fields.insert("cacheHit".into(), Json::Bool(cached.cache_hit));
    fields.insert("cacheId".into(), Json::string(key.cache_id()));
    Ok(result)
}

pub(crate) fn flow_cache_stats_json(flow_cache: &SharedFlowCache) -> Result<Json, String> {
    let stats = flow_cache.stats()?;
    Ok(Json::object([
        ("hits", Json::from(stats.hits)),
        ("misses", Json::from(stats.misses)),
        ("inserts", Json::from(stats.inserts)),
        ("evictions", Json::from(stats.evictions)),
    ]))
}

pub(crate) fn rtl_cts(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let timing_driven = match fields.get("timingDriven") {
        None | Some(Json::Null) => false,
        Some(Json::Bool(value)) => *value,
        Some(_) => return Err("'timingDriven' must be a boolean".into()),
    };
    let target_skew_ns = match fields.get("targetSkewNs") {
        None | Some(Json::Null) => 0.05,
        Some(Json::Number(value)) => {
            if !value.is_finite() || *value < 0.0 {
                return Err("'targetSkewNs' must be finite and nonnegative".into());
            }
            *value
        }
        Some(_) => return Err("'targetSkewNs' must be a number".into()),
    };
    if !timing_driven && fields.contains_key("targetSkewNs") {
        return Err("'targetSkewNs' requires 'timingDriven': true".into());
    }
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if timing_driven {
        let options = TimingDrivenCtsOptions {
            target_skew_ns,
            ..Default::default()
        };
        let result = timing_driven_clock_tree_synthesis_with_placement(
            &flow.netlist,
            &flow.placement,
            &options,
        )?;
        let mut trees = Vec::new();
        for tree in &result.cts.trees {
            trees.push(clock_tree_json(tree)?);
        }
        return Ok(Json::object([
            ("timingDriven", Json::Bool(true)),
            (
                "buffersInserted",
                Json::from(result.cts.buffers_inserted as u64),
            ),
            (
                "originalCells",
                Json::from(result.cts.original_cells as u64),
            ),
            ("originalNets", Json::from(result.cts.original_nets as u64)),
            (
                "selectedBranchFactor",
                Json::from(result.selected_branch_factor as u64),
            ),
            (
                "searchedBranchFactors",
                Json::Array(
                    result
                        .searched_branch_factors
                        .iter()
                        .map(|factor| Json::from(*factor as u64))
                        .collect(),
                ),
            ),
            (
                "bestEstimatedSkewNs",
                Json::number(result.best_estimated_skew_ns)?,
            ),
            ("targetMet", Json::Bool(result.target_met)),
            ("trees", Json::Array(trees)),
            ("report", Json::string(timing_driven_cts_report(&result))),
        ]));
    }
    let result =
        clock_tree_synthesis_with_placement(&flow.netlist, &flow.placement, &Default::default())?;
    let trees = result
        .trees
        .iter()
        .map(clock_tree_json)
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("timingDriven", Json::Bool(false)),
        (
            "buffersInserted",
            Json::from(result.buffers_inserted as u64),
        ),
        ("originalCells", Json::from(result.original_cells as u64)),
        ("originalNets", Json::from(result.original_nets as u64)),
        ("trees", Json::Array(trees)),
        ("report", Json::string(cts_report(&result))),
    ]))
}

fn clock_tree_json(tree: &eda_api::ClockTree) -> Result<Json, String> {
    Ok(Json::object([
        ("domain", Json::string(tree.domain.clone())),
        ("root", Json::from(tree.root as u64)),
        ("sinkCount", Json::from(tree.sinks.len() as u64)),
        ("bufferCells", Json::from(tree.buffer_cells.len() as u64)),
        ("levels", Json::from(tree.levels as u64)),
        ("branchFactor", Json::from(tree.branch_factor as u64)),
        ("maxFanout", Json::from(tree.max_fanout as u64)),
        (
            "insertionDelayNs",
            Json::number(tree.estimated_insertion_delay_ns)?,
        ),
        (
            "logicalSkewNs",
            Json::number(tree.estimated_logical_skew_ns)?,
        ),
        ("wireDelayNs", Json::number(tree.estimated_wire_delay_ns)?),
        (
            "physicalSkewNs",
            Json::number(tree.estimated_physical_skew_ns)?,
        ),
        ("skewNs", Json::number(tree.estimated_skew_ns)?),
        ("placementAware", Json::Bool(tree.placement_aware)),
    ]))
}

pub(crate) fn rtl_eco(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL ECO cancelled".into());
    }
    let source = required_string(params, "source")?;
    let patch_source = required_string(params, "eco")?;
    let top = optional_top(params)?;
    let patch = parse_eco(&patch_source)?;
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL ECO cancelled".into());
    }
    let result = apply_eco(&flow.netlist, &patch)?;
    let placement = eda_api::extend_physical_placement(&result.netlist, &flow.placement)?;
    let stats = result.netlist.stats();
    let changes = result
        .changes
        .iter()
        .map(|change| {
            Ok(Json::object([
                ("operation", Json::from(change.operation as u64)),
                ("description", Json::string(change.description.clone())),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let verilog = export_verilog(&result.netlist)?;
    if cancellation.is_cancelled() {
        return Err("RTL ECO cancelled".into());
    }
    Ok(Json::object([
        ("patchName", Json::string(result.patch_name.clone())),
        (
            "hierarchyScope",
            result
                .hierarchy_scope
                .clone()
                .map_or(Json::Null, Json::string),
        ),
        (
            "operationsApplied",
            Json::from(result.operations_applied as u64),
        ),
        (
            "stats",
            Json::object([
                ("nets", Json::number(stats.nets as f64)?),
                ("cells", Json::number(stats.cells as f64)?),
                ("comb", Json::number(stats.comb as f64)?),
                ("seq", Json::number(stats.seq as f64)?),
                ("consts", Json::number(stats.consts as f64)?),
                ("libCells", Json::number(stats.lib_cells as f64)?),
            ]),
        ),
        ("placementRows", Json::number(placement.rows as f64)?),
        ("changes", Json::Array(changes)),
        ("verilog", Json::string(verilog)),
        ("report", Json::string(eco_report(&result))),
    ]))
}

fn flow_result_json(flow: &Flow) -> Result<Json, String> {
    let drc_violations = flow
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
    let wire_length = flow
        .routing
        .segments
        .iter()
        .map(|segment| segment.length())
        .sum::<usize>();
    Ok(Json::object([
        ("equivalent", Json::Bool(flow.equivalent)),
        ("netlist", Json::string(flow.netlist.name.clone())),
        (
            "stats",
            Json::object([
                ("nets", Json::number(flow.stats.nets as f64)?),
                ("cells", Json::number(flow.stats.cells as f64)?),
                ("comb", Json::number(flow.stats.comb as f64)?),
                ("seq", Json::number(flow.stats.seq as f64)?),
                ("consts", Json::number(flow.stats.consts as f64)?),
                ("libCells", Json::number(flow.stats.lib_cells as f64)?),
            ]),
        ),
        (
            "timing",
            Json::object([
                (
                    "criticalPathDelayNs",
                    Json::number(flow.timing.critical_path_delay)?,
                ),
                (
                    "reg2regCriticalNs",
                    Json::number(flow.timing.reg2reg_critical)?,
                ),
                (
                    "in2regCriticalNs",
                    Json::number(flow.timing.in2reg_critical)?,
                ),
                (
                    "reg2regHoldCriticalNs",
                    Json::number(flow.timing.reg2reg_hold_critical)?,
                ),
                (
                    "in2regHoldCriticalNs",
                    Json::number(flow.timing.in2reg_hold_critical)?,
                ),
                (
                    "disabledTimingArcs",
                    Json::number(flow.timing.disabled_timing_arcs as f64)?,
                ),
                (
                    "caseAnalysisNets",
                    Json::Object(
                        flow.timing
                            .case_analysis_nets
                            .iter()
                            .map(|(net, value)| (net.clone(), Json::Bool(*value)))
                            .collect(),
                    ),
                ),
            ]),
        ),
        (
            "physical",
            Json::object([
                ("placementRows", Json::number(flow.placement.rows as f64)?),
                (
                    "placedCells",
                    Json::number(flow.placement.cell_pos.len() as f64)?,
                ),
                (
                    "placementBlockages",
                    Json::number(flow.placement.blockages.len() as f64)?,
                ),
                (
                    "routingSegments",
                    Json::number(flow.routing.segments.len() as f64)?,
                ),
                ("vias", Json::number(flow.routing.vias as f64)?),
                (
                    "explicitViaLocations",
                    Json::number(flow.routing.via_locations.len() as f64)?,
                ),
                (
                    "tracksPerChannel",
                    Json::number(flow.routing.tracks_per_channel as f64)?,
                ),
                (
                    "congestedChannels",
                    Json::number(flow.routing.congested_channels.len() as f64)?,
                ),
                ("wireLength", Json::number(wire_length as f64)?),
                (
                    "drcErrors",
                    Json::number(
                        flow.drc
                            .violations
                            .iter()
                            .filter(|violation| violation.severity == eda_pnr::DrcSeverity::Error)
                            .count() as f64,
                    )?,
                ),
                (
                    "drcWarnings",
                    Json::number(
                        flow.drc
                            .violations
                            .iter()
                            .filter(|violation| violation.severity == eda_pnr::DrcSeverity::Warning)
                            .count() as f64,
                    )?,
                ),
                ("drcViolations", Json::Array(drc_violations)),
            ]),
        ),
        ("verilog", Json::string(flow.verilog.clone())),
        ("lef", Json::string(flow.lef.clone())),
        ("def", Json::string(flow.def.clone())),
        ("sdf", Json::string(flow.sdf.clone())),
    ]))
}

pub(crate) fn rtl_equiv(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let result = check_rtl_equivalence_cancellable(&source, top, cancellation)?;
    Ok(Json::object([
        ("equivalent", Json::Bool(result.equivalent)),
        (
            "counterexample",
            Json::Object(
                result
                    .counterexample
                    .into_iter()
                    .map(|(input, value)| (input, Json::string(value)))
                    .collect(),
            ),
        ),
        (
            "notes",
            Json::Array(result.notes.into_iter().map(Json::string).collect()),
        ),
        (
            "goldStats",
            Json::object([
                ("nets", Json::number(result.gold_stats.nets as f64)?),
                ("cells", Json::number(result.gold_stats.cells as f64)?),
                ("comb", Json::number(result.gold_stats.comb as f64)?),
                ("seq", Json::number(result.gold_stats.seq as f64)?),
                ("consts", Json::number(result.gold_stats.consts as f64)?),
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
    ]))
}

pub(crate) fn rtl_equiv_views(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL view equivalence cancelled".into());
    }
    let source_a = required_string(params, "sourceA")?;
    let source_b = required_string(params, "sourceB")?;
    let top_a = optional_top_field(params, "topA")?;
    let top_b = optional_top_field(params, "topB")?;
    let flow_a = run_flow_cancellable(&source_a, top_a, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL view equivalence cancelled".into());
    }
    let flow_b = run_flow_cancellable(&source_b, top_b, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL view equivalence cancelled".into());
    }
    let result = check_netlists_equivalent_cancellable(&flow_a.gold, &flow_b.gold, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL view equivalence cancelled".into());
    }

    let flow_stats = |flow: &Flow| -> Result<Json, String> {
        Ok(Json::object([
            ("nets", Json::number(flow.gold.net_count() as f64)?),
            ("cells", Json::number(flow.gold.cell_count() as f64)?),
            ("libCells", Json::number(flow.stats.lib_cells as f64)?),
        ]))
    };
    Ok(Json::object([
        ("equivalent", Json::Bool(result.equivalent)),
        (
            "counterexample",
            Json::Object(
                result
                    .counterexample
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(input, value)| (input, Json::string(value.to_bin_str())))
                    .collect(),
            ),
        ),
        (
            "notes",
            Json::Array(result.notes.into_iter().map(Json::string).collect()),
        ),
        ("viewA", flow_stats(&flow_a)?),
        ("viewB", flow_stats(&flow_b)?),
    ]))
}

pub(crate) fn rtl_sta(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let result = analyze_rtl_timing_cancellable(&source, top, cancellation)?;
    Ok(Json::object([
        (
            "criticalPathDelayNs",
            Json::number(result.critical_path_delay_ns)?,
        ),
        (
            "reg2regCriticalNs",
            Json::number(result.reg2reg_critical_ns)?,
        ),
        ("in2regCriticalNs", Json::number(result.in2reg_critical_ns)?),
        (
            "reg2regHoldCriticalNs",
            Json::number(result.reg2reg_hold_critical_ns)?,
        ),
        (
            "in2regHoldCriticalNs",
            Json::number(result.in2reg_hold_critical_ns)?,
        ),
        (
            "disabledTimingArcs",
            Json::number(result.disabled_timing_arcs as f64)?,
        ),
        (
            "caseAnalysisNets",
            Json::Object(
                result
                    .case_analysis_nets
                    .into_iter()
                    .map(|(net, value)| (net, Json::Bool(value)))
                    .collect(),
            ),
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
    ]))
}

fn parse_quality_limit(value: &Json) -> Result<ScopedQualityLimit, String> {
    let Some(fields) = value.as_object() else {
        return Err("quality limit entries must be objects".into());
    };
    let Some(Json::String(kind)) = fields.get("kind") else {
        return Err("quality limit entry requires a string 'kind'".into());
    };
    let kind = match kind.as_str() {
        "port" => TimingObjectKind::Port,
        "pin" => TimingObjectKind::Pin,
        "net" => TimingObjectKind::Net,
        "cell" => TimingObjectKind::Cell,
        "clock" => TimingObjectKind::Clock,
        other => {
            return Err(format!(
                "quality limit kind must be port, pin, net, cell, or clock, found '{other}'"
            ))
        }
    };
    let Some(Json::String(name)) = fields.get("name") else {
        return Err("quality limit entry requires a string 'name'".into());
    };
    let Some(limit) = fields.get("limit").and_then(Json::as_number) else {
        return Err("quality limit entry requires a numeric 'limit'".into());
    };
    Ok(ScopedQualityLimit {
        object: eda_api::TimingObjectRef::new(kind, name.clone()),
        limit: *limit,
    })
}

fn parse_quality_limits(value: Option<&Json>) -> Result<Vec<ScopedQualityLimit>, String> {
    let Some(Json::Array(values)) = value else {
        return Ok(Vec::new());
    };
    values.iter().map(parse_quality_limit).collect()
}

fn parse_quality_options(params: &Json) -> Result<QualityOptions, String> {
    let Some(fields) = params
        .as_object()
        .and_then(|fields| fields.get("qualityOptions"))
        .and_then(Json::as_object)
    else {
        return Ok(QualityOptions::default());
    };
    let mut options = QualityOptions::default();
    if let Some(value) = fields.get("inputTransitionNs") {
        let Some(value) = value.as_number() else {
            return Err("quality option 'inputTransitionNs' must be a number".into());
        };
        options.input_transition_ns = *value;
    }
    for (json_name, target) in [
        ("maxTransitionNs", &mut options.max_transition_ns),
        ("maxCapacitancePf", &mut options.max_capacitance_pf),
    ] {
        match fields.get(json_name) {
            None | Some(Json::Null) => {}
            Some(value) => {
                let Some(value) = value.as_number() else {
                    return Err(format!(
                        "quality option '{json_name}' must be a number or null"
                    ));
                };
                *target = Some(*value);
            }
        }
    }
    match fields.get("maxFanout") {
        None | Some(Json::Null) => {}
        Some(Json::Number(value)) => {
            if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
                return Err("quality option 'maxFanout' must be a positive integer".into());
            }
            options.max_fanout = Some(*value as usize);
        }
        Some(_) => return Err("quality option 'maxFanout' must be a positive integer".into()),
    }
    options.scoped_max_transition_ns = parse_quality_limits(fields.get("scopedMaxTransitionNs"))?;
    options.scoped_max_capacitance_pf = parse_quality_limits(fields.get("scopedMaxCapacitancePf"))?;
    options.scoped_max_fanout = parse_quality_limits(fields.get("scopedMaxFanout"))?;
    options.input_transitions = parse_quality_limits(fields.get("inputTransitions"))?;
    options.load_capacitances = parse_quality_limits(fields.get("loadCapacitances"))?;
    if let Some(Json::Object(values)) = fields.get("extraCapacitancePf") {
        for (net, value) in values {
            let Some(value) = value.as_number() else {
                return Err(format!(
                    "extra capacitance for net '{net}' must be a number"
                ));
            };
            options.extra_capacitance_pf.insert(net.clone(), *value);
        }
    } else if fields.contains_key("extraCapacitancePf") {
        return Err("quality option 'extraCapacitancePf' must be an object".into());
    }
    Ok(options)
}

pub(crate) fn rtl_signal_quality(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL signal-quality analysis cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let options = parse_quality_options(params)?;
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL signal-quality analysis cancelled".into());
    }
    let result = analyze_signal_quality(&flow.netlist, &options)?;
    if cancellation.is_cancelled() {
        return Err("RTL signal-quality analysis cancelled".into());
    }

    let optional_number = |value: Option<f64>| -> Result<Json, String> {
        value
            .filter(|value| value.is_finite())
            .map_or(Ok(Json::Null), Json::number)
    };
    let violations = result
        .violations
        .iter()
        .map(|violation| {
            Ok(Json::object([
                (
                    "kind",
                    Json::string(match violation.kind {
                        QualityKind::MaxTransition => "maxTransition",
                        QualityKind::MaxCapacitance => "maxCapacitance",
                        QualityKind::MaxFanout => "maxFanout",
                    }),
                ),
                ("cell", Json::from(violation.cell as i64)),
                ("net", Json::from(violation.net as i64)),
                (
                    "netName",
                    Json::string(flow.netlist.net(violation.net).name.clone()),
                ),
                ("actual", Json::number(violation.actual)?),
                ("limit", Json::number(violation.limit)?),
                ("message", Json::string(violation.message.clone())),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("hasViolations", Json::Bool(result.has_violations())),
        ("violationCount", Json::from(result.violations.len() as u64)),
        (
            "transitionsChecked",
            Json::from(result.transitions_checked as u64),
        ),
        (
            "capacitancesChecked",
            Json::from(result.capacitances_checked as u64),
        ),
        ("fanoutsChecked", Json::from(result.fanouts_checked as u64)),
        (
            "worstTransitionSlackNs",
            optional_number(result.worst_transition_slack)?,
        ),
        (
            "worstCapacitanceSlackNs",
            optional_number(result.worst_capacitance_slack)?,
        ),
        ("violations", Json::Array(violations)),
        (
            "mappedStats",
            Json::object([
                ("cells", Json::number(flow.netlist.cell_count() as f64)?),
                ("nets", Json::number(flow.netlist.net_count() as f64)?),
                ("libCells", Json::number(flow.stats.lib_cells as f64)?),
            ]),
        ),
        ("report", Json::string(signal_quality_report(&result))),
    ]))
}
