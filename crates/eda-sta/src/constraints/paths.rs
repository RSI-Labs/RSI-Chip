//! Constrained path reconstruction, reports, timing-object matching, and exception application.

use super::analysis::*;
use super::*;

pub fn constrained_timing_paths(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    max_paths: usize,
) -> Vec<ConstrainedTimingPath> {
    let mut paths = timing
        .base
        .timing_paths(nl, 0)
        .into_iter()
        .filter_map(|path| {
            let endpoint_net = *path.nets.last()?;
            let name = nl.net(endpoint_net).name.clone();
            let exception_key = data_path_endpoint_key(nl, &path);
            let exceptions = timing
                .path_exceptions
                .get(&exception_key)
                .cloned()
                .unwrap_or_default();
            if exceptions
                .iter()
                .any(|exception| exception.multicycle.is_none())
            {
                return None;
            }
            let delay_limits = timing
                .path_max_delay_limits
                .get(&exception_key)
                .cloned()
                .unwrap_or_default();
            let base_slack = match &path.endpoint {
                crate::TimingPathEndpoint::SequentialCell(_) => {
                    timing.net_slacks.get(&name)?.setup_ns
                }
                crate::TimingPathEndpoint::OutputPort(_) => timing.net_slacks.get(&name)?.output_ns,
            }?;
            let slack = delay_limits
                .iter()
                .map(|limit| limit.slack_ns)
                .fold(base_slack, f64::min);
            if !slack.is_finite() {
                return None;
            }
            let required_ns = path.arrival_ns + slack;
            let clocks = constrained_path_clocks(
                nl,
                &path,
                timing,
                endpoint_cycle_multiplier(nl, &path, timing),
            );
            Some(ConstrainedTimingPath {
                path,
                slack_ns: slack,
                required_ns,
                clocks,
                exceptions,
                delay_limits,
            })
        })
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| {
        left.slack_ns
            .total_cmp(&right.slack_ns)
            .then_with(|| right.path.arrival_ns.total_cmp(&left.path.arrival_ns))
    });
    if max_paths > 0 {
        paths.truncate(max_paths);
    }
    paths
}

/// Early hold paths annotated with constrained hold slack.
///
/// Paths are sorted from worst to best hold slack. `max_paths == 0` returns
/// every finite hold endpoint.
pub fn constrained_hold_timing_paths(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    max_paths: usize,
) -> Vec<ConstrainedHoldTimingPath> {
    let mut paths = timing
        .base
        .hold_timing_paths(nl, timing.hold_uncertainty, 0)
        .into_iter()
        .filter_map(|path| {
            let endpoint_net = *path.path.nets.last()?;
            let name = nl.net(endpoint_net).name.clone();
            let exception_key = data_path_endpoint_key(nl, &path.path);
            let exceptions = timing
                .path_exceptions
                .get(&exception_key)
                .cloned()
                .unwrap_or_default();
            if exceptions
                .iter()
                .any(|exception| exception.multicycle.is_none())
            {
                return None;
            }
            let delay_limits = timing
                .path_min_delay_limits
                .get(&exception_key)
                .cloned()
                .unwrap_or_default();
            let slack = timing.net_slacks.get(&name)?.hold_ns?;
            if !slack.is_finite() {
                return None;
            }
            let required_ns = path.path.arrival_ns - slack;
            let clocks = constrained_path_clocks(nl, &path.path, timing, 0.0);
            Some(ConstrainedHoldTimingPath {
                path,
                slack_ns: slack,
                required_ns,
                clocks,
                exceptions,
                delay_limits,
            })
        })
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| {
        left.slack_ns.total_cmp(&right.slack_ns).then_with(|| {
            right
                .path
                .path
                .arrival_ns
                .total_cmp(&left.path.path.arrival_ns)
        })
    });
    if max_paths > 0 {
        paths.truncate(max_paths);
    }
    paths
}

