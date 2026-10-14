//! # eda-api — high-level one-call flows
//!
//! Facade over the whole toolchain: compile Verilog, simulate, synthesize,
//! verify equivalence, timing analysis and netlist export.

mod capabilities;
pub mod flow_cache;
mod pdk;
mod physical_timing;

pub use capabilities::{
    capabilities, capability_report, missing_capabilities, Capability, CapabilityStatus,
};
pub use flow_cache::{CachedFlow, FlowCache, FlowCacheKey, FlowCacheStats, SharedFlowCache};
pub use pdk::*;

pub use eda_ams::{
    ac_sweep as sweep_ac, aging_analysis, constrained_pareto_dominates, constraint_violation,
    dc_operating_point, dc_operating_point_cancellable, dc_sweep, dc_sweep_cancellable,
    derivative_analysis, envelope_follow, global_sensitivity, global_sensitivity_cancellable,
    monte_carlo, monte_carlo_cancellable, noise_sweep, optimize, optimize_cancellable,
    pareto_dominates, pareto_optimize, pareto_optimize_cancellable, parse_resistor_layer_models,
    parse_statistical_model, periodic_steady_state, periodic_steady_state_cancellable,
    reference_direction_count, report as report_ams_result, run_all as run_all_ams_analyses,
    run_analysis as run_ams_analysis, sensitivity,
    threshold_crossings as analog_threshold_crossings, transient, transient_cancellable,
    write_resistor_layer_models, write_statistical_model, write_transient_csv, AgingOptions,
    AgingResult, AmsResult, Analysis, BjtModel, BjtPolarity, ConstraintExpression,
    ConstraintFunction, ConstraintSense, CrossingEdge, DcOperatingPoint, DerivativeEntry,
    DerivativeOptions, DerivativeResult, DerivativeTarget, ElementLocation, EnvelopeOptions,
    EnvelopeResult, GlobalSensitivityOptions, GlobalSensitivityResult, MonteCarloDistribution,
    MonteCarloOptions, MonteCarloResult, MosfetModel, MosfetPolarity, NoiseResult,
    OptimizationObjective, OptimizationOptions, OptimizationParameter, OptimizationResult,
    ParameterCorrelation, ParameterCovariance, ParetoConstraint, ParetoFrontMethod, ParetoOptions,
    ParetoPoint, ParetoResult, ParetoSense, PssOptions, PssResult, ResistorLayerModel,
    ResistorWidthBin, SensitivityOptions, SensitivityResult, SourceWaveform, SpiceCircuit,
    SpiceElement, SpiceElementKind, StatisticalModel, StatisticalParameterModel,
};
pub use eda_assert::{
    parse_assertions as parse_sva, report as assertion_report, Assertion, AssertionKind,
    AssertionResult, AssertionStatus, BoolExpr, Property,
};
pub use eda_cdc::{
    analyze as analyze_cdc, report as cdc_report, CdcFinding, CdcHierarchySummary, CdcPath,
    CdcResult, ClockDomain, Severity,
};
pub use eda_core::cancellation::FlowCancellation;
pub use eda_core::ir::{Design, HierarchicalInstance, HierarchicalPort};
pub use eda_core::liberty::{LibCellMetadata, LibInternalPower, LibLeakageState};
pub use eda_core::library::CellLibrary;
pub use eda_core::library::{LibConstraintTables, LibTimingTables, LibertyLut};
use eda_core::logic::LogicWord;
use eda_core::netlist::{Netlist, NetlistStats};
pub use eda_core::netlist::{NetlistHierarchyInstance, NetlistHierarchyPort};
use eda_core::to_design::netlist_to_design;
pub use eda_coverage::{
    check_coverage, parse_coverage, report as coverage_report, Bin, BinResult, CoverGroup,
    CoverPoint, CoveragePlan, CoverageReport, Cross, CrossBinResult, Edge, Range, SignalSelect,
    Toggle, ToggleResult,
};
pub use eda_cts::{
    report as cts_report, synthesize as synthesize_clock_tree,
    timing_driven_report as timing_driven_cts_report, ClockSink, ClockTree, ClockTreeResult,
    CtsOptions, TimingDrivenCtsOptions, TimingDrivenCtsResult,
};
pub use eda_dft::{
    analyze as analyze_dft, analyze_with_ports as analyze_dft_with_ports, insert_scan_chain,
    report as dft_report, DftReport, ScanCell, ScanChainResult, ScanOptions,
};
pub use eda_eco::{
    apply_eco, apply_eco_in_hierarchy, parse_eco, report as eco_report, EcoCellTarget, EcoChange,
    EcoOperation, EcoPatch, EcoResult, FanoutRepairOptions,
};
pub use eda_fault::{
    automatic_patterns as generate_fault_patterns, fault_universe,
    grade_automatic as grade_faults_automatic, grade_patterns as grade_fault_patterns, parse_stil,
    report as fault_report, write_stil, Fault, FaultCoverageResult, FaultDetection, Pattern,
    PatternOptions, StilPattern, StilPatternSet, StuckAt,
};
pub use eda_formal::{
    proof_report, solve as solve_cnf, solve_cancellable as solve_cnf_cancellable,
    solve_with_assumptions as solve_cnf_with_assumptions,
    solve_with_assumptions_cancellable as solve_cnf_with_assumptions_cancellable, BmcOptions,
    BmcResult, Cnf, EquivResult, ProofInvariant, ProofResult, ProofStatus, SatResult,
};
pub use eda_formal::{
    property_bmc_report, PropertyBmcResult, PropertyBmcStatus, PropertyReplayResult,
};
pub use eda_ir::{
    analyze as analyze_ir_drop, analyze_mesh as analyze_power_mesh, analyze_mesh_currents,
    analyze_multilayer_mesh, analyze_multilayer_mesh_currents, analyze_upf_coupled_multilayer_mesh,
    analyze_upf_domain_multilayer_meshes, analyze_upf_power_modes_with_coupled_multilayer_mesh,
    analyze_upf_power_modes_with_mesh, analyze_upf_power_modes_with_multilayer_mesh,
    estimate_power_mesh_em_lifetime as estimate_multilayer_mesh_em_lifetime, report as ir_report,
    report_mesh as power_mesh_report, report_upf as upf_ir_report,
    report_upf_power_modes as upf_power_mode_ir_report, DomainIrOptions, IrCell, IrFinding,
    IrOptions, IrResult, IrRow, IrSeverity, MultilayerPowerDomainResult,
    MultilayerPowerDomainSupply, MultilayerPowerMeshCell, MultilayerPowerMeshNode,
    MultilayerPowerMeshOptions, MultilayerPowerMeshResult, MultilayerPowerMeshSegment,
    PowerMeshCell, PowerMeshEmOptions, PowerMeshLayer, PowerMeshLayerTap, PowerMeshNetwork,
    PowerMeshNode, PowerMeshOptions, PowerMeshResult, PowerMeshSegment,
    PowerMeshSegmentOrientation, PowerMeshTap, PowerMeshVia, UpfDomainMultilayerMeshResult,
    UpfIrCell, UpfIrDomain, UpfIrDomainRow, UpfIrOptions, UpfIrResult, UpfIrStrategy, UpfIrSwitch,
    UpfPowerModeIr, UpfPowerModeIrResult,
};
pub use eda_lvs::{check_lvs as check_layout_vs_schematic, report as lvs_report, LvsResult};
pub use eda_pnr::any_angle::{
    any_angle_boolean, AnyAngleBooleanOperation, AnyAngleBooleanResult, AnyAnglePolygon,
    AnyAngleRationalPoint, AnyAngleRationalPolygon, AnyAngleSpacingResult, RationalSquare,
};
pub use eda_pnr::any_angle_maze::{
    route_any_angle_maze, route_any_angle_maze_cancellable, route_any_angle_maze_ripgup,
    route_any_angle_maze_ripgup_cancellable, AnyAngleMazeRipupResult, AnyAngleMazeRoute,
    AnyAngleMazeRouting, AnyAngleMazeRoutingOptions, AnyAngleRouteConflict,
};
pub use eda_pnr::any_angle_route::{
    route_any_angle_direct, AnyAngleRoute, AnyAngleRouteRequest, AnyAngleRouting,
    AnyAngleRoutingOptions,
};
pub use eda_pnr::layout::*;
pub use eda_pnr::Placement;
pub use eda_pnr::{
    optimize_placement_timing_driven, timing_driven_placement_report, TimingDrivenPlacementOptions,
    TimingDrivenPlacementResult,
};
pub use eda_pnr::{
    timing_driven_ripgup_report, MazeRipupResult, TimingDrivenRipupResult,
    TimingDrivenRoutingOptions,
};
pub use eda_pnr::{
    Def, DefComponent, DefPlacementBlockage, DefRouteBlockage, DefVia, PlacementBlockage,
    RouteChannelBlockage, RouteOptions, Via, SITES_PER_ROW,
};
pub use eda_pnr::{DrcOptions, DrcResult, DrcSeverity, Routing};
pub use eda_pnr::{LayoutBrowserServer, LayoutBrowserService};
pub use eda_power::report as power_report;
pub use eda_power::{CellPower, InputVector, PowerOptions, PowerReport, SwitchingActivity};
pub use eda_schematic::*;
pub use eda_sdf::{
    annotate_sdf as back_annotate_sdf, parse_sdf, report as sdf_report, write_sdf, Interconnect,
    Iopath, Sdf, SdfDelay, SdfTimingCheck, TimingCheckKind,
};
use eda_sim::Simulator;
pub use eda_spef::{
    analyze as analyze_parasitics, parse_spef, report as parasitic_report, NetParasitics,
    ParasiticReport, Spef, SpefCap, SpefConnection, SpefNet, SpefRes, SpefUnits,
};
pub use eda_sta::{
    quality_report as signal_quality_report, QualityKind, QualityOptions, QualityResult,
    QualityViolation, ScopedQualityLimit,
};
pub use eda_sta::{
    timing_violation_dirty_objects_from_constraints, timing_violation_dirty_objects_from_topology,
    AppliedTimingDelayLimit, CaseAnalysis, Clock, ClockGroup, ClockGroupKind,
    ConstrainedHoldTimingPath, ConstrainedResetTimingPath, ConstrainedTiming,
    ConstrainedTimingPath, ConstraintTimingResult, CornerTiming,
    CrossCornerHierarchyViolationSummary, CrossCornerTimingViolation,
    CrossCornerTimingViolationChange, CrossCornerTimingViolationDiff, DisabledTimingArc,
    ExcludedTimingCheck, ExcludedTimingPath, HierarchicalTimingCrossing, HierarchicalTimingPath,
    HierarchicalTimingSegment, HoldEndpoint, HoldTimingPath, IncrementalMultiCornerTimingResult,
    IncrementalMultiCornerTopologyTimingResult, IncrementalTimingResult, MinPulsePolarity,
    MinPulseWidthCheck, MultiCornerScenarioSetDiff, MultiCornerScenarioUpdate,
    MultiCornerScenarioUpdateKind, MultiCornerTimingResult, NetSlack, PathDelayLimit,
    PathException, PortDelay, ResetCheckKind, ResetEndpoint, ResetTimingPaths,
    ScopedCellTimingDerate, ScopedPinTimingDerate, SdcUnits, StaResult, TimingArc, TimingArcKind,
    TimingClockEdge, TimingClockEdgeKind, TimingConstraintDiff, TimingConstraints, TimingDerate,
    TimingLaunch, TimingObjectKind, TimingObjectPattern, TimingObjectPatternToken, TimingObjectRef,
    TimingPath, TimingPathClocks, TimingPathEndpoint, TimingPathKind, TimingPin, TimingScenario,
    TimingScenarioDefinition, TimingScenarioDiff, TimingScenarioPropertyDiff, TimingTopologyDiff,
    TimingViolation, TimingViolationChange, TimingViolationDiff, TimingViolationDirtyObjects,
    TimingViolationKind, TimingViolationPath, TimingViolationRegistry,
    TimingViolationRegistryUpdate, TimingViolationStore, TimingViolationWaiver,
    TimingWaiverAuditAction, TimingWaiverAuditEntry, TimingWaiverAuditHierarchySummary,
    TopologyTimingResult,
};
pub use eda_sta::{
    BlockTimingClock, BlockTimingClockMap, BlockTimingClockRelationship,
    BlockTimingCompositionConnection, BlockTimingCompositionInstance, BlockTimingCompositionResult,
    BlockTimingDirection, BlockTimingHoldInterfaceCheck, BlockTimingInterfaceCheck,
    BlockTimingModel, BlockTimingPort,
};
pub use eda_sta::{
    MultiCornerHoldTimingPath, MultiCornerResetTimingPath, MultiCornerSetupTimingPath,
    ScenarioSlack,
};
use eda_synth::writer::write_verilog;
use eda_synth::{synthesize, SynthOptions};
pub use eda_upf::{
    check as check_upf, parse_upf, power_mode_strategy_key, report as upf_report, Isolation,
    LevelShifter, PowerDomain, PowerMode, PowerSwitch, Retention, Severity as UpfSeverity,
    SupplyNet, SupplyPort, Upf, UpfFinding, UpfResult,
};
pub use eda_waveform::write_vcd;
pub use eda_waveform::{
    parse_vcd as parse_waveform, ClockMetrics, EdgeDirection, PulseMetrics, SignalComparison, Vcd,
    VcdSignal, WaveformEdge,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Compile Verilog source into an elaborated [`Design`].
pub fn compile(src: &str, top: Option<&str>) -> Result<Design, String> {
    eda_hdl::compile(src, top)
}

/// Run one parsed AMS analysis with cooperative cancellation where supported.
pub fn run_ams_analysis_cancellable(
    circuit: &SpiceCircuit,
    analysis: &Analysis,
    cancellation: &FlowCancellation,
) -> Result<AmsResult, String> {
    match analysis {
        Analysis::OperatingPoint => Ok(AmsResult::OperatingPoint(
            eda_ams::dc_operating_point_cancellable(circuit, cancellation)?,
        )),
        Analysis::DcSweep { .. } => Ok(AmsResult::DcSweep(eda_ams::dc_sweep_cancellable(
            circuit,
            analysis,
            cancellation,
        )?)),
        Analysis::Transient { .. } => Ok(AmsResult::Transient(eda_ams::transient_cancellable(
            circuit,
            analysis,
            cancellation,
        )?)),
        Analysis::PeriodicSteadyState(_) => Ok(AmsResult::PeriodicSteadyState(
            eda_ams::periodic_steady_state_cancellable(circuit, analysis, cancellation)?,
        )),
        Analysis::MonteCarlo(_) => Ok(AmsResult::MonteCarlo(eda_ams::monte_carlo_cancellable(
            circuit,
            analysis,
            cancellation,
        )?)),
        Analysis::GlobalSensitivity(_) => Ok(AmsResult::GlobalSensitivity(
            eda_ams::global_sensitivity_cancellable(circuit, analysis, cancellation)?,
        )),
        Analysis::Optimization(_) => Ok(AmsResult::Optimization(eda_ams::optimize_cancellable(
            circuit,
            analysis,
            cancellation,
        )?)),
        Analysis::Pareto(_) => Ok(AmsResult::Pareto(eda_ams::pareto_optimize_cancellable(
            circuit,
            analysis,
            cancellation,
        )?)),
        _ => run_ams_analysis(circuit, analysis),
    }
}

/// Run all parsed AMS analyses with cooperative cancellation where supported.
pub fn run_all_ams_analyses_cancellable(
    circuit: &SpiceCircuit,
    cancellation: &FlowCancellation,
) -> Result<Vec<AmsResult>, String> {
    circuit
        .analyses
        .iter()
        .map(|analysis| run_ams_analysis_cancellable(circuit, analysis, cancellation))
        .collect()
}

/// Parse a SPICE netlist for analog/mixed-signal analyses.
pub fn parse_spice(source: &str) -> Result<eda_ams::SpiceCircuit, String> {
    eda_ams::parse_spice(source)
}

/// Serialize a validated SPICE circuit.
pub fn write_spice(circuit: &eda_ams::SpiceCircuit) -> Result<String, String> {
    eda_ams::write_spice(circuit)
}

/// Run every analysis directive in a SPICE netlist.
pub fn run_spice(source: &str) -> Result<Vec<AmsResult>, String> {
    let circuit = parse_spice(source)?;
    eda_ams::run_all(&circuit)
}

/// Render all SPICE analysis results as a compact agent-readable report.
pub fn spice_report(results: &[AmsResult]) -> String {
    eda_ams::report_all(results)
}

/// Return the preserved elaborated module hierarchy.
pub fn hierarchy_instances(design: &Design) -> &[HierarchicalInstance] {
    &design.hierarchy
}

/// Find the deepest hierarchical instance owning a flat net bit.
pub fn owning_hierarchy_instance(design: &Design, net: usize) -> Option<&HierarchicalInstance> {
    design.owning_instance(net)
}

/// Render the preserved elaborated module hierarchy.
pub fn hierarchy_report(design: &Design) -> String {
    design.hierarchy_report()
}

/// Return hierarchy metadata retained by a synthesized netlist.
pub fn netlist_hierarchy_instances(netlist: &Netlist) -> &[NetlistHierarchyInstance] {
    &netlist.hierarchy
}

/// Find the deepest post-synthesis hierarchy instance owning a net.
pub fn owning_netlist_hierarchy_instance(
    netlist: &Netlist,
    net: usize,
) -> Option<&NetlistHierarchyInstance> {
    netlist.owning_hierarchy_instance(net)
}

/// Attribute a post-synthesis cell to its output-net hierarchy owner.
pub fn cell_hierarchy_instance(
    netlist: &Netlist,
    cell: usize,
) -> Option<&NetlistHierarchyInstance> {
    netlist.cell_hierarchy_instance(cell)
}

/// Render hierarchy retained by a synthesized netlist.
pub fn netlist_hierarchy_report(netlist: &Netlist) -> String {
    netlist.hierarchy_report()
}

/// Parse a Liberty `.lib` subset into a synthesis/STA/PnR cell library.
pub fn parse_liberty(source: &str) -> Result<CellLibrary, String> {
    eda_core::liberty::parse_liberty(source)
}

/// Export a library-backed netlist as reparseable structural Verilog.
pub fn export_verilog(netlist: &Netlist) -> Result<String, String> {
    let library = netlist
        .library()
        .ok_or_else(|| "Verilog export requires a library-backed netlist".to_string())?;
    Ok(write_verilog(netlist, library.as_ref()))
}

/// Evaluate a combinational design on one input vector.
pub fn simulate_comb(d: &Design, inputs: &[(&str, u64)], out: &str) -> Result<u64, String> {
    let mut sim = Simulator::new(d)?;
    for (p, v) in inputs {
        sim.set_input(p, &LogicWord::from_u64(width_of_port(d, p)?, *v))
            .map_err(|e| e.to_string())?;
    }
    let w = sim
        .read_port(out)
        .or_else(|| sim.read(out))
        .ok_or_else(|| format!("no output named '{}'", out))?;
    w.to_u64().ok_or_else(|| format!("'{}' is X", out))
}

fn width_of_port(d: &Design, name: &str) -> Result<usize, String> {
    d.port(name)
        .map(|p| p.width)
        .or_else(|| d.net_decls.iter().find(|n| n.name == name).map(|n| n.width))
        .ok_or_else(|| format!("no port named '{}'", name))
}

/// Result of the full RTL→mapped-netlist flow.
#[derive(Clone, Debug)]
pub struct Flow {
    pub design: Design,
    /// Optimized + library-mapped netlist.
    pub netlist: Netlist,
    /// Unoptimized generic netlist (equivalence golden model).
    pub gold: Netlist,
    pub stats: NetlistStats,
    pub timing: eda_sta::StaResult,
    pub verilog: String,
    pub equivalent: bool,
    pub placement: Placement,
    pub routing: Routing,
    pub drc: DrcResult,
    pub lef: String,
    pub def: String,
    pub sdf: String,
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;

    #[test]
    fn flow_cancellation_token_is_cooperative_and_idempotent() {
        let cancellation = FlowCancellation::new();
        assert!(!cancellation.is_cancelled());
        assert!(cancellation.cancel());
        assert!(cancellation.is_cancelled());
        assert!(cancellation.cancel());
        let error = match run_flow_cancellable(
            "module top(input a, output y); assign y = ~a; endmodule",
            Some("top"),
            &cancellation,
        ) {
            Err(error) => error,
            Ok(_) => panic!("cancelled flow unexpectedly completed"),
        };
        assert_eq!(error, "RTL flow cancelled");
    }

    #[test]
    fn ams_directives_dispatch_cancellably() {
        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let operating_point =
            parse_spice("cancelled op\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.op\n.end")
                .unwrap();
        assert_eq!(
            run_ams_analysis_cancellable(
                &operating_point,
                &operating_point.analyses[0],
                &cancellation
            )
            .unwrap_err(),
            "DC/transient solve cancelled"
        );

        let sweep = parse_spice(
            "cancelled dc sweep\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.dc Vin 0 1.8 0.1\n.end",
        )
        .unwrap();
        assert_eq!(
            run_ams_analysis_cancellable(&sweep, &sweep.analyses[0], &cancellation).unwrap_err(),
            "DC sweep cancelled"
        );

        let circuit = parse_spice(
            "cancelled optimizer\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim match V(out) goal=1.2 params=R2.R:100:5000 population=8 iterations=2 seed=17 tol=1e-8\n.end",
        )
        .unwrap();
        let error = run_ams_analysis_cancellable(&circuit, &circuit.analyses[0], &cancellation)
            .unwrap_err();
        assert_eq!(error, "optimization analysis cancelled");
    }

    #[test]
    fn rtl_equivalence_is_structured_and_cancellable() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let result = check_rtl_equivalence(source, Some("top")).unwrap();
        assert!(result.equivalent);
        assert!(result.counterexample.is_empty());
        assert!(result.gold_stats.cells > 0);
        assert!(result.mapped_stats.lib_cells > 0);

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = match check_rtl_equivalence_cancellable(
            "module top(input a, output y); assign y = ~a; endmodule",
            Some("top"),
            &cancellation,
        ) {
            Err(error) => error,
            Ok(_) => panic!("cancelled equivalence unexpectedly completed"),
        };
        assert_eq!(error, "RTL equivalence cancelled");
    }

    #[test]
    fn rtl_timing_is_structured_and_cancellable() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let result = analyze_rtl_timing(source, Some("top")).unwrap();
        assert!(result.critical_path_delay_ns.is_finite());
        assert!(result.critical_path_delay_ns >= 0.0);
        assert!(result.mapped_stats.lib_cells > 0);
        assert!(result.case_analysis_nets.is_empty());

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = analyze_rtl_timing_cancellable(source, Some("top"), &cancellation).unwrap_err();
        assert_eq!(error, "RTL timing analysis cancelled");
    }

    #[test]
    fn rtl_pnr_returns_physical_databases_and_cancels() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let result = implement_rtl_pnr(source, Some("top")).unwrap();
        assert!(!result.placement.cell_pos.is_empty());
        assert!(result.mapped_stats.lib_cells > 0);
        assert!(!result.lef.trim().is_empty());
        assert!(result.lef.contains("VERSION"));
        assert!(!result.def.trim().is_empty());
        assert!(result.def.contains("DESIGN"));
        assert!(!result.drc.has_errors());

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = implement_rtl_pnr_cancellable(source, Some("top"), &cancellation).unwrap_err();
        assert_eq!(error, "RTL PnR cancelled");

        let strict = DrcOptions {
            max_antenna_ratio: 0.001,
        };
        let strict_result =
            implement_rtl_pnr_with_drc_options(source, Some("top"), &strict).unwrap();
        assert!(strict_result.drc.has_warnings());
        assert!(strict_result
            .drc
            .violations
            .iter()
            .any(|violation| violation.code == "ANT-001"));
        assert!(implement_rtl_pnr_with_drc_options(
            source,
            Some("top"),
            &DrcOptions {
                max_antenna_ratio: f64::NAN,
            }
        )
        .is_err());
    }

    #[test]
    fn rtl_antenna_repair_is_structured_and_cancellable() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let options = AntennaRepairOptions::default();
        let result = repair_rtl_antennas(source, Some("top"), &options).unwrap();
        assert!(!result.baseline_drc.has_errors());
        assert_eq!(result.repair.diodes_inserted, 0);
        assert!(result.repair.all_antennas_repaired);
        assert!(result.final_mapped_stats.lib_cells > 0);

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            repair_rtl_antennas_cancellable(source, Some("top"), &options, &cancellation)
                .unwrap_err(),
            "RTL antenna repair cancelled"
        );
        assert!(repair_rtl_antennas(
            source,
            Some("top"),
            &AntennaRepairOptions {
                patch_name: " ".into(),
                ..AntennaRepairOptions::default()
            }
        )
        .is_err());
    }

    #[test]
    fn router_cancellation_surfaces_are_typed() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let design = compile(source, Some("top")).unwrap();
        let netlist = eda_synth::synthesize_with_library(
            &design,
            &SynthOptions::default(),
            &CellLibrary::std(),
        )
        .unwrap()
        .netlist;
        let placement = eda_pnr::place(&netlist).unwrap();
        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            route_with_options_cancellable(
                &netlist,
                &placement,
                &RouteOptions::default(),
                &cancellation
            )
            .unwrap_err(),
            "global routing cancelled"
        );
        assert_eq!(
            route_maze_with_options_cancellable(
                &netlist,
                &placement,
                &RouteOptions::default(),
                &cancellation
            )
            .unwrap_err(),
            "maze routing cancelled"
        );
        assert_eq!(
            route_maze_ripgup_cancellable(
                &netlist,
                &placement,
                &RouteOptions::default(),
                4,
                &cancellation
            )
            .unwrap_err(),
            "rip-up routing cancelled"
        );
        assert_eq!(
            route_maze_timing_ripgup_cancellable(
                &netlist,
                &placement,
                &RouteOptions::default(),
                &TimingDrivenRoutingOptions::default(),
                4,
                &cancellation
            )
            .unwrap_err(),
            "rip-up routing cancelled"
        );
    }

    #[test]
    fn physical_timing_cancellation_reaches_stage_helpers() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let design = compile(source, Some("top")).unwrap();
        let netlist = eda_synth::synthesize_with_library(
            &design,
            &SynthOptions::default(),
            &CellLibrary::std(),
        )
        .unwrap()
        .netlist;
        let placement = eda_pnr::place(&netlist).unwrap();
        let routing =
            eda_pnr::route_with_options(&netlist, &placement, &RouteOptions::default()).unwrap();
        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            evaluate_physical_timing_iteration_cancellable(
                usize::MAX,
                &netlist,
                placement,
                routing,
                &InterconnectModel::default(),
                None,
                &cancellation,
            )
            .unwrap_err(),
            "physical timing analysis cancelled"
        );
        assert_eq!(
            run_physical_timing_flow_cancellable(
                &netlist,
                &PhysicalTimingFlowOptions::default(),
                &cancellation
            )
            .unwrap_err(),
            "physical timing analysis cancelled"
        );
    }

    #[test]
    fn multi_corner_physical_timing_cancellation_is_structured() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let design = compile(source, Some("top")).unwrap();
        let netlist = eda_synth::synthesize_with_library(
            &design,
            &SynthOptions::default(),
            &CellLibrary::std(),
        )
        .unwrap()
        .netlist;
        let placement = eda_pnr::place(&netlist).unwrap();
        let routing =
            eda_pnr::route_with_options(&netlist, &placement, &RouteOptions::default()).unwrap();
        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            run_multi_corner_physical_timing_flow_cancellable(
                &MultiCornerPhysicalTimingFlowOptions::default(),
                &cancellation
            )
            .unwrap_err(),
            "multi-corner physical timing analysis cancelled"
        );
        assert_eq!(
            evaluate_multi_corner_physical_timing_iteration_cancellable(
                usize::MAX,
                &[],
                &netlist,
                placement,
                routing,
                &InterconnectModel::default(),
                &cancellation,
            )
            .unwrap_err(),
            "multi-corner physical timing analysis cancelled"
        );
    }

    #[test]
    fn rtl_power_uses_mapped_activity_and_cancels() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let vectors = vec![
            RtlSimulationVector {
                time_ns: 0,
                inputs: BTreeMap::from([("a".into(), "0".into()), ("b".into(), "0".into())]),
            },
            RtlSimulationVector {
                time_ns: 1,
                inputs: BTreeMap::from([("a".into(), "1".into()), ("b".into(), "1".into())]),
            },
        ];
        let result =
            analyze_rtl_power(source, Some("top"), &vectors, &PowerOptions::default()).unwrap();
        assert_eq!(result.report.vectors, 2);
        assert!(result.report.total_power_uw > 0.0);
        assert!(result.mapped_stats.lib_cells > 0);

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = analyze_rtl_power_cancellable(
            source,
            Some("top"),
            &vectors,
            &PowerOptions::default(),
            &cancellation,
        )
        .unwrap_err();
        assert_eq!(error, "RTL power analysis cancelled");
    }

    #[test]
    fn rtl_ir_uses_mapped_placement_power_and_cancels() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let vectors = vec![
            RtlSimulationVector {
                time_ns: 0,
                inputs: BTreeMap::from([("a".into(), "0".into()), ("b".into(), "0".into())]),
            },
            RtlSimulationVector {
                time_ns: 1,
                inputs: BTreeMap::from([("a".into(), "1".into()), ("b".into(), "1".into())]),
            },
        ];
        let ir_options = IrOptions {
            voltage_v: 1.2,
            vdd_ohm_per_site: 0.002,
            gnd_ohm_per_site: 0.002,
            max_total_drop_v: 0.05,
            max_rail_current_a: 0.05,
        };
        let result = analyze_rtl_ir(
            source,
            Some("top"),
            &vectors,
            &PowerOptions::default(),
            &ir_options,
        )
        .unwrap();
        assert_eq!(result.power.vectors, 2);
        assert!(result.power.total_power_uw > 0.0);
        assert!(!result.placement.cell_pos.is_empty());
        assert_eq!(result.ir.cells.len(), result.mapped_stats.cells);
        assert!(result.ir.total_current_a > 0.0);
        assert!(result.ir.worst_drop_v > 0.0);
        assert!(result.ir.worst_row.is_some());
        assert!(result.mapped_stats.lib_cells > 0);

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = analyze_rtl_ir_cancellable(
            source,
            Some("top"),
            &vectors,
            &PowerOptions::default(),
            &ir_options,
            &cancellation,
        )
        .unwrap_err();
        assert_eq!(error, "RTL IR analysis cancelled");
    }

    #[test]
    fn rtl_property_bmc_maps_and_checks_temporal_targets() {
        let source = r#"
module top(input clk, d, output reg q1, q2);
always @(posedge clk) begin
  q1 <= d;
  q2 <= q1;
end
endmodule
"#;
        let properties = r#"
assert property (@(posedge clk) d |-> ##[2:2] q2);
assert property (@(posedge clk) d |-> ##[1:1] q2);
assume property (@(posedge clk) d || !d);
"#;
        let result = check_rtl_properties(source, Some("top"), properties, 3).unwrap();
        assert_eq!(result.results.len(), 2);
        assert!(!result.results[0].violated());
        assert!(result.results[1].violated());
        assert_eq!(result.assumptions.len(), 1);
        assert!(result.report.contains("ASSUMPTION CONSTRAINT applied"));
        assert!(result.report.contains("BOUNDED PASS"));
        assert!(result.report.contains("VIOLATED"));
        assert!(result.mapped_stats.seq > 0);

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error =
            check_rtl_properties_cancellable(source, Some("top"), properties, 3, &cancellation)
                .unwrap_err();
        assert_eq!(error, "RTL property BMC cancelled");
        assert_eq!(
            check_rtl_properties(source, Some("top"), "assume property (d);", 2).unwrap_err(),
            "SVA input contains assumptions but no assert or cover targets"
        );
        assert_eq!(
            check_rtl_properties(source, Some("top"), properties, 0).unwrap_err(),
            "RTL property BMC requires a positive depth"
        );
    }

    #[test]
    fn raw_sat_and_property_cancellation_surfaces_are_typed() {
        let mut cnf = Cnf::new();
        let input = cnf.add_var() as i32;
        cnf.add_clause(vec![input]);
        assert!(matches!(solve_cnf(&cnf), SatResult::Sat(_)));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            solve_cnf_cancellable(&cnf, &cancellation).unwrap_err(),
            "SAT solving cancelled"
        );

        let source =
            "module top(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule";
        let design = compile(source, Some("top")).unwrap();
        let netlist = eda_synth::synthesize_with_library(
            &design,
            &SynthOptions::default(),
            &CellLibrary::std(),
        )
        .unwrap()
        .netlist;
        let assertions = parse_sva("assert property (@(posedge clk) d |-> q);").unwrap();
        assert_eq!(
            check_assertions_bounded_cancellable(&netlist, &assertions, 8, &cancellation)
                .unwrap_err(),
            "temporal property BMC cancelled"
        );
        assert_eq!(
            bounded_model_check_cancellable(&netlist, "q", 8, &cancellation).unwrap_err(),
            "bounded model checking cancelled"
        );
        assert_eq!(
            bounded_model_check_expression_cancellable(
                &netlist,
                &BoolExpr::Signal("q".into()),
                8,
                &cancellation
            )
            .unwrap_err(),
            "bounded model checking cancelled"
        );
        assert_eq!(
            prove_safety_cancellable(&netlist, "q", 8, &cancellation).unwrap_err(),
            "k-induction proof cancelled"
        );
        assert_eq!(
            prove_safety_expression_cancellable(
                &netlist,
                &BoolExpr::Signal("q".into()),
                8,
                &cancellation
            )
            .unwrap_err(),
            "k-induction proof cancelled"
        );
        assert_eq!(
            check_netlists_equivalent_cancellable(&netlist, &netlist, &cancellation).unwrap_err(),
            "equivalence checking cancelled"
        );
    }

    #[test]
    fn rtl_scan_insertion_adds_and_audits_one_chain() {
        let source = r#"
module top(input clk, d, output reg q1, q2);
always @(posedge clk) begin
  q1 <= d;
  q2 <= q1;
end
endmodule
"#;
        let result = insert_rtl_scan(source, Some("top"), &ScanOptions::default()).unwrap();
        assert_eq!(result.scan.chain.len(), 2);
        assert_eq!(result.dft.sequential_cells, 2);
        assert_eq!(result.dft.scan_cells, 2);
        assert_eq!(result.dft.scan_chains, 1);
        assert!(result.dft.unscanned_sequential.is_empty());
        assert!(result.original_stats.seq > 0);
        assert!(result.scanned_stats.cells > result.original_stats.cells);
        assert!(result.report.contains("DFT scan report"));

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = insert_rtl_scan_cancellable(
            source,
            Some("top"),
            &ScanOptions::default(),
            &cancellation,
        )
        .unwrap_err();
        assert_eq!(error, "RTL scan insertion cancelled");
    }

    #[test]
    fn rtl_cdc_reports_unsynchronized_crossing_and_cancels() {
        let source = r#"
module top(input clk_a, clk_b, d, output reg a_q, b_q);
always @(posedge clk_a) a_q <= d;
always @(posedge clk_b) b_q <= a_q;
endmodule
"#;
        let result = analyze_rtl_cdc(source, Some("top")).unwrap();
        assert_eq!(result.cdc.clocks.len(), 2);
        assert!(!result.cdc.crossings.is_empty());
        assert!(result.cdc.has_errors());
        assert!(!result.cdc.clean());
        assert!(result.report.contains("CDC report"));
        assert!(result.mapped_stats.seq >= 2);

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = analyze_rtl_cdc_cancellable(source, Some("top"), &cancellation).unwrap_err();
        assert_eq!(error, "RTL CDC analysis cancelled");
    }

    #[test]
    fn rtl_fault_analysis_grades_and_exports_stil() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let result = analyze_rtl_faults(source, Some("top"), &PatternOptions::default()).unwrap();
        assert!(result.faults.exhaustive);
        assert_eq!(result.faults.patterns.len(), 4);
        assert_eq!(result.faults.detected_faults, result.faults.total_faults);
        assert!(result.faults.undetected_faults.is_empty());
        assert_eq!(result.faults.coverage_percent, 100.0);
        assert!(result.report.contains("Fault report"));
        assert!(result.stil.contains("STIL 1.0"));
        assert!(result.mapped_stats.lib_cells > 0);

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error = analyze_rtl_faults_cancellable(
            source,
            Some("top"),
            &PatternOptions::default(),
            &cancellation,
        )
        .unwrap_err();
        assert_eq!(error, "RTL fault analysis cancelled");

        let sequential = r#"
module top(input clk, d, output reg q);
always @(posedge clk) q <= d;
endmodule
"#;
        let error =
            analyze_rtl_faults(sequential, Some("top"), &PatternOptions::default()).unwrap_err();
        assert!(
            error.contains("sequential stuck-at grading is unsupported"),
            "{error}"
        );
    }

    #[test]
    fn rtl_simulation_returns_timed_output_vectors() {
        let source = "module top(input a, b, output y); assign y = a & b; endmodule";
        let vectors = vec![
            RtlSimulationVector {
                time_ns: 0,
                inputs: BTreeMap::from([("a".into(), "0".into()), ("b".into(), "1".into())]),
            },
            RtlSimulationVector {
                time_ns: 2,
                inputs: BTreeMap::from([("a".into(), "'b1".into()), ("b".into(), "0x1".into())]),
            },
        ];
        let result = simulate_rtl(source, Some("top"), &vectors).unwrap();
        assert_eq!(result.samples.len(), 2);
        assert_eq!(result.samples[0].outputs["y"], "0");
        assert_eq!(result.samples[1].outputs["y"], "1");
        assert_eq!(result.samples[1].time_ns, 2);
        assert!(!result.finished);

        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let error =
            simulate_rtl_cancellable(source, Some("top"), &vectors, &cancellation).unwrap_err();
        assert_eq!(error, "RTL simulation cancelled");
    }

    #[test]
    fn rtl_simulation_advances_sequential_time() {
        let source = r#"module top(input clk, output reg [1:0] q);
            initial q = 0;
            always @(posedge clk) q <= q + 1;
        endmodule"#;
        let vectors: Vec<_> = [0_u64, 1, 2, 3]
            .into_iter()
            .map(|time_ns| RtlSimulationVector {
                time_ns,
                inputs: BTreeMap::from([("clk".into(), (time_ns % 2).to_string())]),
            })
            .collect();
        let result = simulate_rtl(source, Some("top"), &vectors).unwrap();
        let outputs: Vec<_> = result
            .samples
            .iter()
            .map(|sample| sample.outputs["q"].as_str())
            .collect();
        assert_eq!(outputs, vec!["00", "01", "01", "10"]);
        assert_eq!(result.final_time_ns, 3);
    }
}

