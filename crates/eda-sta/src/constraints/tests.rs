use super::*;
use eda_core::library::CellLibrary;
use eda_core::netlist::{Cell, CellKind, Dir, NetlistHierarchyInstance, NetlistHierarchyPort};
use std::sync::Arc;

#[test]
fn timing_object_patterns_match_flat_and_hierarchical_names() {
    let local = TimingObjectPattern::new("u*");
    assert!(local.is_pattern());
    assert!(local.matches("u0"));
    assert!(local.matches("u123"));
    assert!(!local.matches("top/u0"));

    let single_character = TimingObjectPattern::new("u?");
    assert!(single_character.matches("u0"));
    assert!(!single_character.matches("u01"));
    assert!(!single_character.matches("top/u0"));

    let recursive = TimingObjectPattern::new("**/u*");
    assert!(recursive.matches("u0"));
    assert!(recursive.matches("top/u0"));
    assert!(recursive.matches("a/b/u0"));

    let dot_hierarchy = TimingObjectPattern::new("top.*");
    assert!(dot_hierarchy.matches("top.sub"));
    assert!(!dot_hierarchy.matches("top/sub"));

    let escaped = TimingObjectPattern::new(r"u\*");
    assert!(escaped.matches("u*"));
    assert!(!escaped.matches("u0"));
    assert!(!escaped.is_pattern());
}

#[test]
fn parses_common_sdc_subset() {
    let s = parse_sdc(
        r#"
        create_clock -name core -period 4.0 [get_ports clk]
        set_clock_uncertainty 0.10 -setup [get_clocks core]
        set_input_delay 0.30 -clock core [get_ports a]
        set_output_delay 0.40 -clock core [get_ports y]
        set_false_path -from [get_ports b] -to [get_ports y]
        set_multicycle_path 2 -from [get_ports a] -to [get_ports y]
        set_disable_timing -from [get_pins u1/a] -to [get_pins u1/y]
        set_case_analysis 0 [get_ports b]
        set_max_delay 1.5 -from [get_ports a] -to [get_ports y]
        set_min_delay 0.5 -from [get_ports a] -to [get_ports y]
        set_timing_derate -early 0.9 -late 1.1
        set_timing_derate -cell_delay -early 0.7 -late 1.3 [get_cells u1]
    "#,
    )
    .unwrap();
    assert_eq!(s.clocks["core"].period, 4.0);
    assert_eq!(s.input_delays["a"].delay, 0.30);
    assert_eq!(s.output_delays["y"].delay, 0.40);
    assert_eq!(s.false_paths.len(), 1);
    assert_eq!(s.multicycle_paths.len(), 1);
    assert_eq!(s.false_paths[0].check, PathExceptionCheck::Both);
    assert_eq!(s.multicycle_paths[0].check, PathExceptionCheck::Both);
    assert_eq!(s.disabled_timing_arcs.len(), 1);
    assert_eq!(s.case_analyses.len(), 1);
    assert_eq!(s.max_delay_paths.len(), 1);
    assert_eq!(s.min_delay_paths.len(), 1);
    assert_eq!(s.timing_derate.early, 0.9);
    assert_eq!(s.timing_derate.late, 1.1);
    assert_eq!(s.cell_timing_derates["u1"].early, 0.7);
    assert_eq!(s.cell_timing_derates["u1"].late, 1.3);
}

#[test]
fn set_units_normalizes_subsequent_time_and_capacitance_values() {
    let source = r#"
        set_units -time 1ps -capacitance 1ff -resistance 1kohm -voltage 1mv -current 1ma
        create_clock -name clk -period 100 [get_ports clk]
        set_clock_uncertainty 10 -setup [get_clocks clk]
        set_clock_latency 20 [get_clocks clk]
        set_clock_transition 3 [get_clocks clk]
        set_input_delay 40 -clock clk [get_ports a]
        set_max_delay 300 -from [get_ports a] -to [get_ports y]
        set_max_transition 6
        set_max_capacitance 200
        set_load 70 [get_ports y]
    "#;
    let constraints = parse_sdc(source).unwrap();
    assert_eq!(constraints.units.time_unit, "ps");
    assert_eq!(constraints.units.capacitance_unit, "ff");
    assert_eq!(constraints.units.resistance_unit, "kohm");
    assert_eq!(constraints.units.voltage_unit, "mv");
    assert_eq!(constraints.units.current_unit, "ma");
    assert_eq!(constraints.clocks["clk"].period, 0.1);
    assert_eq!(constraints.clocks["clk"].fall, 0.05);
    assert_eq!(constraints.clock_uncertainty_ns["clk"], 0.01);
    assert_eq!(constraints.clocks["clk"].network_latency_ns, 0.02);
    assert_eq!(constraints.clocks["clk"].rise_transition_ns, 0.003);
    assert_eq!(constraints.input_delays["a"].delay, 0.04);
    assert_eq!(constraints.max_delay_paths[0].limit, 0.3);
    assert_eq!(constraints.max_transition_ns, Some(0.006));
    assert_eq!(constraints.max_capacitance_pf, Some(0.2));
    assert_eq!(constraints.load_capacitances[0].limit, 0.07);

    let defaults = parse_sdc("set_units -time ns -capacitance pf\n").unwrap();
    assert_eq!(defaults.units, SdcUnits::default());
    assert!(parse_sdc("set_units -time parsecs").is_err());
    assert!(parse_sdc("set_units -time 1ps -time 1ns").is_err());
    assert!(parse_sdc("set_units -unknown 1").is_err());
}

#[test]
fn rejects_unknown_clock_and_bad_period() {
    assert!(parse_sdc("set_input_delay 1 -clock missing [get_ports a]").is_err());
    assert!(parse_sdc("create_clock -name c -period 0 [get_ports clk]").is_err());
    assert!(parse_sdc("set_max_delay 0 -to [get_ports y]").is_err());
    assert!(parse_sdc("set_min_delay -1 -to [get_ports y]").is_err());
    assert!(parse_sdc("set_clock_latency -1 [get_clocks c]").is_err());
    assert!(parse_sdc("set_timing_derate -early 0").is_err());
    assert!(parse_sdc("set_timing_derate").is_err());
    assert!(parse_sdc("set_timing_derate -early 1 [get_nets a]").is_err());
    assert!(
        parse_sdc("set_timing_derate -cell_delay -cell_check -early 1 [get_cells u1]").is_err()
    );
    assert!(parse_sdc("set_timing_derate -early 1 [get_pins u1]").is_err());
    assert!(parse_sdc("set_timing_derate -cell_check -early 1 [get_pins u1/a]").is_ok());
}

