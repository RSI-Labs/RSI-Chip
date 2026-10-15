//! Constrained timing analysis, derates, checks, slacks, and exceptions.

use super::paths::*;
use super::*;

pub fn analyze_constrained(
    nl: &Netlist,
    base: StaResult,
    c: &TimingConstraints,
) -> Result<ConstrainedTiming, String> {
    let clock = c.default_clock()?;
    let uncertainty = c
        .clock_uncertainty_ns
        .get(&clock.name)
        .copied()
        .unwrap_or(0.0);
    let hold_uncertainty = c
        .hold_clock_uncertainty_ns
        .get(&clock.name)
        .copied()
        .unwrap_or(0.0);
    let source_latency = clock.source_latency_ns;
    let network_latency = clock.network_latency_ns;
    let effective_period = clock.period - uncertainty;
    if effective_period <= 0.0 {
        return Err("effective clock period is non-positive".into());
    }

    let mut exceptions_applied = 0usize;
    let mut reg2reg_slack = f64::INFINITY;
    let mut in2reg_slack = f64::INFINITY;
    let mut reg2reg_hold_slack = f64::INFINITY;
    let mut in2reg_hold_slack = f64::INFINITY;
    let mut max_delay_slack = f64::INFINITY;
    let mut min_delay_slack = f64::INFINITY;
    let mut net_slacks = BTreeMap::new();
    let mut path_cycle_multipliers = BTreeMap::new();
    let mut path_cycle_multipliers_by_endpoint = BTreeMap::new();
    let mut path_exceptions = BTreeMap::new();
    let mut path_max_delay_limits = BTreeMap::new();
    let mut path_min_delay_limits = BTreeMap::new();

    for &(cell_id, data_arrival) in base.setup_endpoint_list() {
        let cell = nl.cell(cell_id);
        let Some(data_net) = cell.inputs.get(1).copied() else {
            continue;
        };
        let endpoint = &nl.net(data_net).name;
        let endpoint_objects = endpoint_timing_objects(nl, data_net, endpoint, Some((cell_id, 1)));
        let endpoint_key = timing_pin_ref(nl, cell_id, 1)
            .unwrap_or_else(|| TimingObjectRef::new(TimingObjectKind::Net, endpoint));
        record_path_exceptions(
            &mut path_exceptions,
            &endpoint_key,
            matching_path_exceptions(
                &base,
                nl,
                data_net,
                endpoint,
                Some(cell_id),
                false,
                &endpoint_objects,
                c,
            ),
        );
        let multiplier = match path_multiplier(
            &base,
            nl,
            data_net,
            endpoint,
            Some(cell_id),
            false,
            &endpoint_objects,
            c,
        ) {
            Some(f64::INFINITY) => {
                exceptions_applied += 1;
                continue;
            }
            Some(m) => m,
            None => 1.0,
        };
        let check_derate = pin_check_derate(nl, c, cell_id, 1);
        let clock_net = cell.inputs.first().copied();
        let setup = crate::seq_setup_at(
            nl,
            cell_id,
            base.net_transition(data_net),
            clock_net.and_then(|net| base.net_transition(net)),
        )? * check_derate.late;
        let slack = effective_period * multiplier - data_arrival - setup;
        path_cycle_multipliers.insert(endpoint.to_string(), multiplier);
        path_cycle_multipliers_by_endpoint.insert(timing_object_key(&endpoint_key), multiplier);
        let endpoint_max_delay_limits = matching_path_delay_limits(
            &base,
            nl,
            data_net,
            endpoint,
            Some(cell_id),
            false,
            c,
            true,
            data_arrival,
            &endpoint_objects,
        );
        let endpoint_min_delay_limits = matching_path_delay_limits(
            &base,
            nl,
            data_net,
            endpoint,
            Some(cell_id),
            true,
            c,
            false,
            data_arrival,
            &endpoint_objects,
        );
        path_max_delay_limits.insert(
            timing_object_key(&endpoint_key),
            endpoint_max_delay_limits.clone(),
        );
        path_min_delay_limits.insert(timing_object_key(&endpoint_key), endpoint_min_delay_limits);
        let net_max_delay = endpoint_max_delay_limits
            .iter()
            .map(|limit| limit.slack_ns)
            .fold(f64::INFINITY, f64::min);
        max_delay_slack = max_delay_slack.min(net_max_delay);
        if crate::driven_from_sequential(nl, data_net) {
            reg2reg_slack = reg2reg_slack.min(slack);
            record_net_slack(
                &mut net_slacks,
                endpoint,
                Some(slack),
                None,
                None,
                Some(net_max_delay),
                None,
            );
        } else {
            let endpoint_slack = slack + network_latency;
            in2reg_slack = in2reg_slack.min(endpoint_slack);
            record_net_slack(
                &mut net_slacks,
                endpoint,
                Some(endpoint_slack),
                None,
                None,
                Some(net_max_delay),
                None,
            );
        }
    }

    for endpoint in base.hold_endpoint_list() {
        let cell = nl.cell(endpoint.cell);
        let Some(data_net) = cell.inputs.get(1).copied() else {
            continue;
        };
        let endpoint_name = &nl.net(data_net).name;
        let endpoint_objects =
            endpoint_timing_objects(nl, data_net, endpoint_name, Some((endpoint.cell, 1)));
        let endpoint_key = timing_pin_ref(nl, endpoint.cell, 1)
            .unwrap_or_else(|| TimingObjectRef::new(TimingObjectKind::Net, endpoint_name));
        record_path_exceptions(
            &mut path_exceptions,
            &endpoint_key,
            matching_path_exceptions(
                &base,
                nl,
                data_net,
                endpoint_name,
                Some(endpoint.cell),
                true,
                &endpoint_objects,
                c,
            ),
        );
        if matches!(
            path_multiplier(
                &base,
                nl,
                data_net,
                endpoint_name,
                Some(endpoint.cell),
                true,
                &endpoint_objects,
                c,
            ),
            Some(f64::INFINITY)
        ) {
            exceptions_applied += 1;
            continue;
        }
        let check_derate = pin_check_derate(nl, c, endpoint.cell, 1);
        let clock_net = cell.inputs.first().copied();
        let hold = crate::seq_hold_at(
            nl,
            endpoint.cell,
            base.net_min_transition(data_net),
            clock_net.and_then(|net| base.net_min_transition(net)),
        )? * check_derate.early;
        let slack = endpoint.data_arrival - hold - hold_uncertainty;
        let endpoint_min_delay_limits = matching_path_delay_limits(
            &base,
            nl,
            data_net,
            endpoint_name,
            Some(endpoint.cell),
            true,
            c,
            false,
            endpoint.data_arrival,
            &endpoint_objects,
        );
        path_min_delay_limits.insert(
            timing_object_key(&endpoint_key),
            endpoint_min_delay_limits.clone(),
        );
        let net_min_delay = endpoint_min_delay_limits
            .iter()
            .map(|limit| limit.slack_ns)
            .fold(f64::INFINITY, f64::min);
        min_delay_slack = min_delay_slack.min(net_min_delay);
        let endpoint_slack = if endpoint.launched_from_sequential {
            slack
        } else {
            slack + network_latency
        };
        record_net_slack(
            &mut net_slacks,
            endpoint_name,
            None,
            Some(endpoint_slack),
            None,
            None,
            Some(net_min_delay),
        );
        if endpoint.launched_from_sequential {
            reg2reg_hold_slack = reg2reg_hold_slack.min(slack);
        } else {
            in2reg_hold_slack = in2reg_hold_slack.min(endpoint_slack);
        }
    }

    let high_width = clock.fall - clock.rise;
    let low_width = clock.period - high_width;
    let mut min_pulse_width_checks = Vec::new();
    for (cell_id, cell) in nl.cells().iter().enumerate() {
        let Some((required_high, required_low)) = crate::seq_min_pulse(nl, &cell.kind) else {
            continue;
        };
        let Some(&clock_net) = cell.inputs.first() else {
            continue;
        };
        let clock_net_name = &nl.net(clock_net).name;
        if base.case_analysis_nets.contains_key(clock_net_name) {
            continue;
        }
        let clock_matches =
            clock.ports.is_empty() || clock.ports.iter().any(|port| port == clock_net_name);
        if clock_matches {
            let check_derate = pin_check_derate(nl, c, cell_id, 0);
            for (polarity, actual, required) in [
                (MinPulsePolarity::High, high_width, required_high),
                (MinPulsePolarity::Low, low_width, required_low),
            ] {
                let effective_required = required * check_derate.late;
                min_pulse_width_checks.push(MinPulseWidthCheck {
                    cell: cell.name.clone(),
                    clock: clock_net_name.clone(),
                    polarity,
                    actual_ns: actual,
                    required_ns: required,
                    effective_required_ns: effective_required,
                    slack_ns: actual - effective_required,
                });
            }
        }
    }
    let min_pulse_high_slack = min_pulse_width_checks
        .iter()
        .filter(|check| check.polarity == MinPulsePolarity::High)
        .map(|check| check.slack_ns)
        .fold(f64::INFINITY, f64::min);
    let min_pulse_low_slack = min_pulse_width_checks
        .iter()
        .filter(|check| check.polarity == MinPulsePolarity::Low)
        .map(|check| check.slack_ns)
        .fold(f64::INFINITY, f64::min);
    let min_pulse_width_slack = min_pulse_high_slack.min(min_pulse_low_slack);

    let mut recovery_slack = f64::INFINITY;
    let mut removal_slack = f64::INFINITY;
    for endpoint in base.reset_endpoint_list() {
        let cell = nl.cell(endpoint.cell);
        let Some((reset_pin, _, _)) = crate::seq_reset(nl, &cell.kind) else {
            continue;
        };
        let Some(&reset_net) = cell.inputs.get(reset_pin) else {
            continue;
        };
        let endpoint_name = &nl.net(reset_net).name;
        let endpoint_objects = endpoint_timing_objects(
            nl,
            reset_net,
            endpoint_name,
            Some((endpoint.cell, reset_pin)),
        );
        let endpoint_key = timing_pin_ref(nl, endpoint.cell, reset_pin)
            .unwrap_or_else(|| TimingObjectRef::new(TimingObjectKind::Net, endpoint_name));
        record_path_exceptions(
            &mut path_exceptions,
            &endpoint_key,
            matching_path_exceptions(
                &base,
                nl,
                reset_net,
                endpoint_name,
                Some(endpoint.cell),
                false,
                &endpoint_objects,
                c,
            ),
        );
        let multiplier = match path_multiplier(
            &base,
            nl,
            reset_net,
            endpoint_name,
            Some(endpoint.cell),
            false,
            &endpoint_objects,
            c,
        ) {
            Some(f64::INFINITY) => {
                exceptions_applied += 1;
                continue;
            }
            Some(multiplier) => multiplier,
            None => 1.0,
        };
        path_cycle_multipliers.insert(endpoint_name.to_string(), multiplier);
        path_cycle_multipliers_by_endpoint.insert(timing_object_key(&endpoint_key), multiplier);
        let clock_net = cell.inputs.first().copied();
        let check_derate = pin_check_derate(nl, c, endpoint.cell, reset_pin);
        let recovery = crate::seq_recovery_at(
            nl,
            endpoint.cell,
            base.net_transition(reset_net),
            clock_net.and_then(|net| base.net_transition(net)),
        )? * check_derate.late;
        let removal = crate::seq_removal_at(
            nl,
            endpoint.cell,
            base.net_min_transition(reset_net),
            clock_net.and_then(|net| base.net_min_transition(net)),
        )? * check_derate.early;
        let endpoint_recovery_slack =
            effective_period * multiplier + network_latency - endpoint.reset_arrival - recovery;
        let endpoint_removal_slack =
            endpoint.reset_min_arrival + network_latency - removal - hold_uncertainty;
        recovery_slack = recovery_slack.min(endpoint_recovery_slack);
        removal_slack = removal_slack.min(endpoint_removal_slack);
        let reset_slack = net_slacks.entry(endpoint_name.clone()).or_default();
        reset_slack.recovery_ns = Some(
            reset_slack
                .recovery_ns
                .map_or(endpoint_recovery_slack, |old| {
                    old.min(endpoint_recovery_slack)
                }),
        );
        reset_slack.removal_ns = Some(
            reset_slack
                .removal_ns
                .map_or(endpoint_removal_slack, |old| {
                    old.min(endpoint_removal_slack)
                }),
        );
    }
    let async_reset_slack = recovery_slack.min(removal_slack);

    let mut output_arrival = f64::NEG_INFINITY;
    let mut output_net = usize::MAX;
    let mut output_name = String::new();
    for port in &nl.ports {
        if port.dir != eda_core::netlist::Dir::Out {
            continue;
        }
        for &net in &port.bits {
            if let Some(arrival) = base.net_arrival(net) {
                let delay = c
                    .output_delays
                    .get(&port.name)
                    .map(|d| d.delay)
                    .unwrap_or(0.0);
                let derate = c
                    .port_timing_derates
                    .get(&port.name)
                    .copied()
                    .unwrap_or_default();
                let external_delay = delay * derate.late;
                if arrival + external_delay > output_arrival {
                    output_arrival = arrival + external_delay;
                    output_net = net;
                    output_name = port.name.clone();
                }
            }
        }
    }

    let output_slack = if output_arrival.is_finite() {
        let launch_latency = if crate::driven_from_sequential(nl, output_net) {
            source_latency + network_latency
        } else {
            0.0
        };
        let endpoint_objects = endpoint_timing_objects(nl, output_net, &output_name, None);
        match path_multiplier(
            &base,
            nl,
            output_net,
            &output_name,
            None,
            false,
            &endpoint_objects,
            c,
        ) {
            Some(f64::INFINITY) => {
                exceptions_applied += 1;
                f64::INFINITY
            }
            Some(m) => effective_period * m - output_arrival - launch_latency,
            None => effective_period - output_arrival - launch_latency,
        }
    } else {
        f64::INFINITY
    };

    for port in &nl.ports {
        if port.dir != eda_core::netlist::Dir::Out {
            continue;
        }
        for &net in &port.bits {
            if let Some(arrival) = base.net_arrival(net) {
                let endpoint_objects = endpoint_timing_objects(nl, net, &port.name, None);
                let output_key = TimingObjectRef::new(TimingObjectKind::Port, &port.name);
                let endpoint_max_delay_limits = matching_path_delay_limits(
                    &base,
                    nl,
                    net,
                    &port.name,
                    None,
                    false,
                    c,
                    true,
                    arrival,
                    &endpoint_objects,
                );
                path_max_delay_limits.insert(
                    timing_object_key(&output_key),
                    endpoint_max_delay_limits.clone(),
                );
                let net_max_delay = endpoint_max_delay_limits
                    .iter()
                    .map(|limit| limit.slack_ns)
                    .fold(f64::INFINITY, f64::min);
                max_delay_slack = max_delay_slack.min(net_max_delay);
                let delay = c
                    .output_delays
                    .get(&port.name)
                    .map(|d| d.delay)
                    .unwrap_or(0.0);
                let derate = c
                    .port_timing_derates
                    .get(&port.name)
                    .copied()
                    .unwrap_or_default();
                let output_arrival = arrival + delay * derate.late;
                let launch_latency = if crate::driven_from_sequential(nl, net) {
                    source_latency + network_latency
                } else {
                    0.0
                };
                let multiplier = match path_multiplier(
                    &base,
                    nl,
                    net,
                    &port.name,
                    None,
                    false,
                    &endpoint_objects,
                    c,
                ) {
                    Some(f64::INFINITY) => f64::INFINITY,
                    Some(multiplier) => multiplier,
                    None => 1.0,
                };
                path_cycle_multipliers.insert(nl.net(net).name.clone(), multiplier);
                path_cycle_multipliers_by_endpoint
                    .insert(timing_object_key(&output_key), multiplier);
                record_path_exceptions(
                    &mut path_exceptions,
                    &output_key,
                    matching_path_exceptions(
                        &base,
                        nl,
                        net,
                        &port.name,
                        None,
                        false,
                        &endpoint_objects,
                        c,
                    ),
                );
                let output_slack = effective_period * multiplier - output_arrival - launch_latency;
                record_net_slack(
                    &mut net_slacks,
                    &nl.net(net).name,
                    None,
                    None,
                    Some(output_slack),
                    Some(net_max_delay),
                    None,
                );
            }
            if let Some(arrival) = base.net_min_arrival(net) {
                let endpoint_objects = endpoint_timing_objects(nl, net, &port.name, None);
                let output_key = TimingObjectRef::new(TimingObjectKind::Port, &port.name);
                let endpoint_min_delay_limits = matching_path_delay_limits(
                    &base,
                    nl,
                    net,
                    &port.name,
                    None,
                    true,
                    c,
                    false,
                    arrival,
                    &endpoint_objects,
                );
                path_min_delay_limits.insert(
                    timing_object_key(&output_key),
                    endpoint_min_delay_limits.clone(),
                );
                let net_min_delay = endpoint_min_delay_limits
                    .iter()
                    .map(|limit| limit.slack_ns)
                    .fold(f64::INFINITY, f64::min);
                min_delay_slack = min_delay_slack.min(net_min_delay);
                record_net_slack(
                    &mut net_slacks,
                    &nl.net(net).name,
                    None,
                    None,
                    None,
                    None,
                    Some(net_min_delay),
                );
            }
        }
    }

    let hold_slack = reg2reg_hold_slack.min(in2reg_hold_slack);
    let worst_slack = reg2reg_slack
        .min(in2reg_slack)
        .min(output_slack)
        .min(hold_slack)
        .min(max_delay_slack)
        .min(min_delay_slack)
        .min(min_pulse_width_slack)
        .min(async_reset_slack);
    Ok(ConstrainedTiming {
        base,
        clock: clock.name.clone(),
        period: clock.period,
        clock_rise: clock.rise,
        clock_fall: clock.fall,
        uncertainty,
        hold_uncertainty,
        source_latency,
        network_latency,
        early_derate: c.timing_derate.early,
        late_derate: c.timing_derate.late,
        cell_derates: c.cell_timing_derates.clone(),
        port_derates: c.port_timing_derates.clone(),
        pin_derates: c.pin_timing_derates.clone(),
        early_check_derate: c.check_timing_derate.early,
        late_check_derate: c.check_timing_derate.late,
        cell_check_derates: c.cell_check_derates.clone(),
        pin_check_derates: c.pin_check_derates.clone(),
        reg2reg_slack,
        in2reg_slack,
        reg2reg_hold_slack,
        in2reg_hold_slack,
        hold_slack,
        output_slack,
        max_delay_slack,
        min_delay_slack,
        min_pulse_high_slack,
        min_pulse_low_slack,
        min_pulse_width_slack,
        min_pulse_width_checks,
        recovery_slack,
        removal_slack,
        async_reset_slack,
        worst_slack,
        exceptions_applied,
        path_cycle_multipliers,
        path_cycle_multipliers_by_endpoint,
        path_exceptions,
        path_max_delay_limits,
        path_min_delay_limits,
        net_slacks,
    })
}

