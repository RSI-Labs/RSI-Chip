//! Persistent timing-violation tracking and update lifecycle reporting.
//!
//! Violations are keyed by stable scenario/check/endpoint names rather than
//! numeric net or cell IDs. This lets an agent compare timing runs across SDC
//! updates, SDF annotation, and stable-name ECO changes.

use crate::constraints::{
    analyze_with_sdc, AppliedTimingDelayLimit, ConstrainedHoldTimingPath,
    ConstrainedResetTimingPath, ConstrainedTiming, ConstrainedTimingPath, MinPulsePolarity,
    MinPulseWidthCheck, ResetCheckKind, TimingConstraints, TimingObjectPattern, TimingPin,
};
use crate::multicorner::TimingScenario;
use crate::{Netlist, TimingLaunch, TimingPath, TimingPathEndpoint, TimingTopologyDiff};
use std::collections::{BTreeMap, BTreeSet};

/// Timing check associated with a violation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TimingViolationKind {
    Setup,
    Hold,
    Recovery,
    Removal,
    Output,
    MaxDelay,
    MinDelay,
    MinPulseHigh,
    MinPulseLow,
}

impl TimingViolationKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Setup => "setup",
            Self::Hold => "hold",
            Self::Recovery => "recovery",
            Self::Removal => "removal",
            Self::Output => "output",
            Self::MaxDelay => "max_delay",
            Self::MinDelay => "min_delay",
            Self::MinPulseHigh => "min_pulse_high",
            Self::MinPulseLow => "min_pulse_low",
        }
    }
}

/// The concrete path associated with a violation.
#[derive(Clone, Debug)]
pub enum TimingViolationPath {
    Setup(ConstrainedTimingPath),
    Hold(ConstrainedHoldTimingPath),
    Reset(ConstrainedResetTimingPath),
    MinDelay(TimingPath),
    MinPulseWidth(MinPulseWidthCheck),
}

/// One negative-slack timing endpoint.
#[derive(Clone, Debug)]
pub struct TimingViolation {
    /// Stable ID: `<scenario>:<check>:<endpoint-name>`.
    pub id: String,
    pub scenario: String,
    pub kind: TimingViolationKind,
    pub endpoint: String,
    pub launch: String,
    pub slack_ns: f64,
    pub arrival_ns: f64,
    pub required_ns: f64,
    pub cells: Vec<String>,
    pub nets: Vec<String>,
    /// Deepest hierarchy instance owning the endpoint or min-pulse cell.
    pub hierarchy: Option<String>,
    /// Stable instance paths touched by the violating path, in path order.
    pub hierarchy_instances: Vec<String>,
    pub path: TimingViolationPath,
}

/// Slack transition for a violation that remained negative across two runs.
#[derive(Clone, Debug)]
pub struct TimingViolationChange {
    pub id: String,
    pub old_slack_ns: f64,
    pub new_slack_ns: f64,
    pub current: TimingViolation,
}

/// Lifecycle changes between two timing runs.
#[derive(Clone, Debug, Default)]
pub struct TimingViolationDiff {
    pub added: BTreeMap<String, TimingViolation>,
    pub removed: BTreeMap<String, TimingViolation>,
    pub improved: BTreeMap<String, TimingViolationChange>,
    pub degraded: BTreeMap<String, TimingViolationChange>,
    pub unchanged: BTreeMap<String, TimingViolation>,
}

impl TimingViolationDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.improved.is_empty()
            && self.degraded.is_empty()
            && self.unchanged.is_empty()
    }
}

/// A scenario-scoped persistent violation collection.
#[derive(Clone, Debug)]
pub struct TimingViolationStore {
    pub scenario: String,
    pub generation: u64,
    violations: BTreeMap<String, TimingViolation>,
    waivers: BTreeMap<String, TimingViolationWaiver>,
    /// Violation ID to the waiver ID that currently covers it.
    waived: BTreeMap<String, String>,
    audit: Vec<TimingWaiverAuditEntry>,
}

/// Stable-name objects whose timing may have changed.
///
/// A dirty clock conservatively marks every check governed by that clock as
/// dirty. Dirty nets and cells select only violations whose concrete path or
/// min-pulse cell touches those objects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimingViolationDirtyObjects {
    pub clocks: BTreeSet<String>,
    pub nets: BTreeSet<String>,
    pub cells: BTreeSet<String>,
}

impl TimingViolationDirtyObjects {
    pub fn add_clock(&mut self, name: impl Into<String>) -> &mut Self {
        self.clocks.insert(name.into());
        self
    }

    pub fn add_net(&mut self, name: impl Into<String>) -> &mut Self {
        self.nets.insert(name.into());
        self
    }

    pub fn add_cell(&mut self, name: impl Into<String>) -> &mut Self {
        self.cells.insert(name.into());
        self
    }

    pub fn is_empty(&self) -> bool {
        self.clocks.is_empty() && self.nets.is_empty() && self.cells.is_empty()
    }

    fn add_all_clocks<'a, I>(&mut self, clocks: I)
    where
        I: IntoIterator<Item = &'a String>,
    {
        for clock in clocks {
            self.clocks.insert(clock.clone());
        }
    }

    fn add_pins<'a, I>(&mut self, pins: I)
    where
        I: IntoIterator<Item = &'a TimingPin>,
    {
        for pin in pins {
            self.cells.insert(pin.cell.clone());
        }
    }
}

/// Derive stable-name dirty checks from an SDC constraint change.
///
/// Exact endpoint, cell, and pin changes are retained as dirty nets/cells.
/// Broad semantic changes — clock waveform, exceptions, delay limits, case
/// analysis, disabled arcs, and global derates — conservatively dirty every
/// clock in the old or new constraint set.
pub fn timing_violation_dirty_objects_from_constraints(
    old: &TimingConstraints,
    new: &TimingConstraints,
) -> TimingViolationDirtyObjects {
    let diff = crate::diff_timing_constraints(old, new);
    let mut dirty = TimingViolationDirtyObjects::default();
    dirty.clocks.extend(diff.changed_clocks);
    dirty.clocks.extend(diff.changed_clock_uncertainty);
    dirty.clocks.extend(diff.changed_hold_uncertainty);
    dirty.nets.extend(diff.changed_input_ports);
    dirty.nets.extend(diff.changed_output_ports);
    dirty.nets.extend(diff.changed_port_derates);
    dirty.cells.extend(diff.changed_cell_derates);
    dirty.cells.extend(diff.changed_cell_check_derates);
    dirty.add_pins(&diff.changed_pin_derates);
    dirty.add_pins(&diff.changed_pin_check_derates);

    let broad_change = diff.timing_exceptions_changed
        || diff.delay_limits_changed
        || diff.disabled_timing_changed
        || diff.case_analysis_changed
        || diff.global_timing_derate_changed
        || diff.global_check_derate_changed;
    if broad_change {
        dirty.add_all_clocks(old.clocks.keys());
        dirty.add_all_clocks(new.clocks.keys());
    }
    dirty
}

/// Derive stable-name dirty checks from a flat netlist topology change.
pub fn timing_violation_dirty_objects_from_topology(
    diff: &TimingTopologyDiff,
) -> TimingViolationDirtyObjects {
    let mut dirty = TimingViolationDirtyObjects::default();
    dirty.cells.extend(diff.added_cells.iter().cloned());
    dirty.cells.extend(diff.removed_cells.iter().cloned());
    dirty.cells.extend(diff.changed_cells.iter().cloned());
    dirty.nets.extend(diff.added_nets.iter().cloned());
    dirty.nets.extend(diff.removed_nets.iter().cloned());
    dirty.nets.extend(diff.changed_nets.iter().cloned());
    dirty.nets.extend(diff.changed_ports.iter().cloned());
    dirty
}

/// Waiver lifecycle action recorded by a persistent store or registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingWaiverAuditAction {
    Added,
    Removed,
}

impl TimingWaiverAuditAction {
    fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
        }
    }

    fn parse(value: &str, line_number: usize) -> Result<Self, String> {
        match value {
            "added" => Ok(Self::Added),
            "removed" => Ok(Self::Removed),
            other => Err(format!(
                "line {}: unknown timing waiver audit action '{}'",
                line_number, other
            )),
        }
    }
}

/// One immutable waiver audit event.
#[derive(Clone, Debug, PartialEq)]
pub struct TimingWaiverAuditEntry {
    pub sequence: u64,
    pub action: TimingWaiverAuditAction,
    pub waiver_id: String,
    pub generation: u64,
    pub changed_by: Option<String>,
    /// Snapshot of the added or removed rule.
    pub waiver: TimingViolationWaiver,
}

/// Waiver audit events aggregated by hierarchy-instance selector.
#[derive(Clone, Debug)]
pub struct TimingWaiverAuditHierarchySummary {
    /// Exact hierarchy selector, including any wildcards.
    ///
    /// `None` means the waiver rule is not hierarchy scoped.
    pub instance_pattern: Option<String>,
    pub events: usize,
    pub added_events: usize,
    pub removed_events: usize,
    pub active_waiver_ids: BTreeSet<String>,
    pub latest_generation: u64,
}

/// A user-managed waiver rule for timing violations.
///
/// Scenario and endpoint names support `*`, `?`, and `**` wildcard patterns. A
/// violation is waived only when all selectors match and its slack is no worse
/// than `slack_floor_ns`.
#[derive(Clone, Debug, PartialEq)]
pub struct TimingViolationWaiver {
    pub id: String,
    pub scenario_pattern: String,
    pub endpoint_pattern: String,
    /// Optional hierarchy-instance selector backed by the same wildcard syntax.
    pub instance_pattern: Option<String>,
    pub kind: Option<TimingViolationKind>,
    /// Slack floor in ns; only violations with `slack >= floor` are waived.
    pub slack_floor_ns: f64,
    pub owner: Option<String>,
    pub note: Option<String>,
    /// The waiver is invalid after this store generation.
    pub expires_after_generation: Option<u64>,
}

impl TimingViolationWaiver {
    pub fn new(
        id: impl Into<String>,
        scenario_pattern: impl Into<String>,
        endpoint_pattern: impl Into<String>,
        slack_floor_ns: f64,
    ) -> Result<Self, String> {
        let waiver = Self {
            id: id.into(),
            scenario_pattern: scenario_pattern.into(),
            endpoint_pattern: endpoint_pattern.into(),
            instance_pattern: None,
            kind: None,
            slack_floor_ns,
            owner: None,
            note: None,
            expires_after_generation: None,
        };
        waiver.validate()?;
        Ok(waiver)
    }