#[test]
fn parses_and_derives_generated_clocks() {
    let s = parse_sdc(
        r#"
        create_clock -name core -period 4.0 [get_ports clk]
        create_generated_clock -name div2 -source [get_clocks core] -divide_by 2 -ports [get_ports div2]
        create_generated_clock -name fast -source [get_ports clk] -multiply_by 2 -ports [get_ports fast]
    "#,
    )
    .unwrap();
    assert_eq!(s.clocks["div2"].source_clock.as_deref(), Some("core"));
    assert_eq!(s.clocks["div2"].divide_by, Some(2.0));
    assert_eq!(s.clocks["div2"].period, 8.0);
    assert_eq!(s.clocks["div2"].ports, vec!["div2".to_string()]);
    assert_eq!(s.clocks["fast"].source_clock.as_deref(), Some("core"));
    assert_eq!(s.clocks["fast"].multiply_by, Some(2.0));
    assert_eq!(s.clocks["fast"].period, 2.0);
    assert_eq!(s.clocks["fast"].source_latency_ns, 0.0);

    let inherited = parse_sdc(
        r#"
        create_clock -name core -period 4.0 [get_ports clk]
        set_clock_latency -source 0.3 [get_clocks core]
        set_clock_latency 0.2 [get_clocks core]
        create_generated_clock -name div4 -source [get_clocks core] -divide_by 4 -ports [get_ports div4]
    "#,
    )
    .unwrap();
    assert_eq!(inherited.clocks["div4"].source_latency_ns, 0.5);
    assert_eq!(inherited.clocks["div4"].period, 16.0);

    assert!(parse_sdc(
        "create_generated_clock -name bad -source [get_clocks missing] -divide_by 2 -ports [get_ports bad]"
    )
    .is_err());
    assert!(parse_sdc(
        "create_clock -name core -period 4 [get_ports clk]\ncreate_generated_clock -name bad -source [get_clocks core] -divide_by 0 -ports [get_ports bad]"
    )
    .is_err());
}

#[test]
fn parses_and_queries_clock_groups() {
    let s = parse_sdc(
        r#"
        create_clock -name core -period 4.0 [get_ports core_clk]
        create_clock -name io -period 6.0 [get_ports io_clk]
        create_clock -name scan -period 8.0 [get_ports scan_clk]
        set_clock_groups -name domains -asynchronous -group [get_clocks core] -group [get_clocks io scan]
    "#,
    )
    .unwrap();
    assert_eq!(s.clock_groups.len(), 1);
    assert_eq!(s.clock_groups[0].kind, ClockGroupKind::Asynchronous);
    assert_eq!(
        s.clock_group_kind("core", "io"),
        Some(ClockGroupKind::Asynchronous)
    );
    assert_eq!(
        s.clock_group_kind("core", "scan"),
        Some(ClockGroupKind::Asynchronous)
    );
    assert_eq!(s.clock_group_kind("io", "scan"), None);
    assert_eq!(s.clock_group_kind("core", "missing"), None);

    let changed = parse_sdc(
        r#"
        create_clock -name core -period 4.0 [get_ports core_clk]
        create_clock -name io -period 6.0 [get_ports io_clk]
        set_clock_groups -name exclusive -logically_exclusive -group [get_clocks core] -group [get_clocks io]
    "#,
    )
    .unwrap();
    let diff = diff_timing_constraints(&s, &changed);
    assert!(diff.clock_groups_changed);
    assert!(diff.constrained_recompute_required);

    assert!(parse_sdc(
        "create_clock -name core -period 4 [get_ports core]\nset_clock_groups -asynchronous -group [get_clocks core]"
    )
    .is_err());
    assert!(parse_sdc(
        "create_clock -name core -period 4 [get_ports core]\nset_clock_groups -asynchronous -group [get_clocks core] -group [get_clocks core]"
    )
    .is_err());
}

#[test]
fn timing_derate_scales_late_and_early_arrivals() {
    let neutral = parse_sdc("create_clock -name clk -period 3 [get_ports clk]").unwrap();
    let derated = parse_sdc(
        r#"
        create_clock -name clk -period 3 [get_ports clk]
        set_input_delay 0.2 -clock clk [get_ports a]
        set_timing_derate -early 0.8 -late 1.2
    "#,
    )
    .unwrap();
    let nl = and_netlist();
    let neutral_base = crate::analyze_with_constraints(&nl, &neutral).unwrap();
    let derated_base = crate::analyze_with_constraints(&nl, &derated).unwrap();
    let y = nl
        .ports
        .iter()
        .find(|port| port.name == "y")
        .and_then(|port| port.bits.first().copied())
        .unwrap();
    assert_eq!(neutral_base.net_arrival(y), Some(1.0));
    assert_eq!(neutral_base.net_min_arrival(y), Some(1.0));
    assert_eq!(derated_base.net_arrival(y), Some(1.44));
    assert_eq!(derated_base.net_min_arrival(y), Some(0.8));

    let constrained = analyze_with_sdc(&nl, &derated).unwrap();
    assert_eq!(constrained.early_derate, 0.8);
    assert_eq!(constrained.late_derate, 1.2);
    assert!(constrained_report(&constrained).contains("early derate 0.800"));
}

#[test]
fn cell_timing_derate_overrides_global_for_cell_arcs() {
    let constrained = parse_sdc(
        r#"
        create_clock -name clk -period 4 [get_ports clk]
        set_input_delay 0.2 -clock clk [get_ports a]
        set_timing_derate -early 0.9 -late 1.1
        set_timing_derate -cell_delay -early 0.5 -late 2.0 [get_cells u1]
    "#,
    )
    .unwrap();
    let nl = and_netlist();
    let base = crate::analyze_with_constraints(&nl, &constrained).unwrap();
    let y = nl
        .ports
        .iter()
        .find(|port| port.name == "y")
        .and_then(|port| port.bits.first().copied())
        .unwrap();
    // Input a late path: 0.2 * global 1.1 + AND arc 1.0 * cell 2.0.
    assert_eq!(base.net_arrival(y), Some(2.22));
    // Input b has no external delay; early path is AND arc 1.0 * 0.5.
    assert_eq!(base.net_min_arrival(y), Some(0.5));
    let result = analyze_with_sdc(&nl, &constrained).unwrap();
    assert_eq!(result.cell_derates["u1"].late, 2.0);
    assert!(constrained_report(&result).contains("cell derates   : 1"));

    let mut missing = constrained.clone();
    missing
        .set_cell_timing_derate("missing", Some(1.0), Some(1.0))
        .unwrap();
    assert!(crate::analyze_with_constraints(&nl, &missing).is_err());
}

