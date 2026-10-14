//! Multi-scenario timing aggregation.
//!
//! A scenario owns the mode/corner name and references the netlist and SDC
//! constraints used for that analysis. Different scenarios may reference
//! different netlists when a corner uses a different Liberty view.

use crate::constraints::{
    analyze_with_sdc, constrained_hold_timing_path_report, constrained_hold_timing_paths,
    constrained_report, constrained_reset_timing_path_report, constrained_reset_timing_paths,
    constrained_timing_path_report, constrained_timing_paths, diff_timing_constraints, parse_sdc,
    ResetCheckKind, TimingConstraintDiff, TimingConstraints,
};
use eda_core::netlist::Netlist;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Copy, Debug)]
pub struct TimingScenario<'a> {
    pub name: &'a str,
    pub netlist: &'a Netlist,
    pub constraints: &'a TimingConstraints,
}

impl<'a> TimingScenario<'a> {
    pub fn new(name: &'a str, netlist: &'a Netlist, constraints: &'a TimingConstraints) -> Self {
        Self {
            name,
            netlist,
            constraints,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CornerTiming {
    pub scenario: String,
    pub timing: crate::ConstrainedTiming,
}

/// One scenario's contribution to a merged endpoint path group.
#[derive(Clone, Debug)]
pub struct ScenarioSlack {
    pub scenario: String,
    pub slack_ns: Option<f64>,
}

/// Worst setup/output timing path for one endpoint across all scenarios.
#[derive(Clone, Debug)]
pub struct MultiCornerSetupTimingPath<'a> {
    pub endpoint: crate::TimingPathEndpoint,
    pub scenario: String,
    pub netlist: &'a Netlist,
    pub path: crate::ConstrainedTimingPath,
    pub corner_slacks: Vec<ScenarioSlack>,
}

/// Worst hold timing path for one sequential endpoint across all scenarios.
#[derive(Clone, Debug)]
pub struct MultiCornerHoldTimingPath<'a> {
    pub endpoint: crate::TimingPathEndpoint,
    pub scenario: String,
    pub netlist: &'a Netlist,
    pub path: crate::ConstrainedHoldTimingPath,
    pub corner_slacks: Vec<ScenarioSlack>,
}

/// Worst recovery or removal path for one asynchronous-reset endpoint across
/// all scenarios.
#[derive(Clone, Debug)]
pub struct MultiCornerResetTimingPath<'a> {
    pub endpoint: crate::TimingPathEndpoint,
    pub check: ResetCheckKind,
    pub scenario: String,
    pub netlist: &'a Netlist,
    pub path: crate::ConstrainedResetTimingPath,
    pub corner_slacks: Vec<ScenarioSlack>,
}

/// An owned scenario parsed from an inline MMMC manifest.
#[derive(Clone, Debug)]
pub struct TimingScenarioDefinition {
    pub name: String,
    pub constraints: TimingConstraints,
    /// Generic scenario properties that are not timing constraints.
    ///
    /// Higher-level flows can attach named mode/corner data such as supply
    /// voltage or power-model coefficients without making the STA core depend
    /// on those analysis domains.
    pub properties: BTreeMap<String, String>,
}

/// Semantic difference between two named mode/corner scenario sets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimingScenarioDiff {
    pub added: BTreeSet<String>,
    pub removed: BTreeSet<String>,
    /// Scenarios whose constraints, properties, or both changed.
    pub changed: BTreeSet<String>,
    /// Scenarios whose SDC timing constraints changed.
    ///
    /// This is a subset of `changed`. A post-route power property change, for
    /// example, changes the scenario but does not invalidate timing.
    pub timing_changed: BTreeSet<String>,
    /// Per-scenario SDC diffs for changed scenarios.
    pub constraints: BTreeMap<String, TimingConstraintDiff>,
    /// Per-scenario generic property diffs.
    pub properties: BTreeMap<String, TimingScenarioPropertyDiff>,
}

/// Property additions, removals, and value changes in one scenario.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimingScenarioPropertyDiff {
    pub added: BTreeSet<String>,
    pub removed: BTreeSet<String>,
    pub changed: BTreeMap<String, (String, String)>,
}

impl TimingScenarioPropertyDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Incremental multi-corner update result and per-scenario work accounting.
#[derive(Clone, Debug)]
pub struct IncrementalMultiCornerTimingResult {
    pub result: MultiCornerTimingResult,
    pub diff: TimingScenarioDiff,
    /// Constraint incremental updates for changed scenarios.
    pub updates: BTreeMap<String, crate::ConstraintTimingResult>,
    /// Scenarios whose constrained timing objects were reused without recompute.
    pub reused: BTreeSet<String>,
}

/// Semantic difference between scenario sets that may use different netlists.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MultiCornerScenarioSetDiff {
    pub added: BTreeSet<String>,
    pub removed: BTreeSet<String>,
    pub changed: BTreeSet<String>,
    pub changed_netlists: BTreeSet<String>,
    pub constraints: BTreeMap<String, TimingConstraintDiff>,
    pub topology: BTreeMap<String, crate::TimingTopologyDiff>,
}

/// Work performed for one scenario in a topology-aware MMMC update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiCornerScenarioUpdateKind {
    Reused,
    ConstraintIncremental,
    TopologyIncremental,
    TopologyAndConstraintIncremental,
    AddedFullAnalysis,
}

#[derive(Clone, Debug)]
pub struct MultiCornerScenarioUpdate {
    pub kind: MultiCornerScenarioUpdateKind,
    pub constraint: Option<crate::ConstraintTimingResult>,
    pub topology: Option<crate::TopologyTimingResult>,
}

/// Topology-aware incremental MMMC result.
#[derive(Clone, Debug)]
pub struct IncrementalMultiCornerTopologyTimingResult {
    pub result: MultiCornerTimingResult,
    pub diff: MultiCornerScenarioSetDiff,
    pub updates: BTreeMap<String, MultiCornerScenarioUpdate>,
    pub reused: BTreeSet<String>,
}

