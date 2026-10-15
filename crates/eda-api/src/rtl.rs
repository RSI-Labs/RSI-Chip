//! High-level RTL synthesis, analysis, implementation, simulation, and scan APIs.

use super::*;

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