/// Options for the placement/routing/RC/STA iteration loop.
#[derive(Clone, Debug)]
pub struct PhysicalTimingFlowOptions {
    pub route: RouteOptions,
    pub interconnect: InterconnectModel,
    /// Optional imported or pre-legalized placement. When absent, the flow
    /// uses deterministic row packing.
    pub initial_placement: Option<Placement>,
    /// Optional timing-driven CTS stage. The flow continues with the post-CTS
    /// netlist and legalized post-CTS placement.
    pub timing_driven_cts: Option<TimingDrivenCtsOptions>,
    /// Optional RC-aware hold-repair stage applied to the baseline route.
    pub hold_repair: Option<HoldRepairOptions>,
    /// Optional SDC constraints. When present, constrained slack drives
    /// physical optimization and best-iteration selection.
    pub constraints: Option<TimingConstraints>,
    /// Waivers applied to each RC-annotated constrained-timing result before
    /// timing-driven placement and routing. Raw violations remain in the
    /// persistent store used by the unified violation APIs.
    pub waivers: Vec<TimingViolationWaiver>,
    /// Number of timing-driven placement/routing iterations after the
    /// baseline physical analysis.
    pub iterations: usize,
    pub placement_passes: usize,
    pub routing_passes: usize,
    /// Insert physical DIODE loads and reroute until antenna warnings clear.
    pub repair_antennas: bool,
    pub antenna_max_diodes_per_net: usize,
}

impl Default for PhysicalTimingFlowOptions {
    fn default() -> Self {
        Self {
            route: RouteOptions::default(),
            interconnect: InterconnectModel::default(),
            initial_placement: None,
            timing_driven_cts: None,
            hold_repair: None,
            constraints: None,
            waivers: Vec::new(),
            iterations: 1,
            placement_passes: 4,
            routing_passes: 8,
            repair_antennas: false,
            antenna_max_diodes_per_net: 4,
        }
    }
}

impl PhysicalTimingFlowOptions {
    pub fn validate(&self) -> Result<(), String> {
        RouteOptions::new(self.route.tracks_per_channel)?;
        self.interconnect.validate()?;
        if self.placement_passes == 0 {
            return Err("physical timing flow requires a positive placement pass count".into());
        }
        if self.routing_passes == 0 {
            return Err("physical timing flow requires a positive routing pass count".into());
        }
        if let Some(cts) = self.timing_driven_cts {
            TimingDrivenCtsOptions::new(
                cts.minimum_branch_factor,
                cts.maximum_branch_factor,
                cts.wire_delay_ns_per_unit,
                cts.target_skew_ns,
            )?;
        }
        if self.hold_repair.is_some() && self.constraints.is_none() {
            return Err("hold-aware physical timing flow requires SDC constraints".into());
        }
        if let Some(hold) = &self.hold_repair {
            hold.validate()?;
        }
        if self.repair_antennas && self.antenna_max_diodes_per_net == 0 {
            return Err(
                "antenna-aware physical timing flow requires a positive diode limit".into(),
            );
        }
        if !self.waivers.is_empty() && self.constraints.is_none() {
            return Err("waiver-aware physical timing flow requires SDC constraints".into());
        }
        for waiver in &self.waivers {
            let mut enabled = TimingViolationStore::new("");
            enabled.add_waiver(waiver.clone()).map_err(|error| {
                format!("invalid physical timing waiver '{}': {}", waiver.id, error)
            })?;
        }
        Ok(())
    }
}

/// One evaluated physical implementation and its RC-annotated timing.
#[derive(Clone, Debug)]
pub struct PhysicalTimingIteration {
    pub index: usize,
    pub placement: Placement,
    pub routing: Routing,
    pub parasitics: Spef,
    pub parasitic_report: ParasiticReport,
    pub sdf: Sdf,
    pub annotated_netlist: Netlist,
    pub timing: StaResult,
    pub constrained_timing: Option<ConstrainedTiming>,
    pub drc: DrcResult,
    pub antenna_repairs: Vec<AntennaRepair>,
    pub antenna_warnings: usize,
    /// Hold repairs applied to this specific physical implementation.
    pub hold_repairs: Vec<HoldRepair>,
    pub hold_repair_applied: bool,
    pub all_active_hold_repaired: bool,
}

#[derive(Clone, Debug)]
pub struct PhysicalTimingFlowResult {
    pub baseline: PhysicalTimingIteration,
    pub optimized: Vec<PhysicalTimingIteration>,
    pub best_index: usize,
    /// Post-synthesis timing-driven CTS result used by every physical
    /// timing iteration.
    pub clock_tree: Option<TimingDrivenCtsResult>,
    /// RC-aware hold repair applied before baseline timing selection.
    pub hold_repair: Option<HoldRepairResult>,
}

impl PhysicalTimingFlowResult {
    pub fn best(&self) -> &PhysicalTimingIteration {
        self.optimized
            .get(self.best_index)
            .unwrap_or(&self.baseline)
    }
}

/// One named timing view used by a multi-corner physical implementation.
///
/// Each scenario may reference a different Liberty/SDF timing view, but all
/// views must currently describe the same physical topology by stable cell
/// and net names. Timing-only differences are allowed; cell names, widths,
/// connectivity, and port topology must match.
#[derive(Clone, Debug)]
pub struct MultiCornerPhysicalTimingScenario<'a> {
    pub name: String,
    pub netlist: &'a Netlist,
    pub constraints: &'a TimingConstraints,
    /// Scenario-local waivers. Raw timing remains in the result; active
    /// waivers only remove checks from the physical-optimization objective.
    pub waivers: Vec<TimingViolationWaiver>,
}

impl<'a> MultiCornerPhysicalTimingScenario<'a> {
    pub fn new(
        name: impl Into<String>,
        netlist: &'a Netlist,
        constraints: &'a TimingConstraints,
        waivers: impl Into<Vec<TimingViolationWaiver>>,
    ) -> Self {
        Self {
            name: name.into(),
            netlist,
            constraints,
            waivers: waivers.into(),
        }
    }
}

/// Options for a common-placement, common-routing MMMC physical flow.
#[derive(Clone, Debug)]
pub struct MultiCornerPhysicalTimingFlowOptions<'a> {
    pub scenarios: Vec<MultiCornerPhysicalTimingScenario<'a>>,
    pub route: RouteOptions,
    pub interconnect: InterconnectModel,
    /// Optional imported or pre-legalized placement. When absent, the flow
    /// uses deterministic row packing.
    pub initial_placement: Option<Placement>,
    /// Optional common timing-driven CTS stage. The branch factor is selected
    /// from the first physical view, then the same tree topology is rebuilt
    /// in every timing scenario.
    pub timing_driven_cts: Option<TimingDrivenCtsOptions>,
    /// Optional common multi-corner hold-repair stage applied before the
    /// baseline physical timing evaluation.
    pub hold_repair: Option<MultiCornerHoldRepairOptions>,
    pub iterations: usize,
    pub placement_passes: usize,
    pub routing_passes: usize,
    /// Insert common physical DIODE loads and reroute if the shared route has
    /// antenna warnings.
    pub repair_antennas: bool,
    pub antenna_max_diodes_per_net: usize,
}

impl Default for MultiCornerPhysicalTimingFlowOptions<'_> {
    fn default() -> Self {
        Self {
            scenarios: Vec::new(),
            route: RouteOptions::default(),
            interconnect: InterconnectModel::default(),
            initial_placement: None,
            timing_driven_cts: None,
            hold_repair: None,
            iterations: 1,
            placement_passes: 4,
            routing_passes: 8,
            repair_antennas: false,
            antenna_max_diodes_per_net: 4,
        }
    }
}

impl MultiCornerPhysicalTimingFlowOptions<'_> {
    pub fn validate(&self) -> Result<(), String> {
        RouteOptions::new(self.route.tracks_per_channel)?;
        self.interconnect.validate()?;
        if let Some(cts) = self.timing_driven_cts {
            TimingDrivenCtsOptions::new(
                cts.minimum_branch_factor,
                cts.maximum_branch_factor,
                cts.wire_delay_ns_per_unit,
                cts.target_skew_ns,
            )?;
        }
        if let Some(hold) = &self.hold_repair {
            hold.validate()?;
        }
        if self.scenarios.is_empty() {
            return Err("multi-corner physical timing flow requires a scenario".into());
        }
        let mut names = std::collections::BTreeSet::new();
        for scenario in &self.scenarios {
            if scenario.name.trim().is_empty() {
                return Err("multi-corner physical timing scenario name cannot be empty".into());
            }
            if !names.insert(scenario.name.as_str()) {
                return Err(format!(
                    "duplicate multi-corner physical timing scenario '{}'",
                    scenario.name
                ));
            }
            for waiver in &scenario.waivers {
                let mut enabled = TimingViolationStore::new("");
                enabled.add_waiver(waiver.clone()).map_err(|error| {
                    format!(
                        "invalid waiver '{}' in physical timing scenario '{}': {}",
                        waiver.id, scenario.name, error
                    )
                })?;
            }
        }
        if self.placement_passes == 0 {
            return Err(
                "multi-corner physical timing flow requires a positive placement pass count".into(),
            );
        }
        if self.routing_passes == 0 {
            return Err(
                "multi-corner physical timing flow requires a positive routing pass count".into(),
            );
        }
        if self.repair_antennas && self.antenna_max_diodes_per_net == 0 {
            return Err(
                "antenna-aware MMMC physical timing flow requires a positive diode limit".into(),
            );
        }
        Ok(())
    }
}

/// One scenario's evaluated physical/timing view.
#[derive(Clone, Debug)]
pub struct MultiCornerPhysicalTimingCorner {
    pub scenario: String,
    pub evaluation: PhysicalTimingIteration,
    /// Raw violations with all scenario waivers installed. Optimization can
    /// query active waivers while callers retain auditable raw timing.
    pub violations: TimingViolationStore,
}

/// One common physical implementation evaluated in every timing scenario.
#[derive(Clone, Debug)]
pub struct MultiCornerPhysicalTimingIteration {
    pub index: usize,
    /// Canonical (first-scenario-indexed) common placement.
    pub placement: Placement,
    /// Canonical (first-scenario-indexed) common routing.
    pub routing: Routing,
    pub corners: Vec<MultiCornerPhysicalTimingCorner>,
    /// Persistent cross-corner registry snapshot for this implementation.
    /// Scenario-local waiver state is preserved in its stores.
    pub registry: TimingViolationRegistry,
    /// Lifecycle change from the previous common implementation. This is
    /// `None` on the baseline snapshot.
    pub registry_update: Option<TimingViolationRegistryUpdate>,
    /// Antenna repairs applied to the common physical implementation.
    pub antenna_repairs: Vec<AntennaRepair>,
    pub antenna_warnings: usize,
    /// Common hold repairs applied to this physical implementation.
    pub hold_repairs: Vec<MultiCornerHoldRepair>,
    pub hold_repair_applied: bool,
    pub all_active_hold_repaired: bool,
    pub worst_slack_corner: usize,
    pub worst_setup_corner: usize,
    pub worst_hold_corner: usize,
    /// Scenario selected for the next optimization pass after active waivers
    /// are removed from the late-path objective. `None` on the baseline.
    pub optimization_corner: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct MultiCornerPhysicalTimingFlowResult {
    pub baseline: MultiCornerPhysicalTimingIteration,
    pub optimized: Vec<MultiCornerPhysicalTimingIteration>,
    pub best_index: usize,
    /// Common CTS search and first-view physical implementation used by every
    /// timing scenario.
    pub clock_tree: Option<TimingDrivenCtsResult>,
    /// Common hold repair applied before baseline timing evaluation.
    pub hold_repair: Option<MultiCornerHoldRepairResult>,
}

impl MultiCornerPhysicalTimingFlowResult {
    pub fn best(&self) -> &MultiCornerPhysicalTimingIteration {
        self.optimized
            .get(self.best_index)
            .unwrap_or(&self.baseline)
    }
}

/// Run the full flow: compile → synthesize (generic + mapped) → formal
/// equivalence → STA → Verilog export.
pub fn run_flow(src: &str, top: Option<&str>) -> Result<Flow, String> {
    run_flow_with_library(src, top, &CellLibrary::std())
}

/// Full RTL flow with cooperative cancellation checked between major stages.
pub fn run_flow_cancellable(
    src: &str,
    top: Option<&str>,
    cancellation: &FlowCancellation,
) -> Result<Flow, String> {
    run_flow_with_library_cancellable(src, top, &CellLibrary::std(), cancellation)
}

/// Full RTL flow using a custom or Liberty-imported standard-cell library.
pub fn run_flow_with_library(
    src: &str,
    top: Option<&str>,
    lib: &CellLibrary,
) -> Result<Flow, String> {
    run_flow_with_library_cancellable(src, top, lib, &FlowCancellation::new())
}

/// Full RTL flow with cooperative cancellation checked between major stages.
pub fn run_flow_with_library_cancellable(
    src: &str,
    top: Option<&str>,
    lib: &CellLibrary,
    cancellation: &FlowCancellation,
) -> Result<Flow, String> {
    run_flow_with_library_options_cancellable(src, top, lib, &RouteOptions::default(), cancellation)
}

/// Full RTL flow with explicit physical-design options (routing capacity).
pub fn run_flow_with_library_options_cancellable(
    src: &str,
    top: Option<&str>,
    lib: &CellLibrary,
    route: &RouteOptions,
    cancellation: &FlowCancellation,
) -> Result<Flow, String> {
    run_flow_staged_cancellable(src, top, lib, route, true, cancellation)
}

/// Staged implementation flow: `include_lec` false skips the (expensive)
/// generic-vs-mapped equivalence proof, for physical-only signoff stages
/// where LEC is tracked as its own separately gated deliverable.
pub fn run_flow_staged_cancellable(
    src: &str,
    top: Option<&str>,
    lib: &CellLibrary,
    route: &RouteOptions,
    include_lec: bool,
    cancellation: &FlowCancellation,
) -> Result<Flow, String> {
    let cancelled = || {
        cancellation
            .is_cancelled()
            .then(|| "RTL flow cancelled".to_string())
    };
    let design = compile(src, top)?;
    if let Some(error) = cancelled() {
        return Err(error);
    }
    let gold = synthesize(
        &design,
        &SynthOptions {
            optimize: false,
            map_lib: false,
        },
    )?
    .netlist;
    if let Some(error) = cancelled() {
        return Err(error);
    }
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), lib)?.netlist;
    if let Some(error) = cancelled() {
        return Err(error);
    }
    let stats = netlist.stats();
    let timing = eda_sta::analyze(&netlist)?;
    if let Some(error) = cancelled() {
        return Err(error);
    }
    let std_lib = Arc::new(lib.clone());
    let verilog = write_verilog(&netlist, std_lib.as_ref());
    let equivalent = if include_lec {
        eda_formal::check_equivalent_cancellable(&gold, &netlist, cancellation)?.equivalent
    } else {
        true
    };
    if let Some(error) = cancelled() {
        return Err(error);
    }
    let placement = eda_pnr::place(&netlist)?;
    if let Some(error) = cancelled() {
        return Err(error);
    }
    let routing =
        eda_pnr::route_with_options_cancellable(&netlist, &placement, route, cancellation)?;
    if let Some(error) = cancelled() {
        return Err(error);
    }
    let drc = eda_pnr::drc(&netlist, &placement, &routing)?;
    if let Some(error) = cancelled() {
        return Err(error);
    }
    let lef = eda_pnr::write_lef(&netlist, &placement)?;
    let def = eda_pnr::write_def(&netlist, &placement, &routing)?;
    let sdf = eda_sdf::write_sdf(&netlist)?;
    if let Some(error) = cancelled() {
        return Err(error);
    }
    Ok(Flow {
        design,
        netlist,
        gold,
        stats,
        timing,
        verilog,
        equivalent,
        placement,
        routing,
        drc,
        lef,
        def,
        sdf,
    })
}

/// Result of an RTL-to-mapped-netlist formal equivalence check.
#[derive(Clone, Debug)]
pub struct RtlEquivalenceResult {
    pub equivalent: bool,
    /// MSB-first binary input assignment that distinguishes the netlists.
    pub counterexample: BTreeMap<String, String>,
    pub notes: Vec<String>,
    pub gold_stats: NetlistStats,
    pub mapped_stats: NetlistStats,
}

/// Compile and synthesize RTL, then formally compare generic and mapped views.
pub fn check_rtl_equivalence(src: &str, top: Option<&str>) -> Result<RtlEquivalenceResult, String> {
    check_rtl_equivalence_cancellable(src, top, &FlowCancellation::new())
}

/// Formal RTL equivalence with cooperative checks between major stages.
pub fn check_rtl_equivalence_cancellable(
    src: &str,
    top: Option<&str>,
    cancellation: &FlowCancellation,
) -> Result<RtlEquivalenceResult, String> {
    let design = compile(src, top)?;
    if cancellation.is_cancelled() {
        return Err("RTL equivalence cancelled".into());
    }
    let gold = synthesize(
        &design,
        &SynthOptions {
            optimize: false,
            map_lib: false,
        },
    )?
    .netlist;
    if cancellation.is_cancelled() {
        return Err("RTL equivalence cancelled".into());
    }
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &CellLibrary::std())?
            .netlist;
    if cancellation.is_cancelled() {
        return Err("RTL equivalence cancelled".into());
    }
    let result = eda_formal::check_equivalent_cancellable(&gold, &netlist, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("RTL equivalence cancelled".into());
    }
    let counterexample = result
        .counterexample
        .unwrap_or_default()
        .iter()
        .map(|(input, value)| (input.clone(), value.to_bin_str()))
        .collect();
    Ok(RtlEquivalenceResult {
        equivalent: result.equivalent,
        counterexample,
        notes: result.notes,
        gold_stats: gold.stats(),
        mapped_stats: netlist.stats(),
    })
}

/// Structured standalone STA summary for a synthesized RTL design.
#[derive(Clone, Debug)]
pub struct RtlTimingResult {
    pub critical_path_delay_ns: f64,
    pub reg2reg_critical_ns: f64,
    pub in2reg_critical_ns: f64,
    pub reg2reg_hold_critical_ns: f64,
    pub in2reg_hold_critical_ns: f64,
    pub disabled_timing_arcs: usize,
    pub case_analysis_nets: BTreeMap<String, bool>,
    pub mapped_stats: NetlistStats,
}

/// Compile and synthesize RTL, then run standalone STA.
pub fn analyze_rtl_timing(src: &str, top: Option<&str>) -> Result<RtlTimingResult, String> {
    analyze_rtl_timing_cancellable(src, top, &FlowCancellation::new())
}

/// Standalone RTL STA with cooperative checks between major stages.
pub fn analyze_rtl_timing_cancellable(
    src: &str,
    top: Option<&str>,
    cancellation: &FlowCancellation,
) -> Result<RtlTimingResult, String> {
    if cancellation.is_cancelled() {
        return Err("RTL timing analysis cancelled".into());
    }
    let design = compile(src, top)?;
    if cancellation.is_cancelled() {
        return Err("RTL timing analysis cancelled".into());
    }
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &CellLibrary::std())?
            .netlist;
    if cancellation.is_cancelled() {
        return Err("RTL timing analysis cancelled".into());
    }
    let timing = eda_sta::analyze(&netlist)?;
    if cancellation.is_cancelled() {
        return Err("RTL timing analysis cancelled".into());
    }
    Ok(RtlTimingResult {
        critical_path_delay_ns: timing.critical_path_delay,
        reg2reg_critical_ns: timing.reg2reg_critical,
        in2reg_critical_ns: timing.in2reg_critical,
        reg2reg_hold_critical_ns: timing.reg2reg_hold_critical,
        in2reg_hold_critical_ns: timing.in2reg_hold_critical,
        disabled_timing_arcs: timing.disabled_timing_arcs,
        case_analysis_nets: timing.case_analysis_nets,
        mapped_stats: netlist.stats(),
    })
}

/// Standalone RTL place-and-route result with physical verification artifacts.
#[derive(Clone, Debug)]
pub struct RtlPnrResult {
    pub placement: Placement,
    pub routing: Routing,
    pub drc: DrcResult,
    pub mapped_stats: NetlistStats,
    pub lef: String,
    pub def: String,
}

/// Compile and synthesize RTL, then place, route, and verify the layout.
pub fn implement_rtl_pnr(src: &str, top: Option<&str>) -> Result<RtlPnrResult, String> {
    implement_rtl_pnr_cancellable(src, top, &FlowCancellation::new())
}

pub fn implement_rtl_pnr_with_drc_options(
    src: &str,
    top: Option<&str>,
    drc_options: &DrcOptions,
) -> Result<RtlPnrResult, String> {
    implement_rtl_pnr_with_drc_options_cancellable(src, top, drc_options, &FlowCancellation::new())
}

/// Standalone RTL PnR with cooperative checks between major stages.
pub fn implement_rtl_pnr_cancellable(
    src: &str,
    top: Option<&str>,
    cancellation: &FlowCancellation,
) -> Result<RtlPnrResult, String> {
    if cancellation.is_cancelled() {
        return Err("RTL PnR cancelled".into());
    }
    let design = compile(src, top)?;
    if cancellation.is_cancelled() {
        return Err("RTL PnR cancelled".into());
    }
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &CellLibrary::std())?
            .netlist;
    if cancellation.is_cancelled() {
        return Err("RTL PnR cancelled".into());
    }
    let placement = eda_pnr::place(&netlist)?;
    if cancellation.is_cancelled() {
        return Err("RTL PnR cancelled".into());
    }
    let routing = eda_pnr::route_with_options_cancellable(
        &netlist,
        &placement,
        &RouteOptions::default(),
        cancellation,
    )?;
    if cancellation.is_cancelled() {
        return Err("RTL PnR cancelled".into());
    }
    let drc = eda_pnr::drc(&netlist, &placement, &routing)?;
    if cancellation.is_cancelled() {
        return Err("RTL PnR cancelled".into());
    }
    let lef = eda_pnr::write_lef(&netlist, &placement)?;
    let def = eda_pnr::write_def(&netlist, &placement, &routing)?;
    if cancellation.is_cancelled() {
        return Err("RTL PnR cancelled".into());
    }
    Ok(RtlPnrResult {
        mapped_stats: netlist.stats(),
        placement,
        routing,
        drc,
        lef,
        def,
    })
}

/// Standalone RTL PnR with a foundry-specific physical verification rule.
pub fn implement_rtl_pnr_with_drc_options_cancellable(
    src: &str,
    top: Option<&str>,
    drc_options: &DrcOptions,
    cancellation: &FlowCancellation,
) -> Result<RtlPnrResult, String> {
    if cancellation.is_cancelled() {
        return Err("RTL PnR cancelled".into());
    }
    let design = compile(src, top)?;
    if cancellation.is_cancelled() {
        return Err("RTL PnR cancelled".into());
    }
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &CellLibrary::std())?
            .netlist;
    if cancellation.is_cancelled() {
        return Err("RTL PnR cancelled".into());
    }
    let placement = eda_pnr::place(&netlist)?;
    if cancellation.is_cancelled() {
        return Err("RTL PnR cancelled".into());
    }
    let routing = eda_pnr::route_with_options_cancellable(
        &netlist,
        &placement,
        &RouteOptions::default(),
        cancellation,
    )?;
    if cancellation.is_cancelled() {
        return Err("RTL PnR cancelled".into());
    }
    let drc = physical_verification_with_options(&netlist, &placement, &routing, drc_options)?;
    if cancellation.is_cancelled() {
        return Err("RTL PnR cancelled".into());
    }
    let lef = eda_pnr::write_lef(&netlist, &placement)?;
    let def = eda_pnr::write_def(&netlist, &placement, &routing)?;
    if cancellation.is_cancelled() {
        return Err("RTL PnR cancelled".into());
    }
    Ok(RtlPnrResult {
        mapped_stats: netlist.stats(),
        placement,
        routing,
        drc,
        lef,
        def,
    })
}

/// Standalone RTL power-analysis result.
#[derive(Clone, Debug)]
pub struct RtlPowerResult {
    pub report: PowerReport,
    pub mapped_stats: NetlistStats,
}

/// Compile and synthesize RTL, then estimate switching and leakage power.
pub fn analyze_rtl_power(
    src: &str,
    top: Option<&str>,
    vectors: &[RtlSimulationVector],
    options: &PowerOptions,
) -> Result<RtlPowerResult, String> {
    analyze_rtl_power_cancellable(src, top, vectors, options, &FlowCancellation::new())
}

/// Standalone RTL power analysis with cooperative vector/stage checks.
pub fn analyze_rtl_power_cancellable(
    src: &str,
    top: Option<&str>,
    vectors: &[RtlSimulationVector],
    options: &PowerOptions,
    cancellation: &FlowCancellation,
) -> Result<RtlPowerResult, String> {
    if cancellation.is_cancelled() {
        return Err("RTL power analysis cancelled".into());
    }
    let design = compile(src, top)?;
    if cancellation.is_cancelled() {
        return Err("RTL power analysis cancelled".into());
    }
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &CellLibrary::std())?
            .netlist;
    if cancellation.is_cancelled() {
        return Err("RTL power analysis cancelled".into());
    }
    let mut power_vectors = Vec::with_capacity(vectors.len());
    for vector in vectors {
        if cancellation.is_cancelled() {
            return Err("RTL power analysis cancelled".into());
        }
        let mut power_vector = InputVector::new();
        for (name, value) in &vector.inputs {
            let port = design
                .port(name)
                .ok_or_else(|| format!("unknown power input '{name}'"))?;
            let value = parse_power_literal(port.width, value).ok_or_else(|| {
                format!("invalid {}-bit value '{value}' for '{name}'", port.width)
            })?;
            power_vector.insert(name.clone(), value);
        }
        power_vectors.push(power_vector);
    }
    let report = eda_power::analyze(&netlist, &power_vectors, options)?;
    if cancellation.is_cancelled() {
        return Err("RTL power analysis cancelled".into());
    }
    Ok(RtlPowerResult {
        report,
        mapped_stats: netlist.stats(),
    })
}

/// Standalone RTL power-grid result.
#[derive(Clone, Debug)]
pub struct RtlIrResult {
    pub power: PowerReport,
    pub ir: IrResult,
    pub placement: Placement,
    pub mapped_stats: NetlistStats,
}

/// Compile, synthesize, place, and analyze RTL static IR drop and rail EM.
pub fn analyze_rtl_ir(
    src: &str,
    top: Option<&str>,
    vectors: &[RtlSimulationVector],
    power_options: &PowerOptions,
    ir_options: &IrOptions,
) -> Result<RtlIrResult, String> {
    analyze_rtl_ir_cancellable(
        src,
        top,
        vectors,
        power_options,
        ir_options,
        &FlowCancellation::new(),
    )
}

