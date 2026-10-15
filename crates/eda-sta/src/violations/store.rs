//! Scenario-scoped violation store and waiver lifecycle.

use crate::constraints::ConstrainedTiming;
use crate::Netlist;
use std::collections::{BTreeMap, BTreeSet};

use super::lifecycle::diff_violations;
use super::paths::collect_violations;
use super::serialization::{active_waivers_from_audit, validate_waiver_audit};
use super::types::*;

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

    pub(super) fn sync_waivers(&mut self) {
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