/// Recovery/removal paths annotated with constrained asynchronous-reset slack.
pub fn constrained_reset_timing_paths(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    max_paths: usize,
) -> Vec<ConstrainedResetTimingPath> {
    let mut paths = Vec::new();
    for reset in timing.base.reset_timing_paths(nl, 0) {
        let recovery_name = &nl.net(reset.recovery.nets.last().copied().unwrap()).name;
        let removal_name = &nl.net(reset.removal.nets.last().copied().unwrap()).name;
        let recovery_clocks = constrained_path_clocks(
            nl,
            &reset.recovery,
            timing,
            reset_endpoint_cycle_multiplier(nl, &reset.recovery, timing),
        );
        let removal_clocks = constrained_path_clocks(nl, &reset.removal, timing, 0.0);
        let recovery_exception_key = reset_path_endpoint_key(nl, &reset.recovery);
        let recovery_exceptions = timing
            .path_exceptions
            .get(&recovery_exception_key)
            .cloned()
            .unwrap_or_default();
        let removal_exception_key = reset_path_endpoint_key(nl, &reset.removal);
        let removal_exceptions = timing
            .path_exceptions
            .get(&removal_exception_key)
            .cloned()
            .unwrap_or_default();
        let candidates = [
            (
                reset.recovery,
                ResetCheckKind::Recovery,
                timing
                    .net_slacks
                    .get(recovery_name)
                    .and_then(|slack| slack.recovery_ns),
                recovery_clocks,
                recovery_exceptions,
            ),
            (
                reset.removal,
                ResetCheckKind::Removal,
                timing
                    .net_slacks
                    .get(removal_name)
                    .and_then(|slack| slack.removal_ns),
                removal_clocks,
                removal_exceptions,
            ),
        ];
        for (path, kind, slack, clocks, exceptions) in candidates {
            if exceptions
                .iter()
                .any(|exception| exception.multicycle.is_none())
            {
                continue;
            }
            let Some(slack) = slack.filter(|slack| slack.is_finite()) else {
                continue;
            };
            let required_ns = match kind {
                ResetCheckKind::Recovery => path.arrival_ns + slack,
                ResetCheckKind::Removal => path.arrival_ns - slack,
            };
            paths.push(ConstrainedResetTimingPath {
                path,
                kind,
                slack_ns: slack,
                required_ns,
                clocks,
                exceptions,
            });
        }
    }
    paths.sort_by(|left, right| {
        left.slack_ns
            .total_cmp(&right.slack_ns)
            .then_with(|| right.path.arrival_ns.total_cmp(&left.path.arrival_ns))
    });
    if max_paths > 0 {
        paths.truncate(max_paths);
    }
    paths
}

/// Concrete setup, hold, recovery, and removal paths omitted by false-path
/// exceptions.
pub fn excluded_timing_paths(nl: &Netlist, timing: &ConstrainedTiming) -> Vec<ExcludedTimingPath> {
    let mut out = Vec::new();
    for path in timing.base.timing_paths(nl, 0) {
        let key = data_path_endpoint_key(nl, &path);
        for exception in timing
            .path_exceptions
            .get(&key)
            .into_iter()
            .flatten()
            .filter(|exception| exception.multicycle.is_none() && exception.check.applies_setup())
        {
            out.push(ExcludedTimingPath {
                check: ExcludedTimingCheck::Setup,
                path: path.clone(),
                exception: exception.clone(),
            });
        }
    }

    for hold in timing
        .base
        .hold_timing_paths(nl, timing.hold_uncertainty, 0)
    {
        let key = data_path_endpoint_key(nl, &hold.path);
        for exception in timing
            .path_exceptions
            .get(&key)
            .into_iter()
            .flatten()
            .filter(|exception| exception.multicycle.is_none() && exception.check.applies_hold())
        {
            out.push(ExcludedTimingPath {
                check: ExcludedTimingCheck::Hold,
                path: hold.path.clone(),
                exception: exception.clone(),
            });
        }
    }

    for reset in timing.base.reset_timing_paths(nl, 0) {
        for (path, check) in [
            (&reset.recovery, ExcludedTimingCheck::Recovery),
            (&reset.removal, ExcludedTimingCheck::Removal),
        ] {
            let key = reset_path_endpoint_key(nl, path);
            for exception in timing
                .path_exceptions
                .get(&key)
                .into_iter()
                .flatten()
                .filter(|exception| {
                    exception.multicycle.is_none()
                        && if check == ExcludedTimingCheck::Recovery {
                            exception.check.applies_setup()
                        } else {
                            exception.check.applies_hold()
                        }
                })
            {
                out.push(ExcludedTimingPath {
                    check,
                    path: path.clone(),
                    exception: exception.clone(),
                });
            }
        }
    }

    out.sort_by(|left, right| {
        endpoint_debug(&left.path.endpoint)
            .cmp(&endpoint_debug(&right.path.endpoint))
            .then(format!("{:?}", left.check).cmp(&format!("{:?}", right.check)))
    });
    out
}

