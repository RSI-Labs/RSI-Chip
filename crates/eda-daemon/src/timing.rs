//! Timing waiver, scenario, MMMC, and timing-violation RPC handlers.

use crate::{
    optional_top, required_string,
    service::{
        SharedTimingViolationRegistries, TimingViolationRegistryState,
        TIMING_WAIVER_REGISTRY_CAPACITY,
    },
    Json,
};
use eda_api::{
    active_timing_waivers_from_audit, add_timing_violation_registry_waiver_with_actor,
    analyze_timing_scenarios_from_definitions, diff_timing_scenarios as diff_api_timing_scenarios,
    multi_corner_hold_timing_path_report, multi_corner_hold_timing_paths,
    multi_corner_reset_timing_path_report, multi_corner_reset_timing_paths,
    multi_corner_setup_timing_path_report, multi_corner_setup_timing_paths,
    multi_corner_timing_report, parse_timing_scenarios, parse_timing_violation_waivers,
    parse_timing_waiver_audit, remove_timing_violation_registry_waiver_with_actor,
    run_flow_cancellable, run_flow_with_library_cancellable, timing_scenario_diff_report,
    timing_violation_registry, timing_violation_registry_report,
    timing_waiver_audit_hierarchy_report, timing_waiver_audit_hierarchy_summaries,
    timing_waiver_audit_report, update_timing_scenarios_incrementally,
    update_timing_violation_registry, write_timing_waiver_audit, CellLibrary, FlowCacheKey,
    FlowCancellation, SharedFlowCache, TimingScenario, TimingWaiverAuditAction,
};
use eda_core::netlist::Netlist;
use std::collections::BTreeMap;

fn string_array(values: impl IntoIterator<Item = String>) -> Json {
    Json::Array(values.into_iter().map(Json::string).collect())
}

fn timing_pin_label(pin: &eda_api::TimingPin) -> String {
    format!("{}/{}", pin.cell, pin.pin)
}

fn timing_constraint_diff_json(diff: &eda_api::TimingConstraintDiff) -> Json {
    Json::object([
        (
            "changedClocks",
            string_array(diff.changed_clocks.iter().cloned()),
        ),
        (
            "changedInputPorts",
            string_array(diff.changed_input_ports.iter().cloned()),
        ),
        (
            "changedOutputPorts",
            string_array(diff.changed_output_ports.iter().cloned()),
        ),
        (
            "changedCellDerates",
            string_array(diff.changed_cell_derates.iter().cloned()),
        ),
        (
            "changedPinDerates",
            string_array(diff.changed_pin_derates.iter().map(timing_pin_label)),
        ),
        (
            "scopedTimingDeratesChanged",
            Json::Bool(diff.scoped_timing_derates_changed),
        ),
        (
            "changedPortDerates",
            string_array(diff.changed_port_derates.iter().cloned()),
        ),
        (
            "changedCellCheckDerates",
            string_array(diff.changed_cell_check_derates.iter().cloned()),
        ),
        (
            "changedPinCheckDerates",
            string_array(diff.changed_pin_check_derates.iter().map(timing_pin_label)),
        ),
        (
            "changedClockUncertainty",
            string_array(diff.changed_clock_uncertainty.iter().cloned()),
        ),
        (
            "changedHoldUncertainty",
            string_array(diff.changed_hold_uncertainty.iter().cloned()),
        ),
        (
            "timingExceptionsChanged",
            Json::Bool(diff.timing_exceptions_changed),
        ),
        ("clockGroupsChanged", Json::Bool(diff.clock_groups_changed)),
        ("delayLimitsChanged", Json::Bool(diff.delay_limits_changed)),
        (
            "disabledTimingChanged",
            Json::Bool(diff.disabled_timing_changed),
        ),
        (
            "caseAnalysisChanged",
            Json::Bool(diff.case_analysis_changed),
        ),
        (
            "globalTimingDerateChanged",
            Json::Bool(diff.global_timing_derate_changed),
        ),
        (
            "globalCheckDerateChanged",
            Json::Bool(diff.global_check_derate_changed),
        ),
        ("designRulesChanged", Json::Bool(diff.design_rules_changed)),
        (
            "inputTransitionsChanged",
            Json::Bool(diff.input_transitions_changed),
        ),
        ("loadsChanged", Json::Bool(diff.loads_changed)),
        (
            "fullBaseRecomputeRequired",
            Json::Bool(diff.full_base_recompute_required),
        ),
        (
            "baseRecomputeRequired",
            Json::Bool(diff.base_recompute_required),
        ),
        (
            "constrainedRecomputeRequired",
            Json::Bool(diff.constrained_recompute_required),
        ),
        (
            "qualityRecomputeRequired",
            Json::Bool(diff.quality_recompute_required),
        ),
    ])
}