    pub fn with_kind(mut self, kind: TimingViolationKind) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn with_instance_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.instance_pattern = Some(pattern.into());
        self
    }

    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    pub fn expires_after(mut self, generation: u64) -> Self {
        self.expires_after_generation = Some(generation);
        self
    }

    fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("timing violation waiver ID cannot be empty".into());
        }
        if self.scenario_pattern.trim().is_empty() {
            return Err("timing violation waiver scenario pattern cannot be empty".into());
        }
        if self.endpoint_pattern.trim().is_empty() {
            return Err("timing violation waiver endpoint pattern cannot be empty".into());
        }
        if self
            .instance_pattern
            .as_ref()
            .is_some_and(|pattern| pattern.trim().is_empty())
        {
            return Err("timing violation waiver instance pattern cannot be empty".into());
        }
        if !self.slack_floor_ns.is_finite() {
            return Err("timing violation waiver slack floor must be finite".into());
        }
        if let Some(expiry) = self.expires_after_generation {
            if expiry == 0 {
                return Err("timing violation waiver expiry must be positive".into());
            }
        }
        Ok(())
    }

    fn matches(&self, violation: &TimingViolation, generation: u64) -> bool {
        if let Some(expiry) = self.expires_after_generation {
            if generation > expiry {
                return false;
            }
        }
        (self.kind.is_none() || self.kind == Some(violation.kind))
            && TimingObjectPattern::new(&self.scenario_pattern).matches(&violation.scenario)
            && TimingObjectPattern::new(&self.endpoint_pattern).matches(&violation.endpoint)
            && self.instance_pattern.as_ref().is_none_or(|pattern| {
                violation
                    .hierarchy
                    .as_deref()
                    .is_some_and(|instance| TimingObjectPattern::new(pattern).matches(instance))
                    || violation
                        .hierarchy_instances
                        .iter()
                        .any(|instance| TimingObjectPattern::new(pattern).matches(instance))
            })
            && violation.slack_ns >= self.slack_floor_ns
    }
}

/// One endpoint/check violation aggregated across all scenarios.
#[derive(Clone, Debug)]
pub struct CrossCornerTimingViolation {
    /// Stable cross-corner key: `<check>:<endpoint-name>`.
    pub key: String,
    pub kind: TimingViolationKind,
    pub endpoint: String,
    /// Scenario name to that scenario's worst path for this endpoint/check.
    pub scenarios: BTreeMap<String, TimingViolation>,
    /// Active scenario violations grouped by endpoint hierarchy owner.
    pub hierarchy_summary: BTreeMap<Option<String>, CrossCornerHierarchyViolationSummary>,
    pub worst_scenario: String,
    pub worst_slack_ns: f64,
}

/// Active violations for one endpoint/check grouped by hierarchy owner.
#[derive(Clone, Debug)]
pub struct CrossCornerHierarchyViolationSummary {
    /// Deepest hierarchy instance owning the violating endpoint.
    ///
    /// `None` represents an endpoint owned by the top-level design.
    pub hierarchy: Option<String>,
    /// Number of active scenario violations owned by this hierarchy.
    pub violations: usize,
    pub worst_slack_ns: f64,
    pub worst_scenario: String,
    pub kinds: BTreeMap<TimingViolationKind, usize>,
}

/// Change to a cross-corner endpoint/check summary.
#[derive(Clone, Debug)]
pub struct CrossCornerTimingViolationChange {
    pub key: String,
    pub old_worst_scenario: String,
    pub old_worst_slack_ns: f64,
    pub current: CrossCornerTimingViolation,
}

/// Cross-corner endpoint/check lifecycle between registry updates.
#[derive(Clone, Debug, Default)]
pub struct CrossCornerTimingViolationDiff {
    pub added: BTreeMap<String, CrossCornerTimingViolation>,
    pub removed: BTreeMap<String, CrossCornerTimingViolation>,
    pub improved: BTreeMap<String, CrossCornerTimingViolationChange>,
    pub degraded: BTreeMap<String, CrossCornerTimingViolationChange>,
    pub unchanged: BTreeMap<String, CrossCornerTimingViolation>,
}

/// Result of replacing or updating all scenarios in a violation registry.
#[derive(Clone, Debug, Default)]
pub struct TimingViolationRegistryUpdate {
    pub added_scenarios: BTreeMap<String, TimingViolationStore>,
    pub removed_scenarios: BTreeMap<String, TimingViolationStore>,
    pub scenario_diffs: BTreeMap<String, TimingViolationDiff>,
    pub cross_corner: CrossCornerTimingViolationDiff,
}

/// Persistent violation stores for a complete MMMC scenario set.
#[derive(Clone, Debug, Default)]
pub struct TimingViolationRegistry {
    stores: BTreeMap<String, TimingViolationStore>,
    summaries: BTreeMap<String, CrossCornerTimingViolation>,
    waivers: BTreeMap<String, TimingViolationWaiver>,
    audit: Vec<TimingWaiverAuditEntry>,
    generation: u64,
}

impl TimingViolationStore {
    pub fn new(scenario: impl Into<String>) -> Self {
        Self {
            scenario: scenario.into(),
            generation: 0,
            violations: BTreeMap::new(),
            waivers: BTreeMap::new(),
            waived: BTreeMap::new(),
            audit: Vec::new(),
        }
    }

    pub fn from_timing(
        scenario: impl Into<String>,
        nl: &Netlist,
        timing: &ConstrainedTiming,
    ) -> Result<Self, String> {
        let mut store = Self::new(scenario);
        let violations = collect_violations(&store.scenario, nl, timing)?;
        store.violations = violations;
        store.generation = 1;
        Ok(store)
    }

    pub fn violations(&self) -> &BTreeMap<String, TimingViolation> {
        &self.violations
    }

    /// Violations not covered by an active waiver.
    pub fn active_violations(&self) -> BTreeMap<String, TimingViolation> {
        self.violations
            .iter()
            .filter(|(id, _)| !self.waived.contains_key(id.as_str()))
            .map(|(id, violation)| (id.clone(), violation.clone()))
            .collect()
    }

    /// Violation ID and covering waiver ID pairs.
    pub fn waived_violations(&self) -> &BTreeMap<String, String> {
        &self.waived
    }

    pub fn waivers(&self) -> &BTreeMap<String, TimingViolationWaiver> {
        &self.waivers
    }

    pub fn audit(&self) -> &[TimingWaiverAuditEntry] {
        &self.audit
    }

    /// Restore an audit history and the active waiver set it represents.
    ///
    /// This is a file/session restore operation, not a new add/remove event, so
    /// the timing-run generation is not incremented.
    pub fn restore_waiver_audit(
        &mut self,
        entries: &[TimingWaiverAuditEntry],
    ) -> Result<(), String> {
        validate_waiver_audit(entries)?;
        if let Some(entry) = entries.last() {
            if entry.generation > self.generation {
                return Err(format!(
                    "waiver audit generation {} is newer than store generation {}",
                    entry.generation, self.generation
                ));
            }
        }
        let active = active_waivers_from_audit(entries);
        self.waivers = active;
        self.audit = entries.to_vec();
        self.sync_waivers();
        Ok(())
    }

    pub fn waived_len(&self) -> usize {
        self.waived.len()
    }

    pub fn len(&self) -> usize {
        self.violations.len() - self.waived.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Replace all violations from a new timing run and return lifecycle changes.
    pub fn update(
        &mut self,
        nl: &Netlist,
        timing: &ConstrainedTiming,
    ) -> Result<TimingViolationDiff, String> {
        let next = collect_violations(&self.scenario, nl, timing)?;
        let diff = diff_violations(&self.violations, &next);
        self.violations = next;
        self.generation = self.generation.saturating_add(1);
        self.sync_waivers();
        Ok(diff)
    }

    /// Incrementally replace only timing checks touched by stable-name dirty
    /// objects while retaining untouched violations and their historical slack.
    ///
    /// `timing` may contain a complete new analysis; this method selects the
    /// affected subset rather than replacing every endpoint. A dirty clock is
    /// treated conservatively as affecting every check in the scenario.
    pub fn update_dirty(
        &mut self,
        nl: &Netlist,
        timing: &ConstrainedTiming,
        dirty: &TimingViolationDirtyObjects,
    ) -> Result<TimingViolationDiff, String> {
        if dirty.is_empty() {
            return Ok(TimingViolationDiff::default());
        }
        let all_next = collect_violations(&self.scenario, nl, timing)?;
        let touched = |violation: &TimingViolation| -> bool {
            !dirty.clocks.is_empty()
                || violation.nets.iter().any(|net| dirty.nets.contains(net))
                || violation
                    .cells
                    .iter()
                    .any(|cell| dirty.cells.contains(cell))
        };
        let selected_ids = self
            .violations
            .values()
            .filter(|violation| touched(violation))
            .map(|violation| violation.id.clone())
            .chain(
                all_next
                    .values()
                    .filter(|violation| touched(violation))
                    .map(|violation| violation.id.clone()),
            )
            .collect::<BTreeSet<_>>();
        let old_selected = self
            .violations
            .iter()
            .filter(|(id, _)| selected_ids.contains(id.as_str()))
            .map(|(id, violation)| (id.clone(), violation.clone()))
            .collect::<BTreeMap<_, _>>();
        let new_selected = all_next
            .iter()
            .filter(|(id, _)| selected_ids.contains(id.as_str()))
            .map(|(id, violation)| (id.clone(), violation.clone()))
            .collect::<BTreeMap<_, _>>();
        let diff = diff_violations(&old_selected, &new_selected);
        let mut next = self
            .violations
            .iter()
            .filter(|(id, _)| !selected_ids.contains(id.as_str()))
            .map(|(id, violation)| (id.clone(), violation.clone()))
            .collect::<BTreeMap<_, _>>();
        for (id, violation) in new_selected {
            next.insert(id, violation);
        }
        self.violations = next;
        self.generation = self.generation.saturating_add(1);
        self.sync_waivers();
        Ok(diff)
    }

    pub fn clear(&mut self) {
        self.violations.clear();
        self.waived.clear();
        self.generation = self.generation.saturating_add(1);
    }

    pub fn add_waiver(&mut self, waiver: TimingViolationWaiver) -> Result<(), String> {
        self.add_waiver_with_actor(waiver, None)
    }

    pub fn add_waiver_with_actor(
        &mut self,
        waiver: TimingViolationWaiver,
        changed_by: Option<String>,
    ) -> Result<(), String> {
        waiver.validate()?;
        if self.waivers.contains_key(&waiver.id) {
            return Err(format!("duplicate timing violation waiver '{}'", waiver.id));
        }
        self.waivers.insert(waiver.id.clone(), waiver.clone());
        self.sync_waivers();
        self.record_waiver_audit(TimingWaiverAuditAction::Added, waiver, changed_by);
        Ok(())
    }

    pub fn remove_waiver(&mut self, id: &str) -> Result<(), String> {
        self.remove_waiver_with_actor(id, None)
    }

    pub fn remove_waiver_with_actor(
        &mut self,
        id: &str,
        changed_by: Option<String>,
    ) -> Result<(), String> {
        let Some(waiver) = self.waivers.remove(id) else {
            return Err(format!("timing violation waiver '{}' not found", id));
        };
        self.sync_waivers();
        self.record_waiver_audit(TimingWaiverAuditAction::Removed, waiver, changed_by);
        Ok(())
    }

    fn record_waiver_audit(
        &mut self,
        action: TimingWaiverAuditAction,
        waiver: TimingViolationWaiver,
        changed_by: Option<String>,
    ) {
        let sequence = self.audit.len() as u64 + 1;
        let waiver_id = waiver.id.clone();
        self.audit.push(TimingWaiverAuditEntry {
            sequence,
            action,
            waiver_id,
            generation: self.generation,
            changed_by,
            waiver,
        });
    }

    fn sync_waivers(&mut self) {
        self.waived.clear();
        for (violation_id, violation) in &self.violations {
            let matching = self
                .waivers
                .values()
                .filter(|waiver| waiver.matches(violation, self.generation))
                .min_by_key(|waiver| waiver.id.clone());
            if let Some(waiver) = matching {
                self.waived.insert(violation_id.clone(), waiver.id.clone());
            }
        }
    }
}

impl TimingViolationRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_scenarios<'a, I>(scenarios: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = TimingScenario<'a>>,
    {
        let mut registry = Self::new();
        let update = registry.replace_scenarios(scenarios)?;
        debug_assert!(update.added_scenarios.is_empty() || registry.generation == 1);
        Ok(registry)
    }

