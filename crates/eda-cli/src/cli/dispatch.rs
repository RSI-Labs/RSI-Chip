//! Command dispatch for the main EDA CLI.

use super::formal::{temporal_bmc_report, write_proof_cex};
use super::physical::{
    coupled_multilayer_mesh_options, multilayer_mesh_options, write_best_physical_timing_outputs,
    write_extracted_spef,
};
use super::timing::{
    multicorner_sta_report, optimization_waivers, sta_hold_paths_report, sta_paths_report,
    sta_reset_paths_report, timing_waiver_report,
};
use eda_api::{
    MultilayerPowerDomainSupply, PdkDefinition, ResistorLayerModel, StatisticalModel,
    TimingViolationWaiver, TimingWaiverAuditEntry, Upf, Vcd,
};
use eda_core::library::CellLibrary;
use std::collections::BTreeMap;

/// Read-only inputs captured by command handlers.
pub(crate) struct RunContext<'a> {
    pub(crate) cmd: &'a str,
    pub(crate) src: &'a str,
    pub(crate) top: &'a Option<String>,
    pub(crate) extracted_spef_path: &'a Option<String>,
    pub(crate) exported_sdf_path: &'a Option<String>,
    pub(crate) exported_verilog_path: &'a Option<String>,
    pub(crate) maze_routing: bool,
    pub(crate) power_mesh: bool,
    pub(crate) multilayer_power_mesh: bool,
    pub(crate) ripgup_passes: &'a Option<usize>,
    pub(crate) timing_ripgup_passes: &'a Option<usize>,
    pub(crate) physical_timing: bool,
    pub(crate) post_route_power: bool,
    pub(crate) all_power_modes: bool,
    pub(crate) repair_antennas: bool,
    pub(crate) antenna_diode_limit: usize,
    pub(crate) antenna_diode_limit_set: bool,
    pub(crate) timing_path_count: &'a Option<usize>,
    pub(crate) hold_path_count: &'a Option<usize>,
    pub(crate) reset_path_count: &'a Option<usize>,
    pub(crate) lef_out: &'a Option<String>,
    pub(crate) def_out: &'a Option<String>,
    pub(crate) stil_out: &'a Option<String>,
    pub(crate) repair_hold: bool,
    pub(crate) hold_buffer_limit: usize,
    pub(crate) hold_buffer_limit_set: bool,
    pub(crate) timing_driven_cts: bool,
    pub(crate) cts_target_skew: f64,
    pub(crate) bad_net: &'a Option<String>,
    pub(crate) cex_path: &'a Option<String>,
    pub(crate) proof_expr: &'a Option<eda_api::BoolExpr>,
    pub(crate) proof_depth: usize,
    pub(crate) invariant_nets: &'a Vec<(String, bool)>,
    pub(crate) expression_invariants: &'a Vec<eda_api::ProofInvariant>,
    pub(crate) pdk: Option<PdkDefinition>,
    pub(crate) sdc: Option<String>,
    pub(crate) mmsc: Option<String>,
    pub(crate) waiver_rules: &'a Option<Vec<TimingViolationWaiver>>,
    pub(crate) waiver_audit: &'a Option<Vec<TimingWaiverAuditEntry>>,
    pub(crate) def_input: Option<String>,
    pub(crate) active_lib: &'a CellLibrary,
    pub(crate) vcd: Option<Vcd>,
    pub(crate) sva: Option<String>,
    pub(crate) sdf: Option<String>,
    pub(crate) spef: Option<String>,
    pub(crate) cover: Option<String>,
    pub(crate) statistical_model: Option<StatisticalModel>,
    pub(crate) location_def: Option<eda_pnr::Def>,
    pub(crate) resistor_layer_models: Option<BTreeMap<String, ResistorLayerModel>>,
    pub(crate) eco: Option<String>,
    pub(crate) upf: Option<String>,
    pub(crate) power_intent: Option<Upf>,
    pub(crate) power_domain_supplies: Option<Vec<MultilayerPowerDomainSupply>>,
}

