//! Core timing implementation module.

use super::library::*;
use super::types::*;
use eda_core::netlist::*;

impl StaResult {
    /// Cells on the critical path, driver first.
    pub fn critical_path_cells(&self) -> &[usize] {
        &self.critical_cells
    }

    /// Net-level arrival times computed during analysis.
    pub fn net_arrival(&self, net: NetId) -> Option<f64> {
        self.arrivals
            .get(net)
            .copied()
            .filter(|value| value.is_finite())
    }

    /// Earliest arrival at a net, used for min-delay/hold analysis.
    pub fn net_min_arrival(&self, net: NetId) -> Option<f64> {
        self.min_arrivals
            .get(net)
            .copied()
            .filter(|value| value.is_finite())
    }

    /// Worst-case transition at a net, in nanoseconds.
    ///
    /// Primary inputs currently have a zero transition unless a downstream
    /// flow explicitly annotates them. Library-backed arcs use Liberty
    /// transition tables when available and retain a conservative scalar
    /// fallback otherwise.
    pub fn net_transition(&self, net: NetId) -> Option<f64> {
        self.transitions
            .get(net)
            .copied()
            .filter(|value| value.is_finite() && *value >= 0.0)
    }

    /// Minimum propagated transition at a net, in nanoseconds.
    pub fn net_min_transition(&self, net: NetId) -> Option<f64> {
        self.min_transitions
            .get(net)
            .copied()
            .filter(|value| value.is_finite() && *value >= 0.0)
    }

    /// Selected late critical-path cells and nets ending at `net`.
    ///
    /// This is the topology used to evaluate ordered `-through` objects.
    pub fn late_path_topology(
        &self,
        nl: &Netlist,
        net: NetId,
        endpoint_cell: Option<usize>,
    ) -> (Vec<usize>, Vec<NetId>) {
        let trace = self.latest_path_to(nl, net, endpoint_cell);
        (trace.cells, trace.nets)
    }

    /// Selected early critical-path cells and nets ending at `net`.
    pub fn early_path_topology(
        &self,
        nl: &Netlist,
        net: NetId,
        endpoint_cell: Option<usize>,
    ) -> (Vec<usize>, Vec<NetId>) {
        let trace = self.earliest_path_to(nl, net, endpoint_cell);
        (trace.cells, trace.nets)
    }

    /// Data-pin endpoints of sequential cells.
    pub fn setup_endpoint_list(&self) -> &[(usize, f64)] {
        &self.setup_endpoints
    }

    /// Data-pin endpoints used by hold checks.
    pub fn hold_endpoint_list(&self) -> &[HoldEndpoint] {
        &self.hold_endpoints
    }

    /// Asynchronous reset endpoints used by recovery/removal checks.
    pub fn reset_endpoint_list(&self) -> &[ResetEndpoint] {
        &self.reset_endpoints
    }

    /// Reconstruct the latest-arrival path ending at `net`.
    fn latest_path_to(&self, nl: &Netlist, net: NetId, endpoint_cell: Option<usize>) -> PathTrace {
        let mut cells = Vec::new();
        let mut nets = vec![net];
        let mut arcs = Vec::new();
        let mut current = net;
        let launch = loop {
            let drivers = nl.net(current).drivers.clone();
            let Some(&driver) = drivers.first() else {
                let port = nl.ports.iter().find(|port| port.bits.contains(&current));
                let launch_net = current;
                let arrival = self.net_arrival(launch_net);
                arcs.push(input_launch_arc(launch_net, arrival));
                break TimingLaunch::PrimaryInput(
                    port.map(|port| port.name.clone())
                        .unwrap_or_else(|| nl.net(current).name.clone()),
                );
            };
            cells.push(driver);
            let cell = nl.cell(driver);
            if nl.cell_is_sequential(&cell.kind) {
                let launch_arc =
                    sequential_launch_arc(|net| self.net_arrival(net), nl, driver, current);
                if let Some(arc) = launch_arc {
                    arcs.push(arc);
                }
                break TimingLaunch::SequentialCell(cell.name.clone());
            }

            let Some(&critical_input) = self
                .late_path_inputs
                .get(current)
                .and_then(|input| input.as_ref())
            else {
                let arrival = self.net_arrival(current);
                arcs.push(input_launch_arc(current, arrival));
                break TimingLaunch::PrimaryInput(nl.net(current).name.clone());
            };
            let from_arrival = self.net_arrival(critical_input);
            let to_arrival = self.net_arrival(current);
            arcs.push(combinational_arc(
                driver,
                critical_input,
                current,
                from_arrival,
                to_arrival,
            ));
            nets.push(critical_input);
            current = critical_input;
        };
        cells.reverse();
        if let Some(endpoint) = endpoint_cell {
            cells.push(endpoint);
        }
        nets.reverse();
        arcs.reverse();
        PathTrace {
            launch,
            cells,
            nets,
            arcs,
        }
    }