impl TimingScenarioDefinition {
    pub fn new(name: impl Into<String>, constraints: TimingConstraints) -> Self {
        Self {
            name: name.into(),
            constraints,
            properties: BTreeMap::new(),
        }
    }

    pub fn with_properties(
        name: impl Into<String>,
        constraints: TimingConstraints,
        properties: BTreeMap<String, String>,
    ) -> Self {
        Self {
            name: name.into(),
            constraints,
            properties,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MultiCornerTimingResult {
    pub corners: Vec<CornerTiming>,
    pub worst_setup_corner: usize,
    pub worst_hold_corner: usize,
    pub worst_slack_corner: usize,
}

impl MultiCornerTimingResult {
    pub fn worst_scenario(&self) -> Option<&str> {
        self.corners
            .get(self.worst_slack_corner)
            .map(|corner| corner.scenario.as_str())
    }

    pub fn worst_setup_slack(&self) -> Option<f64> {
        self.corners.get(self.worst_setup_corner).map(|corner| {
            corner
                .timing
                .reg2reg_slack
                .min(corner.timing.in2reg_slack)
                .min(corner.timing.output_slack)
                .min(corner.timing.max_delay_slack)
        })
    }

    pub fn worst_hold_slack(&self) -> Option<f64> {
        self.corners.get(self.worst_hold_corner).map(|corner| {
            corner
                .timing
                .reg2reg_hold_slack
                .min(corner.timing.in2reg_hold_slack)
                .min(corner.timing.min_delay_slack)
        })
    }
}

pub fn analyze_timing_scenarios<'a, I>(scenarios: I) -> Result<MultiCornerTimingResult, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    let mut corners = Vec::new();
    let mut names = BTreeSet::new();
    for scenario in scenarios {
        if scenario.name.trim().is_empty() {
            return Err("timing scenario name cannot be empty".into());
        }
        if !names.insert(scenario.name) {
            return Err(format!("duplicate timing scenario '{}'", scenario.name));
        }
        let timing = analyze_with_sdc(scenario.netlist, scenario.constraints)?;
        corners.push(CornerTiming {
            scenario: scenario.name.to_string(),
            timing,
        });
    }
    if corners.is_empty() {
        return Err("at least one timing scenario is required".into());
    }

    let worst_setup_corner = worst_index(&corners, |corner| {
        corner
            .timing
            .reg2reg_slack
            .min(corner.timing.in2reg_slack)
            .min(corner.timing.output_slack)
            .min(corner.timing.max_delay_slack)
    });
    let worst_hold_corner = worst_index(&corners, |corner| {
        corner
            .timing
            .reg2reg_hold_slack
            .min(corner.timing.in2reg_hold_slack)
            .min(corner.timing.min_delay_slack)
    });
    let worst_slack_corner = worst_index(&corners, |corner| corner.timing.worst_slack);
    Ok(MultiCornerTimingResult {
        corners,
        worst_setup_corner,
        worst_hold_corner,
        worst_slack_corner,
    })
}

/// Analyze owned inline scenario definitions against one netlist.
pub fn analyze_timing_scenario_definitions(
    netlist: &Netlist,
    definitions: &[TimingScenarioDefinition],
) -> Result<MultiCornerTimingResult, String> {
    let scenarios = definitions
        .iter()
        .map(|definition| TimingScenario::new(&definition.name, netlist, &definition.constraints))
        .collect::<Vec<_>>();
    analyze_timing_scenarios(scenarios)
}

/// Compare named timing scenarios and their SDC constraint sets.
pub fn diff_timing_scenarios(
    old: &[TimingScenarioDefinition],
    new: &[TimingScenarioDefinition],
) -> Result<TimingScenarioDiff, String> {
    let mut old_definitions = BTreeMap::new();
    for definition in old {
        if definition.name.trim().is_empty() {
            return Err("timing scenario name cannot be empty".into());
        }
        if old_definitions
            .insert(definition.name.as_str(), definition)
            .is_some()
        {
            return Err(format!("duplicate timing scenario '{}'", definition.name));
        }
    }
    let mut new_definitions = BTreeMap::new();
    for definition in new {
        if definition.name.trim().is_empty() {
            return Err("timing scenario name cannot be empty".into());
        }
        if new_definitions
            .insert(definition.name.as_str(), definition)
            .is_some()
        {
            return Err(format!("duplicate timing scenario '{}'", definition.name));
        }
    }

    let mut diff = TimingScenarioDiff::default();
    for name in old_definitions.keys() {
        if !new_definitions.contains_key(name) {
            diff.removed.insert((*name).to_string());
        }
    }
    for (name, new_definition) in &new_definitions {
        let Some(old_definition) = old_definitions.get(*name) else {
            diff.added.insert((*name).to_string());
            continue;
        };
        if old_definition.constraints != new_definition.constraints {
            let constraint_diff =
                diff_timing_constraints(&old_definition.constraints, &new_definition.constraints);
            diff.constraints
                .insert((*name).to_string(), constraint_diff);
            diff.changed.insert((*name).to_string());
            diff.timing_changed.insert((*name).to_string());
        }
        let property_diff =
            diff_scenario_properties(&old_definition.properties, &new_definition.properties);
        if !property_diff.is_empty() {
            diff.properties.insert((*name).to_string(), property_diff);
            diff.changed.insert((*name).to_string());
        }
    }
    Ok(diff)
}

fn diff_scenario_properties(
    old: &BTreeMap<String, String>,
    new: &BTreeMap<String, String>,
) -> TimingScenarioPropertyDiff {
    let mut diff = TimingScenarioPropertyDiff::default();
    for key in old.keys() {
        if !new.contains_key(key) {
            diff.removed.insert(key.clone());
        }
    }
    for (key, new_value) in new {
        if let Some(old_value) = old.get(key) {
            if old_value != new_value {
                diff.changed
                    .insert(key.clone(), (old_value.clone(), new_value.clone()));
            }
        } else {
            diff.added.insert(key.clone());
        }
    }
    diff
}

/// Render named-scenario additions, removals, timing changes, and properties.
pub fn timing_scenario_diff_report(diff: &TimingScenarioDiff) -> String {
    let names = |values: &BTreeSet<String>| values.iter().cloned().collect::<Vec<_>>().join(", ");
    let mut out = format!(
        "MMMC scenario diff: added [{}], removed [{}], changed [{}], timing changed [{}]\n",
        names(&diff.added),
        names(&diff.removed),
        names(&diff.changed),
        names(&diff.timing_changed)
    );
    for scenario in diff.changed.iter() {
        out.push_str(&format!("  scenario '{}'\n", scenario));
        if diff.timing_changed.contains(scenario) {
            out.push_str("    timing constraints changed\n");
        }
        if let Some(properties) = diff.properties.get(scenario) {
            if !properties.added.is_empty() {
                out.push_str(&format!(
                    "    added properties [{}]\n",
                    names(&properties.added)
                ));
            }
            if !properties.removed.is_empty() {
                out.push_str(&format!(
                    "    removed properties [{}]\n",
                    names(&properties.removed)
                ));
            }
            for (key, (old, new)) in &properties.changed {
                out.push_str(&format!("    property '{}': '{}' -> '{}'\n", key, old, new));
            }
        }
    }
    out
}

/// Incrementally update multi-corner timing after scenario/SDC changes.
///
/// All scenarios are analyzed against one unchanged netlist topology.
/// Timing-changed scenarios use the single-corner constraint incremental
/// engine. Scenarios with only generic property changes reuse their previous
/// `ConstrainedTiming` object because STA inputs are unchanged.
pub fn update_timing_scenarios_incrementally(
    netlist: &Netlist,
    previous: &MultiCornerTimingResult,
    old: &[TimingScenarioDefinition],
    new: &[TimingScenarioDefinition],
) -> Result<IncrementalMultiCornerTimingResult, String> {
    let diff = diff_timing_scenarios(old, new)?;
    let old_by_name = old
        .iter()
        .map(|definition| (definition.name.as_str(), definition))
        .collect::<BTreeMap<_, _>>();
    let previous_by_name = previous
        .corners
        .iter()
        .map(|corner| (corner.scenario.as_str(), corner))
        .collect::<BTreeMap<_, _>>();

    let mut corners = Vec::new();
    let mut updates = BTreeMap::new();
    let mut reused = BTreeSet::new();
    for definition in new {
        let changed = diff.timing_changed.contains(&definition.name);
        let timing = if changed {
            let previous_corner =
                previous_by_name
                    .get(definition.name.as_str())
                    .ok_or_else(|| {
                        format!(
                            "incremental MMMC result missing previous scenario '{}'",
                            definition.name
                        )
                    })?;
            let old_definition = old_by_name.get(definition.name.as_str()).ok_or_else(|| {
                format!(
                    "incremental MMMC definitions missing old scenario '{}'",
                    definition.name
                )
            })?;
            let update = crate::update_timing_for_constraints(
                netlist,
                &previous_corner.timing.base,
                &old_definition.constraints,
                &definition.constraints,
            )?;
            updates.insert(definition.name.clone(), update.clone());
            update.constrained
        } else if previous_by_name.contains_key(definition.name.as_str()) {
            let corner = previous_by_name[definition.name.as_str()];
            reused.insert(definition.name.clone());
            corner.timing.clone()
        } else {
            // Newly added scenarios have no reusable predecessor.
            analyze_with_sdc(netlist, &definition.constraints)?
        };
        corners.push(CornerTiming {
            scenario: definition.name.clone(),
            timing,
        });
    }
    if corners.is_empty() {
        return Err("at least one timing scenario is required".into());
    }

    let worst_setup_corner = worst_index(&corners, |corner| {
        corner
            .timing
            .reg2reg_slack
            .min(corner.timing.in2reg_slack)
            .min(corner.timing.output_slack)
            .min(corner.timing.max_delay_slack)
    });
    let worst_hold_corner = worst_index(&corners, |corner| {
        corner
            .timing
            .reg2reg_hold_slack
            .min(corner.timing.in2reg_hold_slack)
            .min(corner.timing.min_delay_slack)
    });
    let worst_slack_corner = worst_index(&corners, |corner| corner.timing.worst_slack);
    Ok(IncrementalMultiCornerTimingResult {
        result: MultiCornerTimingResult {
            corners,
            worst_setup_corner,
            worst_hold_corner,
            worst_slack_corner,
        },
        diff,
        updates,
        reused,
    })
}

/// Diff scenario sets that can reference different netlists per corner.
pub fn diff_timing_scenario_sets<'old, 'new>(
    old: &[TimingScenario<'old>],
    new: &[TimingScenario<'new>],
) -> Result<MultiCornerScenarioSetDiff, String> {
    let mut old_by_name = BTreeMap::new();
    for scenario in old {
        if scenario.name.trim().is_empty() {
            return Err("timing scenario name cannot be empty".into());
        }
        if old_by_name.insert(scenario.name, *scenario).is_some() {
            return Err(format!("duplicate timing scenario '{}'", scenario.name));
        }
    }
    let mut new_by_name = BTreeMap::new();
    for scenario in new {
        if scenario.name.trim().is_empty() {
            return Err("timing scenario name cannot be empty".into());
        }
        if new_by_name.insert(scenario.name, *scenario).is_some() {
            return Err(format!("duplicate timing scenario '{}'", scenario.name));
        }
    }

    let mut diff = MultiCornerScenarioSetDiff::default();
    for name in old_by_name.keys() {
        if !new_by_name.contains_key(name) {
            diff.removed.insert((*name).to_string());
        }
    }
    for (name, new_scenario) in &new_by_name {
        let Some(old_scenario) = old_by_name.get(*name) else {
            diff.added.insert((*name).to_string());
            continue;
        };
        let topology_diff = crate::diff_timing_topology(old_scenario.netlist, new_scenario.netlist);
        let topology_changed = topology_diff != crate::TimingTopologyDiff::default();
        let constraints_changed = old_scenario.constraints != new_scenario.constraints;
        if topology_changed {
            diff.changed_netlists.insert((*name).to_string());
            diff.topology.insert((*name).to_string(), topology_diff);
        }
        if constraints_changed {
            let constraint_diff =
                diff_timing_constraints(old_scenario.constraints, new_scenario.constraints);
            diff.constraints
                .insert((*name).to_string(), constraint_diff);
        }
        if topology_changed || constraints_changed {
            diff.changed.insert((*name).to_string());
        }
    }
    Ok(diff)
}

/// Incrementally update MMMC scenarios when corners may use different netlists.
///
/// Unchanged corners are reused. Topology-only changes use stable-name ECO
/// remapping. Constraint-only changes use SDC incremental STA. A scenario with
/// both changes is updated in two safe stages: topology remapping under the old
/// SDC first, then SDC constraint incrementality on the new netlist.
pub fn update_timing_scenarios_with_topology_incrementally<'old, 'new>(
    previous: &MultiCornerTimingResult,
    old: &[TimingScenario<'old>],
    new: &[TimingScenario<'new>],
) -> Result<IncrementalMultiCornerTopologyTimingResult, String> {
    let diff = diff_timing_scenario_sets(old, new)?;
    let old_by_name = old
        .iter()
        .map(|scenario| (scenario.name, *scenario))
        .collect::<BTreeMap<_, _>>();
    let previous_by_name = previous
        .corners
        .iter()
        .map(|corner| (corner.scenario.as_str(), corner))
        .collect::<BTreeMap<_, _>>();

    let mut corners = Vec::new();
    let mut updates = BTreeMap::new();
    let mut reused = BTreeSet::new();
    for new_scenario in new {
        let name = new_scenario.name;
        let Some(old_scenario) = old_by_name.get(name) else {
            let timing = analyze_with_sdc(new_scenario.netlist, new_scenario.constraints)?;
            updates.insert(
                name.to_string(),
                MultiCornerScenarioUpdate {
                    kind: MultiCornerScenarioUpdateKind::AddedFullAnalysis,
                    constraint: None,
                    topology: None,
                },
            );
            corners.push(CornerTiming {
                scenario: name.to_string(),
                timing,
            });
            continue;
        };

        let previous_corner = previous_by_name.get(name).ok_or_else(|| {
            format!(
                "incremental MMMC result missing previous scenario '{}'",
                name
            )
        })?;
        let topology_changed = diff.changed_netlists.contains(name);
        let constraints_changed = diff.constraints.contains_key(name);

        let (timing, update) = match (topology_changed, constraints_changed) {
            (false, false) => {
                reused.insert(name.to_string());
                (previous_corner.timing.clone(), None)
            }
            (false, true) => {
                let constraint = crate::update_timing_for_constraints(
                    new_scenario.netlist,
                    &previous_corner.timing.base,
                    old_scenario.constraints,
                    new_scenario.constraints,
                )?;
                (
                    constraint.constrained.clone(),
                    Some(MultiCornerScenarioUpdate {
                        kind: MultiCornerScenarioUpdateKind::ConstraintIncremental,
                        constraint: Some(constraint),
                        topology: None,
                    }),
                )
            }
            (true, false) => {
                let topology = crate::update_timing_for_topology(
                    old_scenario.netlist,
                    &previous_corner.timing.base,
                    new_scenario.netlist,
                    new_scenario.constraints,
                )?;
                let constrained = crate::constraints::analyze_constrained(
                    new_scenario.netlist,
                    topology.timing.clone(),
                    new_scenario.constraints,
                )?;
                (
                    constrained,
                    Some(MultiCornerScenarioUpdate {
                        kind: MultiCornerScenarioUpdateKind::TopologyIncremental,
                        constraint: None,
                        topology: Some(topology),
                    }),
                )
            }
            (true, true) => {
                let topology = crate::update_timing_for_topology(
                    old_scenario.netlist,
                    &previous_corner.timing.base,
                    new_scenario.netlist,
                    old_scenario.constraints,
                )?;
                let constraint = crate::update_timing_for_constraints(
                    new_scenario.netlist,
                    &topology.timing,
                    old_scenario.constraints,
                    new_scenario.constraints,
                )?;
                (
                    constraint.constrained.clone(),
                    Some(MultiCornerScenarioUpdate {
                        kind: MultiCornerScenarioUpdateKind::TopologyAndConstraintIncremental,
                        constraint: Some(constraint),
                        topology: Some(topology),
                    }),
                )
            }
        };
        if let Some(update) = update {
            updates.insert(name.to_string(), update);
        }
        corners.push(CornerTiming {
            scenario: name.to_string(),
            timing,
        });
    }

    if corners.is_empty() {
        return Err("at least one timing scenario is required".into());
    }
    let worst_setup_corner = worst_index(&corners, |corner| {
        corner
            .timing
            .reg2reg_slack
            .min(corner.timing.in2reg_slack)
            .min(corner.timing.output_slack)
            .min(corner.timing.max_delay_slack)
    });
    let worst_hold_corner = worst_index(&corners, |corner| {
        corner
            .timing
            .reg2reg_hold_slack
            .min(corner.timing.in2reg_hold_slack)
            .min(corner.timing.min_delay_slack)
    });
    let worst_slack_corner = worst_index(&corners, |corner| corner.timing.worst_slack);
    Ok(IncrementalMultiCornerTopologyTimingResult {
        result: MultiCornerTimingResult {
            corners,
            worst_setup_corner,
            worst_hold_corner,
            worst_slack_corner,
        },
        diff,
        updates,
        reused,
    })
}

fn resolved_scenario_views<'a, I>(
    result: &MultiCornerTimingResult,
    scenarios: I,
) -> Result<Vec<(usize, TimingScenario<'a>)>, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    let mut by_name = HashMap::new();
    let mut names = BTreeSet::new();
    for scenario in scenarios {
        if scenario.name.trim().is_empty() {
            return Err("timing scenario name cannot be empty".into());
        }
        if !names.insert(scenario.name) {
            return Err(format!("duplicate timing scenario '{}'", scenario.name));
        }
        by_name.insert(scenario.name, scenario);
    }

    let mut out = Vec::with_capacity(result.corners.len());
    for (index, corner) in result.corners.iter().enumerate() {
        let Some(scenario) = by_name.remove(corner.scenario.as_str()) else {
            return Err(format!(
                "timing path view missing scenario '{}'",
                corner.scenario
            ));
        };
        out.push((index, scenario));
    }
    if let Some(scenario) = by_name.keys().next() {
        return Err(format!(
            "timing path view has unknown scenario '{}'",
            scenario
        ));
    }
    Ok(out)
}

fn endpoint_label(endpoint: &crate::TimingPathEndpoint) -> String {
    match endpoint {
        crate::TimingPathEndpoint::SequentialCell(name) => format!("flop {}", name),
        crate::TimingPathEndpoint::OutputPort(name) => format!("output {}", name),
    }
}

fn check_label(check: ResetCheckKind) -> &'static str {
    match check {
        ResetCheckKind::Recovery => "recovery",
        ResetCheckKind::Removal => "removal",
    }
}

fn push_corner_slacks(slacks: &[ScenarioSlack], out: &mut String) {
    let values = slacks
        .iter()
        .map(|slack| {
            format!(
                "{}={}",
                slack.scenario,
                slack
                    .slack_ns
                    .map_or("<excluded>".into(), |value| format!("{:.6}", value))
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!("    scenario slack: {}\n", values));
}

/// Merge constrained setup/output paths by endpoint and choose the worst
/// scenario for each endpoint.
///
/// `max_paths == 0` returns every merged endpoint; otherwise the globally
/// worst endpoints are retained.
pub fn multi_corner_setup_timing_paths<'a, I>(
    result: &MultiCornerTimingResult,
    scenarios: I,
    max_paths: usize,
) -> Result<Vec<MultiCornerSetupTimingPath<'a>>, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    struct Candidate<'a> {
        corner_index: usize,
        scenario: String,
        netlist: &'a Netlist,
        path: crate::ConstrainedTimingPath,
    }

    let views = resolved_scenario_views(result, scenarios)?;
    let mut candidates = Vec::new();
    for (corner_index, view) in views {
        let corner = &result.corners[corner_index];
        for path in constrained_timing_paths(view.netlist, &corner.timing, 0) {
            candidates.push(Candidate {
                corner_index,
                scenario: corner.scenario.clone(),
                netlist: view.netlist,
                path,
            });
        }
    }

    let mut groups: Vec<(crate::TimingPathEndpoint, Vec<Candidate>)> = Vec::new();
    for candidate in candidates {
        let endpoint = candidate.path.path.endpoint.clone();
        if let Some((_, group)) = groups
            .iter_mut()
            .find(|(group_endpoint, _)| *group_endpoint == endpoint)
        {
            group.push(candidate);
        } else {
            groups.push((endpoint, vec![candidate]));
        }
    }

    let mut paths = groups
        .into_iter()
        .map(|(endpoint, group)| {
            let (_, worst) = group
                .iter()
                .enumerate()
                .min_by(|(left_index, left), (right_index, right)| {
                    left.path
                        .slack_ns
                        .total_cmp(&right.path.slack_ns)
                        .then(left_index.cmp(right_index))
                })
                .expect("endpoint group is nonempty");
            let corner_slacks = result
                .corners
                .iter()
                .enumerate()
                .map(|(corner_index, corner)| ScenarioSlack {
                    scenario: corner.scenario.clone(),
                    slack_ns: group
                        .iter()
                        .find(|candidate| candidate.corner_index == corner_index)
                        .map(|candidate| candidate.path.slack_ns),
                })
                .collect();
            MultiCornerSetupTimingPath {
                endpoint,
                scenario: worst.scenario.clone(),
                netlist: worst.netlist,
                path: worst.path.clone(),
                corner_slacks,
            }
        })
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| {
        left.path
            .slack_ns
            .total_cmp(&right.path.slack_ns)
            .then_with(|| endpoint_label(&left.endpoint).cmp(&endpoint_label(&right.endpoint)))
    });
    if max_paths > 0 {
        paths.truncate(max_paths);
    }
    Ok(paths)
}

/// Merge constrained hold paths by sequential endpoint and choose the worst
/// scenario for each endpoint.
pub fn multi_corner_hold_timing_paths<'a, I>(
    result: &MultiCornerTimingResult,
    scenarios: I,
    max_paths: usize,
) -> Result<Vec<MultiCornerHoldTimingPath<'a>>, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    struct Candidate<'a> {
        corner_index: usize,
        scenario: String,
        netlist: &'a Netlist,
        path: crate::ConstrainedHoldTimingPath,
    }

    let views = resolved_scenario_views(result, scenarios)?;
    let mut candidates = Vec::new();
    for (corner_index, view) in views {
        let corner = &result.corners[corner_index];
        for path in constrained_hold_timing_paths(view.netlist, &corner.timing, 0) {
            candidates.push(Candidate {
                corner_index,
                scenario: corner.scenario.clone(),
                netlist: view.netlist,
                path,
            });
        }
    }

    let mut groups: Vec<(crate::TimingPathEndpoint, Vec<Candidate>)> = Vec::new();
    for candidate in candidates {
        let endpoint = candidate.path.path.path.endpoint.clone();
        if let Some((_, group)) = groups
            .iter_mut()
            .find(|(group_endpoint, _)| *group_endpoint == endpoint)
        {
            group.push(candidate);
        } else {
            groups.push((endpoint, vec![candidate]));
        }
    }

    let mut paths = groups
        .into_iter()
        .map(|(endpoint, group)| {
            let (_, worst) = group
                .iter()
                .enumerate()
                .min_by(|(left_index, left), (right_index, right)| {
                    left.path
                        .slack_ns
                        .total_cmp(&right.path.slack_ns)
                        .then(left_index.cmp(right_index))
                })
                .expect("endpoint group is nonempty");
            let corner_slacks = result
                .corners
                .iter()
                .enumerate()
                .map(|(corner_index, corner)| ScenarioSlack {
                    scenario: corner.scenario.clone(),
                    slack_ns: group
                        .iter()
                        .find(|candidate| candidate.corner_index == corner_index)
                        .map(|candidate| candidate.path.slack_ns),
                })
                .collect();
            MultiCornerHoldTimingPath {
                endpoint,
                scenario: worst.scenario.clone(),
                netlist: worst.netlist,
                path: worst.path.clone(),
                corner_slacks,
            }
        })
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| {
        left.path
            .slack_ns
            .total_cmp(&right.path.slack_ns)
            .then_with(|| endpoint_label(&left.endpoint).cmp(&endpoint_label(&right.endpoint)))
    });
    if max_paths > 0 {
        paths.truncate(max_paths);
    }
    Ok(paths)
}