    pub fn stores(&self) -> &BTreeMap<String, TimingViolationStore> {
        &self.stores
    }

    pub fn summaries(&self) -> &BTreeMap<String, CrossCornerTimingViolation> {
        &self.summaries
    }

    /// Aggregate all active cross-corner endpoint summaries by hierarchy owner.
    ///
    /// Counts are scenario-level paths: one endpoint failing in two corners
    /// contributes two violations. Waived paths are excluded because the
    /// underlying cross-corner summaries only contain active violations.
    pub fn hierarchy_summaries(
        &self,
    ) -> BTreeMap<Option<String>, CrossCornerHierarchyViolationSummary> {
        let mut out: BTreeMap<Option<String>, CrossCornerHierarchyViolationSummary> =
            BTreeMap::new();
        for endpoint in self.summaries.values() {
            for (hierarchy, summary) in &endpoint.hierarchy_summary {
                match out.entry(hierarchy.clone()) {
                    std::collections::btree_map::Entry::Occupied(mut occupied) => {
                        let merged = occupied.get_mut();
                        merged.violations += summary.violations;
                        for (kind, count) in &summary.kinds {
                            *merged.kinds.entry(*kind).or_default() += count;
                        }
                        let slack_order = summary.worst_slack_ns.total_cmp(&merged.worst_slack_ns);
                        if slack_order == std::cmp::Ordering::Less
                            || (slack_order == std::cmp::Ordering::Equal
                                && summary.worst_scenario < merged.worst_scenario)
                        {
                            merged.worst_slack_ns = summary.worst_slack_ns;
                            merged.worst_scenario = summary.worst_scenario.clone();
                        }
                    }
                    std::collections::btree_map::Entry::Vacant(vacant) => {
                        vacant.insert(summary.clone());
                    }
                }
            }
        }
        out
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn waivers(&self) -> &BTreeMap<String, TimingViolationWaiver> {
        &self.waivers
    }

    pub fn audit(&self) -> &[TimingWaiverAuditEntry] {
        &self.audit
    }

    /// Restore a global MMMC waiver audit across all scenario stores.
    pub fn restore_waiver_audit(
        &mut self,
        entries: &[TimingWaiverAuditEntry],
    ) -> Result<(), String> {
        validate_waiver_audit(entries)?;
        if let Some(entry) = entries.last() {
            if entry.generation > self.generation {
                return Err(format!(
                    "waiver audit generation {} is newer than registry generation {}",
                    entry.generation, self.generation
                ));
            }
        }
        let active = active_waivers_from_audit(entries);
        self.waivers = active.clone();
        self.audit = entries.to_vec();
        for store in self.stores.values_mut() {
            store.waivers = active.clone();
            store.audit = entries.to_vec();
            store.sync_waivers();
        }
        self.summaries = aggregate_cross_corner(&self.stores);
        Ok(())
    }

    pub fn add_waiver(&mut self, waiver: TimingViolationWaiver) -> Result<(), String> {
        self.add_waiver_with_actor(waiver, None)
    }

    pub fn add_waiver_with_actor(
        &mut self,
        waiver: TimingViolationWaiver,
        changed_by: Option<String>,
    ) -> Result<(), String> {
        waiver.validate()?;
        if self.waivers.contains_key(&waiver.id) {
            return Err(format!("duplicate timing violation waiver '{}'", waiver.id));
        }
        let waiver_id = waiver.id.clone();
        self.waivers.insert(waiver.id.clone(), waiver.clone());
        for store in self.stores.values_mut() {
            store.add_waiver_with_actor(waiver.clone(), changed_by.clone())?;
        }
        self.summaries = aggregate_cross_corner(&self.stores);
        self.record_waiver_audit(
            TimingWaiverAuditAction::Added,
            waiver,
            changed_by,
            waiver_id,
        );
        self.generation = self.generation.saturating_add(1);
        Ok(())
    }

    pub fn remove_waiver(&mut self, id: &str) -> Result<(), String> {
        self.remove_waiver_with_actor(id, None)
    }

    pub fn remove_waiver_with_actor(
        &mut self,
        id: &str,
        changed_by: Option<String>,
    ) -> Result<(), String> {
        if self.waivers.remove(id).is_none() {
            return Err(format!("timing violation waiver '{}' not found", id));
        }
        let waiver = self
            .audit
            .iter()
            .rev()
            .find(|entry| entry.waiver_id == id && entry.action == TimingWaiverAuditAction::Added)
            .map(|entry| entry.waiver.clone());
        for store in self.stores.values_mut() {
            store.remove_waiver_with_actor(id, changed_by.clone())?;
        }
        let Some(waiver) = waiver else {
            return Err(format!(
                "timing violation waiver '{}' has no audit snapshot",
                id
            ));
        };
        self.summaries = aggregate_cross_corner(&self.stores);
        self.record_waiver_audit(
            TimingWaiverAuditAction::Removed,
            waiver,
            changed_by,
            id.to_string(),
        );
        self.generation = self.generation.saturating_add(1);
        Ok(())
    }

    fn record_waiver_audit(
        &mut self,
        action: TimingWaiverAuditAction,
        waiver: TimingViolationWaiver,
        changed_by: Option<String>,
        waiver_id: String,
    ) {
        let sequence = self.audit.len() as u64 + 1;
        self.audit.push(TimingWaiverAuditEntry {
            sequence,
            action,
            waiver_id,
            generation: self.generation,
            changed_by,
            waiver,
        });
    }

    pub fn len(&self) -> usize {
        self.stores.values().map(TimingViolationStore::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.stores.values().all(TimingViolationStore::is_empty)
    }

    /// Replace the active scenario set, updating persistent stores by name.
    ///
    /// Existing scenario stores keep their lifecycle history and are updated in
    /// place; newly named scenarios are initialized, and scenarios omitted from
    /// `scenarios` are removed.
    pub fn replace_scenarios<'a, I>(
        &mut self,
        scenarios: I,
    ) -> Result<TimingViolationRegistryUpdate, String>
    where
        I: IntoIterator<Item = TimingScenario<'a>>,
    {
        let mut desired = BTreeMap::new();
        let mut update = TimingViolationRegistryUpdate::default();
        for scenario in scenarios {
            if scenario.name.trim().is_empty() {
                return Err("timing violation scenario name cannot be empty".into());
            }
            if desired.insert(scenario.name, scenario).is_some() {
                return Err(format!(
                    "duplicate timing violation scenario '{}'",
                    scenario.name
                ));
            }
        }

        for (name, store) in &self.stores {
            if !desired.contains_key(name.as_str()) {
                update.removed_scenarios.insert(name.clone(), store.clone());
            }
        }
        for (name, scenario) in desired {
            let timing = analyze_with_sdc(scenario.netlist, scenario.constraints)?;
            if let Some(store) = self.stores.get_mut(name) {
                let diff = store.update(scenario.netlist, &timing)?;
                update.scenario_diffs.insert(name.to_string(), diff);
            } else {
                let mut store = TimingViolationStore::from_timing(name, scenario.netlist, &timing)?;
                for waiver in self.waivers.values() {
                    store.add_waiver(waiver.clone())?;
                }
                update
                    .added_scenarios
                    .insert(name.to_string(), store.clone());
                self.stores.insert(name.to_string(), store);
            }
        }
        for name in update.removed_scenarios.keys() {
            self.stores.remove(name);
        }

        let old_summaries = std::mem::take(&mut self.summaries);
        self.summaries = aggregate_cross_corner(&self.stores);
        update.cross_corner = diff_cross_corner(&old_summaries, &self.summaries);
        self.generation = self.generation.saturating_add(1);
        Ok(update)
    }

    /// Update one scenario without removing other scenario stores.
    pub fn update_scenario(
        &mut self,
        scenario: TimingScenario<'_>,
    ) -> Result<TimingViolationDiff, String> {
        let timing = analyze_with_sdc(scenario.netlist, scenario.constraints)?;
        let diff = if let Some(store) = self.stores.get_mut(scenario.name) {
            store.update(scenario.netlist, &timing)?
        } else {
            let store =
                TimingViolationStore::from_timing(scenario.name, scenario.netlist, &timing)?;
            self.stores.insert(scenario.name.to_string(), store.clone());
            TimingViolationDiff::default()
        };
        let old_summaries = std::mem::take(&mut self.summaries);
        self.summaries = aggregate_cross_corner(&self.stores);
        // The returned per-scenario diff is authoritative; cross-corner diff for
        // a one-scenario update is recomputed but not exposed by this method.
        drop(diff_cross_corner(&old_summaries, &self.summaries));
        self.generation = self.generation.saturating_add(1);
        Ok(diff)
    }

    /// Replace all scenario stores from already-materialized snapshots.
    ///
    /// This is the store-based counterpart to [`Self::replace_scenarios`].
    /// Scenario-local waivers and audit histories are preserved. Registry-level
    /// global waivers are merged into every incoming store unless a store
    /// already has a different waiver with the same ID.
    pub fn replace_stores<I>(&mut self, stores: I) -> Result<TimingViolationRegistryUpdate, String>
    where
        I: IntoIterator<Item = (String, TimingViolationStore)>,
    {
        let mut desired = BTreeMap::new();
        for (name, mut store) in stores {
            if name.trim().is_empty() {
                return Err("timing violation scenario name cannot be empty".into());
            }
            if name != store.scenario {
                return Err(format!(
                    "timing violation store name '{}' does not match scenario '{}'",
                    name, store.scenario
                ));
            }
            for waiver in self.waivers.values() {
                match store.waivers().get(waiver.id.as_str()) {
                    None => store.add_waiver(waiver.clone())?,
                    Some(existing) if *existing != *waiver => {
                        return Err(format!(
                            "scenario '{}' waiver '{}' conflicts with registry waiver",
                            name, waiver.id
                        ));
                    }
                    Some(_) => {}
                }
            }
            if desired.insert(name.clone(), store).is_some() {
                return Err(format!("duplicate timing scenario '{}'", name));
            }
        }
        if desired.is_empty() {
            return Err("at least one timing scenario is required".into());
        }

        let mut update = TimingViolationRegistryUpdate::default();
        for (name, store) in &self.stores {
            if !desired.contains_key(name.as_str()) {
                update.removed_scenarios.insert(name.clone(), store.clone());
            }
        }
        for (name, store) in &desired {
            if let Some(old) = self.stores.get(name.as_str()) {
                let diff = diff_violations(old.violations(), store.violations());
                update.scenario_diffs.insert(name.clone(), diff);
            } else {
                update.added_scenarios.insert(name.clone(), store.clone());
            }
        }

        self.stores = desired;
        let old_summaries = std::mem::take(&mut self.summaries);
        self.summaries = aggregate_cross_corner(&self.stores);
        update.cross_corner = diff_cross_corner(&old_summaries, &self.summaries);
        self.generation = self.generation.saturating_add(1);
        Ok(update)
    }

    /// Update one dirty-list scenario without replacing untouched endpoints.
    ///
    /// Other scenario stores, their lifecycle histories, and their waivers stay
    /// unchanged. Cross-corner summaries are recomputed from all active stores.
    pub fn update_scenario_dirty(
        &mut self,
        scenario: TimingScenario<'_>,
        dirty: &TimingViolationDirtyObjects,
    ) -> Result<TimingViolationDiff, String> {
        let timing = analyze_with_sdc(scenario.netlist, scenario.constraints)?;
        let mut changed = !dirty.is_empty();
        let diff = if let Some(store) = self.stores.get_mut(scenario.name) {
            let diff = store.update_dirty(scenario.netlist, &timing, dirty)?;
            changed |= !diff.is_empty();
            diff
        } else {
            let store =
                TimingViolationStore::from_timing(scenario.name, scenario.netlist, &timing)?;
            self.stores.insert(scenario.name.to_string(), store);
            changed = true;
            TimingViolationDiff::default()
        };
        if !changed {
            return Ok(diff);
        }
        let old_summaries = std::mem::take(&mut self.summaries);
        self.summaries = aggregate_cross_corner(&self.stores);
        drop(diff_cross_corner(&old_summaries, &self.summaries));
        self.generation = self.generation.saturating_add(1);
        Ok(diff)
    }
}

fn endpoint_name(endpoint: &TimingPathEndpoint) -> String {
    match endpoint {
        TimingPathEndpoint::SequentialCell(name) => name.clone(),
        TimingPathEndpoint::OutputPort(name) => name.clone(),
    }
}

fn launch_name(launch: &TimingLaunch) -> String {
    match launch {
        TimingLaunch::PrimaryInput(name) => name.clone(),
        TimingLaunch::SequentialCell(name) => name.clone(),
    }
}

fn insert_worst(out: &mut BTreeMap<String, TimingViolation>, violation: TimingViolation) {
    out.entry(violation.id.clone())
        .and_modify(|old| {
            if violation.slack_ns < old.slack_ns {
                *old = violation.clone();
            }
        })
        .or_insert(violation);
}

fn setup_violation(
    scenario: &str,
    nl: &Netlist,
    timing: &ConstrainedTiming,
    path: ConstrainedTimingPath,
) -> Option<TimingViolation> {
    let endpoint = endpoint_name(&path.path.endpoint);
    let kind = if matches!(path.path.endpoint, TimingPathEndpoint::SequentialCell(_)) {
        TimingViolationKind::Setup
    } else {
        TimingViolationKind::Output
    };
    let endpoint_net = path.path.nets.last()?;
    let endpoint_net_name = nl.net(*endpoint_net).name.clone();
    let base_slack = match kind {
        TimingViolationKind::Setup => timing
            .net_slacks
            .get(&endpoint_net_name)
            .and_then(|slack| slack.setup_ns),
        _ => timing
            .net_slacks
            .get(&endpoint_net_name)
            .and_then(|slack| slack.output_ns),
    };
    let slack_ns = base_slack.filter(|slack| slack.is_finite() && *slack < 0.0)?;
    let mut constrained_path = path.clone();
    constrained_path.slack_ns = slack_ns;
    constrained_path.required_ns = path.path.arrival_ns + slack_ns;
    Some(TimingViolation {
        id: format!("{}:{}:{}", scenario, kind.label(), endpoint),
        scenario: scenario.to_string(),
        kind,
        endpoint: endpoint.clone(),
        launch: launch_name(&path.path.launch),
        slack_ns,
        arrival_ns: path.path.arrival_ns,
        required_ns: constrained_path.required_ns,
        cells: path
            .path
            .cells
            .iter()
            .map(|&cell| nl.cell(cell).name.clone())
            .collect(),
        nets: path
            .path
            .nets
            .iter()
            .map(|&net| nl.net(net).name.clone())
            .collect(),
        hierarchy: hierarchy_annotation(nl, &path.path.cells, &endpoint).0,
        hierarchy_instances: hierarchy_annotation(nl, &path.path.cells, &endpoint).1,
        path: TimingViolationPath::Setup(constrained_path),
    })
}

fn hierarchy_annotation(
    nl: &Netlist,
    cells: &[usize],
    endpoint: &str,
) -> (Option<String>, Vec<String>) {
    let endpoint_cell = cells
        .iter()
        .rev()
        .find(|&&cell| nl.cell(cell).name == endpoint)
        .or_else(|| cells.last());
    let hierarchy = endpoint_cell
        .and_then(|&cell| nl.cell_hierarchy_instance(cell))
        .map(|instance| instance.path.clone())
        .or_else(|| {
            nl.net_by_name(endpoint).and_then(|net| {
                nl.owning_hierarchy_instance(net)
                    .map(|instance| instance.path.clone())
            })
        });
    let mut instances = Vec::new();
    for &cell in cells {
        if let Some(instance) = nl.cell_hierarchy_instance(cell) {
            if !instances.contains(&instance.path) {
                instances.push(instance.path.clone());
            }
        }
    }
    (hierarchy, instances)
}

fn cell_name_hierarchy_annotation(nl: &Netlist, cell_name: &str) -> (Option<String>, Vec<String>) {
    let hierarchy = nl
        .cell_by_name(cell_name)
        .and_then(|cell| nl.cell_hierarchy_instance(cell))
        .map(|instance| instance.path.clone());
    let instances = hierarchy.clone().into_iter().collect();
    (hierarchy, instances)
}

fn max_delay_violation(
    scenario: &str,
    nl: &Netlist,
    path: ConstrainedTimingPath,
    limit: &AppliedTimingDelayLimit,
) -> Option<TimingViolation> {
    if limit.slack_ns >= 0.0 || !limit.slack_ns.is_finite() {
        return None;
    }
    let endpoint = endpoint_name(&path.path.endpoint);
    let mut constrained_path = path.clone();
    constrained_path.slack_ns = limit.slack_ns;
    constrained_path.required_ns = path.path.arrival_ns + limit.slack_ns;
    Some(TimingViolation {
        id: format!("{}:max_delay:{}", scenario, endpoint),
        scenario: scenario.to_string(),
        kind: TimingViolationKind::MaxDelay,
        endpoint: endpoint.clone(),
        launch: launch_name(&path.path.launch),
        slack_ns: limit.slack_ns,
        arrival_ns: path.path.arrival_ns,
        required_ns: constrained_path.required_ns,
        cells: path
            .path
            .cells
            .iter()
            .map(|&cell| nl.cell(cell).name.clone())
            .collect(),
        nets: path
            .path
            .nets
            .iter()
            .map(|&net| nl.net(net).name.clone())
            .collect(),
        hierarchy: hierarchy_annotation(nl, &path.path.cells, &endpoint).0,
        hierarchy_instances: hierarchy_annotation(nl, &path.path.cells, &endpoint).1,
        path: TimingViolationPath::Setup(constrained_path),
    })
}

fn hold_violation(
    scenario: &str,
    nl: &Netlist,
    timing: &ConstrainedTiming,
    path: ConstrainedHoldTimingPath,
) -> Option<TimingViolation> {
    let endpoint = endpoint_name(&path.path.path.endpoint);
    let endpoint_net = path.path.path.nets.last()?;
    let endpoint_net_name = nl.net(*endpoint_net).name.clone();
    let slack_ns = timing
        .net_slacks
        .get(&endpoint_net_name)
        .and_then(|slack| slack.hold_ns)
        .filter(|slack| slack.is_finite() && *slack < 0.0)?;
    let mut constrained_path = path.clone();
    constrained_path.slack_ns = slack_ns;
    constrained_path.required_ns = path.path.path.arrival_ns - slack_ns;
    Some(TimingViolation {
        id: format!("{}:hold:{}", scenario, endpoint),
        scenario: scenario.to_string(),
        kind: TimingViolationKind::Hold,
        endpoint: endpoint.clone(),
        launch: launch_name(&path.path.path.launch),
        slack_ns,
        arrival_ns: path.path.path.arrival_ns,
        required_ns: constrained_path.required_ns,
        cells: path
            .path
            .path
            .cells
            .iter()
            .map(|&cell| nl.cell(cell).name.clone())
            .collect(),
        nets: path
            .path
            .path
            .nets
            .iter()
            .map(|&net| nl.net(net).name.clone())
            .collect(),
        hierarchy: hierarchy_annotation(nl, &path.path.path.cells, &endpoint).0,
        hierarchy_instances: hierarchy_annotation(nl, &path.path.path.cells, &endpoint).1,
        path: TimingViolationPath::Hold(constrained_path),
    })
}

fn min_delay_violation(
    scenario: &str,
    nl: &Netlist,
    path: TimingPath,
    slack_ns: f64,
) -> Option<TimingViolation> {
    if slack_ns >= 0.0 || !slack_ns.is_finite() {
        return None;
    }
    let endpoint = endpoint_name(&path.endpoint);
    let required_ns = path.arrival_ns - slack_ns;
    Some(TimingViolation {
        id: format!("{}:min_delay:{}", scenario, endpoint),
        scenario: scenario.to_string(),
        kind: TimingViolationKind::MinDelay,
        endpoint: endpoint.clone(),
        launch: launch_name(&path.launch),
        slack_ns,
        arrival_ns: path.arrival_ns,
        required_ns,
        cells: path
            .cells
            .iter()
            .map(|&cell| nl.cell(cell).name.clone())
            .collect(),
        nets: path
            .nets
            .iter()
            .map(|&net| nl.net(net).name.clone())
            .collect(),
        hierarchy: hierarchy_annotation(nl, &path.cells, &endpoint).0,
        hierarchy_instances: hierarchy_annotation(nl, &path.cells, &endpoint).1,
        path: TimingViolationPath::MinDelay(path),
    })
}

fn reset_violation(
    scenario: &str,
    nl: &Netlist,
    path: ConstrainedResetTimingPath,
) -> Option<TimingViolation> {
    if path.slack_ns >= 0.0 || !path.slack_ns.is_finite() {
        return None;
    }
    let kind = match path.kind {
        ResetCheckKind::Recovery => TimingViolationKind::Recovery,
        ResetCheckKind::Removal => TimingViolationKind::Removal,
    };
    let endpoint = endpoint_name(&path.path.endpoint);
    Some(TimingViolation {
        id: format!("{}:{}:{}", scenario, kind.label(), endpoint),
        scenario: scenario.to_string(),
        kind,
        endpoint: endpoint.clone(),
        launch: launch_name(&path.path.launch),
        slack_ns: path.slack_ns,
        arrival_ns: path.path.arrival_ns,
        required_ns: path.required_ns,
        cells: path
            .path
            .cells
            .iter()
            .map(|&cell| nl.cell(cell).name.clone())
            .collect(),
        nets: path
            .path
            .nets
            .iter()
            .map(|&net| nl.net(net).name.clone())
            .collect(),
        hierarchy: hierarchy_annotation(nl, &path.path.cells, &endpoint).0,
        hierarchy_instances: hierarchy_annotation(nl, &path.path.cells, &endpoint).1,
        path: TimingViolationPath::Reset(path),
    })
}

fn min_pulse_violation(
    scenario: &str,
    nl: &Netlist,
    check: &MinPulseWidthCheck,
) -> Option<TimingViolation> {
    if check.slack_ns >= 0.0 || !check.slack_ns.is_finite() {
        return None;
    }
    let kind = match check.polarity {
        MinPulsePolarity::High => TimingViolationKind::MinPulseHigh,
        MinPulsePolarity::Low => TimingViolationKind::MinPulseLow,
    };
    Some(TimingViolation {
        id: format!("{}:{}:{}", scenario, kind.label(), check.cell),
        scenario: scenario.to_string(),
        kind,
        endpoint: check.cell.clone(),
        launch: check.clock.clone(),
        slack_ns: check.slack_ns,
        arrival_ns: check.actual_ns,
        required_ns: check.effective_required_ns,
        cells: vec![check.cell.clone()],
        nets: vec![check.clock.clone()],
        hierarchy: cell_name_hierarchy_annotation(nl, &check.cell).0,
        hierarchy_instances: cell_name_hierarchy_annotation(nl, &check.cell).1,
        path: TimingViolationPath::MinPulseWidth(check.clone()),
    })
}

fn collect_violations(
    scenario: &str,
    nl: &Netlist,
    timing: &ConstrainedTiming,
) -> Result<BTreeMap<String, TimingViolation>, String> {
    let mut out = BTreeMap::new();
    for path in crate::constraints::constrained_timing_paths(nl, timing, 0) {
        if let Some(violation) = setup_violation(scenario, nl, timing, path.clone()) {
            insert_worst(&mut out, violation);
        }
        for limit in path.delay_limits.iter().filter(|limit| limit.maximum) {
            if let Some(violation) = max_delay_violation(scenario, nl, path.clone(), limit) {
                insert_worst(&mut out, violation);
            }
        }
    }
    for path in crate::constraints::constrained_hold_timing_paths(nl, timing, 0) {
        if let Some(violation) = hold_violation(scenario, nl, timing, path.clone()) {
            insert_worst(&mut out, violation);
        }
    }
    for path in timing.base.min_timing_paths(nl, 0) {
        let Some(endpoint_net) = path.nets.last() else {
            continue;
        };
        let endpoint_name = nl.net(*endpoint_net).name.clone();
        let Some(slack) = timing
            .net_slacks
            .get(&endpoint_name)
            .and_then(|slack| slack.min_delay_ns)
        else {
            continue;
        };
        if let Some(violation) = min_delay_violation(scenario, nl, path.clone(), slack) {
            insert_worst(&mut out, violation);
        }
    }
    for path in crate::constraints::constrained_reset_timing_paths(nl, timing, 0) {
        if let Some(violation) = reset_violation(scenario, nl, path) {
            insert_worst(&mut out, violation);
        }
    }
    for check in &timing.min_pulse_width_checks {
        if let Some(violation) = min_pulse_violation(scenario, nl, check) {
            insert_worst(&mut out, violation);
        }
    }
    Ok(out)
}

fn diff_violations(
    old: &BTreeMap<String, TimingViolation>,
    new: &BTreeMap<String, TimingViolation>,
) -> TimingViolationDiff {
    let mut diff = TimingViolationDiff::default();
    for (id, violation) in old {
        if let Some(current) = new.get(id) {
            let delta = current.slack_ns - violation.slack_ns;
            if delta < -1e-12 {
                diff.degraded.insert(
                    id.clone(),
                    TimingViolationChange {
                        id: id.clone(),
                        old_slack_ns: violation.slack_ns,
                        new_slack_ns: current.slack_ns,
                        current: current.clone(),
                    },
                );
            } else if delta > 1e-12 {
                diff.improved.insert(
                    id.clone(),
                    TimingViolationChange {
                        id: id.clone(),
                        old_slack_ns: violation.slack_ns,
                        new_slack_ns: current.slack_ns,
                        current: current.clone(),
                    },
                );
            } else {
                diff.unchanged.insert(id.clone(), current.clone());
            }
        } else {
            diff.removed.insert(id.clone(), violation.clone());
        }
    }
    for (id, violation) in new {
        if !old.contains_key(id) {
            diff.added.insert(id.clone(), violation.clone());
        }
    }
    diff
}

fn aggregate_cross_corner(
    stores: &BTreeMap<String, TimingViolationStore>,
) -> BTreeMap<String, CrossCornerTimingViolation> {
    let mut grouped = BTreeMap::<String, Vec<TimingViolation>>::new();
    for store in stores.values() {
        for violation in store.active_violations().values() {
            grouped
                .entry(format!("{}:{}", violation.kind.label(), violation.endpoint))
                .or_default()
                .push(violation.clone());
        }
    }

    let mut out = BTreeMap::new();
    for (key, mut violations) in grouped {
        violations.sort_by(|left, right| {
            left.slack_ns
                .total_cmp(&right.slack_ns)
                .then(left.scenario.cmp(&right.scenario))
        });
        let Some(worst) = violations.first().cloned() else {
            continue;
        };
        let hierarchy_summary = aggregate_cross_corner_hierarchies(&violations);
        let summary = CrossCornerTimingViolation {
            key: key.clone(),
            kind: worst.kind,
            endpoint: worst.endpoint.clone(),
            scenarios: violations
                .into_iter()
                .map(|violation| (violation.scenario.clone(), violation))
                .collect(),
            hierarchy_summary,
            worst_scenario: worst.scenario.clone(),
            worst_slack_ns: worst.slack_ns,
        };
        out.insert(key, summary);
    }
    out
}

fn aggregate_cross_corner_hierarchies(
    violations: &[TimingViolation],
) -> BTreeMap<Option<String>, CrossCornerHierarchyViolationSummary> {
    let mut out = BTreeMap::new();
    // The input is sorted by worst slack and then scenario name. Preserving
    // that order while grouping makes the first owner occurrence the worst
    // path even when several scenarios have identical slack.
    for violation in violations {
        let hierarchy = violation.hierarchy.clone();
        let summary =
            out.entry(hierarchy.clone())
                .or_insert_with(|| CrossCornerHierarchyViolationSummary {
                    hierarchy: hierarchy.clone(),
                    violations: 0,
                    worst_slack_ns: violation.slack_ns,
                    worst_scenario: violation.scenario.clone(),
                    kinds: BTreeMap::new(),
                });
        summary.violations += 1;
        *summary.kinds.entry(violation.kind).or_default() += 1;
    }
    out
}

fn diff_cross_corner(
    old: &BTreeMap<String, CrossCornerTimingViolation>,
    new: &BTreeMap<String, CrossCornerTimingViolation>,
) -> CrossCornerTimingViolationDiff {
    let mut diff = CrossCornerTimingViolationDiff::default();
    for (key, old_summary) in old {
        if let Some(current) = new.get(key) {
            let delta = current.worst_slack_ns - old_summary.worst_slack_ns;
            let change = CrossCornerTimingViolationChange {
                key: key.clone(),
                old_worst_scenario: old_summary.worst_scenario.clone(),
                old_worst_slack_ns: old_summary.worst_slack_ns,
                current: current.clone(),
            };
            if delta < -1e-12 {
                diff.degraded.insert(key.clone(), change);
            } else if delta > 1e-12 {
                diff.improved.insert(key.clone(), change);
            } else {
                diff.unchanged.insert(key.clone(), current.clone());
            }
        } else {
            diff.removed.insert(key.clone(), old_summary.clone());
        }
    }
    for (key, summary) in new {
        if !old.contains_key(key) {
            diff.added.insert(key.clone(), summary.clone());
        }
    }
    diff
}

fn quote_waiver_value(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

fn tokenize_waiver_line(line: &str, line_number: usize) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&current) = chars.peek() {
        if current.is_whitespace() {
            chars.next();
            continue;
        }
        if current == '#' {
            break;
        }
        if current == '"' {
            chars.next();
            let mut value = String::new();
            let mut closed = false;
            while let Some(character) = chars.next() {
                match character {
                    '"' => {
                        closed = true;
                        break;
                    }
                    '\\' => {
                        let Some(escaped) = chars.next() else {
                            return Err(format!(
                                "line {}: unterminated waiver escape",
                                line_number
                            ));
                        };
                        match escaped {
                            'n' => value.push('\n'),
                            '\\' => value.push('\\'),
                            '"' => value.push('"'),
                            other => {
                                value.push('\\');
                                value.push(other);
                            }
                        }
                    }
                    other => value.push(other),
                }
            }
            if !closed {
                return Err(format!("line {}: unterminated waiver string", line_number));
            }
            tokens.push(value);
        } else {
            let mut value = String::new();
            while let Some(&character) = chars.peek() {
                if character.is_whitespace() || character == '#' {
                    break;
                }
                value.push(character);
                chars.next();
            }
            tokens.push(value);
        }
    }
    Ok(tokens)
}

