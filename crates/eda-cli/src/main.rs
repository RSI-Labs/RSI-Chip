//! # eda — command-line interface
//!
//! Usage: `eda <command> <file.v> [--top <module>]`
//! Commands: info | slang-info | synth | equiv | sta | pnr | power | drc | pdk | assert | coverage | cdc | spef | dft | fault | eco | upf | cts | ir | prove | ams | flow | daemon [--artifact-store <dir>]
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

mod cli;

use cli::dispatch::{dispatch, RunContext};

pub(crate) use cli::formal::*;
pub(crate) use cli::timing::*;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("capabilities") {
        print!("{}", eda_api::capability_report());
        return ExitCode::SUCCESS;
    }
    if args.get(1).map(String::as_str) == Some("daemon") {
        let mut listen = "127.0.0.1:7822".to_string();
        let mut artifact_store = None::<String>;
        let mut index = 2;
        while index < args.len() {
            if args[index] == "--listen" {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("error: --listen requires <ip>:<port>");
                    return ExitCode::from(2);
                };
                listen = value.clone();
                index += 2;
            } else if args[index] == "--artifact-store" {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("error: --artifact-store requires <directory>");
                    return ExitCode::from(2);
                };
                artifact_store = Some(value.clone());
                index += 2;
            } else {
                eprintln!("error: unknown daemon option '{}'", args[index]);
                return ExitCode::from(2);
            }
        }
        return match eda_daemon::Daemon::bind_with_artifact_store(
            &listen,
            artifact_store.as_deref().map(std::path::Path::new),
        ) {
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
            "usage: eda capabilities | eda daemon [--listen <ip>:<port>] [--artifact-store <dir>] | eda <info|slang-info|synth|equiv|sta|pnr|power|drc|pdk|assert|coverage|cdc|spef|dft|fault|eco|upf|cts|ir|prove|ams|flow> <file.v> [--top <module>]"
        );
        return ExitCode::from(2);
    }
    let cmd = args[1].clone();
    let file = args[2].clone();
    if cmd == "slang-info" {
        let top = args
            .iter()
            .position(|argument| argument == "--top")
            .and_then(|index| args.get(index + 1))
            .cloned();
        return match eda_slang::SlangFrontend::discover() {
            Ok(frontend) => match frontend.compile_files(
                &[std::path::PathBuf::from(&file)],
                top.as_deref(),
                &eda_slang::PreprocessOptions::default(),
            ) {
                Ok(design) => {
                    println!("module {}", design.name);
                    println!("  nets: {} (flat bits)", design.n_nets);
                    println!("  memories: {}", design.memories.len());
                    println!("  memory connections: {}", design.memory_connections.len());
                    println!("  ports: {}", design.ports.len());
                    println!(
                        "  assigns: {}, processes: {}, initials: {}",
                        design.assigns.len(),
                        design.processes.len(),
                        design.initials.len()
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    ExitCode::FAILURE
                }
            },
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        };
    }
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
    let ctx = RunContext {
        cmd: &cmd,
        src: &src,
        top: &top,
        extracted_spef_path: &extracted_spef_path,
        exported_sdf_path: &exported_sdf_path,
        exported_verilog_path: &exported_verilog_path,
        maze_routing,
        power_mesh,
        multilayer_power_mesh,
        ripgup_passes: &ripgup_passes,
        timing_ripgup_passes: &timing_ripgup_passes,
        physical_timing,
        post_route_power,
        all_power_modes,
        repair_antennas,
        antenna_diode_limit,
        antenna_diode_limit_set,
        timing_path_count: &timing_path_count,
        hold_path_count: &hold_path_count,
        reset_path_count: &reset_path_count,
        lef_out: &lef_out,
        def_out: &def_out,
        stil_out: &stil_out,
        repair_hold,
        hold_buffer_limit,
        hold_buffer_limit_set,
        timing_driven_cts,
        cts_target_skew,
        bad_net: &bad_net,
        cex_path: &cex_path,
        proof_expr: &proof_expr,
        proof_depth,
        invariant_nets: &invariant_nets,
        expression_invariants: &expression_invariants,
        pdk,
        sdc,
        mmsc,
        waiver_rules: &waiver_rules,
        waiver_audit: &waiver_audit,
        def_input,
        active_lib,
        vcd,
        sva,
        sdf,
        spef,
        cover,
        statistical_model,
        location_def,
        resistor_layer_models,
        eco,
        upf,
        power_intent,
        power_domain_supplies,
    };
    let run = || dispatch(&ctx);
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
    use cli::physical::write_extracted_spef;

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