/// Merge constrained recovery/removal paths by endpoint and check, choosing
/// the worst scenario for each check.
pub fn multi_corner_reset_timing_paths<'a, I>(
    result: &MultiCornerTimingResult,
    scenarios: I,
    max_paths: usize,
) -> Result<Vec<MultiCornerResetTimingPath<'a>>, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    struct Candidate<'a> {
        corner_index: usize,
        scenario: String,
        netlist: &'a Netlist,
        path: crate::ConstrainedResetTimingPath,
    }

    let views = resolved_scenario_views(result, scenarios)?;
    let mut candidates = Vec::new();
    for (corner_index, view) in views {
        let corner = &result.corners[corner_index];
        for path in constrained_reset_timing_paths(view.netlist, &corner.timing, 0) {
            candidates.push(Candidate {
                corner_index,
                scenario: corner.scenario.clone(),
                netlist: view.netlist,
                path,
            });
        }
    }

    let mut groups: Vec<((crate::TimingPathEndpoint, ResetCheckKind), Vec<Candidate>)> = Vec::new();
    for candidate in candidates {
        let key = (candidate.path.path.endpoint.clone(), candidate.path.kind);
        if let Some((_, group)) = groups.iter_mut().find(|(group_key, _)| *group_key == key) {
            group.push(candidate);
        } else {
            groups.push((key, vec![candidate]));
        }
    }

    let mut paths = groups
        .into_iter()
        .map(|((endpoint, check), group)| {
            let (_, worst) = group
                .iter()
                .enumerate()
                .min_by(|(left_index, left), (right_index, right)| {
                    left.path
                        .slack_ns
                        .total_cmp(&right.path.slack_ns)
                        .then(left_index.cmp(right_index))
                })
                .expect("endpoint group is nonempty");
            let corner_slacks = result
                .corners
                .iter()
                .enumerate()
                .map(|(corner_index, corner)| ScenarioSlack {
                    scenario: corner.scenario.clone(),
                    slack_ns: group
                        .iter()
                        .find(|candidate| candidate.corner_index == corner_index)
                        .map(|candidate| candidate.path.slack_ns),
                })
                .collect();
            MultiCornerResetTimingPath {
                endpoint,
                check,
                scenario: worst.scenario.clone(),
                netlist: worst.netlist,
                path: worst.path.clone(),
                corner_slacks,
            }
        })
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| {
        left.path
            .slack_ns
            .total_cmp(&right.path.slack_ns)
            .then_with(|| {
                endpoint_label(&left.endpoint)
                    .cmp(&endpoint_label(&right.endpoint))
                    .then(check_label(left.check).cmp(check_label(right.check)))
            })
    });
    if max_paths > 0 {
        paths.truncate(max_paths);
    }
    Ok(paths)
}