fn parse_waiver_kind(value: &str, line_number: usize) -> Result<TimingViolationKind, String> {
    match value {
        "setup" => Ok(TimingViolationKind::Setup),
        "hold" => Ok(TimingViolationKind::Hold),
        "recovery" => Ok(TimingViolationKind::Recovery),
        "removal" => Ok(TimingViolationKind::Removal),
        "output" => Ok(TimingViolationKind::Output),
        "max_delay" => Ok(TimingViolationKind::MaxDelay),
        "min_delay" => Ok(TimingViolationKind::MinDelay),
        "min_pulse_high" => Ok(TimingViolationKind::MinPulseHigh),
        "min_pulse_low" => Ok(TimingViolationKind::MinPulseLow),
        other => Err(format!(
            "line {}: unknown timing violation kind '{}'",
            line_number, other
        )),
    }
}

fn validate_waiver_audit(entries: &[TimingWaiverAuditEntry]) -> Result<(), String> {
    let mut next_action = BTreeMap::<String, TimingWaiverAuditAction>::new();
    let mut last_generation = 0u64;
    for (index, entry) in entries.iter().enumerate() {
        let expected_sequence = index as u64 + 1;
        if entry.sequence != expected_sequence {
            return Err(format!(
                "waiver audit sequence {} at index {} should be {}",
                entry.sequence, index, expected_sequence
            ));
        }
        if entry.generation < last_generation {
            return Err(format!(
                "waiver audit generation {} is older than prior generation {}",
                entry.generation, last_generation
            ));
        }
        last_generation = entry.generation;
        if entry.waiver.id != entry.waiver_id {
            return Err(format!(
                "waiver audit entry #{} snapshot ID '{}' does not match event ID '{}'",
                entry.sequence, entry.waiver.id, entry.waiver_id
            ));
        }
        entry
            .waiver
            .validate()
            .map_err(|error| format!("waiver audit entry #{}: {}", entry.sequence, error))?;
        let expected = next_action.remove(&entry.waiver_id);
        let valid = matches!(
            (&expected, entry.action),
            (None, TimingWaiverAuditAction::Added)
                | (
                    Some(TimingWaiverAuditAction::Removed),
                    TimingWaiverAuditAction::Added
                )
                | (
                    Some(TimingWaiverAuditAction::Added),
                    TimingWaiverAuditAction::Removed
                )
        );
        if !valid {
            return Err(format!(
                "waiver audit event #{} has invalid action transition for '{}'",
                entry.sequence, entry.waiver_id
            ));
        }
        next_action.insert(entry.waiver_id.clone(), entry.action);
    }
    Ok(())
}

