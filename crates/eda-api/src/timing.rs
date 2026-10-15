//! Timing constraints, timing paths, waivers, MMMC, and signal-quality facade APIs.

use super::*;

pub fn parse_timing_constraints(sdc: &str) -> Result<eda_sta::TimingConstraints, String> {
    eda_sta::parse_sdc(sdc)
}

/// SDC-aware static timing analysis through the unified API.
pub fn constrained_timing(
    nl: &Netlist,
    constraints: &eda_sta::TimingConstraints,
) -> Result<eda_sta::ConstrainedTiming, String> {
    eda_sta::analyze_with_sdc(nl, constraints)
}

/// Extract a stable-name block timing interface through the unified API.
pub fn extract_block_timing_model(
    nl: &Netlist,
    timing: &eda_sta::ConstrainedTiming,
) -> Result<BlockTimingModel, String> {
    eda_sta::block_timing_model(nl, timing)
}

/// Render a block timing interface model.
pub fn block_timing_model_report(model: &BlockTimingModel) -> String {
    eda_sta::block_timing_model_report(model)
}

/// Serialize a block timing interface model.
pub fn write_block_timing_model(model: &BlockTimingModel) -> Result<String, String> {
    eda_sta::write_block_timing_model(model)
}

/// Parse a serialized block timing interface model.
pub fn parse_block_timing_model(source: &str) -> Result<BlockTimingModel, String> {
    eda_sta::parse_block_timing_model(source)
}

/// Compose block timing interfaces by stable instance and port names.
pub fn compose_block_timing_models(
    models: &[BlockTimingModel],
    instances: &[BlockTimingCompositionInstance],
    connections: &[BlockTimingCompositionConnection],
) -> Result<BlockTimingCompositionResult, String> {
    eda_sta::compose_block_timing_models(models, instances, connections)
}

/// Compose block timing interfaces with per-instance clock-name mappings.
pub fn compose_block_timing_models_with_clock_maps(
    models: &[BlockTimingModel],
    instances: &[BlockTimingCompositionInstance],
    connections: &[BlockTimingCompositionConnection],
    clock_maps: &[BlockTimingClockMap],
) -> Result<BlockTimingCompositionResult, String> {
    eda_sta::compose_block_timing_models_with_clock_maps(models, instances, connections, clock_maps)
}

/// Compose block timing interfaces with multiple named clocks and timed edges.
pub fn compose_block_timing_models_with_clock_domains(
    models: &[BlockTimingModel],
    instances: &[BlockTimingCompositionInstance],
    connections: &[BlockTimingCompositionConnection],
    clock_maps: &[BlockTimingClockMap],
    clocks: &[BlockTimingClock],
    clock_relationships: &[BlockTimingClockRelationship],
) -> Result<BlockTimingCompositionResult, String> {
    eda_sta::compose_block_timing_models_with_clock_domains(
        models,
        instances,
        connections,
        clock_maps,
        clocks,
        clock_relationships,
    )
}

/// Render composed block-boundary timing checks.
pub fn block_timing_composition_report(result: &BlockTimingCompositionResult) -> String {
    eda_sta::block_timing_composition_report(result)
}

/// Compare two SDC constraint sets through the unified API.
pub fn diff_timing_constraints(
    old: &eda_sta::TimingConstraints,
    new: &eda_sta::TimingConstraints,
) -> TimingConstraintDiff {
    eda_sta::diff_timing_constraints(old, new)
}

/// Incrementally update base and constrained timing after SDC changes.
pub fn update_timing_for_constraints(
    nl: &Netlist,
    previous: &StaResult,
    old_constraints: &eda_sta::TimingConstraints,
    new_constraints: &eda_sta::TimingConstraints,
) -> Result<ConstraintTimingResult, String> {
    eda_sta::update_timing_for_constraints(nl, previous, old_constraints, new_constraints)
}

/// Incrementally update same-topology timing after input-delay changes.
pub fn update_timing_for_input_delays<I, S>(
    nl: &Netlist,
    previous: &StaResult,
    constraints: &eda_sta::TimingConstraints,
    changed_ports: I,
) -> Result<IncrementalTimingResult, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    eda_sta::update_timing_for_input_delays(nl, previous, constraints, changed_ports)
}