#[test]
fn port_timing_derate_scales_external_delays() {
    let constraints = parse_sdc(
        r#"
        create_clock -name clk -period 4 [get_ports clk]
        set_input_delay 0.2 -clock clk [get_ports a]
        set_input_delay 0.2 -clock clk [get_ports b]
        set_output_delay 0.4 -clock clk [get_ports y]
        set_timing_derate -early 0.9 -late 1.1
        set_timing_derate -early 0.5 -late 2.0 [get_ports a]
        set_timing_derate -late 2.0 [get_ports y]
    "#,
    )
    .unwrap();
    let nl = and_netlist();
    let base = crate::analyze_with_constraints(&nl, &constraints).unwrap();
    let y = nl
        .ports
        .iter()
        .find(|port| port.name == "y")
        .and_then(|port| port.bits.first().copied())
        .unwrap();
    // Late via a: 0.2 * 2.0 + AND arc 1.0 * global 1.1.
    assert_eq!(base.net_arrival(y), Some(1.5));
    // Early via a: 0.2 * 0.5 + AND arc 1.0 * global 0.9.
    assert_eq!(base.net_min_arrival(y), Some(1.0));

    let result = analyze_with_sdc(&nl, &constraints).unwrap();
    assert_eq!(result.port_derates.len(), 2);
    assert_eq!(result.port_derates["a"].early, 0.5);
    assert_eq!(result.port_derates["y"].late, 2.0);
    // Output arrival includes output delay 0.4 * y-late 2.0.
    assert!((result.output_slack - 1.7).abs() < 1e-12);
    assert!(constrained_report(&result).contains("port derates   : 2"));

    let mut missing = constraints.clone();
    missing
        .set_port_timing_derate("missing", Some(1.0), Some(1.0))
        .unwrap();
    assert!(crate::analyze_with_constraints(&nl, &missing).is_err());
}

#[test]
fn pin_timing_derate_overrides_cell_delay_derate() {
    let constraints = parse_sdc(
        r#"
        create_clock -name clk -period 4 [get_ports clk]
        set_input_delay 0.2 -clock clk [get_ports a]
        set_input_delay 0.2 -clock clk [get_ports b]
        set_timing_derate -early 0.9 -late 1.1
        set_timing_derate -cell_delay -early 0.7 -late 1.3 [get_cells u1]
        set_timing_derate -cell_delay -early 0.5 -late 2.0 [get_pins u1/a]
    "#,
    )
    .unwrap();
    let nl = and_netlist();
    let base = crate::analyze_with_constraints(&nl, &constraints).unwrap();
    let y = nl
        .ports
        .iter()
        .find(|port| port.name == "y")
        .and_then(|port| port.bits.first().copied())
        .unwrap();
    // Late via pin a: 0.2 * global 1.1 + arc 1.0 * pin 2.0.
    assert_eq!(base.net_arrival(y), Some(2.22));
    // Early via pin a: 0.2 * global 0.9 + arc 1.0 * pin 0.5.
    assert_eq!(base.net_min_arrival(y), Some(0.68));

    let result = analyze_with_sdc(&nl, &constraints).unwrap();
    assert_eq!(result.pin_derates.len(), 1);
    assert_eq!(result.pin_derates[&TimingPin::new("u1", "a")].late, 2.0);
    assert!(constrained_report(&result).contains("pin derates   : 1"));

    let mut missing = constraints.clone();
    missing
        .set_pin_timing_derate("u1", "missing", Some(1.0), Some(1.0))
        .unwrap();
    assert!(crate::analyze_with_constraints(&nl, &missing).is_err());
}

#[test]
fn output_pin_timing_derate_scales_cell_and_clk_to_q() {
    let combinational = parse_sdc(
        r#"
        create_clock -name clk -period 4 [get_ports clk]
        set_input_delay 0.2 -clock clk [get_ports a]
        set_timing_derate -cell_delay -early 0.7 -late 1.3 [get_cells u1]
        set_timing_derate -cell_delay -early 0.5 -late 2.0 [get_pins u1/y]
    "#,
    )
    .unwrap();
    let and = and_netlist();
    let and_result = crate::analyze_with_constraints(&and, &combinational).unwrap();
    let y = and
        .ports
        .iter()
        .find(|port| port.name == "y")
        .and_then(|port| port.bits.first().copied())
        .unwrap();
    assert_eq!(and_result.net_arrival(y), Some(2.2));
    // The b input has no external delay; the output-pin early derate
    // applies to its arc as well.
    assert_eq!(and_result.net_min_arrival(y), Some(0.5));

    let sequential = parse_sdc(
        r#"
        create_clock -name clk -period 4.0 [get_ports clk]
        set_timing_derate -cell_delay -early 0.5 -late 2.0 [get_pins launch/q]
    "#,
    )
    .unwrap();
    let flops = two_flop_netlist();
    let flop_result = analyze_with_sdc(&flops, &sequential).unwrap();
    assert!((flop_result.reg2reg_slack - 1.0).abs() < 1e-12);
    assert!((flop_result.reg2reg_hold_slack - 0.4).abs() < 1e-12);
    assert_eq!(flop_result.pin_derates.len(), 1);
}

#[test]
fn cell_check_derate_scales_setup_and_hold_requirements() {
    let nl = two_flop_netlist();
    let global = parse_sdc(
        r#"
        create_clock -name clk -period 4.0 [get_ports clk]
        set_timing_derate -cell_check -early 0.5 -late 2.0
    "#,
    )
    .unwrap();
    let global_result = analyze_with_sdc(&nl, &global).unwrap();
    // Capture endpoint: period 4 - clk-to-q 1.2 - setup 0.6 * late 2.
    assert!((global_result.reg2reg_slack - 1.6).abs() < 1e-12);
    // Input launch endpoint: 0 arrival - hold 0.2 * early 0.5.
    assert!((global_result.in2reg_hold_slack + 0.1).abs() < 1e-12);
    assert_eq!(global_result.early_check_derate, 0.5);
    assert_eq!(global_result.late_check_derate, 2.0);
    // Default 4 ns clock has 2 ns high and low widths; DFF requirement
    // is 0.25 ns * late 2.0.
    assert!((global_result.min_pulse_width_slack - 1.5).abs() < 1e-12);
    assert!(constrained_report(&global_result).contains("check derates  : 0"));

    let capture = parse_sdc(
        r#"
        create_clock -name clk -period 4.0 [get_ports clk]
        set_timing_derate -cell_check -early 0.5 -late 2.0 [get_cells capture]
    "#,
    )
    .unwrap();
    let capture_result = analyze_with_sdc(&nl, &capture).unwrap();
    assert!((capture_result.reg2reg_slack - 1.6).abs() < 1e-12);
    assert_eq!(capture_result.cell_check_derates.len(), 1);
    assert!(constrained_report(&capture_result).contains("check derates  : 1"));

    let mut missing = capture.clone();
    missing
        .set_cell_check_derate("missing", Some(1.0), Some(1.0))
        .unwrap();
    assert!(crate::analyze_with_constraints(&nl, &missing).is_err());
}

