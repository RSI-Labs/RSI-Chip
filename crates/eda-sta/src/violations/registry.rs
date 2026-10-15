//! Cross-scenario violation registry.

use crate::constraints::analyze_with_sdc;
use crate::multicorner::TimingScenario;
use std::collections::BTreeMap;

use super::lifecycle::{aggregate_cross_corner, diff_cross_corner, diff_violations};
use super::serialization::{active_waivers_from_audit, validate_waiver_audit};
use super::types::*;

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
