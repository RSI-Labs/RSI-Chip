//! # eda-sta — static timing analysis
//!
//! Graph-based arrival propagation over the gate netlist:
//! - primary inputs launch at time 0;
//! - combinational cells (generic or library) add their input→output arc
//!   delays, taking the max over input arrivals;
//! - sequential cells terminate combinational paths at their data pins
//!   (checked against setup) and relaunch from their outputs at `clk_to_q`;
//! - `reg2reg_critical` = clk→q + combinational logic + setup of the worst
//!   register-to-register path;
//! - `critical_path_delay` = the worst arrival reaching any observable
//!   endpoint (output port or dangling sink).

use eda_core::logic::LogicValue;
use eda_core::netlist::{CellKind, NetId, Netlist};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub mod block;
pub mod constraints;
pub mod multicorner;
pub mod quality;
pub mod violations;
pub use block::{
    block_timing_composition_report, compose_block_timing_models,
    compose_block_timing_models_with_clock_domains, compose_block_timing_models_with_clock_maps,
};
pub use block::{
    block_timing_model, block_timing_model_report, parse_block_timing_model,
    write_block_timing_model, BlockTimingClock, BlockTimingClockMap, BlockTimingClockRelationship,
    BlockTimingCompositionConnection, BlockTimingCompositionInstance, BlockTimingCompositionResult,
    BlockTimingDirection, BlockTimingHoldInterfaceCheck, BlockTimingInterfaceCheck,
    BlockTimingModel, BlockTimingPort,
};
pub use constraints::{
    analyze_constrained, analyze_with_sdc, constrained_hold_timing_path_report,
    constrained_hold_timing_paths, constrained_report, constrained_reset_timing_path_report,
    constrained_reset_timing_paths, constrained_timing_path_report, constrained_timing_paths,
    diff_timing_constraints, excluded_timing_path_report, excluded_timing_paths, parse_sdc,
    AppliedTimingDelayLimit, CaseAnalysis, Clock, ClockGroup, ClockGroupKind,
    ConstrainedHoldTimingPath, ConstrainedResetTimingPath, ConstrainedTiming,
    ConstrainedTimingPath, DisabledTimingArc, ExcludedTimingCheck, ExcludedTimingPath,
    MinPulsePolarity, MinPulseWidthCheck, NetSlack, PathDelayLimit, PathException, PortDelay,
    ResetCheckKind, ScopedCellTimingDerate, ScopedPinTimingDerate, SdcUnits, TimingClockEdge,
    TimingClockEdgeKind, TimingConstraintDiff, TimingConstraints, TimingDerate, TimingObjectKind,
    TimingObjectPattern, TimingObjectPatternToken, TimingObjectRef, TimingPathClocks, TimingPin,
};
pub use multicorner::{
    analyze_timing_scenario_definitions, analyze_timing_scenarios, diff_timing_scenario_sets,
    diff_timing_scenarios, multi_corner_hold_timing_path_report, multi_corner_hold_timing_paths,
    multi_corner_report, multi_corner_reset_timing_path_report, multi_corner_reset_timing_paths,
    multi_corner_setup_timing_path_report, multi_corner_setup_timing_paths, parse_timing_scenarios,
    timing_scenario_diff_report, update_timing_scenarios_incrementally,
    update_timing_scenarios_with_topology_incrementally, CornerTiming,
    IncrementalMultiCornerTimingResult, IncrementalMultiCornerTopologyTimingResult,
    MultiCornerHoldTimingPath, MultiCornerResetTimingPath, MultiCornerScenarioSetDiff,
    MultiCornerScenarioUpdate, MultiCornerScenarioUpdateKind, MultiCornerSetupTimingPath,
    MultiCornerTimingResult, ScenarioSlack, TimingScenario, TimingScenarioDefinition,
    TimingScenarioDiff, TimingScenarioPropertyDiff,
};
pub use quality::{
    analyze_quality, quality_report, QualityKind, QualityOptions, QualityResult, QualityViolation,
    ScopedQualityLimit,
};
pub use violations::{
    active_timing_waivers_from_audit, parse_timing_violation_waivers, parse_timing_waiver_audit,
    timing_violation_dirty_objects_from_constraints, timing_violation_dirty_objects_from_topology,
    timing_violation_registry_from_stores, timing_violation_registry_report,
    timing_violation_report, timing_waiver_audit_hierarchy_report,
    timing_waiver_audit_hierarchy_summaries, timing_waiver_audit_report,
    write_timing_violation_waivers, write_timing_waiver_audit,
    CrossCornerHierarchyViolationSummary, CrossCornerTimingViolation,
    CrossCornerTimingViolationChange, CrossCornerTimingViolationDiff, TimingViolation,
    TimingViolationChange, TimingViolationDiff, TimingViolationDirtyObjects, TimingViolationKind,
    TimingViolationPath, TimingViolationRegistry, TimingViolationRegistryUpdate,
    TimingViolationStore, TimingViolationWaiver, TimingWaiverAuditAction, TimingWaiverAuditEntry,
    TimingWaiverAuditHierarchySummary,
};

#[derive(Clone, Debug)]
pub struct StaResult {
    /// Arrival time at the worst combinational endpoint (input-launched or
    /// flop-launched).
    pub critical_path_delay: f64,
    /// Worst register-to-register path: clk→q + logic + setup.
    pub reg2reg_critical: f64,
    /// Worst input-to-register setup path (0 when no flop samples a primary
    /// input through logic).
    pub in2reg_critical: f64,
    /// Earliest register-to-register data arrival used by hold checks.
    pub reg2reg_hold_critical: f64,
    /// Earliest input-to-register data arrival used by hold checks.
    pub in2reg_hold_critical: f64,
    /// Number of concrete timing arcs and timing checks removed by SDC.
    pub disabled_timing_arcs: usize,
    /// Constant nets propagated from `set_case_analysis`.
    pub case_analysis_nets: BTreeMap<String, bool>,
    /// Arrival per net.
    arrivals: Vec<f64>,
    /// Earliest arrival per net.
    min_arrivals: Vec<f64>,
    /// Worst-case propagated transition per net, in nanoseconds. NaN marks a
    /// net with no propagated timing event.
    transitions: Vec<f64>,
    /// Minimum propagated transition per net, used by early/hold analysis.
    min_transitions: Vec<f64>,
    /// Selected late critical input per combinational output net.
    late_path_inputs: Vec<Option<NetId>>,
    /// Selected early critical input per combinational output net.
    early_path_inputs: Vec<Option<NetId>>,
    /// Cell sequence (driver first) of the critical path.
    critical_cells: Vec<usize>,
    /// (sequential cell id, data arrival) endpoints.
    setup_endpoints: Vec<(usize, f64)>,
    hold_endpoints: Vec<HoldEndpoint>,
    reset_endpoints: Vec<ResetEndpoint>,
}

/// Result and work accounting for a same-topology incremental timing update.
#[derive(Clone, Debug)]
pub struct IncrementalTimingResult {
    pub timing: StaResult,
    /// Input ports explicitly marked changed by the caller.
    pub changed_ports: BTreeSet<String>,
    /// Nets whose late/early arrivals were recomputed.
    pub recomputed_nets: BTreeSet<String>,
    /// Cells whose output arcs were recomputed.
    pub recomputed_cells: BTreeSet<String>,
    /// Total combinational cells in the unchanged netlist topology.
    pub total_cells: usize,
}

/// Structural differences relevant to incremental timing after an ECO.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimingTopologyDiff {
    pub added_cells: BTreeSet<String>,
    pub removed_cells: BTreeSet<String>,
    pub changed_cells: BTreeSet<String>,
    pub added_nets: BTreeSet<String>,
    pub removed_nets: BTreeSet<String>,
    /// Existing nets whose driver or port mapping changed.
    pub changed_nets: BTreeSet<String>,
    pub changed_ports: BTreeSet<String>,
}

/// Incremental timing result annotated with an old/new topology diff.
#[derive(Clone, Debug)]
pub struct TopologyTimingResult {
    pub timing: StaResult,
    pub diff: TimingTopologyDiff,
    pub recomputed_nets: BTreeSet<String>,
    pub recomputed_cells: BTreeSet<String>,
    pub total_cells: usize,
}