pub fn excluded_timing_path_report(nl: &Netlist, paths: &[ExcludedTimingPath]) -> String {
    let mut out = format!("Excluded timing path report: {} path(s)\n", paths.len());
    for excluded in paths {
        let check = match excluded.check {
            ExcludedTimingCheck::Setup => "setup",
            ExcludedTimingCheck::Hold => "hold",
            ExcludedTimingCheck::Recovery => "recovery",
            ExcludedTimingCheck::Removal => "removal",
        };
        let from = excluded.exception.from.as_deref().unwrap_or("<any>");
        let to = excluded.exception.to.as_deref().unwrap_or("<any>");
        let through = if excluded.exception.through.is_empty() {
            String::new()
        } else {
            format!(" through {}", excluded.exception.through.join(" -> "))
        };
        out.push_str(&format!(
            "  {} {} excluded by false path from {} to {}{}\n",
            check,
            endpoint_debug(&excluded.path.endpoint),
            from,
            to,
            through
        ));
        let cells = excluded
            .path
            .cells
            .iter()
            .map(|&cell| nl.cell(cell).name.clone())
            .collect::<Vec<_>>()
            .join(" -> ");
        out.push_str(&format!("    cells: {}\n", cells));
    }
    out
}

pub(super) fn endpoint_debug(endpoint: &crate::TimingPathEndpoint) -> String {
    match endpoint {
        crate::TimingPathEndpoint::SequentialCell(name) => format!("flop {name}"),
        crate::TimingPathEndpoint::OutputPort(name) => format!("output {name}"),
    }
}

pub fn constrained_timing_path_report(nl: &Netlist, paths: &[ConstrainedTimingPath]) -> String {
    let mut out = format!("Constrained timing path report: {} path(s)\n", paths.len());
    for constrained in paths {
        let path = &constrained.path;
        out.push_str(&format!(
            "  slack {:.6} ns, arrival {:.6} ns, required {:.6} ns\n",
            constrained.slack_ns, path.arrival_ns, constrained.required_ns
        ));
        let endpoint = match &path.endpoint {
            crate::TimingPathEndpoint::SequentialCell(name) => format!("flop {}", name),
            crate::TimingPathEndpoint::OutputPort(name) => format!("output {}", name),
        };
        let cells = path
            .cells
            .iter()
            .map(|&cell| nl.cell(cell).name.clone())
            .collect::<Vec<_>>()
            .join(" -> ");
        out.push_str(&format!(
            "    endpoint: {}\n    cells: {}\n",
            endpoint, cells
        ));
        push_path_clocks_report(&constrained.clocks, &mut out);
        push_path_exceptions_report(&constrained.exceptions, &mut out);
        push_delay_limits_report(&constrained.delay_limits, &mut out);
        crate::push_timing_arcs_report(nl, &path.arcs, &mut out);
    }
    out
}

pub(super) fn push_clock_edge_report(label: &str, edge: &TimingClockEdge, out: &mut String) {
    let edge_kind = match edge.edge {
        TimingClockEdgeKind::Rise => "rise",
        TimingClockEdgeKind::Fall => "fall",
    };
    let reference = if edge.virtual_reference {
        "virtual"
    } else {
        "local"
    };
    out.push_str(&format!(
        "    {}: {} {} at {:.6} ns, source {:.6} ns, network {:.6} ns, absolute {:.6} ns ({})\n",
        label,
        edge.clock,
        edge_kind,
        edge.edge_time_ns,
        edge.source_latency_ns,
        edge.network_latency_ns,
        edge.absolute_time_ns,
        reference
    ));
}

pub(super) fn push_path_clocks_report(clocks: &TimingPathClocks, out: &mut String) {
    out.push_str("    clocks:\n");
    push_clock_edge_report("launch", &clocks.launch, out);
    push_clock_edge_report("capture", &clocks.capture, out);
    out.push_str(&format!(
        "    edge separation {:.6} ns, insertion skew {:.6} ns\n",
        clocks.edge_separation_ns, clocks.insertion_skew_ns
    ));
}

pub(super) fn push_path_exceptions_report(exceptions: &[PathException], out: &mut String) {
    if exceptions.is_empty() {
        out.push_str("    exceptions: <none>\n");
        return;
    }
    out.push_str("    exceptions:\n");
    for exception in exceptions {
        let from = exception.from.as_deref().unwrap_or("<any>");
        let to = exception.to.as_deref().unwrap_or("<any>");
        let through = if exception.through.is_empty() {
            String::new()
        } else {
            format!(" through {}", exception.through.join(" -> "))
        };
        let instance = exception
            .instance_pattern
            .as_deref()
            .map(|pattern| format!(" instance {}", pattern))
            .unwrap_or_default();
        if let Some(multicycle) = exception.multicycle {
            out.push_str(&format!(
                "      multicycle {:.6} ({}), from {} to {}{}{}\n",
                multicycle,
                exception.check.as_str(),
                from,
                to,
                through,
                instance
            ));
        } else {
            out.push_str(&format!(
                "      false path ({}), from {} to {}{}{}\n",
                exception.check.as_str(),
                from,
                to,
                through,
                instance
            ));
        }
    }
}