fn active_waivers_from_audit(
    entries: &[TimingWaiverAuditEntry],
) -> BTreeMap<String, TimingViolationWaiver> {
    let mut active = BTreeMap::new();
    for entry in entries {
        match entry.action {
            TimingWaiverAuditAction::Added => {
                active.insert(entry.waiver_id.clone(), entry.waiver.clone());
            }
            TimingWaiverAuditAction::Removed => {
                active.remove(&entry.waiver_id);
            }
        }
    }
    active
}

/// Serialize waiver rules to a deterministic, dependency-free text format.
pub fn write_timing_violation_waivers(waivers: &[TimingViolationWaiver]) -> Result<String, String> {
    let mut ids = BTreeSet::new();
    let mut out = String::new();
    for waiver in waivers {
        waiver.validate()?;
        if !ids.insert(waiver.id.as_str()) {
            return Err(format!("duplicate timing violation waiver '{}'", waiver.id));
        }
        out.push_str(&format!(
            "waiver {} {} {} {}\n",
            quote_waiver_value(&waiver.id),
            quote_waiver_value(&waiver.scenario_pattern),
            quote_waiver_value(&waiver.endpoint_pattern),
            quote_waiver_value(&waiver.slack_floor_ns.to_string())
        ));
        if let Some(kind) = waiver.kind {
            out.push_str(&format!("kind {}\n", kind.label()));
        }
        if let Some(instance) = &waiver.instance_pattern {
            out.push_str(&format!("instance {}\n", quote_waiver_value(instance)));
        }
        if let Some(owner) = &waiver.owner {
            out.push_str(&format!("owner {}\n", quote_waiver_value(owner)));
        }
        if let Some(note) = &waiver.note {
            out.push_str(&format!("note {}\n", quote_waiver_value(note)));
        }
        if let Some(expiry) = waiver.expires_after_generation {
            out.push_str(&format!("expires_after {}\n", expiry));
        }
    }
    Ok(out)
}