/// Incrementally update same-topology timing after cell/pin derate changes.
pub fn update_timing_for_timing_derates<C, S>(
    nl: &Netlist,
    previous: &StaResult,
    constraints: &eda_sta::TimingConstraints,
    changed_cells: C,
    changed_pins: &[eda_sta::TimingPin],
) -> Result<IncrementalTimingResult, String>
where
    C: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    eda_sta::update_timing_for_timing_derates(
        nl,
        previous,
        constraints,
        changed_cells,
        changed_pins,
    )
}

/// Compare two flat netlists by stable timing-object names.
pub fn diff_timing_topology(old: &Netlist, new: &Netlist) -> TimingTopologyDiff {
    eda_sta::diff_timing_topology(old, new)
}

/// Incrementally update timing after a flat-netlist topology/ECO change.
pub fn update_timing_for_topology(
    old: &Netlist,
    previous: &StaResult,
    new: &Netlist,
    constraints: &eda_sta::TimingConstraints,
) -> Result<TopologyTimingResult, String> {
    eda_sta::update_timing_for_topology(old, previous, new, constraints)
}

/// Extract late setup/output timing paths through the unified API.
pub fn timing_paths(nl: &Netlist, timing: &StaResult, max_paths: usize) -> Vec<TimingPath> {
    timing.timing_paths(nl, max_paths)
}

/// Extract early min-delay paths through the unified API.
pub fn min_timing_paths(nl: &Netlist, timing: &StaResult, max_paths: usize) -> Vec<TimingPath> {
    timing.min_timing_paths(nl, max_paths)
}

/// Analyze base timing through the unified API.
pub fn analyze_timing(nl: &Netlist) -> Result<StaResult, String> {
    eda_sta::analyze(nl)
}

/// Attribute one timing path to retained post-synthesis hierarchy instances.
pub fn hierarchical_timing_path(
    nl: &Netlist,
    path: &TimingPath,
) -> Result<HierarchicalTimingPath, String> {
    eda_sta::hierarchical_timing_path(nl, path)
}

/// Attribute timing paths to retained post-synthesis hierarchy instances.
pub fn hierarchical_timing_paths(
    nl: &Netlist,
    paths: &[TimingPath],
) -> Result<Vec<HierarchicalTimingPath>, String> {
    eda_sta::hierarchical_timing_paths(nl, paths)
}

/// Group constrained setup/output timing paths by hierarchy instance.
pub fn hierarchical_constrained_timing_paths(
    nl: &Netlist,
    paths: &[ConstrainedTimingPath],
) -> Result<Vec<HierarchicalTimingPath>, String> {
    paths
        .iter()
        .map(|constrained| hierarchical_timing_path(nl, &constrained.path))
        .collect()
}

/// Group hold timing paths by hierarchy instance.
pub fn hierarchical_hold_timing_paths(
    nl: &Netlist,
    paths: &[HoldTimingPath],
) -> Result<Vec<HierarchicalTimingPath>, String> {
    paths
        .iter()
        .map(|hold| hierarchical_timing_path(nl, &hold.path))
        .collect()
}

/// Group constrained hold timing paths by hierarchy instance.
pub fn hierarchical_constrained_hold_timing_paths(
    nl: &Netlist,
    paths: &[ConstrainedHoldTimingPath],
) -> Result<Vec<HierarchicalTimingPath>, String> {
    paths
        .iter()
        .map(|hold| hierarchical_timing_path(nl, &hold.path.path))
        .collect()
}

/// Group recovery/removal paths by hierarchy instance.
pub fn hierarchical_reset_timing_paths(
    nl: &Netlist,
    paths: &[ResetTimingPaths],
) -> Result<Vec<HierarchicalTimingPath>, String> {
    paths
        .iter()
        .flat_map(|reset| [&reset.recovery, &reset.removal])
        .map(|path| hierarchical_timing_path(nl, path))
        .collect()
}

/// Group constrained recovery/removal paths by hierarchy instance.
pub fn hierarchical_constrained_reset_timing_paths(
    nl: &Netlist,
    paths: &[ConstrainedResetTimingPath],
) -> Result<Vec<HierarchicalTimingPath>, String> {
    paths
        .iter()
        .map(|reset| hierarchical_timing_path(nl, &reset.path))
        .collect()
}

/// Render hierarchy-grouped timing paths.
pub fn hierarchical_timing_path_report(paths: &[HierarchicalTimingPath]) -> String {
    eda_sta::hierarchical_timing_path_report(paths)
}

/// Render an unconstrained STA summary through the unified API.
pub fn timing_report(nl: &Netlist, timing: &StaResult) -> String {
    eda_sta::report(nl, timing)
}

