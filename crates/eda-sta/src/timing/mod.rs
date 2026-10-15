//! Core timing engine split by model, propagation, incremental updates, and reporting.

mod api;
mod engine;
mod incremental;
mod library;
mod reports;
mod result;
mod types;

pub use api::{
    analyze, analyze_with_constraints, diff_timing_topology, update_timing_for_constraints,
    update_timing_for_input_delays, update_timing_for_timing_derates,
};
pub use incremental::update_timing_for_topology;
pub(crate) use library::{
    driven_from_sequential, input_pin_name, output_pin_name, seq_hold_at, seq_min_pulse,
    seq_recovery_at, seq_removal_at, seq_reset, seq_setup_at,
};
pub(crate) use reports::push_timing_arcs_report;
pub use reports::{
    arrival_by_name, hierarchical_timing_path, hierarchical_timing_path_report,
    hierarchical_timing_paths, hold_timing_path_report, min_arrival_by_name, path_delay, report,
    reset_timing_path_report, timing_path_report, worst_hold_slack, worst_slack,
};
pub use types::{
    ConstraintTimingResult, HierarchicalTimingCrossing, HierarchicalTimingPath,
    HierarchicalTimingSegment, HoldEndpoint, HoldTimingPath, IncrementalTimingResult,
    ResetEndpoint, ResetTimingPaths, StaResult, TimingArc, TimingArcKind, TimingLaunch, TimingPath,
    TimingPathEndpoint, TimingPathKind, TimingTopologyDiff, TopologyTimingResult,
};