#[test]
fn pin_check_derate_overrides_cell_check_rules() {
    let nl = two_flop_netlist();
    let constraints = parse_sdc(
        r#"
        create_clock -name clk -period 4.0 [get_ports clk]
        set_timing_derate -cell_check -early 0.8 -late 1.2
        set_timing_derate -cell_check -early 0.7 -late 1.3 [get_cells capture]
        set_timing_derate -cell_check -early 0.5 -late 2.0 [get_pins capture/d]
    "#,
    )
    .unwrap();
    let result = analyze_with_sdc(&nl, &constraints).unwrap();
    // Capture data-pin setup: 4 - 1.2 clk-to-q - 0.6 setup * pin late 2.
    assert!((result.reg2reg_slack - 1.6).abs() < 1e-12);
    // Capture data-pin hold: 1.2 clk-to-q - 0.2 hold * pin early 0.5.
    assert!((result.reg2reg_hold_slack - 1.1).abs() < 1e-12);
    assert_eq!(result.pin_check_derates.len(), 1);
    assert!(constrained_report(&result).contains("pin check derates : 1"));

    let mut missing = constraints.clone();
    missing
        .set_pin_check_derate("capture", "missing", Some(1.0), Some(1.0))
        .unwrap();
    assert!(crate::analyze_with_constraints(&nl, &missing).is_err());
}

fn and_netlist() -> Netlist {
    let lib = Arc::new(CellLibrary::std());
    let mut nl = Netlist::new("sdc");
    nl.set_library(lib.clone());
    let a = nl.add_net("a");
    let b = nl.add_net("b");
    let y = nl.add_net("y");
    nl.add_cell(Cell {
        name: "u1".into(),
        kind: CellKind::Lib(lib.find("AND2").unwrap()),
        inputs: vec![a, b],
        output: y,
    });
    nl.add_port("a", Dir::In, vec![a]);
    nl.add_port("b", Dir::In, vec![b]);
    nl.add_port("y", Dir::Out, vec![y]);
    nl
}

fn two_flop_netlist() -> Netlist {
    let lib = Arc::new(CellLibrary::std());
    let mut nl = Netlist::new("hold");
    nl.set_library(lib.clone());
    let clk = nl.add_net("clk");
    let d = nl.add_net("d");
    let q1 = nl.add_net("q1");
    let q2 = nl.add_net("q2");
    let dff = lib.find("DFF").unwrap();
    nl.add_cell(Cell {
        name: "launch".into(),
        kind: CellKind::Lib(dff),
        inputs: vec![clk, d],
        output: q1,
    });
    nl.add_cell(Cell {
        name: "capture".into(),
        kind: CellKind::Lib(dff),
        inputs: vec![clk, q1],
        output: q2,
    });
    nl.add_port("clk", Dir::In, vec![clk]);
    nl.add_port("d", Dir::In, vec![d]);
    nl.add_port("q2", Dir::Out, vec![q2]);
    nl
}

#[test]
fn constrained_paths_expose_launch_capture_clock_metadata() {
    let nl = two_flop_netlist();
    let c = parse_sdc(
        r#"
        create_clock -name clk -period 4.0 [get_ports clk]
        set_clock_latency -source 0.1 [get_clocks clk]
        set_clock_latency 0.2 [get_clocks clk]
    "#,
    )
    .unwrap();
    let timing = analyze_with_sdc(&nl, &c).unwrap();
    let paths = constrained_timing_paths(&nl, &timing, 0);
    let sequential = paths
        .iter()
        .find(|path| {
            matches!(path.path.launch, crate::TimingLaunch::SequentialCell(_))
                && matches!(
                    path.path.endpoint,
                    crate::TimingPathEndpoint::SequentialCell(_)
                )
        })
        .unwrap();
    assert_eq!(sequential.clocks.launch.edge, TimingClockEdgeKind::Rise);
    assert_eq!(sequential.clocks.launch.network_latency_ns, 0.2);
    assert_eq!(sequential.clocks.capture.network_latency_ns, 0.2);
    assert!((sequential.clocks.edge_separation_ns - 4.0).abs() < 1e-12);
    assert!((sequential.clocks.insertion_skew_ns - 0.0).abs() < 1e-12);

    let input = paths
        .iter()
        .find(|path| {
            matches!(path.path.launch, crate::TimingLaunch::PrimaryInput(_))
                && matches!(
                    path.path.endpoint,
                    crate::TimingPathEndpoint::SequentialCell(_)
                )
        })
        .unwrap();
    assert!(input.clocks.launch.virtual_reference);
    assert!(!input.clocks.capture.virtual_reference);
    assert!((input.clocks.edge_separation_ns - 4.2).abs() < 1e-12);
    assert!((input.clocks.insertion_skew_ns - 0.2).abs() < 1e-12);

    let report = constrained_timing_path_report(&nl, &paths);
    assert!(
        report.contains("launch: clk rise at 0.000000 ns")
            && report.contains("capture: clk rise at 0.000000 ns")
            && report.contains("edge separation 4.200000 ns, insertion skew 0.200000 ns"),
        "{report}"
    );
}

#[test]
fn input_output_delay_and_uncertainty_affect_slack() {
    let nl = and_netlist();
    let c = parse_sdc(
        r#"
        create_clock -name clk -period 2.0 [get_ports clk]
        set_clock_uncertainty 0.1 -setup [get_clocks clk]
        set_input_delay 0.3 -clock clk [get_ports a]
        set_output_delay 0.4 -clock clk [get_ports y]
    "#,
    )
    .unwrap();
    let r = analyze_with_sdc(&nl, &c).unwrap();
    // 2.0ns period - 0.1 uncertainty - 0.3 input delay
    // - 1.0ns AND2 - 0.4 output delay = 0.2ns.
    assert!((r.output_slack - 0.2).abs() < 1e-9);
    assert!((r.worst_slack - 0.2).abs() < 1e-9);
    let y = r.net_slacks.get("y").unwrap();
    assert!((y.output_ns.unwrap() - 0.2).abs() < 1e-9);
    assert!((y.late_slack_ns().unwrap() - 0.2).abs() < 1e-9);
}