/// Render a constrained STA summary through the unified API.
pub fn constrained_timing_report(timing: &ConstrainedTiming) -> String {
    eda_sta::constrained_report(timing)
}

/// Render an unconstrained timing-path report.
pub fn timing_path_report(nl: &Netlist, paths: &[TimingPath]) -> String {
    eda_sta::timing_path_report(nl, paths)
}

/// Extract constrained setup/output paths sorted by worst slack.
pub fn constrained_timing_paths(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    max_paths: usize,
) -> Vec<ConstrainedTimingPath> {
    eda_sta::constrained_timing_paths(nl, timing, max_paths)
}

/// Render a constrained timing-path report.
pub fn constrained_timing_path_report(nl: &Netlist, paths: &[ConstrainedTimingPath]) -> String {
    eda_sta::constrained_timing_path_report(nl, paths)
}

/// Extract early hold paths through the unified API.
pub fn hold_timing_paths(
    nl: &Netlist,
    timing: &StaResult,
    hold_uncertainty: f64,
    max_paths: usize,
) -> Vec<HoldTimingPath> {
    timing.hold_timing_paths(nl, hold_uncertainty, max_paths)
}

/// Render an unconstrained hold-path report.
pub fn hold_timing_path_report(nl: &Netlist, paths: &[HoldTimingPath]) -> String {
    eda_sta::hold_timing_path_report(nl, paths)
}

/// Extract constrained hold paths sorted by worst hold slack.
pub fn constrained_hold_timing_paths(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    max_paths: usize,
) -> Vec<ConstrainedHoldTimingPath> {
    eda_sta::constrained_hold_timing_paths(nl, timing, max_paths)
}

/// Render a constrained hold-path report.
pub fn constrained_hold_timing_path_report(
    nl: &Netlist,
    paths: &[ConstrainedHoldTimingPath],
) -> String {
    eda_sta::constrained_hold_timing_path_report(nl, paths)
}

/// Extract asynchronous-reset recovery/removal path pairs.
pub fn reset_timing_paths(
    nl: &Netlist,
    timing: &StaResult,
    max_paths: usize,
) -> Vec<ResetTimingPaths> {
    timing.reset_timing_paths(nl, max_paths)
}

/// Render an unconstrained asynchronous-reset path report.
pub fn reset_timing_path_report(nl: &Netlist, paths: &[ResetTimingPaths]) -> String {
    eda_sta::reset_timing_path_report(nl, paths)
}

/// Extract constrained recovery/removal paths sorted by worst reset slack.
pub fn constrained_reset_timing_paths(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    max_paths: usize,
) -> Vec<ConstrainedResetTimingPath> {
    eda_sta::constrained_reset_timing_paths(nl, timing, max_paths)
}

/// Render a constrained asynchronous-reset path report.
pub fn constrained_reset_timing_path_report(
    nl: &Netlist,
    paths: &[ConstrainedResetTimingPath],
) -> String {
    eda_sta::constrained_reset_timing_path_report(nl, paths)
}

/// Extract concrete timing paths omitted by false-path exceptions.
pub fn excluded_timing_paths(nl: &Netlist, timing: &ConstrainedTiming) -> Vec<ExcludedTimingPath> {
    eda_sta::excluded_timing_paths(nl, timing)
}

/// Render concrete false-path exclusions.
pub fn excluded_timing_path_report(nl: &Netlist, paths: &[ExcludedTimingPath]) -> String {
    eda_sta::excluded_timing_path_report(nl, paths)
}

/// Build a persistent scenario timing-violation store.
pub fn timing_violation_store(
    scenario: &str,
    nl: &Netlist,
    timing: &ConstrainedTiming,
) -> Result<TimingViolationStore, String> {
    eda_sta::TimingViolationStore::from_timing(scenario, nl, timing)
}

/// Incrementally update a persistent timing-violation store.
pub fn update_timing_violations(
    store: &mut TimingViolationStore,
    nl: &Netlist,
    timing: &ConstrainedTiming,
) -> Result<TimingViolationDiff, String> {
    store.update(nl, timing)
}

/// Update only timing violations touched by stable-name dirty objects.
///
/// Untouched negative checks retain their previous slack and lifecycle state.
/// A dirty clock conservatively updates every check governed by that clock.
pub fn update_timing_violations_incremental(
    store: &mut TimingViolationStore,
    nl: &Netlist,
    timing: &ConstrainedTiming,
    dirty: &TimingViolationDirtyObjects,
) -> Result<TimingViolationDiff, String> {
    store.update_dirty(nl, timing, dirty)
}