pub(crate) fn timing_scenario_diff(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing scenario diff cancelled".into());
    }
    let old_source = required_string(params, "old")?;
    let new_source = required_string(params, "new")?;
    let old = parse_timing_scenarios(&old_source)?;
    let new = parse_timing_scenarios(&new_source)?;
    if cancellation.is_cancelled() {
        return Err("timing scenario diff cancelled".into());
    }
    let diff = diff_api_timing_scenarios(&old, &new)?;
    if cancellation.is_cancelled() {
        return Err("timing scenario diff cancelled".into());
    }

    let properties = diff
        .properties
        .iter()
        .map(|(scenario, property)| {
            let changed = property
                .changed
                .iter()
                .map(|(key, (old, new))| {
                    (
                        key.clone(),
                        Json::object([
                            ("old", Json::string(old.clone())),
                            ("new", Json::string(new.clone())),
                        ]),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            (
                scenario.clone(),
                Json::object([
                    ("added", string_array(property.added.iter().cloned())),
                    ("removed", string_array(property.removed.iter().cloned())),
                    ("changed", Json::Object(changed)),
                ]),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let constraints = diff
        .constraints
        .iter()
        .map(|(scenario, constraint)| (scenario.clone(), timing_constraint_diff_json(constraint)))
        .collect::<BTreeMap<_, _>>();

    Ok(Json::object([
        ("oldScenarioCount", Json::from(old.len() as u64)),
        ("newScenarioCount", Json::from(new.len() as u64)),
        ("added", string_array(diff.added.iter().cloned())),
        ("removed", string_array(diff.removed.iter().cloned())),
        ("changed", string_array(diff.changed.iter().cloned())),
        (
            "timingChanged",
            string_array(diff.timing_changed.iter().cloned()),
        ),
        ("constraints", Json::Object(constraints)),
        ("properties", Json::Object(properties)),
        ("report", Json::string(timing_scenario_diff_report(&diff))),
    ]))
}

fn timing_waiver_json(waiver: &eda_api::TimingViolationWaiver) -> Result<Json, String> {
    Ok(Json::object([
        ("id", Json::string(waiver.id.clone())),
        (
            "scenarioPattern",
            Json::string(waiver.scenario_pattern.clone()),
        ),
        (
            "endpointPattern",
            Json::string(waiver.endpoint_pattern.clone()),
        ),
        (
            "instancePattern",
            waiver
                .instance_pattern
                .clone()
                .map_or(Json::Null, Json::string),
        ),
        (
            "kind",
            waiver
                .kind
                .map(|kind| Json::string(kind.label().to_string()))
                .unwrap_or(Json::Null),
        ),
        ("slackFloorNs", Json::number(waiver.slack_floor_ns)?),
        (
            "owner",
            waiver.owner.clone().map_or(Json::Null, Json::string),
        ),
        ("note", waiver.note.clone().map_or(Json::Null, Json::string)),
        (
            "expiresAfterGeneration",
            waiver
                .expires_after_generation
                .map(Json::from)
                .unwrap_or(Json::Null),
        ),
    ]))
}

pub(crate) fn timing_waiver_audit(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing waiver audit cancelled".into());
    }
    let source = required_string(params, "audit")?;
    let entries = parse_timing_waiver_audit(&source)?;
    if cancellation.is_cancelled() {
        return Err("timing waiver audit cancelled".into());
    }
    let active = active_timing_waivers_from_audit(&entries)?;
    let summaries = timing_waiver_audit_hierarchy_summaries(&entries)?;
    if cancellation.is_cancelled() {
        return Err("timing waiver audit cancelled".into());
    }

    let events = entries
        .iter()
        .map(|entry| {
            Ok(Json::object([
                ("sequence", Json::from(entry.sequence)),
                (
                    "action",
                    Json::string(match entry.action {
                        TimingWaiverAuditAction::Added => "added",
                        TimingWaiverAuditAction::Removed => "removed",
                    }),
                ),
                ("waiverId", Json::string(entry.waiver_id.clone())),
                ("generation", Json::from(entry.generation)),
                (
                    "changedBy",
                    entry.changed_by.clone().map_or(Json::Null, Json::string),
                ),
                ("waiver", timing_waiver_json(&entry.waiver)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let active_waivers = active
        .iter()
        .map(timing_waiver_json)
        .collect::<Result<Vec<_>, String>>()?;
    let hierarchy = summaries
        .iter()
        .map(|(instance, summary)| {
            let key = instance.clone().unwrap_or_else(|| "(unscoped)".to_string());
            (
                key,
                Json::object([
                    (
                        "instancePattern",
                        summary
                            .instance_pattern
                            .clone()
                            .map_or(Json::Null, Json::string),
                    ),
                    ("events", Json::from(summary.events as u64)),
                    ("addedEvents", Json::from(summary.added_events as u64)),
                    ("removedEvents", Json::from(summary.removed_events as u64)),
                    (
                        "activeWaiverIds",
                        string_array(summary.active_waiver_ids.iter().cloned()),
                    ),
                    ("latestGeneration", Json::from(summary.latest_generation)),
                ]),
            )
        })
        .collect::<BTreeMap<_, _>>();

    Ok(Json::object([
        ("eventCount", Json::from(entries.len() as u64)),
        ("activeWaiverCount", Json::from(active.len() as u64)),
        ("events", Json::Array(events)),
        ("activeWaivers", Json::Array(active_waivers)),
        ("hierarchy", Json::Object(hierarchy)),
        ("report", Json::string(timing_waiver_audit_report(&entries))),
        (
            "hierarchyReport",
            Json::string(timing_waiver_audit_hierarchy_report(&entries)?),
        ),
    ]))
}

fn timing_violation_json(violation: &eda_api::TimingViolation) -> Result<Json, String> {
    Ok(Json::object([
        ("id", Json::string(violation.id.clone())),
        ("scenario", Json::string(violation.scenario.clone())),
        ("kind", Json::string(violation.kind.label().to_string())),
        ("endpoint", Json::string(violation.endpoint.clone())),
        ("launch", Json::string(violation.launch.clone())),
        ("slackNs", Json::number(violation.slack_ns)?),
        ("arrivalNs", Json::number(violation.arrival_ns)?),
        ("requiredNs", Json::number(violation.required_ns)?),
        ("cells", string_array(violation.cells.iter().cloned())),
        ("nets", string_array(violation.nets.iter().cloned())),
        (
            "hierarchy",
            violation.hierarchy.clone().map_or(Json::Null, Json::string),
        ),
        (
            "hierarchyInstances",
            string_array(violation.hierarchy_instances.iter().cloned()),
        ),
    ]))
}

fn timing_violation_store_json(store: &eda_api::TimingViolationStore) -> Result<Json, String> {
    let violations = store
        .active_violations()
        .values()
        .map(timing_violation_json)
        .collect::<Result<Vec<_>, String>>()?;
    let waived = store
        .waived_violations()
        .iter()
        .map(|(violation, waiver)| {
            (
                violation.clone(),
                Json::object([
                    ("violation", Json::string(violation.clone())),
                    ("waiver", Json::string(waiver.clone())),
                ]),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let waivers = store
        .waivers()
        .values()
        .map(timing_waiver_json)
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("scenario", Json::string(store.scenario.clone())),
        ("generation", Json::from(store.generation)),
        ("activeViolations", Json::Array(violations)),
        ("activeViolationCount", Json::from(store.len() as u64)),
        ("waivedViolations", Json::Object(waived)),
        (
            "waivedViolationCount",
            Json::from(store.waived_len() as u64),
        ),
        ("waivers", Json::Array(waivers)),
        (
            "waiverAuditReport",
            Json::string(timing_waiver_audit_report(store.audit())),
        ),
    ]))
}

fn cross_corner_hierarchy_summary_json(
    summary: &eda_api::CrossCornerHierarchyViolationSummary,
) -> Result<Json, String> {
    let kinds = summary
        .kinds
        .iter()
        .map(|(kind, count)| (kind.label().to_string(), Json::from(*count as u64)))
        .collect::<BTreeMap<_, _>>();
    Ok(Json::object([
        (
            "hierarchy",
            summary.hierarchy.clone().map_or(Json::Null, Json::string),
        ),
        ("violations", Json::from(summary.violations as u64)),
        ("worstSlackNs", Json::number(summary.worst_slack_ns)?),
        (
            "worstScenario",
            Json::string(summary.worst_scenario.clone()),
        ),
        ("kinds", Json::Object(kinds)),
    ]))
}

fn cross_corner_timing_violation_json(
    summary: &eda_api::CrossCornerTimingViolation,
) -> Result<Json, String> {
    let scenarios = summary
        .scenarios
        .iter()
        .map(|(scenario, violation)| {
            Ok((
                scenario.clone(),
                Json::object([
                    ("id", Json::string(violation.id.clone())),
                    ("slackNs", Json::number(violation.slack_ns)?),
                ]),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let hierarchy = summary
        .hierarchy_summary
        .iter()
        .map(|(owner, values)| {
            Ok((
                owner.clone().unwrap_or_else(|| "(top)".to_string()),
                cross_corner_hierarchy_summary_json(values)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    Ok(Json::object([
        ("key", Json::string(summary.key.clone())),
        ("kind", Json::string(summary.kind.label().to_string())),
        ("endpoint", Json::string(summary.endpoint.clone())),
        ("scenarios", Json::Object(scenarios)),
        ("hierarchy", Json::Object(hierarchy)),
        (
            "worstScenario",
            Json::string(summary.worst_scenario.clone()),
        ),
        ("worstSlackNs", Json::number(summary.worst_slack_ns)?),
    ]))
}

fn timing_violation_registry_json(
    registry: &eda_api::TimingViolationRegistry,
) -> Result<Json, String> {
    let scenarios = registry
        .stores()
        .iter()
        .map(|(name, store)| Ok((name.clone(), timing_violation_store_json(store)?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let summaries = registry
        .summaries()
        .values()
        .map(cross_corner_timing_violation_json)
        .collect::<Result<Vec<_>, String>>()?;
    let hierarchy_summaries = registry
        .hierarchy_summaries()
        .into_iter()
        .map(|(owner, summary)| {
            Ok((
                owner.unwrap_or_else(|| "(top)".to_string()),
                cross_corner_hierarchy_summary_json(&summary)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    Ok(Json::object([
        ("scenarioCount", Json::from(registry.stores().len() as u64)),
        ("generation", Json::from(registry.generation())),
        ("activeViolationCount", Json::from(registry.len() as u64)),
        ("hasActiveViolations", Json::Bool(!registry.is_empty())),
        ("scenarios", Json::Object(scenarios)),
        ("summaries", Json::Array(summaries)),
        ("hierarchySummaries", Json::Object(hierarchy_summaries)),
        (
            "report",
            Json::string(timing_violation_registry_report(registry, None)),
        ),
    ]))
}

fn timing_waiver_registry_response(
    registry_id: &str,
    state: &TimingViolationRegistryState,
    update: Option<Json>,
    cache_hit: Option<bool>,
) -> Result<Json, String> {
    let mut fields = timing_violation_registry_json(&state.registry)?
        .as_object()
        .cloned()
        .ok_or_else(|| "timing violation registry serialization failed".to_string())?;
    fields.insert("registryId".into(), Json::string(registry_id));
    fields.insert(
        "scenarioNames".into(),
        Json::Array(
            state
                .definitions
                .iter()
                .map(|definition| Json::string(definition.name.clone()))
                .collect(),
        ),
    );
    fields.insert(
        "activeWaiverIds".into(),
        Json::Array(
            state
                .registry
                .waivers()
                .keys()
                .map(|id| Json::string(id.clone()))
                .collect(),
        ),
    );
    fields.insert(
        "audit".into(),
        Json::string(write_timing_waiver_audit(state.registry.audit())?),
    );
    if let Some(update) = update {
        fields.insert("update".into(), update);
    }
    if let Some(cache_hit) = cache_hit {
        fields.insert("cacheHit".into(), Json::Bool(cache_hit));
    }
    fields.insert(
        "mappedStats".into(),
        Json::object([
            (
                "cells",
                Json::number(state.flow.netlist.cell_count() as f64)?,
            ),
            ("nets", Json::number(state.flow.netlist.net_count() as f64)?),
            ("libCells", Json::number(state.flow.stats.lib_cells as f64)?),
        ]),
    );
    Ok(Json::Object(fields))
}

pub(crate) fn timing_waiver_registry_create(
    params: &Json,
    cancellation: &FlowCancellation,
    flow_cache: &SharedFlowCache,
    registries: &SharedTimingViolationRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing waiver registry creation cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    if registry_id.trim().is_empty() {
        return Err("timing waiver registry ID cannot be empty".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let scenario_source = required_string(params, "scenarios")?;
    let definitions = parse_timing_scenarios(&scenario_source)?;
    if definitions.is_empty() {
        return Err("timing waiver registry requires at least one scenario".into());
    }
    {
        let entries = registries
            .lock()
            .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
        if entries.contains_key(&registry_id) {
            return Err(format!(
                "timing waiver registry '{registry_id}' already exists"
            ));
        }
        if entries.len() >= TIMING_WAIVER_REGISTRY_CAPACITY {
            return Err(format!(
                "timing waiver registry store is full ({TIMING_WAIVER_REGISTRY_CAPACITY} entries)"
            ));
        }
    }

    let library = CellLibrary::std();
    let key = FlowCacheKey::new(&source, top, &library);
    let cached = flow_cache.get_or_insert_with(&key, || {
        run_flow_with_library_cancellable(&source, top, &library, cancellation)
    })?;
    if cancellation.is_cancelled() {
        return Err("timing waiver registry creation cancelled".into());
    }
    let scenarios = definitions
        .iter()
        .map(|definition| TimingScenario {
            name: definition.name.as_str(),
            netlist: &cached.flow.netlist,
            constraints: &definition.constraints,
        })
        .collect::<Vec<_>>();
    let registry = timing_violation_registry(scenarios.iter().copied())?;
    let state = TimingViolationRegistryState {
        flow: cached.flow,
        definitions,
        registry,
    };
    {
        let mut entries = registries
            .lock()
            .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
        if entries.contains_key(&registry_id) {
            return Err(format!(
                "timing waiver registry '{registry_id}' already exists"
            ));
        }
        if entries.len() >= TIMING_WAIVER_REGISTRY_CAPACITY {
            return Err(format!(
                "timing waiver registry store is full ({TIMING_WAIVER_REGISTRY_CAPACITY} entries)"
            ));
        }
        entries.insert(registry_id.clone(), state);
    }
    let entries = registries
        .lock()
        .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get(&registry_id)
        .ok_or_else(|| "internal error: timing waiver registry vanished".to_string())?;
    timing_waiver_registry_response(&registry_id, state, None, Some(cached.cache_hit))
}

pub(crate) fn timing_waiver_registry_get(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedTimingViolationRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing waiver registry lookup cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let entries = registries
        .lock()
        .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get(&registry_id)
        .ok_or_else(|| format!("timing waiver registry '{registry_id}' does not exist"))?;
    timing_waiver_registry_response(&registry_id, state, None, None)
}

pub(crate) fn timing_waiver_registry_list(
    cancellation: &FlowCancellation,
    registries: &SharedTimingViolationRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing waiver registry listing cancelled".into());
    }
    let entries = registries
        .lock()
        .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
    let registry_summaries = entries
        .iter()
        .map(|(id, state)| {
            Ok(Json::object([
                ("registryId", Json::string(id.clone())),
                ("generation", Json::from(state.registry.generation())),
                ("scenarioCount", Json::from(state.definitions.len() as u64)),
                (
                    "activeViolationCount",
                    Json::from(state.registry.len() as u64),
                ),
                (
                    "activeWaiverCount",
                    Json::from(state.registry.waivers().len() as u64),
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("count", Json::from(entries.len() as u64)),
        (
            "capacity",
            Json::from(TIMING_WAIVER_REGISTRY_CAPACITY as u64),
        ),
        ("registries", Json::Array(registry_summaries)),
    ]))
}

pub(crate) fn timing_waiver_registry_add_waivers(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedTimingViolationRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing waiver registry update cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let waiver_source = required_string(params, "waivers")?;
    let waivers = parse_timing_violation_waivers(&waiver_source)?;
    if waivers.is_empty() {
        return Err("timing waiver registry update requires at least one waiver".into());
    }
    let actor = match params.as_object().and_then(|fields| fields.get("actor")) {
        None | Some(Json::Null) => None,
        Some(Json::String(actor)) => Some(actor.clone()),
        Some(_) => return Err("RPC parameter 'actor' must be a string".into()),
    };
    let mut entries = registries
        .lock()
        .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get_mut(&registry_id)
        .ok_or_else(|| format!("timing waiver registry '{registry_id}' does not exist"))?;
    for waiver in waivers {
        add_timing_violation_registry_waiver_with_actor(
            &mut state.registry,
            waiver,
            actor.as_deref(),
        )?;
    }
    timing_waiver_registry_response(&registry_id, state, None, None)
}

pub(crate) fn timing_waiver_registry_remove_waiver(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedTimingViolationRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing waiver registry update cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let waiver_id = required_string(params, "waiverId")?;
    let actor = match params.as_object().and_then(|fields| fields.get("actor")) {
        None | Some(Json::Null) => None,
        Some(Json::String(actor)) => Some(actor.clone()),
        Some(_) => return Err("RPC parameter 'actor' must be a string".into()),
    };
    let mut entries = registries
        .lock()
        .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get_mut(&registry_id)
        .ok_or_else(|| format!("timing waiver registry '{registry_id}' does not exist"))?;
    remove_timing_violation_registry_waiver_with_actor(
        &mut state.registry,
        &waiver_id,
        actor.as_deref(),
    )?;
    timing_waiver_registry_response(&registry_id, state, None, None)
}

pub(crate) fn timing_waiver_registry_update_scenarios(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedTimingViolationRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing waiver registry scenario update cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let scenario_source = required_string(params, "scenarios")?;
    let definitions = parse_timing_scenarios(&scenario_source)?;
    if definitions.is_empty() {
        return Err("timing waiver registry requires at least one scenario".into());
    }
    let mut entries = registries
        .lock()
        .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get_mut(&registry_id)
        .ok_or_else(|| format!("timing waiver registry '{registry_id}' does not exist"))?;
    let scenarios = definitions
        .iter()
        .map(|definition| TimingScenario {
            name: definition.name.as_str(),
            netlist: &state.flow.netlist,
            constraints: &definition.constraints,
        })
        .collect::<Vec<_>>();
    let update = update_timing_violation_registry(&mut state.registry, scenarios.iter().copied())?;
    if cancellation.is_cancelled() {
        return Err("timing waiver registry scenario update cancelled".into());
    }
    state.definitions = definitions;
    timing_waiver_registry_response(
        &registry_id,
        state,
        Some(timing_violation_update_json(&update)?),
        None,
    )
}

pub(crate) fn timing_waiver_registry_delete(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedTimingViolationRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("timing waiver registry deletion cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let mut entries = registries
        .lock()
        .map_err(|_| "timing waiver registry mutex poisoned".to_string())?;
    entries
        .remove(&registry_id)
        .ok_or_else(|| format!("timing waiver registry '{registry_id}' does not exist"))?;
    Ok(Json::object([
        ("registryId", Json::string(registry_id)),
        ("deleted", Json::Bool(true)),
        ("remaining", Json::from(entries.len() as u64)),
    ]))
}

fn timing_violation_change_json(change: &eda_api::TimingViolationChange) -> Result<Json, String> {
    Ok(Json::object([
        ("id", Json::string(change.id.clone())),
        ("oldSlackNs", Json::number(change.old_slack_ns)?),
        ("newSlackNs", Json::number(change.new_slack_ns)?),
        ("current", timing_violation_json(&change.current)?),
    ]))
}

fn timing_violation_diff_json(diff: &eda_api::TimingViolationDiff) -> Result<Json, String> {
    let map_violations = |values: &BTreeMap<String, eda_api::TimingViolation>| {
        values
            .values()
            .map(timing_violation_json)
            .collect::<Result<Vec<_>, String>>()
    };
    let map_changes = |values: &BTreeMap<String, eda_api::TimingViolationChange>| {
        values
            .values()
            .map(timing_violation_change_json)
            .collect::<Result<Vec<_>, String>>()
    };
    Ok(Json::object([
        ("added", Json::Array(map_violations(&diff.added)?)),
        ("removed", Json::Array(map_violations(&diff.removed)?)),
        ("improved", Json::Array(map_changes(&diff.improved)?)),
        ("degraded", Json::Array(map_changes(&diff.degraded)?)),
        ("unchanged", Json::Array(map_violations(&diff.unchanged)?)),
        ("addedCount", Json::from(diff.added.len() as u64)),
        ("removedCount", Json::from(diff.removed.len() as u64)),
        ("improvedCount", Json::from(diff.improved.len() as u64)),
        ("degradedCount", Json::from(diff.degraded.len() as u64)),
        ("unchangedCount", Json::from(diff.unchanged.len() as u64)),
    ]))
}

fn cross_corner_timing_violation_change_json(
    change: &eda_api::CrossCornerTimingViolationChange,
) -> Result<Json, String> {
    Ok(Json::object([
        ("key", Json::string(change.current.key.clone())),
        (
            "oldWorstScenario",
            Json::string(change.old_worst_scenario.clone()),
        ),
        ("oldWorstSlackNs", Json::number(change.old_worst_slack_ns)?),
        (
            "current",
            cross_corner_timing_violation_json(&change.current)?,
        ),
    ]))
}

fn cross_corner_timing_violation_diff_json(
    diff: &eda_api::CrossCornerTimingViolationDiff,
) -> Result<Json, String> {
    let map_summaries = |values: &BTreeMap<String, eda_api::CrossCornerTimingViolation>| {
        values
            .values()
            .map(cross_corner_timing_violation_json)
            .collect::<Result<Vec<_>, String>>()
    };
    let map_changes = |values: &BTreeMap<String, eda_api::CrossCornerTimingViolationChange>| {
        values
            .values()
            .map(cross_corner_timing_violation_change_json)
            .collect::<Result<Vec<_>, String>>()
    };
    Ok(Json::object([
        ("added", Json::Array(map_summaries(&diff.added)?)),
        ("removed", Json::Array(map_summaries(&diff.removed)?)),
        ("improved", Json::Array(map_changes(&diff.improved)?)),
        ("degraded", Json::Array(map_changes(&diff.degraded)?)),
        ("unchanged", Json::Array(map_summaries(&diff.unchanged)?)),
        ("addedCount", Json::from(diff.added.len() as u64)),
        ("removedCount", Json::from(diff.removed.len() as u64)),
        ("improvedCount", Json::from(diff.improved.len() as u64)),
        ("degradedCount", Json::from(diff.degraded.len() as u64)),
        ("unchangedCount", Json::from(diff.unchanged.len() as u64)),
    ]))
}

fn timing_violation_update_json(
    update: &eda_api::TimingViolationRegistryUpdate,
) -> Result<Json, String> {
    let added = update
        .added_scenarios
        .iter()
        .map(|(name, store)| {
            Ok(Json::object([
                ("name", Json::string(name.clone())),
                ("store", timing_violation_store_json(store)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let removed = update
        .removed_scenarios
        .iter()
        .map(|(name, store)| {
            Ok(Json::object([
                ("name", Json::string(name.clone())),
                ("store", timing_violation_store_json(store)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let scenario_diffs = update
        .scenario_diffs
        .iter()
        .map(|(name, diff)| Ok((name.clone(), timing_violation_diff_json(diff)?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    Ok(Json::object([
        ("addedScenarios", Json::Array(added)),
        ("removedScenarios", Json::Array(removed)),
        ("scenarioDiffs", Json::Object(scenario_diffs)),
        (
            "crossCorner",
            cross_corner_timing_violation_diff_json(&update.cross_corner)?,
        ),
        (
            "addedScenarioCount",
            Json::from(update.added_scenarios.len() as u64),
        ),
        (
            "removedScenarioCount",
            Json::from(update.removed_scenarios.len() as u64),
        ),
        (
            "scenarioDiffCount",
            Json::from(update.scenario_diffs.len() as u64),
        ),
    ]))
}

pub(crate) fn rtl_incremental_mmmc_timing_violations(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL incremental MMMC timing violations cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let old_source = required_string(params, "old")?;
    let new_source = required_string(params, "new")?;
    let old = parse_timing_scenarios(&old_source)?;
    let new = parse_timing_scenarios(&new_source)?;
    if old.is_empty() || new.is_empty() {
        return Err("RTL incremental MMMC timing violations require old and new scenarios".into());
    }
    let waivers = match params.as_object().and_then(|fields| fields.get("waivers")) {
        None | Some(Json::Null) => Vec::new(),
        Some(Json::String(source)) => parse_timing_violation_waivers(source)?,
        Some(_) => return Err("RPC parameter 'waivers' must be a string".into()),
    };
    let actor = match params.as_object().and_then(|fields| fields.get("actor")) {
        None | Some(Json::Null) => None,
        Some(Json::String(actor)) => Some(actor.clone()),
        Some(_) => return Err("RPC parameter 'actor' must be a string".into()),
    };

    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL incremental MMMC timing violations cancelled".into());
    }
    let old_scenarios = old
        .iter()
        .map(|definition| TimingScenario {
            name: definition.name.as_str(),
            netlist: &flow.netlist,
            constraints: &definition.constraints,
        })
        .collect::<Vec<_>>();
    let mut registry = timing_violation_registry(old_scenarios.iter().copied())?;
    if cancellation.is_cancelled() {
        return Err("RTL incremental MMMC timing violations cancelled".into());
    }
    for waiver in waivers {
        add_timing_violation_registry_waiver_with_actor(&mut registry, waiver, actor.as_deref())?;
        if cancellation.is_cancelled() {
            return Err("RTL incremental MMMC timing violations cancelled".into());
        }
    }
    let baseline_report = timing_violation_registry_report(&registry, None);

    let new_scenarios = new
        .iter()
        .map(|definition| TimingScenario {
            name: definition.name.as_str(),
            netlist: &flow.netlist,
            constraints: &definition.constraints,
        })
        .collect::<Vec<_>>();
    let update = update_timing_violation_registry(&mut registry, new_scenarios.iter().copied())?;
    if cancellation.is_cancelled() {
        return Err("RTL incremental MMMC timing violations cancelled".into());
    }
    let update_report = timing_violation_registry_report(&registry, Some(&update));
    let mut fields = timing_violation_registry_json(&registry)?
        .as_object()
        .cloned()
        .ok_or_else(|| "timing violation registry serialization failed".to_string())?;
    fields.insert("oldScenarioCount".into(), Json::from(old.len() as u64));
    fields.insert("newScenarioCount".into(), Json::from(new.len() as u64));
    fields.insert("baselineReport".into(), Json::string(baseline_report));
    fields.insert("update".into(), timing_violation_update_json(&update)?);
    fields.insert("updateReport".into(), Json::string(update_report));
    fields.insert(
        "mappedStats".into(),
        Json::object([
            ("cells", Json::number(flow.netlist.cell_count() as f64)?),
            ("nets", Json::number(flow.netlist.net_count() as f64)?),
            ("libCells", Json::number(flow.stats.lib_cells as f64)?),
        ]),
    );
    Ok(Json::Object(fields))
}

pub(crate) fn rtl_mmmc_timing_violations(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing violations cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let scenario_source = required_string(params, "scenarios")?;
    let definitions = parse_timing_scenarios(&scenario_source)?;
    if definitions.is_empty() {
        return Err("RTL MMMC timing violations require at least one scenario".into());
    }
    let waivers = match params.as_object().and_then(|fields| fields.get("waivers")) {
        None | Some(Json::Null) => Vec::new(),
        Some(Json::String(source)) => parse_timing_violation_waivers(source)?,
        Some(_) => return Err("RPC parameter 'waivers' must be a string".into()),
    };
    let actor = match params.as_object().and_then(|fields| fields.get("actor")) {
        None | Some(Json::Null) => None,
        Some(Json::String(actor)) => Some(actor.clone()),
        Some(_) => return Err("RPC parameter 'actor' must be a string".into()),
    };

    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing violations cancelled".into());
    }
    let scenarios = definitions
        .iter()
        .map(|definition| TimingScenario {
            name: definition.name.as_str(),
            netlist: &flow.netlist,
            constraints: &definition.constraints,
        })
        .collect::<Vec<_>>();
    let mut registry = timing_violation_registry(scenarios.iter().copied())?;
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing violations cancelled".into());
    }
    for waiver in waivers {
        add_timing_violation_registry_waiver_with_actor(&mut registry, waiver, actor.as_deref())?;
        if cancellation.is_cancelled() {
            return Err("RTL MMMC timing violations cancelled".into());
        }
    }

    let scenario_results = registry
        .stores()
        .iter()
        .map(|(name, store)| {
            let violations = store
                .active_violations()
                .values()
                .map(timing_violation_json)
                .collect::<Result<Vec<_>, String>>()?;
            let waived = store
                .waived_violations()
                .iter()
                .map(|(violation, waiver)| {
                    (
                        violation.clone(),
                        Json::object([
                            ("violation", Json::string(violation.clone())),
                            ("waiver", Json::string(waiver.clone())),
                        ]),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let waivers = store
                .waivers()
                .values()
                .map(timing_waiver_json)
                .collect::<Result<Vec<_>, String>>()?;
            Ok((
                name.clone(),
                Json::object([
                    ("scenario", Json::string(store.scenario.clone())),
                    ("generation", Json::from(store.generation)),
                    ("activeViolations", Json::Array(violations)),
                    ("activeViolationCount", Json::from(store.len() as u64)),
                    ("waivedViolations", Json::Object(waived)),
                    (
                        "waivedViolationCount",
                        Json::from(store.waived_len() as u64),
                    ),
                    ("waivers", Json::Array(waivers)),
                    (
                        "waiverAuditReport",
                        Json::string(timing_waiver_audit_report(store.audit())),
                    ),
                ]),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;

    let summaries = registry
        .summaries()
        .values()
        .map(|summary| {
            let scenarios = summary
                .scenarios
                .iter()
                .map(|(scenario, violation)| -> Result<(String, Json), String> {
                    Ok((
                        scenario.clone(),
                        Json::object([
                            ("id", Json::string(violation.id.clone())),
                            ("slackNs", Json::number(violation.slack_ns)?),
                        ]),
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()?;
            let hierarchy = summary
                .hierarchy_summary
                .iter()
                .map(|(owner, values)| -> Result<(String, Json), String> {
                    let key = owner.clone().unwrap_or_else(|| "(top)".to_string());
                    let kinds = values
                        .kinds
                        .iter()
                        .map(|(kind, count)| (kind.label().to_string(), Json::from(*count as u64)))
                        .collect::<BTreeMap<_, _>>();
                    Ok((
                        key,
                        Json::object([
                            (
                                "hierarchy",
                                values.hierarchy.clone().map_or(Json::Null, Json::string),
                            ),
                            ("violations", Json::from(values.violations as u64)),
                            ("worstSlackNs", Json::number(values.worst_slack_ns)?),
                            ("worstScenario", Json::string(values.worst_scenario.clone())),
                            ("kinds", Json::Object(kinds)),
                        ]),
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()?;
            Ok(Json::object([
                ("key", Json::string(summary.key.clone())),
                ("kind", Json::string(summary.kind.label().to_string())),
                ("endpoint", Json::string(summary.endpoint.clone())),
                ("scenarios", Json::Object(scenarios)),
                ("hierarchy", Json::Object(hierarchy)),
                (
                    "worstScenario",
                    Json::string(summary.worst_scenario.clone()),
                ),
                ("worstSlackNs", Json::number(summary.worst_slack_ns)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let hierarchy_summaries = registry
        .hierarchy_summaries()
        .into_iter()
        .map(|(owner, summary)| -> Result<(String, Json), String> {
            let key = owner.unwrap_or_else(|| "(top)".to_string());
            let kinds = summary
                .kinds
                .iter()
                .map(|(kind, count)| (kind.label().to_string(), Json::from(*count as u64)))
                .collect::<BTreeMap<_, _>>();
            Ok((
                key,
                Json::object([
                    (
                        "hierarchy",
                        summary.hierarchy.clone().map_or(Json::Null, Json::string),
                    ),
                    ("violations", Json::from(summary.violations as u64)),
                    ("worstSlackNs", Json::number(summary.worst_slack_ns)?),
                    (
                        "worstScenario",
                        Json::string(summary.worst_scenario.clone()),
                    ),
                    ("kinds", Json::Object(kinds)),
                ]),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;

    Ok(Json::object([
        ("scenarioCount", Json::from(registry.stores().len() as u64)),
        ("generation", Json::from(registry.generation())),
        ("activeViolationCount", Json::from(registry.len() as u64)),
        ("hasActiveViolations", Json::Bool(!registry.is_empty())),
        ("scenarios", Json::Object(scenario_results)),
        ("summaries", Json::Array(summaries)),
        ("hierarchySummaries", Json::Object(hierarchy_summaries)),
        (
            "report",
            Json::string(timing_violation_registry_report(&registry, None)),
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

pub(crate) fn rtl_mmmc_timing(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing analysis cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let scenario_source = required_string(params, "scenarios")?;
    let definitions = parse_timing_scenarios(&scenario_source)?;
    if definitions.is_empty() {
        return Err("RTL MMMC timing requires at least one scenario".into());
    }
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing analysis cancelled".into());
    }
    let result = analyze_timing_scenarios_from_definitions(&flow.netlist, &definitions)?;
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing analysis cancelled".into());
    }

    let optional_number = |value: Option<f64>| -> Result<Json, String> {
        value.map_or(Ok(Json::Null), |value| {
            if value.is_finite() {
                Json::number(value)
            } else {
                Ok(Json::Null)
            }
        })
    };
    let corners = result
        .corners
        .iter()
        .map(|corner| {
            Ok(Json::object([
                ("scenario", Json::string(corner.scenario.clone())),
                ("clock", Json::string(corner.timing.clock.clone())),
                (
                    "criticalPathDelayNs",
                    optional_number(Some(corner.timing.base.critical_path_delay))?,
                ),
                ("periodNs", optional_number(Some(corner.timing.period))?),
                (
                    "worstSlackNs",
                    optional_number(Some(corner.timing.worst_slack))?,
                ),
                (
                    "setupSlackNs",
                    optional_number(Some(corner.timing.reg2reg_slack))?,
                ),
                (
                    "holdSlackNs",
                    optional_number(Some(corner.timing.reg2reg_hold_slack))?,
                ),
                (
                    "outputSlackNs",
                    optional_number(Some(corner.timing.output_slack))?,
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let scenario_properties = definitions
        .iter()
        .map(|definition| {
            (
                definition.name.clone(),
                Json::Object(
                    definition
                        .properties
                        .iter()
                        .map(|(key, value)| (key.clone(), Json::string(value.clone())))
                        .collect(),
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();

    Ok(Json::object([
        ("scenarioCount", Json::number(result.corners.len() as f64)?),
        (
            "worstScenario",
            result.worst_scenario().map_or(Json::Null, Json::string),
        ),
        (
            "worstSetupSlackNs",
            optional_number(result.worst_setup_slack())?,
        ),
        (
            "worstHoldSlackNs",
            optional_number(result.worst_hold_slack())?,
        ),
        ("corners", Json::Array(corners)),
        ("properties", Json::Object(scenario_properties)),
        (
            "mappedStats",
            Json::object([
                ("cells", Json::number(flow.netlist.cell_count() as f64)?),
                ("nets", Json::number(flow.netlist.net_count() as f64)?),
                ("libCells", Json::number(flow.stats.lib_cells as f64)?),
            ]),
        ),
        ("report", Json::string(multi_corner_timing_report(&result))),
    ]))
}

pub(crate) fn rtl_incremental_mmmc_timing(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL incremental MMMC timing cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let old_source = required_string(params, "old")?;
    let new_source = required_string(params, "new")?;
    let old = parse_timing_scenarios(&old_source)?;
    let new = parse_timing_scenarios(&new_source)?;
    if old.is_empty() || new.is_empty() {
        return Err("RTL incremental MMMC timing requires old and new scenarios".into());
    }
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL incremental MMMC timing cancelled".into());
    }
    let previous = analyze_timing_scenarios_from_definitions(&flow.netlist, &old)?;
    if cancellation.is_cancelled() {
        return Err("RTL incremental MMMC timing cancelled".into());
    }
    let incremental = update_timing_scenarios_incrementally(&flow.netlist, &previous, &old, &new)?;
    if cancellation.is_cancelled() {
        return Err("RTL incremental MMMC timing cancelled".into());
    }
    let diff = &incremental.diff;

    let finite_number = |value: f64| -> Result<Json, String> {
        if value.is_finite() {
            Json::number(value)
        } else {
            Ok(Json::Null)
        }
    };
    let optional_number = |value: Option<f64>| -> Result<Json, String> {
        value.map_or(Ok(Json::Null), finite_number)
    };
    let updates = incremental
        .updates
        .iter()
        .map(|(scenario, update)| {
            Ok((
                scenario.clone(),
                Json::object([
                    (
                        "recomputedNets",
                        string_array(update.recomputed_nets.iter().cloned()),
                    ),
                    (
                        "recomputedCells",
                        string_array(update.recomputed_cells.iter().cloned()),
                    ),
                    (
                        "recomputedNetCount",
                        Json::from(update.recomputed_nets.len() as u64),
                    ),
                    (
                        "recomputedCellCount",
                        Json::from(update.recomputed_cells.len() as u64),
                    ),
                    ("totalCells", Json::from(update.total_cells as u64)),
                    ("fullBaseRecompute", Json::Bool(update.full_base_recompute)),
                    (
                        "criticalPathDelayNs",
                        finite_number(update.timing.critical_path_delay)?,
                    ),
                    (
                        "worstSlackNs",
                        finite_number(update.constrained.worst_slack)?,
                    ),
                    ("constraintDiff", timing_constraint_diff_json(&update.diff)),
                ]),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let corners = incremental
        .result
        .corners
        .iter()
        .map(|corner| {
            Ok(Json::object([
                ("scenario", Json::string(corner.scenario.clone())),
                ("clock", Json::string(corner.timing.clock.clone())),
                ("periodNs", finite_number(corner.timing.period)?),
                (
                    "criticalPathDelayNs",
                    finite_number(corner.timing.base.critical_path_delay)?,
                ),
                ("worstSlackNs", finite_number(corner.timing.worst_slack)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(Json::object([
        ("oldScenarioCount", Json::from(old.len() as u64)),
        ("newScenarioCount", Json::from(new.len() as u64)),
        (
            "baselineWorstScenario",
            previous.worst_scenario().map_or(Json::Null, Json::string),
        ),
        (
            "finalWorstScenario",
            incremental
                .result
                .worst_scenario()
                .map_or(Json::Null, Json::string),
        ),
        (
            "finalWorstSetupSlackNs",
            optional_number(incremental.result.worst_setup_slack())?,
        ),
        (
            "finalWorstHoldSlackNs",
            optional_number(incremental.result.worst_hold_slack())?,
        ),
        ("added", string_array(diff.added.iter().cloned())),
        ("removed", string_array(diff.removed.iter().cloned())),
        ("changed", string_array(diff.changed.iter().cloned())),
        (
            "timingChanged",
            string_array(diff.timing_changed.iter().cloned()),
        ),
        ("reused", string_array(incremental.reused.iter().cloned())),
        ("updates", Json::Object(updates)),
        ("corners", Json::Array(corners)),
        (
            "diffReport",
            Json::string(timing_scenario_diff_report(diff)),
        ),
        (
            "finalReport",
            Json::string(multi_corner_timing_report(&incremental.result)),
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

fn finite_json(value: f64) -> Result<Json, String> {
    if value.is_finite() {
        Json::number(value)
    } else {
        Ok(Json::Null)
    }
}

fn optional_finite_json(value: Option<f64>) -> Result<Json, String> {
    value.map_or(Ok(Json::Null), finite_json)
}

fn timing_object_json(kind: &str, name: &str) -> Json {
    Json::object([("kind", Json::string(kind)), ("name", Json::string(name))])
}

fn timing_launch_json(launch: &eda_api::TimingLaunch) -> Json {
    match launch {
        eda_api::TimingLaunch::PrimaryInput(name) => timing_object_json("primaryInput", name),
        eda_api::TimingLaunch::SequentialCell(name) => timing_object_json("sequentialCell", name),
    }
}

fn timing_endpoint_json(endpoint: &eda_api::TimingPathEndpoint) -> Json {
    match endpoint {
        eda_api::TimingPathEndpoint::SequentialCell(name) => {
            timing_object_json("sequentialCell", name)
        }
        eda_api::TimingPathEndpoint::OutputPort(name) => timing_object_json("outputPort", name),
    }
}

fn timing_path_kind_json(kind: eda_api::TimingPathKind) -> Json {
    Json::string(match kind {
        eda_api::TimingPathKind::InputToSequential => "inputToSequential",
        eda_api::TimingPathKind::SequentialToSequential => "sequentialToSequential",
        eda_api::TimingPathKind::InputToOutput => "inputToOutput",
        eda_api::TimingPathKind::SequentialToOutput => "sequentialToOutput",
    })
}

fn timing_arc_kind_json(kind: eda_api::TimingArcKind) -> Json {
    Json::string(match kind {
        eda_api::TimingArcKind::PrimaryInputLaunch => "primaryInputLaunch",
        eda_api::TimingArcKind::ClockToOutput => "clockToOutput",
        eda_api::TimingArcKind::Combinational => "combinational",
    })
}

fn timing_path_json(nl: &Netlist, path: &eda_api::TimingPath) -> Result<Json, String> {
    let cell_names = path
        .cells
        .iter()
        .map(|&cell| Json::string(nl.cell(cell).name.clone()))
        .collect::<Vec<_>>();
    let net_names = path
        .nets
        .iter()
        .map(|&net| Json::string(nl.net(net).name.clone()))
        .collect::<Vec<_>>();
    let arcs = path
        .arcs
        .iter()
        .map(|arc| {
            Ok(Json::object([
                ("kind", timing_arc_kind_json(arc.kind)),
                (
                    "cell",
                    arc.cell
                        .map(|cell| Json::from(cell as i64))
                        .unwrap_or(Json::Null),
                ),
                (
                    "cellName",
                    arc.cell
                        .map(|cell| Json::string(nl.cell(cell).name.clone()))
                        .unwrap_or(Json::Null),
                ),
                (
                    "fromNet",
                    arc.from_net
                        .map(|net| Json::from(net as i64))
                        .unwrap_or(Json::Null),
                ),
                (
                    "fromNetName",
                    arc.from_net
                        .map(|net| Json::string(nl.net(net).name.clone()))
                        .unwrap_or(Json::Null),
                ),
                ("toNet", Json::from(arc.to_net as i64)),
                ("toNetName", Json::string(nl.net(arc.to_net).name.clone())),
                ("fromArrivalNs", optional_finite_json(arc.from_arrival_ns)?),
                ("toArrivalNs", finite_json(arc.to_arrival_ns)?),
                ("delayNs", finite_json(arc.delay_ns)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("kind", timing_path_kind_json(path.kind)),
        ("launch", timing_launch_json(&path.launch)),
        ("endpoint", timing_endpoint_json(&path.endpoint)),
        (
            "cellIds",
            Json::Array(
                path.cells
                    .iter()
                    .map(|cell| Json::from(*cell as i64))
                    .collect(),
            ),
        ),
        ("cellNames", Json::Array(cell_names)),
        (
            "netIds",
            Json::Array(
                path.nets
                    .iter()
                    .map(|net| Json::from(*net as i64))
                    .collect(),
            ),
        ),
        ("netNames", Json::Array(net_names)),
        ("arcs", Json::Array(arcs)),
        ("arrivalNs", finite_json(path.arrival_ns)?),
    ]))
}

fn corner_slacks_json(slacks: &[eda_api::ScenarioSlack]) -> Result<Json, String> {
    let values = slacks
        .iter()
        .map(|slack| {
            Ok(Json::object([
                ("scenario", Json::string(slack.scenario.clone())),
                ("slackNs", optional_finite_json(slack.slack_ns)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::Array(values))
}

pub(crate) fn rtl_mmmc_timing_paths(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing paths cancelled".into());
    }
    let source = required_string(params, "source")?;
    let top = optional_top(params)?;
    let scenario_source = required_string(params, "scenarios")?;
    let Some(fields) = params.as_object() else {
        return Err("params must be an object".into());
    };
    let max_paths = match fields.get("maxPaths") {
        None => 10,
        Some(Json::Number(value)) => {
            if *value <= 0.0 || value.fract() != 0.0 || *value > usize::MAX as f64 {
                return Err("RPC parameter 'maxPaths' must be a positive integer".into());
            }
            *value as usize
        }
        Some(_) => return Err("RPC parameter 'maxPaths' must be a positive integer".into()),
    };
    let definitions = parse_timing_scenarios(&scenario_source)?;
    if definitions.is_empty() {
        return Err("RTL MMMC timing paths require at least one scenario".into());
    }
    let flow = run_flow_cancellable(&source, top, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing paths cancelled".into());
    }
    let scenarios = definitions
        .iter()
        .map(|definition| TimingScenario {
            name: definition.name.as_str(),
            netlist: &flow.netlist,
            constraints: &definition.constraints,
        })
        .collect::<Vec<_>>();
    let result = analyze_timing_scenarios_from_definitions(&flow.netlist, &definitions)?;
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing paths cancelled".into());
    }

    let setup_paths =
        multi_corner_setup_timing_paths(&result, scenarios.iter().copied(), max_paths)?;
    let hold_paths = multi_corner_hold_timing_paths(&result, scenarios.iter().copied(), max_paths)?;
    let reset_paths =
        multi_corner_reset_timing_paths(&result, scenarios.iter().copied(), max_paths)?;
    if cancellation.is_cancelled() {
        return Err("RTL MMMC timing paths cancelled".into());
    }

    let setup = setup_paths
        .iter()
        .map(|path| {
            Ok(Json::object([
                ("endpoint", timing_endpoint_json(&path.endpoint)),
                ("scenario", Json::string(path.scenario.clone())),
                ("slackNs", finite_json(path.path.slack_ns)?),
                ("requiredNs", finite_json(path.path.required_ns)?),
                ("path", timing_path_json(path.netlist, &path.path.path)?),
                ("cornerSlacks", corner_slacks_json(&path.corner_slacks)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let hold = hold_paths
        .iter()
        .map(|path| {
            Ok(Json::object([
                ("endpoint", timing_endpoint_json(&path.endpoint)),
                ("scenario", Json::string(path.scenario.clone())),
                ("slackNs", finite_json(path.path.slack_ns)?),
                ("requiredNs", finite_json(path.path.required_ns)?),
                ("holdNs", finite_json(path.path.path.hold_ns)?),
                (
                    "path",
                    timing_path_json(path.netlist, &path.path.path.path)?,
                ),
                ("cornerSlacks", corner_slacks_json(&path.corner_slacks)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let reset = reset_paths
        .iter()
        .map(|path| {
            Ok(Json::object([
                ("endpoint", timing_endpoint_json(&path.endpoint)),
                (
                    "check",
                    Json::string(match path.check {
                        eda_api::ResetCheckKind::Recovery => "recovery",
                        eda_api::ResetCheckKind::Removal => "removal",
                    }),
                ),
                ("scenario", Json::string(path.scenario.clone())),
                ("slackNs", finite_json(path.path.slack_ns)?),
                ("requiredNs", finite_json(path.path.required_ns)?),
                ("path", timing_path_json(path.netlist, &path.path.path)?),
                ("cornerSlacks", corner_slacks_json(&path.corner_slacks)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(Json::object([
        ("scenarioCount", Json::from(definitions.len() as u64)),
        ("maxPaths", Json::from(max_paths as u64)),
        (
            "worstScenario",
            result.worst_scenario().map_or(Json::Null, Json::string),
        ),
        ("setupPaths", Json::Array(setup)),
        ("holdPaths", Json::Array(hold)),
        ("resetPaths", Json::Array(reset)),
        (
            "setupReport",
            Json::string(multi_corner_setup_timing_path_report(&setup_paths)),
        ),
        (
            "holdReport",
            Json::string(multi_corner_hold_timing_path_report(&hold_paths)),
        ),
        (
            "resetReport",
            Json::string(multi_corner_reset_timing_path_report(&reset_paths)),
        ),
    ]))
}