/// Standalone RTL IR/EM analysis with cooperative vector/stage checks.
pub fn analyze_rtl_ir_cancellable(
    src: &str,
    top: Option<&str>,
    vectors: &[RtlSimulationVector],
    power_options: &PowerOptions,
    ir_options: &IrOptions,
    cancellation: &FlowCancellation,
) -> Result<RtlIrResult, String> {
    if cancellation.is_cancelled() {
        return Err("RTL IR analysis cancelled".into());
    }
    let design = compile(src, top)?;
    if cancellation.is_cancelled() {
        return Err("RTL IR analysis cancelled".into());
    }
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &CellLibrary::std())?
            .netlist;
    if cancellation.is_cancelled() {
        return Err("RTL IR analysis cancelled".into());
    }
    let placement = eda_pnr::place(&netlist)?;
    if cancellation.is_cancelled() {
        return Err("RTL IR analysis cancelled".into());
    }
    let mut power_vectors = Vec::with_capacity(vectors.len());
    for vector in vectors {
        if cancellation.is_cancelled() {
            return Err("RTL IR analysis cancelled".into());
        }
        let mut power_vector = InputVector::new();
        for (name, value) in &vector.inputs {
            let port = design
                .port(name)
                .ok_or_else(|| format!("unknown IR input '{name}'"))?;
            let value = parse_power_literal(port.width, value).ok_or_else(|| {
                format!("invalid {}-bit value '{value}' for '{name}'", port.width)
            })?;
            power_vector.insert(name.clone(), value);
        }
        power_vectors.push(power_vector);
    }
    if power_vectors.is_empty() {
        return Err("RTL IR analysis requires at least one input vector".into());
    }
    let power = eda_power::analyze(&netlist, &power_vectors, power_options)?;
    if cancellation.is_cancelled() {
        return Err("RTL IR analysis cancelled".into());
    }
    let ir = eda_ir::analyze(&netlist, &placement, &power, ir_options)?;
    if cancellation.is_cancelled() {
        return Err("RTL IR analysis cancelled".into());
    }
    Ok(RtlIrResult {
        power,
        ir,
        placement,
        mapped_stats: netlist.stats(),
    })
}

/// Standalone RTL temporal-property BMC result.
#[derive(Clone, Debug)]
pub struct RtlPropertyBmcResult {
    pub results: Vec<PropertyBmcResult>,
    pub assumptions: Vec<String>,
    pub mapped_stats: NetlistStats,
    pub report: String,
}

/// Compile and synthesize RTL, then boundedly check SVA assert/cover targets.
pub fn check_rtl_properties(
    src: &str,
    top: Option<&str>,
    properties: &str,
    depth: usize,
) -> Result<RtlPropertyBmcResult, String> {
    check_rtl_properties_cancellable(src, top, properties, depth, &FlowCancellation::new())
}

/// Standalone RTL property BMC with cooperative major-stage checks.
pub fn check_rtl_properties_cancellable(
    src: &str,
    top: Option<&str>,
    properties: &str,
    depth: usize,
    cancellation: &FlowCancellation,
) -> Result<RtlPropertyBmcResult, String> {
    if cancellation.is_cancelled() {
        return Err("RTL property BMC cancelled".into());
    }
    if depth == 0 {
        return Err("RTL property BMC requires a positive depth".into());
    }
    let parsed = parse_sva(properties)?;
    let (assertions, assumptions): (Vec<_>, Vec<_>) = parsed
        .into_iter()
        .partition(|property| property.kind != AssertionKind::Assume);
    if assertions.is_empty() {
        return Err("SVA input contains assumptions but no assert or cover targets".into());
    }
    let assumption_names = assumptions
        .iter()
        .map(|assumption| assumption.name.clone())
        .collect::<Vec<_>>();
    if cancellation.is_cancelled() {
        return Err("RTL property BMC cancelled".into());
    }
    let design = compile(src, top)?;
    if cancellation.is_cancelled() {
        return Err("RTL property BMC cancelled".into());
    }
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &CellLibrary::std())?
            .netlist;
    if cancellation.is_cancelled() {
        return Err("RTL property BMC cancelled".into());
    }
    let results = eda_formal::bmc_assertions_with_assumptions_cancellable(
        &netlist,
        &assertions,
        &assumptions,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        cancellation,
    )?;
    if cancellation.is_cancelled() {
        return Err("RTL property BMC cancelled".into());
    }
    let mut report = assumption_names
        .iter()
        .map(|name| format!("Temporal BMC report '{name}': ASSUMPTION CONSTRAINT applied\n"))
        .collect::<String>();
    report.push_str(&results.iter().map(property_bmc_report).collect::<String>());
    Ok(RtlPropertyBmcResult {
        results,
        assumptions: assumption_names,
        mapped_stats: netlist.stats(),
        report,
    })
}

/// Standalone RTL scan-insertion result.
#[derive(Clone, Debug)]
pub struct RtlScanResult {
    pub scan: ScanChainResult,
    pub dft: DftReport,
    pub original_stats: NetlistStats,
    pub scanned_stats: NetlistStats,
    pub report: String,
}

/// Compile and synthesize RTL, then insert and audit one scan chain.
pub fn insert_rtl_scan(
    src: &str,
    top: Option<&str>,
    options: &ScanOptions,
) -> Result<RtlScanResult, String> {
    insert_rtl_scan_cancellable(src, top, options, &FlowCancellation::new())
}

/// Standalone RTL DFT insertion with cooperative major-stage checks.
pub fn insert_rtl_scan_cancellable(
    src: &str,
    top: Option<&str>,
    options: &ScanOptions,
    cancellation: &FlowCancellation,
) -> Result<RtlScanResult, String> {
    if cancellation.is_cancelled() {
        return Err("RTL scan insertion cancelled".into());
    }
    let design = compile(src, top)?;
    if cancellation.is_cancelled() {
        return Err("RTL scan insertion cancelled".into());
    }
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &CellLibrary::std())?
            .netlist;
    if cancellation.is_cancelled() {
        return Err("RTL scan insertion cancelled".into());
    }
    let original_stats = netlist.stats();
    let scan = eda_dft::insert_scan_chain(&netlist, options)?;
    if cancellation.is_cancelled() {
        return Err("RTL scan insertion cancelled".into());
    }
    let dft = eda_dft::analyze_with_ports(
        &scan.netlist,
        &options.scan_en_port,
        &options.scan_in_port,
        &options.scan_out_port,
    )?;
    if cancellation.is_cancelled() {
        return Err("RTL scan insertion cancelled".into());
    }
    let mut report = eda_dft::report(&scan);
    report.push_str(&format!(
        "  sequential={} scanned={} chains={} combinational={}\n",
        dft.sequential_cells, dft.scan_cells, dft.scan_chains, dft.combinational_cells
    ));
    let scanned_stats = scan.netlist.stats();
    Ok(RtlScanResult {
        scan,
        dft,
        original_stats,
        scanned_stats,
        report,
    })
}

/// Standalone RTL CDC-analysis result.
#[derive(Clone, Debug)]
pub struct RtlCdcResult {
    pub cdc: CdcResult,
    pub mapped_stats: NetlistStats,
    pub report: String,
}

/// Compile and synthesize RTL, then infer and audit clock-domain crossings.
pub fn analyze_rtl_cdc(src: &str, top: Option<&str>) -> Result<RtlCdcResult, String> {
    analyze_rtl_cdc_cancellable(src, top, &FlowCancellation::new())
}

/// Standalone RTL CDC analysis with cooperative major-stage checks.
pub fn analyze_rtl_cdc_cancellable(
    src: &str,
    top: Option<&str>,
    cancellation: &FlowCancellation,
) -> Result<RtlCdcResult, String> {
    if cancellation.is_cancelled() {
        return Err("RTL CDC analysis cancelled".into());
    }
    let design = compile(src, top)?;
    if cancellation.is_cancelled() {
        return Err("RTL CDC analysis cancelled".into());
    }
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &CellLibrary::std())?
            .netlist;
    if cancellation.is_cancelled() {
        return Err("RTL CDC analysis cancelled".into());
    }
    let cdc = eda_cdc::analyze(&netlist)?;
    if cancellation.is_cancelled() {
        return Err("RTL CDC analysis cancelled".into());
    }
    Ok(RtlCdcResult {
        report: eda_cdc::report(&cdc),
        mapped_stats: netlist.stats(),
        cdc,
    })
}

/// Standalone RTL stuck-at fault-analysis result.
#[derive(Clone, Debug)]
pub struct RtlFaultResult {
    pub faults: FaultCoverageResult,
    pub stil: String,
    pub mapped_stats: NetlistStats,
    pub report: String,
}

/// Compile and synthesize combinational RTL, then grade stuck-at faults.
pub fn analyze_rtl_faults(
    src: &str,
    top: Option<&str>,
    options: &PatternOptions,
) -> Result<RtlFaultResult, String> {
    analyze_rtl_faults_cancellable(src, top, options, &FlowCancellation::new())
}

/// Standalone RTL fault grading with cooperative major-stage checks.
pub fn analyze_rtl_faults_cancellable(
    src: &str,
    top: Option<&str>,
    options: &PatternOptions,
    cancellation: &FlowCancellation,
) -> Result<RtlFaultResult, String> {
    if cancellation.is_cancelled() {
        return Err("RTL fault analysis cancelled".into());
    }
    let design = compile(src, top)?;
    if cancellation.is_cancelled() {
        return Err("RTL fault analysis cancelled".into());
    }
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &CellLibrary::std())?
            .netlist;
    if cancellation.is_cancelled() {
        return Err("RTL fault analysis cancelled".into());
    }
    let faults = eda_fault::grade_automatic(&netlist, options)?;
    if cancellation.is_cancelled() {
        return Err("RTL fault analysis cancelled".into());
    }
    let stil = eda_fault::write_stil(&netlist, &faults.patterns, "rtl_faults")?;
    if cancellation.is_cancelled() {
        return Err("RTL fault analysis cancelled".into());
    }
    Ok(RtlFaultResult {
        report: eda_fault::report(&faults),
        mapped_stats: netlist.stats(),
        faults,
        stil,
    })
}

/// Parse a scalar/hex/binary literal, tolerating a Verilog sized-literal
/// prefix such as `512'h...`.
fn parse_power_literal(width: usize, text: &str) -> Option<LogicWord> {
    let trimmed = text.trim();
    if let Some((size, rest)) = trimmed.split_once('\'') {
        let size: usize = size.trim().parse().ok()?;
        if size != width {
            return None;
        }
        // rest 形如 "h..." / "b..." / "d..."：重组为 LogicWord 可解析的
        // `'h` 前缀形式。
        return LogicWord::parse(width, &format!("'{rest}"));
    }
    LogicWord::parse(width, trimmed)
}

/// One timed RTL simulation stimulus vector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtlSimulationVector {
    pub time_ns: u64,
    /// Input port name to decimal, binary, hexadecimal, or Verilog literal.
    pub inputs: BTreeMap<String, String>,
}

/// Output and $display state captured after one stimulus vector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtlSimulationSample {
    pub time_ns: u64,
    pub outputs: BTreeMap<String, String>,
    pub displays: Vec<String>,
}

/// Result of a deterministic agent-driven RTL simulation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtlSimulationResult {
    pub samples: Vec<RtlSimulationSample>,
    pub finished: bool,
    pub final_time_ns: u64,
}

/// Simulate timed RTL input vectors and return every output port.
pub fn simulate_rtl(
    src: &str,
    top: Option<&str>,
    vectors: &[RtlSimulationVector],
) -> Result<RtlSimulationResult, String> {
    simulate_rtl_cancellable(src, top, vectors, &FlowCancellation::new())
}