pub(super) fn push_delay_limits_report(limits: &[AppliedTimingDelayLimit], out: &mut String) {
    if limits.is_empty() {
        out.push_str("    delay limits: <none>\n");
        return;
    }
    out.push_str("    delay limits:\n");
    for applied in limits {
        let kind = if applied.maximum { "max" } else { "min" };
        let from = applied.limit.from.as_deref().unwrap_or("<any>");
        let to = applied.limit.to.as_deref().unwrap_or("<any>");
        let through = if applied.limit.through.is_empty() {
            String::new()
        } else {
            format!(" through {}", applied.limit.through.join(" -> "))
        };
        let instance = applied
            .limit
            .instance_pattern
            .as_deref()
            .map(|pattern| format!(" instance {}", pattern))
            .unwrap_or_default();
        out.push_str(&format!(
            "      {} {:.6} ns, slack {:.6} ns, from {} to {}{}{}\n",
            kind, applied.limit.limit, applied.slack_ns, from, to, through, instance
        ));
    }
}

pub fn constrained_hold_timing_path_report(
    nl: &Netlist,
    paths: &[ConstrainedHoldTimingPath],
) -> String {
    let mut out = format!(
        "Constrained hold timing path report: {} path(s)\n",
        paths.len()
    );
    for constrained in paths {
        out.push_str(&format!(
            "  hold slack {:.6} ns, arrival {:.6} ns, required {:.6} ns\n",
            constrained.slack_ns, constrained.path.path.arrival_ns, constrained.required_ns
        ));
        let endpoint = match &constrained.path.path.endpoint {
            crate::TimingPathEndpoint::SequentialCell(name) => format!("flop {}", name),
            crate::TimingPathEndpoint::OutputPort(name) => format!("output {}", name),
        };
        let cells = constrained
            .path
            .path
            .cells
            .iter()
            .map(|&cell| nl.cell(cell).name.clone())
            .collect::<Vec<_>>()
            .join(" -> ");
        out.push_str(&format!(
            "    endpoint: {}\n    cells: {}\n",
            endpoint, cells
        ));
        push_path_clocks_report(&constrained.clocks, &mut out);
        push_path_exceptions_report(&constrained.exceptions, &mut out);
        push_delay_limits_report(&constrained.delay_limits, &mut out);
        crate::push_timing_arcs_report(nl, &constrained.path.path.arcs, &mut out);
    }
    out
}

pub fn constrained_reset_timing_path_report(
    nl: &Netlist,
    paths: &[ConstrainedResetTimingPath],
) -> String {
    let mut out = format!(
        "Constrained reset timing path report: {} path(s)\n",
        paths.len()
    );
    for constrained in paths {
        let kind = match constrained.kind {
            ResetCheckKind::Recovery => "recovery",
            ResetCheckKind::Removal => "removal",
        };
        out.push_str(&format!(
            "  {} slack {:.6} ns, arrival {:.6} ns, required {:.6} ns\n",
            kind, constrained.slack_ns, constrained.path.arrival_ns, constrained.required_ns
        ));
        let cells = constrained
            .path
            .cells
            .iter()
            .map(|&cell| nl.cell(cell).name.clone())
            .collect::<Vec<_>>()
            .join(" -> ");
        out.push_str(&format!("    cells: {}\n", cells));
        push_path_clocks_report(&constrained.clocks, &mut out);
        push_path_exceptions_report(&constrained.exceptions, &mut out);
        crate::push_timing_arcs_report(nl, &constrained.path.arcs, &mut out);
    }
    out
}

pub(super) fn path_start_names(nl: &Netlist, start: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![start];
    let mut seen = std::collections::HashSet::new();
    while let Some(net) = stack.pop() {
        if !seen.insert(net) {
            continue;
        }
        out.push(nl.net(net).name.clone());
        for &ci in &nl.net(net).drivers {
            let cell = nl.cell(ci);
            if nl.cell_is_sequential(&cell.kind) {
                continue;
            }
            for &input in &cell.inputs {
                stack.push(input);
            }
        }
    }
    out
}

