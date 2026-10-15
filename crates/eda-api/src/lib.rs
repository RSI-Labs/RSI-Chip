//! # eda-api — high-level one-call flows
//!
//! Facade over the whole toolchain: compile Verilog, simulate, synthesize,
//! verify equivalence, timing analysis and netlist export.

#[cfg(test)]
mod cancellation_tests;
mod capabilities;
pub mod flow_cache;
mod formal;
mod integration;
mod physical;
mod routing;
mod rtl;
mod timing;
mod timing_flow;
pub use formal::*;
pub use integration::*;
pub use physical::*;
pub use routing::*;
pub use rtl::*;
pub use timing::*;
pub use timing_flow::*;
use timing_flow::{
    map_placement_by_names, map_routing_by_names, validate_common_physical_topology,
};
mod pdk;
pub use pdk::{pdk_liberty_sha256, pdk_liberty_sha256_bytes};
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
    write_resistor_layer_models, write_statistical_model, write_transient_csv,
    write_transient_csv_with_temperatures, AgingOptions, AgingResult, AmsResult, Analysis,
    BjtModel, BjtPolarity, ConstraintExpression, ConstraintFunction, ConstraintSense, CrossingEdge,
    DcOperatingPoint, DerivativeEntry, DerivativeOptions, DerivativeResult, DerivativeTarget,
    ElementLocation, EnvelopeOptions, EnvelopeResult, GlobalSensitivityOptions,
    GlobalSensitivityResult, MonteCarloDistribution, MonteCarloOptions, MonteCarloResult,
    MosfetModel, MosfetPolarity, NoiseResult, OptimizationObjective, OptimizationOptions,
    OptimizationParameter, OptimizationResult, ParameterCorrelation, ParameterCovariance,
    ParetoConstraint, ParetoFrontMethod, ParetoOptions, ParetoPoint, ParetoResult, ParetoSense,
    PssOptions, PssResult, ResistorLayerModel, ResistorWidthBin, SensitivityOptions,
    SensitivityResult, SourceWaveform, SpiceCircuit, SpiceElement, SpiceElementKind,
    StatisticalModel, StatisticalParameterModel, TransientResult, TransientSample,
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
    PathException, PathExceptionCheck, PortDelay, ResetCheckKind, ResetEndpoint, ResetTimingPaths,
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
