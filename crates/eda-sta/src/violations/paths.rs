//! Timing-path-to-violation extraction.

use crate::constraints::{
    AppliedTimingDelayLimit, ConstrainedHoldTimingPath, ConstrainedResetTimingPath,
    ConstrainedTiming, ConstrainedTimingPath, MinPulsePolarity, MinPulseWidthCheck, ResetCheckKind,
};
use crate::{Netlist, TimingLaunch, TimingPath, TimingPathEndpoint};
use std::collections::BTreeMap;

use super::types::*;

pub(super) fn endpoint_name(endpoint: &TimingPathEndpoint) -> String {
    match endpoint {
        TimingPathEndpoint::SequentialCell(name) => name.clone(),
        TimingPathEndpoint::OutputPort(name) => name.clone(),
    }
}

pub(super) fn launch_name(launch: &TimingLaunch) -> String {
    match launch {
        TimingLaunch::PrimaryInput(name) => name.clone(),
        TimingLaunch::SequentialCell(name) => name.clone(),
    }
}

pub(super) fn insert_worst(
    out: &mut BTreeMap<String, TimingViolation>,
    violation: TimingViolation,
) {
    out.entry(violation.id.clone())
        .and_modify(|old| {
            if violation.slack_ns < old.slack_ns {
                *old = violation.clone();
            }
        })
        .or_insert(violation);
}

pub(super) fn setup_violation(
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

pub(super) fn hierarchy_annotation(
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

pub(super) fn cell_name_hierarchy_annotation(
    nl: &Netlist,
    cell_name: &str,
) -> (Option<String>, Vec<String>) {
    let hierarchy = nl
        .cell_by_name(cell_name)
        .and_then(|cell| nl.cell_hierarchy_instance(cell))
        .map(|instance| instance.path.clone());
    let instances = hierarchy.clone().into_iter().collect();
    (hierarchy, instances)
}

pub(super) fn max_delay_violation(
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

pub(super) fn hold_violation(
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

pub(super) fn min_delay_violation(
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

pub(super) fn reset_violation(
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

pub(super) fn min_pulse_violation(
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

pub(super) fn collect_violations(
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