pub(super) fn timing_pin_ref(nl: &Netlist, cell_id: usize, pin: usize) -> Option<TimingObjectRef> {
    let pin = crate::input_pin_name(nl, cell_id, pin)?;
    Some(TimingObjectRef::new(
        TimingObjectKind::Pin,
        format!("{}/{}", nl.cell(cell_id).name, pin),
    ))
}

pub(super) fn timing_output_pin_ref(nl: &Netlist, cell_id: usize) -> Option<TimingObjectRef> {
    let pin = crate::output_pin_name(nl, cell_id)?;
    Some(TimingObjectRef::new(
        TimingObjectKind::Pin,
        format!("{}/{}", nl.cell(cell_id).name, pin),
    ))
}

pub(super) fn disabled_selector_matches_pin(
    selector: &TimingObjectRef,
    nl: &Netlist,
    cell_id: usize,
    pin_ref: Option<&TimingObjectRef>,
) -> bool {
    match selector.kind {
        TimingObjectKind::Cell => selector.matches_name(&nl.cell(cell_id).name),
        TimingObjectKind::Pin => pin_ref.is_some_and(|pin| selector.matches_object(pin)),
        _ => false,
    }
}

pub(super) fn cell_hierarchy_matches(nl: &Netlist, cell_id: usize, pattern: &str) -> bool {
    nl.cell_hierarchy_instance(cell_id)
        .is_some_and(|instance| TimingObjectPattern::new(pattern).matches(&instance.path))
}

/// True when a combinational or sequential cell arc is disabled by SDC.
///
/// `from_pin` and `to_pin` are cell input-pin indexes. A `None` `to_pin`
/// denotes the cell output. Check-only disables use the same selector logic
/// with the checked input pin as the endpoint.
pub(crate) fn timing_arc_disabled(
    nl: &Netlist,
    c: &TimingConstraints,
    cell_id: usize,
    from_pin: Option<usize>,
    to_pin: Option<usize>,
) -> bool {
    let from_ref = from_pin.and_then(|pin| timing_pin_ref(nl, cell_id, pin));
    let to_ref = match to_pin {
        Some(pin) => timing_pin_ref(nl, cell_id, pin),
        None => timing_output_pin_ref(nl, cell_id),
    };
    c.disabled_timing_arcs.iter().any(|disabled| {
        let object_matches = disabled.object.as_ref().is_some_and(|object| {
            object.kind == TimingObjectKind::Cell && object.matches_name(&nl.cell(cell_id).name)
                || (object.kind == TimingObjectKind::Pin
                    && (from_ref
                        .as_ref()
                        .is_some_and(|pin| object.matches_object(pin))
                        || to_ref
                            .as_ref()
                            .is_some_and(|pin| object.matches_object(pin))))
        });
        let from_matches = disabled.from.as_ref().is_none_or(|selector| {
            disabled_selector_matches_pin(selector, nl, cell_id, from_ref.as_ref())
        });
        let to_matches = disabled.to.as_ref().is_none_or(|selector| {
            disabled_selector_matches_pin(selector, nl, cell_id, to_ref.as_ref())
        });
        let qualifiers_match = from_matches && to_matches;
        let instance_matches = disabled
            .instance_pattern
            .as_ref()
            .is_none_or(|pattern| cell_hierarchy_matches(nl, cell_id, pattern));
        (object_matches || disabled.object.is_none()) && qualifiers_match && instance_matches
    })
}

pub(crate) fn timing_check_disabled(
    nl: &Netlist,
    c: &TimingConstraints,
    cell_id: usize,
    check_pin: usize,
) -> bool {
    c.disabled_timing_arcs.iter().any(|disabled| {
        let Some(pin_ref) = timing_pin_ref(nl, cell_id, check_pin) else {
            return false;
        };
        let object_matches = disabled.object.as_ref().is_some_and(|object| {
            object.kind == TimingObjectKind::Cell && object.matches_name(&nl.cell(cell_id).name)
                || (object.kind == TimingObjectKind::Pin && object.matches_object(&pin_ref))
        });
        let from_matches = disabled.from.as_ref().is_none_or(|selector| {
            disabled_selector_matches_pin(selector, nl, cell_id, Some(&pin_ref))
        });
        let to_matches = disabled.to.as_ref().is_none_or(|selector| {
            disabled_selector_matches_pin(selector, nl, cell_id, Some(&pin_ref))
        });
        let qualifiers_match = from_matches && to_matches;
        let instance_matches = disabled
            .instance_pattern
            .as_ref()
            .is_none_or(|pattern| cell_hierarchy_matches(nl, cell_id, pattern));
        (object_matches || disabled.object.is_none()) && qualifiers_match && instance_matches
    })
}