pub(crate) fn dispatch(ctx: &RunContext) -> Result<String, String> {
    match ctx.cmd {
        "pdk" => {
            let pdk = ctx.pdk.as_ref().ok_or("pdk signoff requires --pdk")?;
            let result = eda_api::run_pdk_signoff(ctx.src, ctx.top.as_deref(), pdk)?;
            Ok(eda_api::report_pdk_signoff(&result))
        }
        "info" => {
            let d = eda_api::compile(ctx.src, ctx.top.as_deref())?;
            let mut s = format!("module {}\n", d.name);
            s += &format!("  nets: {} (flat bits)\n", d.n_nets);
            s += &format!("  memories: {}\n", d.memories.len());
            s += &format!("  memory connections: {}\n", d.memory_connections.len());
            s += &format!("  ports: {}\n", d.ports.len());
            s += &format!(
                "  assigns: {}, processes: {}, initials: {}\n",
                d.assigns.len(),
                d.processes.len(),
                d.initials.len()
            );
            if !d.hierarchy.is_empty() {
                s.push_str(&eda_api::hierarchy_report(&d));
            }
            Ok(s)
        }
        "ams" => {
            let spice_source = if let Some(models) = &ctx.resistor_layer_models {
                format!(
                    "{}\n{}",
                    eda_api::write_resistor_layer_models(models)?,
                    ctx.src
                )
            } else {
                ctx.src.to_string()
            };
            let circuit = eda_api::parse_spice(&spice_source)?;
            let mut circuit = circuit;
            if let Some(model) = &ctx.statistical_model {
                let name = model.name.clone();
                circuit
                    .statistical_models
                    .insert(name.clone(), model.clone());
                for analysis in &mut circuit.analyses {
                    if let eda_ams::Analysis::MonteCarlo(options) = analysis {
                        options.statistical_model = Some(name.clone());
                    }
                }
            }
            if let Some(def) = &ctx.location_def {
                eda_api::apply_physical_def_locations(&mut circuit, def, 100.0)?;
            }
            let results = eda_ams::run_all(&circuit)?;
            Ok(eda_ams::report_all(&results))
        }
        "synth" => {
            let f = eda_api::run_flow_with_library(ctx.src, ctx.top.as_deref(), ctx.active_lib)?;
            Ok(f.verilog)
        }
        "equiv" => {
            let f = eda_api::run_flow_with_library(ctx.src, ctx.top.as_deref(), ctx.active_lib)?;
            Ok(if f.equivalent {
                "EQUIVALENT\n".into()
            } else {
                "NOT EQUIVALENT\n".into()
            })
        }
        "sta" => {
            let f = eda_api::run_flow_with_library(ctx.src, ctx.top.as_deref(), ctx.active_lib)?;
            if ctx.spef.is_some() && ctx.sdf.is_some() {
                return Err("choose either --spef or --sdf, not both".into());
            }
            let netlist = if let Some(spef) = ctx.spef.as_deref() {
                let parasitics = eda_api::parse_parasitics(spef)?;
                let generated = eda_api::parasitics_to_sdf(&f.netlist, &parasitics)?;
                let annotations = eda_api::parse_delay_file(&generated)?;
                eda_api::back_annotate(&f.netlist, &annotations)?
            } else if let Some(sdf) = ctx.sdf.as_deref() {
                let annotations = eda_api::parse_delay_file(sdf)?;
                eda_api::back_annotate(&f.netlist, &annotations)?
            } else {
                f.netlist.clone()
            };
            if ctx.sdc.is_some() && ctx.mmsc.is_some() {
                return Err("choose either --sdc or --mmsc, not both".into());
            }
            if ctx.waiver_rules.is_some() && ctx.waiver_audit.is_some() {
                return Err("choose either --waivers or --waiver-audit, not both".into());
            }
            if (ctx.waiver_rules.is_some() || ctx.waiver_audit.is_some())
                && ctx.sdc.is_none()
                && ctx.mmsc.is_none()
            {
                return Err("--waivers and --waiver-audit require --sdc or --mmsc".into());
            }
            if let Some(source) = ctx.mmsc.as_deref() {
                multicorner_sta_report(
                    &netlist,
                    source,
                    *ctx.timing_path_count,
                    *ctx.hold_path_count,
                    *ctx.reset_path_count,
                    ctx.waiver_rules.as_deref(),
                    ctx.waiver_audit.as_deref(),
                )
            } else if ctx.timing_path_count.is_none()
                && ctx.hold_path_count.is_none()
                && ctx.reset_path_count.is_none()
            {
                match ctx.sdc.as_deref() {
                    Some(sdc) => {
                        let constraints = eda_api::parse_timing_constraints(sdc)?;
                        let timing = eda_sta::analyze_with_constraints(&netlist, &constraints)?;
                        let constrained =
                            eda_sta::analyze_constrained(&netlist, timing, &constraints)?;
                        let mut report = eda_sta::constrained_report(&constrained);
                        if ctx.waiver_rules.is_some() || ctx.waiver_audit.is_some() {
                            report.push_str(&timing_waiver_report(
                                &netlist,
                                &constrained,
                                ctx.waiver_rules.as_deref(),
                                ctx.waiver_audit.as_deref(),
                            )?);
                        }
                        Ok(report)
                    }
                    None => {
                        let timing = eda_sta::analyze(&netlist)?;
                        Ok(eda_sta::report(&netlist, &timing))
                    }
                }
            } else {
                let constraints = ctx
                    .sdc
                    .as_deref()
                    .map(eda_api::parse_timing_constraints)
                    .transpose()?;
                let constrained = constraints
                    .as_ref()
                    .map(|constraints| eda_api::constrained_timing(&netlist, constraints))
                    .transpose()?;
                let timing = eda_sta::analyze(&netlist)?;
                let mut report = match &constrained {
                    Some(constrained) => eda_sta::constrained_report(constrained),
                    None => eda_sta::report(&netlist, &timing),
                };
                if let Some(constrained) = &constrained {
                    if ctx.waiver_rules.is_some() || ctx.waiver_audit.is_some() {
                        report.push_str(&timing_waiver_report(
                            &netlist,
                            constrained,
                            ctx.waiver_rules.as_deref(),
                            ctx.waiver_audit.as_deref(),
                        )?);
                    }
                }
                if let Some(path_count) = *ctx.timing_path_count {
                    report.push_str(&sta_paths_report(
                        &netlist,
                        &timing,
                        constrained.as_ref(),
                        path_count,
                    )?);
                }
                if let Some(path_count) = *ctx.hold_path_count {
                    report.push_str(&sta_hold_paths_report(
                        &netlist,
                        &timing,
                        constrained.as_ref(),
                        path_count,
                    )?);
                }
                if let Some(path_count) = *ctx.reset_path_count {
                    report.push_str(&sta_reset_paths_report(
                        &netlist,
                        &timing,
                        constrained.as_ref(),
                        path_count,
                    )?);
                }
                Ok(report)
            }
        }
        "pnr" => {
            let f = eda_api::run_flow_with_library(ctx.src, ctx.top.as_deref(), ctx.active_lib)?;
            let (placement, default_routing) = match ctx.def_input.as_deref() {
                Some(source) => eda_api::import_physical_def_text(&f.netlist, source)?,
                None => (f.placement.clone(), f.routing.clone()),
            };
            let route_options = if ctx.def_input.is_some() {
                eda_api::RouteOptions {
                    tracks_per_channel: default_routing.tracks_per_channel,
                    allow_congestion: false,
                    blocked_channels: default_routing.blocked_channels.clone(),
                }
            } else {
                eda_api::RouteOptions::default()
            };
            if ctx.ripgup_passes.is_some() && ctx.timing_ripgup_passes.is_some() {
                return Err("choose either --ripgup or --timing-ripgup".into());
            }
            let waiver_store = if ctx.waiver_rules.is_some() || ctx.waiver_audit.is_some() {
                let sdc = ctx
                    .sdc
                    .as_deref()
                    .ok_or_else(|| "pnr waivers require --sdc".to_string())?;
                let constraints = eda_api::parse_timing_constraints(sdc)?;
                let timing = eda_sta::analyze_with_constraints(&f.netlist, &constraints)?;
                let constrained = eda_sta::analyze_constrained(&f.netlist, timing, &constraints)?;
                let waivers =
                    optimization_waivers(ctx.waiver_rules.as_deref(), ctx.waiver_audit.as_deref())?;
                let mut store =
                    eda_api::timing_violation_store(&constrained.clock, &f.netlist, &constrained)?;
                if let Some(waivers) = waivers {
                    for waiver in waivers {
                        eda_api::add_timing_violation_waiver(&mut store, waiver)?;
                    }
                }
                Some((store, constrained))
            } else {
                None
            };
            let routing = if let Some(passes) = ctx.timing_ripgup_passes {
                if ctx.maze_routing {
                    return Err("--maze cannot be combined with timing rip-up routing".into());
                }
                match &waiver_store {
                    Some((store, constrained)) => {
                        eda_api::route_maze_timing_ripgup_from_waiver_aware_timing(
                            &f.netlist,
                            &placement,
                            &route_options,
                            constrained,
                            store,
                            *passes,
                        )?
                        .ripup
                        .routing
                    }
                    None => {
                        eda_api::route_maze_timing_ripgup_from_timing(
                            &f.netlist,
                            &placement,
                            &route_options,
                            &f.timing,
                            *passes,
                        )?
                        .ripup
                        .routing
                    }
                }
            } else if let Some(passes) = ctx.ripgup_passes {
                if ctx.maze_routing {
                    return Err("choose either --maze or --ripgup, not both".into());
                }
                eda_api::route_maze_ripgup(&f.netlist, &placement, &route_options, *passes)?.routing
            } else if ctx.maze_routing {
                eda_api::route_maze_with_options(&f.netlist, &placement, &route_options)?
            } else {
                default_routing
            };
            if let Some(path) = &ctx.extracted_spef_path {
                write_extracted_spef(path, &f.netlist, &routing)?;
            }
            if let Some(path) = &ctx.lef_out {
                std::fs::write(path, &f.lef).map_err(|e| e.to_string())?;
            }
            if let Some(path) = &ctx.def_out {
                let def = eda_pnr::write_def(&f.netlist, &placement, &routing)?;
                std::fs::write(path, def).map_err(|e| e.to_string())?;
            }
            let mut s = eda_pnr::report(&f.netlist, &placement);
            s += &eda_pnr::routing_report(&routing);
            s += &format!(
                "  HPWL estimate: {:.1} sites\n",
                eda_pnr::hpwl(&f.netlist, &placement)
            );
            if let Some((store, _)) = waiver_store {
                s += &eda_api::timing_violation_report(&store, None);
            }
            Ok(s)
        }
        "power" => {
            let f = eda_api::run_flow_with_library(ctx.src, ctx.top.as_deref(), ctx.active_lib)?;
            let report = if let Some(vcd) = ctx.vcd.as_ref() {
                eda_api::power_from_waveform(&f.netlist, vcd, &eda_api::PowerOptions::default())?
            } else {
                eda_api::quick_power(&f.netlist, &eda_api::PowerOptions::default())?
            };
            Ok(eda_api::power_report(&report))
        }
        "drc" => {
            let f = eda_api::run_flow_with_library(ctx.src, ctx.top.as_deref(), ctx.active_lib)?;
            let (placement, routing) = match ctx.def_input.as_deref() {
                Some(source) => eda_api::import_physical_def_text(&f.netlist, source)?,
                None => (f.placement.clone(), f.routing.clone()),
            };
            let result = eda_api::physical_verification(&f.netlist, &placement, &routing)?;
            Ok(eda_pnr::drc_report(&result))
        }
        "assert" => {
            let waveform = ctx
                .vcd
                .as_ref()
                .ok_or_else(|| "assert requires --vcd <file.vcd>".to_string())?;
            let properties = ctx
                .sva
                .as_deref()
                .ok_or_else(|| "assert requires --sva <file.sva>".to_string())?;
            let assertions = eda_api::parse_assertions(properties)?;
            let results = eda_api::check_assertions(waveform, &assertions)?;
            Ok(eda_api::assertion_report(&results))
        }
        "coverage" => {
            let waveform = ctx
                .vcd
                .as_ref()
                .ok_or_else(|| "coverage requires --vcd <file.vcd>".to_string())?;
            let source = ctx
                .cover
                .as_deref()
                .ok_or_else(|| "coverage requires --cover <file.cover>".to_string())?;
            let plan = eda_api::parse_covergroups(source)?;
            let result = eda_api::functional_coverage(waveform, &plan)?;
            Ok(eda_api::coverage_report(&result))
        }
        "cdc" => {
            let f = eda_api::run_flow_with_library(ctx.src, ctx.top.as_deref(), ctx.active_lib)?;
            let result = eda_api::clock_domain_crossing(&f.netlist)?;
            Ok(eda_cdc::report(&result))
        }
        "spef" => {
            let parasitics_source = ctx
                .spef
                .as_deref()
                .ok_or_else(|| "spef requires --spef <file.spef>".to_string())?;
            let parasitics = eda_api::parse_parasitics(parasitics_source)?;
            let f = eda_api::run_flow_with_library(ctx.src, ctx.top.as_deref(), ctx.active_lib)?;
            let result = eda_api::parasitic_analysis(&f.netlist, &parasitics)?;
            Ok(eda_api::parasitic_report(&result))
        }
        "dft" => {
            let f = eda_api::run_flow_with_library(ctx.src, ctx.top.as_deref(), ctx.active_lib)?;
            let scan = eda_api::insert_scan(&f.netlist, &eda_api::ScanOptions::default())?;
            let report = eda_api::dft_analysis(&scan.netlist)?;
            let mut out = eda_dft::report(&scan);
            out.push_str(&format!(
                "  sequential={} scanned={} chains={} combinational={}\n",
                report.sequential_cells,
                report.scan_cells,
                report.scan_chains,
                report.combinational_cells
            ));
            Ok(out)
        }
        "fault" => {
            let f = eda_api::run_flow_with_library(ctx.src, ctx.top.as_deref(), ctx.active_lib)?;
            let result = eda_api::fault_coverage(&f.netlist, &eda_api::PatternOptions::default())?;
            if let Some(path) = &ctx.stil_out {
                let stil = eda_api::patterns_to_stil(&f.netlist, &result.patterns, &f.design.name)?;
                std::fs::write(path, stil).map_err(|e| e.to_string())?;
            }
            Ok(eda_fault::report(&result))
        }
        "eco" => {
            if ctx.repair_hold {
                if ctx.eco.is_some() {
                    return Err("choose either --eco or --repair-hold, not both".into());
                }
                if let Some(source) = ctx.mmsc.as_deref() {
                    if ctx.sdc.is_some() {
                        return Err("choose either --sdc or --mmsc, not both".into());
                    }
                    let definitions = eda_api::parse_timing_scenarios(source)?;
                    let f = eda_api::run_flow_with_library(
                        ctx.src,
                        ctx.top.as_deref(),
                        ctx.active_lib,
                    )?;
                    let scenarios = definitions
                        .iter()
                        .map(|definition| {
                            eda_api::MultiCornerPhysicalTimingScenario::new(
                                definition.name.clone(),
                                &f.netlist,
                                &definition.constraints,
                                Vec::new(),
                            )
                        })
                        .collect::<Vec<_>>();
                    let result = eda_api::repair_multicorner_hold_violations(
                        &scenarios,
                        &f.placement,
                        &eda_api::MultiCornerHoldRepairOptions {
                            hold: eda_api::HoldRepairOptions {
                                max_buffers_per_endpoint: ctx.hold_buffer_limit,
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                    )?;
                    return Ok(eda_api::multi_corner_hold_repair_report(&result));
                }
                let source = ctx
                    .sdc
                    .as_deref()
                    .ok_or_else(|| "--repair-hold requires --sdc <file.sdc>".to_string())?;
                let constraints = eda_api::parse_timing_constraints(source)?;
                let f =
                    eda_api::run_flow_with_library(ctx.src, ctx.top.as_deref(), ctx.active_lib)?;
                let result = eda_api::repair_hold_violations_with_placement(
                    &f.netlist,
                    &f.placement,
                    &constraints,
                    &eda_api::HoldRepairOptions {
                        max_buffers_per_endpoint: ctx.hold_buffer_limit,
                        ..Default::default()
                    },
                )?;
                return Ok(eda_api::hold_repair_report(&result));
            }
            if ctx.hold_buffer_limit_set {
                return Err("--hold-max-buffers requires --repair-hold".into());
            }
            let source = ctx
                .eco
                .as_deref()
                .ok_or_else(|| "eco requires --eco <file.eco>".to_string())?;
            let patch = eda_api::parse_eco_patch(source)?;
            let f = eda_api::run_flow_with_library(ctx.src, ctx.top.as_deref(), ctx.active_lib)?;
            let result = eda_api::apply_eco_patch(&f.netlist, &patch)?;
            Ok(eda_eco::report(&result))
        }
        "upf" => {
            let source = ctx
                .upf
                .as_deref()
                .ok_or_else(|| "upf requires --upf <file.upf>".to_string())?;
            let intent = eda_api::parse_power_intent(source)?;
            let design = eda_api::compile(ctx.src, ctx.top.as_deref())?;
            let result = eda_api::power_intent_check(&design, &intent);
            Ok(eda_upf::report(&result))
        }
        "cts" => {
            let f = eda_api::run_flow_with_library(ctx.src, ctx.top.as_deref(), ctx.active_lib)?;
            if ctx.timing_driven_cts {
                let result = eda_api::timing_driven_clock_tree_synthesis_with_placement(
                    &f.netlist,
                    &f.placement,
                    &eda_api::TimingDrivenCtsOptions {
                        target_skew_ns: ctx.cts_target_skew,
                        ..Default::default()
                    },
                )?;
                Ok(eda_api::timing_driven_cts_report(&result))
            } else {
                let result = eda_api::clock_tree_synthesis_with_placement(
                    &f.netlist,
                    &f.placement,
                    &eda_api::CtsOptions::default(),
                )?;
                Ok(eda_cts::report(&result))
            }
        }
        "ir" => {
            let f = eda_api::run_flow_with_library(ctx.src, ctx.top.as_deref(), ctx.active_lib)?;
            let power = if let Some(vcd) = ctx.vcd.as_ref() {
                eda_api::power_from_waveform(&f.netlist, vcd, &eda_api::PowerOptions::default())?
            } else {
                eda_api::quick_power(&f.netlist, &eda_api::PowerOptions::default())?
            };
            if ctx.multilayer_power_mesh {
                if let Some(supplies) = ctx.power_domain_supplies.as_ref() {
                    let intent = ctx
                        .power_intent
                        .as_ref()
                        .ok_or("--power-mesh-domains requires --upf")?;
                    let ir_options = eda_api::multilayer_power_domain_upf_options(
                        supplies,
                        &eda_api::IrOptions::default(),
                    )?;
                    let upf_result = eda_api::power_grid_ir_upf(
                        &f.netlist,
                        &f.placement,
                        &power,
                        intent,
                        &ir_options,
                    )?;
                    let mesh = eda_api::upf_coupled_multilayer_mesh(
                        &f.netlist,
                        &f.placement,
                        &upf_result,
                        &coupled_multilayer_mesh_options(f.placement.rows.max(1), supplies),
                        supplies,
                    )?;
                    return Ok(eda_api::multilayer_power_mesh_report(&mesh));
                }
                let mesh = eda_api::multilayer_power_grid_mesh(
                    &f.netlist,
                    &f.placement,
                    &power,
                    &multilayer_mesh_options(f.placement.rows.max(1)),
                )?;
                return Ok(eda_api::multilayer_power_mesh_report(&mesh));
            }
            if ctx.power_mesh {
                let mesh = eda_api::power_grid_mesh(
                    &f.netlist,
                    &f.placement,
                    &power,
                    &eda_api::PowerMeshOptions {
                        rows: f.placement.rows.max(1),
                        ..eda_api::PowerMeshOptions::default()
                    },
                )?;
                return Ok(eda_api::power_mesh_report(&mesh));
            }
            let result = eda_api::power_grid_ir(
                &f.netlist,
                &f.placement,
                &power,
                &eda_api::IrOptions::default(),
            )?;
            Ok(eda_ir::report(&result))
        }
        "prove" => {
            let f = eda_api::run_flow_with_library(ctx.src, ctx.top.as_deref(), ctx.active_lib)?;
            if let Some(source) = ctx.sva.as_deref() {
                if ctx.proof_expr.is_some()
                    || ctx.bad_net.is_some()
                    || !ctx.invariant_nets.is_empty()
                    || !ctx.expression_invariants.is_empty()
                {
                    return Err("--sva cannot be combined with other prove targets".into());
                }
                return temporal_bmc_report(
                    &f.netlist,
                    source,
                    ctx.proof_depth,
                    ctx.cex_path.as_deref(),
                );
            }
            let invariants: Vec<(&str, bool)> = ctx
                .invariant_nets
                .iter()
                .map(|(net, value)| (net.as_str(), *value))
                .collect();
            if let Some(expr) = &ctx.proof_expr {
                let mut invariants = ctx.expression_invariants.clone();
                invariants.extend(
                    ctx.invariant_nets
                        .iter()
                        .map(|(net, value)| eda_api::ProofInvariant::new(net.clone(), *value)),
                );
                let proof = if invariants.is_empty() {
                    eda_api::prove_safety_expression(&f.netlist, expr, ctx.proof_depth)?
                } else {
                    eda_api::prove_safety_expression_with_proof_invariants(
                        &f.netlist,
                        expr,
                        ctx.proof_depth,
                        &invariants,
                    )?
                };
                if let Some(path) = &ctx.cex_path {
                    if proof.status != eda_api::ProofStatus::Refuted {
                        return Err("--cex requires a refuted proof".into());
                    }
                    write_proof_cex(path, &f.netlist, &proof)?;
                }
                Ok(eda_api::proof_report(&proof))
            } else {
                if !ctx.expression_invariants.is_empty() {
                    return Err("--assume-expr requires --expr".into());
                }
                let bad = ctx.bad_net.as_deref().unwrap_or("bad");
                let proof = if invariants.is_empty() {
                    eda_api::prove_safety(&f.netlist, bad, ctx.proof_depth)?
                } else {
                    eda_api::prove_safety_with_value_invariants(
                        &f.netlist,
                        bad,
                        ctx.proof_depth,
                        &invariants,
                    )?
                };
                if let Some(path) = &ctx.cex_path {
                    if proof.status != eda_api::ProofStatus::Refuted {
                        return Err("--cex requires a refuted proof".into());
                    }
                    write_proof_cex(path, &f.netlist, &proof)?;
                }
                Ok(eda_api::proof_report(&proof))
            }
        }
        "flow" => {
            let f = eda_api::run_flow_with_library(ctx.src, ctx.top.as_deref(), ctx.active_lib)?;
            let imported_physical = match ctx.def_input.as_deref() {
                Some(source) => Some(eda_api::import_physical_def_text(&f.netlist, source)?),
                None => None,
            };
            if imported_physical.is_some() && !ctx.physical_timing {
                return Err("--import-def requires --physical-timing".into());
            }
            if !ctx.physical_timing && ctx.repair_antennas {
                return Err("--repair-antennas requires --physical-timing".into());
            }
            if !ctx.repair_antennas && ctx.antenna_diode_limit_set {
                return Err("--antenna-max-diodes requires --repair-antennas".into());
            }
            if ctx.physical_timing {
                if ctx.sdc.is_some() && ctx.mmsc.is_some() {
                    return Err("choose either --sdc or --mmsc, not both".into());
                }
                let waivers =
                    optimization_waivers(ctx.waiver_rules.as_deref(), ctx.waiver_audit.as_deref())?;
                if let Some(source) = ctx.mmsc.as_deref() {
                    let definitions = eda_api::parse_timing_scenarios(source)?;
                    let mut power_scenarios = if ctx.post_route_power {
                        eda_api::post_route_power_scenarios_from_timing_scenarios(&definitions)?
                    } else {
                        Vec::new()
                    };
                    let scenarios = definitions
                        .iter()
                        .map(|definition| {
                            eda_api::MultiCornerPhysicalTimingScenario::new(
                                definition.name.clone(),
                                &f.netlist,
                                &definition.constraints,
                                waivers.clone().unwrap_or_default(),
                            )
                        })
                        .collect::<Vec<_>>();
                    let result = eda_api::run_multi_corner_physical_timing_flow(
                        &eda_api::MultiCornerPhysicalTimingFlowOptions {
                            scenarios,
                            timing_driven_cts: ctx.timing_driven_cts.then(|| {
                                eda_api::TimingDrivenCtsOptions {
                                    target_skew_ns: ctx.cts_target_skew,
                                    ..Default::default()
                                }
                            }),
                            hold_repair: ctx.repair_hold.then(|| {
                                eda_api::MultiCornerHoldRepairOptions {
                                    hold: eda_api::HoldRepairOptions {
                                        max_buffers_per_endpoint: ctx.hold_buffer_limit,
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                }
                            }),
                            routing_passes: ctx.timing_ripgup_passes.unwrap_or(8),
                            initial_placement: imported_physical
                                .as_ref()
                                .map(|(placement, _)| placement.clone()),
                            route: imported_physical
                                .map(|(_, routing)| eda_api::RouteOptions {
                                    tracks_per_channel: routing.tracks_per_channel,
                                    allow_congestion: false,
                                    blocked_channels: routing.blocked_channels,
                                })
                                .unwrap_or_default(),
                            repair_antennas: ctx.repair_antennas,
                            antenna_max_diodes_per_net: ctx.antenna_diode_limit,
                            ..Default::default()
                        },
                    )?;
                    let best = result.best();
                    let Some(reference) = best.corners.first() else {
                        return Err("MMMC physical timing flow has no scenario".into());
                    };
                    if ctx.power_mesh {
                        let mesh_options = eda_api::PowerMeshOptions {
                            rows: best.placement.rows.max(1),
                            ..Default::default()
                        };
                        for scenario in &mut power_scenarios {
                            scenario.options.power_mesh = Some(mesh_options.clone());
                        }
                    }
                    if ctx.multilayer_power_mesh {
                        let mut mesh_upf_options = None;
                        let mesh_options = match ctx.power_domain_supplies.as_ref() {
                            Some(supplies) => {
                                mesh_upf_options =
                                    Some(eda_api::multilayer_power_domain_upf_options(
                                        supplies,
                                        &eda_api::IrOptions::default(),
                                    )?);
                                coupled_multilayer_mesh_options(
                                    best.placement.rows.max(1),
                                    supplies,
                                )
                            }
                            None => multilayer_mesh_options(best.placement.rows.max(1)),
                        };
                        for scenario in &mut power_scenarios {
                            scenario.options.multilayer_power_mesh = Some(mesh_options.clone());
                            if let Some(upf_options) = &mesh_upf_options {
                                scenario.options.upf = upf_options.clone();
                            }
                        }
                    }
                    write_best_physical_timing_outputs(
                        ctx.lef_out.as_ref(),
                        ctx.def_out.as_ref(),
                        ctx.extracted_spef_path.as_ref(),
                        None,
                        ctx.exported_verilog_path.as_ref(),
                        &reference.evaluation.annotated_netlist,
                        &best.placement,
                        &best.routing,
                        &reference.evaluation.parasitics,
                        &reference.evaluation.sdf,
                    )?;
                    let mut report = eda_api::multi_corner_physical_timing_flow_report(&result);
                    if let Some(template) = &ctx.exported_sdf_path {
                        let exports = eda_api::prepare_multi_corner_delay_file_exports(
                            &best.corners,
                            template,
                        )?;
                        eda_api::write_multi_corner_delay_file_exports(&exports)?;
                        report.push_str(&format!(
                            "MMMC SDF exports: {}\n",
                            exports
                                .iter()
                                .map(|export| format!(
                                    "{}={}",
                                    export.scenario,
                                    export.path.display()
                                ))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    if let Some(path) = &ctx.exported_verilog_path {
                        report.push_str(&format!("Best physical Verilog export: {}\n", path));
                    }
                    if ctx.post_route_power {
                        if ctx.all_power_modes {
                            let intent = ctx
                                .power_intent
                                .as_ref()
                                .ok_or("--all-power-modes requires --upf")?;
                            let power_modes = match ctx.vcd.as_ref() {
                                    Some(waveform) => {
                                        eda_api::analyze_multi_corner_physical_timing_power_ir_power_modes_from_waveform_with_upf(
                                            best,
                                            waveform,
                                            &power_scenarios,
                                            intent,
                                        )?
                                    }
                                    None => {
                                        let vectors =
                                            eda_api::default_power_vectors(
                                                &reference.evaluation.annotated_netlist,
                                            );
                                        eda_api::analyze_multi_corner_physical_timing_power_ir_power_modes_with_upf(
                                            best,
                                            &vectors,
                                            &power_scenarios,
                                            intent,
                                        )?
                                    }
                                };
                            report.push_str(
                                &eda_api::multi_corner_post_route_power_mode_ir_report(
                                    &power_modes,
                                ),
                            );
                        } else {
                            let power_ir = match (ctx.vcd.as_ref(), ctx.power_intent.as_ref()) {
                                    (Some(waveform), Some(intent)) => {
                                        eda_api::analyze_multi_corner_physical_timing_power_ir_from_waveform_with_upf(
                                            best,
                                            waveform,
                                            &power_scenarios,
                                            intent,
                                        )?
                                    }
                                    (Some(waveform), None) => {
                                        eda_api::analyze_multi_corner_physical_timing_power_ir_from_waveform(
                                            best,
                                            waveform,
                                            &power_scenarios,
                                        )?
                                    }
                                    (None, Some(intent)) => {
                                        let vectors = eda_api::default_power_vectors(
                                            &reference.evaluation.annotated_netlist,
                                        );
                                        eda_api::analyze_multi_corner_physical_timing_power_ir_with_upf(
                                            best,
                                            &vectors,
                                            &power_scenarios,
                                            intent,
                                        )?
                                    }
                                    (None, None) => {
                                        let vectors = eda_api::default_power_vectors(
                                            &reference.evaluation.annotated_netlist,
                                        );
                                        eda_api::analyze_multi_corner_physical_timing_power_ir(
                                            best,
                                            &vectors,
                                            &power_scenarios,
                                        )?
                                    }
                                };
                            report.push_str(&eda_api::multi_corner_post_route_power_ir_report(
                                &power_ir,
                            ));
                        }
                    }
                    return Ok(report);
                }
                let constraints = ctx
                    .sdc
                    .as_deref()
                    .map(eda_api::parse_timing_constraints)
                    .transpose()?;
                if waivers.is_some() && constraints.is_none() {
                    return Err("flow waivers require --sdc".into());
                }
                let result = eda_api::run_physical_timing_flow(
                    &f.netlist,
                    &eda_api::PhysicalTimingFlowOptions {
                        constraints,
                        waivers: waivers.unwrap_or_default(),
                        timing_driven_cts: ctx.timing_driven_cts.then(|| {
                            eda_api::TimingDrivenCtsOptions {
                                target_skew_ns: ctx.cts_target_skew,
                                ..Default::default()
                            }
                        }),
                        hold_repair: ctx.repair_hold.then(|| eda_api::HoldRepairOptions {
                            max_buffers_per_endpoint: ctx.hold_buffer_limit,
                            ..Default::default()
                        }),
                        routing_passes: ctx.timing_ripgup_passes.unwrap_or(8),
                        initial_placement: imported_physical
                            .as_ref()
                            .map(|(placement, _)| placement.clone()),
                        route: imported_physical
                            .map(|(_, routing)| eda_api::RouteOptions {
                                tracks_per_channel: routing.tracks_per_channel,
                                allow_congestion: false,
                                blocked_channels: routing.blocked_channels,
                            })
                            .unwrap_or_default(),
                        repair_antennas: ctx.repair_antennas,
                        antenna_max_diodes_per_net: ctx.antenna_diode_limit,
                        ..Default::default()
                    },
                )?;
                let best = result.best();
                write_best_physical_timing_outputs(
                    ctx.lef_out.as_ref(),
                    ctx.def_out.as_ref(),
                    ctx.extracted_spef_path.as_ref(),
                    ctx.exported_sdf_path.as_ref(),
                    ctx.exported_verilog_path.as_ref(),
                    &best.annotated_netlist,
                    &best.placement,
                    &best.routing,
                    &best.parasitics,
                    &best.sdf,
                )?;
                let mut report = eda_api::physical_timing_flow_report(&result);
                let mut post_route_options = eda_api::PostRoutePowerIrOptions::default();
                if ctx.multilayer_power_mesh {
                    if let Some(supplies) = ctx.power_domain_supplies.as_ref() {
                        post_route_options.upf = eda_api::multilayer_power_domain_upf_options(
                            supplies,
                            &eda_api::IrOptions::default(),
                        )?;
                        post_route_options.multilayer_power_mesh = Some(
                            coupled_multilayer_mesh_options(best.placement.rows.max(1), supplies),
                        );
                    } else {
                        post_route_options.multilayer_power_mesh =
                            Some(multilayer_mesh_options(best.placement.rows.max(1)));
                    }
                }
                if ctx.power_mesh {
                    post_route_options.power_mesh = Some(eda_api::PowerMeshOptions {
                        rows: best.placement.rows.max(1),
                        ..Default::default()
                    });
                }
                if ctx.post_route_power {
                    if ctx.all_power_modes {
                        let intent = ctx
                            .power_intent
                            .as_ref()
                            .ok_or("--all-power-modes requires --upf")?;
                        let power_modes = match ctx.vcd.as_ref() {
                                Some(waveform) => {
                                    eda_api::analyze_physical_timing_power_ir_power_modes_from_waveform_with_upf(
                                        best,
                                        waveform,
                                        &post_route_options,
                                        intent,
                                    )?
                                }
                                None => {
                                    let vectors =
                                        eda_api::default_power_vectors(&best.annotated_netlist);
                                    eda_api::analyze_physical_timing_power_ir_power_modes_with_upf(
                                        best,
                                        &vectors,
                                        &post_route_options,
                                        intent,
                                    )?
                                }
                            };
                        report.push_str(&eda_api::upf_power_mode_ir_report(&power_modes));
                    } else {
                        let power_ir = match (ctx.vcd.as_ref(), ctx.power_intent.as_ref()) {
                            (Some(waveform), Some(intent)) => {
                                eda_api::analyze_physical_timing_power_ir_from_waveform_with_upf(
                                    best,
                                    waveform,
                                    &post_route_options,
                                    intent,
                                )?
                            }
                            (Some(waveform), None) => {
                                eda_api::analyze_physical_timing_power_ir_from_waveform(
                                    best,
                                    waveform,
                                    &post_route_options,
                                )?
                            }
                            (None, Some(intent)) => {
                                let vectors =
                                    eda_api::default_power_vectors(&best.annotated_netlist);
                                eda_api::analyze_physical_timing_power_ir_with_upf(
                                    best,
                                    &vectors,
                                    &post_route_options,
                                    intent,
                                )?
                            }
                            (None, None) => {
                                let vectors =
                                    eda_api::default_power_vectors(&best.annotated_netlist);
                                eda_api::analyze_physical_timing_power_ir(
                                    best,
                                    &vectors,
                                    &post_route_options,
                                )?
                            }
                        };
                        report.push_str(&eda_api::post_route_power_ir_report(&power_ir));
                    }
                }
                if let Some(path) = &ctx.exported_verilog_path {
                    report.push_str(&format!("Best physical Verilog export: {}\n", path));
                }
                return Ok(report);
            }
            if ctx.waiver_rules.is_some() || ctx.waiver_audit.is_some() {
                return Err("flow waivers require --physical-timing".into());
            }
            if let Some(path) = &ctx.lef_out {
                std::fs::write(path, &f.lef).map_err(|e| e.to_string())?;
            }
            if let Some(path) = &ctx.def_out {
                std::fs::write(path, &f.def).map_err(|e| e.to_string())?;
            }
            let mut s = format!("module {}\n", f.design.name);
            s += &format!(
                "  netlist: {} cells ({}, seq {}, const {})\n",
                f.stats.cells, f.stats.comb, f.stats.seq, f.stats.consts
            );
            s += &format!(
                "  equivalence: {}\n",
                if f.equivalent { "OK" } else { "FAIL" }
            );
            s += &eda_sta::report(&f.netlist, &f.timing);
            Ok(s)
        }
        other => Err(format!("unknown command '{}'", other)),
    }
}