#[test]
fn hold_uncertainty_and_min_arrivals_affect_hold_slack() {
    let nl = two_flop_netlist();
    let c = parse_sdc(
        r#"
        create_clock -name clk -period 4.0 [get_ports clk]
        set_input_delay 1.0 -clock clk [get_ports d]
        set_clock_uncertainty 0.10 -setup [get_clocks clk]
        set_clock_uncertainty 0.05 -hold [get_clocks clk]
    "#,
    )
    .unwrap();
    let r = analyze_with_sdc(&nl, &c).unwrap();
    // Launch flop: 1.2ns clk->q; capture hold is 0.2ns.
    assert!((r.reg2reg_hold_slack - 0.95).abs() < 1e-9);
    // Primary input launch: 1.0ns input delay - 0.2ns hold - 0.05ns uncertainty.
    assert!((r.in2reg_hold_slack - 0.75).abs() < 1e-9);
    assert!((r.hold_slack - 0.75).abs() < 1e-9);
    assert!((r.worst_slack - 0.75).abs() < 1e-9);
    assert!((r.hold_uncertainty - 0.05).abs() < 1e-9);
    let d = r.net_slacks.get("d").unwrap();
    let q1 = r.net_slacks.get("q1").unwrap();
    assert!((d.setup_ns.unwrap() - 2.3).abs() < 1e-9);
    assert!((d.hold_ns.unwrap() - 0.75).abs() < 1e-9);
    assert!((q1.setup_ns.unwrap() - 2.1).abs() < 1e-9);
    assert!((q1.hold_ns.unwrap() - 0.95).abs() < 1e-9);

    let paths = constrained_timing_paths(&nl, &r, 0);
    assert_eq!(paths.len(), 3);
    assert!((paths[0].slack_ns - 2.1).abs() < 1e-9);
    assert!((paths[0].path.arrival_ns - 1.2).abs() < 1e-9);
    assert!((paths[0].required_ns - 3.3).abs() < 1e-9);
    assert!(matches!(
        paths[0].path.endpoint,
        crate::TimingPathEndpoint::SequentialCell(_)
    ));
    assert!(constrained_timing_path_report(&nl, &paths).contains("slack 2.100"));

    let hold_paths = constrained_hold_timing_paths(&nl, &r, 0);
    assert_eq!(hold_paths.len(), 2);
    assert!((hold_paths[0].slack_ns - 0.75).abs() < 1e-9);
    assert!((hold_paths[0].path.path.arrival_ns - 1.0).abs() < 1e-9);
    assert!((hold_paths[0].required_ns - 0.25).abs() < 1e-9);
    assert!(matches!(
        hold_paths[0].path.path.launch,
        crate::TimingLaunch::PrimaryInput(_)
    ));
    assert!(matches!(
        hold_paths[1].path.path.launch,
        crate::TimingLaunch::SequentialCell(_)
    ));
    assert!(constrained_hold_timing_path_report(&nl, &hold_paths).contains("hold slack 0.750"));
}

#[test]
fn max_and_min_delay_limits_apply_to_paths() {
    let nl = and_netlist();
    let c = parse_sdc(
        r#"
        create_clock -name clk -period 2.0 [get_ports clk]
        set_max_delay 0.8 -from [get_ports a] -to [get_ports y]
        set_min_delay 0.5 -from [get_ports a] -to [get_ports y]
    "#,
    )
    .unwrap();
    let r = analyze_with_sdc(&nl, &c).unwrap();
    // AND2 max and min input-to-output delay are both 1.0 ns.
    assert!((r.max_delay_slack + 0.2).abs() < 1e-9);
    assert!((r.min_delay_slack - 0.5).abs() < 1e-9);
    assert!((r.worst_slack + 0.2).abs() < 1e-9);
}

#[test]
fn parses_design_wide_quality_limits() {
    let sdc = parse_sdc(
        "set_max_transition 0.25\n\
         set_max_capacitance 0.04\n\
         set_max_fanout 8",
    )
    .unwrap();
    assert_eq!(sdc.max_transition_ns, Some(0.25));
    assert_eq!(sdc.max_capacitance_pf, Some(0.04));
    assert_eq!(sdc.max_fanout, Some(8));
    let options = sdc.quality_options();
    assert_eq!(options.max_transition_ns, Some(0.25));
    assert_eq!(options.max_capacitance_pf, Some(0.04));
    assert_eq!(options.max_fanout, Some(8));

    assert!(parse_sdc("set_max_transition 0").is_err());
    assert!(parse_sdc("set_max_capacitance -1").is_err());
    assert!(parse_sdc("set_max_fanout 0").is_err());
    assert!(parse_sdc("set_max_fanout x").is_err());
    assert!(parse_sdc("set_max_transition 0.1 [get_clocks clk]").is_err());
    assert!(parse_sdc("set_max_transition 0.1 -from [get_ports a]").is_err());
}

#[test]
fn parses_object_scoped_quality_limits() {
    let sdc = parse_sdc(
        "set_max_transition 10\n\
         set_max_transition 0.11 [get_ports y]\n\
         set_max_transition 0.12 [get_pins u1/Y]\n\
         set_max_capacitance 10\n\
         set_max_capacitance 0.021 [get_nets n*]\n\
         set_max_fanout 16\n\
         set_max_fanout 2 [get_cells u*]",
    )
    .unwrap();
    assert_eq!(sdc.max_transition_ns, Some(10.0));
    assert_eq!(sdc.scoped_max_transition_ns.len(), 2);
    assert_eq!(
        sdc.scoped_max_transition_ns[0].object,
        TimingObjectRef::new(TimingObjectKind::Port, "y")
    );
    assert_eq!(sdc.scoped_max_transition_ns[0].limit, 0.11);
    assert_eq!(
        sdc.scoped_max_transition_ns[1].object,
        TimingObjectRef::new(TimingObjectKind::Pin, "u1/Y")
    );
    assert_eq!(sdc.scoped_max_capacitance_pf.len(), 1);
    assert_eq!(
        sdc.scoped_max_capacitance_pf[0].object.kind,
        TimingObjectKind::Net
    );
    assert_eq!(sdc.scoped_max_fanout[0].limit, 2.0);
    let options = sdc.quality_options();
    assert_eq!(
        options.scoped_max_transition_ns,
        sdc.scoped_max_transition_ns
    );
    assert_eq!(
        options.scoped_max_capacitance_pf,
        sdc.scoped_max_capacitance_pf
    );
    assert_eq!(options.scoped_max_fanout, sdc.scoped_max_fanout);
}