pub(super) fn push_unique_object(objects: &mut Vec<TimingObjectRef>, object: TimingObjectRef) {
    if !objects.contains(&object) {
        objects.push(object);
    }
}

pub(super) fn path_start_objects(nl: &Netlist, endpoint_net: usize) -> Vec<TimingObjectRef> {
    let mut objects = Vec::new();
    for net in path_start_names(nl, endpoint_net) {
        let Some(net_id) = nl.net_by_name(&net) else {
            continue;
        };
        push_unique_object(
            &mut objects,
            TimingObjectRef::new(TimingObjectKind::Net, net.clone()),
        );
        if let Some(port) = nl
            .ports
            .iter()
            .find(|port| port.dir == eda_core::netlist::Dir::In && port.bits.contains(&net_id))
        {
            push_unique_object(
                &mut objects,
                TimingObjectRef::new(TimingObjectKind::Port, port.name.clone()),
            );
        }
        for &cell_id in &nl.net(net_id).drivers {
            let cell = nl.cell(cell_id);
            push_unique_object(
                &mut objects,
                TimingObjectRef::new(TimingObjectKind::Cell, cell.name.clone()),
            );
            if nl.cell_is_sequential(&cell.kind) {
                if let Some(pin) = timing_pin_ref(nl, cell_id, 0) {
                    push_unique_object(&mut objects, pin);
                }
                if let Some(pin) = timing_output_pin_ref(nl, cell_id) {
                    push_unique_object(&mut objects, pin);
                }
            } else {
                for pin in 0..cell.inputs.len() {
                    if let Some(pin_ref) = timing_pin_ref(nl, cell_id, pin) {
                        push_unique_object(&mut objects, pin_ref);
                    }
                }
                if let Some(pin_ref) = timing_output_pin_ref(nl, cell_id) {
                    push_unique_object(&mut objects, pin_ref);
                }
            }
        }
    }
    objects
}

pub(super) fn endpoint_timing_objects(
    nl: &Netlist,
    endpoint_net: usize,
    endpoint_name: &str,
    endpoint_cell: Option<(usize, usize)>,
) -> Vec<TimingObjectRef> {
    let mut objects = vec![TimingObjectRef::new(TimingObjectKind::Net, endpoint_name)];
    if let Some(port) = nl
        .ports
        .iter()
        .find(|port| port.dir == eda_core::netlist::Dir::Out && port.bits.contains(&endpoint_net))
    {
        push_unique_object(
            &mut objects,
            TimingObjectRef::new(TimingObjectKind::Port, port.name.clone()),
        );
    }
    if let Some((cell_id, input_pin)) = endpoint_cell {
        push_unique_object(
            &mut objects,
            TimingObjectRef::new(TimingObjectKind::Cell, nl.cell(cell_id).name.clone()),
        );
        if let Some(pin) = timing_pin_ref(nl, cell_id, input_pin) {
            push_unique_object(&mut objects, pin);
        }
    }
    for &cell_id in &nl.net(endpoint_net).drivers {
        push_unique_object(
            &mut objects,
            TimingObjectRef::new(TimingObjectKind::Cell, nl.cell(cell_id).name.clone()),
        );
        if let Some(pin) = timing_output_pin_ref(nl, cell_id) {
            push_unique_object(&mut objects, pin);
        }
    }
    objects
}

pub(super) fn timing_object_matches(
    object: &TimingObjectRef,
    candidates: &[TimingObjectRef],
    fallback_name: &str,
) -> bool {
    if candidates
        .iter()
        .any(|candidate| object.matches_object(candidate))
    {
        return true;
    }

    // A cell exception also matches an explicitly referenced pin of that cell.
    if object.kind == TimingObjectKind::Cell {
        return candidates.iter().any(|candidate| {
            if candidate.kind != TimingObjectKind::Pin {
                return false;
            }
            candidate
                .name
                .rsplit_once('/')
                .is_some_and(|(cell_name, _pin)| object.matches_name(cell_name))
        });
    }
    object.matches_name(fallback_name)
}