/// Render a timing-violation store and optional lifecycle diff.
pub fn timing_violation_report(
    store: &TimingViolationStore,
    diff: Option<&TimingViolationDiff>,
) -> String {
    eda_sta::timing_violation_report(store, diff)
}

/// Build a validated timing-violation waiver.
pub fn timing_violation_waiver(
    id: &str,
    scenario_pattern: &str,
    endpoint_pattern: &str,
    slack_floor_ns: f64,
) -> Result<TimingViolationWaiver, String> {
    eda_sta::TimingViolationWaiver::new(id, scenario_pattern, endpoint_pattern, slack_floor_ns)
}

/// Add a waiver to one scenario's persistent violation store.
pub fn add_timing_violation_waiver(
    store: &mut TimingViolationStore,
    waiver: TimingViolationWaiver,
) -> Result<(), String> {
    store.add_waiver(waiver)
}

/// Add a waiver to one scenario store with an audit actor.
pub fn add_timing_violation_waiver_with_actor(
    store: &mut TimingViolationStore,
    waiver: TimingViolationWaiver,
    actor: Option<&str>,
) -> Result<(), String> {
    store.add_waiver_with_actor(waiver, actor.map(str::to_string))
}

/// Remove a waiver from one scenario's persistent violation store.
pub fn remove_timing_violation_waiver(
    store: &mut TimingViolationStore,
    id: &str,
) -> Result<(), String> {
    store.remove_waiver(id)
}

/// Remove a waiver from one scenario store with an audit actor.
pub fn remove_timing_violation_waiver_with_actor(
    store: &mut TimingViolationStore,
    id: &str,
    actor: Option<&str>,
) -> Result<(), String> {
    store.remove_waiver_with_actor(id, actor.map(str::to_string))
}

/// Serialize timing-violation waivers.
pub fn write_timing_violation_waivers(waivers: &[TimingViolationWaiver]) -> Result<String, String> {
    eda_sta::write_timing_violation_waivers(waivers)
}

/// Parse serialized timing-violation waivers.
pub fn parse_timing_violation_waivers(source: &str) -> Result<Vec<TimingViolationWaiver>, String> {
    eda_sta::parse_timing_violation_waivers(source)
}

/// Render store waiver audit history.
pub fn timing_waiver_audit_report(entries: &[TimingWaiverAuditEntry]) -> String {
    eda_sta::timing_waiver_audit_report(entries)
}

/// Serialize waiver add/remove audit history.
pub fn write_timing_waiver_audit(entries: &[TimingWaiverAuditEntry]) -> Result<String, String> {
    eda_sta::write_timing_waiver_audit(entries)
}

/// Parse serialized waiver audit history.
pub fn parse_timing_waiver_audit(source: &str) -> Result<Vec<TimingWaiverAuditEntry>, String> {
    eda_sta::parse_timing_waiver_audit(source)
}

/// Replay waiver audit history and return active waivers.
pub fn active_timing_waivers_from_audit(
    entries: &[TimingWaiverAuditEntry],
) -> Result<Vec<TimingViolationWaiver>, String> {
    eda_sta::active_timing_waivers_from_audit(entries)
}

/// Aggregate waiver audit events by hierarchy-instance selector.
pub fn timing_waiver_audit_hierarchy_summaries(
    entries: &[TimingWaiverAuditEntry],
) -> Result<std::collections::BTreeMap<Option<String>, TimingWaiverAuditHierarchySummary>, String> {
    eda_sta::timing_waiver_audit_hierarchy_summaries(entries)
}

/// Render hierarchy-scoped waiver audit aggregation.
pub fn timing_waiver_audit_hierarchy_report(
    entries: &[TimingWaiverAuditEntry],
) -> Result<String, String> {
    eda_sta::timing_waiver_audit_hierarchy_report(entries)
}

/// Restore a serialized waiver audit into one scenario store.
pub fn restore_timing_waiver_audit(
    store: &mut TimingViolationStore,
    entries: &[TimingWaiverAuditEntry],
) -> Result<(), String> {
    store.restore_waiver_audit(entries)
}

/// Build a persistent multi-scenario timing-violation registry.
pub fn timing_violation_registry<'a, I>(scenarios: I) -> Result<TimingViolationRegistry, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    eda_sta::TimingViolationRegistry::from_scenarios(scenarios)
}