#[test]
fn parses_input_transition_and_load_rules() {
    let sdc = parse_sdc(
        "set_input_transition 0.12 [get_ports a]\n\
         set_input_transition 0.20 [get_pins u1/a]\n\
         set_input_transition 0.30 [get_pins u1/a]\n\
         set_load 0.021 [get_ports y]\n\
         set_load -pin_load 0.032 [get_pins u1/a]",
    )
    .unwrap();
    assert_eq!(sdc.input_transitions.len(), 2);
    assert_eq!(sdc.input_transitions[0].limit, 0.12);
    assert_eq!(sdc.input_transitions[1].object.name, "u1/a");
    assert_eq!(sdc.input_transitions[1].limit, 0.30);
    assert_eq!(sdc.load_capacitances.len(), 2);
    assert_eq!(sdc.load_capacitances[0].limit, 0.021);
    assert_eq!(sdc.load_capacitances[1].limit, 0.032);
    let options = sdc.quality_options();
    assert_eq!(options.input_transitions, sdc.input_transitions);
    assert_eq!(options.load_capacitances, sdc.load_capacitances);

    assert!(parse_sdc("set_input_transition -1 [get_ports a]").is_err());
    assert!(parse_sdc("set_load -1 [get_ports y]").is_err());
    assert!(parse_sdc("set_input_transition 0.1 [get_cells u1]").is_err());
    assert!(parse_sdc("set_load 0.1 [get_clocks clk]").is_err());
    assert!(parse_sdc("set_load -wire_load 0.1 [get_ports y]").is_err());
}

#[test]
fn parses_clock_transition_rules() {
    let sdc = parse_sdc(
        "create_clock -name clk -period 4 [get_ports clk]\n\
         set_clock_transition -rise 0.12 [get_clocks clk]\n\
         set_clock_transition -fall 0.24 [get_clocks clk]",
    )
    .unwrap();
    assert_eq!(sdc.clocks["clk"].rise_transition_ns, 0.12);
    assert_eq!(sdc.clocks["clk"].fall_transition_ns, 0.24);

    let both = parse_sdc(
        "create_clock -name c -period 2 [get_ports c]\n\
         set_clock_transition -min -max 0.31 [get_clocks c]",
    )
    .unwrap();
    assert_eq!(both.clocks["c"].rise_transition_ns, 0.31);
    assert_eq!(both.clocks["c"].fall_transition_ns, 0.31);

    let split = parse_sdc(
        "create_clock -name c -period 2 [get_ports c]\n\
         set_clock_transition -min 0.21 [get_clocks c]\n\
         set_clock_transition -max 0.33 [get_clocks c]",
    )
    .unwrap();
    assert_eq!(split.clocks["c"].min_rise_transition_ns, 0.21);
    assert_eq!(split.clocks["c"].min_fall_transition_ns, 0.21);
    assert_eq!(split.clocks["c"].rise_transition_ns, 0.33);
    assert_eq!(split.clocks["c"].fall_transition_ns, 0.33);

    assert!(parse_sdc(
        "create_clock -name c -period 2 [get_ports c]\n\
         set_clock_transition -rise -1 [get_clocks c]"
    )
    .is_err());
    assert!(parse_sdc(
        "create_clock -name c -period 2 [get_ports c]\n\
         set_clock_transition 0.1 [get_ports c]"
    )
    .is_err());
}

#[test]
fn min_pulse_width_checks_clock_waveform() {
    let nl = two_flop_netlist();
    let c = parse_sdc(
        r#"
        create_clock -name clk -period 4.0 -fall 0.3 [get_ports clk]
        set_input_delay 1.0 -clock clk [get_ports d]
    "#,
    )
    .unwrap();
    let r = analyze_with_sdc(&nl, &c).unwrap();
    // Standard DFFs require 0.25 ns high and low pulses.
    assert!((r.min_pulse_high_slack - 0.05).abs() < 1e-9);
    assert!((r.min_pulse_low_slack - 3.45).abs() < 1e-9);
    assert!((r.min_pulse_width_slack - 0.05).abs() < 1e-9);
    assert!((r.worst_slack - 0.05).abs() < 1e-9);
}

#[test]
fn recovery_and_removal_checks_async_reset() {
    let lib = Arc::new(CellLibrary::std());
    let mut nl = Netlist::new("async");
    nl.set_library(lib.clone());
    let clk = nl.add_net("clk");
    let d = nl.add_net("d");
    let rst = nl.add_net("rst");
    let q = nl.add_net("q");
    nl.add_cell(Cell {
        name: "ff".into(),
        kind: CellKind::Lib(lib.find("DFFR").unwrap()),
        inputs: vec![clk, d, rst],
        output: q,
    });
    nl.add_port("clk", Dir::In, vec![clk]);
    nl.add_port("d", Dir::In, vec![d]);
    nl.add_port("rst", Dir::In, vec![rst]);
    nl.add_port("q", Dir::Out, vec![q]);
    let c = parse_sdc(
        r#"
        create_clock -name clk -period 4.0 [get_ports clk]
        set_input_delay 1.2 -clock clk [get_ports rst]
        set_clock_uncertainty 0.10 -setup [get_clocks clk]
        set_clock_uncertainty 0.05 -hold [get_clocks clk]
    "#,
    )
    .unwrap();
    let base = crate::analyze_with_constraints(&nl, &c).unwrap();
    assert_eq!(base.reset_endpoint_list().len(), 1);
    let unconstrained_reset = base.reset_timing_paths(&nl, 0);
    assert_eq!(unconstrained_reset.len(), 1);
    assert_eq!(unconstrained_reset[0].recovery.cells.len(), 1);
    assert_eq!(unconstrained_reset[0].removal.nets, vec![rst]);
    assert!(crate::reset_timing_path_report(&nl, &unconstrained_reset).contains("recovery arrival"));
    let r = analyze_constrained(&nl, base, &c).unwrap();
    // Effective period 3.9 - reset arrival 1.2 - recovery 0.3.
    assert!((r.recovery_slack - 2.4).abs() < 1e-9);
    // Reset arrival 1.2 - removal 0.15 - hold uncertainty 0.05.
    assert!((r.removal_slack - 1.0).abs() < 1e-9);
    assert!((r.async_reset_slack - 1.0).abs() < 1e-9);

    let reset_slack = r.net_slacks.get("rst").unwrap();
    assert!((reset_slack.recovery_ns.unwrap() - 2.4).abs() < 1e-9);
    assert!((reset_slack.removal_ns.unwrap() - 1.0).abs() < 1e-9);
    let reset_paths = constrained_reset_timing_paths(&nl, &r, 0);
    assert_eq!(reset_paths.len(), 2);
    assert_eq!(reset_paths[1].kind, ResetCheckKind::Recovery);
    assert!((reset_paths[1].slack_ns - 2.4).abs() < 1e-9);
    assert!((reset_paths[1].required_ns - 3.6).abs() < 1e-9);
    assert_eq!(reset_paths[0].kind, ResetCheckKind::Removal);
    assert!((reset_paths[0].slack_ns - 1.0).abs() < 1e-9);
    assert!((reset_paths[0].required_ns - 0.2).abs() < 1e-9);
    assert!(constrained_reset_timing_path_report(&nl, &reset_paths)
        .contains("Constrained reset timing path report"));

    let derated = parse_sdc(
        r#"
        create_clock -name clk -period 4.0 [get_ports clk]
        set_input_delay 1.2 -clock clk [get_ports rst]
        set_clock_uncertainty 0.10 -setup [get_clocks clk]
        set_clock_uncertainty 0.05 -hold [get_clocks clk]
        set_timing_derate -cell_check -early 0.5 -late 2.0 [get_cells ff]
    "#,
    )
    .unwrap();
    let derated_result = analyze_with_sdc(&nl, &derated).unwrap();
    // Recovery is a max check: 3.9 - 1.2 - 0.3 * late 2.0.
    assert!((derated_result.recovery_slack - 2.1).abs() < 1e-12);
    // Removal is a min check: 1.2 - 0.15 * early 0.5 - 0.05.
    assert!((derated_result.removal_slack - 1.075).abs() < 1e-12);
    assert_eq!(derated_result.cell_check_derates.len(), 1);
}

