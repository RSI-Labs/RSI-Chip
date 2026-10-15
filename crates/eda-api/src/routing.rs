//! Placement, global routing, maze routing, rip-up/reroute, and interconnect APIs.

use super::*;

pub fn place_and_route(nl: &Netlist) -> Result<(Placement, Routing), String> {
    place_and_route_cancellable(nl, &FlowCancellation::new())
}

/// Place and globally route with cooperative cancellation.
pub fn place_and_route_cancellable(
    nl: &Netlist,
    cancellation: &FlowCancellation,
) -> Result<(Placement, Routing), String> {
    if cancellation.is_cancelled() {
        return Err("global routing cancelled".into());
    }
    let placement = eda_pnr::place(nl)?;
    let routing = eda_pnr::route_with_options_cancellable(
        nl,
        &placement,
        &RouteOptions::default(),
        cancellation,
    )?;
    Ok((placement, routing))
}

/// Place around hard placement obstructions.
pub fn place_with_blockages(
    nl: &Netlist,
    blockages: &[PlacementBlockage],
) -> Result<Placement, String> {
    eda_pnr::place_with_blockages(nl, blockages)
}

/// Place and route using explicit global-routing capacity options.
pub fn place_and_route_with_options(
    nl: &Netlist,
    options: &RouteOptions,
) -> Result<(Placement, Routing), String> {
    place_and_route_with_options_cancellable(nl, options, &FlowCancellation::new())
}

/// Place and route with options and cooperative cancellation.
pub fn place_and_route_with_options_cancellable(
    nl: &Netlist,
    options: &RouteOptions,
    cancellation: &FlowCancellation,
) -> Result<(Placement, Routing), String> {
    if cancellation.is_cancelled() {
        return Err("global routing cancelled".into());
    }
    let placement = eda_pnr::place(nl)?;
    let routing = eda_pnr::route_with_options_cancellable(nl, &placement, options, cancellation)?;
    Ok((placement, routing))
}

/// Route an existing placement using explicit capacity options.
pub fn route_with_options(
    nl: &Netlist,
    placement: &Placement,
    options: &RouteOptions,
) -> Result<Routing, String> {
    eda_pnr::route_with_options(nl, placement, options)
}

/// Cooperatively cancel capacity-aware global routing.
pub fn route_with_options_cancellable(
    nl: &Netlist,
    placement: &Placement,
    options: &RouteOptions,
    cancellation: &FlowCancellation,
) -> Result<Routing, String> {
    eda_pnr::route_with_options_cancellable(nl, placement, options, cancellation)
}

/// Detailed maze routing using the unified API.
pub fn route_maze(nl: &Netlist, placement: &Placement) -> Result<Routing, String> {
    eda_pnr::route_maze(nl, placement)
}

/// Cooperatively cancel detailed maze routing.
pub fn route_maze_cancellable(
    nl: &Netlist,
    placement: &Placement,
    cancellation: &FlowCancellation,
) -> Result<Routing, String> {
    eda_pnr::route_maze_with_options_cancellable(
        nl,
        placement,
        &RouteOptions::default(),
        cancellation,
    )
}

/// Detailed maze routing with explicit capacity options.
pub fn route_maze_with_options(
    nl: &Netlist,
    placement: &Placement,
    options: &RouteOptions,
) -> Result<Routing, String> {
    eda_pnr::route_maze_with_options(nl, placement, options)
}

/// Cooperatively cancel detailed maze routing with explicit options.
pub fn route_maze_with_options_cancellable(
    nl: &Netlist,
    placement: &Placement,
    options: &RouteOptions,
    cancellation: &FlowCancellation,
) -> Result<Routing, String> {
    eda_pnr::route_maze_with_options_cancellable(nl, placement, options, cancellation)
}

/// Negotiated-congestion rip-up/reroute maze routing.
pub fn route_maze_ripgup(
    nl: &Netlist,
    placement: &Placement,
    options: &RouteOptions,
    max_passes: usize,
) -> Result<MazeRipupResult, String> {
    eda_pnr::route_maze_ripgup(nl, placement, options, max_passes)
}

