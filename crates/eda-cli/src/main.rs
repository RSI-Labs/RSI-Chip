//! # eda — command-line interface
//!
//! Usage: `eda <command> <file.v> [--top <module>]`
//! Commands: info | synth | equiv | sta | pnr | power | drc | pdk | assert | coverage | cdc | spef | dft | fault | eco | upf | cts | ir | prove | ams | flow | daemon
//! Library: `--lib <file.lib>` imports a Liberty subset.
//! Activity: `--vcd <file.vcd>` drives `eda power` and `eda assert`.
//! Properties: `--sva <file.sva>` drives `eda assert`.
//! Temporal BMC: `--sva <file.sva>` also drives `eda prove`.
//! SVA cover properties are supported by both `assert` and `prove`.
//! SVA assume properties constrain formal `prove` targets.
//! Repeated antecedents such as `req[*2]`, `req[*]`, and `req[+]` are supported.
//! Nonconsecutive antecedents such as `req[=2]` are supported.
//! Two-event sequence antecedents such as `req ##[1:2] ack` are supported.
//! Safety proofs: `--bad <net-name>` drives `eda prove`.
//! Expression proofs: `--expr <boolean-expression>` drives `eda prove`.
//! Proof depth: `--depth <cycles>` controls bounded proof horizons.
//! Formal counterexamples: `--cex <out.vcd>` exports full SAT net traces.
//! Expression invariants: `--assume-expr <name>:<boolean-expression>`.
//! Proof strengthening: `--assume <net[=0|1][,...]>` proves each invariant first.
//! Coverage: `--cover <file.cover>` drives `eda coverage`.
//! Timing: `--sdf <file.sdf>` back-annotates `eda sta`.
//! MMMC timing: `--mmsc <file.mmsc>` supplies inline named timing scenarios.
//! Hierarchy-scoped exceptions use SDC `-instance <pattern>` with retained
//! synthesis hierarchy metadata.
//! Scenario-local `set_scenario_property post_route.*` values configure
//! MMMC post-route power, supply voltage, and rail resistance; properties of
//! the form `post_route.domain.<name>.<field>` configure UPF domain rails.
//! Timing paths: `--paths <count>` appends endpoint path details to `eda sta`;
//! with `--mmsc`, paths are merged by endpoint across scenarios.
//! Hierarchical path grouping is appended automatically when synthesized
//! hierarchy metadata is available.
//! Hold paths: `--hold-paths <count>` appends min-delay path details.
//! Async reset: `--reset-paths <count>` appends recovery/removal paths.
//! Timing waivers: `--waivers <file>` filters timing violations and
//! `--waiver-audit <file>` restores audited waiver lifecycle state.
//! Waiver export: `--write-waivers` and `--write-waiver-audit` serialize the
//! selected active rules or lifecycle history.
//! Waiver-aware physical optimization uses `--physical-timing` in `eda flow`.
//! MMMC physical optimization combines `--mmsc` with `--physical-timing`.
//! Parasitics: `--spef <file.spef>` drives `eda spef` and RC-aware `eda sta`.
//! Detailed routing: `--maze` makes `eda pnr` use rectilinear maze routing.
//! Negotiated routing: `--ripgup <passes>` enables rip-up/reroute.
//! Timing-aware routing: `--timing-ripgup <passes>` prioritizes late STA nets.
//! Antenna repair: `--repair-antennas` in `eda flow` inserts DIODE loads and
//! reroutes physical-timing iterations.
//! Timing-driven CTS: `eda cts --timing-driven-cts` searches branch factors
//! against a physical skew target.
//! Parasitic output: `--extract-spef <out.spef>` estimates routing RC.
//! Best-implementation SDF output: `--write-sdf <out.sdf>` serializes the
//! selected single-corner physical-timing view. With `--mmsc`, the path must
//! contain an explicit `{corner}` placeholder, such as `best.{corner}.sdf`.
//! Best-implementation netlist output: `--write-verilog <out.v>` exports the
//! post-CTS/repair structural view selected by physical timing.
//! Post-route integrity: `--post-route-power` adds SPEF-aware switching power
//! and placement-aware IR/EM analysis to physical timing.
//! Supply domains: `--upf <file.upf>` makes post-route IR/EM domain-resolved.
//! Power modes: `--all-power-modes` analyzes every switch-state table in the
//! supplied UPF instead of only the active mode.
//! Pattern output: `--stil <out.stil>` writes ATE patterns from `eda fault`.
//! ECO: `--eco <file.eco>` drives `eda eco`.
//! Hold repair: `eda eco --sdc <file.sdc> --repair-hold` inserts transparent
//! buffers before hold-critical sequential data pins.
//! Power intent: `--upf <file.upf>` drives `eda upf`.
//! Power mesh: `eda ir --power-mesh` solves a finite-difference VDD/GND grid;
//! with `eda flow --physical-timing --post-route-power`, it uses the best
//! routed implementation and extracted SPEF activity.
//! Physical output: `--lef <out.lef>` and `--def <out.def>`.

use eda_core::library::CellLibrary;
use std::process::ExitCode;

fn parse_invariants(value: &str) -> Result<Vec<(String, bool)>, String> {
    let mut out = Vec::new();
    for item in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let invariant = match item.split_once('=') {
            Some((net, "0")) => (net.trim().to_string(), false),
            Some((net, "1")) => (net.trim().to_string(), true),
            Some(_) => {
                return Err(format!(
                    "invalid invariant '{}'; expected net=0 or net=1",
                    item
                ))
            }
            None => (item.to_string(), false),
        };
        if invariant.0.is_empty() {
            return Err(format!("invalid invariant '{}'", item));
        }
        out.push(invariant);
    }
    if out.is_empty() {
        Err("--assume requires at least one net or net=value".into())
    } else {
        Ok(out)
    }
}

fn parse_proof_expression(value: &str) -> Result<eda_api::BoolExpr, String> {
    let source = format!("assert property ({});", value);
    let assertions = eda_api::parse_sva(&source)?;
    let assertion = assertions
        .first()
        .ok_or_else(|| "no Boolean proof expression found".to_string())?;
    match &assertion.property {
        eda_api::Property::Immediate(expr) | eda_api::Property::Always(expr) => Ok(expr.clone()),
        eda_api::Property::Implication { .. } => {
            Err("temporal implications are not yet supported by expression proving".into())
        }
        eda_api::Property::RepeatedImplication { .. } => {
            Err("repeated temporal implications are not yet supported by expression proving".into())
        }
        eda_api::Property::NonconsecutiveImplication { .. } => Err(
            "nonconsecutive temporal implications are not supported by expression proving".into(),
        ),
        eda_api::Property::SequenceImplication { .. } => {
            Err("sequence antecedents are not supported by expression proving".into())
        }
    }
}

fn parse_named_proof_expression(value: &str) -> Result<eda_api::ProofInvariant, String> {
    let (name, expression) = value
        .split_once(':')
        .ok_or_else(|| "expected <name>:<boolean-expression>".to_string())?;
    let name = name.trim();
    if name.is_empty() {
        return Err("expression invariant requires a name".into());
    }
    Ok(eda_api::ProofInvariant::expression(
        name,
        parse_proof_expression(expression)?,
    ))
}

fn timing_waiver_report(
    netlist: &eda_core::netlist::Netlist,
    constrained: &eda_api::ConstrainedTiming,
    waivers: Option<&[eda_api::TimingViolationWaiver]>,
    audit: Option<&[eda_api::TimingWaiverAuditEntry]>,
) -> Result<String, String> {
    if waivers.is_some() && audit.is_some() {
        return Err("choose either --waivers or --waiver-audit, not both".into());
    }
    let mut store = eda_api::timing_violation_store(&constrained.clock, netlist, constrained)?;
    if let Some(waivers) = waivers {
        for waiver in waivers {
            eda_api::add_timing_violation_waiver(&mut store, waiver.clone())?;
        }
    }
    if let Some(entries) = audit {
        eda_api::restore_timing_waiver_audit(&mut store, entries)?;
    }
    let mut out = eda_api::timing_violation_report(&store, None);
    if audit.is_some() {
        out.push_str(&eda_api::timing_waiver_audit_report(store.audit()));
        out.push_str(&eda_api::timing_waiver_audit_hierarchy_report(
            store.audit(),
        )?);
    }
    Ok(out)
}

fn optimization_waivers(
    waivers: Option<&[eda_api::TimingViolationWaiver]>,
    audit: Option<&[eda_api::TimingWaiverAuditEntry]>,
) -> Result<Option<Vec<eda_api::TimingViolationWaiver>>, String> {
    if let Some(waivers) = waivers {
        Ok(Some(waivers.to_vec()))
    } else if let Some(entries) = audit {
        Ok(Some(eda_api::active_timing_waivers_from_audit(entries)?))
    } else {
        Ok(None)
    }
}