/// Simulate RTL with cooperative checks between stimulus vectors.
pub fn simulate_rtl_cancellable(
    src: &str,
    top: Option<&str>,
    vectors: &[RtlSimulationVector],
    cancellation: &FlowCancellation,
) -> Result<RtlSimulationResult, String> {
    if cancellation.is_cancelled() {
        return Err("RTL simulation cancelled".into());
    }
    let design = compile(src, top)?;
    let mut simulator = Simulator::new(&design)?;
    simulator.set_cancellation(cancellation);
    let mut samples = Vec::with_capacity(vectors.len());
    let mut displays = Vec::new();
    let mut final_time_ns = 0_u64;
    for vector in vectors {
        if cancellation.is_cancelled() {
            return Err("RTL simulation cancelled".into());
        }
        if vector.time_ns < final_time_ns {
            return Err(format!(
                "RTL simulation time moves backward at {} after {}",
                vector.time_ns, final_time_ns
            ));
        }
        for (name, value) in &vector.inputs {
            let width = design
                .port(name)
                .ok_or_else(|| format!("unknown simulation input '{name}'"))?
                .width;
            let value = LogicWord::parse(width, value)
                .ok_or_else(|| format!("invalid {width}-bit value '{value}' for input '{name}'"))?;
            simulator.set_input(name, &value)?;
        }
        simulator.run_until(vector.time_ns)?;
        final_time_ns = simulator.now();
        let outputs = design
            .ports
            .iter()
            .filter(|port| port.dir == eda_core::ir::Dir::Out)
            .map(|port| {
                let value = simulator
                    .read_port(&port.name)
                    .ok_or_else(|| format!("missing simulation output '{}'", port.name))?;
                Ok((port.name.clone(), value.to_bin_str()))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        displays.extend(simulator.take_displays());
        samples.push(RtlSimulationSample {
            time_ns: final_time_ns,
            outputs,
            displays: displays.clone(),
        });
    }
    let finished = simulator.finished();
    displays.extend(simulator.take_displays());
    if let Some(last) = samples.last_mut() {
        last.displays = displays.clone();
    }
    Ok(RtlSimulationResult {
        samples,
        finished,
        final_time_ns,
    })
}

/// Place and globally route a synthesized netlist.
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

fn evaluate_physical_timing_iteration_cancellable(
    index: usize,
    netlist: &Netlist,
    placement: Placement,
    routing: Routing,
    model: &InterconnectModel,
    constraints: Option<&TimingConstraints>,
    cancellation: &FlowCancellation,
) -> Result<PhysicalTimingIteration, String> {
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let parasitics = extract_interconnect_parasitics(netlist, &routing, model)?;
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let parasitic_report = parasitic_analysis(netlist, &parasitics)?;
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let sdf_text = parasitics_to_sdf(netlist, &parasitics)?;
    let sdf = parse_delay_file(&sdf_text)?;
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let annotated_netlist = back_annotate(netlist, &sdf)?;
    let timing = eda_sta::analyze(&annotated_netlist)?;
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let constrained_timing = constraints
        .map(|constraints| constrained_timing(&annotated_netlist, constraints))
        .transpose()?;
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let drc = physical_verification(netlist, &placement, &routing)?;
    let antenna_warnings = drc
        .violations
        .iter()
        .filter(|violation| violation.code == "ANT-001")
        .count();
    if drc.has_errors() {
        return Err(format!(
            "physical timing iteration {} has DRC errors: {:?}",
            index, drc.violations
        ));
    }
    Ok(PhysicalTimingIteration {
        index,
        placement,
        routing,
        parasitics,
        parasitic_report,
        sdf,
        annotated_netlist,
        timing,
        constrained_timing,
        drc,
        antenna_repairs: Vec::new(),
        antenna_warnings,
        hold_repairs: Vec::new(),
        hold_repair_applied: false,
        all_active_hold_repaired: true,
    })
}

pub(crate) fn repair_physical_timing_antennas_cancellable(
    iteration: PhysicalTimingIteration,
    index: usize,
    options: &PhysicalTimingFlowOptions,
    cancellation: &FlowCancellation,
) -> Result<PhysicalTimingIteration, String> {
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    if !options.repair_antennas || iteration.antenna_warnings == 0 {
        return Ok(iteration);
    }
    let patch_name = if index == usize::MAX {
        "physical_antenna_baseline".to_string()
    } else {
        format!("physical_antenna_{index}")
    };
    let repaired = repair_antennas(
        &iteration.annotated_netlist,
        &iteration.placement,
        &iteration.routing,
        &AntennaRepairOptions {
            patch_name,
            max_diodes_per_net: options.antenna_max_diodes_per_net,
            route: options.route.clone(),
        },
    )?;
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let mut evaluated = evaluate_physical_timing_iteration_cancellable(
        index,
        &repaired.netlist,
        repaired.physical_placement,
        repaired.routing,
        &options.interconnect,
        options.constraints.as_ref(),
        cancellation,
    )?;
    evaluated.antenna_repairs = repaired.repairs;
    evaluated.antenna_warnings = repaired.antenna_warnings;
    Ok(evaluated)
}

fn repair_physical_timing_hold_cancellable(
    iteration: PhysicalTimingIteration,
    options: &PhysicalTimingFlowOptions,
    patch_name: Option<&str>,
    cancellation: &FlowCancellation,
) -> Result<(PhysicalTimingIteration, HoldRepairResult), String> {
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let hold = options
        .hold_repair
        .as_ref()
        .ok_or_else(|| "physical timing hold repair is disabled".to_string())?;
    let constraints = options
        .constraints
        .as_ref()
        .ok_or_else(|| "physical timing hold repair requires SDC constraints".to_string())?;
    let mut current = iteration;
    let antenna_repairs = current.antenna_repairs.clone();
    let patch_name = patch_name.unwrap_or(hold.patch_name.as_str());
    let mut old_slacks = std::collections::BTreeMap::new();
    let mut inserted = std::collections::BTreeMap::new();

    loop {
        if cancellation.is_cancelled() {
            return Err("physical timing analysis cancelled".into());
        }
        let constrained = current
            .constrained_timing
            .clone()
            .ok_or_else(|| "physical timing hold repair requires constrained timing".to_string())?;
        let mut store =
            timing_violation_store(&constrained.clock, &current.annotated_netlist, &constrained)?;
        for waiver in &options.waivers {
            add_timing_violation_waiver(&mut store, waiver.clone())?;
        }
        for waiver in &hold.waivers {
            add_timing_violation_waiver(&mut store, waiver.clone())?;
        }
        let waived = store.waived_violations().clone();
        let mut repaired_this_round = false;
        for path in constrained_hold_timing_paths(&current.annotated_netlist, &constrained, 0) {
            if cancellation.is_cancelled() {
                return Err("physical timing analysis cancelled".into());
            }
            let TimingPathEndpoint::SequentialCell(endpoint) = &path.path.path.endpoint else {
                continue;
            };
            if path.slack_ns >= 0.0
                || !path.slack_ns.is_finite()
                || waived.contains_key(&format!("{}:hold:{}", constrained.clock, endpoint))
            {
                continue;
            }
            old_slacks.entry(endpoint.clone()).or_insert(path.slack_ns);
            let count = inserted.get(endpoint).copied().unwrap_or(0);
            if count >= hold.max_buffers_per_endpoint {
                continue;
            }
            let mut netlist = current.annotated_netlist.clone();
            insert_hold_buffer(&mut netlist, endpoint, patch_name, count)?;
            let placement = extend_physical_placement(&netlist, &current.placement)?;
            let routing = eda_pnr::route_with_options_cancellable(
                &netlist,
                &placement,
                &options.route,
                cancellation,
            )?;
            current = evaluate_physical_timing_iteration_cancellable(
                current.index,
                &netlist,
                placement,
                routing,
                &options.interconnect,
                Some(constraints),
                cancellation,
            )?;
            current.antenna_repairs = antenna_repairs.clone();
            inserted.insert(endpoint.clone(), count + 1);
            repaired_this_round = true;
        }
        if !repaired_this_round {
            break;
        }
    }

    let final_timing = current
        .constrained_timing
        .as_ref()
        .ok_or_else(|| "physical timing hold repair lost constrained timing".to_string())?;
    let final_paths = constrained_hold_timing_paths(&current.annotated_netlist, final_timing, 0);
    let mut repairs = Vec::new();
    for (endpoint, old_slack) in old_slacks {
        let buffers = inserted.get(&endpoint).copied().unwrap_or(0);
        let new_slack = final_paths
            .iter()
            .find(|path| {
                matches!(
                    &path.path.path.endpoint,
                    TimingPathEndpoint::SequentialCell(name) if *name == endpoint
                )
            })
            .map(|path| path.slack_ns)
            .unwrap_or(f64::INFINITY);
        repairs.push(HoldRepair {
            endpoint: endpoint.clone(),
            endpoint_pin: 1,
            old_slack_ns: old_slack,
            new_slack_ns: new_slack,
            buffers_inserted: buffers,
        });
    }

    let mut store = timing_violation_store(
        &final_timing.clock,
        &current.annotated_netlist,
        final_timing,
    )?;
    for waiver in &options.waivers {
        add_timing_violation_waiver(&mut store, waiver.clone())?;
    }
    for waiver in &hold.waivers {
        add_timing_violation_waiver(&mut store, waiver.clone())?;
    }
    let all_active_repaired = !final_paths.into_iter().any(|path| {
        path.slack_ns < 0.0
            && path.slack_ns.is_finite()
            && !store.waived_violations().contains_key(&format!(
                "{}:hold:{}",
                final_timing.clock,
                endpoint_name(&path.path.path.endpoint)
            ))
    });
    let buffers_inserted = inserted.values().sum();
    let result = HoldRepairResult {
        netlist: current.annotated_netlist.clone(),
        physical_placement: Some(current.placement.clone()),
        repairs,
        buffers_inserted,
        all_active_violations_repaired: all_active_repaired,
    };
    current.antenna_repairs = antenna_repairs;
    Ok((current, result))
}

struct MultiCornerAntennaRepairState {
    scenario_netlists: Vec<Netlist>,
    placement: Placement,
    routing: Routing,
    repairs: Vec<AntennaRepair>,
    antenna_warnings: usize,
}

fn propagate_common_antenna_repair(
    original_owner: &Netlist,
    repaired_owner: &Netlist,
    scenarios: &[MultiCornerPhysicalTimingScenario<'_>],
) -> Result<Vec<Netlist>, String> {
    let original_names = original_owner
        .cells()
        .iter()
        .map(|cell| cell.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut out = Vec::with_capacity(scenarios.len());
    for scenario in scenarios {
        let mut view = scenario.netlist.clone();
        let target_diode = view
            .library()
            .and_then(|library| library.find("DIODE"))
            .ok_or_else(|| {
                format!(
                    "MMMC antenna repair scenario '{}' has no library DIODE cell",
                    scenario.name
                )
            })?;
        for repaired_cell in repaired_owner.cells() {
            if original_names.contains(repaired_cell.name.as_str()) {
                continue;
            }
            if (0..view.cell_count()).any(|cell| view.cell(cell).name == repaired_cell.name) {
                return Err(format!(
                    "MMMC antenna repair cell '{}' already exists in scenario '{}'",
                    repaired_cell.name, scenario.name
                ));
            }
            let source_kind = repaired_owner
                .library()
                .and_then(|library| {
                    let eda_core::netlist::CellKind::Lib(id) = &repaired_cell.kind else {
                        return None;
                    };
                    Some(library.get(*id).name.clone())
                })
                .unwrap_or_else(|| "GENERIC".to_string());
            if source_kind != "DIODE" {
                return Err(format!(
                    "MMMC antenna repair attempted to propagate non-DIODE cell '{}'",
                    repaired_cell.name
                ));
            }
            let inputs = repaired_cell
                .inputs
                .iter()
                .map(|net| {
                    let name = repaired_owner.net(*net).name.clone();
                    view.net_by_name(&name).ok_or_else(|| {
                        format!(
                            "net '{}' is missing from scenario '{}'",
                            name, scenario.name
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let output_name = repaired_owner.net(repaired_cell.output).name.clone();
            let output = match view.net_by_name(&output_name) {
                Some(net) => net,
                None => view.add_net(output_name),
            };
            view.add_cell(eda_core::netlist::Cell {
                name: repaired_cell.name.clone(),
                kind: eda_core::netlist::CellKind::Lib(target_diode),
                inputs,
                output,
            });
        }
        out.push(view);
    }
    Ok(out)
}

fn repair_multicorner_antennas(
    scenarios: &[MultiCornerPhysicalTimingScenario<'_>],
    geometry_owner: &Netlist,
    placement: &Placement,
    routing: &Routing,
    options: &MultiCornerPhysicalTimingFlowOptions<'_>,
    index: usize,
) -> Result<Option<MultiCornerAntennaRepairState>, String> {
    if !options.repair_antennas {
        return Ok(None);
    }
    let drc = physical_verification(geometry_owner, placement, routing)?;
    let warnings = drc
        .violations
        .iter()
        .filter(|violation| violation.code == "ANT-001")
        .count();
    if warnings == 0 {
        return Ok(None);
    }
    let patch_name = if index == usize::MAX {
        "mmmc_antenna_baseline".to_string()
    } else {
        format!("mmmc_antenna_{index}")
    };
    let repaired = repair_antennas(
        geometry_owner,
        placement,
        routing,
        &AntennaRepairOptions {
            patch_name,
            max_diodes_per_net: options.antenna_max_diodes_per_net,
            route: options.route.clone(),
        },
    )?;
    let scenario_netlists =
        propagate_common_antenna_repair(geometry_owner, &repaired.netlist, scenarios)?;
    Ok(Some(MultiCornerAntennaRepairState {
        scenario_netlists,
        placement: repaired.physical_placement,
        routing: repaired.routing,
        repairs: repaired.repairs,
        antenna_warnings: repaired.antenna_warnings,
    }))
}

type NameIdMap = std::collections::HashMap<String, usize>;

fn physical_name_maps(nl: &Netlist, scenario: &str) -> Result<(NameIdMap, NameIdMap), String> {
    let mut cells = NameIdMap::new();
    for (cell_id, cell) in nl.cells().iter().enumerate() {
        if cell.name.trim().is_empty() {
            return Err(format!(
                "physical timing scenario '{}' has an empty cell name",
                scenario
            ));
        }
        if cells.insert(cell.name.clone(), cell_id).is_some() {
            return Err(format!(
                "physical timing scenario '{}' has duplicate cell '{}'",
                scenario, cell.name
            ));
        }
    }
    let mut nets = NameIdMap::new();
    for (net_id, net) in nl.nets().iter().enumerate() {
        if net.name.trim().is_empty() {
            return Err(format!(
                "physical timing scenario '{}' has an empty net name",
                scenario
            ));
        }
        if nets.insert(net.name.clone(), net_id).is_some() {
            return Err(format!(
                "physical timing scenario '{}' has duplicate net '{}'",
                scenario, net.name
            ));
        }
    }
    Ok((cells, nets))
}

fn library_cell_identity(nl: &Netlist, cell_id: usize, scenario: &str) -> Result<String, String> {
    let cell = nl.cell(cell_id);
    let eda_core::netlist::CellKind::Lib(library_cell) = &cell.kind else {
        return Ok(format!("{:?}", cell.kind));
    };
    let library = nl.library().ok_or_else(|| {
        format!(
            "physical timing scenario '{}' has library cell '{}' without a library",
            scenario, cell.name
        )
    })?;
    let definition = library.get(*library_cell);
    Ok(format!(
        "{}|{}|{}",
        definition.name,
        definition.inputs.join(","),
        definition.output
    ))
}

fn check_common_physical_topology(
    reference: &Netlist,
    reference_placement: &Placement,
    other: &Netlist,
    other_placement: &Placement,
    scenario: &str,
) -> Result<(), String> {
    let (reference_cells, reference_nets) = physical_name_maps(reference, "reference")?;
    let (other_cells, other_nets) = physical_name_maps(other, scenario)?;
    if reference_cells.len() != other_cells.len() || reference_nets.len() != other_nets.len() {
        return Err(format!(
            "physical timing scenario '{}' topology differs: {} cells / {} nets versus {} / {}",
            scenario,
            reference_cells.len(),
            reference_nets.len(),
            other_cells.len(),
            other_nets.len()
        ));
    }
    if reference_cells
        .keys()
        .any(|name| !other_cells.contains_key(name))
        || reference_nets
            .keys()
            .any(|name| !other_nets.contains_key(name))
    {
        return Err(format!(
            "physical timing scenario '{}' has different stable cell/net names",
            scenario
        ));
    }
    if reference.ports.len() != other.ports.len() {
        return Err(format!(
            "physical timing scenario '{}' has a different port count",
            scenario
        ));
    }
    for (reference_port, other_port) in reference.ports.iter().zip(other.ports.iter()) {
        let reference_bits = reference_port
            .bits
            .iter()
            .map(|net| reference.net(*net).name.as_str())
            .collect::<Vec<_>>();
        let other_bits = other_port
            .bits
            .iter()
            .map(|net| other.net(*net).name.as_str())
            .collect::<Vec<_>>();
        if reference_port.name != other_port.name
            || reference_port.dir != other_port.dir
            || reference_bits != other_bits
        {
            return Err(format!(
                "physical timing scenario '{}' port '{}' differs",
                scenario, reference_port.name
            ));
        }
    }
    for (name, reference_cell_id) in &reference_cells {
        let other_cell_id = other_cells[name];
        let reference_cell = reference.cell(*reference_cell_id);
        let other_cell = other.cell(other_cell_id);
        if reference_placement.cell_width[*reference_cell_id]
            != other_placement.cell_width[other_cell_id]
        {
            return Err(format!(
                "physical timing scenario '{}' cell '{}' width differs: {} versus {} sites",
                scenario,
                name,
                reference_placement.cell_width[*reference_cell_id],
                other_placement.cell_width[other_cell_id]
            ));
        }
        if library_cell_identity(reference, *reference_cell_id, "reference")?
            != library_cell_identity(other, other_cell_id, scenario)?
        {
            return Err(format!(
                "physical timing scenario '{}' cell '{}' has a different physical cell type",
                scenario, name
            ));
        }
        if reference.net(reference_cell.output).name != other.net(other_cell.output).name
            || reference_cell
                .inputs
                .iter()
                .map(|net| reference.net(*net).name.as_str())
                .collect::<Vec<_>>()
                != other_cell
                    .inputs
                    .iter()
                    .map(|net| other.net(*net).name.as_str())
                    .collect::<Vec<_>>()
        {
            return Err(format!(
                "physical timing scenario '{}' cell '{}' connectivity differs",
                scenario, name
            ));
        }
    }
    Ok(())
}

fn validate_common_physical_topology(
    scenarios: &[MultiCornerPhysicalTimingScenario<'_>],
) -> Result<(), String> {
    let reference = scenarios[0].netlist;
    let reference_placement = eda_pnr::place(reference)?;
    for scenario in scenarios.iter().skip(1) {
        let placement = eda_pnr::place(scenario.netlist)?;
        check_common_physical_topology(
            reference,
            &reference_placement,
            scenario.netlist,
            &placement,
            &scenario.name,
        )?;
    }
    Ok(())
}

fn map_placement_by_names(
    owner: &Netlist,
    placement: &Placement,
    target: &Netlist,
    scenario: &str,
) -> Result<Placement, String> {
    let (owner_cells, _) = physical_name_maps(owner, "geometry-owner")?;
    let (target_cells, _) = physical_name_maps(target, scenario)?;
    let mut cell_pos = Vec::with_capacity(target.cell_count());
    let mut cell_width = Vec::with_capacity(target.cell_count());
    for target_cell_id in 0..target.cell_count() {
        let name = target.cell(target_cell_id).name.as_str();
        let owner_cell_id = *owner_cells
            .get(name)
            .ok_or_else(|| format!("cell '{}' is missing from common geometry owner", name))?;
        let position = placement.cell_pos[owner_cell_id];
        cell_pos.push(position);
        cell_width.push(placement.cell_width[owner_cell_id]);
    }
    let _ = target_cells;
    Ok(Placement {
        rows: placement.rows,
        cell_pos,
        cell_width,
        blockages: placement.blockages.clone(),
    })
}

fn map_routing_by_names(
    owner: &Netlist,
    routing: &Routing,
    target: &Netlist,
    scenario: &str,
) -> Result<Routing, String> {
    let (_, owner_nets) = physical_name_maps(owner, "geometry-owner")?;
    let (_, target_nets) = physical_name_maps(target, scenario)?;
    let mut mapped = routing.clone();
    for segment in &mut mapped.segments {
        let name = owner.net(segment.net).name.clone();
        segment.net = *target_nets
            .get(&name)
            .ok_or_else(|| format!("net '{}' is missing from physical scenario", name))?;
    }
    for via in &mut mapped.via_locations {
        let name = owner.net(via.net).name.clone();
        via.net = *target_nets
            .get(&name)
            .ok_or_else(|| format!("net '{}' is missing from physical scenario", name))?;
    }
    let _ = owner_nets;
    Ok(mapped)
}

fn constrained_setup_slack(timing: &ConstrainedTiming) -> f64 {
    timing
        .reg2reg_slack
        .min(timing.in2reg_slack)
        .min(timing.output_slack)
        .min(timing.max_delay_slack)
}

fn constrained_hold_slack(timing: &ConstrainedTiming) -> f64 {
    timing
        .reg2reg_hold_slack
        .min(timing.in2reg_hold_slack)
        .min(timing.min_delay_slack)
}

fn worst_corner_index(
    corners: &[MultiCornerPhysicalTimingCorner],
    slack: impl Fn(&ConstrainedTiming) -> f64,
) -> usize {
    let mut worst = 0_usize;
    for (index, corner) in corners.iter().enumerate().skip(1) {
        let Some(timing) = corner.evaluation.constrained_timing.as_ref() else {
            continue;
        };
        let Some(previous) = corners[worst]
            .evaluation
            .constrained_timing
            .as_ref()
            .map(&slack)
        else {
            worst = index;
            continue;
        };
        if slack(timing).total_cmp(&previous) == std::cmp::Ordering::Less {
            worst = index;
        }
    }
    worst
}

fn objective_timing_after_waivers(
    timing: &ConstrainedTiming,
    store: &TimingViolationStore,
) -> Result<ConstrainedTiming, String> {
    let mut objective = timing.clone();
    for violation_id in store.waived_violations().keys() {
        let Some(violation) = store.violations().get(violation_id.as_str()) else {
            return Err(format!(
                "waived timing violation '{}' is missing from its store",
                violation_id
            ));
        };
        if matches!(
            violation.kind,
            TimingViolationKind::MinPulseHigh | TimingViolationKind::MinPulseLow
        ) {
            // Min-pulse checks are clock-waveform constraints, not routed
            // data-path endpoint slacks, so there is no data-net objective to
            // remove.
            continue;
        }
        let Some(net_name) = violation.nets.last() else {
            return Err(format!(
                "waived timing violation '{}' has no endpoint net",
                violation_id
            ));
        };
        let name = net_name.clone();
        let Some(slack) = objective.net_slacks.get_mut(&name) else {
            return Err(format!(
                "waived timing endpoint net '{}' has no constrained slack",
                name
            ));
        };
        match violation.kind {
            TimingViolationKind::Setup => slack.setup_ns = None,
            TimingViolationKind::Hold => slack.hold_ns = None,
            TimingViolationKind::Recovery => slack.recovery_ns = None,
            TimingViolationKind::Removal => slack.removal_ns = None,
            TimingViolationKind::Output => slack.output_ns = None,
            TimingViolationKind::MaxDelay => slack.max_delay_ns = None,
            TimingViolationKind::MinDelay => slack.min_delay_ns = None,
            TimingViolationKind::MinPulseHigh | TimingViolationKind::MinPulseLow => continue,
        }
    }
    Ok(objective)
}

fn active_late_slack(
    timing: &ConstrainedTiming,
    store: &TimingViolationStore,
) -> Result<Option<f64>, String> {
    let objective = objective_timing_after_waivers(timing, store)?;
    Ok(objective
        .net_slacks
        .values()
        .filter_map(|slack| slack.late_slack_ns())
        .filter(|slack| slack.is_finite())
        .reduce(f64::min))
}

fn optimization_corner(iteration: &MultiCornerPhysicalTimingIteration) -> Result<usize, String> {
    let mut selected = None;
    for (index, corner) in iteration.corners.iter().enumerate() {
        let Some(timing) = corner.evaluation.constrained_timing.as_ref() else {
            continue;
        };
        let Some(slack) = active_late_slack(timing, &corner.violations)? else {
            continue;
        };
        if selected
            .map(|(_, previous_slack)| slack.total_cmp(&previous_slack) == std::cmp::Ordering::Less)
            .unwrap_or(true)
        {
            selected = Some((index, slack));
        }
    }
    Ok(selected
        .map(|(index, _)| index)
        .unwrap_or(iteration.worst_slack_corner))
}

fn evaluate_multi_corner_physical_timing_iteration_cancellable(
    index: usize,
    scenarios: &[MultiCornerPhysicalTimingScenario<'_>],
    geometry_owner: &Netlist,
    placement: Placement,
    routing: Routing,
    model: &InterconnectModel,
    cancellation: &FlowCancellation,
) -> Result<MultiCornerPhysicalTimingIteration, String> {
    if cancellation.is_cancelled() {
        return Err("multi-corner physical timing analysis cancelled".into());
    }
    let parasitics = extract_interconnect_parasitics(geometry_owner, &routing, model)?;
    let mut corners = Vec::with_capacity(scenarios.len());
    for scenario in scenarios {
        if cancellation.is_cancelled() {
            return Err("multi-corner physical timing analysis cancelled".into());
        }
        let mapped_placement =
            map_placement_by_names(geometry_owner, &placement, scenario.netlist, &scenario.name)?;
        let mapped_routing =
            map_routing_by_names(geometry_owner, &routing, scenario.netlist, &scenario.name)?;
        let parasitic_report = parasitic_analysis(scenario.netlist, &parasitics)?;
        let sdf_text = parasitics_to_sdf(scenario.netlist, &parasitics)?;
        let sdf = parse_delay_file(&sdf_text)?;
        let annotated_netlist = back_annotate(scenario.netlist, &sdf)?;
        let timing = eda_sta::analyze(&annotated_netlist)?;
        if cancellation.is_cancelled() {
            return Err("multi-corner physical timing analysis cancelled".into());
        }
        let constrained_timing = constrained_timing(&annotated_netlist, scenario.constraints)?;
        let drc = physical_verification(&annotated_netlist, &mapped_placement, &mapped_routing)?;
        let antenna_warnings = drc
            .violations
            .iter()
            .filter(|violation| violation.code == "ANT-001")
            .count();
        if drc.has_errors() {
            return Err(format!(
                "multi-corner physical timing iteration {} scenario '{}' has DRC errors: {:?}",
                index, scenario.name, drc.violations
            ));
        }
        let mut violations =
            timing_violation_store(&scenario.name, &annotated_netlist, &constrained_timing)?;
        for waiver in &scenario.waivers {
            if cancellation.is_cancelled() {
                return Err("multi-corner physical timing analysis cancelled".into());
            }
            add_timing_violation_waiver(&mut violations, waiver.clone())?;
        }
        corners.push(MultiCornerPhysicalTimingCorner {
            scenario: scenario.name.clone(),
            evaluation: PhysicalTimingIteration {
                index,
                placement: mapped_placement,
                routing: mapped_routing,
                parasitics: parasitics.clone(),
                parasitic_report,
                sdf,
                annotated_netlist,
                timing,
                constrained_timing: Some(constrained_timing),
                drc,
                antenna_repairs: Vec::new(),
                antenna_warnings,
                hold_repairs: Vec::new(),
                hold_repair_applied: false,
                all_active_hold_repaired: true,
            },
            violations,
        });
    }
    let worst_slack_corner = worst_corner_index(&corners, |timing| timing.worst_slack);
    let worst_setup_corner = worst_corner_index(&corners, constrained_setup_slack);
    let worst_hold_corner = worst_corner_index(&corners, constrained_hold_slack);
    let registry = timing_violation_registry_from_stores(
        corners
            .iter()
            .map(|corner| (corner.scenario.clone(), corner.violations.clone())),
    )?;
    let Some(first) = corners.first() else {
        return Err("multi-corner physical timing flow requires a scenario".into());
    };
    let antenna_warnings = first.evaluation.antenna_warnings;
    Ok(MultiCornerPhysicalTimingIteration {
        index,
        placement: first.evaluation.placement.clone(),
        routing: first.evaluation.routing.clone(),
        corners,
        registry,
        registry_update: None,
        antenna_repairs: Vec::new(),
        antenna_warnings,
        hold_repairs: Vec::new(),
        hold_repair_applied: false,
        all_active_hold_repaired: true,
        worst_slack_corner,
        worst_setup_corner,
        worst_hold_corner,
        optimization_corner: None,
    })
}

/// Run one common physical implementation across named timing scenarios.
///
/// The flow places and routes the first scenario, extracts one name-keyed RC
/// view, back-annotates that RC separately into every scenario, and performs
/// constrained STA in each view. Optimization uses one common placement and
/// routing. Scenario netlists may differ only in timing views (Liberty arcs
/// or SDF annotations); physical cell names, widths, connectivity, and port
/// topology must be stable. Hold-critical fast-corner repair is reported but
/// is not yet a separate hold-aware placement/routing objective.
pub fn run_multi_corner_physical_timing_flow(
    options: &MultiCornerPhysicalTimingFlowOptions<'_>,
) -> Result<MultiCornerPhysicalTimingFlowResult, String> {
    run_multi_corner_physical_timing_flow_cancellable(options, &FlowCancellation::new())
}

/// Run MMMC physical timing with cooperative major-stage/router checks.
pub fn run_multi_corner_physical_timing_flow_cancellable(
    options: &MultiCornerPhysicalTimingFlowOptions<'_>,
    cancellation: &FlowCancellation,
) -> Result<MultiCornerPhysicalTimingFlowResult, String> {
    if cancellation.is_cancelled() {
        return Err("multi-corner physical timing analysis cancelled".into());
    }
    options.validate()?;
    validate_common_physical_topology(&options.scenarios)?;

    fn scenarios_for_local_netlists<'a>(
        source: &'a [MultiCornerPhysicalTimingScenario<'a>],
        netlists: &'a [Netlist],
    ) -> Vec<MultiCornerPhysicalTimingScenario<'a>> {
        source
            .iter()
            .zip(netlists)
            .map(|(scenario, netlist)| MultiCornerPhysicalTimingScenario {
                name: scenario.name.clone(),
                netlist,
                constraints: scenario.constraints,
                waivers: scenario.waivers.clone(),
            })
            .collect()
    }

    let mut physical_netlists = Vec::new();
    let clock_tree = if let Some(cts) = options.timing_driven_cts {
        if cancellation.is_cancelled() {
            return Err("multi-corner physical timing analysis cancelled".into());
        }
        let reference = options.scenarios[0].netlist;
        let seed_placement = match &options.initial_placement {
            Some(placement) => {
                eda_pnr::check_legal(reference, placement)?;
                placement.clone()
            }
            None => eda_pnr::place(reference)?,
        };
        let reference_cts =
            timing_driven_clock_tree_synthesis_with_placement(reference, &seed_placement, &cts)?;
        let reference_placement = reference_cts
            .cts
            .physical_placement
            .clone()
            .ok_or_else(|| "timing-driven MMMC CTS produced no placement".to_string())?;
        let branch_factor = reference_cts.selected_branch_factor;
        for scenario in &options.scenarios {
            if cancellation.is_cancelled() {
                return Err("multi-corner physical timing analysis cancelled".into());
            }
            let view_cts = clock_tree_synthesis_with_placement(
                scenario.netlist,
                &seed_placement,
                &CtsOptions {
                    branch_factor,
                    wire_delay_ns_per_unit: cts.wire_delay_ns_per_unit,
                },
            )?;
            if view_cts.physical_placement.as_ref() != Some(&reference_placement) {
                return Err(format!(
                    "MMMC timing-driven CTS produced different physical topology in scenario '{}'",
                    scenario.name
                ));
            }
            physical_netlists.push(view_cts.netlist);
        }
        Some(reference_cts)
    } else {
        for scenario in &options.scenarios {
            if cancellation.is_cancelled() {
                return Err("multi-corner physical timing analysis cancelled".into());
            }
            if let Some(placement) = &options.initial_placement {
                eda_pnr::check_legal(scenario.netlist, placement)?;
            }
            physical_netlists.push(scenario.netlist.clone());
        }
        None
    };
    if cancellation.is_cancelled() {
        return Err("multi-corner physical timing analysis cancelled".into());
    }

    let mut effective_scenarios =
        scenarios_for_local_netlists(&options.scenarios, &physical_netlists);
    validate_common_physical_topology(&effective_scenarios)?;
    let reference = &physical_netlists[0];
    let mut baseline_placement = match &clock_tree {
        Some(cts) => cts
            .cts
            .physical_placement
            .clone()
            .ok_or_else(|| "timing-driven MMMC CTS produced no placement".to_string())?,
        None => match &options.initial_placement {
            Some(placement) => placement.clone(),
            None => eda_pnr::place(reference)?,
        },
    };
    let mut baseline_routing = eda_pnr::route_with_options_cancellable(
        reference,
        &baseline_placement,
        &options.route,
        cancellation,
    )?;
    let mut baseline_antenna_repairs = Vec::new();
    if let Some(repaired) = repair_multicorner_antennas(
        &effective_scenarios,
        reference,
        &baseline_placement,
        &baseline_routing,
        options,
        usize::MAX,
    )? {
        if cancellation.is_cancelled() {
            return Err("multi-corner physical timing analysis cancelled".into());
        }
        physical_netlists = repaired.scenario_netlists;
        effective_scenarios = scenarios_for_local_netlists(&options.scenarios, &physical_netlists);
        validate_common_physical_topology(&effective_scenarios)?;
        baseline_placement = repaired.placement;
        baseline_routing = repaired.routing;
        baseline_antenna_repairs = repaired.repairs;
    }
    let hold_repair_result = if let Some(hold_options) = &options.hold_repair {
        if cancellation.is_cancelled() {
            return Err("multi-corner physical timing analysis cancelled".into());
        }
        let repaired = repair_multicorner_hold_violations(
            &effective_scenarios,
            &baseline_placement,
            hold_options,
        )?;
        physical_netlists = repaired
            .scenarios
            .iter()
            .map(|scenario| scenario.netlist.clone())
            .collect();
        effective_scenarios = scenarios_for_local_netlists(&options.scenarios, &physical_netlists);
        validate_common_physical_topology(&effective_scenarios)?;
        baseline_placement = repaired.physical_placement.clone();
        baseline_routing = repaired.routing.clone();
        Some(repaired)
    } else {
        None
    };
    let mut baseline = evaluate_multi_corner_physical_timing_iteration_cancellable(
        usize::MAX,
        &effective_scenarios,
        &physical_netlists[0],
        baseline_placement,
        baseline_routing,
        &options.interconnect,
        cancellation,
    )?;
    baseline.antenna_repairs = baseline_antenna_repairs;
    baseline.antenna_warnings = baseline.antenna_warnings.max(baseline.antenna_warnings);
    if let Some(hold) = &hold_repair_result {
        baseline.hold_repairs = hold.repairs.clone();
        baseline.hold_repair_applied = true;
        baseline.all_active_hold_repaired = hold.all_active_violations_repaired;
    }

    let mut optimized = Vec::new();
    let mut persistent_registry = baseline.registry.clone();
    let mut current = baseline.clone();
    for index in 0..options.iterations {
        if cancellation.is_cancelled() {
            return Err("multi-corner physical timing analysis cancelled".into());
        }
        let objective_index = optimization_corner(&current)?;
        let corner = &current.corners[objective_index];
        let scenario = &effective_scenarios[objective_index];
        let constrained = corner
            .evaluation
            .constrained_timing
            .as_ref()
            .ok_or_else(|| {
                format!(
                    "physical timing scenario '{}' has no constrained timing",
                    scenario.name
                )
            })?;
        let placement = if scenario.waivers.is_empty() {
            timing_driven_place_from_constraints(
                &corner.evaluation.annotated_netlist,
                constrained,
                options.placement_passes,
            )?
            .placement
        } else {
            timing_driven_place_from_waiver_aware_timing(
                &corner.evaluation.annotated_netlist,
                constrained,
                &corner.violations,
                options.placement_passes,
            )?
            .placement
        };
        if cancellation.is_cancelled() {
            return Err("multi-corner physical timing analysis cancelled".into());
        }
        let routing = if scenario.waivers.is_empty() {
            route_maze_timing_ripgup_from_constraints_cancellable(
                &corner.evaluation.annotated_netlist,
                &placement,
                &options.route,
                constrained,
                options.routing_passes,
                cancellation,
            )?
            .ripup
            .routing
        } else {
            route_maze_timing_ripgup_from_waiver_aware_timing_cancellable(
                &corner.evaluation.annotated_netlist,
                &placement,
                &options.route,
                constrained,
                &corner.violations,
                options.routing_passes,
                cancellation,
            )?
            .ripup
            .routing
        };
        if cancellation.is_cancelled() {
            return Err("multi-corner physical timing analysis cancelled".into());
        }

        let mut antenna_repairs = Vec::new();
        let mut antenna_warnings = 0;
        let (geometry_owner, placement, routing) = if let Some(repaired) =
            repair_multicorner_antennas(
                &effective_scenarios,
                &corner.evaluation.annotated_netlist,
                &placement,
                &routing,
                options,
                index,
            )? {
            if cancellation.is_cancelled() {
                return Err("multi-corner physical timing analysis cancelled".into());
            }
            physical_netlists = repaired.scenario_netlists;
            effective_scenarios =
                scenarios_for_local_netlists(&options.scenarios, &physical_netlists);
            validate_common_physical_topology(&effective_scenarios)?;
            antenna_repairs = repaired.repairs;
            antenna_warnings = repaired.antenna_warnings;
            (&physical_netlists[0], repaired.placement, repaired.routing)
        } else {
            (&corner.evaluation.annotated_netlist, placement, routing)
        };
        let mut hold_repairs = Vec::new();
        let mut hold_repair_applied = false;
        let mut all_active_hold_repaired = true;
        let (geometry_owner, placement, routing) = if let Some(hold_options) = &options.hold_repair
        {
            if cancellation.is_cancelled() {
                return Err("multi-corner physical timing analysis cancelled".into());
            }
            let mut iteration_options = hold_options.clone();
            iteration_options.hold.patch_name = format!("mmmc_physical_hold_{index}");
            let repaired = repair_multicorner_hold_violations(
                &effective_scenarios,
                &placement,
                &iteration_options,
            )?;
            if cancellation.is_cancelled() {
                return Err("multi-corner physical timing analysis cancelled".into());
            }
            physical_netlists = repaired
                .scenarios
                .iter()
                .map(|scenario| scenario.netlist.clone())
                .collect();
            effective_scenarios =
                scenarios_for_local_netlists(&options.scenarios, &physical_netlists);
            validate_common_physical_topology(&effective_scenarios)?;
            hold_repairs = repaired.repairs;
            hold_repair_applied = true;
            all_active_hold_repaired = repaired.all_active_violations_repaired;
            (
                &physical_netlists[0],
                repaired.physical_placement,
                repaired.routing,
            )
        } else {
            (geometry_owner, placement, routing)
        };
        let mut iteration = evaluate_multi_corner_physical_timing_iteration_cancellable(
            index,
            &effective_scenarios,
            geometry_owner,
            placement,
            routing,
            &options.interconnect,
            cancellation,
        )?;
        iteration.optimization_corner = Some(objective_index);
        iteration.antenna_repairs = antenna_repairs;
        let _ = antenna_warnings;
        iteration.hold_repairs = hold_repairs;
        iteration.hold_repair_applied = hold_repair_applied;
        iteration.all_active_hold_repaired = all_active_hold_repaired;
        let registry_update = persistent_registry.replace_stores(
            iteration
                .corners
                .iter()
                .map(|corner| (corner.scenario.clone(), corner.violations.clone())),
        )?;
        iteration.registry = persistent_registry.clone();
        iteration.registry_update = Some(registry_update);
        current = iteration.clone();
        optimized.push(iteration);
    }

    let best_index = optimized
        .iter()
        .enumerate()
        .min_by(|(left_index, left), (right_index, right)| {
            let left_slack = left.corners[left.worst_slack_corner]
                .evaluation
                .constrained_timing
                .as_ref()
                .map(|timing| timing.worst_slack)
                .unwrap_or(f64::NEG_INFINITY);
            let right_slack = right.corners[right.worst_slack_corner]
                .evaluation
                .constrained_timing
                .as_ref()
                .map(|timing| timing.worst_slack)
                .unwrap_or(f64::NEG_INFINITY);
            right_slack
                .total_cmp(&left_slack)
                .then(left.routing.wirelength().cmp(&right.routing.wirelength()))
                .then(left_index.cmp(right_index))
        })
        .map(|(index, _)| index)
        .unwrap_or(0);
    baseline.optimization_corner = None;
    Ok(MultiCornerPhysicalTimingFlowResult {
        baseline,
        optimized,
        best_index,
        clock_tree,
        hold_repair: hold_repair_result,
    })
}

pub fn multi_corner_physical_timing_flow_report(
    result: &MultiCornerPhysicalTimingFlowResult,
) -> String {
    let best = result.best();
    let corner_slack = |iteration: &MultiCornerPhysicalTimingIteration| -> f64 {
        iteration.corners[iteration.worst_slack_corner]
            .evaluation
            .constrained_timing
            .as_ref()
            .map(|timing| timing.worst_slack)
            .unwrap_or(f64::NAN)
    };
    let iteration_name = |index: usize| -> String {
        if index == usize::MAX {
            "baseline".to_string()
        } else {
            index.to_string()
        }
    };
    let mut out = format!(
        "Multi-corner physical timing flow: baseline worst slack {:.6} ns, best worst slack {:.6} ns, best iteration {}, scenarios {}\n",
        corner_slack(&result.baseline),
        corner_slack(best),
        iteration_name(best.index),
        result.baseline.corners.len()
    );
    if let Some(clock_tree) = &result.clock_tree {
        out.push_str(&timing_driven_cts_report(clock_tree));
    }
    if let Some(hold_repair) = &result.hold_repair {
        out.push_str(&multi_corner_hold_repair_report(hold_repair));
    }
    for iteration in std::iter::once(&result.baseline).chain(result.optimized.iter()) {
        let Some(reference) = iteration.corners.first() else {
            continue;
        };
        out.push_str(&format!(
            "  iteration {}: common HPWL {:.3} sites, wirelength {}, RC nets {}, worst slack scenario '{}', DRC errors {}, antenna diodes {}, antenna warnings {}, hold buffers {}, hold status {}, active violations {}, cross-corner checks {}\n",
            iteration_name(iteration.index),
            eda_pnr::hpwl(
                &reference.evaluation.annotated_netlist,
                &iteration.placement
            ),
            iteration.routing.wirelength(),
            reference.evaluation.parasitic_report.net_count,
            iteration.corners[iteration.worst_slack_corner].scenario,
            reference.evaluation.drc.error_count(),
            iteration
                .antenna_repairs
                .iter()
                .map(|repair| repair.diodes_inserted)
                .sum::<usize>(),
            iteration.antenna_warnings,
            iteration
                .hold_repairs
                .iter()
                .map(|repair| repair.buffers_inserted)
                .sum::<usize>(),
            if iteration.hold_repair_applied {
                if iteration.all_active_hold_repaired {
                    "PASS"
                } else {
                    "LIMITED"
                }
            } else {
                "NONE"
            },
            iteration.registry.len(),
            iteration.registry.summaries().len()
        ));
        if let Some(update) = &iteration.registry_update {
            out.push_str(&format!(
                "    registry lifecycle: scenarios +{}/-{}, violations added={}, removed={}, improved={}, degraded={}, unchanged={}\n",
                update.added_scenarios.len(),
                update.removed_scenarios.len(),
                update.cross_corner.added.len(),
                update.cross_corner.removed.len(),
                update.cross_corner.improved.len(),
                update.cross_corner.degraded.len(),
                update.cross_corner.unchanged.len()
            ));
        }
        for (corner_index, corner) in iteration.corners.iter().enumerate() {
            let Some(timing) = corner.evaluation.constrained_timing.as_ref() else {
                continue;
            };
            let worst_marker = |index: usize| {
                if index == corner_index {
                    " *"
                } else {
                    ""
                }
            };
            out.push_str(&format!(
                "    scenario '{}'{}: delay {:.6} ns, worst slack {:.6} ns, setup {:.6} ns, hold {:.6} ns, waived violations {}{}\n",
                corner.scenario,
                worst_marker(iteration.worst_slack_corner),
                corner.evaluation.timing.critical_path_delay,
                timing.worst_slack,
                constrained_setup_slack(timing),
                constrained_hold_slack(timing),
                corner.violations.waived_violations().len(),
                if iteration.optimization_corner == Some(corner_index) {
                    ", optimization objective"
                } else {
                    ""
                }
            ));
        }
    }
    out
}

/// Run placement → routing → RC extraction → SDF back-annotation → STA.
///
/// The baseline uses deterministic global routing. Each requested iteration
/// feeds the previous RC-annotated STA arrivals into timing-driven placement
/// and timing-prioritized rip-up/reroute. All candidates are evaluated with
/// the same interconnect model, and the lowest critical-path delay wins.
pub fn run_physical_timing_flow(
    netlist: &Netlist,
    options: &PhysicalTimingFlowOptions,
) -> Result<PhysicalTimingFlowResult, String> {
    run_physical_timing_flow_cancellable(netlist, options, &FlowCancellation::new())
}

/// Run the physical timing loop with cooperative stage/router checks.
pub fn run_physical_timing_flow_cancellable(
    netlist: &Netlist,
    options: &PhysicalTimingFlowOptions,
    cancellation: &FlowCancellation,
) -> Result<PhysicalTimingFlowResult, String> {
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    options.validate()?;
    let clock_tree = match options.timing_driven_cts {
        Some(cts) => {
            if cancellation.is_cancelled() {
                return Err("physical timing analysis cancelled".into());
            }
            let seed_placement = match &options.initial_placement {
                Some(placement) => {
                    eda_pnr::check_legal(netlist, placement)?;
                    placement.clone()
                }
                None => eda_pnr::place(netlist)?,
            };
            let synthesized =
                timing_driven_clock_tree_synthesis_with_placement(netlist, &seed_placement, &cts)?;
            if cancellation.is_cancelled() {
                return Err("physical timing analysis cancelled".into());
            }
            Some(synthesized)
        }
        None => {
            if let Some(placement) = &options.initial_placement {
                eda_pnr::check_legal(netlist, placement)?;
            }
            None
        }
    };
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let effective_netlist = clock_tree
        .as_ref()
        .and_then(|result| {
            result
                .cts
                .physical_placement
                .as_ref()
                .map(|_| &result.cts.netlist)
        })
        .unwrap_or(netlist);
    if options.repair_antennas
        && effective_netlist
            .library()
            .and_then(|library| library.find("DIODE"))
            .is_none()
    {
        return Err("antenna-aware physical timing flow requires a library DIODE cell".into());
    }
    let baseline_placement = match clock_tree
        .as_ref()
        .and_then(|result| result.cts.physical_placement.clone())
    {
        Some(placement) => placement,
        None => match &options.initial_placement {
            Some(placement) => placement.clone(),
            None => eda_pnr::place(effective_netlist)?,
        },
    };
    let baseline_routing = eda_pnr::route_with_options_cancellable(
        effective_netlist,
        &baseline_placement,
        &options.route,
        cancellation,
    )?;
    let antenna_baseline = repair_physical_timing_antennas_cancellable(
        evaluate_physical_timing_iteration_cancellable(
            usize::MAX,
            effective_netlist,
            baseline_placement,
            baseline_routing,
            &options.interconnect,
            options.constraints.as_ref(),
            cancellation,
        )?,
        usize::MAX,
        options,
        cancellation,
    )?;
    let (baseline, hold_repair) = if options.hold_repair.is_some() {
        let (mut iteration, repair) =
            repair_physical_timing_hold_cancellable(antenna_baseline, options, None, cancellation)?;
        iteration.hold_repairs = repair.repairs.clone();
        iteration.hold_repair_applied = true;
        iteration.all_active_hold_repaired = repair.all_active_violations_repaired;
        (iteration, Some(repair))
    } else {
        (antenna_baseline, None::<HoldRepairResult>)
    };

    let mut optimized = Vec::new();
    let mut current = baseline.clone();
    for index in 0..options.iterations {
        if cancellation.is_cancelled() {
            return Err("physical timing analysis cancelled".into());
        }
        let placement = if let Some(constrained) = &current.constrained_timing {
            if options.waivers.is_empty() {
                timing_driven_place_from_constraints(
                    &current.annotated_netlist,
                    constrained,
                    options.placement_passes,
                )?
                .placement
            } else {
                let mut violation_store = timing_violation_store(
                    &constrained.clock,
                    &current.annotated_netlist,
                    constrained,
                )?;
                for waiver in &options.waivers {
                    if cancellation.is_cancelled() {
                        return Err("physical timing analysis cancelled".into());
                    }
                    add_timing_violation_waiver(&mut violation_store, waiver.clone())?;
                }
                timing_driven_place_from_waiver_aware_timing(
                    &current.annotated_netlist,
                    constrained,
                    &violation_store,
                    options.placement_passes,
                )?
                .placement
            }
        } else {
            timing_driven_place_from_timing(
                &current.annotated_netlist,
                &current.timing,
                options.placement_passes,
            )?
            .placement
        };
        if cancellation.is_cancelled() {
            return Err("physical timing analysis cancelled".into());
        }
        let routing = if let Some(constrained) = &current.constrained_timing {
            if options.waivers.is_empty() {
                route_maze_timing_ripgup_from_constraints_cancellable(
                    &current.annotated_netlist,
                    &placement,
                    &options.route,
                    constrained,
                    options.routing_passes,
                    cancellation,
                )?
                .ripup
                .routing
            } else {
                let mut violation_store = timing_violation_store(
                    &constrained.clock,
                    &current.annotated_netlist,
                    constrained,
                )?;
                for waiver in &options.waivers {
                    if cancellation.is_cancelled() {
                        return Err("physical timing analysis cancelled".into());
                    }
                    add_timing_violation_waiver(&mut violation_store, waiver.clone())?;
                }
                route_maze_timing_ripgup_from_waiver_aware_timing_cancellable(
                    &current.annotated_netlist,
                    &placement,
                    &options.route,
                    constrained,
                    &violation_store,
                    options.routing_passes,
                    cancellation,
                )?
                .ripup
                .routing
            }
        } else {
            route_maze_timing_ripgup_from_timing_cancellable(
                &current.annotated_netlist,
                &placement,
                &options.route,
                &current.timing,
                options.routing_passes,
                cancellation,
            )?
            .ripup
            .routing
        };
        let antenna_iteration = repair_physical_timing_antennas_cancellable(
            evaluate_physical_timing_iteration_cancellable(
                index,
                &current.annotated_netlist,
                placement,
                routing,
                &options.interconnect,
                options.constraints.as_ref(),
                cancellation,
            )?,
            index,
            options,
            cancellation,
        )?;
        let (mut iteration, hold_result) = if options.hold_repair.is_some() {
            let patch_name = format!("physical_hold_{index}");
            let (iteration, result) = repair_physical_timing_hold_cancellable(
                antenna_iteration,
                options,
                Some(&patch_name),
                cancellation,
            )?;
            (iteration, Some(result))
        } else {
            (antenna_iteration, None)
        };
        if let Some(result) = hold_result {
            iteration.hold_repairs = result.repairs;
            iteration.hold_repair_applied = true;
            iteration.all_active_hold_repaired = result.all_active_violations_repaired;
        }
        current = iteration.clone();
        optimized.push(iteration);
    }

    let best_index = optimized
        .iter()
        .enumerate()
        .min_by(|(left_index, left), (right_index, right)| {
            let left_quality = left
                .constrained_timing
                .as_ref()
                .map(|timing| timing.worst_slack)
                .unwrap_or(-left.timing.critical_path_delay);
            let right_quality = right
                .constrained_timing
                .as_ref()
                .map(|timing| timing.worst_slack)
                .unwrap_or(-right.timing.critical_path_delay);
            right_quality
                .total_cmp(&left_quality)
                .then(left.routing.wirelength().cmp(&right.routing.wirelength()))
                .then(left_index.cmp(right_index))
        })
        .map(|(index, _)| index)
        .unwrap_or(0);
    Ok(PhysicalTimingFlowResult {
        baseline,
        optimized,
        best_index,
        clock_tree,
        hold_repair,
    })
}

pub fn physical_timing_flow_report(result: &PhysicalTimingFlowResult) -> String {
    let best = result.best();
    let slack_text = |iteration: &PhysicalTimingIteration| -> String {
        iteration
            .constrained_timing
            .as_ref()
            .map(|timing| format!("{:.6}", timing.worst_slack))
            .unwrap_or_else(|| "n/a".into())
    };
    let mut out = format!(
        "Physical timing flow: baseline delay {:.6} ns, baseline slack {}, best delay {:.6} ns, best slack {}, best iteration {}\n",
        result.baseline.timing.critical_path_delay,
        slack_text(&result.baseline),
        best.timing.critical_path_delay,
        slack_text(best),
        if best.index == usize::MAX {
            "baseline".to_string()
        } else {
            best.index.to_string()
        }
    );
    if let Some(clock_tree) = &result.clock_tree {
        out.push_str(&timing_driven_cts_report(clock_tree));
    }
    if let Some(hold_repair) = &result.hold_repair {
        out.push_str(&hold_repair_report(hold_repair));
    }
    for iteration in std::iter::once(&result.baseline).chain(result.optimized.iter()) {
        out.push_str(&format!(
            "  iteration {}: HPWL {:.3} sites, wirelength {}, RC nets {}, delay {:.6} ns, DRC errors {}, antenna diodes {}, antenna warnings {}, hold buffers {}, hold status {}\n",
            if iteration.index == usize::MAX {
                "baseline".to_string()
            } else {
                iteration.index.to_string()
            },
            eda_pnr::hpwl(&iteration.annotated_netlist, &iteration.placement),
            iteration.routing.wirelength(),
            iteration.parasitic_report.net_count,
            iteration.timing.critical_path_delay,
            iteration.drc.error_count(),
            iteration
                .antenna_repairs
                .iter()
                .map(|repair| repair.diodes_inserted)
                .sum::<usize>(),
            iteration.antenna_warnings,
            iteration
                .hold_repairs
                .iter()
                .map(|repair| repair.buffers_inserted)
                .sum::<usize>(),
            if iteration.hold_repair_applied {
                if iteration.all_active_hold_repaired {
                    "PASS"
                } else {
                    "LIMITED"
                }
            } else {
                "NONE"
            }
        ));
        out.push_str(&format!(
            "    constrained slack: {}\n",
            slack_text(iteration)
        ));
    }
    out
}

/// Place a netlist using caller-supplied timing/criticality weights.
pub fn timing_driven_place(
    nl: &Netlist,
    options: &TimingDrivenPlacementOptions,
) -> Result<TimingDrivenPlacementResult, String> {
    eda_pnr::place_timing_driven(nl, options)
}

/// Improve an existing legal placement using a weighted-HPWL objective.
pub fn optimize_timing_driven_placement(
    nl: &Netlist,
    placement: Placement,
    options: &TimingDrivenPlacementOptions,
) -> Result<TimingDrivenPlacementResult, String> {
    eda_pnr::optimize_placement_timing_driven(nl, placement, options)
}

/// Derive late-path criticality weights from STA and run timing-driven placement.
pub fn timing_driven_place_from_timing(
    nl: &Netlist,
    timing: &StaResult,
    max_passes: usize,
) -> Result<TimingDrivenPlacementResult, String> {
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
    let options = TimingDrivenPlacementOptions {
        net_weights,
        default_net_weight: 1.0,
        max_passes,
    };
    eda_pnr::place_timing_driven(nl, &options)
}

/// Derive net weights from constrained late-path slack.
///
/// The most critical endpoint net receives weight `1 + gain`; the least
/// critical receives weight 1. Weights are propagated through the endpoint's
/// combinational fan-in cone so the complete late path is prioritized.
pub fn timing_weights_from_constrained_timing(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    gain: f64,
) -> Result<std::collections::HashMap<usize, f64>, String> {
    if !gain.is_finite() || gain < 0.0 {
        return Err("timing criticality gain must be finite and non-negative".into());
    }

    let endpoint_slacks = timing
        .net_slacks
        .iter()
        .filter_map(|(name, slack)| {
            slack
                .late_slack_ns()
                .filter(|value| value.is_finite())
                .map(|value| (name.as_str(), value))
        })
        .collect::<Vec<_>>();
    let mut weights = std::collections::HashMap::new();
    if endpoint_slacks.is_empty() {
        return Ok(weights);
    }
    let min_slack = endpoint_slacks
        .iter()
        .map(|(_, slack)| *slack)
        .fold(f64::INFINITY, f64::min);
    let max_slack = endpoint_slacks
        .iter()
        .map(|(_, slack)| *slack)
        .fold(f64::NEG_INFINITY, f64::max);
    let range = max_slack - min_slack;

    for (name, slack) in endpoint_slacks {
        let criticality = if range.abs() < 1e-12 {
            1.0
        } else {
            (max_slack - slack) / range
        };
        let weight = 1.0 + gain * criticality;
        let Some(endpoint) = nl.net_by_name(name) else {
            return Err(format!("constrained slack net '{}' not found", name));
        };
        let mut stack = vec![endpoint];
        let mut seen = std::collections::HashSet::new();
        while let Some(net) = stack.pop() {
            if !seen.insert(net) {
                continue;
            }
            weights
                .entry(net)
                .and_modify(|old| *old = (*old).max(weight))
                .or_insert(weight);
            for &driver in &nl.net(net).drivers {
                let cell = nl.cell(driver);
                if nl.cell_is_sequential(&cell.kind) {
                    continue;
                }
                stack.extend(cell.inputs.iter().copied());
            }
        }
    }
    Ok(weights)
}

/// Derive timing weights while excluding checks covered by active waivers.
///
/// Raw violations remain available in `store`; only their endpoint slack is
/// removed from the optimizer objective. This lets an implementation keep
/// auditable timing history without spending physical-optimization effort on
/// accepted exceptions.
pub fn timing_weights_from_waiver_aware_timing(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    store: &TimingViolationStore,
    gain: f64,
) -> Result<std::collections::HashMap<usize, f64>, String> {
    let mut objective_timing = timing.clone();
    for violation_id in store.waived_violations().keys() {
        let Some(violation) = store.violations().get(violation_id.as_str()) else {
            return Err(format!(
                "waived timing violation '{}' is missing from its store",
                violation_id
            ));
        };
        if matches!(
            violation.kind,
            TimingViolationKind::MinPulseHigh | TimingViolationKind::MinPulseLow
        ) {
            continue;
        }
        let Some(net_name) = violation.nets.last() else {
            return Err(format!(
                "waived timing violation '{}' has no endpoint net",
                violation_id
            ));
        };
        let name = net_name.clone();
        let Some(slack) = objective_timing.net_slacks.get_mut(&name) else {
            return Err(format!(
                "waived timing endpoint net '{}' has no constrained slack",
                name
            ));
        };
        match violation.kind {
            TimingViolationKind::Setup => slack.setup_ns = None,
            TimingViolationKind::Hold => slack.hold_ns = None,
            TimingViolationKind::Recovery => slack.recovery_ns = None,
            TimingViolationKind::Removal => slack.removal_ns = None,
            TimingViolationKind::Output => slack.output_ns = None,
            TimingViolationKind::MaxDelay => slack.max_delay_ns = None,
            TimingViolationKind::MinDelay => slack.min_delay_ns = None,
            TimingViolationKind::MinPulseHigh | TimingViolationKind::MinPulseLow => continue,
        }
    }
    timing_weights_from_constrained_timing(nl, &objective_timing, gain)
}

/// Slack-aware timing-driven placement.
pub fn timing_driven_place_from_constraints(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    max_passes: usize,
) -> Result<TimingDrivenPlacementResult, String> {
    let net_weights = timing_weights_from_constrained_timing(nl, timing, 9.0)?;
    eda_pnr::place_timing_driven(
        nl,
        &TimingDrivenPlacementOptions {
            net_weights,
            default_net_weight: 1.0,
            max_passes,
        },
    )
}

/// Waiver-aware slack-driven placement.
pub fn timing_driven_place_from_waiver_aware_timing(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    store: &TimingViolationStore,
    max_passes: usize,
) -> Result<TimingDrivenPlacementResult, String> {
    let net_weights = timing_weights_from_waiver_aware_timing(nl, timing, store, 9.0)?;
    eda_pnr::place_timing_driven(
        nl,
        &TimingDrivenPlacementOptions {
            net_weights,
            default_net_weight: 1.0,
            max_passes,
        },
    )
}

/// Parse an SDC constraint subset through the unified API.
pub fn parse_timing_constraints(sdc: &str) -> Result<eda_sta::TimingConstraints, String> {
    eda_sta::parse_sdc(sdc)
}

/// SDC-aware static timing analysis through the unified API.
pub fn constrained_timing(
    nl: &Netlist,
    constraints: &eda_sta::TimingConstraints,
) -> Result<eda_sta::ConstrainedTiming, String> {
    eda_sta::analyze_with_sdc(nl, constraints)
}

/// Extract a stable-name block timing interface through the unified API.
pub fn extract_block_timing_model(
    nl: &Netlist,
    timing: &eda_sta::ConstrainedTiming,
) -> Result<BlockTimingModel, String> {
    eda_sta::block_timing_model(nl, timing)
}

/// Render a block timing interface model.
pub fn block_timing_model_report(model: &BlockTimingModel) -> String {
    eda_sta::block_timing_model_report(model)
}

/// Serialize a block timing interface model.
pub fn write_block_timing_model(model: &BlockTimingModel) -> Result<String, String> {
    eda_sta::write_block_timing_model(model)
}

/// Parse a serialized block timing interface model.
pub fn parse_block_timing_model(source: &str) -> Result<BlockTimingModel, String> {
    eda_sta::parse_block_timing_model(source)
}

/// Compose block timing interfaces by stable instance and port names.
pub fn compose_block_timing_models(
    models: &[BlockTimingModel],
    instances: &[BlockTimingCompositionInstance],
    connections: &[BlockTimingCompositionConnection],
) -> Result<BlockTimingCompositionResult, String> {
    eda_sta::compose_block_timing_models(models, instances, connections)
}

/// Compose block timing interfaces with per-instance clock-name mappings.
pub fn compose_block_timing_models_with_clock_maps(
    models: &[BlockTimingModel],
    instances: &[BlockTimingCompositionInstance],
    connections: &[BlockTimingCompositionConnection],
    clock_maps: &[BlockTimingClockMap],
) -> Result<BlockTimingCompositionResult, String> {
    eda_sta::compose_block_timing_models_with_clock_maps(models, instances, connections, clock_maps)
}

/// Compose block timing interfaces with multiple named clocks and timed edges.
pub fn compose_block_timing_models_with_clock_domains(
    models: &[BlockTimingModel],
    instances: &[BlockTimingCompositionInstance],
    connections: &[BlockTimingCompositionConnection],
    clock_maps: &[BlockTimingClockMap],
    clocks: &[BlockTimingClock],
    clock_relationships: &[BlockTimingClockRelationship],
) -> Result<BlockTimingCompositionResult, String> {
    eda_sta::compose_block_timing_models_with_clock_domains(
        models,
        instances,
        connections,
        clock_maps,
        clocks,
        clock_relationships,
    )
}

/// Render composed block-boundary timing checks.
pub fn block_timing_composition_report(result: &BlockTimingCompositionResult) -> String {
    eda_sta::block_timing_composition_report(result)
}

/// Compare two SDC constraint sets through the unified API.
pub fn diff_timing_constraints(
    old: &eda_sta::TimingConstraints,
    new: &eda_sta::TimingConstraints,
) -> TimingConstraintDiff {
    eda_sta::diff_timing_constraints(old, new)
}

/// Incrementally update base and constrained timing after SDC changes.
pub fn update_timing_for_constraints(
    nl: &Netlist,
    previous: &StaResult,
    old_constraints: &eda_sta::TimingConstraints,
    new_constraints: &eda_sta::TimingConstraints,
) -> Result<ConstraintTimingResult, String> {
    eda_sta::update_timing_for_constraints(nl, previous, old_constraints, new_constraints)
}

/// Incrementally update same-topology timing after input-delay changes.
pub fn update_timing_for_input_delays<I, S>(
    nl: &Netlist,
    previous: &StaResult,
    constraints: &eda_sta::TimingConstraints,
    changed_ports: I,
) -> Result<IncrementalTimingResult, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    eda_sta::update_timing_for_input_delays(nl, previous, constraints, changed_ports)
}

/// Incrementally update same-topology timing after cell/pin derate changes.
pub fn update_timing_for_timing_derates<C, S>(
    nl: &Netlist,
    previous: &StaResult,
    constraints: &eda_sta::TimingConstraints,
    changed_cells: C,
    changed_pins: &[eda_sta::TimingPin],
) -> Result<IncrementalTimingResult, String>
where
    C: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    eda_sta::update_timing_for_timing_derates(
        nl,
        previous,
        constraints,
        changed_cells,
        changed_pins,
    )
}

/// Compare two flat netlists by stable timing-object names.
pub fn diff_timing_topology(old: &Netlist, new: &Netlist) -> TimingTopologyDiff {
    eda_sta::diff_timing_topology(old, new)
}

/// Incrementally update timing after a flat-netlist topology/ECO change.
pub fn update_timing_for_topology(
    old: &Netlist,
    previous: &StaResult,
    new: &Netlist,
    constraints: &eda_sta::TimingConstraints,
) -> Result<TopologyTimingResult, String> {
    eda_sta::update_timing_for_topology(old, previous, new, constraints)
}

/// Extract late setup/output timing paths through the unified API.
pub fn timing_paths(nl: &Netlist, timing: &StaResult, max_paths: usize) -> Vec<TimingPath> {
    timing.timing_paths(nl, max_paths)
}

/// Extract early min-delay paths through the unified API.
pub fn min_timing_paths(nl: &Netlist, timing: &StaResult, max_paths: usize) -> Vec<TimingPath> {
    timing.min_timing_paths(nl, max_paths)
}

/// Analyze base timing through the unified API.
pub fn analyze_timing(nl: &Netlist) -> Result<StaResult, String> {
    eda_sta::analyze(nl)
}

/// Attribute one timing path to retained post-synthesis hierarchy instances.
pub fn hierarchical_timing_path(
    nl: &Netlist,
    path: &TimingPath,
) -> Result<HierarchicalTimingPath, String> {
    eda_sta::hierarchical_timing_path(nl, path)
}

/// Attribute timing paths to retained post-synthesis hierarchy instances.
pub fn hierarchical_timing_paths(
    nl: &Netlist,
    paths: &[TimingPath],
) -> Result<Vec<HierarchicalTimingPath>, String> {
    eda_sta::hierarchical_timing_paths(nl, paths)
}

/// Group constrained setup/output timing paths by hierarchy instance.
pub fn hierarchical_constrained_timing_paths(
    nl: &Netlist,
    paths: &[ConstrainedTimingPath],
) -> Result<Vec<HierarchicalTimingPath>, String> {
    paths
        .iter()
        .map(|constrained| hierarchical_timing_path(nl, &constrained.path))
        .collect()
}

/// Group hold timing paths by hierarchy instance.
pub fn hierarchical_hold_timing_paths(
    nl: &Netlist,
    paths: &[HoldTimingPath],
) -> Result<Vec<HierarchicalTimingPath>, String> {
    paths
        .iter()
        .map(|hold| hierarchical_timing_path(nl, &hold.path))
        .collect()
}

/// Group constrained hold timing paths by hierarchy instance.
pub fn hierarchical_constrained_hold_timing_paths(
    nl: &Netlist,
    paths: &[ConstrainedHoldTimingPath],
) -> Result<Vec<HierarchicalTimingPath>, String> {
    paths
        .iter()
        .map(|hold| hierarchical_timing_path(nl, &hold.path.path))
        .collect()
}

/// Group recovery/removal paths by hierarchy instance.
pub fn hierarchical_reset_timing_paths(
    nl: &Netlist,
    paths: &[ResetTimingPaths],
) -> Result<Vec<HierarchicalTimingPath>, String> {
    paths
        .iter()
        .flat_map(|reset| [&reset.recovery, &reset.removal])
        .map(|path| hierarchical_timing_path(nl, path))
        .collect()
}

/// Group constrained recovery/removal paths by hierarchy instance.
pub fn hierarchical_constrained_reset_timing_paths(
    nl: &Netlist,
    paths: &[ConstrainedResetTimingPath],
) -> Result<Vec<HierarchicalTimingPath>, String> {
    paths
        .iter()
        .map(|reset| hierarchical_timing_path(nl, &reset.path))
        .collect()
}

/// Render hierarchy-grouped timing paths.
pub fn hierarchical_timing_path_report(paths: &[HierarchicalTimingPath]) -> String {
    eda_sta::hierarchical_timing_path_report(paths)
}

/// Render an unconstrained STA summary through the unified API.
pub fn timing_report(nl: &Netlist, timing: &StaResult) -> String {
    eda_sta::report(nl, timing)
}

/// Render a constrained STA summary through the unified API.
pub fn constrained_timing_report(timing: &ConstrainedTiming) -> String {
    eda_sta::constrained_report(timing)
}

/// Render an unconstrained timing-path report.
pub fn timing_path_report(nl: &Netlist, paths: &[TimingPath]) -> String {
    eda_sta::timing_path_report(nl, paths)
}

/// Extract constrained setup/output paths sorted by worst slack.
pub fn constrained_timing_paths(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    max_paths: usize,
) -> Vec<ConstrainedTimingPath> {
    eda_sta::constrained_timing_paths(nl, timing, max_paths)
}

/// Render a constrained timing-path report.
pub fn constrained_timing_path_report(nl: &Netlist, paths: &[ConstrainedTimingPath]) -> String {
    eda_sta::constrained_timing_path_report(nl, paths)
}

/// Extract early hold paths through the unified API.
pub fn hold_timing_paths(
    nl: &Netlist,
    timing: &StaResult,
    hold_uncertainty: f64,
    max_paths: usize,
) -> Vec<HoldTimingPath> {
    timing.hold_timing_paths(nl, hold_uncertainty, max_paths)
}

/// Render an unconstrained hold-path report.
pub fn hold_timing_path_report(nl: &Netlist, paths: &[HoldTimingPath]) -> String {
    eda_sta::hold_timing_path_report(nl, paths)
}

/// Extract constrained hold paths sorted by worst hold slack.
pub fn constrained_hold_timing_paths(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    max_paths: usize,
) -> Vec<ConstrainedHoldTimingPath> {
    eda_sta::constrained_hold_timing_paths(nl, timing, max_paths)
}

/// Render a constrained hold-path report.
pub fn constrained_hold_timing_path_report(
    nl: &Netlist,
    paths: &[ConstrainedHoldTimingPath],
) -> String {
    eda_sta::constrained_hold_timing_path_report(nl, paths)
}

/// Extract asynchronous-reset recovery/removal path pairs.
pub fn reset_timing_paths(
    nl: &Netlist,
    timing: &StaResult,
    max_paths: usize,
) -> Vec<ResetTimingPaths> {
    timing.reset_timing_paths(nl, max_paths)
}

/// Render an unconstrained asynchronous-reset path report.
pub fn reset_timing_path_report(nl: &Netlist, paths: &[ResetTimingPaths]) -> String {
    eda_sta::reset_timing_path_report(nl, paths)
}

/// Extract constrained recovery/removal paths sorted by worst reset slack.
pub fn constrained_reset_timing_paths(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    max_paths: usize,
) -> Vec<ConstrainedResetTimingPath> {
    eda_sta::constrained_reset_timing_paths(nl, timing, max_paths)
}

/// Render a constrained asynchronous-reset path report.
pub fn constrained_reset_timing_path_report(
    nl: &Netlist,
    paths: &[ConstrainedResetTimingPath],
) -> String {
    eda_sta::constrained_reset_timing_path_report(nl, paths)
}

/// Extract concrete timing paths omitted by false-path exceptions.
pub fn excluded_timing_paths(nl: &Netlist, timing: &ConstrainedTiming) -> Vec<ExcludedTimingPath> {
    eda_sta::excluded_timing_paths(nl, timing)
}

/// Render concrete false-path exclusions.
pub fn excluded_timing_path_report(nl: &Netlist, paths: &[ExcludedTimingPath]) -> String {
    eda_sta::excluded_timing_path_report(nl, paths)
}

/// Build a persistent scenario timing-violation store.
pub fn timing_violation_store(
    scenario: &str,
    nl: &Netlist,
    timing: &ConstrainedTiming,
) -> Result<TimingViolationStore, String> {
    eda_sta::TimingViolationStore::from_timing(scenario, nl, timing)
}

/// Incrementally update a persistent timing-violation store.
pub fn update_timing_violations(
    store: &mut TimingViolationStore,
    nl: &Netlist,
    timing: &ConstrainedTiming,
) -> Result<TimingViolationDiff, String> {
    store.update(nl, timing)
}

/// Update only timing violations touched by stable-name dirty objects.
///
/// Untouched negative checks retain their previous slack and lifecycle state.
/// A dirty clock conservatively updates every check governed by that clock.
pub fn update_timing_violations_incremental(
    store: &mut TimingViolationStore,
    nl: &Netlist,
    timing: &ConstrainedTiming,
    dirty: &TimingViolationDirtyObjects,
) -> Result<TimingViolationDiff, String> {
    store.update_dirty(nl, timing, dirty)
}

/// Render a timing-violation store and optional lifecycle diff.
pub fn timing_violation_report(
    store: &TimingViolationStore,
    diff: Option<&TimingViolationDiff>,
) -> String {
    eda_sta::timing_violation_report(store, diff)
}

/// Build a validated timing-violation waiver.
pub fn timing_violation_waiver(
    id: &str,
    scenario_pattern: &str,
    endpoint_pattern: &str,
    slack_floor_ns: f64,
) -> Result<TimingViolationWaiver, String> {
    eda_sta::TimingViolationWaiver::new(id, scenario_pattern, endpoint_pattern, slack_floor_ns)
}

/// Add a waiver to one scenario's persistent violation store.
pub fn add_timing_violation_waiver(
    store: &mut TimingViolationStore,
    waiver: TimingViolationWaiver,
) -> Result<(), String> {
    store.add_waiver(waiver)
}

/// Add a waiver to one scenario store with an audit actor.
pub fn add_timing_violation_waiver_with_actor(
    store: &mut TimingViolationStore,
    waiver: TimingViolationWaiver,
    actor: Option<&str>,
) -> Result<(), String> {
    store.add_waiver_with_actor(waiver, actor.map(str::to_string))
}

/// Remove a waiver from one scenario's persistent violation store.
pub fn remove_timing_violation_waiver(
    store: &mut TimingViolationStore,
    id: &str,
) -> Result<(), String> {
    store.remove_waiver(id)
}

/// Remove a waiver from one scenario store with an audit actor.
pub fn remove_timing_violation_waiver_with_actor(
    store: &mut TimingViolationStore,
    id: &str,
    actor: Option<&str>,
) -> Result<(), String> {
    store.remove_waiver_with_actor(id, actor.map(str::to_string))
}

/// Serialize timing-violation waivers.
pub fn write_timing_violation_waivers(waivers: &[TimingViolationWaiver]) -> Result<String, String> {
    eda_sta::write_timing_violation_waivers(waivers)
}

/// Parse serialized timing-violation waivers.
pub fn parse_timing_violation_waivers(source: &str) -> Result<Vec<TimingViolationWaiver>, String> {
    eda_sta::parse_timing_violation_waivers(source)
}

/// Render store waiver audit history.
pub fn timing_waiver_audit_report(entries: &[TimingWaiverAuditEntry]) -> String {
    eda_sta::timing_waiver_audit_report(entries)
}

/// Serialize waiver add/remove audit history.
pub fn write_timing_waiver_audit(entries: &[TimingWaiverAuditEntry]) -> Result<String, String> {
    eda_sta::write_timing_waiver_audit(entries)
}

/// Parse serialized waiver audit history.
pub fn parse_timing_waiver_audit(source: &str) -> Result<Vec<TimingWaiverAuditEntry>, String> {
    eda_sta::parse_timing_waiver_audit(source)
}

/// Replay waiver audit history and return active waivers.
pub fn active_timing_waivers_from_audit(
    entries: &[TimingWaiverAuditEntry],
) -> Result<Vec<TimingViolationWaiver>, String> {
    eda_sta::active_timing_waivers_from_audit(entries)
}

/// Aggregate waiver audit events by hierarchy-instance selector.
pub fn timing_waiver_audit_hierarchy_summaries(
    entries: &[TimingWaiverAuditEntry],
) -> Result<std::collections::BTreeMap<Option<String>, TimingWaiverAuditHierarchySummary>, String> {
    eda_sta::timing_waiver_audit_hierarchy_summaries(entries)
}

/// Render hierarchy-scoped waiver audit aggregation.
pub fn timing_waiver_audit_hierarchy_report(
    entries: &[TimingWaiverAuditEntry],
) -> Result<String, String> {
    eda_sta::timing_waiver_audit_hierarchy_report(entries)
}

/// Restore a serialized waiver audit into one scenario store.
pub fn restore_timing_waiver_audit(
    store: &mut TimingViolationStore,
    entries: &[TimingWaiverAuditEntry],
) -> Result<(), String> {
    store.restore_waiver_audit(entries)
}

/// Build a persistent multi-scenario timing-violation registry.
pub fn timing_violation_registry<'a, I>(scenarios: I) -> Result<TimingViolationRegistry, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    eda_sta::TimingViolationRegistry::from_scenarios(scenarios)
}

/// Build an MMMC registry snapshot while preserving scenario-local waivers.
pub fn timing_violation_registry_from_stores<I>(
    stores: I,
) -> Result<TimingViolationRegistry, String>
where
    I: IntoIterator<Item = (String, TimingViolationStore)>,
{
    eda_sta::timing_violation_registry_from_stores(stores)
}

/// Replace all stores in a persistent MMMC registry from materialized stores.
pub fn update_timing_violation_registry_from_stores<I>(
    registry: &mut TimingViolationRegistry,
    stores: I,
) -> Result<TimingViolationRegistryUpdate, String>
where
    I: IntoIterator<Item = (String, TimingViolationStore)>,
{
    registry.replace_stores(stores)
}

/// Replace the scenario set in a persistent multi-scenario violation registry.
pub fn update_timing_violation_registry<'a, I>(
    registry: &mut TimingViolationRegistry,
    scenarios: I,
) -> Result<TimingViolationRegistryUpdate, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    registry.replace_scenarios(scenarios)
}

/// Update one MMMC scenario using stable-name dirty timing objects.
pub fn update_timing_violation_registry_scenario_incremental(
    registry: &mut TimingViolationRegistry,
    scenario: TimingScenario<'_>,
    dirty: &TimingViolationDirtyObjects,
) -> Result<TimingViolationDiff, String> {
    registry.update_scenario_dirty(scenario, dirty)
}

/// Add a waiver to every scenario in a multi-corner violation registry.
pub fn add_timing_violation_registry_waiver(
    registry: &mut TimingViolationRegistry,
    waiver: TimingViolationWaiver,
) -> Result<(), String> {
    registry.add_waiver(waiver)
}

/// Add a registry waiver with an audit actor.
pub fn add_timing_violation_registry_waiver_with_actor(
    registry: &mut TimingViolationRegistry,
    waiver: TimingViolationWaiver,
    actor: Option<&str>,
) -> Result<(), String> {
    registry.add_waiver_with_actor(waiver, actor.map(str::to_string))
}

/// Remove a waiver from every scenario in a multi-corner violation registry.
pub fn remove_timing_violation_registry_waiver(
    registry: &mut TimingViolationRegistry,
    id: &str,
) -> Result<(), String> {
    registry.remove_waiver(id)
}

/// Remove a registry waiver with an audit actor.
pub fn remove_timing_violation_registry_waiver_with_actor(
    registry: &mut TimingViolationRegistry,
    id: &str,
    actor: Option<&str>,
) -> Result<(), String> {
    registry.remove_waiver_with_actor(id, actor.map(str::to_string))
}

/// Restore serialized waiver audit history across a violation registry.
pub fn restore_timing_waiver_registry_audit(
    registry: &mut TimingViolationRegistry,
    entries: &[TimingWaiverAuditEntry],
) -> Result<(), String> {
    registry.restore_waiver_audit(entries)
}

/// Render a multi-scenario timing-violation registry.
pub fn timing_violation_registry_report(
    registry: &TimingViolationRegistry,
    update: Option<&TimingViolationRegistryUpdate>,
) -> String {
    eda_sta::timing_violation_registry_report(registry, update)
}

/// Analyze named mode/corner scenarios and aggregate the worst results.
pub fn analyze_timing_scenarios<'a, I>(scenarios: I) -> Result<MultiCornerTimingResult, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    eda_sta::analyze_timing_scenarios(scenarios)
}

/// Parse inline named timing-mode/corner definitions.
pub fn parse_timing_scenarios(source: &str) -> Result<Vec<TimingScenarioDefinition>, String> {
    eda_sta::parse_timing_scenarios(source)
}

/// Analyze inline timing scenarios against one mapped netlist.
pub fn analyze_timing_scenarios_from_definitions(
    netlist: &Netlist,
    scenarios: &[TimingScenarioDefinition],
) -> Result<MultiCornerTimingResult, String> {
    eda_sta::analyze_timing_scenario_definitions(netlist, scenarios)
}

/// Compare named mode/corner scenario definitions through the unified API.
pub fn diff_timing_scenarios(
    old: &[TimingScenarioDefinition],
    new: &[TimingScenarioDefinition],
) -> Result<TimingScenarioDiff, String> {
    eda_sta::diff_timing_scenarios(old, new)
}

/// Render an MMMC scenario diff through the unified API.
pub fn timing_scenario_diff_report(diff: &TimingScenarioDiff) -> String {
    eda_sta::timing_scenario_diff_report(diff)
}

/// Incrementally update multi-corner timing after scenario/SDC changes.
pub fn update_timing_scenarios_incrementally(
    netlist: &Netlist,
    previous: &MultiCornerTimingResult,
    old: &[TimingScenarioDefinition],
    new: &[TimingScenarioDefinition],
) -> Result<IncrementalMultiCornerTimingResult, String> {
    eda_sta::update_timing_scenarios_incrementally(netlist, previous, old, new)
}

/// Compare scenario sets that may use different netlists per corner.
pub fn diff_timing_scenario_sets<'old, 'new>(
    old: &[TimingScenario<'old>],
    new: &[TimingScenario<'new>],
) -> Result<MultiCornerScenarioSetDiff, String> {
    eda_sta::diff_timing_scenario_sets(old, new)
}

/// Incrementally update MMMC scenarios with per-corner netlist topology changes.
pub fn update_timing_scenarios_with_topology_incrementally<'old, 'new>(
    previous: &MultiCornerTimingResult,
    old: &[TimingScenario<'old>],
    new: &[TimingScenario<'new>],
) -> Result<IncrementalMultiCornerTopologyTimingResult, String> {
    eda_sta::update_timing_scenarios_with_topology_incrementally(previous, old, new)
}

/// Render a multi-scenario STA report.
pub fn multi_corner_timing_report(result: &MultiCornerTimingResult) -> String {
    eda_sta::multi_corner_report(result)
}

/// Merge setup/output timing paths across named scenarios through the API.
pub fn multi_corner_setup_timing_paths<'a, I>(
    result: &MultiCornerTimingResult,
    scenarios: I,
    max_paths: usize,
) -> Result<Vec<MultiCornerSetupTimingPath<'a>>, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    eda_sta::multi_corner_setup_timing_paths(result, scenarios, max_paths)
}

/// Render merged multi-scenario setup/output paths.
pub fn multi_corner_setup_timing_path_report(paths: &[MultiCornerSetupTimingPath<'_>]) -> String {
    eda_sta::multi_corner_setup_timing_path_report(paths)
}

/// Merge hold timing paths across named scenarios through the API.
pub fn multi_corner_hold_timing_paths<'a, I>(
    result: &MultiCornerTimingResult,
    scenarios: I,
    max_paths: usize,
) -> Result<Vec<MultiCornerHoldTimingPath<'a>>, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    eda_sta::multi_corner_hold_timing_paths(result, scenarios, max_paths)
}

/// Render merged multi-scenario hold paths.
pub fn multi_corner_hold_timing_path_report(paths: &[MultiCornerHoldTimingPath<'_>]) -> String {
    eda_sta::multi_corner_hold_timing_path_report(paths)
}

/// Merge recovery/removal timing paths across named scenarios through the API.
pub fn multi_corner_reset_timing_paths<'a, I>(
    result: &MultiCornerTimingResult,
    scenarios: I,
    max_paths: usize,
) -> Result<Vec<MultiCornerResetTimingPath<'a>>, String>
where
    I: IntoIterator<Item = TimingScenario<'a>>,
{
    eda_sta::multi_corner_reset_timing_paths(result, scenarios, max_paths)
}

/// Render merged multi-scenario recovery/removal paths.
pub fn multi_corner_reset_timing_path_report(paths: &[MultiCornerResetTimingPath<'_>]) -> String {
    eda_sta::multi_corner_reset_timing_path_report(paths)
}

/// Run transition, capacitance, and fanout signal-quality checks.
pub fn analyze_signal_quality(
    netlist: &Netlist,
    options: &QualityOptions,
) -> Result<QualityResult, String> {
    eda_sta::analyze_quality(netlist, options)
}

/// Derive design-wide signal-quality options from parsed SDC constraints.
pub fn quality_options_from_sdc(constraints: &TimingConstraints) -> QualityOptions {
    constraints.quality_options()
}

/// Run signal-quality checks with SPEF grounded/coupling capacitance included.
pub fn analyze_signal_quality_with_parasitics(
    netlist: &Netlist,
    parasitics: &ParasiticReport,
    options: &QualityOptions,
) -> Result<QualityResult, String> {
    let mut options = options.clone();
    for net in &parasitics.nets {
        let effective_cap = net.grounded_cap_pf + 0.5 * net.coupling_cap_pf;
        options
            .extra_capacitance_pf
            .insert(net.net.clone(), effective_cap);
    }
    eda_sta::analyze_quality(netlist, &options)
}

/// Parse an SDF back-annotation file through the unified API.
pub fn parse_delay_file(source: &str) -> Result<Sdf, String> {
    eda_sdf::parse_sdf(source)
}

/// Serialize an in-memory SDF through the unified API.
///
/// Unlike [`write_sdf`], this preserves parsed or extracted `INTERCONNECT`
/// timing rather than regenerating nominal library delays.
pub fn write_delay_file(sdf: &Sdf) -> Result<String, String> {
    eda_sdf::write_delay_file(sdf)
}

/// One prepared per-corner SDF output from an MMMC physical implementation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiCornerDelayFileExport {
    pub scenario: String,
    pub path: PathBuf,
    pub text: String,
}

/// Serialize every timing view of an MMMC physical implementation without
/// writing files.
///
/// The template must contain `{corner}` exactly as a placeholder for the
/// stable scenario name, for example `timing/best.{corner}.sdf`.
pub fn prepare_multi_corner_delay_file_exports(
    corners: &[MultiCornerPhysicalTimingCorner],
    path_template: &str,
) -> Result<Vec<MultiCornerDelayFileExport>, String> {
    if corners.is_empty() {
        return Err("MMMC SDF export has no timing corner".into());
    }
    if !path_template.contains("{corner}") {
        return Err(
            "MMMC SDF export template requires a '{corner}' placeholder, for example best.{corner}.sdf"
                .into(),
        );
    }

    let mut scenarios = Vec::new();
    let mut out = Vec::new();
    for corner in corners {
        let scenario = &corner.scenario;
        if scenario.is_empty()
            || scenario == "."
            || scenario == ".."
            || scenario.trim() != scenario
            || scenario.contains('/')
            || scenario.contains('\\')
            || scenario.contains('\0')
        {
            return Err(format!(
                "MMMC SDF export requires safe scenario names; got '{}'",
                corner.scenario
            ));
        }
        if scenarios.iter().any(|name: &String| name == scenario) {
            return Err(format!("duplicate MMMC SDF export scenario '{}'", scenario));
        }
        scenarios.push(scenario.clone());

        let path = PathBuf::from(path_template.replace("{corner}", scenario));
        let text = write_delay_file(&corner.evaluation.sdf)?;
        if out.iter().any(|export: &MultiCornerDelayFileExport| {
            export.path == path || export.scenario == scenario.as_str()
        }) {
            return Err(format!(
                "MMMC SDF template '{}' does not produce unique files",
                path_template
            ));
        }
        out.push(MultiCornerDelayFileExport {
            scenario: scenario.clone(),
            path,
            text,
        });
    }
    Ok(out)
}

/// Write prepared MMMC SDF outputs and return their paths.
pub fn write_multi_corner_delay_file_exports(
    exports: &[MultiCornerDelayFileExport],
) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for export in exports {
        if paths.iter().any(|path: &PathBuf| path == &export.path) {
            return Err(format!(
                "duplicate MMMC SDF output path '{}'",
                export.path.display()
            ));
        }
        paths.push(export.path.clone());
    }
    for export in exports {
        std::fs::write(&export.path, &export.text).map_err(|error| {
            format!(
                "cannot write SDF for corner '{}' to {}: {}",
                export.scenario,
                export.path.display(),
                error
            )
        })?;
    }
    Ok(paths)
}

/// Apply SDF timing annotations to a mapped netlist.
pub fn back_annotate(netlist: &Netlist, annotations: &Sdf) -> Result<Netlist, String> {
    eda_sdf::annotate_sdf(netlist, annotations)
}

/// Parse a SPEF parasitic-extraction file through the unified API.
pub fn parse_parasitics(source: &str) -> Result<Spef, String> {
    eda_spef::parse_spef(source)
}

/// Analyze SPEF parasitics against a mapped netlist.
pub fn parasitic_analysis(netlist: &Netlist, parasitics: &Spef) -> Result<ParasiticReport, String> {
    eda_spef::analyze(netlist, parasitics)
}

/// Convert SPEF RC estimates into SDF INTERCONNECT annotations.
pub fn parasitics_to_sdf(netlist: &Netlist, parasitics: &Spef) -> Result<String, String> {
    eda_spef::write_sdf(netlist, parasitics)
}

/// Physical verification through the unified API: placement legality,
/// routing legality, opens, dangling wires, and conservative antenna checks.
pub fn physical_verification(
    nl: &Netlist,
    placement: &Placement,
    routing: &Routing,
) -> Result<DrcResult, String> {
    eda_pnr::drc(nl, placement, routing)
}

/// Physical verification with a foundry-specific maximum antenna ratio.
pub fn physical_verification_with_options(
    nl: &Netlist,
    placement: &Placement,
    routing: &Routing,
    options: &DrcOptions,
) -> Result<DrcResult, String> {
    eda_pnr::drc_with_options(nl, placement, routing, options)
}

/// Options for automatic antenna-ratio repair.
#[derive(Clone, Debug)]
pub struct AntennaRepairOptions {
    pub patch_name: String,
    pub max_diodes_per_net: usize,
    pub route: RouteOptions,
}

impl Default for AntennaRepairOptions {
    fn default() -> Self {
        Self {
            patch_name: "antenna_repair".into(),
            max_diodes_per_net: 4,
            route: RouteOptions::default(),
        }
    }
}

impl AntennaRepairOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.patch_name.trim().is_empty() {
            return Err("antenna repair patch name cannot be empty".into());
        }
        if self.max_diodes_per_net == 0 {
            return Err("antenna repair requires a positive diode limit".into());
        }
        self.route.validate()
    }
}

/// One repaired antenna-violating net.
#[derive(Clone, Debug)]
pub struct AntennaRepair {
    pub net: String,
    pub old_ratio: f64,
    pub new_ratio: f64,
    pub diodes_inserted: usize,
}

/// Result of antenna diode insertion and post-repair routing.
#[derive(Clone, Debug)]
pub struct AntennaRepairResult {
    pub netlist: Netlist,
    pub physical_placement: Placement,
    pub routing: Routing,
    pub repairs: Vec<AntennaRepair>,
    pub diodes_inserted: usize,
    pub antenna_warnings: usize,
    pub all_antennas_repaired: bool,
}

/// End-to-end RTL physical-implementation antenna repair result.
#[derive(Clone, Debug)]
pub struct RtlAntennaRepairResult {
    pub baseline_drc: DrcResult,
    pub repair: AntennaRepairResult,
    pub final_mapped_stats: NetlistStats,
}

/// Compile RTL, place/route it, verify antenna ratios, and insert physical DIODEs.
pub fn repair_rtl_antennas(
    src: &str,
    top: Option<&str>,
    options: &AntennaRepairOptions,
) -> Result<RtlAntennaRepairResult, String> {
    repair_rtl_antennas_cancellable(src, top, options, &FlowCancellation::new())
}

pub fn repair_rtl_antennas_cancellable(
    src: &str,
    top: Option<&str>,
    options: &AntennaRepairOptions,
    cancellation: &FlowCancellation,
) -> Result<RtlAntennaRepairResult, String> {
    options.validate()?;
    if cancellation.is_cancelled() {
        return Err("RTL antenna repair cancelled".into());
    }
    let design = compile(src, top)?;
    if cancellation.is_cancelled() {
        return Err("RTL antenna repair cancelled".into());
    }
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &CellLibrary::std())?
            .netlist;
    if cancellation.is_cancelled() {
        return Err("RTL antenna repair cancelled".into());
    }
    let placement = eda_pnr::place(&netlist)?;
    if cancellation.is_cancelled() {
        return Err("RTL antenna repair cancelled".into());
    }
    let routing = eda_pnr::route_maze_with_options_cancellable(
        &netlist,
        &placement,
        &options.route,
        cancellation,
    )?;
    if cancellation.is_cancelled() {
        return Err("RTL antenna repair cancelled".into());
    }
    let baseline_drc = physical_verification(&netlist, &placement, &routing)?;
    if baseline_drc.has_errors() {
        return Err(format!(
            "RTL antenna repair requires an error-free route: {}",
            eda_pnr::drc_report(&baseline_drc)
        ));
    }
    let repair = repair_antennas(&netlist, &placement, &routing, options)?;
    if cancellation.is_cancelled() {
        return Err("RTL antenna repair cancelled".into());
    }
    let final_mapped_stats = repair.netlist.stats();
    Ok(RtlAntennaRepairResult {
        baseline_drc,
        repair,
        final_mapped_stats,
    })
}