/// Cooperatively cancel negotiated-congestion rip-up/reroute.
pub fn route_maze_ripgup_cancellable(
    nl: &Netlist,
    placement: &Placement,
    options: &RouteOptions,
    max_passes: usize,
    cancellation: &FlowCancellation,
) -> Result<MazeRipupResult, String> {
    eda_pnr::route_maze_ripgup_cancellable(nl, placement, options, max_passes, cancellation)
}

/// Timing-prioritized negotiated-congestion rip-up/reroute.
pub fn route_maze_timing_ripgup(
    nl: &Netlist,
    placement: &Placement,
    options: &RouteOptions,
    timing: &TimingDrivenRoutingOptions,
    max_passes: usize,
) -> Result<TimingDrivenRipupResult, String> {
    eda_pnr::route_maze_timing_ripgup(nl, placement, options, timing, max_passes)
}

/// Cooperatively cancel timing-prioritized rip-up/reroute.
pub fn route_maze_timing_ripgup_cancellable(
    nl: &Netlist,
    placement: &Placement,
    options: &RouteOptions,
    timing: &TimingDrivenRoutingOptions,
    max_passes: usize,
    cancellation: &FlowCancellation,
) -> Result<TimingDrivenRipupResult, String> {
    eda_pnr::route_maze_timing_ripgup_cancellable(
        nl,
        placement,
        options,
        timing,
        max_passes,
        cancellation,
    )
}

/// Derive late-path net criticality from STA and run timing-prioritized R&R.
pub fn route_maze_timing_ripgup_from_timing(
    nl: &Netlist,
    placement: &Placement,
    options: &RouteOptions,
    timing: &StaResult,
    max_passes: usize,
) -> Result<TimingDrivenRipupResult, String> {
    route_maze_timing_ripgup_from_timing_cancellable(
        nl,
        placement,
        options,
        timing,
        max_passes,
        &FlowCancellation::new(),
    )
}

/// Cooperatively cancel STA-derived timing-prioritized R&R.
pub fn route_maze_timing_ripgup_from_timing_cancellable(
    nl: &Netlist,
    placement: &Placement,
    options: &RouteOptions,
    timing: &StaResult,
    max_passes: usize,
    cancellation: &FlowCancellation,
) -> Result<TimingDrivenRipupResult, String> {
    if cancellation.is_cancelled() {
        return Err("rip-up routing cancelled".into());
    }
    let mut max_arrival = 0.0_f64;
    for net in 0..nl.net_count() {
        if let Some(arrival) = timing.net_arrival(net) {
            max_arrival = max_arrival.max(arrival);
        }
    }
    let mut net_weights = std::collections::HashMap::new();
    if max_arrival > 0.0 {
        for net in 0..nl.net_count() {
            if let Some(arrival) = timing.net_arrival(net) {
                net_weights.insert(net, 1.0 + 9.0 * (arrival / max_arrival));
            }
        }
    }
    let routing_timing = TimingDrivenRoutingOptions {
        net_weights,
        default_net_weight: 1.0,
    };
    eda_pnr::route_maze_timing_ripgup_cancellable(
        nl,
        placement,
        options,
        &routing_timing,
        max_passes,
        cancellation,
    )
}

/// Slack-aware timing-prioritized negotiated-congestion routing.
pub fn route_maze_timing_ripgup_from_constraints(
    nl: &Netlist,
    placement: &Placement,
    options: &RouteOptions,
    timing: &ConstrainedTiming,
    max_passes: usize,
) -> Result<TimingDrivenRipupResult, String> {
    route_maze_timing_ripgup_from_constraints_cancellable(
        nl,
        placement,
        options,
        timing,
        max_passes,
        &FlowCancellation::new(),
    )
}