fn write_waiver_exports(
    waiver_rules: Option<&[eda_api::TimingViolationWaiver]>,
    waiver_audit: Option<&[eda_api::TimingWaiverAuditEntry]>,
    waiver_output: Option<&str>,
    audit_output: Option<&str>,
) -> Result<(), String> {
    if waiver_output.is_none() && audit_output.is_none() {
        return Ok(());
    }
    if waiver_rules.is_some() && waiver_audit.is_some() {
        return Err("choose either --waivers or --waiver-audit for waiver export".into());
    }
    if waiver_rules.is_none() && waiver_audit.is_none() {
        return Err("waiver export requires --waivers or --waiver-audit".into());
    }
    if let Some(path) = waiver_output {
        let waivers = if let Some(rules) = waiver_rules {
            rules.to_vec()
        } else {
            eda_api::active_timing_waivers_from_audit(
                waiver_audit.ok_or("waiver export source is missing")?,
            )?
        };
        let text = eda_api::write_timing_violation_waivers(&waivers)?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    if let Some(path) = audit_output {
        let text = if let Some(rules) = waiver_rules {
            let mut store = eda_api::TimingViolationStore::new("export");
            for waiver in rules {
                eda_api::add_timing_violation_waiver(&mut store, waiver.clone())?;
            }
            eda_api::write_timing_waiver_audit(store.audit())?
        } else {
            eda_api::write_timing_waiver_audit(
                waiver_audit.ok_or("waiver export source is missing")?,
            )?
        };
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
fn constrained_sta_report(
    netlist: &eda_core::netlist::Netlist,
    sdc: &str,
) -> Result<String, String> {
    let constraints = eda_api::parse_timing_constraints(sdc)?;
    let timing = eda_sta::analyze_with_constraints(netlist, &constraints)?;
    let constrained = eda_sta::analyze_constrained(netlist, timing, &constraints)?;
    Ok(eda_sta::constrained_report(&constrained))
}

fn multicorner_timing_waiver_report<'a>(
    _result: &eda_api::MultiCornerTimingResult,
    scenarios: &[eda_api::TimingScenario<'a>],
    waivers: Option<&[eda_api::TimingViolationWaiver]>,
    audit: Option<&[eda_api::TimingWaiverAuditEntry]>,
) -> Result<String, String> {
    if waivers.is_some() && audit.is_some() {
        return Err("choose either --waivers or --waiver-audit, not both".into());
    }
    let mut registry = eda_api::timing_violation_registry(scenarios.iter().copied())?;
    if let Some(waivers) = waivers {
        for waiver in waivers {
            eda_api::add_timing_violation_registry_waiver(&mut registry, waiver.clone())?;
        }
    }
    if let Some(entries) = audit {
        eda_api::restore_timing_waiver_registry_audit(&mut registry, entries)?;
    }
    let mut out = eda_api::timing_violation_registry_report(&registry, None);
    if audit.is_some() {
        out.push_str(&eda_api::timing_waiver_audit_report(registry.audit()));
        out.push_str(&eda_api::timing_waiver_audit_hierarchy_report(
            registry.audit(),
        )?);
    }
    Ok(out)
}

fn multilayer_mesh_options(rows: usize) -> eda_api::MultilayerPowerMeshOptions {
    let mut options = eda_api::MultilayerPowerMeshOptions {
        rows,
        ..Default::default()
    };
    options.use_boundary_taps = false;
    options.layers = vec![
        eda_api::PowerMeshLayer::new("M8", 0.02, 0.02, 0.02, 0.02),
        eda_api::PowerMeshLayer::new("M7", 0.03, 0.03, 0.03, 0.03),
    ];
    for row in 0..options.rows {
        for column in 0..options.columns {
            if row == 0 || row + 1 == options.rows || column == 0 || column + 1 == options.columns {
                options.taps.push(eda_api::PowerMeshLayerTap {
                    layer: 0,
                    row,
                    column,
                    vdd: true,
                    gnd: true,
                });
            }
            options
                .vias
                .push(eda_api::PowerMeshVia::new(row, column, 0, 1, 0.02, 0.02));
        }
    }
    options
}

fn coupled_multilayer_mesh_options(
    rows: usize,
    supplies: &[eda_api::MultilayerPowerDomainSupply],
) -> eda_api::MultilayerPowerMeshOptions {
    let mut options = multilayer_mesh_options(rows);
    for tap in &mut options.taps {
        tap.vdd = false;
    }
    options.power_domains = supplies.to_vec();
    options
}

fn sta_paths_report(
    netlist: &eda_core::netlist::Netlist,
    timing: &eda_api::StaResult,
    constrained: Option<&eda_api::ConstrainedTiming>,
    path_count: usize,
) -> Result<String, String> {
    let mut out;
    if let Some(constrained) = constrained {
        let paths = eda_api::constrained_timing_paths(netlist, constrained, path_count);
        out = eda_api::constrained_timing_path_report(netlist, &paths);
        if !netlist.hierarchy.is_empty() {
            out.push_str(&eda_api::hierarchical_timing_path_report(
                &eda_api::hierarchical_constrained_timing_paths(netlist, &paths)?,
            ));
        }
    } else {
        let paths = eda_api::timing_paths(netlist, timing, path_count);
        out = eda_api::timing_path_report(netlist, &paths);
        if !netlist.hierarchy.is_empty() {
            out.push_str(&eda_api::hierarchical_timing_path_report(
                &eda_api::hierarchical_timing_paths(netlist, &paths)?,
            ));
        }
    }
    Ok(out)
}

fn sta_hold_paths_report(
    netlist: &eda_core::netlist::Netlist,
    timing: &eda_api::StaResult,
    constrained: Option<&eda_api::ConstrainedTiming>,
    path_count: usize,
) -> Result<String, String> {
    let mut out;
    if let Some(constrained) = constrained {
        let paths = eda_api::constrained_hold_timing_paths(netlist, constrained, path_count);
        out = eda_api::constrained_hold_timing_path_report(netlist, &paths);
        if !netlist.hierarchy.is_empty() {
            out.push_str(&eda_api::hierarchical_timing_path_report(
                &eda_api::hierarchical_constrained_hold_timing_paths(netlist, &paths)?,
            ));
        }
    } else {
        let paths = eda_api::hold_timing_paths(netlist, timing, 0.0, path_count);
        out = eda_api::hold_timing_path_report(netlist, &paths);
        if !netlist.hierarchy.is_empty() {
            out.push_str(&eda_api::hierarchical_timing_path_report(
                &eda_api::hierarchical_hold_timing_paths(netlist, &paths)?,
            ));
        }
    }
    Ok(out)
}

fn sta_reset_paths_report(
    netlist: &eda_core::netlist::Netlist,
    timing: &eda_api::StaResult,
    constrained: Option<&eda_api::ConstrainedTiming>,
    path_count: usize,
) -> Result<String, String> {
    let mut out;
    if let Some(constrained) = constrained {
        let paths = eda_api::constrained_reset_timing_paths(netlist, constrained, path_count);
        out = eda_api::constrained_reset_timing_path_report(netlist, &paths);
        if !netlist.hierarchy.is_empty() {
            out.push_str(&eda_api::hierarchical_timing_path_report(
                &eda_api::hierarchical_constrained_reset_timing_paths(netlist, &paths)?,
            ));
        }
    } else {
        let paths = eda_api::reset_timing_paths(netlist, timing, path_count);
        out = eda_api::reset_timing_path_report(netlist, &paths);
        if !netlist.hierarchy.is_empty() {
            out.push_str(&eda_api::hierarchical_timing_path_report(
                &eda_api::hierarchical_reset_timing_paths(netlist, &paths)?,
            ));
        }
    }
    Ok(out)
}

fn multicorner_sta_report(
    netlist: &eda_core::netlist::Netlist,
    source: &str,
    setup_path_count: Option<usize>,
    hold_path_count: Option<usize>,
    reset_path_count: Option<usize>,
    waivers: Option<&[eda_api::TimingViolationWaiver]>,
    audit: Option<&[eda_api::TimingWaiverAuditEntry]>,
) -> Result<String, String> {
    let scenarios = eda_api::parse_timing_scenarios(source)?;
    let result = eda_api::analyze_timing_scenarios_from_definitions(netlist, &scenarios)?;
    let mut report = eda_api::multi_corner_timing_report(&result);
    let views = scenarios
        .iter()
        .map(|scenario| {
            eda_api::TimingScenario::new(scenario.name.as_str(), netlist, &scenario.constraints)
        })
        .collect::<Vec<_>>();
    if waivers.is_some() || audit.is_some() {
        report.push_str(&multicorner_timing_waiver_report(
            &result, &views, waivers, audit,
        )?);
    }
    if let Some(path_count) = setup_path_count {
        let paths =
            eda_api::multi_corner_setup_timing_paths(&result, views.iter().copied(), path_count)?;
        report.push_str(&eda_api::multi_corner_setup_timing_path_report(&paths));
    }
    if let Some(path_count) = hold_path_count {
        let paths =
            eda_api::multi_corner_hold_timing_paths(&result, views.iter().copied(), path_count)?;
        report.push_str(&eda_api::multi_corner_hold_timing_path_report(&paths));
    }
    if let Some(path_count) = reset_path_count {
        let paths =
            eda_api::multi_corner_reset_timing_paths(&result, views.iter().copied(), path_count)?;
        report.push_str(&eda_api::multi_corner_reset_timing_path_report(&paths));
    }
    Ok(report)
}

fn write_extracted_spef(
    path: &str,
    netlist: &eda_core::netlist::Netlist,
    routing: &eda_pnr::Routing,
) -> Result<(), String> {
    let parasitics = eda_api::extract_interconnect_parasitics(
        netlist,
        routing,
        &eda_api::InterconnectModel::default(),
    )?;
    let text = eda_api::write_parasitics(&parasitics)?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

fn temporal_bmc_report(
    netlist: &eda_core::netlist::Netlist,
    source: &str,
    depth: usize,
    cex_path: Option<&str>,
) -> Result<String, String> {
    let properties = eda_api::parse_sva(source)?;
    let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
        .iter()
        .cloned()
        .partition(|property| property.kind != eda_api::AssertionKind::Assume);
    if assertions.is_empty() {
        return Err("SVA input contains assumptions but no assert or cover targets".into());
    }
    let results = eda_api::check_assertions_bounded_with_assumptions(
        netlist,
        &assertions,
        &assumptions,
        depth,
    )?;
    if let Some(path) = cex_path {
        let Some(violation) = results.iter().find(|result| result.violated()) else {
            return Err("--cex requires a violating property".into());
        };
        eda_api::verify_property_counterexample(netlist, violation)?;
        let text = eda_api::export_property_counterexample_vcd(netlist, violation)?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    let mut report = assumptions
        .iter()
        .map(|assumption| {
            format!(
                "Temporal BMC report '{}': ASSUMPTION CONSTRAINT applied\n",
                assumption.name
            )
        })
        .collect::<Vec<_>>()
        .join("");
    report.push_str(
        &results
            .iter()
            .map(eda_api::property_bmc_report)
            .collect::<Vec<_>>()
            .join(""),
    );
    Ok(report)
}

fn write_proof_cex(
    path: &str,
    netlist: &eda_core::netlist::Netlist,
    proof: &eda_api::ProofResult,
) -> Result<(), String> {
    let text = eda_api::export_proof_counterexample_vcd(netlist, proof)?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

#[allow(clippy::too_many_arguments)]
fn write_best_physical_timing_outputs(
    lef_out: Option<&String>,
    def_out: Option<&String>,
    spef_out: Option<&String>,
    sdf_out: Option<&String>,
    verilog_out: Option<&String>,
    netlist: &eda_core::netlist::Netlist,
    placement: &eda_api::Placement,
    routing: &eda_pnr::Routing,
    parasitics: &eda_api::Spef,
    sdf: &eda_api::Sdf,
) -> Result<(), String> {
    if let Some(path) = lef_out {
        let lef = eda_pnr::write_lef(netlist, placement)?;
        std::fs::write(path, lef).map_err(|e| e.to_string())?;
    }
    if let Some(path) = def_out {
        let def = eda_pnr::write_def(netlist, placement, routing)?;
        std::fs::write(path, def).map_err(|e| e.to_string())?;
    }
    if let Some(path) = spef_out {
        let text = eda_api::write_parasitics(parasitics)?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    if let Some(path) = sdf_out {
        let text = eda_api::write_delay_file(sdf)?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    if let Some(path) = verilog_out {
        let text = eda_api::export_verilog(netlist)?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("capabilities") {
        print!("{}", eda_api::capability_report());
        return ExitCode::SUCCESS;
    }
    if args.get(1).map(String::as_str) == Some("daemon") {
        let mut listen = "127.0.0.1:7822".to_string();
        let mut index = 2;
        while index < args.len() {
            if args[index] == "--listen" {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("error: --listen requires <ip>:<port>");
                    return ExitCode::from(2);
                };
                listen = value.clone();
                index += 2;
            } else {
                eprintln!("error: unknown daemon option '{}'", args[index]);
                return ExitCode::from(2);
            }
        }
        return match eda_daemon::Daemon::bind(&listen) {
            Ok(daemon) => {
                println!("EDA daemon listening on {}", daemon.local_address());
                match daemon.wait() {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(error) => {
                        eprintln!("error: {error}");
                        ExitCode::FAILURE
                    }
                }
            }
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if args.len() < 3 {
        eprintln!(
            "usage: eda capabilities | eda daemon [--listen <ip>:<port>] | eda <info|synth|equiv|sta|pnr|power|drc|pdk|assert|coverage|cdc|spef|dft|fault|eco|upf|cts|ir|prove|ams|flow> <file.v> [--top <module>]"
        );
        return ExitCode::from(2);
    }
    let cmd = args[1].clone();
    let file = args[2].clone();
    let mut top: Option<String> = None;
    let mut sdc_path: Option<String> = None;
    let mut mmsc_path: Option<String> = None;
    let mut waiver_path: Option<String> = None;
    let mut waiver_audit_path: Option<String> = None;
    let mut waiver_export_path: Option<String> = None;
    let mut waiver_audit_export_path: Option<String> = None;
    let mut lib_path: Option<String> = None;
    let mut pdk_path: Option<String> = None;
    let mut vcd_path: Option<String> = None;
    let mut sva_path: Option<String> = None;
    let mut statistical_model_path: Option<String> = None;
    let mut def_locations_path: Option<String> = None;
    let mut resistor_layer_model_path: Option<String> = None;
    let mut sdf_path: Option<String> = None;
    let mut spef_path: Option<String> = None;
    let mut extracted_spef_path: Option<String> = None;
    let mut exported_sdf_path: Option<String> = None;
    let mut exported_verilog_path: Option<String> = None;
    let mut cover_path: Option<String> = None;
    let mut maze_routing = false;
    let mut power_mesh = false;
    let mut multilayer_power_mesh = false;
    let mut power_mesh_domains_path: Option<String> = None;
    let mut ripgup_passes: Option<usize> = None;
    let mut timing_ripgup_passes: Option<usize> = None;
    let mut physical_timing = false;
    let mut post_route_power = false;
    let mut all_power_modes = false;
    let mut repair_antennas = false;
    let mut antenna_diode_limit = 4usize;
    let mut antenna_diode_limit_set = false;
    let mut timing_path_count: Option<usize> = None;
    let mut hold_path_count: Option<usize> = None;
    let mut reset_path_count: Option<usize> = None;
    let mut lef_out: Option<String> = None;
    let mut def_out: Option<String> = None;
    let mut def_input_path: Option<String> = None;
    let mut stil_out: Option<String> = None;
    let mut eco_path: Option<String> = None;
    let mut repair_hold = false;
    let mut hold_buffer_limit = 4usize;
    let mut hold_buffer_limit_set = false;
    let mut timing_driven_cts = false;
    let mut cts_target_skew = 0.05_f64;
    let mut cts_target_skew_set = false;
    let mut upf_path: Option<String> = None;
    let mut bad_net: Option<String> = None;
    let mut cex_path: Option<String> = None;
    let mut proof_expr: Option<eda_api::BoolExpr> = None;
    let mut proof_depth = 16usize;
    let mut invariant_nets: Vec<(String, bool)> = Vec::new();
    let mut expression_invariants: Vec<eda_api::ProofInvariant> = Vec::new();
    let mut i = 3;
    while i < args.len() {
        if args.get(i).map(|s| s == "--top").unwrap_or(false) {
            if let Some(t) = args.get(i + 1) {
                top = Some(t.clone());
            }
        }
        if args.get(i).map(|s| s == "--sdc").unwrap_or(false) {
            if let Some(path) = args.get(i + 1) {
                sdc_path = Some(path.clone());
            }
        }
        if args.get(i).map(|s| s == "--mmsc").unwrap_or(false) {
            match args.get(i + 1) {
                Some(path) => mmsc_path = Some(path.clone()),
                None => {
                    eprintln!("error: --mmsc requires a timing scenario file");
                    return ExitCode::from(2);
                }
            }
        }
        if args.get(i).map(|s| s == "--waivers").unwrap_or(false) {
            match args.get(i + 1) {
                Some(path) => waiver_path = Some(path.clone()),
                None => {
                    eprintln!("error: --waivers requires a timing waiver file");
                    return ExitCode::from(2);
                }
            }
        }
        if args.get(i).map(|s| s == "--waiver-audit").unwrap_or(false) {
            match args.get(i + 1) {
                Some(path) => waiver_audit_path = Some(path.clone()),
                None => {
                    eprintln!("error: --waiver-audit requires a timing waiver audit file");
                    return ExitCode::from(2);
                }
            }
        }
        if args.get(i).map(|s| s == "--write-waivers").unwrap_or(false) {
            match args.get(i + 1) {
                Some(path) => waiver_export_path = Some(path.clone()),
                None => {
                    eprintln!("error: --write-waivers requires an output file");
                    return ExitCode::from(2);
                }
            }
        }
        if args
            .get(i)
            .map(|s| s == "--write-waiver-audit")
            .unwrap_or(false)
        {
            match args.get(i + 1) {
                Some(path) => waiver_audit_export_path = Some(path.clone()),
                None => {
                    eprintln!("error: --write-waiver-audit requires an output file");
                    return ExitCode::from(2);
                }
            }
        }
        if args.get(i).map(|s| s == "--maze").unwrap_or(false) {
            maze_routing = true;
        }
        if args.get(i).map(|s| s == "--power-mesh").unwrap_or(false) {
            power_mesh = true;
        }
        if args
            .get(i)
            .map(|s| s == "--multilayer-power-mesh")
            .unwrap_or(false)
        {
            multilayer_power_mesh = true;
        }
        if args
            .get(i)
            .map(|s| s == "--power-mesh-domains")
            .unwrap_or(false)
        {
            match args.get(i + 1) {
                Some(path) => power_mesh_domains_path = Some(path.clone()),
                None => {
                    eprintln!("error: --power-mesh-domains requires a supply-domain file");
                    return ExitCode::from(2);
                }
            }
        }
        if args
            .get(i)
            .map(|s| s == "--physical-timing")
            .unwrap_or(false)
        {
            physical_timing = true;
        }
        if args
            .get(i)
            .map(|s| s == "--post-route-power")
            .unwrap_or(false)
        {
            post_route_power = true;
        }
        if args
            .get(i)
            .map(|s| s == "--all-power-modes")
            .unwrap_or(false)
        {
            all_power_modes = true;
        }
        if args
            .get(i)
            .map(|s| s == "--repair-antennas")
            .unwrap_or(false)
        {
            repair_antennas = true;
        }
        if args
            .get(i)
            .map(|s| s == "--antenna-max-diodes")
            .unwrap_or(false)
        {
            let Some(value) = args
                .get(i + 1)
                .and_then(|value| value.parse::<usize>().ok())
            else {
                eprintln!("error: --antenna-max-diodes requires a positive count");
                return ExitCode::from(2);
            };
            if value == 0 {
                eprintln!("error: --antenna-max-diodes requires a positive count");
                return ExitCode::from(2);
            }
            antenna_diode_limit = value;
            antenna_diode_limit_set = true;
        }
        if args
            .get(i)
            .map(|s| s == "--timing-driven-cts")
            .unwrap_or(false)
        {
            timing_driven_cts = true;
        }
        if args
            .get(i)
            .map(|s| s == "--cts-target-skew")
            .unwrap_or(false)
        {
            let Some(value) = args.get(i + 1).and_then(|value| value.parse::<f64>().ok()) else {
                eprintln!("error: --cts-target-skew requires a nonnegative nanosecond value");
                return ExitCode::from(2);
            };
            if !value.is_finite() || value < 0.0 {
                eprintln!("error: --cts-target-skew requires a nonnegative nanosecond value");
                return ExitCode::from(2);
            }
            cts_target_skew = value;
            cts_target_skew_set = true;
        }
        if args.get(i).map(|s| s == "--ripgup").unwrap_or(false) {
            let Some(value) = args
                .get(i + 1)
                .and_then(|value| value.parse::<usize>().ok())
            else {
                eprintln!("error: --ripgup requires a positive pass count");
                return ExitCode::from(2);
            };
            if value == 0 {
                eprintln!("error: --ripgup requires a positive pass count");
                return ExitCode::from(2);
            }
            ripgup_passes = Some(value);
        }
        if args.get(i).map(|s| s == "--timing-ripgup").unwrap_or(false) {
            let Some(value) = args
                .get(i + 1)
                .and_then(|value| value.parse::<usize>().ok())
            else {
                eprintln!("error: --timing-ripgup requires a positive pass count");
                return ExitCode::from(2);
            };
            if value == 0 {
                eprintln!("error: --timing-ripgup requires a positive pass count");
                return ExitCode::from(2);
            }
            timing_ripgup_passes = Some(value);
        }
        if args.get(i).map(|s| s == "--paths").unwrap_or(false) {
            let Some(value) = args
                .get(i + 1)
                .and_then(|value| value.parse::<usize>().ok())
            else {
                eprintln!("error: --paths requires a positive path count");
                return ExitCode::from(2);
            };
            if value == 0 {
                eprintln!("error: --paths requires a positive path count");
                return ExitCode::from(2);
            }
            timing_path_count = Some(value);
        }
        if args.get(i).map(|s| s == "--hold-paths").unwrap_or(false) {
            let Some(value) = args
                .get(i + 1)
                .and_then(|value| value.parse::<usize>().ok())
            else {
                eprintln!("error: --hold-paths requires a positive path count");
                return ExitCode::from(2);
            };
            if value == 0 {
                eprintln!("error: --hold-paths requires a positive path count");
                return ExitCode::from(2);
            }
            hold_path_count = Some(value);
        }
        if args.get(i).map(|s| s == "--reset-paths").unwrap_or(false) {
            let Some(value) = args
                .get(i + 1)
                .and_then(|value| value.parse::<usize>().ok())
            else {
                eprintln!("error: --reset-paths requires a positive path count");
                return ExitCode::from(2);
            };
            if value == 0 {
                eprintln!("error: --reset-paths requires a positive path count");
                return ExitCode::from(2);
            }
            reset_path_count = Some(value);
        }
        if args.get(i).map(|s| s == "--extract-spef").unwrap_or(false) {
            match args.get(i + 1) {
                Some(path) => extracted_spef_path = Some(path.clone()),
                None => {
                    eprintln!("error: --extract-spef requires an output SPEF path");
                    return ExitCode::from(2);
                }
            }
        }
        if args.get(i).map(|s| s == "--write-sdf").unwrap_or(false) {
            match args.get(i + 1) {
                Some(path) => exported_sdf_path = Some(path.clone()),
                None => {
                    eprintln!("error: --write-sdf requires an output SDF path");
                    return ExitCode::from(2);
                }
            }
        }
        if args.get(i).map(|s| s == "--write-verilog").unwrap_or(false) {
            match args.get(i + 1) {
                Some(path) => exported_verilog_path = Some(path.clone()),
                None => {
                    eprintln!("error: --write-verilog requires an output Verilog path");
                    return ExitCode::from(2);
                }
            }
        }
        if args.get(i).map(|s| s == "--lib").unwrap_or(false) {
            if let Some(path) = args.get(i + 1) {
                lib_path = Some(path.clone());
            }
        }
        if args.get(i).map(|s| s == "--pdk").unwrap_or(false) {
            match args.get(i + 1) {
                Some(path) => pdk_path = Some(path.clone()),
                None => {
                    eprintln!("error: --pdk requires a PDK manifest file");
                    return ExitCode::from(2);
                }
            }
        }
        if args.get(i).map(|s| s == "--vcd").unwrap_or(false) {
            if let Some(path) = args.get(i + 1) {
                vcd_path = Some(path.clone());
            }
        }
        if args.get(i).map(|s| s == "--sva").unwrap_or(false) {
            if let Some(path) = args.get(i + 1) {
                sva_path = Some(path.clone());
            }
        }
        if args.get(i).map(|s| s == "--stats").unwrap_or(false) {
            match args.get(i + 1) {
                Some(path) => statistical_model_path = Some(path.clone()),
                None => {
                    eprintln!("error: --stats requires a statistical model file");
                    return ExitCode::from(2);
                }
            }
        }
        if args.get(i).map(|s| s == "--def-locations").unwrap_or(false) {
            match args.get(i + 1) {
                Some(path) => def_locations_path = Some(path.clone()),
                None => {
                    eprintln!("error: --def-locations requires a DEF file");
                    return ExitCode::from(2);
                }
            }
        }
        if args.get(i).map(|s| s == "--rmodels").unwrap_or(false) {
            match args.get(i + 1) {
                Some(path) => resistor_layer_model_path = Some(path.clone()),
                None => {
                    eprintln!("error: --rmodels requires a resistor layer table");
                    return ExitCode::from(2);
                }
            }
        }
        if args.get(i).map(|s| s == "--sdf").unwrap_or(false) {
            if let Some(path) = args.get(i + 1) {
                sdf_path = Some(path.clone());
            }
        }
        if args.get(i).map(|s| s == "--spef").unwrap_or(false) {
            if let Some(path) = args.get(i + 1) {
                spef_path = Some(path.clone());
            }
        }
        if args.get(i).map(|s| s == "--cover").unwrap_or(false) {
            if let Some(path) = args.get(i + 1) {
                cover_path = Some(path.clone());
            }
        }
        if args.get(i).map(|s| s == "--lef").unwrap_or(false) {
            if let Some(path) = args.get(i + 1) {
                lef_out = Some(path.clone());
            }
        }
        if args.get(i).map(|s| s == "--def").unwrap_or(false) {
            if let Some(path) = args.get(i + 1) {
                def_out = Some(path.clone());
            }
        }
        if args.get(i).map(|s| s == "--import-def").unwrap_or(false) {
            match args.get(i + 1) {
                Some(path) => def_input_path = Some(path.clone()),
                None => {
                    eprintln!("error: --import-def requires an input DEF file");
                    return ExitCode::from(2);
                }
            }
        }
        if args.get(i).map(|s| s == "--stil").unwrap_or(false) {
            if let Some(path) = args.get(i + 1) {
                stil_out = Some(path.clone());
            }
        }
        if args.get(i).map(|s| s == "--eco").unwrap_or(false) {
            if let Some(path) = args.get(i + 1) {
                eco_path = Some(path.clone());
            }
        }
        if args.get(i).map(|s| s == "--repair-hold").unwrap_or(false) {
            repair_hold = true;
        }
        if args
            .get(i)
            .map(|s| s == "--hold-max-buffers")
            .unwrap_or(false)
        {
            let Some(value) = args
                .get(i + 1)
                .and_then(|value| value.parse::<usize>().ok())
            else {
                eprintln!("error: --hold-max-buffers requires a positive count");
                return ExitCode::from(2);
            };
            if value == 0 {
                eprintln!("error: --hold-max-buffers requires a positive count");
                return ExitCode::from(2);
            }
            hold_buffer_limit = value;
            hold_buffer_limit_set = true;
        }
        if args.get(i).map(|s| s == "--upf").unwrap_or(false) {
            if let Some(path) = args.get(i + 1) {
                upf_path = Some(path.clone());
            }
        }
        if args.get(i).map(|s| s == "--bad").unwrap_or(false) {
            if let Some(net) = args.get(i + 1) {
                bad_net = Some(net.clone());
            }
        }
        if args.get(i).map(|s| s == "--depth").unwrap_or(false) {
            let Some(value) = args
                .get(i + 1)
                .and_then(|value| value.parse::<usize>().ok())
            else {
                eprintln!("error: --depth requires a positive cycle count");
                return ExitCode::from(2);
            };
            if value == 0 {
                eprintln!("error: --depth requires a positive cycle count");
                return ExitCode::from(2);
            }
            proof_depth = value;
        }
        if args.get(i).map(|s| s == "--cex").unwrap_or(false) {
            match args.get(i + 1) {
                Some(path) => cex_path = Some(path.clone()),
                None => {
                    eprintln!("error: --cex requires an output VCD path");
                    return ExitCode::from(2);
                }
            }
        }
        if args.get(i).map(|s| s == "--assume").unwrap_or(false) {
            if let Some(value) = args.get(i + 1) {
                match parse_invariants(value) {
                    Ok(parsed) => invariant_nets.extend(parsed),
                    Err(e) => {
                        eprintln!("error: {}", e);
                        return ExitCode::FAILURE;
                    }
                }
            }
        }
        if args.get(i).map(|s| s == "--expr").unwrap_or(false) {
            match args
                .get(i + 1)
                .map(String::as_str)
                .map(parse_proof_expression)
            {
                Some(Ok(expr)) => proof_expr = Some(expr),
                Some(Err(e)) => {
                    eprintln!("error: {}", e);
                    return ExitCode::FAILURE;
                }
                None => {
                    eprintln!("error: --expr requires a Boolean expression");
                    return ExitCode::FAILURE;
                }
            }
        }
        if args.get(i).map(|s| s == "--assume-expr").unwrap_or(false) {
            match args
                .get(i + 1)
                .map(String::as_str)
                .map(parse_named_proof_expression)
            {
                Some(Ok(invariant)) => expression_invariants.push(invariant),
                Some(Err(e)) => {
                    eprintln!("error: {}", e);
                    return ExitCode::FAILURE;
                }
                None => {
                    eprintln!("error: --assume-expr requires <name>:<boolean-expression>");
                    return ExitCode::FAILURE;
                }
            }
        }
        i += 1;
    }
    let src = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {}", file, e);
            return ExitCode::FAILURE;
        }
    };
    let pdk = match pdk_path.as_deref().map(std::fs::read_to_string) {
        Some(Ok(text)) => match eda_api::parse_pdk(&text) {
            Ok(pdk) => Some(pdk),
            Err(error) => {
                eprintln!(
                    "error: invalid PDK manifest {}: {}",
                    pdk_path.unwrap_or_default(),
                    error
                );
                return ExitCode::FAILURE;
            }
        },
        Some(Err(error)) => {
            eprintln!(
                "error: cannot read {}: {}",
                pdk_path.unwrap_or_default(),
                error
            );
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let sdc = match sdc_path.as_deref().map(std::fs::read_to_string) {
        Some(Ok(text)) => Some(text),
        Some(Err(e)) => {
            eprintln!("error: cannot read {}: {}", sdc_path.unwrap_or_default(), e);
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let mmsc = match mmsc_path.as_deref().map(std::fs::read_to_string) {
        Some(Ok(text)) => Some(text),
        Some(Err(e)) => {
            eprintln!(
                "error: cannot read {}: {}",
                mmsc_path.unwrap_or_default(),
                e
            );
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let waiver_rules = match waiver_path.as_deref().map(std::fs::read_to_string) {
        Some(Ok(text)) => match eda_api::parse_timing_violation_waivers(&text) {
            Ok(waivers) => Some(waivers),
            Err(e) => {
                eprintln!(
                    "error: invalid waiver file {}: {}",
                    waiver_path.unwrap_or_default(),
                    e
                );
                return ExitCode::FAILURE;
            }
        },
        Some(Err(e)) => {
            eprintln!(
                "error: cannot read {}: {}",
                waiver_path.unwrap_or_default(),
                e
            );
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let waiver_audit = match waiver_audit_path.as_deref().map(std::fs::read_to_string) {
        Some(Ok(text)) => match eda_api::parse_timing_waiver_audit(&text) {
            Ok(entries) => Some(entries),
            Err(e) => {
                eprintln!(
                    "error: invalid waiver audit file {}: {}",
                    waiver_audit_path.unwrap_or_default(),
                    e
                );
                return ExitCode::FAILURE;
            }
        },
        Some(Err(e)) => {
            eprintln!(
                "error: cannot read {}: {}",
                waiver_audit_path.unwrap_or_default(),
                e
            );
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let def_input = match def_input_path.as_deref().map(std::fs::read_to_string) {
        Some(Ok(text)) => Some(text),
        Some(Err(e)) => {
            eprintln!(
                "error: cannot read {}: {}",
                def_input_path.unwrap_or_default(),
                e
            );
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let imported_lib = match lib_path.as_deref().map(std::fs::read_to_string) {
        Some(Ok(text)) => match eda_api::parse_liberty(&text) {
            Ok(lib) => Some(lib),
            Err(e) => {
                eprintln!(
                    "error: invalid Liberty file {}: {}",
                    lib_path.unwrap_or_default(),
                    e
                );
                return ExitCode::FAILURE;
            }
        },
        Some(Err(e)) => {
            eprintln!("error: cannot read {}: {}", lib_path.unwrap_or_default(), e);
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let std_lib = CellLibrary::std();
    let active_lib = imported_lib.as_ref().unwrap_or(&std_lib);
    let vcd = match vcd_path.as_deref().map(std::fs::read_to_string) {
        Some(Ok(text)) => match eda_api::parse_vcd(&text) {
            Ok(vcd) => Some(vcd),
            Err(e) => {
                eprintln!(
                    "error: invalid VCD file {}: {}",
                    vcd_path.unwrap_or_default(),
                    e
                );
                return ExitCode::FAILURE;
            }
        },
        Some(Err(e)) => {
            eprintln!("error: cannot read {}: {}", vcd_path.unwrap_or_default(), e);
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let sva = match sva_path.as_deref().map(std::fs::read_to_string) {
        Some(Ok(text)) => Some(text),
        Some(Err(e)) => {
            eprintln!("error: cannot read {}: {}", sva_path.unwrap_or_default(), e);
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let sdf = match sdf_path.as_deref().map(std::fs::read_to_string) {
        Some(Ok(text)) => Some(text),
        Some(Err(e)) => {
            eprintln!("error: cannot read {}: {}", sdf_path.unwrap_or_default(), e);
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let spef = match spef_path.as_deref().map(std::fs::read_to_string) {
        Some(Ok(text)) => Some(text),
        Some(Err(e)) => {
            eprintln!(
                "error: cannot read {}: {}",
                spef_path.unwrap_or_default(),
                e
            );
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let cover = match cover_path.as_deref().map(std::fs::read_to_string) {
        Some(Ok(text)) => Some(text),
        Some(Err(e)) => {
            eprintln!(
                "error: cannot read {}: {}",
                cover_path.unwrap_or_default(),
                e
            );
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let statistical_model = match statistical_model_path
        .as_deref()
        .map(std::fs::read_to_string)
    {
        Some(Ok(text)) => match eda_api::parse_statistical_model(&text) {
            Ok(model) => Some(model),
            Err(e) => {
                eprintln!(
                    "error: invalid statistical model file {}: {}",
                    statistical_model_path.unwrap_or_default(),
                    e
                );
                return ExitCode::FAILURE;
            }
        },
        Some(Err(e)) => {
            eprintln!(
                "error: cannot read {}: {}",
                statistical_model_path.unwrap_or_default(),
                e
            );
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let location_def = match def_locations_path.as_deref().map(std::fs::read_to_string) {
        Some(Ok(text)) => match eda_api::parse_physical_def(&text) {
            Ok(def) => Some(def),
            Err(e) => {
                eprintln!(
                    "error: invalid DEF location file {}: {}",
                    def_locations_path.unwrap_or_default(),
                    e
                );
                return ExitCode::FAILURE;
            }
        },
        Some(Err(e)) => {
            eprintln!(
                "error: cannot read {}: {}",
                def_locations_path.unwrap_or_default(),
                e
            );
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let resistor_layer_models = match resistor_layer_model_path
        .as_deref()
        .map(std::fs::read_to_string)
    {
        Some(Ok(text)) => match eda_api::parse_resistor_layer_models(&text) {
            Ok(models) => Some(models),
            Err(e) => {
                eprintln!(
                    "error: invalid resistor layer table {}: {}",
                    resistor_layer_model_path.unwrap_or_default(),
                    e
                );
                return ExitCode::FAILURE;
            }
        },
        Some(Err(e)) => {
            eprintln!(
                "error: cannot read {}: {}",
                resistor_layer_model_path.unwrap_or_default(),
                e
            );
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let eco = match eco_path.as_deref().map(std::fs::read_to_string) {
        Some(Ok(text)) => Some(text),
        Some(Err(e)) => {
            eprintln!("error: cannot read {}: {}", eco_path.unwrap_or_default(), e);
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let upf = match upf_path.as_deref().map(std::fs::read_to_string) {
        Some(Ok(text)) => Some(text),
        Some(Err(e)) => {
            eprintln!("error: cannot read {}: {}", upf_path.unwrap_or_default(), e);
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let power_intent = match upf.as_deref().map(eda_api::parse_upf) {
        Some(Ok(intent)) => Some(intent),
        Some(Err(e)) => {
            eprintln!(
                "error: invalid UPF file {}: {}",
                upf_path.unwrap_or_default(),
                e
            );
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let power_domain_supplies = match power_mesh_domains_path
        .as_deref()
        .map(std::fs::read_to_string)
    {
        Some(Ok(text)) => match eda_api::parse_multilayer_power_domain_supplies(&text) {
            Ok(supplies) => Some(supplies),
            Err(e) => {
                eprintln!(
                    "error: invalid power-domain file {}: {}",
                    power_mesh_domains_path.unwrap_or_default(),
                    e
                );
                return ExitCode::FAILURE;
            }
        },
        Some(Err(e)) => {
            eprintln!(
                "error: cannot read {}: {}",
                power_mesh_domains_path.unwrap_or_default(),
                e
            );
            return ExitCode::FAILURE;
        }
        None => None,
    };
    if def_input.is_some() && !matches!(cmd.as_str(), "pnr" | "drc" | "flow") {
        eprintln!("error: --import-def requires pnr, drc, or flow");
        return ExitCode::FAILURE;
    }
    if statistical_model.is_some() && cmd != "ams" {
        eprintln!("error: --stats requires the ams command");
        return ExitCode::FAILURE;
    }
    if location_def.is_some() && cmd != "ams" {
        eprintln!("error: --def-locations requires the ams command");
        return ExitCode::FAILURE;
    }
    if resistor_layer_models.is_some() && cmd != "ams" {
        eprintln!("error: --rmodels requires the ams command");
        return ExitCode::FAILURE;
    }
    if exported_sdf_path.is_some() && cmd != "flow" {
        eprintln!("error: --write-sdf requires the flow command");
        return ExitCode::FAILURE;
    }
    if exported_sdf_path.is_some() && !physical_timing {
        eprintln!("error: --write-sdf requires --physical-timing");
        return ExitCode::FAILURE;
    }
    if exported_verilog_path.is_some() && cmd != "flow" {
        eprintln!("error: --write-verilog requires the flow command");
        return ExitCode::FAILURE;
    }
    if exported_verilog_path.is_some() && !physical_timing {
        eprintln!("error: --write-verilog requires --physical-timing");
        return ExitCode::FAILURE;
    }
    if post_route_power && cmd != "flow" {
        eprintln!("error: --post-route-power requires the flow command");
        return ExitCode::FAILURE;
    }
    if post_route_power && !physical_timing {
        eprintln!("error: --post-route-power requires --physical-timing");
        return ExitCode::FAILURE;
    }
    if all_power_modes && cmd != "flow" {
        eprintln!("error: --all-power-modes requires the flow command");
        return ExitCode::FAILURE;
    }
    if all_power_modes && !post_route_power {
        eprintln!("error: --all-power-modes requires --post-route-power");
        return ExitCode::FAILURE;
    }
    if all_power_modes && power_intent.is_none() {
        eprintln!("error: --all-power-modes requires --upf");
        return ExitCode::FAILURE;
    }
    if let Some(path) = &exported_sdf_path {
        if mmsc_path.is_some() && !path.contains("{corner}") {
            eprintln!(
                "error: --write-sdf with --mmsc requires a '{{corner}}' template such as best.{{corner}}.sdf"
            );
            return ExitCode::FAILURE;
        }
    }
    if (timing_driven_cts || cts_target_skew_set) && !matches!(cmd.as_str(), "cts" | "flow") {
        eprintln!("error: timing-driven CTS options require the cts or flow command");
        return ExitCode::FAILURE;
    }
    if power_mesh && multilayer_power_mesh {
        eprintln!("error: choose either --power-mesh or --multilayer-power-mesh, not both");
        return ExitCode::FAILURE;
    }
    if pdk_path.is_some() && cmd != "pdk" {
        eprintln!("error: --pdk requires the pdk command");
        return ExitCode::FAILURE;
    }
    if cmd == "pdk" && pdk.is_none() {
        eprintln!("error: pdk signoff requires --pdk <manifest>");
        return ExitCode::FAILURE;
    }
    if power_domain_supplies.is_some() && !multilayer_power_mesh {
        eprintln!("error: --power-mesh-domains requires --multilayer-power-mesh");
        return ExitCode::FAILURE;
    }
    if power_domain_supplies.is_some() && power_intent.is_none() {
        eprintln!("error: --power-mesh-domains requires --upf");
        return ExitCode::FAILURE;
    }
    let mesh_analysis = power_mesh || multilayer_power_mesh;
    if multilayer_power_mesh && !matches!(cmd.as_str(), "ir" | "flow") {
        eprintln!("error: --multilayer-power-mesh requires the ir or flow command");
        return ExitCode::FAILURE;
    }
    if power_mesh && !matches!(cmd.as_str(), "ir" | "flow") {
        eprintln!("error: --power-mesh requires the ir or flow command");
        return ExitCode::FAILURE;
    }
    if mesh_analysis && cmd == "flow" && !physical_timing {
        if power_mesh {
            eprintln!("error: --power-mesh in flow requires --physical-timing");
        } else {
            eprintln!("error: --multilayer-power-mesh in flow requires --physical-timing");
        }
        return ExitCode::FAILURE;
    }
    if mesh_analysis && cmd == "flow" && !post_route_power {
        if power_mesh {
            eprintln!("error: --power-mesh in flow requires --post-route-power");
        } else {
            eprintln!("error: --multilayer-power-mesh in flow requires --post-route-power");
        }
        return ExitCode::FAILURE;
    }
    if cts_target_skew_set && !timing_driven_cts {
        eprintln!("error: --cts-target-skew requires --timing-driven-cts");
        return ExitCode::FAILURE;
    }
    if cmd == "flow" && timing_driven_cts && !physical_timing {
        eprintln!("error: --timing-driven-cts in flow requires --physical-timing");
        return ExitCode::FAILURE;
    }
    if cmd == "flow" && repair_hold && !physical_timing {
        eprintln!("error: --repair-hold in flow requires --physical-timing");
        return ExitCode::FAILURE;
    }
    let run = || -> Result<String, String> {
        match cmd.as_str() {
            "pdk" => {
                let pdk = pdk.as_ref().ok_or("pdk signoff requires --pdk")?;
                let result = eda_api::run_pdk_signoff(&src, top.as_deref(), pdk)?;
                Ok(eda_api::report_pdk_signoff(&result))
            }
            "info" => {
                let d = eda_api::compile(&src, top.as_deref())?;
                let mut s = format!("module {}\n", d.name);
                s += &format!("  nets: {} (flat bits)\n", d.n_nets);
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
                let spice_source = if let Some(models) = &resistor_layer_models {
                    format!("{}\n{}", eda_api::write_resistor_layer_models(models)?, src)
                } else {
                    src.clone()
                };
                let circuit = eda_api::parse_spice(&spice_source)?;
                let mut circuit = circuit;
                if let Some(model) = &statistical_model {
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
                if let Some(def) = &location_def {
                    eda_api::apply_physical_def_locations(&mut circuit, def, 100.0)?;
                }
                let results = eda_ams::run_all(&circuit)?;
                Ok(eda_ams::report_all(&results))
            }
            "synth" => {
                let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
                Ok(f.verilog)
            }
            "equiv" => {
                let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
                Ok(if f.equivalent {
                    "EQUIVALENT\n".into()
                } else {
                    "NOT EQUIVALENT\n".into()
                })
            }
            "sta" => {
                let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
                if spef.is_some() && sdf.is_some() {
                    return Err("choose either --spef or --sdf, not both".into());
                }
                let netlist = if let Some(spef) = spef.as_deref() {
                    let parasitics = eda_api::parse_parasitics(spef)?;
                    let generated = eda_api::parasitics_to_sdf(&f.netlist, &parasitics)?;
                    let annotations = eda_api::parse_delay_file(&generated)?;
                    eda_api::back_annotate(&f.netlist, &annotations)?
                } else if let Some(sdf) = sdf.as_deref() {
                    let annotations = eda_api::parse_delay_file(sdf)?;
                    eda_api::back_annotate(&f.netlist, &annotations)?
                } else {
                    f.netlist.clone()
                };
                if sdc.is_some() && mmsc.is_some() {
                    return Err("choose either --sdc or --mmsc, not both".into());
                }
                if waiver_rules.is_some() && waiver_audit.is_some() {
                    return Err("choose either --waivers or --waiver-audit, not both".into());
                }
                if (waiver_rules.is_some() || waiver_audit.is_some())
                    && sdc.is_none()
                    && mmsc.is_none()
                {
                    return Err("--waivers and --waiver-audit require --sdc or --mmsc".into());
                }
                if let Some(source) = mmsc.as_deref() {
                    multicorner_sta_report(
                        &netlist,
                        source,
                        timing_path_count,
                        hold_path_count,
                        reset_path_count,
                        waiver_rules.as_deref(),
                        waiver_audit.as_deref(),
                    )
                } else if timing_path_count.is_none()
                    && hold_path_count.is_none()
                    && reset_path_count.is_none()
                {
                    match sdc.as_deref() {
                        Some(sdc) => {
                            let constraints = eda_api::parse_timing_constraints(sdc)?;
                            let timing = eda_sta::analyze_with_constraints(&netlist, &constraints)?;
                            let constrained =
                                eda_sta::analyze_constrained(&netlist, timing, &constraints)?;
                            let mut report = eda_sta::constrained_report(&constrained);
                            if waiver_rules.is_some() || waiver_audit.is_some() {
                                report.push_str(&timing_waiver_report(
                                    &netlist,
                                    &constrained,
                                    waiver_rules.as_deref(),
                                    waiver_audit.as_deref(),
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
                    let constraints = sdc
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
                        if waiver_rules.is_some() || waiver_audit.is_some() {
                            report.push_str(&timing_waiver_report(
                                &netlist,
                                constrained,
                                waiver_rules.as_deref(),
                                waiver_audit.as_deref(),
                            )?);
                        }
                    }
                    if let Some(path_count) = timing_path_count {
                        report.push_str(&sta_paths_report(
                            &netlist,
                            &timing,
                            constrained.as_ref(),
                            path_count,
                        )?);
                    }
                    if let Some(path_count) = hold_path_count {
                        report.push_str(&sta_hold_paths_report(
                            &netlist,
                            &timing,
                            constrained.as_ref(),
                            path_count,
                        )?);
                    }
                    if let Some(path_count) = reset_path_count {
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
                let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
                let (placement, default_routing) = match def_input.as_deref() {
                    Some(source) => eda_api::import_physical_def_text(&f.netlist, source)?,
                    None => (f.placement.clone(), f.routing.clone()),
                };
                let route_options = if def_input.is_some() {
                    eda_api::RouteOptions {
                        tracks_per_channel: default_routing.tracks_per_channel,
                        allow_congestion: false,
                        blocked_channels: default_routing.blocked_channels.clone(),
                    }
                } else {
                    eda_api::RouteOptions::default()
                };
                if ripgup_passes.is_some() && timing_ripgup_passes.is_some() {
                    return Err("choose either --ripgup or --timing-ripgup".into());
                }
                let waiver_store = if waiver_rules.is_some() || waiver_audit.is_some() {
                    let sdc = sdc
                        .as_deref()
                        .ok_or_else(|| "pnr waivers require --sdc".to_string())?;
                    let constraints = eda_api::parse_timing_constraints(sdc)?;
                    let timing = eda_sta::analyze_with_constraints(&f.netlist, &constraints)?;
                    let constrained =
                        eda_sta::analyze_constrained(&f.netlist, timing, &constraints)?;
                    let waivers =
                        optimization_waivers(waiver_rules.as_deref(), waiver_audit.as_deref())?;
                    let mut store = eda_api::timing_violation_store(
                        &constrained.clock,
                        &f.netlist,
                        &constrained,
                    )?;
                    if let Some(waivers) = waivers {
                        for waiver in waivers {
                            eda_api::add_timing_violation_waiver(&mut store, waiver)?;
                        }
                    }
                    Some((store, constrained))
                } else {
                    None
                };
                let routing = if let Some(passes) = timing_ripgup_passes {
                    if maze_routing {
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
                                passes,
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
                                passes,
                            )?
                            .ripup
                            .routing
                        }
                    }
                } else if let Some(passes) = ripgup_passes {
                    if maze_routing {
                        return Err("choose either --maze or --ripgup, not both".into());
                    }
                    eda_api::route_maze_ripgup(&f.netlist, &placement, &route_options, passes)?
                        .routing
                } else if maze_routing {
                    eda_api::route_maze_with_options(&f.netlist, &placement, &route_options)?
                } else {
                    default_routing
                };
                if let Some(path) = &extracted_spef_path {
                    write_extracted_spef(path, &f.netlist, &routing)?;
                }
                if let Some(path) = &lef_out {
                    std::fs::write(path, &f.lef).map_err(|e| e.to_string())?;
                }
                if let Some(path) = &def_out {
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
                let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
                let report = if let Some(vcd) = vcd.as_ref() {
                    eda_api::power_from_waveform(
                        &f.netlist,
                        vcd,
                        &eda_api::PowerOptions::default(),
                    )?
                } else {
                    eda_api::quick_power(&f.netlist, &eda_api::PowerOptions::default())?
                };
                Ok(eda_api::power_report(&report))
            }
            "drc" => {
                let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
                let (placement, routing) = match def_input.as_deref() {
                    Some(source) => eda_api::import_physical_def_text(&f.netlist, source)?,
                    None => (f.placement.clone(), f.routing.clone()),
                };
                let result = eda_api::physical_verification(&f.netlist, &placement, &routing)?;
                Ok(eda_pnr::drc_report(&result))
            }
            "assert" => {
                let waveform = vcd
                    .as_ref()
                    .ok_or_else(|| "assert requires --vcd <file.vcd>".to_string())?;
                let properties = sva
                    .as_deref()
                    .ok_or_else(|| "assert requires --sva <file.sva>".to_string())?;
                let assertions = eda_api::parse_assertions(properties)?;
                let results = eda_api::check_assertions(waveform, &assertions)?;
                Ok(eda_api::assertion_report(&results))
            }
            "coverage" => {
                let waveform = vcd
                    .as_ref()
                    .ok_or_else(|| "coverage requires --vcd <file.vcd>".to_string())?;
                let source = cover
                    .as_deref()
                    .ok_or_else(|| "coverage requires --cover <file.cover>".to_string())?;
                let plan = eda_api::parse_covergroups(source)?;
                let result = eda_api::functional_coverage(waveform, &plan)?;
                Ok(eda_api::coverage_report(&result))
            }
            "cdc" => {
                let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
                let result = eda_api::clock_domain_crossing(&f.netlist)?;
                Ok(eda_cdc::report(&result))
            }
            "spef" => {
                let parasitics_source = spef
                    .as_deref()
                    .ok_or_else(|| "spef requires --spef <file.spef>".to_string())?;
                let parasitics = eda_api::parse_parasitics(parasitics_source)?;
                let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
                let result = eda_api::parasitic_analysis(&f.netlist, &parasitics)?;
                Ok(eda_api::parasitic_report(&result))
            }
            "dft" => {
                let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
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
                let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
                let result =
                    eda_api::fault_coverage(&f.netlist, &eda_api::PatternOptions::default())?;
                if let Some(path) = &stil_out {
                    let stil =
                        eda_api::patterns_to_stil(&f.netlist, &result.patterns, &f.design.name)?;
                    std::fs::write(path, stil).map_err(|e| e.to_string())?;
                }
                Ok(eda_fault::report(&result))
            }
            "eco" => {
                if repair_hold {
                    if eco.is_some() {
                        return Err("choose either --eco or --repair-hold, not both".into());
                    }
                    if let Some(source) = mmsc.as_deref() {
                        if sdc.is_some() {
                            return Err("choose either --sdc or --mmsc, not both".into());
                        }
                        let definitions = eda_api::parse_timing_scenarios(source)?;
                        let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
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
                                    max_buffers_per_endpoint: hold_buffer_limit,
                                    ..Default::default()
                                },
                                ..Default::default()
                            },
                        )?;
                        return Ok(eda_api::multi_corner_hold_repair_report(&result));
                    }
                    let source = sdc
                        .as_deref()
                        .ok_or_else(|| "--repair-hold requires --sdc <file.sdc>".to_string())?;
                    let constraints = eda_api::parse_timing_constraints(source)?;
                    let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
                    let result = eda_api::repair_hold_violations_with_placement(
                        &f.netlist,
                        &f.placement,
                        &constraints,
                        &eda_api::HoldRepairOptions {
                            max_buffers_per_endpoint: hold_buffer_limit,
                            ..Default::default()
                        },
                    )?;
                    return Ok(eda_api::hold_repair_report(&result));
                }
                if hold_buffer_limit_set {
                    return Err("--hold-max-buffers requires --repair-hold".into());
                }
                let source = eco
                    .as_deref()
                    .ok_or_else(|| "eco requires --eco <file.eco>".to_string())?;
                let patch = eda_api::parse_eco_patch(source)?;
                let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
                let result = eda_api::apply_eco_patch(&f.netlist, &patch)?;
                Ok(eda_eco::report(&result))
            }
            "upf" => {
                let source = upf
                    .as_deref()
                    .ok_or_else(|| "upf requires --upf <file.upf>".to_string())?;
                let intent = eda_api::parse_power_intent(source)?;
                let design = eda_api::compile(&src, top.as_deref())?;
                let result = eda_api::power_intent_check(&design, &intent);
                Ok(eda_upf::report(&result))
            }
            "cts" => {
                let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
                if timing_driven_cts {
                    let result = eda_api::timing_driven_clock_tree_synthesis_with_placement(
                        &f.netlist,
                        &f.placement,
                        &eda_api::TimingDrivenCtsOptions {
                            target_skew_ns: cts_target_skew,
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
                let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
                let power = if let Some(vcd) = vcd.as_ref() {
                    eda_api::power_from_waveform(
                        &f.netlist,
                        vcd,
                        &eda_api::PowerOptions::default(),
                    )?
                } else {
                    eda_api::quick_power(&f.netlist, &eda_api::PowerOptions::default())?
                };
                if multilayer_power_mesh {
                    if let Some(supplies) = power_domain_supplies.as_ref() {
                        let intent = power_intent
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
                if power_mesh {
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
                let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
                if let Some(source) = sva.as_deref() {
                    if proof_expr.is_some()
                        || bad_net.is_some()
                        || !invariant_nets.is_empty()
                        || !expression_invariants.is_empty()
                    {
                        return Err("--sva cannot be combined with other prove targets".into());
                    }
                    return temporal_bmc_report(
                        &f.netlist,
                        source,
                        proof_depth,
                        cex_path.as_deref(),
                    );
                }
                let invariants: Vec<(&str, bool)> = invariant_nets
                    .iter()
                    .map(|(net, value)| (net.as_str(), *value))
                    .collect();
                if let Some(expr) = &proof_expr {
                    let mut invariants = expression_invariants.clone();
                    invariants.extend(
                        invariant_nets
                            .iter()
                            .map(|(net, value)| eda_api::ProofInvariant::new(net.clone(), *value)),
                    );
                    let proof = if invariants.is_empty() {
                        eda_api::prove_safety_expression(&f.netlist, expr, proof_depth)?
                    } else {
                        eda_api::prove_safety_expression_with_proof_invariants(
                            &f.netlist,
                            expr,
                            proof_depth,
                            &invariants,
                        )?
                    };
                    if let Some(path) = &cex_path {
                        if proof.status != eda_api::ProofStatus::Refuted {
                            return Err("--cex requires a refuted proof".into());
                        }
                        write_proof_cex(path, &f.netlist, &proof)?;
                    }
                    Ok(eda_api::proof_report(&proof))
                } else {
                    if !expression_invariants.is_empty() {
                        return Err("--assume-expr requires --expr".into());
                    }
                    let bad = bad_net.as_deref().unwrap_or("bad");
                    let proof = if invariants.is_empty() {
                        eda_api::prove_safety(&f.netlist, bad, proof_depth)?
                    } else {
                        eda_api::prove_safety_with_value_invariants(
                            &f.netlist,
                            bad,
                            proof_depth,
                            &invariants,
                        )?
                    };
                    if let Some(path) = &cex_path {
                        if proof.status != eda_api::ProofStatus::Refuted {
                            return Err("--cex requires a refuted proof".into());
                        }
                        write_proof_cex(path, &f.netlist, &proof)?;
                    }
                    Ok(eda_api::proof_report(&proof))
                }
            }
            "flow" => {
                let f = eda_api::run_flow_with_library(&src, top.as_deref(), active_lib)?;
                let imported_physical = match def_input.as_deref() {
                    Some(source) => Some(eda_api::import_physical_def_text(&f.netlist, source)?),
                    None => None,
                };
                if imported_physical.is_some() && !physical_timing {
                    return Err("--import-def requires --physical-timing".into());
                }
                if !physical_timing && repair_antennas {
                    return Err("--repair-antennas requires --physical-timing".into());
                }
                if !repair_antennas && antenna_diode_limit_set {
                    return Err("--antenna-max-diodes requires --repair-antennas".into());
                }
                if physical_timing {
                    if sdc.is_some() && mmsc.is_some() {
                        return Err("choose either --sdc or --mmsc, not both".into());
                    }
                    let waivers =
                        optimization_waivers(waiver_rules.as_deref(), waiver_audit.as_deref())?;
                    if let Some(source) = mmsc.as_deref() {
                        let definitions = eda_api::parse_timing_scenarios(source)?;
                        let mut power_scenarios = if post_route_power {
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
                                timing_driven_cts: timing_driven_cts.then(|| {
                                    eda_api::TimingDrivenCtsOptions {
                                        target_skew_ns: cts_target_skew,
                                        ..Default::default()
                                    }
                                }),
                                hold_repair: repair_hold.then(|| {
                                    eda_api::MultiCornerHoldRepairOptions {
                                        hold: eda_api::HoldRepairOptions {
                                            max_buffers_per_endpoint: hold_buffer_limit,
                                            ..Default::default()
                                        },
                                        ..Default::default()
                                    }
                                }),
                                routing_passes: timing_ripgup_passes.unwrap_or(8),
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
                                repair_antennas,
                                antenna_max_diodes_per_net: antenna_diode_limit,
                                ..Default::default()
                            },
                        )?;
                        let best = result.best();
                        let Some(reference) = best.corners.first() else {
                            return Err("MMMC physical timing flow has no scenario".into());
                        };
                        if power_mesh {
                            let mesh_options = eda_api::PowerMeshOptions {
                                rows: best.placement.rows.max(1),
                                ..Default::default()
                            };
                            for scenario in &mut power_scenarios {
                                scenario.options.power_mesh = Some(mesh_options.clone());
                            }
                        }
                        if multilayer_power_mesh {
                            let mut mesh_upf_options = None;
                            let mesh_options = match power_domain_supplies.as_ref() {
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
                            lef_out.as_ref(),
                            def_out.as_ref(),
                            extracted_spef_path.as_ref(),
                            None,
                            exported_verilog_path.as_ref(),
                            &reference.evaluation.annotated_netlist,
                            &best.placement,
                            &best.routing,
                            &reference.evaluation.parasitics,
                            &reference.evaluation.sdf,
                        )?;
                        let mut report = eda_api::multi_corner_physical_timing_flow_report(&result);
                        if let Some(template) = &exported_sdf_path {
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
                        if let Some(path) = &exported_verilog_path {
                            report.push_str(&format!("Best physical Verilog export: {}\n", path));
                        }
                        if post_route_power {
                            if all_power_modes {
                                let intent = power_intent
                                    .as_ref()
                                    .ok_or("--all-power-modes requires --upf")?;
                                let power_modes = match vcd.as_ref() {
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
                                let power_ir = match (vcd.as_ref(), power_intent.as_ref()) {
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
                    let constraints = sdc
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
                            timing_driven_cts: timing_driven_cts.then(|| {
                                eda_api::TimingDrivenCtsOptions {
                                    target_skew_ns: cts_target_skew,
                                    ..Default::default()
                                }
                            }),
                            hold_repair: repair_hold.then(|| eda_api::HoldRepairOptions {
                                max_buffers_per_endpoint: hold_buffer_limit,
                                ..Default::default()
                            }),
                            routing_passes: timing_ripgup_passes.unwrap_or(8),
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
                            repair_antennas,
                            antenna_max_diodes_per_net: antenna_diode_limit,
                            ..Default::default()
                        },
                    )?;
                    let best = result.best();
                    write_best_physical_timing_outputs(
                        lef_out.as_ref(),
                        def_out.as_ref(),
                        extracted_spef_path.as_ref(),
                        exported_sdf_path.as_ref(),
                        exported_verilog_path.as_ref(),
                        &best.annotated_netlist,
                        &best.placement,
                        &best.routing,
                        &best.parasitics,
                        &best.sdf,
                    )?;
                    let mut report = eda_api::physical_timing_flow_report(&result);
                    let mut post_route_options = eda_api::PostRoutePowerIrOptions::default();
                    if multilayer_power_mesh {
                        if let Some(supplies) = power_domain_supplies.as_ref() {
                            post_route_options.upf = eda_api::multilayer_power_domain_upf_options(
                                supplies,
                                &eda_api::IrOptions::default(),
                            )?;
                            post_route_options.multilayer_power_mesh =
                                Some(coupled_multilayer_mesh_options(
                                    best.placement.rows.max(1),
                                    supplies,
                                ));
                        } else {
                            post_route_options.multilayer_power_mesh =
                                Some(multilayer_mesh_options(best.placement.rows.max(1)));
                        }
                    }
                    if power_mesh {
                        post_route_options.power_mesh = Some(eda_api::PowerMeshOptions {
                            rows: best.placement.rows.max(1),
                            ..Default::default()
                        });
                    }
                    if post_route_power {
                        if all_power_modes {
                            let intent = power_intent
                                .as_ref()
                                .ok_or("--all-power-modes requires --upf")?;
                            let power_modes = match vcd.as_ref() {
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
                            let power_ir = match (vcd.as_ref(), power_intent.as_ref()) {
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
                    if let Some(path) = &exported_verilog_path {
                        report.push_str(&format!("Best physical Verilog export: {}\n", path));
                    }
                    return Ok(report);
                }
                if waiver_rules.is_some() || waiver_audit.is_some() {
                    return Err("flow waivers require --physical-timing".into());
                }
                if let Some(path) = &lef_out {
                    std::fs::write(path, &f.lef).map_err(|e| e.to_string())?;
                }
                if let Some(path) = &def_out {
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
    };
    match run() {
        Ok(out) => {
            if waiver_export_path.is_some() || waiver_audit_export_path.is_some() {
                if !matches!(cmd.as_str(), "sta" | "pnr" | "flow") {
                    eprintln!(
                        "error: --write-waivers and --write-waiver-audit require sta, pnr, or flow"
                    );
                    return ExitCode::FAILURE;
                }
                if let Err(error) = write_waiver_exports(
                    waiver_rules.as_deref(),
                    waiver_audit.as_deref(),
                    waiver_export_path.as_deref(),
                    waiver_audit_export_path.as_deref(),
                ) {
                    eprintln!("error: {}", error);
                    return ExitCode::FAILURE;
                }
            }
            print!("{}", out);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {}", e);
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constrained_sta_report_applies_input_and_hold_delays() {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let report = constrained_sta_report(
            &flow.netlist,
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.8 -clock clk [get_ports d]
            set_clock_uncertainty 0.15 -hold [get_clocks clk]
        "#,
        )
        .unwrap();
        assert!(report.contains("hold in-to-reg   : 0.450 ns"), "{report}");
        assert!(report.contains("worst slack : 0.450 ns"), "{report}");
    }

    #[test]
    fn sta_paths_report_supports_constrained_timing() {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let unconstrained = sta_paths_report(&flow.netlist, &flow.timing, None, 2).unwrap();
        assert!(
            unconstrained.contains("Timing path report: 2 path(s)"),
            "{unconstrained}"
        );

        let constraints = eda_api::parse_timing_constraints(
            r#"
                create_clock -name clk -period 4.0 [get_ports clk]
                set_input_delay 1.0 -clock clk [get_ports d]
                set_clock_latency -source 0.1 [get_clocks clk]
                set_clock_latency 0.2 [get_clocks clk]
            "#,
        )
        .unwrap();
        let constrained = eda_api::constrained_timing(&flow.netlist, &constraints).unwrap();
        let constrained_report =
            sta_paths_report(&flow.netlist, &flow.timing, Some(&constrained), 2).unwrap();
        assert!(
            constrained_report.contains("Constrained timing path report: 2 path(s)")
                && constrained_report.contains("slack")
                && constrained_report.contains("launch: clk rise")
                && constrained_report.contains("capture: clk rise")
                && constrained_report.contains("insertion skew"),
            "{constrained_report}"
        );
    }

    #[test]
    fn sta_hold_paths_report_supports_constrained_timing() {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let unconstrained = sta_hold_paths_report(&flow.netlist, &flow.timing, None, 2).unwrap();
        assert!(
            unconstrained.contains("Hold timing path report: 2 path(s)"),
            "{unconstrained}"
        );

        let constraints = eda_api::parse_timing_constraints(
            r#"
                create_clock -name clk -period 4.0 [get_ports clk]
                set_clock_uncertainty 0.05 -hold [get_clocks clk]
            "#,
        )
        .unwrap();
        let constrained = eda_api::constrained_timing(&flow.netlist, &constraints).unwrap();
        let constrained_report =
            sta_hold_paths_report(&flow.netlist, &flow.timing, Some(&constrained), 2).unwrap();
        assert!(
            constrained_report.contains("Constrained hold timing path report: 2 path(s)")
                && constrained_report.contains("hold slack"),
            "{constrained_report}"
        );
    }

    #[test]
    fn sta_reset_paths_report_supports_constrained_timing() {
        let src = "module m(input clk, d, rst, output reg q);
            always @(posedge clk or posedge rst)
                if (rst) q <= 1'b0; else q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let unconstrained = sta_reset_paths_report(&flow.netlist, &flow.timing, None, 1).unwrap();
        assert!(
            unconstrained.contains("Reset timing path report: 1 endpoint(s)")
                && unconstrained.contains("recovery arrival")
                && unconstrained.contains("removal arrival"),
            "{unconstrained}"
        );

        let constraints = eda_api::parse_timing_constraints(
            r#"
                create_clock -name clk -period 4.0 [get_ports clk]
                set_input_delay 1.2 -clock clk [get_ports rst]
            "#,
        )
        .unwrap();
        let constrained = eda_api::constrained_timing(&flow.netlist, &constraints).unwrap();
        let constrained_report =
            sta_reset_paths_report(&flow.netlist, &flow.timing, Some(&constrained), 2).unwrap();
        assert!(
            constrained_report.contains("Constrained reset timing path report: 2 path(s)")
                && constrained_report.contains("recovery slack")
                && constrained_report.contains("removal slack"),
            "{constrained_report}"
        );
    }

    #[test]
    fn constrained_sta_report_applies_timing_derate() {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let report = constrained_sta_report(
            &flow.netlist,
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_timing_derate -early 0.8 -late 1.2
        "#,
        )
        .unwrap();
        assert!(
            report.contains("early derate 0.800")
                && report.contains("late derate 1.200")
                && report.contains("output        : 2.160 ns"),
            "{report}"
        );
    }

    #[test]
    fn constrained_sta_report_applies_cell_timing_derate() {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let cell = flow.netlist.cells().first().unwrap().name.clone();
        let report = constrained_sta_report(
            &flow.netlist,
            &format!(
                r#"
                create_clock -name clk -period 4.0 [get_ports clk]
                set_input_delay 0.2 -clock clk [get_ports a]
                set_output_delay 0.4 -clock clk [get_ports y]
                set_timing_derate -early 0.9 -late 1.1
                set_timing_derate -cell_delay -late 2.0 [get_cells {}]
            "#,
                cell
            ),
        )
        .unwrap();
        assert!(
            report.contains("cell derates   : 1") && report.contains("output        : 1.380 ns"),
            "{report}"
        );
    }

    #[test]
    fn constrained_sta_report_applies_port_timing_derate() {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let report = constrained_sta_report(
            &flow.netlist,
            r#"
                create_clock -name clk -period 4.0 [get_ports clk]
                set_input_delay 0.2 -clock clk [get_ports a]
                set_output_delay 0.4 -clock clk [get_ports y]
                set_timing_derate -early 0.5 -late 2.0 [get_ports a]
                set_timing_derate -late 2.0 [get_ports y]
            "#,
        )
        .unwrap();
        assert!(
            report.contains("port derates   : 2") && report.contains("output        : 1.800 ns"),
            "{report}"
        );
    }

    #[test]
    fn constrained_sta_report_applies_pin_timing_derate() {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let cell = flow.netlist.cells().first().unwrap().name.clone();
        let report = constrained_sta_report(
            &flow.netlist,
            &format!(
                r#"
                    create_clock -name clk -period 4.0 [get_ports clk]
                    set_input_delay 0.2 -clock clk [get_ports a]
                    set_output_delay 0.4 -clock clk [get_ports y]
                    set_timing_derate -late 2.0 [get_pins {}/a]
                "#,
                cell
            ),
        )
        .unwrap();
        assert!(
            report.contains("pin derates   : 1") && report.contains("output        : 1.400 ns"),
            "{report}"
        );
    }

    #[test]
    fn constrained_sta_report_applies_clk_to_q_pin_derate() {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let launch = flow.netlist.cells().first().unwrap().name.clone();
        let report = constrained_sta_report(
            &flow.netlist,
            &format!(
                r#"
                    create_clock -name clk -period 4.0 [get_ports clk]
                    set_timing_derate -cell_delay -early 0.5 -late 2.0 [get_pins {}/q]
                "#,
                launch
            ),
        )
        .unwrap();
        assert!(
            report.contains("pin derates   : 1")
                && report.contains("setup reg-to-reg : 1.000 ns")
                && report.contains("hold reg-to-reg  : 0.400 ns"),
            "{report}"
        );
    }

    #[test]
    fn constrained_sta_report_applies_cell_check_derate() {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let report = constrained_sta_report(
            &flow.netlist,
            r#"
                create_clock -name clk -period 4.0 [get_ports clk]
                set_timing_derate -cell_check -early 0.5 -late 2.0
            "#,
        )
        .unwrap();
        assert!(
            report.contains("check early 0.500")
                && report.contains("check late 2.000")
                && report.contains("setup reg-to-reg : 1.600 ns")
                && report.contains("min pulse high : 1.500 ns"),
            "{report}"
        );
    }

    #[test]
    fn writes_extracted_routing_spef() {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let path = std::env::temp_dir().join(format!("eda-routing-{}.spef", std::process::id()));
        write_extracted_spef(&path.to_string_lossy(), &flow.netlist, &flow.routing).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let spef = eda_api::parse_parasitics(&text).unwrap();
        assert_eq!(spef.nets.len(), flow.netlist.net_count());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn constrained_sta_report_applies_pin_check_derate() {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let capture = flow.netlist.cells().last().unwrap().name.clone();
        let report = constrained_sta_report(
            &flow.netlist,
            &format!(
                r#"
                    create_clock -name clk -period 4.0 [get_ports clk]
                    set_timing_derate -cell_check -early 0.5 -late 2.0 [get_pins {}/d]
                "#,
                capture
            ),
        )
        .unwrap();
        assert!(
            report.contains("pin check derates : 1")
                && report.contains("setup reg-to-reg : 1.600 ns")
                && report.contains("hold reg-to-reg  : 1.100 ns"),
            "{report}"
        );
    }

    #[test]
    fn multicorner_sta_report_accepts_inline_manifest() {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let report = multicorner_sta_report(
            &flow.netlist,
            r#"
            scenario slow
                create_clock -name clk -period 4.0 [get_ports clk]
            endscenario
            scenario fast
                create_clock -name clk -period 2.0 [get_ports clk]
            endscenario
        "#,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(
            report.contains("Multi-corner STA report: 2 scenario(s)")
                && report.contains("worst scenario: fast"),
            "{report}"
        );
    }

    #[test]
    fn multicorner_sta_report_merges_timing_paths() {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let report = multicorner_sta_report(
            &flow.netlist,
            r#"
            scenario slow
                create_clock -name clk -period 4.0 [get_ports clk]
            endscenario
            scenario fast
                create_clock -name clk -period 2.0 [get_ports clk]
            endscenario
        "#,
            Some(2),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(
            report.contains("Multi-corner setup timing path report: 1 endpoint(s)")
                && report.contains("output y: worst scenario 'fast'")
                && report.contains("scenario slack: slow=3.000000, fast=1.000000")
                && report.contains("arcs:"),
            "{report}"
        );
    }

    #[test]
    fn temporal_bmc_report_accepts_sva_subset() {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let report = temporal_bmc_report(
            &flow.netlist,
            "property two_cycle;
                assert property (@(posedge clk) d |-> ##[2:2] q2);
             endproperty",
            3,
            None,
        )
        .unwrap();
        assert!(
            report.contains("BOUNDED PASS through 3 cycle(s)"),
            "{report}"
        );
        assert!(report.contains("6 frame(s)"), "{report}");
    }

    #[test]
    fn temporal_bmc_report_labels_cover_properties() {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let report = temporal_bmc_report(
            &flow.netlist,
            "cover property (@(posedge clk) d |-> ##[2:2] q2);",
            3,
            None,
        )
        .unwrap();
        assert!(report.contains("COVER HIT"), "{report}");
    }

    #[test]
    fn temporal_bmc_report_applies_sva_assumptions() {
        let src = "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let report = temporal_bmc_report(
            &flow.netlist,
            r#"
            assert property (@(posedge clk) !d);
            assume property (@(posedge clk) !d);
        "#,
            3,
            None,
        )
        .unwrap();
        assert!(
            report.contains("ASSUMPTION CONSTRAINT applied") && report.contains("BOUNDED PASS"),
            "{report}"
        );

        let error = temporal_bmc_report(
            &flow.netlist,
            "assume property (@(posedge clk) !d);",
            3,
            None,
        )
        .unwrap_err();
        assert!(error.contains("no assert or cover targets"), "{error}");
    }

    #[test]
    fn temporal_bmc_report_accepts_repeated_antecedents() {
        let src = "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let report = temporal_bmc_report(
            &flow.netlist,
            "assert property (@(posedge clk) d[*2] |-> q);",
            3,
            None,
        )
        .unwrap();
        assert!(report.contains("BOUNDED PASS"), "{report}");
    }

    #[test]
    fn temporal_bmc_report_accepts_unbounded_repetition() {
        let src = "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let report = temporal_bmc_report(
            &flow.netlist,
            "assert property (@(posedge clk) d[*] |-> q);",
            3,
            None,
        )
        .unwrap();
        assert!(
            report.contains("VIOLATED") && report.contains("repeat=0"),
            "{report}"
        );
    }

    #[test]
    fn temporal_bmc_report_accepts_nonconsecutive_repetition() {
        let src = "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let report = temporal_bmc_report(
            &flow.netlist,
            "assert property (@(posedge clk) d[=2] |-> q);",
            4,
            None,
        )
        .unwrap();
        assert!(
            report.contains("VIOLATED") && report.contains("repeat=2"),
            "{report}"
        );
    }

    #[test]
    fn temporal_bmc_report_accepts_sequence_antecedents() {
        let src = "module m(input clk, a, b, output reg state, output q);
            always @(posedge clk) state <= a;
            assign q = b & state;
         endmodule";
        let flow = eda_api::run_flow(src, None).unwrap();
        let report = temporal_bmc_report(
            &flow.netlist,
            "assert property (@(posedge clk) a ##[1:2] b |-> !q);",
            3,
            None,
        )
        .unwrap();
        assert!(report.contains("VIOLATED"), "{report}");
    }
}
