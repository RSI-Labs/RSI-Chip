//! Timing-violation data model.

use crate::constraints::{
    ConstrainedHoldTimingPath, ConstrainedResetTimingPath, ConstrainedTimingPath,
    MinPulseWidthCheck, TimingObjectPattern, TimingPin,
};
use crate::TimingPath;
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
    pub(super) violations: BTreeMap<String, TimingViolation>,
    pub(super) waivers: BTreeMap<String, TimingViolationWaiver>,
    /// Violation ID to the waiver ID that currently covers it.
    pub(super) waived: BTreeMap<String, String>,
    pub(super) audit: Vec<TimingWaiverAuditEntry>,
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

    pub(super) fn add_all_clocks<'a, I>(&mut self, clocks: I)
    where
        I: IntoIterator<Item = &'a String>,
    {
        for clock in clocks {
            self.clocks.insert(clock.clone());
        }
    }

    pub(super) fn add_pins<'a, I>(&mut self, pins: I)
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
/// Waiver lifecycle action recorded by a persistent store or registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingWaiverAuditAction {
    Added,
    Removed,
}

impl TimingWaiverAuditAction {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
        }
    }

    pub(super) fn parse(value: &str, line_number: usize) -> Result<Self, String> {
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

    pub(super) fn validate(&self) -> Result<(), String> {
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

    pub(super) fn matches(&self, violation: &TimingViolation, generation: u64) -> bool {
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
    pub(super) stores: BTreeMap<String, TimingViolationStore>,
    pub(super) summaries: BTreeMap<String, CrossCornerTimingViolation>,
    pub(super) waivers: BTreeMap<String, TimingViolationWaiver>,
    pub(super) audit: Vec<TimingWaiverAuditEntry>,
    pub(super) generation: u64,
}