/// Build an MMMC registry snapshot while preserving scenario-local waivers.
pub fn timing_violation_registry_from_stores<I>(
    stores: I,
) -> Result<TimingViolationRegistry, String>
where
    I: IntoIterator<Item = (String, TimingViolationStore)>,
{
    eda_sta::timing_violation_registry_from_stores(stores)
}

/// Replace all stores in a persistent MMMC registry from materialized stores.
pub fn update_timing_violation_registry_from_stores<I>(
    registry: &mut TimingViolationRegistry,
    stores: I,
) -> Result<TimingViolationRegistryUpdate, String>
where
    I: IntoIterator<Item = (String, TimingViolationStore)>,
{
    registry.replace_stores(stores)
}

/// Replace the scenario set in a persistent multi-scenario violation registry.
pub fn update_timing_violation_registry<'a, I>(
    registry: &mut TimingViolationRegistry,
    scenarios: I,
) -> Result<TimingViolationRegistryUpdate, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    registry.replace_scenarios(scenarios)
}

/// Update one MMMC scenario using stable-name dirty timing objects.
pub fn update_timing_violation_registry_scenario_incremental(
    registry: &mut TimingViolationRegistry,
    scenario: TimingScenario<'_>,
    dirty: &TimingViolationDirtyObjects,
) -> Result<TimingViolationDiff, String> {
    registry.update_scenario_dirty(scenario, dirty)
}

/// Add a waiver to every scenario in a multi-corner violation registry.
pub fn add_timing_violation_registry_waiver(
    registry: &mut TimingViolationRegistry,
    waiver: TimingViolationWaiver,
) -> Result<(), String> {
    registry.add_waiver(waiver)
}

/// Add a registry waiver with an audit actor.
pub fn add_timing_violation_registry_waiver_with_actor(
    registry: &mut TimingViolationRegistry,
    waiver: TimingViolationWaiver,
    actor: Option<&str>,
) -> Result<(), String> {
    registry.add_waiver_with_actor(waiver, actor.map(str::to_string))
}

/// Remove a waiver from every scenario in a multi-corner violation registry.
pub fn remove_timing_violation_registry_waiver(
    registry: &mut TimingViolationRegistry,
    id: &str,
) -> Result<(), String> {
    registry.remove_waiver(id)
}

/// Remove a registry waiver with an audit actor.
pub fn remove_timing_violation_registry_waiver_with_actor(
    registry: &mut TimingViolationRegistry,
    id: &str,
    actor: Option<&str>,
) -> Result<(), String> {
    registry.remove_waiver_with_actor(id, actor.map(str::to_string))
}

/// Restore serialized waiver audit history across a violation registry.
pub fn restore_timing_waiver_registry_audit(
    registry: &mut TimingViolationRegistry,
    entries: &[TimingWaiverAuditEntry],
) -> Result<(), String> {
    registry.restore_waiver_audit(entries)
}

/// Render a multi-scenario timing-violation registry.
pub fn timing_violation_registry_report(
    registry: &TimingViolationRegistry,
    update: Option<&TimingViolationRegistryUpdate>,
) -> String {
    eda_sta::timing_violation_registry_report(registry, update)
}

/// Analyze named mode/corner scenarios and aggregate the worst results.
pub fn analyze_timing_scenarios<'a, I>(scenarios: I) -> Result<MultiCornerTimingResult, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    eda_sta::analyze_timing_scenarios(scenarios)
}

/// Parse inline named timing-mode/corner definitions.
pub fn parse_timing_scenarios(source: &str) -> Result<Vec<TimingScenarioDefinition>, String> {
    eda_sta::parse_timing_scenarios(source)
}

/// Analyze inline timing scenarios against one mapped netlist.
pub fn analyze_timing_scenarios_from_definitions(
    netlist: &Netlist,
    scenarios: &[TimingScenarioDefinition],
) -> Result<MultiCornerTimingResult, String> {
    eda_sta::analyze_timing_scenario_definitions(netlist, scenarios)
}

/// Compare named mode/corner scenario definitions through the unified API.
pub fn diff_timing_scenarios(
    old: &[TimingScenarioDefinition],
    new: &[TimingScenarioDefinition],
) -> Result<TimingScenarioDiff, String> {
    eda_sta::diff_timing_scenarios(old, new)
}

/// Render an MMMC scenario diff through the unified API.
pub fn timing_scenario_diff_report(diff: &TimingScenarioDiff) -> String {
    eda_sta::timing_scenario_diff_report(diff)
}