    /// Reconstruct the earliest-arrival path ending at a sequential data net.
    fn earliest_path_to(
        &self,
        nl: &Netlist,
        net: NetId,
        endpoint_cell: Option<usize>,
    ) -> PathTrace {
        let mut cells = Vec::new();
        let mut nets = vec![net];
        let mut arcs = Vec::new();
        let mut current = net;
        let launch = loop {
            let drivers = nl.net(current).drivers.clone();
            let Some(&driver) = drivers.first() else {
                let port = nl.ports.iter().find(|port| port.bits.contains(&current));
                let launch_net = current;
                let arrival = self.net_min_arrival(launch_net);
                arcs.push(input_launch_arc(launch_net, arrival));
                break TimingLaunch::PrimaryInput(
                    port.map(|port| port.name.clone())
                        .unwrap_or_else(|| nl.net(current).name.clone()),
                );
            };
            cells.push(driver);
            let cell = nl.cell(driver);
            if nl.cell_is_sequential(&cell.kind) {
                let launch_arc =
                    sequential_launch_arc(|net| self.net_min_arrival(net), nl, driver, current);
                if let Some(arc) = launch_arc {
                    arcs.push(arc);
                }
                break TimingLaunch::SequentialCell(cell.name.clone());
            }

            let Some(&critical_input) = self
                .early_path_inputs
                .get(current)
                .and_then(|input| input.as_ref())
            else {
                let arrival = self.net_min_arrival(current);
                arcs.push(input_launch_arc(current, arrival));
                break TimingLaunch::PrimaryInput(nl.net(current).name.clone());
            };
            let from_arrival = self.net_min_arrival(critical_input);
            let to_arrival = self.net_min_arrival(current);
            arcs.push(combinational_arc(
                driver,
                critical_input,
                current,
                from_arrival,
                to_arrival,
            ));
            nets.push(critical_input);
            current = critical_input;
        };
        cells.reverse();
        if let Some(endpoint_cell) = endpoint_cell {
            cells.push(endpoint_cell);
        }
        nets.reverse();
        arcs.reverse();
        PathTrace {
            launch,
            cells,
            nets,
            arcs,
        }
    }