/// Parse an inline MMMC manifest.
///
/// Each scenario contains ordinary SDC commands:
///
/// ```text
/// scenario slow
///     create_clock -name clk -period 4.0 [get_ports clk]
/// endscenario
/// ```
pub fn parse_timing_scenarios(source: &str) -> Result<Vec<TimingScenarioDefinition>, String> {
    let mut out = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_sdc = Vec::new();
    let mut current_properties = BTreeMap::new();

    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let mut words = line.split_whitespace();
        let command = words.next().unwrap_or_default();
        if let Some(name) = current_name.as_ref() {
            if command == "endscenario" {
                if words.next().is_some() {
                    return Err(format!("line {}: endscenario takes no name", line_number));
                }
                let constraints = parse_sdc(&current_sdc.join("\n"))
                    .map_err(|e| format!("scenario '{}': {}", name, e))?;
                out.push(TimingScenarioDefinition::with_properties(
                    name.clone(),
                    constraints,
                    current_properties.clone(),
                ));
                current_name = None;
                current_sdc.clear();
                current_properties.clear();
            } else if command == "set_scenario_property" {
                let key = words.next().ok_or_else(|| {
                    format!("line {}: scenario property requires a name", line_number)
                })?;
                let value = words.next().ok_or_else(|| {
                    format!(
                        "line {}: scenario property '{}' requires a value",
                        line_number, key
                    )
                })?;
                if words.next().is_some() {
                    return Err(format!(
                        "line {}: scenario property value cannot contain whitespace",
                        line_number
                    ));
                }
                if current_properties
                    .insert(key.to_string(), value.to_string())
                    .is_some()
                {
                    return Err(format!(
                        "line {}: duplicate scenario property '{}'",
                        line_number, key
                    ));
                }
            } else {
                current_sdc.push(line.to_string());
            }
        } else if command == "scenario" {
            let name = words
                .next()
                .ok_or_else(|| format!("line {}: scenario requires a name", line_number))?;
            if words.next().is_some() {
                return Err(format!(
                    "line {}: scenario name cannot contain whitespace",
                    line_number
                ));
            }
            current_name = Some(name.to_string());
        } else {
            return Err(format!(
                "line {}: expected scenario or endscenario, found '{}'",
                line_number, command
            ));
        }
    }

    if let Some(name) = current_name {
        return Err(format!("scenario '{}' is missing endscenario", name));
    }
    if out.is_empty() {
        return Err("timing scenario manifest contains no scenarios".into());
    }
    let mut names = BTreeSet::new();
    for scenario in &out {
        if !names.insert(scenario.name.as_str()) {
            return Err(format!("duplicate timing scenario '{}'", scenario.name));
        }
    }
    Ok(out)
}