/// Repair conservative antenna-ratio warnings with physical DIODE loads.
///
/// The repair loop reroutes the complete netlist after every diode-insertion
/// round. Diodes are placed in legal free sites and connected by the detailed
/// maze router; they are not silently counted as logical fanout without a
/// physical path.
pub fn repair_antennas(
    netlist: &Netlist,
    placement: &Placement,
    routing: &Routing,
    options: &AntennaRepairOptions,
) -> Result<AntennaRepairResult, String> {
    options.validate()?;
    let initial_drc = physical_verification(netlist, placement, routing)?;
    if initial_drc.has_errors() {
        return Err(format!(
            "antenna repair requires an error-free route: {}",
            eda_pnr::drc_report(&initial_drc)
        ));
    }

    let mut out = netlist.clone();
    let mut current_placement = placement.clone();
    let mut current_routing = routing.clone();
    let mut inserted = std::collections::BTreeMap::<usize, usize>::new();
    let mut old_ratios = std::collections::BTreeMap::<usize, f64>::new();
    for violation in &initial_drc.violations {
        if violation.code == "ANT-001" {
            if let Some(net) = violation.net {
                old_ratios.insert(
                    net,
                    antenna_ratio(&out, &current_placement, &current_routing, net),
                );
            }
        }
    }

    let mut final_drc = initial_drc;
    loop {
        let violating_nets = final_drc
            .violations
            .iter()
            .filter(|violation| violation.code == "ANT-001")
            .filter_map(|violation| violation.net)
            .collect::<Vec<_>>();
        if violating_nets.is_empty() {
            break;
        }

        let mut inserted_this_round = false;
        for net in violating_nets {
            let count = inserted.get(&net).copied().unwrap_or(0);
            if count >= options.max_diodes_per_net {
                continue;
            }
            insert_antenna_diode(&mut out, net, &options.patch_name, count)?;
            inserted.insert(net, count + 1);
            inserted_this_round = true;
        }
        if !inserted_this_round {
            break;
        }

        current_placement = extend_physical_placement(&out, &current_placement)?;
        current_routing =
            eda_pnr::route_maze_with_options(&out, &current_placement, &options.route)?;
        final_drc = physical_verification(&out, &current_placement, &current_routing)?;
        if final_drc.has_errors() {
            return Err(format!(
                "antenna repair created DRC errors: {}",
                eda_pnr::drc_report(&final_drc)
            ));
        }
    }

    let mut repairs = Vec::new();
    for (net, old_ratio) in old_ratios {
        repairs.push(AntennaRepair {
            net: out.net(net).name.clone(),
            old_ratio,
            new_ratio: antenna_ratio(&out, &current_placement, &current_routing, net),
            diodes_inserted: inserted.get(&net).copied().unwrap_or(0),
        });
    }
    let antenna_warnings = final_drc
        .violations
        .iter()
        .filter(|violation| violation.code == "ANT-001")
        .count();
    let diodes_inserted = inserted.values().sum();
    Ok(AntennaRepairResult {
        netlist: out,
        physical_placement: current_placement,
        routing: current_routing,
        repairs,
        diodes_inserted,
        antenna_warnings,
        all_antennas_repaired: antenna_warnings == 0,
    })
}

