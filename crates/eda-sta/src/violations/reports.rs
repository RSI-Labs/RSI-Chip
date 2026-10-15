//! Human-readable timing-violation reports.

use std::collections::{BTreeMap, BTreeSet};

use super::lifecycle::aggregate_cross_corner;
use super::serialization::{active_waivers_from_audit, validate_waiver_audit};
use super::types::*;

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