/// Parse SDC, analyze input-delay-adjusted arrivals, and report setup slacks.
pub fn analyze_with_sdc(
    nl: &Netlist,
    constraints: &TimingConstraints,
) -> Result<ConstrainedTiming, String> {
    let base = crate::analyze_with_constraints(nl, constraints)?;
    analyze_constrained(nl, base, constraints)
}

pub(super) fn constrained_clock_edge(
    timing: &ConstrainedTiming,
    local_reference: bool,
    cycle_multiplier: f64,
) -> TimingClockEdge {
    let network_latency = if local_reference {
        timing.network_latency
    } else {
        0.0
    };
    TimingClockEdge {
        clock: timing.clock.clone(),
        edge: TimingClockEdgeKind::Rise,
        edge_time_ns: timing.clock_rise,
        source_latency_ns: timing.source_latency,
        network_latency_ns: network_latency,
        absolute_time_ns: timing.clock_rise
            + timing.source_latency
            + network_latency
            + timing.period * cycle_multiplier,
        virtual_reference: !local_reference,
    }
}

pub(super) fn constrained_path_clocks(
    nl: &Netlist,
    path: &TimingPath,
    timing: &ConstrainedTiming,
    capture_cycles: f64,
) -> TimingPathClocks {
    let launch_local = matches!(path.launch, crate::TimingLaunch::SequentialCell(_));
    let capture_local = matches!(path.endpoint, crate::TimingPathEndpoint::SequentialCell(_));
    let launch = constrained_clock_edge(timing, launch_local, 0.0);
    let capture = constrained_clock_edge(timing, capture_local, capture_cycles);
    let edge_separation_ns = capture.absolute_time_ns - launch.absolute_time_ns;
    let insertion_skew_ns = capture.network_latency_ns - launch.network_latency_ns;
    let _ = nl;
    TimingPathClocks {
        launch,
        capture,
        edge_separation_ns,
        insertion_skew_ns,
    }
}