fn antenna_ratio(nl: &Netlist, placement: &Placement, routing: &Routing, net: usize) -> f64 {
    let wirelength = routing
        .segments
        .iter()
        .filter(|segment| segment.net == net)
        .map(|segment| segment.length())
        .sum::<usize>();
    let mut pins = std::collections::BTreeSet::new();
    let physical_net = nl.net(net);
    let push_pin = |cell: usize, pins: &mut std::collections::BTreeSet<(usize, usize)>| {
        if let Some(position) = placement.cell_pos.get(cell) {
            let width = placement.cell_width.get(cell).copied().unwrap_or(1);
            let x = (position.site as f64 + width as f64 / 2.0).round() as usize;
            pins.insert((x, position.row));
        }
    };
    for driver in &physical_net.drivers {
        push_pin(*driver, &mut pins);
    }
    for sink in &physical_net.sinks {
        push_pin(*sink, &mut pins);
    }
    wirelength as f64 / pins.len().max(1) as f64
}

fn insert_antenna_diode(
    netlist: &mut Netlist,
    net: usize,
    patch_name: &str,
    index: usize,
) -> Result<(), String> {
    let diode = netlist
        .library()
        .and_then(|library| library.find("DIODE"))
        .ok_or_else(|| "antenna repair requires a library cell named 'DIODE'".to_string())?;
    let safe_net = netlist
        .net(net)
        .name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    let safe_patch = patch_name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    let cell_name = format!("{}_{}_{}", safe_patch, safe_net, index);
    let output_name = format!("{}_net", cell_name);
    if netlist.net_by_name(&output_name).is_some()
        || (0..netlist.cell_count()).any(|cell| netlist.cell(cell).name == cell_name)
    {
        return Err(format!(
            "antenna repair name '{}' is already used",
            cell_name
        ));
    }
    let output = netlist.add_net(output_name);
    netlist.add_cell(eda_core::netlist::Cell {
        name: cell_name,
        kind: eda_core::netlist::CellKind::Lib(diode),
        inputs: vec![net],
        output,
    });
    Ok(())
}

/// Render antenna repair actions and post-repair status.
pub fn antenna_repair_report(result: &AntennaRepairResult) -> String {
    let mut out = format!(
        "Antenna repair: {} diode(s), {} net(s), status {}\n",
        result.diodes_inserted,
        result.repairs.len(),
        if result.all_antennas_repaired {
            "PASS"
        } else {
            "LIMITED"
        }
    );
    for repair in &result.repairs {
        out.push_str(&format!(
            "  net '{}': {} diode(s), ratio {:.3} -> {:.3}\n",
            repair.net, repair.diodes_inserted, repair.old_ratio, repair.new_ratio
        ));
    }
    out
}

/// Structural layout-versus-schematic check through the unified API.
pub fn layout_vs_schematic(reference: &Netlist, layout: &Netlist) -> Result<LvsResult, String> {
    eda_lvs::check_lvs(reference, layout)
}

/// Export the physical view of a placed/routed netlist as LEF/DEF text.
pub fn export_physical(
    nl: &Netlist,
    placement: &Placement,
    routing: &Routing,
) -> Result<(String, String), String> {
    Ok((
        eda_pnr::write_lef(nl, placement)?,
        eda_pnr::write_def(nl, placement, routing)?,
    ))
}

/// Parse the compact DEF subset through the unified API.
pub fn parse_physical_def(source: &str) -> Result<eda_pnr::Def, String> {
    eda_pnr::parse_def(source)
}

/// Copy placed-component coordinates from a DEF view into AMS element
/// locations for spatial-mismatch Monte Carlo.
///
/// Components are matched to SPICE elements by case-insensitive stable name.
/// Coordinates use the DEF database-unit scale supplied by the caller. The
/// compact DEF parser currently preserves component coordinates but not its
/// `UNITS DISTANCE MICRONS` statement, so the scale cannot be inferred
/// automatically.
pub fn apply_physical_def_locations(
    circuit: &mut eda_ams::SpiceCircuit,
    def: &eda_pnr::Def,
    database_units_per_micron: f64,
) -> Result<usize, String> {
    if !database_units_per_micron.is_finite() || database_units_per_micron <= 0.0 {
        return Err("DEF database units per micron must be finite and positive".into());
    }
    let meters_per_database_unit = 1.0e-6 / database_units_per_micron;
    let mut matched = 0usize;
    for component in &def.components {
        let Some(element) = circuit
            .elements
            .iter()
            .find(|element| element.name.eq_ignore_ascii_case(&component.name))
        else {
            continue;
        };
        let location = eda_ams::ElementLocation {
            x_m: component.x as f64 * meters_per_database_unit,
            y_m: component.y as f64 * meters_per_database_unit,
        };
        circuit.locations.insert(element.name.clone(), location);
        matched += 1;
    }
    if matched == 0 {
        return Err("DEF has no components matching SPICE element names".into());
    }
    circuit.validate()?;
    Ok(matched)
}

/// Parse and reconstruct placement/routing from compact DEF text.
pub fn import_physical_def_text(
    netlist: &Netlist,
    source: &str,
) -> Result<(Placement, Routing), String> {
    let def = parse_physical_def(source)?;
    import_physical_def(netlist, &def)
}

/// Reconstruct placement and routing from a parsed compact DEF subset.
pub fn import_physical_def(
    netlist: &Netlist,
    def: &eda_pnr::Def,
) -> Result<(Placement, Routing), String> {
    eda_pnr::physical_from_def(netlist, def)
}

/// Gate-level switching-power analysis through the unified API.
pub fn analyze_power(
    nl: &Netlist,
    vectors: &[InputVector],
    opts: &PowerOptions,
) -> Result<PowerReport, String> {
    eda_power::analyze(nl, vectors, opts)
}

/// Estimate switching power with explicit per-net switched capacitance.
pub fn analyze_power_with_capacitances(
    nl: &Netlist,
    vectors: &[InputVector],
    capacitance_pf: &BTreeMap<String, f64>,
    opts: &PowerOptions,
) -> Result<PowerReport, String> {
    eda_power::analyze_with_capacitances(nl, vectors, capacitance_pf, opts)
}

/// Estimate waveform-driven power with explicit switched capacitance.
pub fn power_from_waveform_with_capacitances(
    nl: &Netlist,
    waveform: &Vcd,
    capacitance_pf: &BTreeMap<String, f64>,
    opts: &PowerOptions,
) -> Result<PowerReport, String> {
    eda_power::analyze_waveform_with_capacitances(nl, waveform, capacitance_pf, opts)
}

/// Generate a deterministic zero → all-ones stimulus pair for a quick power
/// estimate when no application-specific VCD/activity vectors are available.
pub fn default_power_vectors(nl: &Netlist) -> Vec<InputVector> {
    let zero: InputVector = nl
        .ports
        .iter()
        .filter(|p| p.dir == eda_core::netlist::Dir::In)
        .map(|p| (p.name.clone(), LogicWord::zero(p.bits.len())))
        .collect();
    let ones: InputVector = nl
        .ports
        .iter()
        .filter(|p| p.dir == eda_core::netlist::Dir::In)
        .map(|p| (p.name.clone(), LogicWord::ones(p.bits.len())))
        .collect();
    vec![zero, ones]
}

/// Quick default-vector power estimate through the unified API.
pub fn quick_power(nl: &Netlist, opts: &PowerOptions) -> Result<PowerReport, String> {
    let vectors = default_power_vectors(nl);
    analyze_power(nl, &vectors, opts)
}

/// Options for post-route power and power-grid integrity analysis.
#[derive(Clone, Debug)]
pub struct PostRoutePowerIrOptions {
    pub power: PowerOptions,
    pub ir: IrOptions,
    /// Domain-specific supplies and rails for UPF-aware IR analysis.
    pub upf: UpfIrOptions,
    /// Optional finite-difference VDD/GND mesh analysis.
    pub power_mesh: Option<PowerMeshOptions>,
    /// Optional layer/via-aware finite-difference mesh analysis.
    pub multilayer_power_mesh: Option<MultilayerPowerMeshOptions>,
    /// Derive `C*V^2` switched-capacitance energy from the IR supply voltage.
    ///
    /// When disabled, `power.capacitive_energy_per_pf_pj` is used unchanged.
    pub voltage_scaled_capacitance: bool,
}

impl Default for PostRoutePowerIrOptions {
    fn default() -> Self {
        Self {
            power: PowerOptions::default(),
            ir: IrOptions::default(),
            upf: UpfIrOptions::default(),
            power_mesh: None,
            multilayer_power_mesh: None,
            voltage_scaled_capacitance: true,
        }
    }
}

/// Post-route switching power and IR/EM result for one physical iteration.
#[derive(Clone, Debug)]
pub struct PostRoutePowerIrResult {
    pub power: PowerReport,
    pub ir: IrResult,
    /// Domain-resolved result when a UPF power intent was supplied.
    pub upf_ir: Option<UpfIrResult>,
    /// Optional post-route finite-difference mesh result.
    pub power_mesh: Option<PowerMeshResult>,
    /// Optional post-route layer/via-aware finite-difference mesh result.
    pub multilayer_power_mesh: Option<MultilayerPowerMeshResult>,
    /// Dynamic-power increase caused by extracted switched capacitance.
    pub parasitic_dynamic_power_uw: f64,
    /// Effective capacitance used per stable net name.
    pub effective_capacitance_pf: BTreeMap<String, f64>,
}

/// One named post-route power/IR operating condition.
#[derive(Clone, Debug)]
pub struct MultiCornerPostRoutePowerIrScenario {
    pub name: String,
    pub options: PostRoutePowerIrOptions,
}

impl MultiCornerPostRoutePowerIrScenario {
    pub fn new(name: impl Into<String>, options: PostRoutePowerIrOptions) -> Result<Self, String> {
        let name = name.into();
        if name.trim().is_empty()
            || name.trim() != name
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
            || name.contains('\0')
        {
            return Err(format!("invalid post-route power scenario name '{}'", name));
        }
        Ok(Self { name, options })
    }
}

/// Convert generic MMMC scenario properties into post-route power options.
///
/// Properties use the `post_route.` prefix. For example:
///
/// ```text
/// scenario slow
///     set_scenario_property post_route.voltage_v 0.90
///     set_scenario_property post_route.vdd_ohm_per_site 0.05
///     create_clock -name clk -period 4.0 [get_ports clk]
/// endscenario
/// ```
pub fn post_route_power_scenarios_from_timing_scenarios(
    definitions: &[TimingScenarioDefinition],
) -> Result<Vec<MultiCornerPostRoutePowerIrScenario>, String> {
    let mut out = Vec::with_capacity(definitions.len());
    for definition in definitions {
        let mut options = PostRoutePowerIrOptions::default();
        for (property, text) in &definition.properties {
            let Some(key) = property.strip_prefix("post_route.") else {
                continue;
            };
            if key.starts_with("domain.") {
                continue;
            }
            let parse_f64 = || -> Result<f64, String> {
                text.parse::<f64>()
                    .map_err(|_| format!("invalid post-route property '{}': '{}'", property, text))
            };
            match key {
                "frequency_mhz" => options.power.frequency_mhz = parse_f64()?,
                "base_cell_energy_pj" => options.power.base_cell_energy_pj = parse_f64()?,
                "energy_per_site_pj" => options.power.energy_per_site_pj = parse_f64()?,
                "capacitive_energy_per_pf_pj" => {
                    options.power.capacitive_energy_per_pf_pj = parse_f64()?
                }
                "leakage_per_site_nw" => options.power.leakage_per_site_nw = parse_f64()?,
                "voltage_scaled_capacitance" => {
                    options.voltage_scaled_capacitance = match text.as_str() {
                        "true" => true,
                        "false" => false,
                        _ => {
                            return Err(format!(
                                "post-route property '{}' must be true or false",
                                property
                            ))
                        }
                    }
                }
                "voltage_v" => options.ir.voltage_v = parse_f64()?,
                "vdd_ohm_per_site" => options.ir.vdd_ohm_per_site = parse_f64()?,
                "gnd_ohm_per_site" => options.ir.gnd_ohm_per_site = parse_f64()?,
                "max_total_drop_v" => options.ir.max_total_drop_v = parse_f64()?,
                "max_rail_current_a" => options.ir.max_rail_current_a = parse_f64()?,
                _ => {
                    return Err(format!(
                        "unknown post-route scenario property '{}'",
                        property
                    ))
                }
            }
        }

        let default_domain = DomainIrOptions::from(options.ir.clone());
        for (property, text) in &definition.properties {
            let Some(key) = property.strip_prefix("post_route.domain.") else {
                continue;
            };
            let Some((domain, field)) = key.rsplit_once('.') else {
                return Err(format!(
                    "invalid post-route domain property '{}'; expected post_route.domain.<name>.<field>",
                    property
                ));
            };
            if domain.is_empty() || field.is_empty() {
                return Err(format!(
                    "invalid post-route domain property '{}'; domain and field names cannot be empty",
                    property
                ));
            }
            let value = text.parse::<f64>().map_err(|_| {
                format!(
                    "invalid post-route domain property '{}': '{}'",
                    property, text
                )
            })?;
            let domain_options = options
                .upf
                .domains
                .entry(domain.to_string())
                .or_insert(default_domain);
            match field {
                "voltage_v" => domain_options.voltage_v = value,
                "vdd_ohm_per_site" => domain_options.vdd_ohm_per_site = value,
                "gnd_ohm_per_site" => domain_options.gnd_ohm_per_site = value,
                "max_total_drop_v" => domain_options.max_total_drop_v = value,
                "max_rail_current_a" => domain_options.max_rail_current_a = value,
                _ => return Err(format!("unknown post-route domain property '{}'", property)),
            }
        }
        if options.upf.default.is_none() {
            options.upf.default = Some(default_domain);
        }
        validate_post_route_power_ir_options(&options).map_err(|error| {
            format!(
                "invalid post-route options for scenario '{}': {}",
                definition.name, error
            )
        })?;
        out.push(MultiCornerPostRoutePowerIrScenario::new(
            definition.name.clone(),
            options,
        )?);
    }
    Ok(out)
}

/// One scenario result in a multi-corner post-route power/IR analysis.
#[derive(Clone, Debug)]
pub struct MultiCornerPostRoutePowerIrCorner {
    pub scenario: String,
    pub result: PostRoutePowerIrResult,
}

/// Per-corner post-route power and power-grid results for one implementation.
#[derive(Clone, Debug)]
pub struct MultiCornerPostRoutePowerIrResult {
    pub corners: Vec<MultiCornerPostRoutePowerIrCorner>,
    pub worst_power_corner: usize,
    pub worst_drop_corner: usize,
    pub error_findings: usize,
}

/// One MMMC scenario's post-route power results for all UPF modes.
#[derive(Clone, Debug)]
pub struct MultiCornerPostRoutePowerModeIr {
    pub scenario: String,
    pub modes: UpfPowerModeIrResult,
}

/// MMMC × UPF-power-mode post-route IR/EM aggregation.
#[derive(Clone, Debug)]
pub struct MultiCornerPostRoutePowerModeIrResult {
    pub corners: Vec<MultiCornerPostRoutePowerModeIr>,
    pub worst_power_corner: usize,
    pub worst_power_mode: usize,
    pub worst_drop_corner: usize,
    pub worst_drop_mode: usize,
    pub worst_em_lifetime_corner: usize,
    pub worst_em_lifetime_mode: usize,
    pub error_findings: usize,
}

fn effective_post_route_capacitance(iteration: &PhysicalTimingIteration) -> BTreeMap<String, f64> {
    iteration
        .parasitic_report
        .nets
        .iter()
        .map(|net| {
            (
                net.net.clone(),
                net.grounded_cap_pf + 0.5 * net.coupling_cap_pf,
            )
        })
        .collect()
}

fn post_route_power_option_set(options: &PostRoutePowerIrOptions) -> (PowerOptions, PowerOptions) {
    let mut pre_route = options.power.clone();
    pre_route.capacitive_energy_per_pf_pj = 0.0;
    let mut post_route = options.power.clone();
    if options.voltage_scaled_capacitance {
        post_route.capacitive_energy_per_pf_pj = options.ir.voltage_v * options.ir.voltage_v;
    }
    (pre_route, post_route)
}

fn validate_post_route_power_ir_options(options: &PostRoutePowerIrOptions) -> Result<(), String> {
    let power_fields = [
        ("frequency", options.power.frequency_mhz),
        ("base cell energy", options.power.base_cell_energy_pj),
        ("site energy", options.power.energy_per_site_pj),
        (
            "capacitive energy",
            options.power.capacitive_energy_per_pf_pj,
        ),
        ("leakage per site", options.power.leakage_per_site_nw),
    ];
    for (name, value) in power_fields {
        if !value.is_finite() || value < 0.0 {
            return Err(format!("invalid post-route power {} {}", name, value));
        }
    }
    if options.power.frequency_mhz == 0.0 {
        return Err("post-route power frequency must be positive".into());
    }

    let ir_fields = [
        ("voltage", options.ir.voltage_v),
        ("VDD resistance", options.ir.vdd_ohm_per_site),
        ("GND resistance", options.ir.gnd_ohm_per_site),
        ("maximum drop", options.ir.max_total_drop_v),
        ("maximum rail current", options.ir.max_rail_current_a),
    ];
    for (name, value) in ir_fields {
        if !value.is_finite() || value <= 0.0 {
            return Err(format!("invalid post-route IR {} {}", name, value));
        }
    }
    for (domain_name, domain) in &options.upf.domains {
        let fields = [
            ("voltage", domain.voltage_v),
            ("VDD resistance", domain.vdd_ohm_per_site),
            ("GND resistance", domain.gnd_ohm_per_site),
            ("maximum drop", domain.max_total_drop_v),
            ("maximum rail current", domain.max_rail_current_a),
        ];
        for (name, value) in fields {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!(
                    "invalid post-route IR domain '{}' {} {}",
                    domain_name, name, value
                ));
            }
        }
    }
    if let Some(default) = options.upf.default {
        let fields = [
            ("voltage", default.voltage_v),
            ("VDD resistance", default.vdd_ohm_per_site),
            ("GND resistance", default.gnd_ohm_per_site),
            ("maximum drop", default.max_total_drop_v),
            ("maximum rail current", default.max_rail_current_a),
        ];
        for (name, value) in fields {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!(
                    "invalid post-route default IR domain {} {}",
                    name, value
                ));
            }
        }
    }
    Ok(())
}

fn finish_post_route_power_ir(
    iteration: &PhysicalTimingIteration,
    options: &PostRoutePowerIrOptions,
    power_intent: Option<&Upf>,
    pre_route_dynamic_power_uw: f64,
    power: PowerReport,
    effective_capacitance_pf: BTreeMap<String, f64>,
) -> Result<PostRoutePowerIrResult, String> {
    let ir = eda_ir::analyze(
        &iteration.annotated_netlist,
        &iteration.placement,
        &power,
        &options.ir,
    )?;
    let mut upf_options = options.upf.clone();
    if upf_options.default.is_none() {
        upf_options.default = Some(DomainIrOptions::from(options.ir.clone()));
    }
    let upf_ir = match power_intent {
        Some(upf) => Some(eda_ir::analyze_upf(
            &iteration.annotated_netlist,
            &iteration.placement,
            &power,
            upf,
            &upf_options,
        )?),
        None => None,
    };
    let parasitic_dynamic_power_uw = power.dynamic_power_uw - pre_route_dynamic_power_uw;
    if !parasitic_dynamic_power_uw.is_finite() || parasitic_dynamic_power_uw < 0.0 {
        return Err("post-route parasitic power contribution is invalid".into());
    }
    let power_mesh = match &options.power_mesh {
        Some(mesh_options) => Some(eda_ir::analyze_mesh(
            &iteration.annotated_netlist,
            &iteration.placement,
            &power,
            mesh_options,
        )?),
        None => None,
    };
    let multilayer_power_mesh = match &options.multilayer_power_mesh {
        Some(mesh_options) => {
            if !mesh_options.power_domains.is_empty() {
                let Some(upf_result) = &upf_ir else {
                    return Err("coupled multilayer power domains require UPF power intent".into());
                };
                Some(eda_ir::analyze_upf_coupled_multilayer_mesh(
                    &iteration.annotated_netlist,
                    &iteration.placement,
                    upf_result,
                    mesh_options,
                    &mesh_options.power_domains,
                )?)
            } else {
                Some(eda_ir::analyze_multilayer_mesh(
                    &iteration.annotated_netlist,
                    &iteration.placement,
                    &power,
                    mesh_options,
                )?)
            }
        }
        None => None,
    };
    Ok(PostRoutePowerIrResult {
        power,
        ir,
        upf_ir,
        power_mesh,
        multilayer_power_mesh,
        parasitic_dynamic_power_uw,
        effective_capacitance_pf,
    })
}

/// Analyze the final RC-annotated netlist, placement, and power grid.
///
/// Coupling capacitance is conservatively halved for effective switching
/// load. Dynamic cell activity is simulated on the supplied vectors, while
/// leakage and IR current are distributed over the placed cells.
pub fn analyze_physical_timing_power_ir(
    iteration: &PhysicalTimingIteration,
    vectors: &[InputVector],
    options: &PostRoutePowerIrOptions,
) -> Result<PostRoutePowerIrResult, String> {
    if vectors.is_empty() {
        return Err("post-route power analysis requires at least one input vector".into());
    }
    if !options.ir.voltage_v.is_finite() || options.ir.voltage_v <= 0.0 {
        return Err("post-route power analysis requires a positive supply voltage".into());
    }

    let effective_capacitance_pf = effective_post_route_capacitance(iteration);
    let (pre_route_options, post_route_options) = post_route_power_option_set(options);
    let pre_route = eda_power::analyze(&iteration.annotated_netlist, vectors, &pre_route_options)?;
    let power = eda_power::analyze_with_capacitances(
        &iteration.annotated_netlist,
        vectors,
        &effective_capacitance_pf,
        &post_route_options,
    )?;
    finish_post_route_power_ir(
        iteration,
        options,
        None,
        pre_route.dynamic_power_uw,
        power,
        effective_capacitance_pf,
    )
}

/// Analyze post-route power/IR from gate-level VCD activity.
pub fn analyze_physical_timing_power_ir_from_waveform(
    iteration: &PhysicalTimingIteration,
    waveform: &Vcd,
    options: &PostRoutePowerIrOptions,
) -> Result<PostRoutePowerIrResult, String> {
    if !options.ir.voltage_v.is_finite() || options.ir.voltage_v <= 0.0 {
        return Err("post-route power analysis requires a positive supply voltage".into());
    }
    if waveform.time_points().is_empty() {
        return Err("post-route VCD power analysis requires at least one timestamp".into());
    }
    let effective_capacitance_pf = effective_post_route_capacitance(iteration);
    let (pre_route_options, post_route_options) = post_route_power_option_set(options);
    let pre_route =
        eda_power::analyze_waveform(&iteration.annotated_netlist, waveform, &pre_route_options)?;
    let power = eda_power::analyze_waveform_with_capacitances(
        &iteration.annotated_netlist,
        waveform,
        &effective_capacitance_pf,
        &post_route_options,
    )?;
    finish_post_route_power_ir(
        iteration,
        options,
        None,
        pre_route.dynamic_power_uw,
        power,
        effective_capacitance_pf,
    )
}