/// Parse the text emitted by [`write_timing_violation_waivers`].
pub fn parse_timing_violation_waivers(source: &str) -> Result<Vec<TimingViolationWaiver>, String> {
    struct Partial {
        id: String,
        scenario: String,
        endpoint: String,
        instance: Option<String>,
        slack_floor: f64,
        kind: Option<TimingViolationKind>,
        owner: Option<String>,
        note: Option<String>,
        expiry: Option<u64>,
    }

    fn finish(
        partial: Partial,
        line_number: usize,
        ids: &mut BTreeSet<String>,
    ) -> Result<TimingViolationWaiver, String> {
        let mut waiver = TimingViolationWaiver::new(
            partial.id,
            partial.scenario,
            partial.endpoint,
            partial.slack_floor,
        )
        .map_err(|error| format!("line {}: {}", line_number, error))?;
        if let Some(kind) = partial.kind {
            waiver = waiver.with_kind(kind);
        }
        if let Some(instance) = partial.instance {
            waiver = waiver.with_instance_pattern(instance);
        }
        if let Some(owner) = partial.owner {
            waiver = waiver.with_owner(owner);
        }
        if let Some(note) = partial.note {
            waiver = waiver.with_note(note);
        }
        if let Some(expiry) = partial.expiry {
            waiver = waiver.expires_after(expiry);
        }
        waiver
            .validate()
            .map_err(|error| format!("line {}: {}", line_number, error))?;
        if !ids.insert(waiver.id.clone()) {
            return Err(format!(
                "line {}: duplicate timing violation waiver '{}'",
                line_number, waiver.id
            ));
        }
        Ok(waiver)
    }

    let mut out = Vec::new();
    let mut ids = BTreeSet::new();
    let mut current: Option<Partial> = None;
    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let tokens = tokenize_waiver_line(raw, line_number)?;
        if tokens.is_empty() {
            continue;
        }
        match tokens[0].as_str() {
            "waiver" => {
                if let Some(partial) = current.take() {
                    out.push(finish(partial, line_number.saturating_sub(1), &mut ids)?);
                }
                if tokens.len() != 5 {
                    return Err(format!(
                        "line {}: waiver requires ID, scenario, endpoint, and slack floor",
                        line_number
                    ));
                }
                let slack_floor = tokens[4]
                    .parse::<f64>()
                    .map_err(|_| format!("line {}: invalid waiver slack floor", line_number))?;
                current = Some(Partial {
                    id: tokens[1].clone(),
                    scenario: tokens[2].clone(),
                    endpoint: tokens[3].clone(),
                    instance: None,
                    slack_floor,
                    kind: None,
                    owner: None,
                    note: None,
                    expiry: None,
                });
            }
            "kind" => {
                let Some(partial) = current.as_mut() else {
                    return Err(format!("line {}: kind appears before waiver", line_number));
                };
                if tokens.len() != 2 || partial.kind.is_some() {
                    return Err(format!("line {}: malformed or duplicate kind", line_number));
                }
                partial.kind = Some(parse_waiver_kind(&tokens[1], line_number)?);
            }
            "instance" => {
                let Some(partial) = current.as_mut() else {
                    return Err(format!(
                        "line {}: instance appears before waiver",
                        line_number
                    ));
                };
                if tokens.len() != 2 || partial.instance.is_some() {
                    return Err(format!(
                        "line {}: malformed or duplicate instance",
                        line_number
                    ));
                }
                partial.instance = Some(tokens[1].clone());
            }
            "owner" => {
                let Some(partial) = current.as_mut() else {
                    return Err(format!("line {}: owner appears before waiver", line_number));
                };
                if tokens.len() != 2 || partial.owner.is_some() {
                    return Err(format!(
                        "line {}: malformed or duplicate owner",
                        line_number
                    ));
                }
                partial.owner = Some(tokens[1].clone());
            }
            "note" => {
                let Some(partial) = current.as_mut() else {
                    return Err(format!("line {}: note appears before waiver", line_number));
                };
                if tokens.len() != 2 || partial.note.is_some() {
                    return Err(format!("line {}: malformed or duplicate note", line_number));
                }
                partial.note = Some(tokens[1].clone());
            }
            "expires_after" => {
                let Some(partial) = current.as_mut() else {
                    return Err(format!(
                        "line {}: expires_after appears before waiver",
                        line_number
                    ));
                };
                if tokens.len() != 2 || partial.expiry.is_some() {
                    return Err(format!(
                        "line {}: malformed or duplicate expires_after",
                        line_number
                    ));
                }
                partial.expiry = Some(
                    tokens[1]
                        .parse::<u64>()
                        .map_err(|_| format!("line {}: invalid waiver expiry", line_number))?,
                );
            }
            other => {
                return Err(format!(
                    "line {}: unknown timing waiver token '{}'",
                    line_number, other
                ));
            }
        }
    }
    if let Some(partial) = current.take() {
        let line_number = source.lines().count();
        out.push(finish(partial, line_number, &mut ids)?);
    }
    Ok(out)
}

/// Serialize immutable waiver audit events and their complete rule snapshots.
pub fn write_timing_waiver_audit(entries: &[TimingWaiverAuditEntry]) -> Result<String, String> {
    validate_waiver_audit(entries)?;
    let mut out = String::new();
    for entry in entries {
        let actor = entry.changed_by.as_deref().unwrap_or_default();
        out.push_str(&format!(
            "audit {} {} {} {}\n",
            entry.sequence,
            entry.action.label(),
            entry.generation,
            quote_waiver_value(actor)
        ));
        out.push_str(&write_timing_violation_waivers(std::slice::from_ref(
            &entry.waiver,
        ))?);
        out.push_str("end\n");
    }
    Ok(out)
}

/// Parse the text emitted by [`write_timing_waiver_audit`].
pub fn parse_timing_waiver_audit(source: &str) -> Result<Vec<TimingWaiverAuditEntry>, String> {
    struct Header {
        sequence: u64,
        action: TimingWaiverAuditAction,
        generation: u64,
        changed_by: Option<String>,
    }

    let mut entries = Vec::new();
    let mut current_header: Option<Header> = None;
    let mut block = String::new();
    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let tokens = tokenize_waiver_line(raw, line_number)?;
        if tokens.is_empty() {
            continue;
        }
        match tokens[0].as_str() {
            "audit" => {
                if current_header.is_some() {
                    return Err(format!(
                        "line {}: previous timing waiver audit block is not closed",
                        line_number
                    ));
                }
                if tokens.len() != 5 {
                    return Err(format!(
                        "line {}: audit requires sequence, action, generation, and actor",
                        line_number
                    ));
                }
                let sequence = tokens[1]
                    .parse::<u64>()
                    .map_err(|_| format!("line {}: invalid audit sequence", line_number))?;
                let action = TimingWaiverAuditAction::parse(&tokens[2], line_number)?;
                let generation = tokens[3]
                    .parse::<u64>()
                    .map_err(|_| format!("line {}: invalid audit generation", line_number))?;
                current_header = Some(Header {
                    sequence,
                    action,
                    generation,
                    changed_by: (!tokens[4].is_empty()).then(|| tokens[4].clone()),
                });
                block.clear();
            }
            "end" => {
                let Some(header) = current_header.take() else {
                    return Err(format!(
                        "line {}: end appears outside a timing waiver audit block",
                        line_number
                    ));
                };
                if tokens.len() != 1 {
                    return Err(format!("line {}: malformed audit end", line_number));
                }
                let mut waivers = parse_timing_violation_waivers(&block)
                    .map_err(|error| format!("line {}: {}", line_number, error))?;
                if waivers.len() != 1 {
                    return Err(format!(
                        "line {}: audit block must contain exactly one waiver",
                        line_number
                    ));
                }
                let waiver = waivers.remove(0);
                entries.push(TimingWaiverAuditEntry {
                    sequence: header.sequence,
                    action: header.action,
                    waiver_id: waiver.id.clone(),
                    generation: header.generation,
                    changed_by: header.changed_by,
                    waiver,
                });
            }
            _ => {
                if current_header.is_some() {
                    block.push_str(raw);
                    block.push('\n');
                } else {
                    return Err(format!(
                        "line {}: token '{}' appears outside a timing waiver audit block",
                        line_number, tokens[0]
                    ));
                }
            }
        }
    }
    if current_header.is_some() {
        return Err("unterminated timing waiver audit block".into());
    }
    validate_waiver_audit(&entries)?;
    Ok(entries)
}