/// Incrementally update multi-corner timing after scenario/SDC changes.
pub fn update_timing_scenarios_incrementally(
    netlist: &Netlist,
    previous: &MultiCornerTimingResult,
    old: &[TimingScenarioDefinition],
    new: &[TimingScenarioDefinition],
) -> Result<IncrementalMultiCornerTimingResult, String> {
    eda_sta::update_timing_scenarios_incrementally(netlist, previous, old, new)
}

/// Compare scenario sets that may use different netlists per corner.
pub fn diff_timing_scenario_sets<'old, 'new>(
    old: &[TimingScenario<'old>],
    new: &[TimingScenario<'new>],
) -> Result<MultiCornerScenarioSetDiff, String> {
    eda_sta::diff_timing_scenario_sets(old, new)
}

/// Incrementally update MMMC scenarios with per-corner netlist topology changes.
pub fn update_timing_scenarios_with_topology_incrementally<'old, 'new>(
    previous: &MultiCornerTimingResult,
    old: &[TimingScenario<'old>],
    new: &[TimingScenario<'new>],
) -> Result<IncrementalMultiCornerTopologyTimingResult, String> {
    eda_sta::update_timing_scenarios_with_topology_incrementally(previous, old, new)
}

/// Render a multi-scenario STA report.
pub fn multi_corner_timing_report(result: &MultiCornerTimingResult) -> String {
    eda_sta::multi_corner_report(result)
}

/// Merge setup/output timing paths across named scenarios through the API.
pub fn multi_corner_setup_timing_paths<'a, I>(
    result: &MultiCornerTimingResult,
    scenarios: I,
    max_paths: usize,
) -> Result<Vec<MultiCornerSetupTimingPath<'a>>, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    eda_sta::multi_corner_setup_timing_paths(result, scenarios, max_paths)
}

/// Render merged multi-scenario setup/output paths.
pub fn multi_corner_setup_timing_path_report(paths: &[MultiCornerSetupTimingPath<'_>]) -> String {
    eda_sta::multi_corner_setup_timing_path_report(paths)
}

/// Merge hold timing paths across named scenarios through the API.
pub fn multi_corner_hold_timing_paths<'a, I>(
    result: &MultiCornerTimingResult,
    scenarios: I,
    max_paths: usize,
) -> Result<Vec<MultiCornerHoldTimingPath<'a>>, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    eda_sta::multi_corner_hold_timing_paths(result, scenarios, max_paths)
}

/// Render merged multi-scenario hold paths.
pub fn multi_corner_hold_timing_path_report(paths: &[MultiCornerHoldTimingPath<'_>]) -> String {
    eda_sta::multi_corner_hold_timing_path_report(paths)
}

/// Merge recovery/removal timing paths across named scenarios through the API.
pub fn multi_corner_reset_timing_paths<'a, I>(
    result: &MultiCornerTimingResult,
    scenarios: I,
    max_paths: usize,
) -> Result<Vec<MultiCornerResetTimingPath<'a>>, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    eda_sta::multi_corner_reset_timing_paths(result, scenarios, max_paths)
}

/// Render merged multi-scenario recovery/removal paths.
pub fn multi_corner_reset_timing_path_report(paths: &[MultiCornerResetTimingPath<'_>]) -> String {
    eda_sta::multi_corner_reset_timing_path_report(paths)
}

/// Run transition, capacitance, and fanout signal-quality checks.
pub fn analyze_signal_quality(
    netlist: &Netlist,
    options: &QualityOptions,
) -> Result<QualityResult, String> {
    eda_sta::analyze_quality(netlist, options)
}

/// Derive design-wide signal-quality options from parsed SDC constraints.
pub fn quality_options_from_sdc(constraints: &TimingConstraints) -> QualityOptions {
    constraints.quality_options()
}

/// Run signal-quality checks with SPEF grounded/coupling capacitance included.
pub fn analyze_signal_quality_with_parasitics(
    netlist: &Netlist,
    parasitics: &ParasiticReport,
    options: &QualityOptions,
) -> Result<QualityResult, String> {
    let mut options = options.clone();
    for net in &parasitics.nets {
        let effective_cap = net.grounded_cap_pf + 0.5 * net.coupling_cap_pf;
        options
            .extra_capacitance_pf
            .insert(net.net.clone(), effective_cap);
    }
    eda_sta::analyze_quality(netlist, &options)
}