/// Cooperatively cancel constrained-timing-prioritized routing.
pub fn route_maze_timing_ripgup_from_constraints_cancellable(
    nl: &Netlist,
    placement: &Placement,
    options: &RouteOptions,
    timing: &ConstrainedTiming,
    max_passes: usize,
    cancellation: &FlowCancellation,
) -> Result<TimingDrivenRipupResult, String> {
    if cancellation.is_cancelled() {
        return Err("rip-up routing cancelled".into());
    }
    let net_weights = timing_weights_from_constrained_timing(nl, timing, 9.0)?;
    let routing_timing = TimingDrivenRoutingOptions {
        net_weights,
        default_net_weight: 1.0,
    };
    eda_pnr::route_maze_timing_ripgup_cancellable(
        nl,
        placement,
        options,
        &routing_timing,
        max_passes,
        cancellation,
    )
}

/// Waiver-aware timing-prioritized negotiated-congestion routing.
pub fn route_maze_timing_ripgup_from_waiver_aware_timing(
    nl: &Netlist,
    placement: &Placement,
    options: &RouteOptions,
    timing: &ConstrainedTiming,
    store: &TimingViolationStore,
    max_passes: usize,
) -> Result<TimingDrivenRipupResult, String> {
    route_maze_timing_ripgup_from_waiver_aware_timing_cancellable(
        nl,
        placement,
        options,
        timing,
        store,
        max_passes,
        &FlowCancellation::new(),
    )
}

/// Cooperatively cancel waiver-aware timing-prioritized routing.
pub fn route_maze_timing_ripgup_from_waiver_aware_timing_cancellable(
    nl: &Netlist,
    placement: &Placement,
    options: &RouteOptions,
    timing: &ConstrainedTiming,
    store: &TimingViolationStore,
    max_passes: usize,
    cancellation: &FlowCancellation,
) -> Result<TimingDrivenRipupResult, String> {
    if cancellation.is_cancelled() {
        return Err("rip-up routing cancelled".into());
    }
    let net_weights = timing_weights_from_waiver_aware_timing(nl, timing, store, 9.0)?;
    let routing_timing = TimingDrivenRoutingOptions {
        net_weights,
        default_net_weight: 1.0,
    };
    eda_pnr::route_maze_timing_ripgup_cancellable(
        nl,
        placement,
        options,
        &routing_timing,
        max_passes,
        cancellation,
    )
}

/// lumped interconnect-extraction model over placement-site routing units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InterconnectModel {
    pub wire_resistance_ohm_per_site: f64,
    pub wire_capacitance_pf_per_site: f64,
    pub coupling_capacitance_pf_per_site: f64,
    pub via_resistance_ohm: f64,
    pub via_capacitance_pf: f64,
}

impl Default for InterconnectModel {
    fn default() -> Self {
        Self {
            wire_resistance_ohm_per_site: 0.05,
            wire_capacitance_pf_per_site: 0.002,
            coupling_capacitance_pf_per_site: 0.001,
            via_resistance_ohm: 0.5,
            via_capacitance_pf: 0.001,
        }
    }
}

impl InterconnectModel {
    pub fn validate(&self) -> Result<(), String> {
        let values = [
            (
                "wire resistance per site",
                self.wire_resistance_ohm_per_site,
            ),
            (
                "wire capacitance per site",
                self.wire_capacitance_pf_per_site,
            ),
            (
                "coupling capacitance per site",
                self.coupling_capacitance_pf_per_site,
            ),
            ("via resistance", self.via_resistance_ohm),
            ("via capacitance", self.via_capacitance_pf),
        ];
        for (name, value) in values {
            if !value.is_finite() || value < 0.0 {
                return Err(format!("{} must be finite and non-negative", name));
            }
        }
        Ok(())
    }
}

