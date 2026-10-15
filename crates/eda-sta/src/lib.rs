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

use eda_core::netlist::Netlist;

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
    MinPulsePolarity, MinPulseWidthCheck, NetSlack, PathDelayLimit, PathException,
    PathExceptionCheck, PortDelay, ResetCheckKind, ScopedCellTimingDerate, ScopedPinTimingDerate,
    SdcUnits, TimingClockEdge, TimingClockEdgeKind, TimingConstraintDiff, TimingConstraints,
    TimingDerate, TimingObjectKind, TimingObjectPattern, TimingObjectPatternToken, TimingObjectRef,
    TimingPathClocks, TimingPin,
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

// Core timing engine split by data model, propagation, incremental updates, and reporting.
mod timing;
pub use timing::*;
pub(crate) use timing::{
    driven_from_sequential, input_pin_name, output_pin_name, push_timing_arcs_report, seq_hold_at,
    seq_min_pulse, seq_recovery_at, seq_removal_at, seq_reset, seq_setup_at,
};

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::library::CellLibrary;
    use eda_core::netlist::{Cell, CellKind, Dir};
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