/// Base and constrained timing produced by an SDC constraint incremental update.
#[derive(Clone, Debug)]
pub struct ConstraintTimingResult {
    pub timing: StaResult,
    pub constrained: ConstrainedTiming,
    pub diff: constraints::TimingConstraintDiff,
    pub recomputed_nets: BTreeSet<String>,
    pub recomputed_cells: BTreeSet<String>,
    pub total_cells: usize,
    pub full_base_recompute: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct HoldEndpoint {
    pub cell: usize,
    /// Earliest data-pin arrival in nanoseconds.
    pub data_arrival: f64,
    pub launched_from_sequential: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct ResetEndpoint {
    pub cell: usize,
    /// Latest asynchronous-reset arrival in nanoseconds.
    pub reset_arrival: f64,
    /// Earliest asynchronous-reset arrival in nanoseconds.
    pub reset_min_arrival: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimingLaunch {
    PrimaryInput(String),
    SequentialCell(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimingPathEndpoint {
    SequentialCell(String),
    OutputPort(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingPathKind {
    InputToSequential,
    SequentialToSequential,
    InputToOutput,
    SequentialToOutput,
}

/// Classification of one incremental stage in a reconstructed timing path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingArcKind {
    PrimaryInputLaunch,
    ClockToOutput,
    Combinational,
}

/// One launch or cell timing arc in driver-first path order.
///
/// Delays are derived from the analyzed arrival difference, not by re-reading
/// the nominal library arc. This preserves SDF back-annotation and all
/// late/early timing derates used by the analysis.
#[derive(Clone, Debug, PartialEq)]
pub struct TimingArc {
    pub kind: TimingArcKind,
    pub cell: Option<usize>,
    pub from_net: Option<NetId>,
    pub to_net: NetId,
    pub from_arrival_ns: Option<f64>,
    pub to_arrival_ns: f64,
    pub delay_ns: f64,
}

#[derive(Clone, Debug)]
pub struct TimingPath {
    pub kind: TimingPathKind,
    pub launch: TimingLaunch,
    pub endpoint: TimingPathEndpoint,
    /// Driver-first cells, including the launch flop and capture flop when
    /// they are present.
    pub cells: Vec<usize>,
    /// Nets from launch through the endpoint, inclusive.
    pub nets: Vec<NetId>,
    /// Incremental launch and cell arcs in driver-first order.
    pub arcs: Vec<TimingArc>,
    /// Late data arrival at the endpoint net in nanoseconds.
    pub arrival_ns: f64,
}

/// One contiguous group of timing arcs attributed to a hierarchy instance.
#[derive(Clone, Debug, PartialEq)]
pub struct HierarchicalTimingSegment {
    pub instance: Option<String>,
    pub module: Option<String>,
    pub first_arc: usize,
    pub last_arc: usize,
    pub cells: Vec<usize>,
    pub delay_ns: f64,
}

/// One timing-arc transition between hierarchy instances.
#[derive(Clone, Debug, PartialEq)]
pub struct HierarchicalTimingCrossing {
    pub arc: usize,
    pub cell: usize,
    pub cell_name: String,
    pub from_instance: Option<String>,
    pub to_instance: Option<String>,
}

/// A timing path grouped by retained post-synthesis module instances.
#[derive(Clone, Debug)]
pub struct HierarchicalTimingPath {
    pub path: TimingPath,
    pub segments: Vec<HierarchicalTimingSegment>,
    pub crossings: Vec<HierarchicalTimingCrossing>,
}

#[derive(Clone, Debug)]
pub struct HoldTimingPath {
    pub path: TimingPath,
    pub hold_ns: f64,
    pub slack_ns: f64,
    /// Latest allowed early arrival: `arrival - slack`.
    pub required_ns: f64,
}

/// Late recovery and early removal paths for one asynchronous reset endpoint.
#[derive(Clone, Debug)]
pub struct ResetTimingPaths {
    pub recovery: TimingPath,
    pub removal: TimingPath,
}

/// Internal path reconstruction result.
#[derive(Debug)]
struct PathTrace {
    launch: TimingLaunch,
    cells: Vec<usize>,
    nets: Vec<NetId>,
    arcs: Vec<TimingArc>,
}

fn input_launch_arc(net: NetId, arrival: Option<f64>) -> TimingArc {
    let to_arrival = arrival.filter(|value| value.is_finite()).unwrap_or(0.0);
    TimingArc {
        kind: TimingArcKind::PrimaryInputLaunch,
        cell: None,
        from_net: Some(net),
        to_net: net,
        from_arrival_ns: None,
        to_arrival_ns: to_arrival,
        delay_ns: to_arrival,
    }
}

fn sequential_clock_pin(nl: &Netlist, kind: &CellKind) -> Option<usize> {
    match kind {
        CellKind::Lib(id) => nl
            .library()
            .and_then(|library| library.get(*id).seq.as_ref().map(|seq| seq.clk_pin)),
        CellKind::Dff { .. } | CellKind::DffR { .. } => Some(0),
        _ => None,
    }
}

fn sequential_launch_arc<F>(
    arrival: F,
    nl: &Netlist,
    cell_id: usize,
    output: NetId,
) -> Option<TimingArc>
where
    F: Fn(NetId) -> Option<f64>,
{
    let cell = nl.cell(cell_id);
    let clock_pin = sequential_clock_pin(nl, &cell.kind)?;
    let clock_net = cell.inputs.get(clock_pin).copied()?;
    let to_arrival = arrival(output).filter(|value| value.is_finite())?;
    let from_arrival = arrival(clock_net).filter(|value| value.is_finite());
    Some(TimingArc {
        kind: TimingArcKind::ClockToOutput,
        cell: Some(cell_id),
        from_net: Some(clock_net),
        to_net: output,
        from_arrival_ns: from_arrival,
        to_arrival_ns: to_arrival,
        delay_ns: from_arrival.map_or(to_arrival, |from| to_arrival - from),
    })
}

fn combinational_arc(
    cell: usize,
    from_net: NetId,
    to_net: NetId,
    from_arrival: Option<f64>,
    to_arrival: Option<f64>,
) -> TimingArc {
    let from = from_arrival.filter(|value| value.is_finite());
    let to = to_arrival.filter(|value| value.is_finite()).unwrap_or(0.0);
    TimingArc {
        kind: TimingArcKind::Combinational,
        cell: Some(cell),
        from_net: Some(from_net),
        to_net,
        from_arrival_ns: from,
        to_arrival_ns: to,
        delay_ns: from.map_or(to, |from| to - from),
    }
}

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

/// Worst slack against a single clock `period` (ns).
pub fn worst_slack(r: &StaResult, period: f64) -> f64 {
    let limiting = r
        .reg2reg_critical
        .max(r.in2reg_critical)
        .max(r.critical_path_delay);
    period - limiting
}

/// Worst unconstrained hold slack in nanoseconds. Hold uncertainty tightens
/// the required minimum data-stable time.
pub fn worst_hold_slack(nl: &Netlist, r: &StaResult, hold_uncertainty: f64) -> f64 {
    r.hold_endpoint_list()
        .iter()
        .map(|endpoint| {
            let data_net = nl.cell(endpoint.cell).inputs.get(1).copied();
            let clock_net = nl.cell(endpoint.cell).inputs.first().copied();
            let hold = seq_hold_at(
                nl,
                endpoint.cell,
                data_net.and_then(|net| r.net_min_transition(net)),
                clock_net.and_then(|net| r.net_min_transition(net)),
            )
            .unwrap_or_else(|_| seq_hold(nl, &nl.cell(endpoint.cell).kind).unwrap_or(0.0));
            endpoint.data_arrival - hold - hold_uncertainty
        })
        .fold(f64::INFINITY, f64::min)
}

fn generic_input_pin_label(index: usize) -> String {
    match index {
        0 => "a".into(),
        1 => "b".into(),
        2 => "c".into(),
        3 => "d".into(),
        index => index.to_string(),
    }
}

fn timing_arc_input_pin(nl: &Netlist, arc: &TimingArc) -> String {
    let Some(cell_id) = arc.cell else {
        return arc
            .from_net
            .map(|net| nl.net(net).name.clone())
            .unwrap_or_else(|| "<none>".into());
    };
    let cell = nl.cell(cell_id);
    match (&cell.kind, arc.from_net) {
        (CellKind::Lib(id), Some(from_net)) => {
            let Some(library) = nl.library() else {
                return "<none>".into();
            };
            let definition = library.get(*id);
            cell.inputs
                .iter()
                .position(|&net| net == from_net)
                .and_then(|pin| definition.inputs.get(pin).cloned())
                .unwrap_or_else(|| "<none>".into())
        }
        (_, Some(from_net)) => cell
            .inputs
            .iter()
            .position(|&net| net == from_net)
            .map(generic_input_pin_label)
            .unwrap_or_else(|| "<none>".into()),
        (_, None) => "<none>".into(),
    }
}

fn timing_arc_output_pin(nl: &Netlist, arc: &TimingArc) -> String {
    let Some(cell_id) = arc.cell else {
        return nl.net(arc.to_net).name.clone();
    };
    output_pin_name(nl, cell_id).unwrap_or_else(|| nl.net(arc.to_net).name.clone())
}

fn push_timing_arc_report(nl: &Netlist, arc: &TimingArc, out: &mut String) {
    let from_net = arc
        .from_net
        .map(|net| nl.net(net).name.clone())
        .unwrap_or_else(|| "<none>".into());
    let to_net = nl.net(arc.to_net).name.clone();
    let description = match arc.kind {
        TimingArcKind::PrimaryInputLaunch => format!(
            "input {} -> {}: launch delay {:.6} ns, arrival {:.6} ns",
            from_net, to_net, arc.delay_ns, arc.to_arrival_ns
        ),
        TimingArcKind::ClockToOutput => {
            let cell_name = arc
                .cell
                .map(|cell| nl.cell(cell).name.clone())
                .unwrap_or_default();
            let input_pin = timing_arc_input_pin(nl, arc);
            let output_pin = timing_arc_output_pin(nl, arc);
            format!(
                "{}/{} -> {}/{}: clock-to-output {:.6} ns, arrival {:.6} ns",
                cell_name, input_pin, cell_name, output_pin, arc.delay_ns, arc.to_arrival_ns
            )
        }
        TimingArcKind::Combinational => {
            let cell_name = arc
                .cell
                .map(|cell| nl.cell(cell).name.clone())
                .unwrap_or_default();
            let input_pin = timing_arc_input_pin(nl, arc);
            let output_pin = timing_arc_output_pin(nl, arc);
            format!(
                "{}/{} -> {}/{}: combinational {:.6} ns, arrival {:.6} ns",
                cell_name, input_pin, cell_name, output_pin, arc.delay_ns, arc.to_arrival_ns
            )
        }
    };
    out.push_str(&format!("    {}\n", description));
}

fn push_timing_arcs_report(nl: &Netlist, arcs: &[TimingArc], out: &mut String) {
    if arcs.is_empty() {
        out.push_str("    arcs: <none>\n");
        return;
    }
    out.push_str("    arcs:\n");
    for arc in arcs {
        push_timing_arc_report(nl, arc, out);
    }
}

/// Human-readable timing-path report.
pub fn timing_path_report(nl: &Netlist, paths: &[TimingPath]) -> String {
    let mut out = format!("Timing path report: {} path(s)\n", paths.len());
    for path in paths {
        let kind = match path.kind {
            TimingPathKind::InputToSequential => "input-to-sequential",
            TimingPathKind::SequentialToSequential => "sequential-to-sequential",
            TimingPathKind::InputToOutput => "input-to-output",
            TimingPathKind::SequentialToOutput => "sequential-to-output",
        };
        let launch = match &path.launch {
            TimingLaunch::PrimaryInput(name) => format!("input {}", name),
            TimingLaunch::SequentialCell(name) => format!("flop {}", name),
        };
        let endpoint = match &path.endpoint {
            TimingPathEndpoint::SequentialCell(name) => format!("flop {}", name),
            TimingPathEndpoint::OutputPort(name) => format!("output {}", name),
        };
        out.push_str(&format!(
            "  {} -> {}: arrival {:.6} ns\n    launch: {}\n    endpoint: {}\n",
            kind, endpoint, path.arrival_ns, launch, endpoint
        ));
        if path.cells.is_empty() {
            out.push_str("    cells: <none>\n");
        } else {
            let cells = path
                .cells
                .iter()
                .map(|&cell| nl.cell(cell).name.clone())
                .collect::<Vec<_>>()
                .join(" -> ");
            out.push_str(&format!("    cells: {}\n", cells));
        }
        let nets = path
            .nets
            .iter()
            .map(|&net| nl.net(net).name.clone())
            .collect::<Vec<_>>()
            .join(" -> ");
        out.push_str(&format!("    nets: {}\n", nets));
        push_timing_arcs_report(nl, &path.arcs, &mut out);
    }
    out
}

/// Group one timing path by the hierarchy instance that owns each cell.
pub fn hierarchical_timing_path(
    nl: &Netlist,
    path: &TimingPath,
) -> Result<HierarchicalTimingPath, String> {
    #[derive(Clone)]
    struct OpenSegment {
        instance: Option<String>,
        module: Option<String>,
        first_arc: usize,
        last_arc: usize,
        cells: Vec<usize>,
        delay_ns: f64,
    }

    let mut segments: Vec<OpenSegment> = Vec::new();
    let mut crossings = Vec::new();
    let mut previous_instance: Option<String> = None;

    for (arc_index, arc) in path.arcs.iter().enumerate() {
        if !arc.delay_ns.is_finite() || arc.delay_ns < 0.0 {
            return Err(format!(
                "timing arc {} has invalid delay {}",
                arc_index, arc.delay_ns
            ));
        }
        let owner = arc
            .cell
            .map(|cell| {
                if cell >= nl.cell_count() {
                    return Err(format!("timing arc {} references cell {}", arc_index, cell));
                }
                Ok(nl
                    .cell_hierarchy_instance(cell)
                    .map(|instance| (instance.path.clone(), instance.module.clone())))
            })
            .transpose()?
            .flatten();
        let (instance, module) = owner
            .clone()
            .map_or((None, None), |(path, module)| (Some(path), Some(module)));

        if segments.last().map(|segment| &segment.instance) != Some(&instance) {
            let previous = previous_instance.clone();
            if previous != instance {
                let cell = arc
                    .cell
                    .ok_or_else(|| format!("crossing arc {} has no cell", arc_index))?;
                crossings.push(HierarchicalTimingCrossing {
                    arc: arc_index,
                    cell,
                    cell_name: nl.cell(cell).name.clone(),
                    from_instance: previous,
                    to_instance: instance.clone(),
                });
            }
            segments.push(OpenSegment {
                instance: instance.clone(),
                module,
                first_arc: arc_index,
                last_arc: arc_index,
                cells: Vec::new(),
                delay_ns: 0.0,
            });
        }
        let segment = segments
            .last_mut()
            .ok_or_else(|| format!("timing arc {} produced no hierarchy segment", arc_index))?;
        segment.last_arc = arc_index;
        segment.delay_ns += arc.delay_ns;
        if let Some(cell) = arc.cell {
            if !segment.cells.contains(&cell) {
                segment.cells.push(cell);
            }
        }
        previous_instance = instance;
    }

    Ok(HierarchicalTimingPath {
        path: path.clone(),
        segments: segments
            .into_iter()
            .map(|segment| HierarchicalTimingSegment {
                instance: segment.instance,
                module: segment.module,
                first_arc: segment.first_arc,
                last_arc: segment.last_arc,
                cells: segment.cells,
                delay_ns: segment.delay_ns,
            })
            .collect(),
        crossings,
    })
}

/// Group timing paths by retained hierarchy instances.
pub fn hierarchical_timing_paths(
    nl: &Netlist,
    paths: &[TimingPath],
) -> Result<Vec<HierarchicalTimingPath>, String> {
    paths
        .iter()
        .map(|path| hierarchical_timing_path(nl, path))
        .collect()
}

/// Render a hierarchy-grouped timing path report.
pub fn hierarchical_timing_path_report(paths: &[HierarchicalTimingPath]) -> String {
    let mut out = format!("Hierarchical timing path report: {} path(s)\n", paths.len());
    for hierarchical in paths {
        let path = &hierarchical.path;
        out.push_str(&format!(
            "  arrival {:.6} ns, {} segment(s), {} crossing(s)\n",
            path.arrival_ns,
            hierarchical.segments.len(),
            hierarchical.crossings.len()
        ));
        for segment in &hierarchical.segments {
            let instance = segment.instance.as_deref().unwrap_or("<top-level>");
            let module = segment.module.as_deref().unwrap_or("<flat>");
            out.push_str(&format!(
                "    instance '{}' module '{}': arcs {}..{}, cells {}, delay {:.6} ns\n",
                instance,
                module,
                segment.first_arc,
                segment.last_arc,
                segment.cells.len(),
                segment.delay_ns
            ));
        }
        for crossing in &hierarchical.crossings {
            out.push_str(&format!(
                "    crossing arc {}: {} -> {} at cell '{}'\n",
                crossing.arc,
                crossing.from_instance.as_deref().unwrap_or("<top-level>"),
                crossing.to_instance.as_deref().unwrap_or("<top-level>"),
                crossing.cell_name
            ));
        }
    }
    out
}

/// Human-readable hold-path report.
pub fn hold_timing_path_report(nl: &Netlist, paths: &[HoldTimingPath]) -> String {
    let mut out = format!("Hold timing path report: {} path(s)\n", paths.len());
    for hold in paths {
        let path = &hold.path;
        out.push_str(&format!(
            "  hold slack {:.6} ns, arrival {:.6} ns, required {:.6} ns, hold {:.6} ns\n",
            hold.slack_ns, path.arrival_ns, hold.required_ns, hold.hold_ns
        ));
        let cells = path
            .cells
            .iter()
            .map(|&cell| nl.cell(cell).name.clone())
            .collect::<Vec<_>>()
            .join(" -> ");
        let nets = path
            .nets
            .iter()
            .map(|&net| nl.net(net).name.clone())
            .collect::<Vec<_>>()
            .join(" -> ");
        out.push_str(&format!("    cells: {}\n    nets: {}\n", cells, nets));
        push_timing_arcs_report(nl, &path.arcs, &mut out);
    }
    out
}

/// Human-readable asynchronous-reset path report.
pub fn reset_timing_path_report(nl: &Netlist, paths: &[ResetTimingPaths]) -> String {
    let mut out = format!("Reset timing path report: {} endpoint(s)\n", paths.len());
    for reset in paths {
        for (kind, path) in [("recovery", &reset.recovery), ("removal", &reset.removal)] {
            let cells = path
                .cells
                .iter()
                .map(|&cell| nl.cell(cell).name.clone())
                .collect::<Vec<_>>()
                .join(" -> ");
            let nets = path
                .nets
                .iter()
                .map(|&net| nl.net(net).name.clone())
                .collect::<Vec<_>>()
                .join(" -> ");
            out.push_str(&format!(
                "  {} arrival {:.6} ns\n    endpoint: {}\n    cells: {}\n    nets: {}\n",
                kind,
                path.arrival_ns,
                match &path.endpoint {
                    TimingPathEndpoint::SequentialCell(name) => format!("flop {}", name),
                    TimingPathEndpoint::OutputPort(name) => format!("output {}", name),
                },
                cells,
                nets
            ));
            push_timing_arcs_report(nl, &path.arcs, &mut out);
        }
    }
    out
}

/// Analyze `nl`. Requires library-backed `Lib` cells (`Netlist::set_library`)
/// for arc delays; generic primitives get a nominal 0.5 ns per level.
fn clock_transition_bounds(
    constraints: Option<&constraints::TimingConstraints>,
    port: &str,
) -> (f64, f64) {
    let Some(constraints) = constraints else {
        return (0.0, 0.0);
    };
    let mut bounds = (0.0_f64, 0.0_f64);
    for clock in constraints
        .clocks
        .values()
        .filter(|clock| clock.ports.iter().any(|clock_port| clock_port == port))
    {
        bounds.0 = bounds.0.max(
            clock
                .min_rise_transition_ns
                .max(clock.min_fall_transition_ns),
        );
        bounds.1 = bounds
            .1
            .max(clock.rise_transition_ns.max(clock.fall_transition_ns));
    }
    bounds
}

pub fn analyze(nl: &Netlist) -> Result<StaResult, String> {
    analyze_impl(nl, None)
}

/// Analyze with SDC input delays applied as primary-input arrival offsets.
pub fn analyze_with_constraints(
    nl: &Netlist,
    constraints: &constraints::TimingConstraints,
) -> Result<StaResult, String> {
    analyze_impl(nl, Some(constraints))
}

/// Incrementally update timing after input-delay changes on one flat netlist.
///
/// The topology, library, case analysis, exceptions, and derates must remain
/// unchanged. Only arrival propagation through the transitive fanout of the
/// listed input ports is recomputed; unrelated branches retain their previous
/// late/early arrivals and selected path inputs.
pub fn update_timing_for_input_delays<I, S>(
    nl: &Netlist,
    previous: &StaResult,
    constraints: &constraints::TimingConstraints,
    changed_ports: I,
) -> Result<IncrementalTimingResult, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut changed_port_names = BTreeSet::new();
    for port in changed_ports {
        if !changed_port_names.insert(port.as_ref().to_string()) {
            return Err("duplicate incremental STA input port".into());
        }
    }
    if changed_port_names.is_empty() {
        return Err("incremental STA requires at least one changed input port".into());
    }

    let mut changed_nets = BTreeSet::new();
    for name in &changed_port_names {
        let Some(port) = nl.ports.iter().find(|port| &port.name == name) else {
            return Err(format!("incremental STA input port '{}' not found", name));
        };
        if port.dir != eda_core::netlist::Dir::In {
            return Err(format!("incremental STA port '{}' is not an input", name));
        }
        changed_nets.extend(port.bits.iter().copied());
    }
    update_timing_incremental(
        nl,
        previous,
        constraints,
        changed_port_names,
        changed_nets,
        BTreeSet::new(),
    )
}

/// Incrementally update timing after cell/pin delay-derate or SDF arc changes.
///
/// The caller must list every changed cell or pin and keep all other timing
/// inputs unchanged. Cell and pin seeds recompute that cell's output arc and
/// its transitive combinational fanout; unrelated branches are retained.
pub fn update_timing_for_timing_derates<C, S>(
    nl: &Netlist,
    previous: &StaResult,
    constraints: &constraints::TimingConstraints,
    changed_cells: C,
    changed_pins: &[constraints::TimingPin],
) -> Result<IncrementalTimingResult, String>
where
    C: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut changed_cell_names = BTreeSet::new();
    for cell in changed_cells {
        if !changed_cell_names.insert(cell.as_ref().to_string()) {
            return Err("duplicate incremental STA cell".into());
        }
    }
    if changed_cell_names.is_empty() && changed_pins.is_empty() {
        return Err("incremental STA requires at least one changed cell or pin".into());
    }

    let mut seed_cells = BTreeSet::new();
    for name in &changed_cell_names {
        let Some(cell_id) = nl.cells().iter().position(|cell| &cell.name == name) else {
            return Err(format!("incremental STA cell '{}' not found", name));
        };
        seed_cells.insert(cell_id);
    }
    for pin in changed_pins {
        let Some(cell_id) = nl.cells().iter().position(|cell| cell.name == pin.cell) else {
            return Err(format!("incremental STA cell '{}' not found", pin.cell));
        };
        if !valid_timing_pin_name(nl, cell_id, &pin.pin) {
            return Err(format!(
                "incremental STA pin '{}/{}' not found",
                pin.cell, pin.pin
            ));
        }
        seed_cells.insert(cell_id);
    }

    let changed_nets = seed_cells
        .iter()
        .map(|&cell_id| nl.cell(cell_id).output)
        .collect::<BTreeSet<_>>();
    update_timing_incremental(
        nl,
        previous,
        constraints,
        BTreeSet::new(),
        changed_nets,
        seed_cells,
    )
}

/// Incrementally update base and constrained timing after SDC changes.
///
/// The netlist topology must be unchanged. Input-delay and cell/pin delay-derate
/// changes use local propagation. Case analysis, disable timing, and global
/// delay-derate changes conservatively rebuild the base graph. Constraint-only
/// changes reuse base arrivals and rebuild the compact constrained summary.
pub fn update_timing_for_constraints(
    nl: &Netlist,
    previous: &StaResult,
    old_constraints: &constraints::TimingConstraints,
    new_constraints: &constraints::TimingConstraints,
) -> Result<ConstraintTimingResult, String> {
    let diff = constraints::diff_timing_constraints(old_constraints, new_constraints);
    let (timing, recomputed_nets, recomputed_cells, full_base_recompute) =
        if diff.full_base_recompute_required {
            let constrained = constraints::analyze_with_sdc(nl, new_constraints)?;
            let all_nets = (0..nl.net_count())
                .map(|net| nl.net(net).name.clone())
                .collect::<BTreeSet<_>>();
            let all_cells = nl
                .cells()
                .iter()
                .map(|cell| cell.name.clone())
                .collect::<BTreeSet<_>>();
            (constrained.base.clone(), all_nets, all_cells, true)
        } else if diff.base_recompute_required {
            let mut current = previous.clone();
            let mut changed_ports = diff.changed_input_ports.clone();
            changed_ports.extend(diff.changed_port_derates.iter().cloned());
            let mut recomputed_nets = BTreeSet::new();
            let mut recomputed_cells = BTreeSet::new();

            if !changed_ports.is_empty() {
                let update = update_timing_for_input_delays(
                    nl,
                    &current,
                    new_constraints,
                    changed_ports.iter().cloned(),
                )?;
                current = update.timing;
                recomputed_nets.extend(update.recomputed_nets);
                recomputed_cells.extend(update.recomputed_cells);
            }

            if !diff.changed_cell_derates.is_empty() || !diff.changed_pin_derates.is_empty() {
                let changed_pins = diff.changed_pin_derates.iter().cloned().collect::<Vec<_>>();
                let update = update_timing_for_timing_derates(
                    nl,
                    &current,
                    new_constraints,
                    diff.changed_cell_derates.iter().cloned(),
                    &changed_pins,
                )?;
                current = update.timing;
                recomputed_nets.extend(update.recomputed_nets);
                recomputed_cells.extend(update.recomputed_cells);
            }
            (current, recomputed_nets, recomputed_cells, false)
        } else {
            (previous.clone(), BTreeSet::new(), BTreeSet::new(), false)
        };

    let constrained = constraints::analyze_constrained(nl, timing.clone(), new_constraints)?;
    Ok(ConstraintTimingResult {
        timing,
        constrained,
        diff,
        recomputed_nets,
        recomputed_cells,
        total_cells: nl.cell_count(),
        full_base_recompute,
    })
}

fn timing_cell_signature(nl: &Netlist, cell: &eda_core::netlist::Cell) -> String {
    let library_cell = match &cell.kind {
        CellKind::Lib(id) => nl
            .library()
            .map(|library| format!("{}:{}", library.name, library.get(*id).name)),
        _ => None,
    };
    let inputs = cell
        .inputs
        .iter()
        .map(|&net| nl.net(net).name.clone())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{}|{}|{}",
        library_cell.unwrap_or_else(|| format!("{:?}", cell.kind)),
        inputs,
        nl.net(cell.output).name
    )
}

fn timing_net_drivers(nl: &Netlist, net: NetId) -> BTreeSet<String> {
    nl.net(net)
        .drivers
        .iter()
        .map(|&cell| nl.cell(cell).name.clone())
        .collect()
}

/// Compare two flat netlists by stable cell/net/port names.
///
/// Numeric IDs are intentionally ignored. Library cells are compared by
/// library/definition name, so an SDF-annotated replacement is detected even
/// when its numeric `LibCellId` changes.
pub fn diff_timing_topology(old: &Netlist, new: &Netlist) -> TimingTopologyDiff {
    let mut diff = TimingTopologyDiff::default();
    let old_cells = old
        .cells()
        .iter()
        .map(|cell| (cell.name.clone(), cell))
        .collect::<BTreeMap<_, _>>();
    let new_cells = new
        .cells()
        .iter()
        .map(|cell| (cell.name.clone(), cell))
        .collect::<BTreeMap<_, _>>();
    for name in old_cells.keys() {
        if !new_cells.contains_key(name) {
            diff.removed_cells.insert(name.clone());
        }
    }
    for (name, cell) in &new_cells {
        if !old_cells.contains_key(name) {
            diff.added_cells.insert(name.clone());
            continue;
        }
        let old_cell = old_cells[name];
        if timing_cell_signature(old, old_cell) != timing_cell_signature(new, cell) {
            diff.changed_cells.insert(name.clone());
        }
    }

    let old_nets = (0..old.net_count())
        .map(|net| (old.net(net).name.clone(), net))
        .collect::<BTreeMap<_, _>>();
    let new_nets = (0..new.net_count())
        .map(|net| (new.net(net).name.clone(), net))
        .collect::<BTreeMap<_, _>>();
    for name in old_nets.keys() {
        if !new_nets.contains_key(name) {
            diff.removed_nets.insert(name.clone());
        }
    }
    for (name, &net) in &new_nets {
        if !old_nets.contains_key(name) {
            diff.added_nets.insert(name.clone());
            continue;
        }
        if timing_net_drivers(old, old_nets[name]) != timing_net_drivers(new, net) {
            diff.changed_nets.insert(name.clone());
        }
    }

    let old_ports = old
        .ports
        .iter()
        .map(|port| (port.name.clone(), port))
        .collect::<BTreeMap<_, _>>();
    let new_ports = new
        .ports
        .iter()
        .map(|port| (port.name.clone(), port))
        .collect::<BTreeMap<_, _>>();
    for name in old_ports.keys().chain(new_ports.keys()) {
        let (Some(old_port), Some(new_port)) = (old_ports.get(name), new_ports.get(name)) else {
            diff.changed_ports.insert(name.clone());
            continue;
        };
        let old_bits = old_port
            .bits
            .iter()
            .map(|&net| old.net(net).name.clone())
            .collect::<BTreeSet<_>>();
        let new_bits = new_port
            .bits
            .iter()
            .map(|&net| new.net(net).name.clone())
            .collect::<BTreeSet<_>>();
        if old_port.dir != new_port.dir || old_bits != new_bits {
            diff.changed_ports.insert(name.clone());
            if new_port.dir == eda_core::netlist::Dir::In {
                diff.changed_nets.extend(new_bits);
            }
        }
    }
    diff
}

fn count_disabled_timing(new: &Netlist, constraints: &constraints::TimingConstraints) -> usize {
    let mut count = 0usize;
    for (cell_id, cell) in new.cells().iter().enumerate() {
        if nl_is_sequential(new, cell_id) {
            if constraints::timing_arc_disabled(new, constraints, cell_id, Some(0), None) {
                count += 1;
            }
            if constraints::timing_check_disabled(new, constraints, cell_id, 1) {
                count += 1;
            }
            if let Some((reset_pin, _, _)) = seq_reset(new, &cell.kind) {
                if constraints::timing_check_disabled(new, constraints, cell_id, reset_pin) {
                    count += 1;
                }
            }
        } else {
            for pin in 0..cell.inputs.len() {
                if constraints::timing_arc_disabled(new, constraints, cell_id, Some(pin), None) {
                    count += 1;
                }
            }
        }
    }
    count
}

fn nl_is_sequential(nl: &Netlist, cell_id: usize) -> bool {
    nl.cell_is_sequential(&nl.cell(cell_id).kind)
}

fn remap_previous_timing(
    old: &Netlist,
    previous: &StaResult,
    new: &Netlist,
    diff: &TimingTopologyDiff,
    constraints: &constraints::TimingConstraints,
) -> Result<StaResult, String> {
    if old.net_count() != previous.arrivals.len()
        || old.net_count() != previous.min_arrivals.len()
        || old.net_count() != previous.transitions.len()
        || old.net_count() != previous.late_path_inputs.len()
        || old.net_count() != previous.early_path_inputs.len()
    {
        return Err("previous STA result does not match the old netlist".into());
    }

    let old_nets = (0..old.net_count())
        .map(|net| (old.net(net).name.clone(), net))
        .collect::<BTreeMap<_, _>>();
    let new_nets = (0..new.net_count())
        .map(|net| (new.net(net).name.clone(), net))
        .collect::<BTreeMap<_, _>>();
    let changed_or_added = diff
        .changed_nets
        .iter()
        .chain(&diff.added_nets)
        .cloned()
        .collect::<BTreeSet<_>>();

    let mut arrivals = vec![f64::NEG_INFINITY; new.net_count()];
    let mut min_arrivals = vec![f64::INFINITY; new.net_count()];
    let mut transitions = vec![f64::NAN; new.net_count()];
    let mut min_transitions = vec![f64::NAN; new.net_count()];
    let mut late_path_inputs = vec![None; new.net_count()];
    let mut early_path_inputs = vec![None; new.net_count()];
    let mut case_analysis_nets = BTreeMap::new();

    for (name, &new_net) in &new_nets {
        if changed_or_added.contains(name) {
            continue;
        }
        let Some(&old_net) = old_nets.get(name) else {
            continue;
        };
        arrivals[new_net] = previous.arrivals[old_net];
        min_arrivals[new_net] = previous.min_arrivals[old_net];
        transitions[new_net] = previous.transitions[old_net];
        min_transitions[new_net] = previous.min_transitions[old_net];
        if let Some(old_input) = previous.late_path_inputs[old_net] {
            let input_name = old.net(old_input).name.clone();
            if let Some(&mapped_input) = new_nets.get(&input_name) {
                late_path_inputs[new_net] = Some(mapped_input);
            }
        }
        if let Some(old_input) = previous.early_path_inputs[old_net] {
            let input_name = old.net(old_input).name.clone();
            if let Some(&mapped_input) = new_nets.get(&input_name) {
                early_path_inputs[new_net] = Some(mapped_input);
            }
        }
        if let Some(value) = previous.case_analysis_nets.get(name) {
            case_analysis_nets.insert(name.clone(), *value);
        }
    }

    Ok(StaResult {
        critical_path_delay: 0.0,
        reg2reg_critical: 0.0,
        in2reg_critical: 0.0,
        reg2reg_hold_critical: 0.0,
        in2reg_hold_critical: 0.0,
        disabled_timing_arcs: count_disabled_timing(new, constraints),
        case_analysis_nets,
        arrivals,
        min_arrivals,
        transitions,
        min_transitions,
        late_path_inputs,
        early_path_inputs,
        critical_cells: Vec::new(),
        setup_endpoints: Vec::new(),
        hold_endpoints: Vec::new(),
        reset_endpoints: Vec::new(),
    })
}

/// Incrementally update timing after a flat-netlist ECO.
///
/// Stable names identify unchanged nets and cells. Unchanged branches are
/// remapped into new numeric IDs and retained; added/changed cells and their
/// transitive combinational fanout are recomputed.
pub fn update_timing_for_topology(
    old: &Netlist,
    previous: &StaResult,
    new: &Netlist,
    constraints: &constraints::TimingConstraints,
) -> Result<TopologyTimingResult, String> {
    let diff = diff_timing_topology(old, new);
    let remapped = remap_previous_timing(old, previous, new, &diff, constraints)?;

    let new_nets = (0..new.net_count())
        .map(|net| (new.net(net).name.clone(), net))
        .collect::<BTreeMap<_, _>>();
    let mut changed_nets = BTreeSet::new();
    for name in diff.changed_nets.iter().chain(&diff.added_nets) {
        if let Some(&net) = new_nets.get(name) {
            changed_nets.insert(net);
        }
    }

    let mut seed_cells = BTreeSet::new();
    for name in diff.added_cells.iter().chain(&diff.changed_cells) {
        let Some(cell_id) = new.cells().iter().position(|cell| &cell.name == name) else {
            continue;
        };
        seed_cells.insert(cell_id);
        let output = new.cell(cell_id).output;
        changed_nets.insert(output);
    }

    let mut changed_ports = BTreeSet::new();
    for port in &diff.changed_ports {
        if new
            .ports
            .iter()
            .any(|candidate| candidate.name == *port && candidate.dir == eda_core::netlist::Dir::In)
        {
            changed_ports.insert(port.clone());
        }
    }

    let incremental = update_timing_incremental(
        new,
        &remapped,
        constraints,
        changed_ports,
        changed_nets,
        seed_cells,
    )?;
    Ok(TopologyTimingResult {
        timing: incremental.timing,
        diff,
        recomputed_nets: incremental.recomputed_nets,
        recomputed_cells: incremental.recomputed_cells,
        total_cells: incremental.total_cells,
    })
}

/// Shared same-topology incremental propagation engine.
///
/// Callers provide already validated changed nets and seed cells. The engine
/// recomputes their combinational fanout, rebuilds compact sequential endpoint
/// summaries, and retains unrelated late/early arrivals from `previous`.
#[allow(clippy::too_many_arguments)]
fn update_timing_incremental(
    nl: &Netlist,
    previous: &StaResult,
    constraints: &constraints::TimingConstraints,
    changed_port_names: BTreeSet<String>,
    changed_nets: BTreeSet<NetId>,
    seed_cells: BTreeSet<usize>,
) -> Result<IncrementalTimingResult, String> {
    if nl.net_count() != previous.arrivals.len()
        || nl.net_count() != previous.min_arrivals.len()
        || nl.net_count() != previous.transitions.len()
        || nl.net_count() != previous.min_transitions.len()
        || nl.net_count() != previous.late_path_inputs.len()
        || nl.net_count() != previous.early_path_inputs.len()
    {
        return Err("incremental STA requires an unchanged netlist topology".into());
    }

    let (constant_nets, case_analysis_nets) = resolve_case_analyses(nl, Some(constraints))?;
    let mut constant_nets = constant_nets;
    let mut case_analysis_nets = case_analysis_nets;
    for (name, value) in &previous.case_analysis_nets {
        let Some(net) = nl.net_by_name(name) else {
            continue;
        };
        if changed_nets.contains(&net) {
            continue;
        }
        if constant_nets[net].is_none() {
            constant_nets[net] = Some(*value);
        }
        case_analysis_nets.entry(name.clone()).or_insert(*value);
    }
    let mut arrival = previous.arrivals.clone();
    let mut min_arrival = previous.min_arrivals.clone();
    let mut transition = previous.transitions.clone();
    let mut min_transition = previous.min_transitions.clone();
    let mut late_path_inputs = previous.late_path_inputs.clone();
    let mut early_path_inputs = previous.early_path_inputs.clone();
    let disabled_timing_arcs = previous.disabled_timing_arcs;

    // Recompute the explicitly changed launch points. Their downstream cone is
    // collected below, while all unrelated old arrivals remain in place.
    for name in &changed_port_names {
        let Some(port) = nl.ports.iter().find(|port| &port.name == name) else {
            unreachable!("input port validated above");
        };
        let input_delay = constraints.input_delays.get(name).map(|delay| delay.delay);
        let derate = timing_derate_for_port(Some(constraints), name);
        for &net in &port.bits {
            if constant_nets[net].is_some() {
                arrival[net] = f64::NEG_INFINITY;
                min_arrival[net] = f64::INFINITY;
                transition[net] = 0.0;
                min_transition[net] = 0.0;
            } else {
                arrival[net] = input_delay.unwrap_or(0.0) * derate.late;
                min_arrival[net] = input_delay.unwrap_or(0.0) * derate.early;
                transition[net] = 0.0;
                min_transition[net] = 0.0;
                let (minimum, maximum) = clock_transition_bounds(Some(constraints), name);
                transition[net] = maximum;
                min_transition[net] = minimum;
            }
        }
    }

    // A changed sequential-cell timing arc relaunches its q output. Data and
    // reset input changes stop at the sequential boundary instead.
    for &cell_id in &seed_cells {
        let cell = nl.cell(cell_id);
        if let Some(c2q) = seq_clk_to_q(nl, &cell.kind) {
            if constant_nets[cell.output].is_some()
                || constraints::timing_arc_disabled(nl, constraints, cell_id, Some(0), None)
            {
                arrival[cell.output] = f64::NEG_INFINITY;
                min_arrival[cell.output] = f64::INFINITY;
                transition[cell.output] = 0.0;
                min_transition[cell.output] = 0.0;
                continue;
            }
            let derate = timing_derate_for_output_pin(nl, Some(constraints), &cell.name, cell_id);
            arrival[cell.output] = c2q * derate.late;
            min_arrival[cell.output] = c2q * derate.early;
            transition[cell.output] = c2q;
            min_transition[cell.output] = c2q;
        }
    }

    // Find the combinational fanout of the changed seeds. Sequential cells are
    // timing boundaries: their inputs are checked, but their q outputs do not
    // automatically become changing data-launch points.
    let mut affected_cells = BTreeSet::new();
    let mut affected_nets = changed_nets.clone();
    let mut stack = changed_nets.iter().copied().collect::<Vec<_>>();
    while let Some(net) = stack.pop() {
        for &cell_id in &nl.net(net).sinks {
            let cell = nl.cell(cell_id);
            if nl.cell_is_sequential(&cell.kind) {
                continue;
            }
            if !affected_cells.insert(cell_id) {
                continue;
            }
            if affected_nets.insert(cell.output) {
                stack.push(cell.output);
            }
        }
    }
    affected_cells.extend(seed_cells.iter().copied());

    let order = nl
        .topo_cells()
        .ok_or_else(|| "netlist has a combinational loop".to_string())?;
    for ci in order {
        if !affected_cells.contains(&ci) {
            continue;
        }
        let cell = nl.cell(ci);
        if nl.cell_is_sequential(&cell.kind) {
            continue;
        }
        let out = cell.output;
        if constant_nets[out].is_some() {
            arrival[out] = f64::NEG_INFINITY;
            min_arrival[out] = f64::INFINITY;
            transition[out] = 0.0;
            min_transition[out] = 0.0;
            late_path_inputs[out] = None;
            early_path_inputs[out] = None;
            continue;
        }

        let mut latest = f64::NEG_INFINITY;
        let mut latest_input = None;
        let mut latest_transition = 0.0_f64;
        for (pin, &input) in cell.inputs.iter().enumerate() {
            if constraints::timing_arc_disabled(nl, constraints, ci, Some(pin), None) {
                continue;
            }
            let input_time = arrival[input];
            if input_time.is_finite() {
                let derate =
                    timing_derate_for_input_pin(nl, Some(constraints), &cell.name, ci, pin);
                let arc_delay =
                    input_arc_delay_with_transition(nl, ci, pin, derate.late, transition[input]);
                let candidate = input_time + arc_delay;
                if candidate > latest {
                    latest = candidate;
                    latest_input = Some(input);
                    latest_transition = output_arc_transition(
                        nl,
                        ci,
                        pin,
                        derate.late,
                        transition[input],
                        arc_delay,
                    )
                    .max(0.0);
                }
            }
        }
        if latest.is_finite() {
            arrival[out] = latest;
            late_path_inputs[out] = latest_input;
            transition[out] = latest_transition;
        } else {
            arrival[out] = f64::NEG_INFINITY;
            late_path_inputs[out] = None;
            transition[out] = f64::NAN;
        }

        let mut earliest = f64::INFINITY;
        let mut earliest_input = None;
        let mut earliest_transition = f64::INFINITY;
        for (pin, &input) in cell.inputs.iter().enumerate() {
            if constraints::timing_arc_disabled(nl, constraints, ci, Some(pin), None) {
                continue;
            }
            let input_time = min_arrival[input];
            if input_time.is_finite() {
                let derate =
                    timing_derate_for_input_pin(nl, Some(constraints), &cell.name, ci, pin);
                let arc_delay = input_arc_delay_with_transition(
                    nl,
                    ci,
                    pin,
                    derate.early,
                    min_transition[input],
                );
                let candidate = input_time + arc_delay;
                if candidate < earliest {
                    earliest = candidate;
                    earliest_input = Some(input);
                    earliest_transition = output_arc_transition(
                        nl,
                        ci,
                        pin,
                        derate.early,
                        min_transition[input],
                        arc_delay,
                    )
                    .max(0.0);
                }
            }
        }
        if earliest.is_finite() {
            min_arrival[out] = earliest;
            early_path_inputs[out] = earliest_input;
            min_transition[out] = if earliest_transition.is_finite() {
                earliest_transition
            } else {
                f64::NAN
            };
        } else {
            min_arrival[out] = f64::INFINITY;
            early_path_inputs[out] = None;
            min_transition[out] = f64::NAN;
        }
    }

    // Sequential checks are compact endpoint summaries and are rebuilt without
    // propagating through the flops.
    let mut reg2reg = 0.0f64;
    let mut in2reg = 0.0f64;
    let mut reg2reg_hold = f64::INFINITY;
    let mut in2reg_hold = f64::INFINITY;
    let mut setup_endpoints = Vec::new();
    let mut hold_endpoints = Vec::new();
    let mut reset_endpoints = Vec::new();
    let mut best_end = (f64::NEG_INFINITY, None::<NetId>);

    for (ci, cell) in nl.cells().iter().enumerate() {
        if seq_setup(nl, &cell.kind).is_none() {
            continue;
        }
        if cell
            .inputs
            .first()
            .and_then(|&clock_net| constant_nets[clock_net])
            .is_some()
        {
            continue;
        }
        let data_pin = 1;
        if constraints::timing_check_disabled(nl, constraints, ci, data_pin) {
            continue;
        }
        let Some(&dnet) = cell.inputs.get(data_pin) else {
            continue;
        };
        let clock_net = cell.inputs.first().copied();
        let setup = seq_setup_at(
            nl,
            ci,
            transition.get(dnet).copied(),
            clock_net.and_then(|net| transition.get(net).copied()),
        )?;
        let a = arrival[dnet];
        let min_a = min_arrival[dnet];
        if a.is_finite() {
            let check_derate = constraints::pin_check_derate(nl, constraints, ci, 1);
            let path = a + setup * check_derate.late;
            if driven_from_sequential(nl, dnet) {
                reg2reg = reg2reg.max(path);
            } else {
                in2reg = in2reg.max(path);
            }
            setup_endpoints.push((ci, a));
            if a > best_end.0 {
                best_end = (a, Some(dnet));
            }
        }
        if min_a.is_finite() {
            let from_sequential = driven_from_sequential(nl, dnet);
            if from_sequential {
                reg2reg_hold = reg2reg_hold.min(min_a);
            } else {
                in2reg_hold = in2reg_hold.min(min_a);
            }
            hold_endpoints.push(HoldEndpoint {
                cell: ci,
                data_arrival: min_a,
                launched_from_sequential: from_sequential,
            });
        }

        if let Some((reset_pin, _, _)) = seq_reset(nl, &cell.kind) {
            if let Some(&reset_net) = cell.inputs.get(reset_pin) {
                if constraints::timing_check_disabled(nl, constraints, ci, reset_pin) {
                    continue;
                }
                let reset_arrival = arrival[reset_net];
                let reset_min_arrival = min_arrival[reset_net];
                if reset_arrival.is_finite() && reset_min_arrival.is_finite() {
                    reset_endpoints.push(ResetEndpoint {
                        cell: ci,
                        reset_arrival,
                        reset_min_arrival,
                    });
                }
            }
        }
    }

    for (net, &time) in arrival.iter().enumerate() {
        if time.is_finite() && time > best_end.0 {
            best_end = (time, Some(net));
        }
    }
    let critical_path_delay = arrival
        .iter()
        .copied()
        .filter(|time| time.is_finite())
        .fold(0.0f64, f64::max);
    let critical_cells = match best_end.1 {
        Some(net) => backtrack(nl, &arrival, net),
        None => Vec::new(),
    };

    let timing = StaResult {
        critical_path_delay: critical_path_delay.max(0.0),
        reg2reg_critical: reg2reg.max(0.0),
        in2reg_critical: in2reg.max(0.0),
        reg2reg_hold_critical: if reg2reg_hold.is_finite() {
            reg2reg_hold
        } else {
            0.0
        },
        in2reg_hold_critical: if in2reg_hold.is_finite() {
            in2reg_hold
        } else {
            0.0
        },
        disabled_timing_arcs,
        case_analysis_nets,
        arrivals: arrival,
        min_arrivals: min_arrival,
        transitions: transition,
        min_transitions: min_transition,
        late_path_inputs,
        early_path_inputs,
        critical_cells,
        setup_endpoints,
        hold_endpoints,
        reset_endpoints,
    };

    let recomputed_nets = affected_nets
        .iter()
        .map(|&net| nl.net(net).name.clone())
        .collect::<BTreeSet<_>>();
    let recomputed_cells = affected_cells
        .iter()
        .map(|&cell| nl.cell(cell).name.clone())
        .collect::<BTreeSet<_>>();
    Ok(IncrementalTimingResult {
        timing,
        changed_ports: changed_port_names,
        recomputed_nets,
        recomputed_cells,
        total_cells: nl.cell_count(),
    })
}

fn case_analysis_matches_net(
    nl: &Netlist,
    object: &constraints::TimingObjectRef,
    net: NetId,
) -> bool {
    match object.kind {
        constraints::TimingObjectKind::Port => nl
            .ports
            .iter()
            .any(|port| object.matches_name(&port.name) && port.bits.contains(&net)),
        constraints::TimingObjectKind::Net => object.matches_name(&nl.net(net).name),
        constraints::TimingObjectKind::Pin => {
            nl.net(net).drivers.iter().any(|&cell_id| {
                output_pin_name(nl, cell_id).is_some_and(|pin| {
                    object.matches_name(&format!("{}/{}", nl.cell(cell_id).name, pin))
                })
            }) || nl.net(net).sinks.iter().any(|&cell_id| {
                nl.cell(cell_id)
                    .inputs
                    .iter()
                    .enumerate()
                    .any(|(pin, &input)| {
                        input == net
                            && input_pin_name(nl, cell_id, pin).is_some_and(|pin_name| {
                                object.matches_name(&format!(
                                    "{}/{}",
                                    nl.cell(cell_id).name,
                                    pin_name
                                ))
                            })
                    })
            })
        }
        _ => false,
    }
}

fn net_hierarchy_matches(nl: &Netlist, net: NetId, pattern: &str) -> bool {
    nl.owning_hierarchy_instance(net).is_some_and(|instance| {
        constraints::TimingObjectPattern::new(pattern).matches(&instance.path)
    })
}

type CaseAnalysisResolution = (Vec<Option<bool>>, BTreeMap<String, bool>);

fn resolve_case_analyses(
    nl: &Netlist,
    constraints: Option<&constraints::TimingConstraints>,
) -> Result<CaseAnalysisResolution, String> {
    let mut constants = vec![None; nl.net_count()];
    let mut named = BTreeMap::new();
    let Some(constraints) = constraints else {
        return Ok((constants, named));
    };
    for analysis in &constraints.case_analyses {
        for (net, constant) in constants.iter_mut().enumerate() {
            if !case_analysis_matches_net(nl, &analysis.object, net) {
                continue;
            }
            if let Some(pattern) = &analysis.instance_pattern {
                if !net_hierarchy_matches(nl, net, pattern) {
                    continue;
                }
            }
            if let Some(old) = *constant {
                if old != analysis.value {
                    return Err(format!(
                        "conflicting set_case_analysis values for net '{}'",
                        nl.net(net).name
                    ));
                }
            }
            *constant = Some(analysis.value);
            named.insert(nl.net(net).name.clone(), analysis.value);
        }
    }
    Ok((constants, named))
}

fn evaluated_constant(nl: &Netlist, cell_id: usize, constants: &[Option<bool>]) -> Option<bool> {
    let cell = nl.cell(cell_id);
    let inputs = cell
        .inputs
        .iter()
        .map(|&net| constants[net].map(LogicValue::bit).unwrap_or(LogicValue::X))
        .collect::<Vec<_>>();
    let value = match &cell.kind {
        CellKind::Lib(id) => {
            let library = nl.library()?;
            let definition = library.get(*id);
            if definition.seq.is_some() {
                return None;
            }
            definition.func.eval(&inputs)
        }
        other => other.eval(&inputs),
    };
    value.as_bool()
}

/// Return a timing evaluation order with every combinational stage before its
/// sequential sinks.
///
/// `Netlist::topo_cells` treats sequential cells as cut points and therefore
/// does not require a flop to follow combinational logic on its data, clock, or
/// reset pins. STA consumes those arrival values when it visits the flop, so it
/// deliberately partitions the generic order into combinational and sequential
/// phases while preserving each phase's relative deterministic order.
fn analysis_order(nl: &Netlist) -> Option<Vec<usize>> {
    let order = nl.topo_cells()?;
    let mut timing_order = order
        .iter()
        .copied()
        .filter(|&cell| !nl.cell_is_sequential(&nl.cell(cell).kind))
        .collect::<Vec<_>>();
    timing_order.extend(
        order
            .into_iter()
            .filter(|cell| nl.cell_is_sequential(&nl.cell(*cell).kind)),
    );
    Some(timing_order)
}

fn analyze_impl(
    nl: &Netlist,
    constraints: Option<&constraints::TimingConstraints>,
) -> Result<StaResult, String> {
    let order = analysis_order(nl).ok_or_else(|| "netlist has a combinational loop".to_string())?;
    let n_nets = nl.net_count();
    if let Some(constraints) = constraints {
        let cell_names: std::collections::BTreeSet<_> =
            nl.cells().iter().map(|cell| cell.name.as_str()).collect();
        for cell in constraints.cell_timing_derates.keys() {
            if !cell_names.contains(cell.as_str()) {
                return Err(format!("timing derate cell '{}' not found", cell));
            }
        }
        for cell in constraints.cell_check_derates.keys() {
            if !cell_names.contains(cell.as_str()) {
                return Err(format!("check derate cell '{}' not found", cell));
            }
        }
        let port_names: std::collections::BTreeSet<_> =
            nl.ports.iter().map(|port| port.name.as_str()).collect();
        for port in constraints.port_timing_derates.keys() {
            if !port_names.contains(port.as_str()) {
                return Err(format!("timing derate port '{}' not found", port));
            }
        }
        for pin in constraints.pin_timing_derates.keys() {
            let Some((cell_id, _cell)) = nl
                .cells()
                .iter()
                .enumerate()
                .find(|(_, cell)| cell.name == pin.cell)
            else {
                return Err(format!("timing derate cell '{}' not found", pin.cell));
            };
            if !valid_timing_pin_name(nl, cell_id, &pin.pin) {
                return Err(format!(
                    "timing derate pin '{}/{}' not found",
                    pin.cell, pin.pin
                ));
            }
        }
        for pin in constraints.pin_check_derates.keys() {
            let Some((cell_id, _cell)) = nl
                .cells()
                .iter()
                .enumerate()
                .find(|(_, cell)| cell.name == pin.cell)
            else {
                return Err(format!("check derate cell '{}' not found", pin.cell));
            };
            if !valid_input_pin_name(nl, cell_id, &pin.pin) {
                return Err(format!(
                    "check derate pin '{}/{}' not found",
                    pin.cell, pin.pin
                ));
            }
        }
    }
    let (mut constant_nets, mut case_analysis_nets) = resolve_case_analyses(nl, constraints)?;
    let mut arrival = vec![f64::NEG_INFINITY; n_nets];
    let mut min_arrival = vec![f64::INFINITY; n_nets];
    let mut transition = vec![f64::NAN; n_nets];
    let mut min_transition = vec![f64::NAN; n_nets];
    let mut late_path_inputs = vec![None; n_nets];
    let mut early_path_inputs = vec![None; n_nets];
    let mut disabled_timing_arcs = 0usize;
    // Primary inputs launch at 0, or at their SDC input delay.
    for p in &nl.ports {
        if p.dir == eda_core::netlist::Dir::In {
            let input_delay = constraints
                .and_then(|c| c.input_delays.get(&p.name))
                .map(|d| d.delay);
            if let Some(delay) = input_delay {
                if !delay.is_finite() {
                    return Err(format!("input delay for '{}' is not finite", p.name));
                }
            }
            for &b in &p.bits {
                let derate = timing_derate_for_port(constraints, &p.name);
                arrival[b] = input_delay.unwrap_or(0.0) * derate.late;
                min_arrival[b] = input_delay.unwrap_or(0.0) * derate.early;
                transition[b] = 0.0;
                min_transition[b] = 0.0;
                if let Some(constraints) = constraints {
                    let (minimum, maximum) = clock_transition_bounds(Some(constraints), &p.name);
                    transition[b] = maximum;
                    min_transition[b] = minimum;
                }
            }
        }
    }
    for (net, value) in constant_nets.iter().enumerate() {
        if value.is_some() {
            arrival[net] = f64::NEG_INFINITY;
            min_arrival[net] = f64::INFINITY;
            transition[net] = 0.0;
            min_transition[net] = 0.0;
        }
    }

    // Sequential outputs launch at clk_to_q (topological order is safe:
    // flops break combinational cycles).
    for (cell_id, cell) in nl.cells().iter().enumerate() {
        if let Some(c2q) = seq_clk_to_q(nl, &cell.kind) {
            if cell
                .inputs
                .first()
                .and_then(|&clock_net| constant_nets[clock_net])
                .is_some()
            {
                continue;
            }
            if constant_nets[cell.output].is_some() {
                continue;
            }
            if constraints.is_some_and(|constraints| {
                constraints::timing_arc_disabled(nl, constraints, cell_id, Some(0), None)
            }) {
                disabled_timing_arcs += 1;
                continue;
            }
            let derate = timing_derate_for_output_pin(nl, constraints, &cell.name, cell_id);
            arrival[cell.output] = c2q * derate.late;
            min_arrival[cell.output] = c2q * derate.early;
            transition[cell.output] = c2q;
            min_transition[cell.output] = c2q;
        }
    }

    let mut reg2reg = 0.0f64;
    let mut in2reg = 0.0f64;
    let mut reg2reg_hold = f64::INFINITY;
    let mut in2reg_hold = f64::INFINITY;
    let mut setup_endpoints: Vec<(usize, f64)> = Vec::new();
    let mut hold_endpoints: Vec<HoldEndpoint> = Vec::new();
    let mut reset_endpoints: Vec<ResetEndpoint> = Vec::new();
    let mut best_end = (f64::NEG_INFINITY, None::<NetId>);

    for &ci in &order {
        let cell = nl.cell(ci);
        if seq_setup(nl, &cell.kind).is_some() {
            if cell
                .inputs
                .first()
                .and_then(|&clock_net| constant_nets[clock_net])
                .is_some()
            {
                continue;
            }
            // Setup check on the data pin (pin 1 of DFF-like cells).
            let data_pin = 1;
            let data_check_disabled = constraints.is_some_and(|constraints| {
                constraints::timing_check_disabled(nl, constraints, ci, data_pin)
            });
            if data_check_disabled {
                disabled_timing_arcs += 1;
            }
            if !data_check_disabled && data_pin < cell.inputs.len() {
                let dnet = cell.inputs[data_pin];
                let a = arrival[dnet];
                let min_a = min_arrival[dnet];
                let clock_net = cell.inputs.first().copied();
                let setup = seq_setup_at(
                    nl,
                    ci,
                    transition.get(dnet).copied(),
                    clock_net.and_then(|net| transition.get(net).copied()),
                )?;
                if a.is_finite() {
                    let path = a + setup;
                    if driven_from_sequential(nl, dnet) {
                        reg2reg = reg2reg.max(path);
                    } else {
                        in2reg = in2reg.max(path);
                    }
                    setup_endpoints.push((ci, a));
                    if a > best_end.0 {
                        best_end = (a, Some(dnet));
                    }
                }
                if min_a.is_finite() {
                    let from_sequential = driven_from_sequential(nl, dnet);
                    if from_sequential {
                        reg2reg_hold = reg2reg_hold.min(min_a);
                    } else {
                        in2reg_hold = in2reg_hold.min(min_a);
                    }
                    hold_endpoints.push(HoldEndpoint {
                        cell: ci,
                        data_arrival: min_a,
                        launched_from_sequential: from_sequential,
                    });
                }
            }
            if let Some((reset_pin, _, _)) = seq_reset(nl, &cell.kind) {
                if let Some(&reset_net) = cell.inputs.get(reset_pin) {
                    if constraints.is_some_and(|constraints| {
                        constraints::timing_check_disabled(nl, constraints, ci, reset_pin)
                    }) {
                        disabled_timing_arcs += 1;
                        continue;
                    }
                    let reset_arrival = arrival[reset_net];
                    let reset_min_arrival = min_arrival[reset_net];
                    if reset_arrival.is_finite() && reset_min_arrival.is_finite() {
                        reset_endpoints.push(ResetEndpoint {
                            cell: ci,
                            reset_arrival,
                            reset_min_arrival,
                        });
                    }
                }
            }
            continue;
        }
        // Combinational cell: pair each input arrival with its own arc so
        // pin-specific derates cannot leak onto unrelated input arcs.
        let out = cell.output;
        if constant_nets[out].is_some() {
            continue;
        }
        if let Some(value) = evaluated_constant(nl, ci, &constant_nets) {
            constant_nets[out] = Some(value);
            case_analysis_nets.insert(nl.net(out).name.clone(), value);
            arrival[out] = f64::NEG_INFINITY;
            min_arrival[out] = f64::INFINITY;
            transition[out] = 0.0;
            min_transition[out] = 0.0;
            late_path_inputs[out] = None;
            early_path_inputs[out] = None;
            continue;
        }
        let mut t = f64::NEG_INFINITY;
        let mut late_input = None;
        let mut late_transition = 0.0_f64;
        for (pin, &input) in cell.inputs.iter().enumerate() {
            if constraints.is_some_and(|constraints| {
                constraints::timing_arc_disabled(nl, constraints, ci, Some(pin), None)
            }) {
                disabled_timing_arcs += 1;
                continue;
            }
            let input_time = arrival[input];
            if input_time.is_finite() {
                let derate = timing_derate_for_input_pin(nl, constraints, &cell.name, ci, pin);
                let arc_delay =
                    input_arc_delay_with_transition(nl, ci, pin, derate.late, transition[input]);
                let candidate = input_time + arc_delay;
                if candidate > t {
                    t = candidate;
                    late_input = Some(input);
                    late_transition = output_arc_transition(
                        nl,
                        ci,
                        pin,
                        derate.late,
                        transition[input],
                        arc_delay,
                    )
                    .max(0.0);
                }
            }
        }
        if !t.is_finite() {
            continue; // unreachable from any source
        }
        if t > arrival[out] {
            arrival[out] = t;
            late_path_inputs[out] = late_input;
            transition[out] = late_transition;
        }
        let mut earliest = f64::INFINITY;
        let mut early_input = None;
        let mut early_transition = f64::INFINITY;
        for (pin, &input) in cell.inputs.iter().enumerate() {
            let input_time = min_arrival[input];
            if input_time.is_finite() {
                let derate = timing_derate_for_input_pin(nl, constraints, &cell.name, ci, pin);
                let arc_delay = input_arc_delay_with_transition(
                    nl,
                    ci,
                    pin,
                    derate.early,
                    min_transition[input],
                );
                let candidate = input_time + arc_delay;
                if candidate < earliest {
                    earliest = candidate;
                    early_input = Some(input);
                    early_transition = output_arc_transition(
                        nl,
                        ci,
                        pin,
                        derate.early,
                        min_transition[input],
                        arc_delay,
                    )
                    .max(0.0);
                }
            }
        }
        if earliest.is_finite() && earliest < min_arrival[out] {
            min_arrival[out] = earliest;
            early_path_inputs[out] = early_input;
            min_transition[out] = early_transition;
        }
        if t > best_end.0 {
            best_end = (t, Some(out));
        }
    }

    let critical_path_delay = arrival
        .iter()
        .copied()
        .filter(|t| t.is_finite())
        .fold(0.0f64, f64::max);

    let critical_cells = match best_end.1 {
        Some(net) => backtrack(nl, &arrival, net),
        None => Vec::new(),
    };

    Ok(StaResult {
        critical_path_delay: critical_path_delay.max(0.0),
        reg2reg_critical: reg2reg.max(0.0),
        in2reg_critical: in2reg.max(0.0),
        reg2reg_hold_critical: if reg2reg_hold.is_finite() {
            reg2reg_hold
        } else {
            0.0
        },
        in2reg_hold_critical: if in2reg_hold.is_finite() {
            in2reg_hold
        } else {
            0.0
        },
        disabled_timing_arcs,
        case_analysis_nets,
        arrivals: arrival,
        min_arrivals: min_arrival,
        transitions: transition,
        min_transitions: min_transition,
        late_path_inputs,
        early_path_inputs,
        critical_cells,
        setup_endpoints,
        hold_endpoints,
        reset_endpoints,
    })
}

fn timing_derate_for_cell(
    nl: &Netlist,
    constraints: Option<&constraints::TimingConstraints>,
    cell_id: usize,
    check: bool,
) -> constraints::TimingDerate {
    let cell = &nl.cell(cell_id).name;
    let global = constraints
        .map(|c| {
            if check {
                c.check_timing_derate
            } else {
                c.timing_derate
            }
        })
        .unwrap_or_default();
    let exact = constraints
        .and_then(|c| {
            if check {
                c.cell_check_derates.get(cell).copied()
            } else {
                c.cell_timing_derates.get(cell).copied()
            }
        })
        .unwrap_or(global);
    constraints
        .and_then(|c| constraints::scoped_cell_timing_derate(nl, c, cell_id, check))
        .unwrap_or(exact)
}

fn timing_derate_for_port(
    constraints: Option<&constraints::TimingConstraints>,
    port: &str,
) -> constraints::TimingDerate {
    let global = constraints.map(|c| c.timing_derate).unwrap_or_default();
    constraints
        .and_then(|c| c.port_timing_derates.get(port).copied())
        .unwrap_or(global)
}

fn input_pin_name(nl: &Netlist, cell_id: usize, pin: usize) -> Option<String> {
    let cell = nl.cell(cell_id);
    if pin >= cell.inputs.len() {
        return None;
    }
    match &cell.kind {
        CellKind::Lib(id) => nl
            .library()
            .and_then(|library| library.get(*id).inputs.get(pin).cloned()),
        _ => Some(pin.to_string()),
    }
}

fn valid_input_pin_name(nl: &Netlist, cell_id: usize, name: &str) -> bool {
    let cell = nl.cell(cell_id);
    (0..cell.inputs.len())
        .any(|pin| input_pin_name(nl, cell_id, pin).is_some_and(|pin_name| pin_name == name))
}

fn output_pin_name(nl: &Netlist, cell_id: usize) -> Option<String> {
    match &nl.cell(cell_id).kind {
        CellKind::Lib(id) => nl.library().map(|library| library.get(*id).output.clone()),
        CellKind::Dff { .. } | CellKind::DffR { .. } => Some("q".into()),
        _ => Some("y".into()),
    }
}

fn valid_timing_pin_name(nl: &Netlist, cell_id: usize, name: &str) -> bool {
    valid_input_pin_name(nl, cell_id, name)
        || output_pin_name(nl, cell_id).is_some_and(|output| output == name)
}

fn timing_derate_for_output_pin(
    nl: &Netlist,
    constraints: Option<&constraints::TimingConstraints>,
    cell: &str,
    cell_id: usize,
) -> constraints::TimingDerate {
    let Some(pin_name) = output_pin_name(nl, cell_id) else {
        return timing_derate_for_cell(nl, constraints, cell_id, false);
    };
    if let Some(derate) = constraints
        .and_then(|c| constraints::scoped_pin_timing_derate(nl, c, cell_id, &pin_name, false))
    {
        return derate;
    }
    let cell_derate = timing_derate_for_cell(nl, constraints, cell_id, false);
    constraints
        .and_then(|c| {
            c.pin_timing_derates
                .get(&constraints::TimingPin::new(cell, pin_name))
                .copied()
        })
        .unwrap_or(cell_derate)
}

fn timing_derate_for_input_pin(
    nl: &Netlist,
    constraints: Option<&constraints::TimingConstraints>,
    cell: &str,
    cell_id: usize,
    pin: usize,
) -> constraints::TimingDerate {
    let Some(pin_name) = input_pin_name(nl, cell_id, pin) else {
        return timing_derate_for_cell(nl, constraints, cell_id, false);
    };
    if let Some(derate) = constraints
        .and_then(|c| constraints::scoped_pin_timing_derate(nl, c, cell_id, &pin_name, false))
    {
        return derate;
    }
    let input_key = constraints::TimingPin::new(cell, pin_name);
    if let Some(derate) = constraints.and_then(|c| c.pin_timing_derates.get(&input_key).copied()) {
        return derate;
    }
    timing_derate_for_output_pin(nl, constraints, cell, cell_id)
}

/// True when a finite arrival path traced backwards from `net` reaches a
/// sequential launch edge. The direct-driver check used to miss logic between
/// two registers.
fn driven_from_sequential(nl: &Netlist, start: NetId) -> bool {
    let mut stack = vec![start];
    let mut seen = std::collections::HashSet::new();
    while let Some(net) = stack.pop() {
        if !seen.insert(net) {
            continue;
        }
        for &ci in &nl.net(net).drivers {
            let cell = nl.cell(ci);
            if nl.cell_is_sequential(&cell.kind) {
                return true;
            }
            for &input in &cell.inputs {
                stack.push(input);
            }
        }
    }
    false
}

fn seq_clk_to_q(nl: &Netlist, kind: &CellKind) -> Option<f64> {
    match kind {
        CellKind::Lib(id) => nl
            .library()
            .and_then(|l| l.get(*id).seq.as_ref().map(|s| s.clk_to_q)),
        _ => None,
    }
}

fn seq_setup(nl: &Netlist, kind: &CellKind) -> Option<f64> {
    match kind {
        CellKind::Lib(id) => nl
            .library()
            .and_then(|l| l.get(*id).seq.as_ref().map(|s| s.setup)),
        _ => None,
    }
}

/// Setup constraint at a concrete sequential cell and transition pair.
///
/// Indexed Liberty constraints use constrained-pin transition as index 1 and
/// related clock-pin transition as index 2. Missing transitions default to
/// zero, matching the scalar-delay launch model used by unconstrained STA.
fn seq_setup_at(
    nl: &Netlist,
    cell_id: usize,
    data_transition_ns: Option<f64>,
    clock_transition_ns: Option<f64>,
) -> Result<f64, String> {
    let cell = nl.cell(cell_id);
    let fallback = seq_setup(nl, &cell.kind).unwrap_or(0.0);
    let CellKind::Lib(id) = &cell.kind else {
        return Ok(fallback);
    };
    let Some(seq) = nl.library().map(|library| library.get(*id).seq.clone()) else {
        return Ok(fallback);
    };
    let Some(tables) = seq.as_ref().and_then(|seq| seq.setup_tables.as_ref()) else {
        return Ok(fallback);
    };
    let data_transition = data_transition_ns.unwrap_or(0.0);
    let clock_transition = clock_transition_ns.unwrap_or(0.0);
    Ok(tables
        .evaluate_worst(data_transition, clock_transition)?
        .unwrap_or(fallback)
        .max(0.0))
}

fn seq_hold(nl: &Netlist, kind: &CellKind) -> Option<f64> {
    match kind {
        CellKind::Lib(id) => nl
            .library()
            .and_then(|l| l.get(*id).seq.as_ref().map(|s| s.hold)),
        _ => None,
    }
}

/// Hold constraint at a concrete sequential cell and transition pair.
fn seq_hold_at(
    nl: &Netlist,
    cell_id: usize,
    data_transition_ns: Option<f64>,
    clock_transition_ns: Option<f64>,
) -> Result<f64, String> {
    let cell = nl.cell(cell_id);
    let fallback = seq_hold(nl, &cell.kind).unwrap_or(0.0);
    let CellKind::Lib(id) = &cell.kind else {
        return Ok(fallback);
    };
    let Some(seq) = nl.library().map(|library| library.get(*id).seq.clone()) else {
        return Ok(fallback);
    };
    let Some(tables) = seq.as_ref().and_then(|seq| seq.hold_tables.as_ref()) else {
        return Ok(fallback);
    };
    let data_transition = data_transition_ns.unwrap_or(0.0);
    let clock_transition = clock_transition_ns.unwrap_or(0.0);
    Ok(tables
        .evaluate_worst(data_transition, clock_transition)?
        .unwrap_or(fallback)
        .max(0.0))
}

fn seq_min_pulse(nl: &Netlist, kind: &CellKind) -> Option<(f64, f64)> {
    match kind {
        CellKind::Lib(id) => nl.library().and_then(|l| {
            l.get(*id)
                .seq
                .as_ref()
                .map(|s| (s.min_pulse_high, s.min_pulse_low))
        }),
        _ => None,
    }
}

fn seq_reset(nl: &Netlist, kind: &CellKind) -> Option<(usize, f64, f64)> {
    match kind {
        CellKind::Lib(id) => nl.library().and_then(|l| {
            l.get(*id)
                .seq
                .as_ref()
                .and_then(|s| s.reset_pin.map(|pin| (pin, s.recovery, s.removal)))
        }),
        _ => None,
    }
}

/// Recovery constraint at a concrete sequential cell and transition pair.
///
/// Indexed Liberty constraints use the asynchronous reset-pin transition as
/// index 1 and the related clock-pin transition as index 2.
fn seq_recovery_at(
    nl: &Netlist,
    cell_id: usize,
    reset_transition_ns: Option<f64>,
    clock_transition_ns: Option<f64>,
) -> Result<f64, String> {
    let cell = nl.cell(cell_id);
    let fallback = seq_reset(nl, &cell.kind)
        .map(|(_, recovery, _)| recovery)
        .unwrap_or(0.0);
    let CellKind::Lib(id) = &cell.kind else {
        return Ok(fallback);
    };
    let Some(seq) = nl.library().map(|library| library.get(*id).seq.clone()) else {
        return Ok(fallback);
    };
    let Some(tables) = seq.as_ref().and_then(|seq| seq.recovery_tables.as_ref()) else {
        return Ok(fallback);
    };
    let reset_transition = reset_transition_ns.unwrap_or(0.0);
    let clock_transition = clock_transition_ns.unwrap_or(0.0);
    Ok(tables
        .evaluate_worst(reset_transition, clock_transition)?
        .unwrap_or(fallback)
        .max(0.0))
}

/// Removal constraint at a concrete sequential cell and transition pair.
fn seq_removal_at(
    nl: &Netlist,
    cell_id: usize,
    reset_transition_ns: Option<f64>,
    clock_transition_ns: Option<f64>,
) -> Result<f64, String> {
    let cell = nl.cell(cell_id);
    let fallback = seq_reset(nl, &cell.kind)
        .map(|(_, _, removal)| removal)
        .unwrap_or(0.0);
    let CellKind::Lib(id) = &cell.kind else {
        return Ok(fallback);
    };
    let Some(seq) = nl.library().map(|library| library.get(*id).seq.clone()) else {
        return Ok(fallback);
    };
    let Some(tables) = seq.as_ref().and_then(|seq| seq.removal_tables.as_ref()) else {
        return Ok(fallback);
    };
    let reset_transition = reset_transition_ns.unwrap_or(0.0);
    let clock_transition = clock_transition_ns.unwrap_or(0.0);
    Ok(tables
        .evaluate_worst(reset_transition, clock_transition)?
        .unwrap_or(fallback)
        .max(0.0))
}

fn output_capacitance_pf(nl: &Netlist, output: NetId) -> f64 {
    let mut capacitance_pf = 0.0;
    for &sink in &nl.net(output).sinks {
        let cell = nl.cell(sink);
        let CellKind::Lib(id) = &cell.kind else {
            continue;
        };
        let Some(library) = nl.library() else {
            continue;
        };
        let definition = library.get(*id);
        let metadata = library.metadata(&definition.name);
        for (position, &input) in cell.inputs.iter().enumerate() {
            if input != output {
                continue;
            }
            if let Some(pin) = definition.inputs.get(position) {
                capacitance_pf += metadata
                    .input_capacitance_pf
                    .get(pin)
                    .copied()
                    .unwrap_or(0.0);
            }
        }
    }
    capacitance_pf
}

fn indexed_arc_delay(
    nl: &Netlist,
    ci: usize,
    input_pin: usize,
    input_transition_ns: f64,
) -> Option<f64> {
    let cell = nl.cell(ci);
    let CellKind::Lib(id) = &cell.kind else {
        return None;
    };
    let library = nl.library()?;
    let definition = library.get(*id);
    let tables = definition
        .timing_tables
        .iter()
        .find(|tables| tables.input_pin == input_pin)?;
    let output_cap_pf = output_capacitance_pf(nl, cell.output);
    let rise = tables
        .rise
        .as_ref()
        .and_then(|lut| lut.evaluate(input_transition_ns, output_cap_pf).ok());
    let fall = tables
        .fall
        .as_ref()
        .and_then(|lut| lut.evaluate(input_transition_ns, output_cap_pf).ok());
    Some(rise?.max(fall.unwrap_or(f64::NEG_INFINITY)))
}

fn input_arc_transition(
    nl: &Netlist,
    ci: usize,
    input_pin: usize,
    input_transition_ns: f64,
) -> Option<f64> {
    let cell = nl.cell(ci);
    let CellKind::Lib(id) = &cell.kind else {
        return None;
    };
    let library = nl.library()?;
    let definition = library.get(*id);
    let tables = definition
        .timing_tables
        .iter()
        .find(|tables| tables.input_pin == input_pin)?;
    let output_cap_pf = output_capacitance_pf(nl, cell.output);
    let rise = tables
        .rise_transition
        .as_ref()
        .and_then(|lut| lut.evaluate(input_transition_ns, output_cap_pf).ok());
    let fall = tables
        .fall_transition
        .as_ref()
        .and_then(|lut| lut.evaluate(input_transition_ns, output_cap_pf).ok());
    Some(rise?.max(fall.unwrap_or(f64::NEG_INFINITY)))
}

/// Transition produced by one input→output arc.
///
/// Liberty transition tables take precedence. Arcs without a transition table
/// use the same conservative scalar approximation as quality analysis: the
/// incoming transition plus the arc delay.
fn output_arc_transition(
    nl: &Netlist,
    ci: usize,
    input_pin: usize,
    derate: f64,
    input_transition_ns: f64,
    arc_delay_ns: f64,
) -> f64 {
    input_arc_transition(nl, ci, input_pin, input_transition_ns)
        .map(|transition| transition * derate)
        .unwrap_or(input_transition_ns + arc_delay_ns)
        .max(0.0)
}

/// Per-cell worst input→output arc delay in ns.
fn arc_delay(nl: &Netlist, ci: usize, derate: f64) -> f64 {
    let cell = nl.cell(ci);
    match &cell.kind {
        CellKind::Lib(id) => {
            if let Some(lib) = nl.library() {
                let def = lib.get(*id);
                if def.seq.is_none() {
                    let indexed = def
                        .timing_tables
                        .iter()
                        .filter_map(|tables| indexed_arc_delay(nl, ci, tables.input_pin, 0.0))
                        .fold(f64::NEG_INFINITY, f64::max);
                    return if indexed.is_finite() {
                        indexed
                    } else {
                        def.arcs.iter().map(|(_, d)| *d).fold(0.0f64, f64::max)
                    } * derate;
                }
            }
            0.0
        }
        _ => 0.5 * derate, // nominal generic primitive delay
    }
}

/// Delay for one input→output arc at a specific input transition.
///
/// Generic cells use the same nominal delay as setup analysis.
fn input_arc_delay_with_transition(
    nl: &Netlist,
    ci: usize,
    input: usize,
    derate: f64,
    input_transition_ns: f64,
) -> f64 {
    let cell = nl.cell(ci);
    match &cell.kind {
        CellKind::Lib(id) => {
            if let Some(lib) = nl.library() {
                let def = lib.get(*id);
                if def.seq.is_none() {
                    let scalar = def
                        .arcs
                        .iter()
                        .find(|&&(pin, _)| pin == input)
                        .map(|&(_, delay)| delay * derate)
                        .unwrap_or(0.0);
                    return indexed_arc_delay(nl, ci, input, input_transition_ns)
                        .map_or(scalar, |delay| delay * derate);
                }
            }
            0.0
        }
        _ => 0.5 * derate,
    }
}

/// Walk backwards from `net` collecting the cells on the max-arrival path.
fn backtrack(nl: &Netlist, arrival: &[f64], mut net: NetId) -> Vec<usize> {
    let mut path = Vec::new();
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > 10_000 {
            break;
        }
        let drivers = nl.net(net).drivers.clone();
        let mut best: Option<(usize, f64, NetId)> = None; // (cell, pin time, pin net)
        for d in drivers {
            let cell = nl.cell(d);
            if nl.cell_is_sequential(&cell.kind) {
                if best.is_none() {
                    best = Some((d, f64::NEG_INFINITY, usize::MAX));
                }
                continue;
            }
            let mut pin_time = f64::NEG_INFINITY;
            let mut pin_net = usize::MAX;
            for &i in &cell.inputs {
                if arrival[i].is_finite() && arrival[i] > pin_time {
                    pin_time = arrival[i];
                    pin_net = i;
                }
            }
            if best.is_none_or(|(_, bt, _)| pin_time > bt) {
                best = Some((d, pin_time, pin_net));
            }
        }
        match best {
            None => break,
            Some((cell, _t, prev)) => {
                path.push(cell);
                if prev == usize::MAX {
                    break; // launched by a flop
                }
                net = prev;
                let is_input = nl
                    .ports
                    .iter()
                    .any(|p| p.dir == eda_core::netlist::Dir::In && p.bits.contains(&net));
                if is_input {
                    break;
                }
            }
        }
    }
    path.reverse();
    path
}

/// Sum of cell delays along a cell path (for reports).
pub fn path_delay(nl: &Netlist, cells: &[usize]) -> f64 {
    cells.iter().map(|&c| arc_delay(nl, c, 1.0)).sum()
}

/// Human-readable timing report.
pub fn report(nl: &Netlist, r: &StaResult) -> String {
    format!(
        "STA report for '{}':\n  critical path : {:.3} ns ({} cells)\n  setup reg2reg : {:.3} ns\n  setup in2reg  : {:.3} ns\n  hold reg2reg  : {:.3} ns\n  hold in2reg   : {:.3} ns\n",
        nl.name,
        r.critical_path_delay,
        r.critical_cells.len(),
        r.reg2reg_critical,
        r.in2reg_critical,
        r.reg2reg_hold_critical,
        r.in2reg_hold_critical
    )
}

/// Map of net name → arrival time (debug helper).
pub fn arrival_by_name(nl: &Netlist, r: &StaResult) -> HashMap<String, f64> {
    let mut m = HashMap::new();
    for (i, n) in nl.nets().iter().enumerate() {
        if let Some(&t) = r.arrivals.get(i) {
            if t.is_finite() {
                m.insert(n.name.clone(), t);
            }
        }
    }
    m
}

/// Map of net name → earliest arrival time (hold/min-delay debug helper).
pub fn min_arrival_by_name(nl: &Netlist, r: &StaResult) -> HashMap<String, f64> {
    let mut m = HashMap::new();
    for (i, n) in nl.nets().iter().enumerate() {
        if let Some(&t) = r.min_arrivals.get(i) {
            if t.is_finite() {
                m.insert(n.name.clone(), t);
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::library::CellLibrary;
    use eda_core::netlist::{Cell, Dir};
    use std::sync::Arc;

    #[test]
    fn inv_chain() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("c");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let mut prev = a;
        for index in 0..3 {
            let out = nl.add_net(format!("o{}", index));
            nl.add_cell(Cell {
                name: format!("u{}", index),
                kind: CellKind::Lib(lib.find("INV").unwrap()),
                inputs: vec![prev],
                output: out,
            });
            prev = out;
        }
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y", Dir::Out, vec![prev]);
        let r = analyze(&nl).unwrap();
        assert!((r.critical_path_delay - 1.2).abs() < 1e-9);
        assert_eq!(r.critical_path_cells().len(), 3);

        let paths = r.timing_paths(&nl, 1);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].kind, TimingPathKind::InputToOutput);
        assert_eq!(paths[0].launch, TimingLaunch::PrimaryInput("a".into()));
        assert_eq!(
            paths[0].endpoint,
            TimingPathEndpoint::OutputPort("y".into())
        );
        assert_eq!(paths[0].cells, vec![0, 1, 2]);
        assert_eq!(
            paths[0].nets,
            vec![
                a,
                nl.net_by_name("o0").unwrap(),
                nl.net_by_name("o1").unwrap(),
                prev
            ]
        );
        assert_eq!(paths[0].arcs.len(), 4);
        assert_eq!(paths[0].arcs[0].kind, TimingArcKind::PrimaryInputLaunch);
        assert!(paths[0].arcs[1..]
            .iter()
            .all(|arc| arc.kind == TimingArcKind::Combinational));
        assert!((paths[0].arrival_ns - 1.2).abs() < 1e-9);
        let report = timing_path_report(&nl, &paths);
        assert!(report.contains("input-to-output -> output y"), "{report}");
        assert!(report.contains("launch: input a"), "{report}");
        assert!(report.contains("combinational 0.400000 ns"), "{report}");
    }

    #[test]
    fn min_timing_paths_are_sorted_by_earliest_arrival() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("min-paths");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let y1 = nl.add_net("y1");
        let y2 = nl.add_net("y2");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![a],
            output: y1,
        });
        nl.add_cell(Cell {
            name: "u2".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![y1],
            output: y2,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y1", Dir::Out, vec![y1]);
        nl.add_port("y2", Dir::Out, vec![y2]);

        let timing = analyze(&nl).unwrap();
        let late = timing.timing_paths(&nl, 0);
        let early = timing.min_timing_paths(&nl, 0);
        assert_eq!(
            late[0].endpoint,
            TimingPathEndpoint::OutputPort("y2".into())
        );
        assert_eq!(
            early[0].endpoint,
            TimingPathEndpoint::OutputPort("y1".into())
        );
        assert!((early[0].arrival_ns - 0.4).abs() < 1e-9);
        assert!((early[1].arrival_ns - 0.8).abs() < 1e-9);
    }
}