#[test]
fn clock_latency_affects_io_but_not_reg2reg() {
    let nl = two_flop_netlist();
    let c = parse_sdc(
        r#"
        create_clock -name clk -period 4.0 [get_ports clk]
        set_clock_latency -source 0.7 [get_clocks clk]
        set_clock_latency 0.3 [get_clocks clk]
        set_input_delay 1.0 -clock clk [get_ports d]
    "#,
    )
    .unwrap();
    let r = analyze_with_sdc(&nl, &c).unwrap();
    // Common source/network latency cancels for same-clock reg-to-reg.
    assert!((r.reg2reg_slack - 2.2).abs() < 1e-9);
    // External input launch does not include local clock network latency.
    assert!((r.in2reg_slack - 2.7).abs() < 1e-9);
    // Internal flop launch includes source plus local network latency.
    assert!((r.output_slack - 1.8).abs() < 1e-9);
    assert!((r.source_latency - 0.7).abs() < 1e-9);
    assert!((r.network_latency - 0.3).abs() < 1e-9);
}

#[test]
fn false_path_relaxes_external_endpoint() {
    let nl = and_netlist();
    let c = parse_sdc(
        r#"
        create_clock -name clk -period 2.0 [get_ports clk]
        set_input_delay 0.3 -clock clk [get_ports a]
        set_output_delay 0.4 -clock clk [get_ports y]
        set_false_path -from [get_ports a] -to [get_ports y]
    "#,
    )
    .unwrap();
    let r = analyze_with_sdc(&nl, &c).unwrap();
    assert!(r.output_slack.is_infinite());
    assert_eq!(r.exceptions_applied, 1);
}

#[test]
fn setup_or_hold_scoped_false_paths_apply_independently() {
    let source = |option: &str| {
        format!(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 1.0 -clock clk [get_ports d]
            set_clock_uncertainty 0.10 -setup [get_clocks clk]
            set_clock_uncertainty 0.05 -hold [get_clocks clk]
            set_false_path {option} -to [get_ports d]
        "#
        )
    };
    let nl = two_flop_netlist();

    let setup_only = parse_sdc(&source("-setup")).unwrap();
    assert_eq!(setup_only.false_paths[0].check, PathExceptionCheck::Setup);
    let setup_result = analyze_with_sdc(&nl, &setup_only).unwrap();
    assert!(setup_result.in2reg_slack.is_infinite());
    assert!((setup_result.in2reg_hold_slack - 0.75).abs() < 1e-9);
    let setup_excluded = excluded_timing_paths(&nl, &setup_result);
    assert!(setup_excluded
        .iter()
        .any(|path| path.check == ExcludedTimingCheck::Setup));
    assert!(setup_excluded
        .iter()
        .all(|path| path.check != ExcludedTimingCheck::Hold));

    let hold_only = parse_sdc(&source("-hold")).unwrap();
    assert_eq!(hold_only.false_paths[0].check, PathExceptionCheck::Hold);
    let hold_result = analyze_with_sdc(&nl, &hold_only).unwrap();
    assert!((hold_result.in2reg_slack - 2.3).abs() < 1e-9);
    assert!(hold_result.in2reg_hold_slack.is_infinite());
    let hold_excluded = excluded_timing_paths(&nl, &hold_result);
    assert!(hold_excluded
        .iter()
        .any(|path| path.check == ExcludedTimingCheck::Hold));
    assert!(hold_excluded
        .iter()
        .all(|path| path.check != ExcludedTimingCheck::Setup));

    assert!(parse_sdc(&source("-setup -hold")).is_err());
}

#[test]
fn instance_scoped_false_path_applies_only_to_matching_hierarchy() {
    let mut nl = and_netlist();
    let y = nl.net_by_name("y").unwrap();
    nl.hierarchy = vec![NetlistHierarchyInstance {
        path: "top.u1.u_leaf".into(),
        module: "leaf".into(),
        parent: Some("top.u1".into()),
        depth: 2,
        nets: vec![y],
        ports: vec![NetlistHierarchyPort {
            name: "y".into(),
            direction: Dir::Out,
            local_nets: vec![y],
            nets: vec![y],
            width: 1,
        }],
    }];
    let source = r#"
        create_clock -name clk -period 2.0 [get_ports a]
        set_input_delay 0.3 -clock clk [get_ports a]
        set_output_delay 0.4 -clock clk [get_ports y]
        set_false_path -from [get_ports a] -to [get_ports y] -instance top.u1.u_leaf
    "#;
    let matching = parse_sdc(source).unwrap();
    assert_eq!(
        matching.false_paths[0].instance_pattern.as_deref(),
        Some("top.u1.u_leaf")
    );
    let result = analyze_with_sdc(&nl, &matching).unwrap();
    assert!(result.output_slack.is_infinite());
    assert_eq!(result.exceptions_applied, 1);

    let nonmatching = parse_sdc(
        r#"
        create_clock -name clk -period 2.0 [get_ports a]
        set_input_delay 0.3 -clock clk [get_ports a]
        set_output_delay 0.4 -clock clk [get_ports y]
        set_false_path -from [get_ports a] -to [get_ports y] -instance top.other.*
    "#,
    )
    .unwrap();
    let result = analyze_with_sdc(&nl, &nonmatching).unwrap();
    assert!(result.output_slack.is_finite());
    assert_eq!(result.exceptions_applied, 0);
    assert!(parse_sdc("set_false_path -from a -instance").is_err());
}