pub(super) fn timing_object_name_matches(
    pattern: &str,
    candidates: &[TimingObjectRef],
    fallback_name: &str,
) -> bool {
    let pattern = TimingObjectPattern::new(pattern);
    pattern.matches(fallback_name)
        || candidates
            .iter()
            .any(|candidate| pattern.matches(&candidate.name))
}

pub(super) fn net_objects(nl: &Netlist, net: NetId) -> Vec<TimingObjectRef> {
    let mut objects = vec![TimingObjectRef::new(
        TimingObjectKind::Net,
        nl.net(net).name.clone(),
    )];
    if let Some(port) = nl.ports.iter().find(|port| port.bits.contains(&net)) {
        push_unique_object(
            &mut objects,
            TimingObjectRef::new(TimingObjectKind::Port, port.name.clone()),
        );
    }
    objects
}

pub(super) fn path_object_stages(
    nl: &Netlist,
    cells: &[usize],
    nets: &[NetId],
) -> Vec<Vec<TimingObjectRef>> {
    let mut stages = Vec::new();
    if let Some(&first_net) = nets.first() {
        stages.push(net_objects(nl, first_net));
    }

    for (cell_index, &cell_id) in cells.iter().enumerate() {
        let cell = nl.cell(cell_id);
        let first = cell_index == 0;
        let last = cell_index + 1 == cells.len();
        let sequential = nl.cell_is_sequential(&cell.kind);
        let mut input_stage = vec![TimingObjectRef::new(
            TimingObjectKind::Cell,
            cell.name.clone(),
        )];
        for (pin, &input) in cell.inputs.iter().enumerate() {
            let on_path = nets.contains(&input);
            let clock_pin = sequential && pin == 0 && (first || last);
            if on_path || clock_pin {
                if let Some(pin_ref) = timing_pin_ref(nl, cell_id, pin) {
                    push_unique_object(&mut input_stage, pin_ref);
                }
            }
        }
        stages.push(input_stage);

        let mut output_stage = Vec::new();
        if let Some(pin) = timing_output_pin_ref(nl, cell_id) {
            output_stage.push(pin);
        }
        if let Some(output_net) = nets.iter().find(|&&net| net == cell.output) {
            for object in net_objects(nl, *output_net) {
                push_unique_object(&mut output_stage, object);
            }
        }
        stages.push(output_stage);
    }
    stages
}

pub(super) fn stage_contains_object(stage: &[TimingObjectRef], object: &TimingObjectRef) -> bool {
    timing_object_matches(object, stage, "")
}

pub(super) fn through_groups_match(
    groups: &[Vec<TimingObjectRef>],
    stages: &[Vec<TimingObjectRef>],
) -> bool {
    let mut stage_index = 0;
    for group in groups {
        let Some(match_index) = stages[stage_index..].iter().position(|stage| {
            group
                .iter()
                .any(|object| stage_contains_object(stage, object))
        }) else {
            return false;
        };
        stage_index += match_index + 1;
    }
    true
}