/// Replay audit events and return the final active waiver set.
pub fn active_timing_waivers_from_audit(
    entries: &[TimingWaiverAuditEntry],
) -> Result<Vec<TimingViolationWaiver>, String> {
    validate_waiver_audit(entries)?;
    Ok(active_waivers_from_audit(entries).into_values().collect())
}

/// Aggregate waiver audit events by their hierarchy-instance selector.
pub fn timing_waiver_audit_hierarchy_summaries(
    entries: &[TimingWaiverAuditEntry],
) -> Result<BTreeMap<Option<String>, TimingWaiverAuditHierarchySummary>, String> {
    validate_waiver_audit(entries)?;
    let mut out = BTreeMap::new();
    for entry in entries {
        let instance_pattern = entry.waiver.instance_pattern.clone();
        let summary = out.entry(instance_pattern.clone()).or_insert_with(|| {
            TimingWaiverAuditHierarchySummary {
                instance_pattern,
                events: 0,
                added_events: 0,
                removed_events: 0,
                active_waiver_ids: BTreeSet::new(),
                latest_generation: 0,
            }
        });
        summary.events += 1;
        match entry.action {
            TimingWaiverAuditAction::Added => summary.added_events += 1,
            TimingWaiverAuditAction::Removed => summary.removed_events += 1,
        }
        summary.latest_generation = summary.latest_generation.max(entry.generation);
    }
    for (waiver_id, waiver) in active_waivers_from_audit(entries) {
        let summary = out
            .entry(waiver.instance_pattern.clone())
            .or_insert_with(|| TimingWaiverAuditHierarchySummary {
                instance_pattern: waiver.instance_pattern.clone(),
                events: 0,
                added_events: 0,
                removed_events: 0,
                active_waiver_ids: BTreeSet::new(),
                latest_generation: 0,
            });
        summary.active_waiver_ids.insert(waiver_id);
    }
    Ok(out)
}

/// Render the hierarchy-scoped distribution of waiver audit events.
pub fn timing_waiver_audit_hierarchy_report(
    entries: &[TimingWaiverAuditEntry],
) -> Result<String, String> {
    let summaries = timing_waiver_audit_hierarchy_summaries(entries)?;
    let mut out = format!(
        "Timing waiver audit hierarchy distribution: {} instance scope(s)\n",
        summaries.len()
    );
    for summary in summaries.values() {
        let instance = summary.instance_pattern.as_deref().unwrap_or("(unscoped)");
        let active_ids = if summary.active_waiver_ids.is_empty() {
            "<none>".to_string()
        } else {
            summary
                .active_waiver_ids
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",")
        };
        out.push_str(&format!(
            "  '{}': events={}, added={}, removed={}, active={}, latest_generation={}, active_ids={}\n",
            instance,
            summary.events,
            summary.added_events,
            summary.removed_events,
            summary.active_waiver_ids.len(),
            summary.latest_generation,
            active_ids
        ));
    }
    Ok(out)
}

/// Render immutable waiver add/remove audit entries.
pub fn timing_waiver_audit_report(entries: &[TimingWaiverAuditEntry]) -> String {
    let mut out = format!("Timing waiver audit: {} event(s)\n", entries.len());
    for entry in entries {
        out.push_str(&format!(
            "  #{} {} '{}' at generation {} by {}\n",
            entry.sequence,
            entry.action.label(),
            entry.waiver_id,
            entry.generation,
            entry.changed_by.as_deref().unwrap_or("<unknown>")
        ));
    }
    out
}

/// Render a multi-scenario violation registry and optional lifecycle update.
pub fn timing_violation_registry_report(
    registry: &TimingViolationRegistry,
    update: Option<&TimingViolationRegistryUpdate>,
) -> String {
    let mut out = format!(
        "Multi-corner timing violation report: {} scenario(s), {} violation(s), generation {}\n",
        registry.stores.len(),
        registry.len(),
        registry.generation
    );
    for summary in registry.summaries.values() {
        out.push_str(&format!(
            "  {} at {}: worst scenario '{}' slack {:.6} ns, violating corners={}\n",
            summary.kind.label(),
            summary.endpoint,
            summary.worst_scenario,
            summary.worst_slack_ns,
            summary.scenarios.len()
        ));
    }
    let hierarchy_summaries = registry.hierarchy_summaries();
    if !hierarchy_summaries.is_empty() {
        out.push_str("  hierarchy distribution:\n");
        for summary in hierarchy_summaries.values() {
            let hierarchy = summary.hierarchy.as_deref().unwrap_or("(top)");
            let kinds = summary
                .kinds
                .iter()
                .map(|(kind, count)| format!("{}:{}", kind.label(), count))
                .collect::<Vec<_>>()
                .join(",");
            out.push_str(&format!(
                "    '{}': violations={}, worst scenario '{}' slack {:.6} ns, kinds={}\n",
                hierarchy,
                summary.violations,
                summary.worst_scenario,
                summary.worst_slack_ns,
                kinds
            ));
        }
    }
    let waived_count = registry
        .stores
        .values()
        .map(TimingViolationStore::waived_len)
        .sum::<usize>();
    out.push_str(&format!("  waived violations: {}\n", waived_count));
    if let Some(update) = update {
        out.push_str(&format!(
            "  scenarios: added={}, removed={}, updated={}\n",
            update.added_scenarios.len(),
            update.removed_scenarios.len(),
            update.scenario_diffs.len()
        ));
        out.push_str(&format!(
            "  cross-corner: added={}, removed={}, improved={}, degraded={}, unchanged={}\n",
            update.cross_corner.added.len(),
            update.cross_corner.removed.len(),
            update.cross_corner.improved.len(),
            update.cross_corner.degraded.len(),
            update.cross_corner.unchanged.len()
        ));
    }
    out
}

/// Build a registry snapshot from already-materialized scenario stores.
///
/// Store-local waivers and their audit histories are preserved. The
/// registry-level global waiver set starts empty; this constructor is intended
/// for flows that own per-scenario waiver state rather than a single global
/// MMMC waiver policy.
pub fn timing_violation_registry_from_stores<I>(
    stores: I,
) -> Result<TimingViolationRegistry, String>
where
    I: IntoIterator<Item = (String, TimingViolationStore)>,
{
    let mut materialized = BTreeMap::new();
    let mut generation = 0_u64;
    for (name, store) in stores {
        if name.trim().is_empty() {
            return Err("timing violation scenario name cannot be empty".into());
        }
        if name != store.scenario {
            return Err(format!(
                "timing violation store name '{}' does not match scenario '{}'",
                name, store.scenario
            ));
        }
        if materialized.insert(name.clone(), store.clone()).is_some() {
            return Err(format!("duplicate timing scenario '{}'", name));
        }
        generation = generation.max(store.generation);
    }
    if materialized.is_empty() {
        return Err("at least one timing scenario is required".into());
    }
    let summaries = aggregate_cross_corner(&materialized);
    Ok(TimingViolationRegistry {
        stores: materialized,
        summaries,
        waivers: BTreeMap::new(),
        audit: Vec::new(),
        generation,
    })
}