#[test]
fn instance_scoped_delay_limit_applies_only_to_matching_hierarchy() {
    let mut nl = and_netlist();
    let y = nl.net_by_name("y").unwrap();
    nl.hierarchy = vec![NetlistHierarchyInstance {
        path: "top.u1.u_leaf".into(),
        module: "leaf".into(),
        parent: Some("top.u1".into()),
        depth: 2,
        nets: vec![y],
        ports: vec![NetlistHierarchyPort {
            name: "y".into(),
            direction: Dir::Out,
            local_nets: vec![y],
            nets: vec![y],
            width: 1,
        }],
    }];
    let source = r#"
        create_clock -name clk -period 2.0 [get_ports a]
        set_input_delay 0.3 -clock clk [get_ports a]
        set_max_delay 0.2 -from [get_ports a] -to [get_ports y] -instance top.u1.u_leaf
    "#;
    let matching = parse_sdc(source).unwrap();
    assert_eq!(
        matching.max_delay_paths[0].instance_pattern.as_deref(),
        Some("top.u1.u_leaf")
    );
    let result = analyze_with_sdc(&nl, &matching).unwrap();
    assert_eq!(result.path_max_delay_limits["Port:y"].len(), 1);
    assert!(result.max_delay_slack.is_finite());
    assert!((result.max_delay_slack + 1.1).abs() < 1e-12);

    let nonmatching = parse_sdc(
        r#"
        create_clock -name clk -period 2.0 [get_ports a]
        set_input_delay 0.3 -clock clk [get_ports a]
        set_max_delay 0.2 -from [get_ports a] -to [get_ports y] -instance top.other.*
    "#,
    )
    .unwrap();
    let result = analyze_with_sdc(&nl, &nonmatching).unwrap();
    assert_eq!(result.path_max_delay_limits["Port:y"].len(), 0);
    assert!(result.max_delay_slack.is_infinite());
    assert!(parse_sdc("set_max_delay 1 -from a -instance").is_err());
}

#[test]
fn instance_scoped_disabled_timing_arc_matches_only_hierarchy_owner() {
    let mut nl = and_netlist();
    let y = nl.net_by_name("y").unwrap();
    nl.hierarchy = vec![NetlistHierarchyInstance {
        path: "top.u1.u_leaf".into(),
        module: "leaf".into(),
        parent: Some("top.u1".into()),
        depth: 2,
        nets: vec![y],
        ports: vec![NetlistHierarchyPort {
            name: "y".into(),
            direction: Dir::Out,
            local_nets: vec![y],
            nets: vec![y],
            width: 1,
        }],
    }];
    let source = "set_disable_timing [get_cells u1] -instance top.u1.u_leaf";
    let matching = parse_sdc(source).unwrap();
    assert_eq!(
        matching.disabled_timing_arcs[0].instance_pattern.as_deref(),
        Some("top.u1.u_leaf")
    );
    assert!(timing_arc_disabled(&nl, &matching, 0, Some(0), None));

    let nonmatching = parse_sdc("set_disable_timing [get_cells u1] -instance top.other.*").unwrap();
    assert!(!timing_arc_disabled(&nl, &nonmatching, 0, Some(0), None));
    assert!(parse_sdc("set_disable_timing -instance top.u1").is_err());
    assert!(parse_sdc("set_disable_timing -instance").is_err());
}

#[test]
fn instance_scoped_timing_derate_applies_only_to_hierarchy_owner() {
    let mut nl = and_netlist();
    let y = nl.net_by_name("y").unwrap();
    nl.hierarchy = vec![NetlistHierarchyInstance {
        path: "top.u1.u_leaf".into(),
        module: "leaf".into(),
        parent: Some("top.u1".into()),
        depth: 2,
        nets: vec![y],
        ports: vec![NetlistHierarchyPort {
            name: "y".into(),
            direction: Dir::Out,
            local_nets: vec![y],
            nets: vec![y],
            width: 1,
        }],
    }];
    let matching = parse_sdc(
        r#"
        create_clock -name clk -period 2.0 [get_ports a]
        set_timing_derate -cell_delay -late 2.0 [get_cells *] -instance top.u1.u_leaf
    "#,
    )
    .unwrap();
    assert_eq!(matching.scoped_cell_timing_derates.len(), 1);
    assert_eq!(
        matching.scoped_cell_timing_derates[0]
            .instance_pattern
            .as_deref(),
        Some("top.u1.u_leaf")
    );
    let result = analyze_with_sdc(&nl, &matching).unwrap();
    assert!((result.base.net_arrival(y).unwrap() - 2.0).abs() < 1e-12);

    let nonmatching = parse_sdc(
        r#"
        create_clock -name clk -period 2.0 [get_ports a]
        set_timing_derate -cell_delay -late 2.0 [get_cells *] -instance top.other.*
    "#,
    )
    .unwrap();
    let result = analyze_with_sdc(&nl, &nonmatching).unwrap();
    assert!((result.base.net_arrival(y).unwrap() - 1.0).abs() < 1e-12);
    assert!(parse_sdc("set_timing_derate -late 2 -instance top.u1").is_err());
    assert!(parse_sdc("set_timing_derate -late 2 [get_ports a] -instance top.u1").is_err());
}

#[test]
fn multicycle_path_doubles_output_requirement() {
    let nl = and_netlist();
    let source = |check: &str| {
        format!(
            r#"
        create_clock -name clk -period 2.0 [get_ports clk]
        set_input_delay 0.3 -clock clk [get_ports a]
        set_output_delay 0.4 -clock clk [get_ports y]
        set_multicycle_path 2 {check} -from [get_ports a] -to [get_ports y]
    "#
        )
    };
    let c = parse_sdc(&source("")).unwrap();
    let r = analyze_with_sdc(&nl, &c).unwrap();
    assert!((r.output_slack - 2.3).abs() < 1e-9);

    let setup = parse_sdc(&source("-setup")).unwrap();
    assert_eq!(setup.multicycle_paths[0].check, PathExceptionCheck::Setup);
    let setup_result = analyze_with_sdc(&nl, &setup).unwrap();
    assert!((setup_result.output_slack - 2.3).abs() < 1e-9);

    let hold = parse_sdc(&source("-hold")).unwrap();
    assert_eq!(hold.multicycle_paths[0].check, PathExceptionCheck::Hold);
    let hold_result = analyze_with_sdc(&nl, &hold).unwrap();
    assert!((hold_result.output_slack - 0.3).abs() < 1e-9);
}