/// Analyze a post-route physical view with UPF domain-resolved supplies.
pub fn analyze_physical_timing_power_ir_with_upf(
    iteration: &PhysicalTimingIteration,
    vectors: &[InputVector],
    options: &PostRoutePowerIrOptions,
    power_intent: &Upf,
) -> Result<PostRoutePowerIrResult, String> {
    if vectors.is_empty() {
        return Err("post-route power analysis requires at least one input vector".into());
    }
    let effective_capacitance_pf = effective_post_route_capacitance(iteration);
    let (pre_route_options, post_route_options) = post_route_power_option_set(options);
    let pre_route = eda_power::analyze(&iteration.annotated_netlist, vectors, &pre_route_options)?;
    let power = eda_power::analyze_with_capacitances(
        &iteration.annotated_netlist,
        vectors,
        &effective_capacitance_pf,
        &post_route_options,
    )?;
    finish_post_route_power_ir(
        iteration,
        options,
        Some(power_intent),
        pre_route.dynamic_power_uw,
        power,
        effective_capacitance_pf,
    )
}

/// Analyze UPF-aware post-route power/IR from gate-level VCD activity.
pub fn analyze_physical_timing_power_ir_from_waveform_with_upf(
    iteration: &PhysicalTimingIteration,
    waveform: &Vcd,
    options: &PostRoutePowerIrOptions,
    power_intent: &Upf,
) -> Result<PostRoutePowerIrResult, String> {
    if waveform.time_points().is_empty() {
        return Err("post-route VCD power analysis requires at least one timestamp".into());
    }
    let effective_capacitance_pf = effective_post_route_capacitance(iteration);
    let (pre_route_options, post_route_options) = post_route_power_option_set(options);
    let pre_route =
        eda_power::analyze_waveform(&iteration.annotated_netlist, waveform, &pre_route_options)?;
    let power = eda_power::analyze_waveform_with_capacitances(
        &iteration.annotated_netlist,
        waveform,
        &effective_capacitance_pf,
        &post_route_options,
    )?;
    finish_post_route_power_ir(
        iteration,
        options,
        Some(power_intent),
        pre_route.dynamic_power_uw,
        power,
        effective_capacitance_pf,
    )
}

fn post_route_upf_options(options: &PostRoutePowerIrOptions) -> UpfIrOptions {
    let mut upf = options.upf.clone();
    if upf.default.is_none() {
        upf.default = Some(DomainIrOptions::from(options.ir.clone()));
    }
    upf
}

/// Analyze every declared UPF power mode on one post-route implementation.
pub fn analyze_physical_timing_power_ir_power_modes_with_upf(
    iteration: &PhysicalTimingIteration,
    vectors: &[InputVector],
    options: &PostRoutePowerIrOptions,
    power_intent: &Upf,
) -> Result<UpfPowerModeIrResult, String> {
    if vectors.is_empty() {
        return Err("post-route power analysis requires at least one input vector".into());
    }
    let effective_capacitance_pf = effective_post_route_capacitance(iteration);
    let (_, post_route_options) = post_route_power_option_set(options);
    let power = eda_power::analyze_with_capacitances(
        &iteration.annotated_netlist,
        vectors,
        &effective_capacitance_pf,
        &post_route_options,
    )?;
    if let Some(mesh_options) = &options.multilayer_power_mesh {
        if mesh_options.power_domains.is_empty() {
            eda_ir::analyze_upf_power_modes_with_multilayer_mesh(
                &iteration.annotated_netlist,
                &iteration.placement,
                &power,
                power_intent,
                &post_route_upf_options(options),
                mesh_options,
            )
        } else {
            eda_ir::analyze_upf_power_modes_with_coupled_multilayer_mesh(
                &iteration.annotated_netlist,
                &iteration.placement,
                &power,
                power_intent,
                &post_route_upf_options(options),
                mesh_options,
            )
        }
    } else if let Some(mesh_options) = &options.power_mesh {
        eda_ir::analyze_upf_power_modes_with_mesh(
            &iteration.annotated_netlist,
            &iteration.placement,
            &power,
            power_intent,
            &post_route_upf_options(options),
            mesh_options,
        )
    } else {
        eda_ir::analyze_upf_power_modes(
            &iteration.annotated_netlist,
            &iteration.placement,
            &power,
            power_intent,
            &post_route_upf_options(options),
        )
    }
}

/// Analyze all UPF power modes from post-route VCD activity.
pub fn analyze_physical_timing_power_ir_power_modes_from_waveform_with_upf(
    iteration: &PhysicalTimingIteration,
    waveform: &Vcd,
    options: &PostRoutePowerIrOptions,
    power_intent: &Upf,
) -> Result<UpfPowerModeIrResult, String> {
    if waveform.time_points().is_empty() {
        return Err("post-route VCD power analysis requires at least one timestamp".into());
    }
    let effective_capacitance_pf = effective_post_route_capacitance(iteration);
    let (_, post_route_options) = post_route_power_option_set(options);
    let power = eda_power::analyze_waveform_with_capacitances(
        &iteration.annotated_netlist,
        waveform,
        &effective_capacitance_pf,
        &post_route_options,
    )?;
    if let Some(mesh_options) = &options.multilayer_power_mesh {
        if mesh_options.power_domains.is_empty() {
            eda_ir::analyze_upf_power_modes_with_multilayer_mesh(
                &iteration.annotated_netlist,
                &iteration.placement,
                &power,
                power_intent,
                &post_route_upf_options(options),
                mesh_options,
            )
        } else {
            eda_ir::analyze_upf_power_modes_with_coupled_multilayer_mesh(
                &iteration.annotated_netlist,
                &iteration.placement,
                &power,
                power_intent,
                &post_route_upf_options(options),
                mesh_options,
            )
        }
    } else if let Some(mesh_options) = &options.power_mesh {
        eda_ir::analyze_upf_power_modes_with_mesh(
            &iteration.annotated_netlist,
            &iteration.placement,
            &power,
            power_intent,
            &post_route_upf_options(options),
            mesh_options,
        )
    } else {
        eda_ir::analyze_upf_power_modes(
            &iteration.annotated_netlist,
            &iteration.placement,
            &power,
            power_intent,
            &post_route_upf_options(options),
        )
    }
}

fn validate_multi_corner_post_route_power_scenarios(
    corners: &[MultiCornerPhysicalTimingCorner],
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
) -> Result<(), String> {
    if corners.is_empty() || scenarios.is_empty() {
        return Err("multi-corner post-route power analysis has no scenario".into());
    }
    if corners.len() != scenarios.len() {
        return Err(format!(
            "multi-corner post-route power scenario count mismatch: {} physical corners, {} power scenarios",
            corners.len(),
            scenarios.len()
        ));
    }
    for (index, (corner, scenario)) in corners.iter().zip(scenarios.iter()).enumerate() {
        if corner.scenario != scenario.name {
            return Err(format!(
                "post-route power scenario {} is '{}', but physical corner is '{}'",
                index, scenario.name, corner.scenario
            ));
        }
    }
    Ok(())
}

fn finish_multi_corner_post_route_power_ir(
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
    results: Vec<PostRoutePowerIrResult>,
) -> Result<MultiCornerPostRoutePowerIrResult, String> {
    let mut corners = Vec::with_capacity(results.len());
    let mut worst_power_corner = 0;
    let mut worst_drop_corner = 0;
    let mut error_findings = 0;

    for (index, (scenario, result)) in scenarios.iter().zip(results.iter()).enumerate() {
        if result.power.total_power_uw
            > results
                .get(worst_power_corner)
                .map(|previous| previous.power.total_power_uw)
                .unwrap_or(f64::NEG_INFINITY)
        {
            worst_power_corner = index;
        }
        if result.ir.worst_drop_v
            > results
                .get(worst_drop_corner)
                .map(|previous| previous.ir.worst_drop_v)
                .unwrap_or(f64::NEG_INFINITY)
        {
            worst_drop_corner = index;
        }
        error_findings += result
            .ir
            .findings
            .iter()
            .filter(|finding| finding.severity == IrSeverity::Error)
            .count();
        corners.push(MultiCornerPostRoutePowerIrCorner {
            scenario: scenario.name.clone(),
            result: result.clone(),
        });
    }
    Ok(MultiCornerPostRoutePowerIrResult {
        corners,
        worst_power_corner,
        worst_drop_corner,
        error_findings,
    })
}

/// Analyze every MMMC timing view with scenario-specific supply and power
/// models while retaining the common post-route physical implementation.
pub fn analyze_multi_corner_physical_timing_power_ir(
    iteration: &MultiCornerPhysicalTimingIteration,
    vectors: &[InputVector],
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
) -> Result<MultiCornerPostRoutePowerIrResult, String> {
    validate_multi_corner_post_route_power_scenarios(&iteration.corners, scenarios)?;
    if vectors.is_empty() {
        return Err("post-route power analysis requires at least one input vector".into());
    }
    let mut results = Vec::with_capacity(iteration.corners.len());
    for (corner, scenario) in iteration.corners.iter().zip(scenarios.iter()) {
        results.push(analyze_physical_timing_power_ir(
            &corner.evaluation,
            vectors,
            &scenario.options,
        )?);
    }
    finish_multi_corner_post_route_power_ir(scenarios, results)
}

/// Analyze every MMMC timing view from VCD activity.
pub fn analyze_multi_corner_physical_timing_power_ir_from_waveform(
    iteration: &MultiCornerPhysicalTimingIteration,
    waveform: &Vcd,
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
) -> Result<MultiCornerPostRoutePowerIrResult, String> {
    validate_multi_corner_post_route_power_scenarios(&iteration.corners, scenarios)?;
    if waveform.time_points().is_empty() {
        return Err("post-route VCD power analysis requires at least one timestamp".into());
    }
    let mut results = Vec::with_capacity(iteration.corners.len());
    for (corner, scenario) in iteration.corners.iter().zip(scenarios.iter()) {
        results.push(analyze_physical_timing_power_ir_from_waveform(
            &corner.evaluation,
            waveform,
            &scenario.options,
        )?);
    }
    finish_multi_corner_post_route_power_ir(scenarios, results)
}

/// Analyze every MMMC view with UPF domain-resolved post-route supplies.
pub fn analyze_multi_corner_physical_timing_power_ir_with_upf(
    iteration: &MultiCornerPhysicalTimingIteration,
    vectors: &[InputVector],
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
    power_intent: &Upf,
) -> Result<MultiCornerPostRoutePowerIrResult, String> {
    validate_multi_corner_post_route_power_scenarios(&iteration.corners, scenarios)?;
    if vectors.is_empty() {
        return Err("post-route power analysis requires at least one input vector".into());
    }
    let mut results = Vec::with_capacity(iteration.corners.len());
    for (corner, scenario) in iteration.corners.iter().zip(scenarios.iter()) {
        results.push(analyze_physical_timing_power_ir_with_upf(
            &corner.evaluation,
            vectors,
            &scenario.options,
            power_intent,
        )?);
    }
    finish_multi_corner_post_route_power_ir(scenarios, results)
}

/// Analyze every MMMC view with UPF supplies and VCD activity.
pub fn analyze_multi_corner_physical_timing_power_ir_from_waveform_with_upf(
    iteration: &MultiCornerPhysicalTimingIteration,
    waveform: &Vcd,
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
    power_intent: &Upf,
) -> Result<MultiCornerPostRoutePowerIrResult, String> {
    validate_multi_corner_post_route_power_scenarios(&iteration.corners, scenarios)?;
    if waveform.time_points().is_empty() {
        return Err("post-route VCD power analysis requires at least one timestamp".into());
    }
    let mut results = Vec::with_capacity(iteration.corners.len());
    for (corner, scenario) in iteration.corners.iter().zip(scenarios.iter()) {
        results.push(analyze_physical_timing_power_ir_from_waveform_with_upf(
            &corner.evaluation,
            waveform,
            &scenario.options,
            power_intent,
        )?);
    }
    finish_multi_corner_post_route_power_ir(scenarios, results)
}

fn finish_multi_corner_post_route_power_mode_ir(
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
    results: Vec<UpfPowerModeIrResult>,
) -> Result<MultiCornerPostRoutePowerModeIrResult, String> {
    if scenarios.len() != results.len() || results.is_empty() {
        return Err("MMMC UPF power-mode result/scenario count mismatch".into());
    }
    let mode_count = results[0].modes.len();
    if mode_count == 0
        || results
            .iter()
            .any(|result| result.modes.len() != mode_count)
    {
        return Err("MMMC UPF power modes are inconsistent between scenarios".into());
    }

    let mut corners = Vec::with_capacity(results.len());
    let mut worst_power_corner = 0;
    let mut worst_power_mode = 0;
    let mut worst_drop_corner = 0;
    let mut worst_drop_mode = 0;
    let mut worst_em_lifetime_corner = 0;
    let mut worst_em_lifetime_mode = 0;
    let mut worst_power = f64::NEG_INFINITY;
    let mut worst_drop = f64::NEG_INFINITY;
    let mut worst_em_lifetime: Option<f64> = None;
    let mut error_findings = 0;

    for (corner_index, (scenario, result)) in scenarios.iter().zip(results.into_iter()).enumerate()
    {
        for (mode_index, mode) in result.modes.iter().enumerate() {
            if mode.result.total_power_uw > worst_power {
                worst_power = mode.result.total_power_uw;
                worst_power_corner = corner_index;
                worst_power_mode = mode_index;
            }
            if mode.result.worst_drop_v > worst_drop {
                worst_drop = mode.result.worst_drop_v;
                worst_drop_corner = corner_index;
                worst_drop_mode = mode_index;
            }
            let update_worst_em = match (worst_em_lifetime, mode.worst_em_lifetime_hours) {
                (Some(current), Some(candidate)) if candidate < current => true,
                (None, Some(_)) => true,
                _ => false,
            };
            if update_worst_em {
                worst_em_lifetime = mode.worst_em_lifetime_hours;
                worst_em_lifetime_corner = corner_index;
                worst_em_lifetime_mode = mode_index;
            }
            error_findings += result.error_findings;
        }
        corners.push(MultiCornerPostRoutePowerModeIr {
            scenario: scenario.name.clone(),
            modes: result,
        });
    }
    Ok(MultiCornerPostRoutePowerModeIrResult {
        corners,
        worst_power_corner,
        worst_power_mode,
        worst_drop_corner,
        worst_drop_mode,
        worst_em_lifetime_corner,
        worst_em_lifetime_mode,
        error_findings,
    })
}

/// Analyze every MMMC timing view and every declared UPF power mode.
pub fn analyze_multi_corner_physical_timing_power_ir_power_modes_with_upf(
    iteration: &MultiCornerPhysicalTimingIteration,
    vectors: &[InputVector],
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
    power_intent: &Upf,
) -> Result<MultiCornerPostRoutePowerModeIrResult, String> {
    validate_multi_corner_post_route_power_scenarios(&iteration.corners, scenarios)?;
    let mut results = Vec::with_capacity(iteration.corners.len());
    for (corner, scenario) in iteration.corners.iter().zip(scenarios.iter()) {
        results.push(analyze_physical_timing_power_ir_power_modes_with_upf(
            &corner.evaluation,
            vectors,
            &scenario.options,
            power_intent,
        )?);
    }
    finish_multi_corner_post_route_power_mode_ir(scenarios, results)
}

/// Analyze MMMC × UPF power modes from VCD activity.
pub fn analyze_multi_corner_physical_timing_power_ir_power_modes_from_waveform_with_upf(
    iteration: &MultiCornerPhysicalTimingIteration,
    waveform: &Vcd,
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
    power_intent: &Upf,
) -> Result<MultiCornerPostRoutePowerModeIrResult, String> {
    validate_multi_corner_post_route_power_scenarios(&iteration.corners, scenarios)?;
    let mut results = Vec::with_capacity(iteration.corners.len());
    for (corner, scenario) in iteration.corners.iter().zip(scenarios.iter()) {
        results.push(
            analyze_physical_timing_power_ir_power_modes_from_waveform_with_upf(
                &corner.evaluation,
                waveform,
                &scenario.options,
                power_intent,
            )?,
        );
    }
    finish_multi_corner_post_route_power_mode_ir(scenarios, results)
}

/// Render a post-route power/IR summary.
pub fn post_route_power_ir_report(result: &PostRoutePowerIrResult) -> String {
    let effective_cap_pf: f64 = result.effective_capacitance_pf.values().sum();
    let mut out = format!(
        "Post-route power/IR: parasitic dynamic {:.6} uW, effective capacitance {:.6} pF\n{}{}",
        result.parasitic_dynamic_power_uw,
        effective_cap_pf,
        power_report(&result.power),
        ir_report(&result.ir)
    );
    if let Some(upf_result) = &result.upf_ir {
        out.push_str(&upf_ir_report(upf_result));
    }
    if let Some(mesh_result) = &result.power_mesh {
        out.push_str(&power_mesh_report(mesh_result));
    }
    if let Some(mesh_result) = &result.multilayer_power_mesh {
        out.push_str(&multilayer_power_mesh_report(mesh_result));
    }
    out
}

/// Render all scenario-specific post-route power/IR results.
pub fn multi_corner_post_route_power_ir_report(
    result: &MultiCornerPostRoutePowerIrResult,
) -> String {
    let worst_power = &result.corners[result.worst_power_corner];
    let worst_drop = &result.corners[result.worst_drop_corner];
    let mut out = format!(
        "Multi-corner post-route power/IR: {} scenarios, worst power '{}', worst IR drop '{}', error findings {}\n",
        result.corners.len(),
        worst_power.scenario,
        worst_drop.scenario,
        result.error_findings
    );
    for corner in &result.corners {
        out.push_str(&format!("  scenario '{}'\n", corner.scenario));
        out.push_str(&post_route_power_ir_report(&corner.result));
    }
    out
}

/// Render the MMMC × UPF-power-mode result matrix.
pub fn multi_corner_post_route_power_mode_ir_report(
    result: &MultiCornerPostRoutePowerModeIrResult,
) -> String {
    let worst_power_corner = &result.corners[result.worst_power_corner];
    let worst_power = &worst_power_corner.modes.modes[result.worst_power_mode];
    let worst_drop_corner = &result.corners[result.worst_drop_corner];
    let worst_drop = &worst_drop_corner.modes.modes[result.worst_drop_mode];
    let worst_em_corner = &result.corners[result.worst_em_lifetime_corner];
    let worst_em_mode = &worst_em_corner.modes.modes[result.worst_em_lifetime_mode];
    let worst_em_label = worst_em_mode
        .worst_em_lifetime_hours
        .map_or("<unavailable>".to_string(), |_| {
            worst_em_mode.power_mode.clone()
        });
    let mut out = format!(
        "Multi-corner UPF power-mode IR report: {} scenario(s), worst power '{}' / '{}', worst drop '{}' / '{}', worst EM '{}' / '{}', error findings {}\n",
        result.corners.len(),
        worst_power_corner.scenario,
        worst_power.power_mode,
        worst_drop_corner.scenario,
        worst_drop.power_mode,
        worst_em_corner.scenario,
        worst_em_label,
        result.error_findings
    );
    for corner in &result.corners {
        out.push_str(&format!("  scenario '{}'\n", corner.scenario));
        out.push_str(&upf_power_mode_ir_report(&corner.modes));
    }
    out
}

/// Parse VCD through the unified API.
pub fn parse_vcd(source: &str) -> Result<Vcd, String> {
    eda_waveform::parse_vcd(source)
}

/// Parse SVA-style assertions through the unified API.
pub fn parse_assertions(source: &str) -> Result<Vec<Assertion>, String> {
    eda_assert::parse_assertions(source)
}

/// Check assertions against a VCD through the unified API.
pub fn check_assertions(
    waveform: &Vcd,
    assertions: &[Assertion],
) -> Result<Vec<AssertionResult>, String> {
    eda_assert::check_vcd(waveform, assertions)
}

/// Parse a functional-coverage plan through the unified API.
pub fn parse_covergroups(source: &str) -> Result<CoveragePlan, String> {
    eda_coverage::parse_coverage(source)
}

/// Check functional and toggle coverage against a VCD.
pub fn functional_coverage(waveform: &Vcd, plan: &CoveragePlan) -> Result<CoverageReport, String> {
    eda_coverage::check_coverage(waveform, plan)
}

/// Infer clock domains and report clock-domain crossings.
pub fn clock_domain_crossing(netlist: &Netlist) -> Result<CdcResult, String> {
    eda_cdc::analyze(netlist)
}

/// Insert a scan chain into a mapped sequential netlist.
pub fn insert_scan(netlist: &Netlist, options: &ScanOptions) -> Result<ScanChainResult, String> {
    eda_dft::insert_scan_chain(netlist, options)
}

/// Analyze scan coverage and unscanned sequential cells.
pub fn dft_analysis(netlist: &Netlist) -> Result<DftReport, String> {
    eda_dft::analyze(netlist)
}

/// Generate deterministic ATPG patterns for a combinational netlist.
pub fn generate_patterns(
    netlist: &Netlist,
    options: &PatternOptions,
) -> Result<Vec<Pattern>, String> {
    eda_fault::automatic_patterns(netlist, options)
}

/// Generate patterns and grade single stuck-at net faults.
pub fn fault_coverage(
    netlist: &Netlist,
    options: &PatternOptions,
) -> Result<FaultCoverageResult, String> {
    eda_fault::grade_automatic(netlist, options)
}

/// Export generated fault patterns as the compact STIL subset.
pub fn patterns_to_stil(
    netlist: &Netlist,
    patterns: &[Pattern],
    name: &str,
) -> Result<String, String> {
    eda_fault::write_stil(netlist, patterns, name)
}

/// Read back the compact STIL subset emitted by [`patterns_to_stil`].
pub fn parse_stil_patterns(source: &str) -> Result<StilPatternSet, String> {
    eda_fault::parse_stil(source)
}

/// Parse an ECO patch through the unified API.
pub fn parse_eco_patch(source: &str) -> Result<EcoPatch, String> {
    eda_eco::parse_eco(source)
}

/// Apply a structural ECO patch to a mapped netlist.
pub fn apply_eco_patch(netlist: &Netlist, patch: &EcoPatch) -> Result<EcoResult, String> {
    eda_eco::apply_eco(netlist, patch)
}

/// Apply an ECO patch within one stable hierarchy-instance namespace.
pub fn apply_eco_patch_in_hierarchy(
    netlist: &Netlist,
    patch: &EcoPatch,
    hierarchy_pattern: &str,
) -> Result<EcoResult, String> {
    eda_eco::apply_eco_in_hierarchy(netlist, patch, hierarchy_pattern)
}

/// Automatically repair high-fanout nets with functionally transparent buffers.
pub fn repair_fanout(
    netlist: &Netlist,
    options: &FanoutRepairOptions,
) -> Result<EcoResult, String> {
    eda_eco::repair_fanout(netlist, options)
}

/// Automatically repair high fanout and legalize inserted ECO buffers.
pub fn repair_fanout_with_placement(
    netlist: &Netlist,
    placement: &Placement,
    options: &FanoutRepairOptions,
) -> Result<EcoResult, String> {
    eda_eco::repair_fanout_with_placement(netlist, placement, options)
}

/// Automatically repair high-fanout nets in one hierarchy namespace.
pub fn repair_fanout_in_hierarchy(
    netlist: &Netlist,
    options: &FanoutRepairOptions,
    hierarchy_pattern: &str,
) -> Result<EcoResult, String> {
    eda_eco::repair_fanout_in_hierarchy(netlist, options, hierarchy_pattern)
}

/// Repair and legalize high-fanout nets in one hierarchy namespace.
pub fn repair_fanout_with_placement_in_hierarchy(
    netlist: &Netlist,
    placement: &eda_pnr::Placement,
    options: &FanoutRepairOptions,
    hierarchy_pattern: &str,
) -> Result<EcoResult, String> {
    eda_eco::repair_fanout_with_placement_in_hierarchy(
        netlist,
        placement,
        options,
        hierarchy_pattern,
    )
}

/// Options for automatic hold-violation repair.
#[derive(Clone, Debug)]
pub struct HoldRepairOptions {
    pub patch_name: String,
    pub max_buffers_per_endpoint: usize,
    /// Hold checks covered by these active waivers are not repaired. Raw
    /// timing remains available to the caller.
    pub waivers: Vec<TimingViolationWaiver>,
}

impl Default for HoldRepairOptions {
    fn default() -> Self {
        Self {
            patch_name: "hold_repair".into(),
            max_buffers_per_endpoint: 4,
            waivers: Vec::new(),
        }
    }
}

impl HoldRepairOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.patch_name.trim().is_empty() {
            return Err("hold repair patch name cannot be empty".into());
        }
        if self.max_buffers_per_endpoint == 0 {
            return Err("hold repair requires a positive buffer limit".into());
        }
        for waiver in &self.waivers {
            let mut enabled = TimingViolationStore::new("");
            enabled.add_waiver(waiver.clone()).map_err(|error| {
                format!("invalid hold repair waiver '{}': {}", waiver.id, error)
            })?;
        }
        Ok(())
    }
}

/// One repaired hold endpoint.
#[derive(Clone, Debug)]
pub struct HoldRepair {
    pub endpoint: String,
    pub endpoint_pin: usize,
    pub old_slack_ns: f64,
    pub new_slack_ns: f64,
    pub buffers_inserted: usize,
}

/// Result of transparent buffer insertion on negative hold-slack endpoints.
#[derive(Clone, Debug)]
pub struct HoldRepairResult {
    pub netlist: Netlist,
    pub physical_placement: Option<Placement>,
    pub repairs: Vec<HoldRepair>,
    pub buffers_inserted: usize,
    pub all_active_violations_repaired: bool,
}

/// Render hold-repair actions and post-repair status.
pub fn hold_repair_report(result: &HoldRepairResult) -> String {
    let mut out = format!(
        "Hold repair: {} buffer(s), {} endpoint(s), status {}\n",
        result.buffers_inserted,
        result.repairs.len(),
        if result.all_active_violations_repaired {
            "PASS"
        } else {
            "LIMITED"
        }
    );
    for repair in &result.repairs {
        out.push_str(&format!(
            "  {} pin {}: {} buffer(s), slack {:.6} -> {:.6} ns\n",
            repair.endpoint,
            repair.endpoint_pin,
            repair.buffers_inserted,
            repair.old_slack_ns,
            repair.new_slack_ns
        ));
    }
    out
}

/// Options for common multi-corner hold repair.
#[derive(Clone, Debug, Default)]
pub struct MultiCornerHoldRepairOptions {
    pub hold: HoldRepairOptions,
    pub route: RouteOptions,
    pub interconnect: InterconnectModel,
}

impl MultiCornerHoldRepairOptions {
    pub fn validate(&self) -> Result<(), String> {
        self.hold.validate()?;
        self.route.validate()?;
        self.interconnect.validate()
    }
}

/// Per-endpoint hold repair evidence across every timing scenario.
#[derive(Clone, Debug)]
pub struct MultiCornerHoldRepair {
    pub endpoint: String,
    pub endpoint_pin: usize,
    pub old_slack_ns_by_scenario: std::collections::BTreeMap<String, f64>,
    pub new_slack_ns_by_scenario: std::collections::BTreeMap<String, f64>,
    pub buffers_inserted: usize,
}

/// Final repaired state for one timing scenario.
#[derive(Clone, Debug)]
pub struct MultiCornerHoldRepairScenario {
    pub scenario: String,
    pub netlist: Netlist,
    pub timing: ConstrainedTiming,
    pub all_active_violations_repaired: bool,
}

/// Result of common buffer insertion across multi-corner timing views.
#[derive(Clone, Debug)]
pub struct MultiCornerHoldRepairResult {
    pub scenarios: Vec<MultiCornerHoldRepairScenario>,
    pub physical_placement: Placement,
    pub routing: Routing,
    pub repairs: Vec<MultiCornerHoldRepair>,
    pub buffers_inserted: usize,
    pub all_active_violations_repaired: bool,
}

/// Repair active hold violations in every timing scenario with one common,
/// stable-name buffer topology.
///
/// After every common buffer round the first physical view is rerouted. RC is
/// extracted from that common route, converted to SDF, and back-annotated
/// separately into every scenario before hold constraints are re-evaluated.
pub fn repair_multicorner_hold_violations(
    scenarios: &[MultiCornerPhysicalTimingScenario<'_>],
    placement: &Placement,
    options: &MultiCornerHoldRepairOptions,
) -> Result<MultiCornerHoldRepairResult, String> {
    options.validate()?;
    validate_common_physical_topology(scenarios)?;
    let mut netlists = scenarios
        .iter()
        .map(|scenario| scenario.netlist.clone())
        .collect::<Vec<_>>();
    for scenario in scenarios {
        eda_pnr::check_legal(scenario.netlist, placement)?;
    }

    let mut current_placement = placement.clone();
    let mut current_routing =
        eda_pnr::route_with_options(&netlists[0], &current_placement, &options.route)?;
    let mut inserted = std::collections::BTreeMap::<String, usize>::new();
    let mut old_slacks = std::collections::BTreeMap::<(String, String), f64>::new();

    fn local_scenarios<'a>(
        source: &'a [MultiCornerPhysicalTimingScenario<'a>],
        netlists: &'a [Netlist],
    ) -> Vec<MultiCornerPhysicalTimingScenario<'a>> {
        source
            .iter()
            .zip(netlists)
            .map(|(scenario, netlist)| MultiCornerPhysicalTimingScenario {
                name: scenario.name.clone(),
                netlist,
                constraints: scenario.constraints,
                waivers: scenario.waivers.clone(),
            })
            .collect()
    }

    fn evaluate_views(
        source: &[MultiCornerPhysicalTimingScenario<'_>],
        netlists: &[Netlist],
        placement: &Placement,
        routing: &Routing,
        model: &InterconnectModel,
        common_waivers: &[TimingViolationWaiver],
    ) -> Result<Vec<(Netlist, ConstrainedTiming, TimingViolationStore)>, String> {
        let mut out = Vec::with_capacity(source.len());
        for (scenario, netlist) in source.iter().zip(netlists) {
            let mapped_placement =
                map_placement_by_names(&netlists[0], placement, netlist, &scenario.name)?;
            let mapped_routing =
                map_routing_by_names(&netlists[0], routing, netlist, &scenario.name)?;
            let parasitics = extract_interconnect_parasitics(netlist, &mapped_routing, model)?;
            let sdf_text = parasitics_to_sdf(netlist, &parasitics)?;
            let sdf = parse_delay_file(&sdf_text)?;
            let annotated = back_annotate(netlist, &sdf)?;
            let timing = constrained_timing(&annotated, scenario.constraints)?;
            let mut store = timing_violation_store(&scenario.name, &annotated, &timing)?;
            for waiver in &scenario.waivers {
                add_timing_violation_waiver(&mut store, waiver.clone())?;
            }
            for waiver in common_waivers {
                add_timing_violation_waiver(&mut store, waiver.clone())?;
            }
            let drc = physical_verification(&annotated, &mapped_placement, &mapped_routing)?;
            if drc.has_errors() {
                return Err(format!(
                    "multi-corner hold repair scenario '{}' has DRC errors: {}",
                    scenario.name,
                    eda_pnr::drc_report(&drc)
                ));
            }
            out.push((annotated, timing, store));
        }
        Ok(out)
    }

    loop {
        let views = local_scenarios(scenarios, &netlists);
        let states = evaluate_views(
            &views,
            &netlists,
            &current_placement,
            &current_routing,
            &options.interconnect,
            &options.hold.waivers,
        )?;
        let mut endpoints = std::collections::BTreeSet::new();
        for (scenario, state) in scenarios.iter().zip(&states) {
            let (_, timing, store) = state;
            for path in constrained_hold_timing_paths(&state.0, timing, 0) {
                let TimingPathEndpoint::SequentialCell(endpoint) = &path.path.path.endpoint else {
                    continue;
                };
                if path.slack_ns >= 0.0
                    || !path.slack_ns.is_finite()
                    || store
                        .waived_violations()
                        .contains_key(&format!("{}:hold:{}", timing.clock, endpoint))
                {
                    continue;
                }
                old_slacks
                    .entry((scenario.name.clone(), endpoint.clone()))
                    .or_insert(path.slack_ns);
                endpoints.insert(endpoint.clone());
            }
        }

        let mut inserted_this_round = false;
        for endpoint in endpoints {
            let count = inserted.get(&endpoint).copied().unwrap_or(0);
            if count >= options.hold.max_buffers_per_endpoint {
                continue;
            }
            for netlist in &mut netlists {
                insert_hold_buffer(netlist, &endpoint, &options.hold.patch_name, count)?;
            }
            inserted.insert(endpoint, count + 1);
            inserted_this_round = true;
        }
        if !inserted_this_round {
            break;
        }

        let views = local_scenarios(scenarios, &netlists);
        validate_common_physical_topology(&views)?;
        let mut placements = Vec::with_capacity(netlists.len());
        for netlist in &netlists {
            placements.push(extend_physical_placement(netlist, &current_placement)?);
        }
        if placements.iter().any(|extended| *extended != placements[0]) {
            return Err(
                "multi-corner hold repair produced inconsistent physical placements".into(),
            );
        }
        current_placement = placements.remove(0);
        current_routing =
            eda_pnr::route_with_options(&netlists[0], &current_placement, &options.route)?;
    }

    let views = local_scenarios(scenarios, &netlists);
    let final_states = evaluate_views(
        &views,
        &netlists,
        &current_placement,
        &current_routing,
        &options.interconnect,
        &options.hold.waivers,
    )?;
    let mut repaired_scenarios = Vec::new();
    let mut all_repaired = true;
    for ((scenario, state), netlist) in scenarios.iter().zip(&final_states).zip(&netlists) {
        let (annotated, timing, store) = state;
        let scenario_repaired = !constrained_hold_timing_paths(annotated, timing, 0)
            .into_iter()
            .any(|path| {
                path.slack_ns < 0.0
                    && path.slack_ns.is_finite()
                    && !store.waived_violations().contains_key(&format!(
                        "{}:hold:{}",
                        timing.clock,
                        endpoint_name(&path.path.path.endpoint)
                    ))
            });
        all_repaired &= scenario_repaired;
        repaired_scenarios.push(MultiCornerHoldRepairScenario {
            scenario: scenario.name.clone(),
            netlist: netlist.clone(),
            timing: timing.clone(),
            all_active_violations_repaired: scenario_repaired,
        });
    }

    let endpoints = inserted.keys().cloned().collect::<Vec<_>>();
    let mut repairs = Vec::new();
    for endpoint in endpoints {
        let mut old_by_scenario = std::collections::BTreeMap::new();
        let mut new_by_scenario = std::collections::BTreeMap::new();
        for (scenario, state) in scenarios.iter().zip(&final_states) {
            let old = old_slacks
                .get(&(scenario.name.clone(), endpoint.clone()))
                .copied();
            if let Some(old) = old {
                old_by_scenario.insert(scenario.name.clone(), old);
            }
            let new = constrained_hold_timing_paths(&state.0, &state.1, 0)
                .into_iter()
                .find(|path| {
                    matches!(
                        &path.path.path.endpoint,
                        TimingPathEndpoint::SequentialCell(name) if *name == endpoint
                    )
                })
                .map(|path| path.slack_ns)
                .unwrap_or(f64::INFINITY);
            new_by_scenario.insert(scenario.name.clone(), new);
        }
        repairs.push(MultiCornerHoldRepair {
            endpoint: endpoint.clone(),
            endpoint_pin: 1,
            old_slack_ns_by_scenario: old_by_scenario,
            new_slack_ns_by_scenario: new_by_scenario,
            buffers_inserted: inserted.get(&endpoint).copied().unwrap_or(0),
        });
    }
    let buffers_inserted = inserted.values().sum();
    Ok(MultiCornerHoldRepairResult {
        scenarios: repaired_scenarios,
        physical_placement: current_placement,
        routing: current_routing,
        repairs,
        buffers_inserted,
        all_active_violations_repaired: all_repaired,
    })
}