pub(super) fn timing_object_key(object: &TimingObjectRef) -> String {
    format!("{:?}:{}", object.kind, object.name)
}

pub(super) fn data_path_endpoint_key(nl: &Netlist, path: &TimingPath) -> String {
    match &path.endpoint {
        crate::TimingPathEndpoint::SequentialCell(_) => path
            .cells
            .last()
            .and_then(|&cell| timing_pin_ref(nl, cell, 1))
            .map(|pin| timing_object_key(&pin))
            .unwrap_or_else(|| {
                timing_object_key(&TimingObjectRef::new(
                    TimingObjectKind::Net,
                    path.nets
                        .last()
                        .map(|&net| nl.net(net).name.clone())
                        .unwrap_or_default(),
                ))
            }),
        crate::TimingPathEndpoint::OutputPort(name) => {
            timing_object_key(&TimingObjectRef::new(TimingObjectKind::Port, name))
        }
    }
}

pub(super) fn reset_path_endpoint_key(nl: &Netlist, path: &TimingPath) -> String {
    path.cells
        .last()
        .and_then(|&cell| {
            crate::seq_reset(nl, &nl.cell(cell).kind)
                .and_then(|(pin, _, _)| timing_pin_ref(nl, cell, pin))
        })
        .map(|pin| timing_object_key(&pin))
        .unwrap_or_else(|| {
            timing_object_key(&TimingObjectRef::new(
                TimingObjectKind::Net,
                path.nets
                    .last()
                    .map(|&net| nl.net(net).name.clone())
                    .unwrap_or_default(),
            ))
        })
}

pub(super) fn endpoint_cycle_multiplier(
    nl: &Netlist,
    path: &TimingPath,
    timing: &ConstrainedTiming,
) -> f64 {
    let endpoint_key = data_path_endpoint_key(nl, path);
    if let Some(multiplier) = timing.path_cycle_multipliers_by_endpoint.get(&endpoint_key) {
        return *multiplier;
    }
    path.nets
        .last()
        .map(|&net| {
            timing
                .path_cycle_multipliers
                .get(&nl.net(net).name)
                .copied()
                .unwrap_or(1.0)
        })
        .unwrap_or(1.0)
}

pub(super) fn reset_endpoint_cycle_multiplier(
    nl: &Netlist,
    path: &TimingPath,
    timing: &ConstrainedTiming,
) -> f64 {
    let endpoint_key = reset_path_endpoint_key(nl, path);
    if let Some(multiplier) = timing.path_cycle_multipliers_by_endpoint.get(&endpoint_key) {
        return *multiplier;
    }
    path.nets
        .last()
        .map(|&net| {
            timing
                .path_cycle_multipliers
                .get(&nl.net(net).name)
                .copied()
                .unwrap_or(1.0)
        })
        .unwrap_or(1.0)
}