    /// Late setup/output timing paths, sorted from worst arrival to best.
    ///
    /// `max_paths == 0` returns every endpoint path.
    pub fn timing_paths(&self, nl: &Netlist, max_paths: usize) -> Vec<TimingPath> {
        let mut paths = Vec::new();
        for &(endpoint_cell, arrival) in &self.setup_endpoints {
            let endpoint_net = match nl.cell(endpoint_cell).inputs.get(1) {
                Some(&net) => net,
                None => continue,
            };
            let trace = self.latest_path_to(nl, endpoint_net, Some(endpoint_cell));
            let kind = match &trace.launch {
                TimingLaunch::PrimaryInput(_) => TimingPathKind::InputToSequential,
                TimingLaunch::SequentialCell(_) => TimingPathKind::SequentialToSequential,
            };
            paths.push(TimingPath {
                kind,
                launch: trace.launch,
                endpoint: TimingPathEndpoint::SequentialCell(nl.cell(endpoint_cell).name.clone()),
                cells: trace.cells,
                nets: trace.nets,
                arcs: trace.arcs,
                arrival_ns: arrival,
            });
        }

        for port in &nl.ports {
            if port.dir != eda_core::netlist::Dir::Out {
                continue;
            }
            for &net in &port.bits {
                let Some(arrival) = self.net_arrival(net) else {
                    continue;
                };
                if !arrival.is_finite() {
                    continue;
                }
                let trace = self.latest_path_to(nl, net, None);
                let kind = match &trace.launch {
                    TimingLaunch::PrimaryInput(_) => TimingPathKind::InputToOutput,
                    TimingLaunch::SequentialCell(_) => TimingPathKind::SequentialToOutput,
                };
                paths.push(TimingPath {
                    kind,
                    launch: trace.launch,
                    endpoint: TimingPathEndpoint::OutputPort(port.name.clone()),
                    cells: trace.cells,
                    nets: trace.nets,
                    arcs: trace.arcs,
                    arrival_ns: arrival,
                });
            }
        }

        paths.sort_by(|left, right| {
            right
                .arrival_ns
                .total_cmp(&left.arrival_ns)
                .then_with(|| left.cells.len().cmp(&right.cells.len()))
        });
        if max_paths > 0 {
            paths.truncate(max_paths);
        }
        paths
    }

    /// Early min-delay paths to sequential and output endpoints.
    ///
    /// Unlike hold paths, these paths retain raw early arrivals so callers can
    /// audit an explicit `set_min_delay` check independently of hold checks.
    pub fn min_timing_paths(&self, nl: &Netlist, max_paths: usize) -> Vec<TimingPath> {
        let mut paths = Vec::new();
        for endpoint in &self.hold_endpoints {
            let Some(&endpoint_net) = nl.cell(endpoint.cell).inputs.get(1) else {
                continue;
            };
            let Some(arrival) = self.net_min_arrival(endpoint_net) else {
                continue;
            };
            let trace = self.earliest_path_to(nl, endpoint_net, Some(endpoint.cell));
            let kind = match &trace.launch {
                TimingLaunch::PrimaryInput(_) => TimingPathKind::InputToSequential,
                TimingLaunch::SequentialCell(_) => TimingPathKind::SequentialToSequential,
            };
            paths.push(TimingPath {
                kind,
                launch: trace.launch,
                endpoint: TimingPathEndpoint::SequentialCell(nl.cell(endpoint.cell).name.clone()),
                cells: trace.cells,
                nets: trace.nets,
                arcs: trace.arcs,
                arrival_ns: arrival,
            });
        }

        for port in &nl.ports {
            if port.dir != eda_core::netlist::Dir::Out {
                continue;
            }
            for &net in &port.bits {
                let Some(arrival) = self.net_min_arrival(net) else {
                    continue;
                };
                let trace = self.earliest_path_to(nl, net, None);
                let kind = match &trace.launch {
                    TimingLaunch::PrimaryInput(_) => TimingPathKind::InputToOutput,
                    TimingLaunch::SequentialCell(_) => TimingPathKind::SequentialToOutput,
                };
                paths.push(TimingPath {
                    kind,
                    launch: trace.launch,
                    endpoint: TimingPathEndpoint::OutputPort(port.name.clone()),
                    cells: trace.cells,
                    nets: trace.nets,
                    arcs: trace.arcs,
                    arrival_ns: arrival,
                });
            }
        }

        paths.sort_by(|left, right| {
            left.arrival_ns
                .total_cmp(&right.arrival_ns)
                .then_with(|| left.cells.len().cmp(&right.cells.len()))
        });
        if max_paths > 0 {
            paths.truncate(max_paths);
        }
        paths
    }

