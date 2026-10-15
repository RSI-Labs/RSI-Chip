//! Core timing implementation module.

use crate::constraints;
use crate::ConstrainedTiming;
use eda_core::netlist::*;
use std::collections::*;

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
    pub(super) arrivals: Vec<f64>,
    /// Earliest arrival per net.
    pub(super) min_arrivals: Vec<f64>,
    /// Worst-case propagated transition per net, in nanoseconds. NaN marks a
    /// net with no propagated timing event.
    pub(super) transitions: Vec<f64>,
    /// Minimum propagated transition per net, used by early/hold analysis.
    pub(super) min_transitions: Vec<f64>,
    /// Selected late critical input per combinational output net.
    pub(super) late_path_inputs: Vec<Option<NetId>>,
    /// Selected early critical input per combinational output net.
    pub(super) early_path_inputs: Vec<Option<NetId>>,
    /// Cell sequence (driver first) of the critical path.
    pub(super) critical_cells: Vec<usize>,
    /// (sequential cell id, data arrival) endpoints.
    pub(super) setup_endpoints: Vec<(usize, f64)>,
    pub(super) hold_endpoints: Vec<HoldEndpoint>,
    pub(super) reset_endpoints: Vec<ResetEndpoint>,
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
pub(super) struct PathTrace {
    pub(super) launch: TimingLaunch,
    pub(super) cells: Vec<usize>,
    pub(super) nets: Vec<NetId>,
    pub(super) arcs: Vec<TimingArc>,
}

pub(super) fn input_launch_arc(net: NetId, arrival: Option<f64>) -> TimingArc {
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

pub(super) fn sequential_clock_pin(nl: &Netlist, kind: &CellKind) -> Option<usize> {
    match kind {
        CellKind::Lib(id) => nl
            .library()
            .and_then(|library| library.get(*id).seq.as_ref().map(|seq| seq.clk_pin)),
        CellKind::Dff { .. } | CellKind::DffR { .. } => Some(0),
        _ => None,
    }
}

pub(super) fn sequential_launch_arc<F>(
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

pub(super) fn combinational_arc(
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