fn worst_index(corners: &[CornerTiming], slack: impl Fn(&CornerTiming) -> f64) -> usize {
    corners
        .iter()
        .enumerate()
        .min_by(|(left_index, left), (right_index, right)| {
            let left_slack = slack(left);
            let right_slack = slack(right);
            left_slack
                .partial_cmp(&right_slack)
                .map(|order| order.then(left_index.cmp(right_index)))
                .unwrap_or(left_index.cmp(right_index))
        })
        .map(|(index, _)| index)
        .expect("corner list is nonempty")
}

pub fn multi_corner_report(result: &MultiCornerTimingResult) -> String {
    let mut out = format!(
        "Multi-corner STA report: {} scenario(s)\n",
        result.corners.len()
    );
    for (index, corner) in result.corners.iter().enumerate() {
        let marker = if index == result.worst_slack_corner {
            " (WORST)"
        } else {
            ""
        };
        let setup_slack = corner
            .timing
            .reg2reg_slack
            .min(corner.timing.in2reg_slack)
            .min(corner.timing.output_slack)
            .min(corner.timing.max_delay_slack);
        let hold_slack = corner
            .timing
            .reg2reg_hold_slack
            .min(corner.timing.in2reg_hold_slack)
            .min(corner.timing.min_delay_slack);
        out.push_str(&format!(
            "scenario '{}'{}: worst slack {:.3} ns, setup {:.3} ns, hold {:.3} ns\n",
            corner.scenario, marker, corner.timing.worst_slack, setup_slack, hold_slack
        ));
    }
    if let Some(scenario) = result.worst_scenario() {
        out.push_str(&format!("worst scenario: {}\n", scenario));
    }
    for corner in &result.corners {
        out.push_str(&format!("[{}]\n", corner.scenario));
        for line in constrained_report(&corner.timing).lines() {
            out.push_str(&format!("  {}\n", line));
        }
    }
    out
}