pub(super) fn path_hierarchy_instances(
    nl: &Netlist,
    cells: &[usize],
    endpoint_net: NetId,
) -> Vec<String> {
    cells
        .iter()
        .filter_map(|&cell| nl.cell_hierarchy_instance(cell))
        .map(|instance| instance.path.clone())
        .chain(
            nl.owning_hierarchy_instance(endpoint_net)
                .map(|instance| instance.path.clone()),
        )
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn matching_path_exceptions(
    base: &StaResult,
    nl: &Netlist,
    endpoint_net: usize,
    endpoint_name: &str,
    endpoint_cell: Option<usize>,
    early: bool,
    endpoint_objects: &[TimingObjectRef],
    c: &TimingConstraints,
) -> Vec<PathException> {
    let starts = path_start_objects(nl, endpoint_net);
    let (cells, nets) = if early {
        base.early_path_topology(nl, endpoint_net, endpoint_cell)
    } else {
        base.late_path_topology(nl, endpoint_net, endpoint_cell)
    };
    let stages = path_object_stages(nl, &cells, &nets);
    let path_instances = path_hierarchy_instances(nl, &cells, endpoint_net);
    let matches = |ex: &PathException| {
        let to_matches = match &ex.to_object {
            Some(object) => timing_object_matches(object, endpoint_objects, endpoint_name),
            None => ex
                .to
                .as_deref()
                .map(|to| timing_object_name_matches(to, endpoint_objects, endpoint_name))
                .unwrap_or(true),
        };
        let from_matches = match &ex.from_object {
            Some(object) => timing_object_matches(object, &starts, endpoint_name),
            None => ex
                .from
                .as_deref()
                .map(|from| timing_object_name_matches(from, &starts, endpoint_name))
                .unwrap_or(true),
        };
        let through_matches =
            ex.through_groups.is_empty() || through_groups_match(&ex.through_groups, &stages);
        let instance_matches = ex.instance_pattern.as_ref().is_none_or(|pattern| {
            path_instances
                .iter()
                .any(|instance| TimingObjectPattern::new(pattern).matches(instance))
        });
        to_matches && from_matches && through_matches && instance_matches
    };
    c.false_paths
        .iter()
        .chain(c.multicycle_paths.iter())
        .filter(|ex| ex.check.applies_early(early))
        .filter(|ex| matches(ex))
        .cloned()
        .collect()
}

pub(super) fn record_path_exceptions(
    out: &mut BTreeMap<String, Vec<PathException>>,
    endpoint: &TimingObjectRef,
    exceptions: Vec<PathException>,
) {
    if exceptions.is_empty() {
        return;
    }
    let entry = out
        .entry(format!("{:?}:{}", endpoint.kind, endpoint.name))
        .or_default();
    if entry.is_empty() {
        *entry = exceptions;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn path_multiplier(
    base: &StaResult,
    nl: &Netlist,
    endpoint_net: usize,
    endpoint_name: &str,
    endpoint_cell: Option<usize>,
    early: bool,
    endpoint_objects: &[TimingObjectRef],
    c: &TimingConstraints,
) -> Option<f64> {
    let exceptions = matching_path_exceptions(
        base,
        nl,
        endpoint_net,
        endpoint_name,
        endpoint_cell,
        early,
        endpoint_objects,
        c,
    );
    if exceptions
        .iter()
        .any(|exception| exception.multicycle.is_none())
    {
        return Some(f64::INFINITY);
    }
    let mut multiplier = 1.0;
    let mut found = false;
    for ex in exceptions {
        multiplier *= ex.multicycle.unwrap_or(1.0);
        found = true;
    }
    if found {
        Some(multiplier)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn matching_path_delay_limits(
    base: &StaResult,
    nl: &Netlist,
    endpoint_net: usize,
    endpoint_name: &str,
    endpoint_cell: Option<usize>,
    early: bool,
    c: &TimingConstraints,
    maximum: bool,
    arrival: f64,
    endpoint_objects: &[TimingObjectRef],
) -> Vec<AppliedTimingDelayLimit> {
    if matches!(
        path_multiplier(
            base,
            nl,
            endpoint_net,
            endpoint_name,
            endpoint_cell,
            early,
            endpoint_objects,
            c
        ),
        Some(f64::INFINITY)
    ) {
        return Vec::new();
    }
    let starts = path_start_objects(nl, endpoint_net);
    let (cells, nets) = if early {
        base.early_path_topology(nl, endpoint_net, endpoint_cell)
    } else {
        base.late_path_topology(nl, endpoint_net, endpoint_cell)
    };
    let stages = path_object_stages(nl, &cells, &nets);
    let path_instances = path_hierarchy_instances(nl, &cells, endpoint_net);
    let matches = |limit: &PathDelayLimit| {
        let to_matches = match &limit.to_object {
            Some(object) => timing_object_matches(object, endpoint_objects, endpoint_name),
            None => limit
                .to
                .as_deref()
                .map(|to| timing_object_name_matches(to, endpoint_objects, endpoint_name))
                .unwrap_or(true),
        };
        let from_matches = match &limit.from_object {
            Some(object) => timing_object_matches(object, &starts, endpoint_name),
            None => limit
                .from
                .as_deref()
                .map(|from| timing_object_name_matches(from, &starts, endpoint_name))
                .unwrap_or(true),
        };
        let through_matches =
            limit.through_groups.is_empty() || through_groups_match(&limit.through_groups, &stages);
        let instance_matches = limit.instance_pattern.as_ref().is_none_or(|pattern| {
            path_instances
                .iter()
                .any(|instance| TimingObjectPattern::new(pattern).matches(instance))
        });
        to_matches && from_matches && through_matches && instance_matches
    };
    let limits = if maximum {
        &c.max_delay_paths
    } else {
        &c.min_delay_paths
    };
    limits
        .iter()
        .filter(|limit| matches(limit))
        .map(|limit| {
            let slack_ns = if maximum {
                limit.limit - arrival
            } else {
                arrival - limit.limit
            };
            AppliedTimingDelayLimit {
                maximum,
                limit: limit.clone(),
                slack_ns,
            }
        })
        .collect()
}