/// Render common multi-corner hold-repair actions and post-repair status.
pub fn multi_corner_hold_repair_report(result: &MultiCornerHoldRepairResult) -> String {
    let mut out = format!(
        "Multi-corner hold repair: {} common buffer(s), {} endpoint(s), status {}\n",
        result.buffers_inserted,
        result.repairs.len(),
        if result.all_active_violations_repaired {
            "PASS"
        } else {
            "LIMITED"
        }
    );
    for repair in &result.repairs {
        out.push_str(&format!(
            "  {} pin {}: {} common buffer(s)\n",
            repair.endpoint, repair.endpoint_pin, repair.buffers_inserted
        ));
        for scenario in repair.old_slack_ns_by_scenario.keys() {
            out.push_str(&format!(
                "    scenario '{}': slack {:.6} -> {:.6} ns\n",
                scenario,
                repair.old_slack_ns_by_scenario[scenario],
                repair
                    .new_slack_ns_by_scenario
                    .get(scenario)
                    .copied()
                    .unwrap_or(f64::INFINITY)
            ));
        }
    }
    out
}

/// Repair unwaived hold violations by inserting transparent buffers before
/// sequential data pins.
///
/// Repair is iterative: after each buffer round the constrained timing is
/// recomputed. This avoids over-buffering endpoints that need only one delay
/// element while still supporting endpoints that require several buffers.
pub fn repair_hold_violations(
    netlist: &Netlist,
    constraints: &TimingConstraints,
    options: &HoldRepairOptions,
) -> Result<HoldRepairResult, String> {
    repair_hold_violations_impl(netlist, constraints, None, options)
}

/// Repair hold violations and legalize every inserted physical buffer.
pub fn repair_hold_violations_with_placement(
    netlist: &Netlist,
    placement: &Placement,
    constraints: &TimingConstraints,
    options: &HoldRepairOptions,
) -> Result<HoldRepairResult, String> {
    eda_pnr::check_legal(netlist, placement)?;
    repair_hold_violations_impl(netlist, constraints, Some(placement), options)
}

fn repair_hold_violations_impl(
    netlist: &Netlist,
    constraints: &TimingConstraints,
    placement: Option<&Placement>,
    options: &HoldRepairOptions,
) -> Result<HoldRepairResult, String> {
    options.validate()?;
    let mut out = netlist.clone();
    let mut old_slacks = std::collections::BTreeMap::new();
    let mut inserted = std::collections::BTreeMap::new();
    let mut current = constrained_timing(&out, constraints)?;

    loop {
        let mut violation_store = timing_violation_store(&current.clock, &out, &current)?;
        for waiver in &options.waivers {
            add_timing_violation_waiver(&mut violation_store, waiver.clone())?;
        }
        let waived = violation_store.waived_violations().clone();
        let mut repaired_this_round = false;
        for path in constrained_hold_timing_paths(&out, &current, 0) {
            let endpoint = match &path.path.path.endpoint {
                TimingPathEndpoint::SequentialCell(name) => name.clone(),
                TimingPathEndpoint::OutputPort(_) => continue,
            };
            if path.slack_ns >= 0.0
                || !path.slack_ns.is_finite()
                || waived.contains_key(&format!("{}:hold:{}", current.clock, endpoint))
            {
                continue;
            }
            old_slacks.entry(endpoint.clone()).or_insert(path.slack_ns);
            let count = *inserted.get(&endpoint).unwrap_or(&0);
            if count >= options.max_buffers_per_endpoint {
                continue;
            }
            insert_hold_buffer(&mut out, &endpoint, &options.patch_name, count)?;
            inserted.insert(endpoint, count + 1);
            repaired_this_round = true;
        }
        if !repaired_this_round {
            break;
        }
        current = constrained_timing(&out, constraints)?;
    }

    let final_paths = constrained_hold_timing_paths(&out, &current, 0);
    let mut repairs = Vec::new();
    for (endpoint, old_slack) in old_slacks {
        let buffers = inserted.get(&endpoint).copied().unwrap_or(0);
        let new_slack = final_paths
            .iter()
            .find(|path| {
                matches!(
                    &path.path.path.endpoint,
                    TimingPathEndpoint::SequentialCell(name) if *name == endpoint
                )
            })
            .map(|path| path.slack_ns)
            .unwrap_or(f64::INFINITY);
        repairs.push(HoldRepair {
            endpoint: endpoint.clone(),
            endpoint_pin: 1,
            old_slack_ns: old_slack,
            new_slack_ns: new_slack,
            buffers_inserted: buffers,
        });
    }

    let mut violation_store = timing_violation_store(&current.clock, &out, &current)?;
    for waiver in &options.waivers {
        add_timing_violation_waiver(&mut violation_store, waiver.clone())?;
    }
    let active_hold_negative = constrained_hold_timing_paths(&out, &current, 0)
        .into_iter()
        .any(|path| {
            path.slack_ns < 0.0
                && path.slack_ns.is_finite()
                && !violation_store.waived_violations().contains_key(&format!(
                    "{}:hold:{}",
                    current.clock,
                    endpoint_name(&path.path.path.endpoint)
                ))
        });
    let physical_placement = match placement {
        Some(original) if !repairs.is_empty() => Some(eda_pnr::extend_placement(&out, original)?),
        Some(original) => Some(original.clone()),
        None => None,
    };
    let buffers_inserted = inserted.values().sum();
    Ok(HoldRepairResult {
        netlist: out,
        physical_placement,
        repairs,
        buffers_inserted,
        all_active_violations_repaired: !active_hold_negative,
    })
}

fn insert_hold_buffer(
    netlist: &mut Netlist,
    endpoint: &str,
    patch_name: &str,
    index: usize,
) -> Result<(), String> {
    let endpoint_id = (0..netlist.cell_count())
        .find(|&cell| netlist.cell(cell).name == endpoint)
        .ok_or_else(|| format!("hold endpoint cell '{}' not found", endpoint))?;
    let pin = 1;
    let Some(&source) = netlist.cell(endpoint_id).inputs.get(pin) else {
        return Err(format!("hold endpoint '{}' has no data pin", endpoint));
    };
    let safe_endpoint = endpoint
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    let safe_patch = patch_name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    let cell_name = format!("{}_{}_{}", safe_patch, safe_endpoint, index);
    let net_name = format!("{}_net", cell_name);
    if netlist.net_by_name(&net_name).is_some()
        || (0..netlist.cell_count()).any(|cell| netlist.cell(cell).name == cell_name)
    {
        return Err(format!("hold repair name '{}' is already used", cell_name));
    }
    let new_net = netlist.add_net(net_name);
    let kind = hold_buffer_kind(netlist);
    netlist.add_cell(eda_core::netlist::Cell {
        name: cell_name,
        kind,
        inputs: vec![source],
        output: new_net,
    });
    let mut endpoint_cell = netlist.cell(endpoint_id).clone();
    endpoint_cell.inputs[pin] = new_net;
    netlist.replace_cell(endpoint_id, endpoint_cell);
    Ok(())
}

fn hold_buffer_kind(netlist: &Netlist) -> eda_core::netlist::CellKind {
    netlist
        .library()
        .and_then(|library| library.find("BUF"))
        .map(eda_core::netlist::CellKind::Lib)
        .unwrap_or(eda_core::netlist::CellKind::Buf)
}

fn endpoint_name(endpoint: &TimingPathEndpoint) -> String {
    match endpoint {
        TimingPathEndpoint::SequentialCell(name) => name.clone(),
        TimingPathEndpoint::OutputPort(name) => name.clone(),
    }
}

/// Extend a legal placement prefix to cover newly appended physical cells.
pub fn extend_physical_placement(
    netlist: &Netlist,
    placement: &Placement,
) -> Result<Placement, String> {
    eda_pnr::extend_placement(netlist, placement)
}

/// Parse a UPF power-intent subset through the unified API.
pub fn parse_power_intent(source: &str) -> Result<Upf, String> {
    eda_upf::parse_upf(source)
}

/// Check UPF power intent against an elaborated design.
pub fn power_intent_check(design: &Design, upf: &Upf) -> UpfResult {
    eda_upf::check(design, upf)
}

/// Synthesize balanced clock trees for every inferred clock domain.
pub fn clock_tree_synthesis(
    netlist: &Netlist,
    options: &CtsOptions,
) -> Result<ClockTreeResult, String> {
    eda_cts::synthesize(netlist, options)
}

/// Placement-aware clock-tree synthesis through the unified API.
pub fn clock_tree_synthesis_with_placement(
    netlist: &Netlist,
    placement: &Placement,
    options: &CtsOptions,
) -> Result<ClockTreeResult, String> {
    eda_cts::synthesize_with_placement(netlist, placement, options)
}

/// Search placement-aware CTS implementations against a skew target.
pub fn timing_driven_clock_tree_synthesis_with_placement(
    netlist: &Netlist,
    placement: &Placement,
    options: &TimingDrivenCtsOptions,
) -> Result<TimingDrivenCtsResult, String> {
    eda_cts::synthesize_timing_driven_with_placement(netlist, placement, options)
}

/// Static power-grid IR-drop and electromigration analysis.
pub fn power_grid_ir(
    netlist: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    options: &IrOptions,
) -> Result<IrResult, String> {
    eda_ir::analyze(netlist, placement, power, options)
}

/// Finite-difference VDD/GND power-mesh IR analysis.
pub fn power_grid_mesh(
    netlist: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    options: &PowerMeshOptions,
) -> Result<PowerMeshResult, String> {
    eda_ir::analyze_mesh(netlist, placement, power, options)
}

/// Layer/via-aware finite-difference VDD/GND power-mesh IR analysis.
pub fn multilayer_power_grid_mesh(
    netlist: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    options: &MultilayerPowerMeshOptions,
) -> Result<MultilayerPowerMeshResult, String> {
    eda_ir::analyze_multilayer_mesh(netlist, placement, power, options)
}

/// Layer/via-aware mesh analysis from explicit cell currents.
pub fn multilayer_power_grid_mesh_currents(
    netlist: &Netlist,
    placement: &Placement,
    cell_current_a: &[f64],
    total_power_uw: f64,
    options: &MultilayerPowerMeshOptions,
) -> Result<MultilayerPowerMeshResult, String> {
    eda_ir::analyze_multilayer_mesh_currents(
        netlist,
        placement,
        cell_current_a,
        total_power_uw,
        options,
    )
}

/// Render a layer/via-aware power-mesh report.
pub fn multilayer_power_mesh_report(result: &MultilayerPowerMeshResult) -> String {
    eda_ir::multilayer_power_mesh_report(result)
}

/// Serialize multilayer power-domain supply/tap definitions.
pub fn write_multilayer_power_domain_supplies(
    supplies: &[MultilayerPowerDomainSupply],
) -> Result<String, String> {
    let mut names = std::collections::BTreeSet::new();
    let mut out = String::new();
    for supply in supplies {
        if supply.name.trim().is_empty() {
            return Err("power-domain supply name cannot be empty".into());
        }
        if !names.insert(supply.name.as_str()) {
            return Err(format!("duplicate power-domain supply '{}'", supply.name));
        }
        if !supply.voltage_v.is_finite() || supply.voltage_v <= 0.0 {
            return Err(format!(
                "power-domain supply '{}' has invalid voltage {}",
                supply.name, supply.voltage_v
            ));
        }
        if supply.taps.is_empty() {
            return Err(format!(
                "power-domain supply '{}' requires at least one tap",
                supply.name
            ));
        }
        out.push_str(&format!("supply {} {:?}\n", supply.name, supply.voltage_v));
        for tap in &supply.taps {
            if !tap.vdd && !tap.gnd {
                return Err(format!(
                    "power-domain supply '{}' has a tap with neither VDD nor GND enabled",
                    supply.name
                ));
            }
            let network = match (tap.vdd, tap.gnd) {
                (true, true) => "both",
                (true, false) => "vdd",
                (false, true) => "gnd",
                (false, false) => unreachable!(),
            };
            out.push_str(&format!(
                "tap {} {} {} {}\n",
                tap.layer, tap.row, tap.column, network
            ));
        }
        out.push_str("end\n");
    }
    Ok(out)
}

/// Parse the power-domain supply/tap format emitted by
/// [`write_multilayer_power_domain_supplies`].
pub fn parse_multilayer_power_domain_supplies(
    source: &str,
) -> Result<Vec<MultilayerPowerDomainSupply>, String> {
    struct PartialSupply {
        name: String,
        voltage_v: f64,
        taps: Vec<PowerMeshLayerTap>,
    }

    let mut supplies = Vec::new();
    let mut current: Option<PartialSupply> = None;
    let mut names = std::collections::BTreeSet::new();
    for (index, raw) in source.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let line_number = index + 1;
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        match tokens.first().copied() {
            Some("supply") => {
                if current.is_some() {
                    return Err(format!(
                        "line {}: previous power-domain supply block is not closed",
                        line_number
                    ));
                }
                if tokens.len() != 3 {
                    return Err(format!(
                        "line {}: supply requires a name and voltage",
                        line_number
                    ));
                }
                let name = tokens[1].to_string();
                if name.trim().is_empty() {
                    return Err(format!("line {}: supply name cannot be empty", line_number));
                }
                if !names.insert(name.clone()) {
                    return Err(format!("line {}: duplicate supply '{}'", line_number, name));
                }
                let voltage_v = tokens[2].parse::<f64>().map_err(|_| {
                    format!(
                        "line {}: invalid supply voltage '{}'",
                        line_number, tokens[2]
                    )
                })?;
                if !voltage_v.is_finite() || voltage_v <= 0.0 {
                    return Err(format!(
                        "line {}: supply '{}' voltage must be positive and finite",
                        line_number, name
                    ));
                }
                current = Some(PartialSupply {
                    name,
                    voltage_v,
                    taps: Vec::new(),
                });
            }
            Some("tap") => {
                let Some(supply) = current.as_mut() else {
                    return Err(format!(
                        "line {}: tap appears outside a supply block",
                        line_number
                    ));
                };
                if tokens.len() != 5 {
                    return Err(format!(
                        "line {}: tap requires layer, row, column, and network",
                        line_number
                    ));
                }
                let parse_coordinate = |token: &str, name: &str| {
                    token.parse::<usize>().map_err(|_| {
                        format!("line {}: invalid tap {} '{}'", line_number, name, token)
                    })
                };
                let layer = parse_coordinate(tokens[1], "layer")?;
                let row = parse_coordinate(tokens[2], "row")?;
                let column = parse_coordinate(tokens[3], "column")?;
                let (vdd, gnd) = match tokens[4] {
                    "vdd" => (true, false),
                    "gnd" => (false, true),
                    "both" => (true, true),
                    other => {
                        return Err(format!(
                            "line {}: unknown tap network '{}'; expected vdd, gnd, or both",
                            line_number, other
                        ))
                    }
                };
                supply.taps.push(PowerMeshLayerTap {
                    layer,
                    row,
                    column,
                    vdd,
                    gnd,
                });
            }
            Some("end") => {
                let Some(supply) = current.take() else {
                    return Err(format!(
                        "line {}: end appears outside a supply block",
                        line_number
                    ));
                };
                if tokens.len() != 1 {
                    return Err(format!("line {}: malformed supply end", line_number));
                }
                if supply.taps.is_empty() {
                    return Err(format!(
                        "power-domain supply '{}' requires at least one tap",
                        supply.name
                    ));
                }
                if !supply.taps.iter().any(|tap| tap.vdd) {
                    return Err(format!(
                        "power-domain supply '{}' requires at least one VDD tap",
                        supply.name
                    ));
                }
                supplies.push(MultilayerPowerDomainSupply {
                    name: supply.name,
                    voltage_v: supply.voltage_v,
                    taps: supply.taps,
                });
            }
            Some(other) => {
                return Err(format!(
                    "line {}: unknown power-domain supply token '{}'",
                    line_number, other
                ));
            }
            None => {}
        }
    }
    if current.is_some() {
        return Err("unterminated power-domain supply block".into());
    }
    if supplies.is_empty() {
        return Err("power-domain supply file contains no supplies".into());
    }
    Ok(supplies)
}

/// Build UPF IR domain options from multilayer supply-domain definitions.
pub fn multilayer_power_domain_upf_options(
    supplies: &[MultilayerPowerDomainSupply],
    fallback: &IrOptions,
) -> Result<UpfIrOptions, String> {
    let mut options = UpfIrOptions {
        default: Some(DomainIrOptions::from(fallback.clone())),
        domains: std::collections::BTreeMap::new(),
    };
    for supply in supplies {
        if supply.name.trim().is_empty() {
            return Err("power-domain supply name cannot be empty".into());
        }
        if !supply.voltage_v.is_finite() || supply.voltage_v <= 0.0 {
            return Err(format!(
                "power-domain supply '{}' has invalid voltage {}",
                supply.name, supply.voltage_v
            ));
        }
        options.domains.insert(
            supply.name.clone(),
            DomainIrOptions {
                voltage_v: supply.voltage_v,
                ..DomainIrOptions::from(fallback.clone())
            },
        );
    }
    Ok(options)
}

/// UPF-aware static power-grid IR-drop and electromigration analysis.
pub fn power_grid_ir_upf(
    netlist: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    power_intent: &Upf,
    options: &UpfIrOptions,
) -> Result<UpfIrResult, String> {
    eda_ir::analyze_upf(netlist, placement, power, power_intent, options)
}

/// Analyze every declared UPF power mode through the unified API.
pub fn power_grid_ir_upf_power_modes(
    netlist: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    power_intent: &Upf,
    options: &UpfIrOptions,
) -> Result<UpfPowerModeIrResult, String> {
    eda_ir::analyze_upf_power_modes(netlist, placement, power, power_intent, options)
}

/// Analyze every UPF power mode with a multilayer power mesh.
pub fn power_grid_ir_upf_power_modes_with_multilayer_mesh(
    netlist: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    power_intent: &Upf,
    options: &UpfIrOptions,
    mesh_options: &MultilayerPowerMeshOptions,
) -> Result<UpfPowerModeIrResult, String> {
    eda_ir::analyze_upf_power_modes_with_multilayer_mesh(
        netlist,
        placement,
        power,
        power_intent,
        options,
        mesh_options,
    )
}

/// Analyze all UPF power modes with one shared multi-supply mesh.
pub fn power_grid_ir_upf_power_modes_with_coupled_multilayer_mesh(
    netlist: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    power_intent: &Upf,
    options: &UpfIrOptions,
    mesh_options: &MultilayerPowerMeshOptions,
) -> Result<UpfPowerModeIrResult, String> {
    eda_ir::analyze_upf_power_modes_with_coupled_multilayer_mesh(
        netlist,
        placement,
        power,
        power_intent,
        options,
        mesh_options,
    )
}

/// Build domain-decomposed multilayer power meshes from a UPF IR result.
pub fn upf_domain_multilayer_meshes(
    netlist: &Netlist,
    placement: &Placement,
    result: &UpfIrResult,
    options: &MultilayerPowerMeshOptions,
) -> Result<Vec<UpfDomainMultilayerMeshResult>, String> {
    eda_ir::analyze_upf_domain_multilayer_meshes(netlist, placement, result, options)
}

/// Solve one shared multilayer mesh with multiple UPF supply-domain voltages.
pub fn upf_coupled_multilayer_mesh(
    netlist: &Netlist,
    placement: &Placement,
    result: &UpfIrResult,
    options: &MultilayerPowerMeshOptions,
    domain_supplies: &[MultilayerPowerDomainSupply],
) -> Result<MultilayerPowerMeshResult, String> {
    eda_ir::analyze_upf_coupled_multilayer_mesh(
        netlist,
        placement,
        result,
        options,
        domain_supplies,
    )
}

/// Estimate power from a gate-level VCD through the unified API.
pub fn power_from_waveform(
    nl: &Netlist,
    waveform: &Vcd,
    opts: &PowerOptions,
) -> Result<PowerReport, String> {
    eda_power::analyze_waveform(nl, waveform, opts)
}

/// Formal equivalence inside a flow.
pub fn flow_equivalent(f: &Flow) -> Result<bool, String> {
    Ok(eda_formal::check_equivalent(&f.gold, &f.netlist)?.equivalent)
}

/// Symbolically compare two netlists through the unified API.
pub fn check_netlists_equivalent(gold: &Netlist, mapped: &Netlist) -> Result<EquivResult, String> {
    check_netlists_equivalent_cancellable(gold, mapped, &FlowCancellation::new())
}

/// Cooperatively cancel netlist equivalence checking.
pub fn check_netlists_equivalent_cancellable(
    gold: &Netlist,
    mapped: &Netlist,
    cancellation: &FlowCancellation,
) -> Result<EquivResult, String> {
    eda_formal::check_equivalent_cancellable(gold, mapped, cancellation)
}

/// Search for a initialized bad-state trace through the unified API.
pub fn bounded_model_check(
    netlist: &Netlist,
    bad_net: &str,
    depth: usize,
) -> Result<BmcResult, String> {
    bounded_model_check_cancellable(netlist, bad_net, depth, &FlowCancellation::new())
}

/// Cooperatively cancel bad-state bounded model checking.
pub fn bounded_model_check_cancellable(
    netlist: &Netlist,
    bad_net: &str,
    depth: usize,
    cancellation: &FlowCancellation,
) -> Result<BmcResult, String> {
    eda_formal::bmc_cancellable(
        netlist,
        bad_net,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        cancellation,
    )
}

/// Search for an initialized false-property trace through the unified API.
pub fn bounded_model_check_expression(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
) -> Result<BmcResult, String> {
    bounded_model_check_expression_cancellable(netlist, property, depth, &FlowCancellation::new())
}

/// Cooperatively cancel expression bounded model checking.
pub fn bounded_model_check_expression_cancellable(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
    cancellation: &FlowCancellation,
) -> Result<BmcResult, String> {
    eda_formal::bmc_expression_cancellable(
        netlist,
        property,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        cancellation,
    )
}

/// Prove a safety property using bounded reachability plus k-induction.
pub fn prove_safety(netlist: &Netlist, bad_net: &str, depth: usize) -> Result<ProofResult, String> {
    prove_safety_cancellable(netlist, bad_net, depth, &FlowCancellation::new())
}

/// Cooperatively cancel a safety proof.
pub fn prove_safety_cancellable(
    netlist: &Netlist,
    bad_net: &str,
    depth: usize,
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    eda_formal::prove_cancellable(
        netlist,
        bad_net,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        cancellation,
    )
}

/// Prove a Boolean safety expression using BMC plus k-induction.
pub fn prove_safety_expression(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
) -> Result<ProofResult, String> {
    prove_safety_expression_cancellable(netlist, property, depth, &FlowCancellation::new())
}

/// Cooperatively cancel an expression safety proof.
pub fn prove_safety_expression_cancellable(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    eda_formal::prove_expression_cancellable(
        netlist,
        property,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        cancellation,
    )
}

/// Prove a Boolean expression using independently verified net invariants.
pub fn prove_safety_expression_with_value_invariants(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
    invariant_nets: &[(&str, bool)],
) -> Result<ProofResult, String> {
    prove_safety_expression_with_value_invariants_cancellable(
        netlist,
        property,
        depth,
        invariant_nets,
        &FlowCancellation::new(),
    )
}

/// Cooperatively cancel an expression proof with net invariants.
pub fn prove_safety_expression_with_value_invariants_cancellable(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
    invariant_nets: &[(&str, bool)],
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    eda_formal::prove_expression_with_value_invariants_cancellable(
        netlist,
        property,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        invariant_nets,
        cancellation,
    )
}

/// Prove a Boolean expression using independently verified expression invariants.
pub fn prove_safety_expression_with_expression_invariants(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
    invariants: &[(&str, &BoolExpr)],
) -> Result<ProofResult, String> {
    prove_safety_expression_with_expression_invariants_cancellable(
        netlist,
        property,
        depth,
        invariants,
        &FlowCancellation::new(),
    )
}

/// Cooperatively cancel an expression proof with expression invariants.
pub fn prove_safety_expression_with_expression_invariants_cancellable(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
    invariants: &[(&str, &BoolExpr)],
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    eda_formal::prove_expression_with_expression_invariants_cancellable(
        netlist,
        property,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        invariants,
        cancellation,
    )
}

/// Prove a Boolean expression using independently verified net or expression invariants.
pub fn prove_safety_expression_with_proof_invariants(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
    invariants: &[ProofInvariant],
) -> Result<ProofResult, String> {
    prove_safety_expression_with_proof_invariants_cancellable(
        netlist,
        property,
        depth,
        invariants,
        &FlowCancellation::new(),
    )
}

/// Cooperatively cancel an expression proof with typed invariants.
pub fn prove_safety_expression_with_proof_invariants_cancellable(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
    invariants: &[ProofInvariant],
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    eda_formal::prove_expression_with_proof_invariants_cancellable(
        netlist,
        property,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        invariants,
        cancellation,
    )
}

/// Bounded SAT checking of immediate and temporal assertions.
pub fn check_assertions_bounded(
    netlist: &Netlist,
    assertions: &[Assertion],
    depth: usize,
) -> Result<Vec<PropertyBmcResult>, String> {
    eda_formal::bmc_assertions(
        netlist,
        assertions,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
    )
}

/// Cooperatively cancel bounded SAT checking of assertions.
pub fn check_assertions_bounded_cancellable(
    netlist: &Netlist,
    assertions: &[Assertion],
    depth: usize,
    cancellation: &FlowCancellation,
) -> Result<Vec<PropertyBmcResult>, String> {
    eda_formal::bmc_assertions_cancellable(
        netlist,
        assertions,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        cancellation,
    )
}

/// Bounded SAT checking of assert/cover targets under SVA assumptions.
pub fn check_assertions_bounded_with_assumptions(
    netlist: &Netlist,
    assertions: &[Assertion],
    assumptions: &[Assertion],
    depth: usize,
) -> Result<Vec<PropertyBmcResult>, String> {
    eda_formal::bmc_assertions_with_assumptions(
        netlist,
        assertions,
        assumptions,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
    )
}

/// Cooperatively cancel bounded SAT checking under SVA assumptions.
pub fn check_assertions_bounded_with_assumptions_cancellable(
    netlist: &Netlist,
    assertions: &[Assertion],
    assumptions: &[Assertion],
    depth: usize,
    cancellation: &FlowCancellation,
) -> Result<Vec<PropertyBmcResult>, String> {
    eda_formal::bmc_assertions_with_assumptions_cancellable(
        netlist,
        assertions,
        assumptions,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        cancellation,
    )
}

/// Export a SAT input-assignment trace as VCD through the unified API.
pub fn export_formal_trace_vcd(
    netlist: &Netlist,
    trace: &[std::collections::HashMap<String, u64>],
) -> Result<String, String> {
    eda_formal::trace_input_vcd(netlist, trace)
}

/// Export every SAT-reconstructed net value in a formal trace as VCD.
pub fn export_formal_net_trace_vcd(
    netlist: &Netlist,
    trace: &[std::collections::HashMap<String, u64>],
) -> Result<String, String> {
    eda_formal::trace_net_vcd(netlist, trace)
}

/// Export a temporal-property counterexample as VCD.
pub fn export_property_counterexample_vcd(
    netlist: &Netlist,
    result: &PropertyBmcResult,
) -> Result<String, String> {
    eda_formal::trace_property_vcd(netlist, result)
}

/// Independently replay a formal property counterexample at netlist level.
pub fn verify_property_counterexample(
    netlist: &Netlist,
    result: &PropertyBmcResult,
) -> Result<PropertyReplayResult, String> {
    eda_formal::verify_property_trace(netlist, result)
}

/// Parse and independently replay an exported formal property VCD.
pub fn replay_property_counterexample_vcd(
    netlist: &Netlist,
    result: &PropertyBmcResult,
    vcd: &str,
) -> Result<PropertyReplayResult, String> {
    eda_formal::replay_property_vcd(netlist, result, vcd)
}

/// Export a refuted safety proof's complete SAT net trace as VCD.
pub fn export_proof_counterexample_vcd(
    netlist: &Netlist,
    result: &ProofResult,
) -> Result<String, String> {
    let trace = result
        .bmc
        .net_trace
        .as_ref()
        .ok_or_else(|| "proof result has no counterexample net trace".to_string())?;
    eda_formal::trace_net_vcd(netlist, trace)
}

/// Prove a safety property using independently verified zero-valued invariants.
pub fn prove_safety_with_invariants(
    netlist: &Netlist,
    bad_net: &str,
    depth: usize,
    invariant_nets: &[&str],
) -> Result<ProofResult, String> {
    prove_safety_with_invariants_cancellable(
        netlist,
        bad_net,
        depth,
        invariant_nets,
        &FlowCancellation::new(),
    )
}

/// Cooperatively cancel a proof with zero-valued invariants.
pub fn prove_safety_with_invariants_cancellable(
    netlist: &Netlist,
    bad_net: &str,
    depth: usize,
    invariant_nets: &[&str],
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    eda_formal::prove_with_invariants_cancellable(
        netlist,
        bad_net,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        invariant_nets,
        cancellation,
    )
}

/// Prove a safety property using independently verified Boolean invariants.
pub fn prove_safety_with_value_invariants(
    netlist: &Netlist,
    bad_net: &str,
    depth: usize,
    invariants: &[(&str, bool)],
) -> Result<ProofResult, String> {
    prove_safety_with_value_invariants_cancellable(
        netlist,
        bad_net,
        depth,
        invariants,
        &FlowCancellation::new(),
    )
}

/// Cooperatively cancel a proof with Boolean invariants.
pub fn prove_safety_with_value_invariants_cancellable(
    netlist: &Netlist,
    bad_net: &str,
    depth: usize,
    invariants: &[(&str, bool)],
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    eda_formal::prove_with_value_invariants_cancellable(
        netlist,
        bad_net,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        invariants,
        cancellation,
    )
}

/// Evaluate the gate-level netlist of a flow on one combinational vector.
pub fn gate_eval_comb(f: &Flow, inputs: &[(&str, u64)], out: &str) -> Result<u64, String> {
    let d = netlist_to_design(&f.netlist);
    let mut sim = Simulator::new(&d)?;
    for (p, v) in inputs {
        let w = f
            .netlist
            .port_by_name(p)
            .map(|pt| pt.bits.len())
            .ok_or_else(|| format!("no port '{}'", p))?;
        sim.set_input(p, &LogicWord::from_u64(w, *v))
            .map_err(|e| e.to_string())?;
    }
    let w = sim
        .read_port(out)
        .ok_or_else(|| format!("no port '{}'", out))?;
    w.to_u64().ok_or_else(|| format!("'{}' is X", out))
}