/// Extract a dependency-free lumped SPEF estimate from routed wire length.
///
/// Grounded capacitance and resistance are summed from each segment and via.
/// Coupling capacitance is estimated between different nets running on
/// adjacent tracks in the same channel with overlapping spans.
pub fn extract_interconnect_parasitics(
    netlist: &Netlist,
    routing: &Routing,
    model: &InterconnectModel,
) -> Result<Spef, String> {
    model.validate()?;
    eda_pnr::check_routing(routing)?;
    if routing.tracks_per_channel == 0 {
        return Err("interconnect extraction requires routing tracks".into());
    }

    #[derive(Clone, Debug, Default)]
    struct ExtractedNet {
        grounded_cap_pf: f64,
        resistance_ohm: f64,
        coupling_pf: f64,
        coupled_nets: Vec<(String, f64)>,
        segment_count: usize,
    }

    let mut extracted = vec![ExtractedNet::default(); netlist.net_count()];
    let mut names = std::collections::BTreeMap::new();
    for ni in 0..netlist.net_count() {
        let name = netlist.net(ni).name.clone();
        if name.trim().is_empty() {
            return Err(format!("net {} has an empty SPEF name", ni));
        }
        if names.insert(name.clone(), ni).is_some() {
            return Err(format!("duplicate netlist net name '{}'", name));
        }
    }

    for segment in &routing.segments {
        if segment.net >= extracted.len() {
            return Err(format!(
                "routing segment references unknown net {}",
                segment.net
            ));
        }
        let length = segment.length() as f64;
        let net = &mut extracted[segment.net];
        net.grounded_cap_pf += length * model.wire_capacitance_pf_per_site;
        net.resistance_ohm += length * model.wire_resistance_ohm_per_site;
        net.segment_count += 1;
    }

    for via in &routing.via_locations {
        if via.net >= extracted.len() {
            return Err(format!("via references unknown net {}", via.net));
        }
        let net = &mut extracted[via.net];
        net.grounded_cap_pf += model.via_capacitance_pf;
        net.resistance_ohm += model.via_resistance_ohm;
        net.segment_count += 1;
    }

    let mut coupling = std::collections::BTreeMap::<(usize, usize), f64>::new();
    for (left_index, left) in routing.segments.iter().enumerate() {
        for right in routing.segments.iter().skip(left_index + 1) {
            if left.net == right.net
                || left.orientation != right.orientation
                || left.channel != right.channel
                || left.track.abs_diff(right.track) != 1
            {
                continue;
            }
            let overlap_start = left.start.max(right.start);
            let overlap_end = left.end.min(right.end);
            if overlap_end > overlap_start {
                let key = (left.net.min(right.net), left.net.max(right.net));
                *coupling.entry(key).or_default() +=
                    (overlap_end - overlap_start) as f64 * model.coupling_capacitance_pf_per_site;
            }
        }
    }
    for ((left, right), value) in coupling {
        let (left_name, right_name) = (&netlist.net(left).name, &netlist.net(right).name);
        let left = &mut extracted[left];
        left.coupling_pf += value;
        left.coupled_nets.push((right_name.clone(), value));
        let right = &mut extracted[right];
        right.coupling_pf += value;
        right.coupled_nets.push((left_name.clone(), value));
    }

    let mut spef = Spef {
        design: netlist.name.clone(),
        units: SpefUnits::default(),
        ..Default::default()
    };
    for (ni, extraction) in extracted.iter().enumerate() {
        let name = netlist.net(ni).name.clone();
        let mut cap_id = 1usize;
        let mut caps = Vec::new();
        if extraction.grounded_cap_pf > 0.0 {
            caps.push(SpefCap {
                id: cap_id,
                node1: name.clone(),
                node2: None,
                value: extraction.grounded_cap_pf,
            });
            cap_id += 1;
        }
        for (other_net, value) in &extraction.coupled_nets {
            caps.push(SpefCap {
                id: cap_id,
                node1: name.clone(),
                node2: Some(other_net.clone()),
                value: *value,
            });
            cap_id += 1;
        }
        let mut resistors = Vec::new();
        if extraction.resistance_ohm > 0.0 {
            resistors.push(SpefRes {
                id: 1,
                node1: format!("{}:a", name),
                node2: format!("{}:b", name),
                value: extraction.resistance_ohm,
            });
        }
        let total_capacitance = extraction.grounded_cap_pf + extraction.coupling_pf;
        spef.nets.insert(
            name.clone(),
            SpefNet {
                name,
                total_capacitance,
                connections: Vec::new(),
                caps,
                resistors,
            },
        );
    }
    Ok(spef)
}

/// Serialize an in-memory SPEF object.
pub fn write_parasitics(spef: &Spef) -> Result<String, String> {
    eda_spef::write_spef(spef)
}