/// Render merged setup/output paths with their per-scenario slack values.
pub fn multi_corner_setup_timing_path_report(paths: &[MultiCornerSetupTimingPath<'_>]) -> String {
    let mut out = format!(
        "Multi-corner setup timing path report: {} endpoint(s)\n",
        paths.len()
    );
    for path in paths {
        out.push_str(&format!(
            "  {}: worst scenario '{}', slack {:.6} ns\n",
            endpoint_label(&path.endpoint),
            path.scenario,
            path.path.slack_ns
        ));
        push_corner_slacks(&path.corner_slacks, &mut out);
        out.push_str(&constrained_timing_path_report(
            path.netlist,
            std::slice::from_ref(&path.path),
        ));
    }
    out
}

/// Render merged hold paths with their per-scenario slack values.
pub fn multi_corner_hold_timing_path_report(paths: &[MultiCornerHoldTimingPath<'_>]) -> String {
    let mut out = format!(
        "Multi-corner hold timing path report: {} endpoint(s)\n",
        paths.len()
    );
    for path in paths {
        out.push_str(&format!(
            "  {}: worst scenario '{}', hold slack {:.6} ns\n",
            endpoint_label(&path.endpoint),
            path.scenario,
            path.path.slack_ns
        ));
        push_corner_slacks(&path.corner_slacks, &mut out);
        out.push_str(&constrained_hold_timing_path_report(
            path.netlist,
            std::slice::from_ref(&path.path),
        ));
    }
    out
}

/// Render merged recovery/removal paths with per-scenario slack values.
pub fn multi_corner_reset_timing_path_report(paths: &[MultiCornerResetTimingPath<'_>]) -> String {
    let mut out = format!(
        "Multi-corner reset timing path report: {} endpoint/check(s)\n",
        paths.len()
    );
    for path in paths {
        out.push_str(&format!(
            "  {} {}: worst scenario '{}', slack {:.6} ns\n",
            endpoint_label(&path.endpoint),
            check_label(path.check),
            path.scenario,
            path.path.slack_ns
        ));
        push_corner_slacks(&path.corner_slacks, &mut out);
        out.push_str(&constrained_reset_timing_path_report(
            path.netlist,
            std::slice::from_ref(&path.path),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_sdc;
    use eda_core::library::CellLibrary;
    use eda_core::netlist::{Cell, CellKind, Dir};
    use std::sync::Arc;

    fn netlist() -> Netlist {
        let library = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("mcsta");
        nl.set_library(library.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(library.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        nl
    }

    #[test]
    fn aggregates_worst_corner_and_reports_it() {
        let nl = netlist();
        let slow = parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
        "#,
        )
        .unwrap();
        let fast = parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
        "#,
        )
        .unwrap();
        let result = analyze_timing_scenarios([
            TimingScenario::new("slow", &nl, &slow),
            TimingScenario::new("fast", &nl, &fast),
        ])
        .unwrap();
        assert_eq!(result.corners.len(), 2);
        assert_eq!(result.worst_slack_corner, 1);
        assert_eq!(result.worst_scenario(), Some("fast"));
        let report = multi_corner_report(&result);
        assert!(report.contains("scenario 'fast' (WORST)"), "{report}");
    }

    #[test]
    fn merges_worst_setup_path_by_endpoint() {
        let nl = netlist();
        let slow = parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
        "#,
        )
        .unwrap();
        let fast = parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
        "#,
        )
        .unwrap();
        let result = analyze_timing_scenarios([
            TimingScenario::new("slow", &nl, &slow),
            TimingScenario::new("fast", &nl, &fast),
        ])
        .unwrap();
        let paths = multi_corner_setup_timing_paths(
            &result,
            [
                TimingScenario::new("slow", &nl, &slow),
                TimingScenario::new("fast", &nl, &fast),
            ],
            0,
        )
        .unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].scenario, "fast");
        assert_eq!(paths[0].corner_slacks.len(), 2);
        assert_eq!(paths[0].corner_slacks[0].scenario, "slow");
        assert_eq!(paths[0].corner_slacks[1].scenario, "fast");
        assert!(paths[0].corner_slacks[0].slack_ns.unwrap() > paths[0].path.slack_ns);
        let report = multi_corner_setup_timing_path_report(&paths);
        assert!(
            report.contains("Multi-corner setup timing path report: 1 endpoint(s)")
                && report.contains("output y: worst scenario 'fast'")
                && report.contains("scenario slack: slow=2.400000, fast=0.400000")
                && report.contains("arcs:"),
            "{report}"
        );
    }

    #[test]
    fn rejects_empty_and_duplicate_scenarios() {
        let nl = netlist();
        let constraints = parse_sdc("create_clock -name clk -period 1 [get_ports clk]").unwrap();
        assert!(analyze_timing_scenarios(Vec::<TimingScenario>::new()).is_err());
        assert!(analyze_timing_scenarios([TimingScenario::new("", &nl, &constraints),]).is_err());
        assert!(analyze_timing_scenarios([
            TimingScenario::new("same", &nl, &constraints),
            TimingScenario::new("same", &nl, &constraints),
        ])
        .is_err());
    }

    #[test]
    fn parses_inline_timing_scenario_manifest() {
        let definitions = parse_timing_scenarios(
            r#"
            # Mode/corner definitions use inline SDC.
            scenario slow
                create_clock -name clk -period 4.0 [get_ports clk]
                set_input_delay 0.2 -clock clk [get_ports a]
                set_scenario_property post_route.voltage_v 0.90
            endscenario

            scenario fast
                create_clock -name clk -period 2.0 [get_ports clk]
            endscenario
        "#,
        )
        .unwrap();
        assert_eq!(definitions.len(), 2);
        assert_eq!(definitions[0].name, "slow");
        assert_eq!(definitions[0].constraints.clocks["clk"].period, 4.0);
        assert_eq!(definitions[0].properties["post_route.voltage_v"], "0.90");
        assert_eq!(definitions[1].constraints.clocks["clk"].period, 2.0);

        let nl = netlist();
        let result = analyze_timing_scenario_definitions(&nl, &definitions).unwrap();
        assert_eq!(result.worst_scenario(), Some("fast"));
    }

    #[test]
    fn property_only_scenario_change_reuses_timing() {
        let old = parse_timing_scenarios(
            "scenario slow\ncreate_clock -name clk -period 4 [get_ports a]\nset_scenario_property post_route.voltage_v 0.9\nendscenario",
        )
        .unwrap();
        let new = parse_timing_scenarios(
            "scenario slow\ncreate_clock -name clk -period 4 [get_ports a]\nset_scenario_property post_route.voltage_v 0.8\nset_scenario_property post_route.frequency_mhz 200\nendscenario",
        )
        .unwrap();
        let diff = diff_timing_scenarios(&old, &new).unwrap();
        assert!(diff.changed.contains("slow"));
        assert!(!diff.timing_changed.contains("slow"));
        assert_eq!(
            diff.properties["slow"].changed["post_route.voltage_v"],
            ("0.9".to_string(), "0.8".to_string())
        );
        assert!(diff.properties["slow"]
            .added
            .contains("post_route.frequency_mhz"));

        let nl = netlist();
        let previous = analyze_timing_scenario_definitions(&nl, &old).unwrap();
        let updated = update_timing_scenarios_incrementally(&nl, &previous, &old, &new).unwrap();
        assert!(updated.reused.contains("slow"));
        assert!(updated.updates.is_empty());
        let report = timing_scenario_diff_report(&diff);
        assert!(
            report.contains("timing changed []")
                && report.contains("property 'post_route.voltage_v': '0.9' -> '0.8'")
                && report.contains("added properties [post_route.frequency_mhz]"),
            "{report}"
        );
    }

    #[test]
    fn rejects_malformed_timing_scenario_manifests() {
        assert!(parse_timing_scenarios("// empty").is_err());
        assert!(parse_timing_scenarios("scenario a\ncreate_clock").is_err());
        assert!(parse_timing_scenarios("scenario\nendscenario").is_err());
        assert!(parse_timing_scenarios("scenario a\nscenario b\nendscenario").is_err());
        assert!(parse_timing_scenarios(
            "scenario a\ncreate_clock -name c -period 1 [get_ports c]\nendscenario\nscenario a\nendscenario"
        )
        .is_err());
        assert!(parse_timing_scenarios(
            "scenario a\nset_scenario_property x 1\nset_scenario_property x 2\nendscenario"
        )
        .is_err());
    }
}