/// Render a persistent violation store and optional lifecycle diff.
pub fn timing_violation_report(
    store: &TimingViolationStore,
    diff: Option<&TimingViolationDiff>,
) -> String {
    let mut out = format!(
        "Timing violation report for '{}': {} violation(s), generation {}\n",
        store.scenario,
        store.len(),
        store.generation
    );
    let active = store.active_violations();
    for violation in active.values() {
        out.push_str(&format!(
            "  {} {} at {} from {}: slack {:.6} ns, arrival {:.6} ns, required {:.6} ns\n",
            violation.kind.label(),
            violation.id,
            violation.endpoint,
            violation.launch,
            violation.slack_ns,
            violation.arrival_ns,
            violation.required_ns
        ));
        if let Some(instance) = &violation.hierarchy {
            out.push_str(&format!("    hierarchy: {}\n", instance));
        }
        if !violation.hierarchy_instances.is_empty() {
            out.push_str(&format!(
                "    hierarchy path: {}\n",
                violation.hierarchy_instances.join(" -> ")
            ));
        }
    }
    out.push_str(&format!("  waived violations: {}\n", store.waived_len()));
    if let Some(diff) = diff {
        out.push_str(&format!(
            "  lifecycle: added={}, removed={}, improved={}, degraded={}, unchanged={}\n",
            diff.added.len(),
            diff.removed.len(),
            diff.improved.len(),
            diff.degraded.len(),
            diff.unchanged.len()
        ));
        for change in diff.degraded.values() {
            out.push_str(&format!(
                "    degraded {} from {:.6} to {:.6} ns\n",
                change.id, change.old_slack_ns, change.new_slack_ns
            ));
        }
        for change in diff.improved.values() {
            out.push_str(&format!(
                "    improved {} from {:.6} to {:.6} ns\n",
                change.id, change.old_slack_ns, change.new_slack_ns
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::library::CellLibrary;
    use eda_core::netlist::{Cell, CellKind, Dir, NetlistHierarchyInstance, NetlistHierarchyPort};
    use std::sync::Arc;

    fn two_flop_netlist() -> Netlist {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("min-pulse");
        nl.set_library(lib.clone());
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let q1 = nl.add_net("q1");
        let q2 = nl.add_net("q2");
        let dff = lib.find("DFF").unwrap();
        nl.add_cell(Cell {
            name: "launch".into(),
            kind: CellKind::Lib(dff),
            inputs: vec![clk, d],
            output: q1,
        });
        nl.add_cell(Cell {
            name: "capture".into(),
            kind: CellKind::Lib(dff),
            inputs: vec![clk, q1],
            output: q2,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("q2", Dir::Out, vec![q2]);
        nl
    }

    fn and_netlist() -> Netlist {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("delay-limits");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        let z = nl.add_net("z");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_cell(Cell {
            name: "u2".into(),
            kind: CellKind::Lib(lib.find("OR2").unwrap()),
            inputs: vec![a, b],
            output: z,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        nl.add_port("z", Dir::Out, vec![z]);
        nl
    }

    #[test]
    fn tracks_explicit_max_and_min_delay_violations() {
        let nl = and_netlist();
        let constraints = crate::parse_sdc(
            r#"
            create_clock -name clk -period 3.0 [get_ports clk]
            set_max_delay 0.8 -from [get_ports a] -to [get_ports y]
            set_min_delay 1.2 -from [get_ports a] -to [get_ports y]
        "#,
        )
        .unwrap();
        let timing = analyze_with_sdc(&nl, &constraints).unwrap();
        assert!((timing.max_delay_slack + 0.2).abs() < 1e-12);
        assert!((timing.min_delay_slack + 0.2).abs() < 1e-12);
        assert_eq!(
            timing.net_slacks["y"].min_delay_ns,
            Some(timing.min_delay_slack)
        );
        let mut store = TimingViolationStore::from_timing("sdc", &nl, &timing).unwrap();
        assert_eq!(store.len(), 2);
        assert_eq!(
            store.violations()["sdc:max_delay:y"].kind,
            TimingViolationKind::MaxDelay
        );
        assert_eq!(
            store.violations()["sdc:min_delay:y"].kind,
            TimingViolationKind::MinDelay
        );

        let waiver = TimingViolationWaiver::new("max-only", "sdc", "y", -1.0)
            .unwrap()
            .with_kind(TimingViolationKind::MaxDelay);
        store.add_waiver(waiver.clone()).unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(store.waived_len(), 1);
        assert!(store.active_violations().contains_key("sdc:min_delay:y"));

        let serialized = write_timing_violation_waivers(&[waiver]).unwrap();
        let parsed = parse_timing_violation_waivers(&serialized).unwrap();
        assert_eq!(parsed[0].kind, Some(TimingViolationKind::MaxDelay));
    }

    #[test]
    fn registry_from_stores_preserves_scenario_local_waivers() {
        let nl = and_netlist();
        let constraints = crate::parse_sdc(
            r#"
            create_clock -name clk -period 0.2 [get_ports a]
            set_output_delay 0.1 -clock clk [get_ports y]
        "#,
        )
        .unwrap();
        let timing = analyze_with_sdc(&nl, &constraints).unwrap();
        let mut slow = TimingViolationStore::from_timing("slow", &nl, &timing).unwrap();
        let fast = TimingViolationStore::from_timing("fast", &nl, &timing).unwrap();
        let waiver = TimingViolationWaiver::new("slow-only", "slow", "*", -10.0)
            .unwrap()
            .with_kind(TimingViolationKind::Output);
        slow.add_waiver(waiver).unwrap();

        let mut registry = timing_violation_registry_from_stores([
            ("slow".to_string(), slow),
            ("fast".to_string(), fast),
        ])
        .unwrap();
        assert_eq!(registry.stores()["slow"].len(), 0);
        assert_eq!(registry.len(), registry.stores()["fast"].len());
        assert_eq!(registry.summaries()["output:y"].worst_scenario, "fast");
        assert!(registry.stores()["slow"]
            .waivers()
            .contains_key("slow-only"));
        assert_eq!(registry.stores()["fast"].waived_len(), 0);
        assert!(registry.waivers().is_empty());

        let relaxed = crate::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_output_delay 0.1 -clock clk [get_ports y]
        "#,
        )
        .unwrap();
        let relaxed_timing = analyze_with_sdc(&nl, &relaxed).unwrap();
        let mut relaxed_slow =
            TimingViolationStore::from_timing("slow", &nl, &relaxed_timing).unwrap();
        relaxed_slow
            .add_waiver(
                TimingViolationWaiver::new("slow-only", "slow", "*", -10.0)
                    .unwrap()
                    .with_kind(TimingViolationKind::Output),
            )
            .unwrap();
        let relaxed_fast = TimingViolationStore::from_timing("fast", &nl, &relaxed_timing).unwrap();
        let update = registry
            .replace_stores([
                ("slow".to_string(), relaxed_slow),
                ("fast".to_string(), relaxed_fast),
            ])
            .unwrap();
        assert_eq!(registry.generation(), 2);
        assert!(update.scenario_diffs.contains_key("slow"));
        assert!(registry.stores()["slow"]
            .waivers()
            .contains_key("slow-only"));
    }

    #[test]
    fn dirty_object_update_preserves_untouched_violations() {
        let nl = and_netlist();
        let old = crate::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_max_delay 0.5 -from [get_ports a] -to [get_ports y]
            set_max_delay 0.5 -from [get_ports a] -to [get_ports z]
        "#,
        )
        .unwrap();
        let new = crate::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_max_delay 1.5 -from [get_ports a] -to [get_ports y]
            set_max_delay 0.5 -from [get_ports a] -to [get_ports z]
        "#,
        )
        .unwrap();
        let old_timing = analyze_with_sdc(&nl, &old).unwrap();
        let new_timing = analyze_with_sdc(&nl, &new).unwrap();
        let mut store = TimingViolationStore::from_timing("sdc", &nl, &old_timing).unwrap();
        assert_eq!(store.len(), 2);
        let preserved_slack = store.violations()["sdc:max_delay:z"].slack_ns;

        let mut dirty = TimingViolationDirtyObjects::default();
        dirty.add_net("y");
        let diff = store.update_dirty(&nl, &new_timing, &dirty).unwrap();
        assert_eq!(diff.removed.len(), 1);
        assert!(diff.removed.contains_key("sdc:max_delay:y"));
        assert!(store.violations().contains_key("sdc:max_delay:z"));
        assert!(!store.violations().contains_key("sdc:max_delay:y"));
        assert_eq!(
            store.violations()["sdc:max_delay:z"].slack_ns,
            preserved_slack
        );
        assert_eq!(store.generation, 2);

        let no_op = store
            .update_dirty(&nl, &new_timing, &TimingViolationDirtyObjects::default())
            .unwrap();
        assert!(no_op.is_empty());
        assert_eq!(store.generation, 2);
    }

    #[test]
    fn derives_dirty_objects_from_constraint_and_topology_diffs() {
        let old = crate::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_output_delay 0.1 -clock clk [get_ports y]
        "#,
        )
        .unwrap();
        let changed_endpoint = crate::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_output_delay 0.3 -clock clk [get_ports y]
        "#,
        )
        .unwrap();
        let endpoint_dirty =
            timing_violation_dirty_objects_from_constraints(&old, &changed_endpoint);
        assert_eq!(endpoint_dirty.nets, BTreeSet::from(["y".to_string()]));
        assert!(endpoint_dirty.clocks.is_empty());
        assert!(endpoint_dirty.cells.is_empty());

        let changed_clock = crate::parse_sdc(
            r#"
            create_clock -name clk -period 5.0 [get_ports clk]
            set_output_delay 0.1 -clock clk [get_ports y]
        "#,
        )
        .unwrap();
        let clock_dirty = timing_violation_dirty_objects_from_constraints(&old, &changed_clock);
        assert_eq!(clock_dirty.clocks, BTreeSet::from(["clk".to_string()]));

        let broad = crate::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_output_delay 0.1 -clock clk [get_ports y]
            set_max_delay 1.0 -to [get_ports y]
        "#,
        )
        .unwrap();
        let broad_dirty = timing_violation_dirty_objects_from_constraints(&old, &broad);
        assert_eq!(broad_dirty.clocks, BTreeSet::from(["clk".to_string()]));

        let topology = TimingTopologyDiff {
            changed_cells: BTreeSet::from(["u1".to_string()]),
            removed_nets: BTreeSet::from(["old".to_string()]),
            changed_ports: BTreeSet::from(["y".to_string()]),
            ..Default::default()
        };
        let topology_dirty = timing_violation_dirty_objects_from_topology(&topology);
        assert_eq!(topology_dirty.cells, BTreeSet::from(["u1".to_string()]));
        assert_eq!(
            topology_dirty.nets,
            BTreeSet::from(["old".to_string(), "y".to_string()])
        );
    }

    #[test]
    fn tracks_and_waives_min_pulse_violations() {
        let nl = two_flop_netlist();
        let constraints = crate::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 -fall 0.1 [get_ports clk]
            set_input_delay 1.0 -clock clk [get_ports d]
        "#,
        )
        .unwrap();
        let timing = analyze_with_sdc(&nl, &constraints).unwrap();
        assert_eq!(timing.min_pulse_width_checks.len(), 4);
        assert!((timing.min_pulse_width_slack + 0.15).abs() < 1e-12);

        let mut store = TimingViolationStore::from_timing("sdc", &nl, &timing).unwrap();
        assert_eq!(store.len(), 2);
        assert_eq!(
            store.violations()["sdc:min_pulse_high:launch"].kind,
            TimingViolationKind::MinPulseHigh
        );
        assert_eq!(
            store.violations()["sdc:min_pulse_high:capture"].endpoint,
            "capture"
        );

        let waiver = TimingViolationWaiver::new("capture-pulse", "sdc", "capture", -1.0)
            .unwrap()
            .with_kind(TimingViolationKind::MinPulseHigh);
        store.add_waiver(waiver.clone()).unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(store.waived_len(), 1);
        assert!(store
            .active_violations()
            .contains_key("sdc:min_pulse_high:launch"));

        let serialized = write_timing_violation_waivers(&[waiver]).unwrap();
        let parsed = parse_timing_violation_waivers(&serialized).unwrap();
        assert_eq!(parsed[0].kind, Some(TimingViolationKind::MinPulseHigh));
        assert!(timing_violation_report(&store, None).contains("min_pulse_high"));
    }

    #[test]
    fn waives_by_hierarchy_instance_namespace() {
        let mut nl = and_netlist();
        let y = nl.net_by_name("y").unwrap();
        let z = nl.net_by_name("z").unwrap();
        nl.hierarchy = vec![
            NetlistHierarchyInstance {
                path: "top.u1.u_leaf".into(),
                module: "leaf".into(),
                parent: Some("top.u1".into()),
                depth: 2,
                nets: vec![y],
                ports: vec![NetlistHierarchyPort {
                    name: "y".into(),
                    direction: Dir::Out,
                    local_nets: vec![y],
                    nets: vec![y],
                    width: 1,
                }],
            },
            NetlistHierarchyInstance {
                path: "top.u1".into(),
                module: "sub".into(),
                parent: Some("top".into()),
                depth: 1,
                nets: vec![z],
                ports: vec![NetlistHierarchyPort {
                    name: "z".into(),
                    direction: Dir::Out,
                    local_nets: vec![z],
                    nets: vec![z],
                    width: 1,
                }],
            },
        ];
        let constraints = crate::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_max_delay 0.1 -from [get_ports a] -to [get_ports y]
            set_max_delay 0.1 -from [get_ports a] -to [get_ports z]
        "#,
        )
        .unwrap();
        let timing = analyze_with_sdc(&nl, &constraints).unwrap();
        let mut store = TimingViolationStore::from_timing("sdc", &nl, &timing).unwrap();
        assert_eq!(store.len(), 2);
        assert_eq!(
            store.violations()["sdc:max_delay:y"].hierarchy.as_deref(),
            Some("top.u1.u_leaf")
        );
        assert_eq!(
            store.violations()["sdc:max_delay:z"].hierarchy.as_deref(),
            Some("top.u1")
        );

        let waiver = TimingViolationWaiver::new("leaf-only", "sdc", "*", -100.0)
            .unwrap()
            .with_instance_pattern("top.u1.u_leaf");
        store.add_waiver(waiver.clone()).unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(store.waived_len(), 1);
        assert!(store.active_violations().contains_key("sdc:max_delay:z"));
        assert!(!store.active_violations().contains_key("sdc:max_delay:y"));

        let serialized = write_timing_violation_waivers(&[waiver]).unwrap();
        let parsed = parse_timing_violation_waivers(&serialized).unwrap();
        assert_eq!(parsed[0].instance_pattern.as_deref(), Some("top.u1.u_leaf"));
        let report = timing_violation_report(&store, None);
        assert!(report.contains("hierarchy: top.u1"), "{report}");
    }
}