    /// Early hold paths, sorted from worst slack to best.
    ///
    /// `max_paths == 0` returns every hold endpoint.
    pub fn hold_timing_paths(
        &self,
        nl: &Netlist,
        hold_uncertainty: f64,
        max_paths: usize,
    ) -> Vec<HoldTimingPath> {
        let mut paths = Vec::new();
        for endpoint in &self.hold_endpoints {
            let Some(&endpoint_net) = nl.cell(endpoint.cell).inputs.get(1) else {
                continue;
            };
            let clock_net = nl.cell(endpoint.cell).inputs.first().copied();
            let trace = self.earliest_path_to(nl, endpoint_net, Some(endpoint.cell));
            let kind = match &trace.launch {
                TimingLaunch::PrimaryInput(_) => TimingPathKind::InputToSequential,
                TimingLaunch::SequentialCell(_) => TimingPathKind::SequentialToSequential,
            };
            let path = TimingPath {
                kind,
                launch: trace.launch,
                endpoint: TimingPathEndpoint::SequentialCell(nl.cell(endpoint.cell).name.clone()),
                cells: trace.cells,
                nets: trace.nets,
                arcs: trace.arcs,
                arrival_ns: endpoint.data_arrival,
            };
            let hold = seq_hold_at(
                nl,
                endpoint.cell,
                self.net_min_transition(endpoint_net),
                clock_net.and_then(|net| self.net_min_transition(net)),
            )
            .unwrap_or_else(|_| seq_hold(nl, &nl.cell(endpoint.cell).kind).unwrap_or(0.0));
            let slack = endpoint.data_arrival - hold - hold_uncertainty;
            paths.push(HoldTimingPath {
                path,
                hold_ns: hold,
                slack_ns: slack,
                required_ns: endpoint.data_arrival - slack,
            });
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

    /// Reconstruct late recovery and early removal paths for async resets.
    pub fn reset_timing_paths(&self, nl: &Netlist, max_paths: usize) -> Vec<ResetTimingPaths> {
        let mut paths = Vec::new();
        for endpoint in &self.reset_endpoints {
            let cell = nl.cell(endpoint.cell);
            let Some((reset_pin, _, _)) = seq_reset(nl, &cell.kind) else {
                continue;
            };
            let Some(&reset_net) = cell.inputs.get(reset_pin) else {
                continue;
            };
            let recovery = self.latest_path_to(nl, reset_net, Some(endpoint.cell));
            let removal = self.earliest_path_to(nl, reset_net, Some(endpoint.cell));
            let recovery_kind = match &recovery.launch {
                TimingLaunch::PrimaryInput(_) => TimingPathKind::InputToSequential,
                TimingLaunch::SequentialCell(_) => TimingPathKind::SequentialToSequential,
            };
            let removal_kind = match &removal.launch {
                TimingLaunch::PrimaryInput(_) => TimingPathKind::InputToSequential,
                TimingLaunch::SequentialCell(_) => TimingPathKind::SequentialToSequential,
            };
            paths.push(ResetTimingPaths {
                recovery: TimingPath {
                    kind: recovery_kind,
                    launch: recovery.launch,
                    endpoint: TimingPathEndpoint::SequentialCell(cell.name.clone()),
                    cells: recovery.cells,
                    nets: recovery.nets,
                    arcs: recovery.arcs,
                    arrival_ns: endpoint.reset_arrival,
                },
                removal: TimingPath {
                    kind: removal_kind,
                    launch: removal.launch,
                    endpoint: TimingPathEndpoint::SequentialCell(cell.name.clone()),
                    cells: removal.cells,
                    nets: removal.nets,
                    arcs: removal.arcs,
                    arrival_ns: endpoint.reset_min_arrival,
                },
            });
        }
        if max_paths > 0 {
            paths.truncate(max_paths);
        }
        paths
    }
}
