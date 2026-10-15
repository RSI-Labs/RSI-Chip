//! Violation diff and cross-corner aggregation.

use std::collections::BTreeMap;

use super::types::*;

pub(super) fn diff_violations(
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

pub(super) fn aggregate_cross_corner(
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

pub(super) fn aggregate_cross_corner_hierarchies(
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

pub(super) fn diff_cross_corner(
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
