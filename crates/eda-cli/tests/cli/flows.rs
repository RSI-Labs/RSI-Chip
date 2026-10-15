//! CLI integration-test domain.

use super::common::{run_edu, TempFile};
use std::fs;

#[test]
fn ams_reports_power_law_aging_analysis() {
    let input = TempFile::new(
        "aging.spice",
        r#"aging smoke
Iin out 0 1m
R1 out 0 2k
.aging V(out) years=9 exp=0.5 rinc=0.1
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Aging: target=op, output=V(out)")
            && report.contains("stress=3.000000000")
            && report.contains("nominal=-2.000000000")
            && report.contains("aged=-2.600000000")
            && report.contains("delta=-0.600000000"),
        "{report}"
    );
}

#[test]
fn eco_repairs_hold_violations() {
    let input = TempFile::new(
        "hold-repair.v",
        "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
        endmodule",
    );
    let sdc = TempFile::new(
        "hold-repair.sdc",
        r#"
create_clock -name clk -period 8.0 [get_ports clk]
set_input_delay 0.0 -clock clk [get_ports d]
set_clock_uncertainty 1.2 -hold [get_clocks clk]
"#,
    );
    let (ok, report) = run_edu(&["eco", input.path(), "--sdc", sdc.path(), "--repair-hold"]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Hold repair: 2 buffer(s), 1 endpoint(s), status PASS")
            && report.contains("slack -1.400000 -> 0.200000 ns"),
        "{report}"
    );
}

#[test]
fn eco_repairs_multicorner_hold_violations() {
    let input = TempFile::new(
        "hold-repair-mmsc.v",
        "module m(input clk, d, output reg q, output y);
            always @(posedge clk) q <= d;
            assign y = ~d;
        endmodule",
    );
    let mmsc = TempFile::new(
        "hold-repair-mmsc.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 12.0 [get_ports clk]
    set_input_delay 0.0 -clock clk [get_ports d]
    set_clock_uncertainty 2.0 -hold [get_clocks clk]
endscenario
scenario fast
    create_clock -name clk -period 10.0 [get_ports clk]
    set_input_delay 0.0 -clock clk [get_ports d]
    set_clock_uncertainty 1.2 -hold [get_clocks clk]
endscenario
"#,
    );
    let (ok, report) = run_edu(&["eco", input.path(), "--mmsc", mmsc.path(), "--repair-hold"]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multi-corner hold repair:")
            && report.contains("status PASS")
            && report.contains("scenario 'slow'")
            && report.contains("scenario 'fast'"),
        "{report}"
    );
}

#[test]
fn flow_accepts_mmsc_hold_repair() {
    let input = TempFile::new(
        "flow-mmsc-hold.v",
        "module m(input clk, d, output reg q, output y);
            always @(posedge clk) q <= d;
            assign y = ~d;
        endmodule",
    );
    let mmsc = TempFile::new(
        "flow-mmsc-hold.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 12.0 [get_ports clk]
    set_input_delay 0.0 -clock clk [get_ports d]
    set_clock_uncertainty 1.0 -hold [get_clocks clk]
endscenario
scenario fast
    create_clock -name clk -period 10.0 [get_ports clk]
    set_input_delay 0.0 -clock clk [get_ports d]
    set_clock_uncertainty 0.8 -hold [get_clocks clk]
endscenario
"#,
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--mmsc",
        mmsc.path(),
        "--physical-timing",
        "--repair-hold",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multi-corner physical timing flow:")
            && report.contains("Multi-corner hold repair:")
            && report.contains("status PASS"),
        "{report}"
    );
}

#[test]
fn flow_accepts_single_corner_hold_repair() {
    let input = TempFile::new(
        "flow-hold.v",
        "module m(input clk, d, output reg q, output y);
            always @(posedge clk) q <= d;
            assign y = ~d;
        endmodule",
    );
    let sdc = TempFile::new(
        "flow-hold.sdc",
        r#"
create_clock -name clk -period 8.0 [get_ports clk]
set_input_delay 0.0 -clock clk [get_ports d]
set_clock_uncertainty 1.2 -hold [get_clocks clk]
"#,
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--sdc",
        sdc.path(),
        "--physical-timing",
        "--repair-hold",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Physical timing flow:")
            && report.contains("Hold repair:")
            && report.contains("status PASS"),
        "{report}"
    );
}

#[test]
fn flow_exports_best_physical_timing_views() {
    let input = TempFile::new(
        "flow-physical-export.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let lef = TempFile::new("flow-physical-export.lef", "");
    let def = TempFile::new("flow-physical-export.def", "");
    let spef = TempFile::new("flow-physical-export.spef", "");
    let sdf = TempFile::new("flow-physical-export.sdf", "");
    let verilog = TempFile::new("flow-physical-export.best.v", "");
    let upf = TempFile::new(
        "flow-physical-export.upf",
        r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_port vdd -domain TOP
create_supply_port vss -domain TOP
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_on_resistance sw 10.0
create_power_mode active
set_power_mode_switch active sw on
create_power_mode sleep
set_power_mode_switch sleep sw off
set_active_power_mode active
"#,
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--physical-timing",
        "--post-route-power",
        "--upf",
        upf.path(),
        "--all-power-modes",
        "--timing-ripgup",
        "1",
        "--lef",
        lef.path(),
        "--def",
        def.path(),
        "--extract-spef",
        spef.path(),
        "--write-sdf",
        sdf.path(),
        "--write-verilog",
        verilog.path(),
    ]);
    assert!(ok, "{report}");
    let lef_text = fs::read_to_string(lef.path()).expect("read best LEF");
    let def_text = fs::read_to_string(def.path()).expect("read best DEF");
    let spef_text = fs::read_to_string(spef.path()).expect("read best SPEF");
    let sdf_text = fs::read_to_string(sdf.path()).expect("read best SDF");
    let verilog_text = fs::read_to_string(verilog.path()).expect("read best Verilog");
    let parsed_def = eda_api::parse_physical_def(&def_text).expect("parse best DEF");
    let parsed_spef = eda_api::parse_parasitics(&spef_text).expect("parse best SPEF");
    let parsed_sdf = eda_api::parse_delay_file(&sdf_text).expect("parse best SDF");
    let reparsed_flow =
        eda_api::run_flow(&verilog_text, Some("m")).expect("verify best physical Verilog");
    assert!(
        report.contains("Physical timing flow:")
            && lef_text.contains("MACRO ")
            && parsed_def.tracks_per_channel > 0
            && !parsed_spef.nets.is_empty()
            && !parsed_sdf.interconnects.is_empty()
            && report.contains("Best physical Verilog export:")
            && report.contains("UPF IR report")
            && report.contains("DOMAIN 'TOP'")
            && report.contains("UPF power-mode IR report:")
            && report.contains("power mode 'active'")
            && report.contains("power mode 'sleep'")
            && reparsed_flow.equivalent,
        "{report}\n{lef_text}\n{def_text}\n{spef_text}\n{verilog_text}"
    );
}

#[test]
fn flow_write_sdf_requires_physical_timing_and_mmsc_template() {
    let input = TempFile::new(
        "flow-write-sdf-requires.v",
        "module m(input a, output y); assign y = a; endmodule",
    );
    let sdf = TempFile::new("flow-write-sdf-requires.sdf", "");
    let verilog = TempFile::new("flow-write-sdf-requires.best.v", "");
    let (no_timing_ok, no_timing_report) =
        run_edu(&["flow", input.path(), "--write-sdf", sdf.path()]);
    assert!(!no_timing_ok);
    assert!(
        no_timing_report.contains("--write-sdf requires --physical-timing"),
        "{no_timing_report}"
    );

    let (no_verilog_timing_ok, no_verilog_timing_report) =
        run_edu(&["flow", input.path(), "--write-verilog", verilog.path()]);
    assert!(!no_verilog_timing_ok);
    assert!(
        no_verilog_timing_report.contains("--write-verilog requires --physical-timing"),
        "{no_verilog_timing_report}"
    );

    let (no_post_route_timing_ok, no_post_route_timing_report) =
        run_edu(&["flow", input.path(), "--post-route-power"]);
    assert!(!no_post_route_timing_ok);
    assert!(
        no_post_route_timing_report.contains("--post-route-power requires --physical-timing"),
        "{no_post_route_timing_report}"
    );

    let (all_modes_no_upf_ok, all_modes_no_upf_report) = run_edu(&[
        "flow",
        input.path(),
        "--physical-timing",
        "--post-route-power",
        "--all-power-modes",
    ]);
    assert!(!all_modes_no_upf_ok);
    assert!(
        all_modes_no_upf_report.contains("--all-power-modes requires --upf"),
        "{all_modes_no_upf_report}"
    );

    let mmsc = TempFile::new(
        "flow-write-sdf-requires.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
endscenario
"#,
    );
    let (mmmc_ok, mmmc_report) = run_edu(&[
        "flow",
        input.path(),
        "--mmsc",
        mmsc.path(),
        "--physical-timing",
        "--write-sdf",
        sdf.path(),
    ]);
    assert!(!mmmc_ok);
    assert!(
        mmmc_report.contains("--write-sdf with --mmsc requires a '{corner}' template"),
        "{mmmc_report}"
    );
}

#[test]
fn ir_accepts_power_mesh_analysis() {
    let input = TempFile::new(
        "ir-power-mesh.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let (ok, report) = run_edu(&["ir", input.path(), "--power-mesh"]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Power-mesh IR report:")
            && report.contains("NODE (")
            && report.contains("CELL '"),
        "{report}"
    );

    let (wrong_command_ok, wrong_command_report) = run_edu(&["pnr", input.path(), "--power-mesh"]);
    assert!(!wrong_command_ok);
    assert!(
        wrong_command_report.contains("--power-mesh requires the ir or flow command"),
        "{wrong_command_report}"
    );

    let (missing_power_ok, missing_power_report) =
        run_edu(&["flow", input.path(), "--physical-timing", "--power-mesh"]);
    assert!(!missing_power_ok);
    assert!(
        missing_power_report.contains("--power-mesh in flow requires --post-route-power"),
        "{missing_power_report}"
    );

    let (flow_ok, flow_report) = run_edu(&[
        "flow",
        input.path(),
        "--physical-timing",
        "--post-route-power",
        "--power-mesh",
        "--timing-ripgup",
        "1",
    ]);
    assert!(flow_ok, "{flow_report}");
    assert!(
        flow_report.contains("Post-route power/IR:")
            && flow_report.contains("Power-mesh IR report:")
            && flow_report.contains("worst segment"),
        "{flow_report}"
    );
}

#[test]
fn ir_accepts_multilayer_power_mesh_analysis() {
    let input = TempFile::new(
        "ir-multilayer-power-mesh.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let (ok, report) = run_edu(&["ir", input.path(), "--multilayer-power-mesh"]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multilayer power-mesh IR report:")
            && report.contains("via Vdd")
            && report.contains("via Gnd")
            && report.contains("worst drop:"),
        "{report}"
    );

    let (wrong_command_ok, wrong_command_report) =
        run_edu(&["pnr", input.path(), "--multilayer-power-mesh"]);
    assert!(!wrong_command_ok);
    assert!(
        wrong_command_report.contains("--multilayer-power-mesh requires the ir or flow command"),
        "{wrong_command_report}"
    );

    let (conflict_ok, conflict_report) = run_edu(&[
        "ir",
        input.path(),
        "--power-mesh",
        "--multilayer-power-mesh",
    ]);
    assert!(!conflict_ok);
    assert!(
        conflict_report.contains("choose either --power-mesh or --multilayer-power-mesh"),
        "{conflict_report}"
    );
}

#[test]
fn flow_accepts_multilayer_post_route_power_mesh() {
    let input = TempFile::new(
        "flow-multilayer-power-mesh.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--physical-timing",
        "--post-route-power",
        "--multilayer-power-mesh",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Post-route power/IR:")
            && report.contains("Multilayer power-mesh IR report:")
            && report.contains("via Vdd")
            && report.contains("worst drop:"),
        "{report}"
    );
}

#[test]
fn ir_accepts_coupled_power_domain_mesh() {
    let input = TempFile::new(
        "ir-coupled-power-domains.v",
        "module m(input a, b, c, d, output y, z);
            assign y = a & b;
            assign z = c | d;
         endmodule",
    );
    let upf = TempFile::new(
        "ir-coupled-power-domains.upf",
        r#"
upf_version 2.1
set_design_top m
create_power_domain DOMAIN_A -elements {u1_and2}
create_power_domain DOMAIN_B -elements {u2_or2}
create_supply_net vdda -domain DOMAIN_A
create_supply_net vssa -domain DOMAIN_A
create_supply_net vddb -domain DOMAIN_B
create_supply_net vssb -domain DOMAIN_B
set_domain_supply_net DOMAIN_A -primary_power_net vdda -primary_ground_net vssa
set_domain_supply_net DOMAIN_B -primary_power_net vddb -primary_ground_net vssb
create_power_mode active
set_active_power_mode active
"#,
    );
    let domains = TempFile::new(
        "ir-coupled-power-domains.mesh",
        r#"
supply DOMAIN_A 0.8
tap 0 0 0 vdd
end
supply DOMAIN_B 0.65
tap 0 0 2 vdd
end
"#,
    );
    let (ok, report) = run_edu(&[
        "ir",
        input.path(),
        "--upf",
        upf.path(),
        "--multilayer-power-mesh",
        "--power-mesh-domains",
        domains.path(),
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multilayer power-mesh IR report:")
            && report.contains("domain 'DOMAIN_A' V=0.800000")
            && report.contains("domain 'DOMAIN_B' V=0.650000")
            && report.contains("worst EM"),
        "{report}"
    );

    let (missing_upf_ok, missing_upf_report) = run_edu(&[
        "ir",
        input.path(),
        "--multilayer-power-mesh",
        "--power-mesh-domains",
        domains.path(),
    ]);
    assert!(!missing_upf_ok);
    assert!(
        missing_upf_report.contains("--power-mesh-domains requires --upf"),
        "{missing_upf_report}"
    );
}

#[test]
fn mmsc_flow_accepts_coupled_power_domain_mesh() {
    let input = TempFile::new(
        "mmsc-coupled-power-domains.v",
        "module m(input a, b, c, output z);
            wire n;
            assign n = a & b;
            assign z = c | n;
         endmodule",
    );
    let mmsc = TempFile::new(
        "mmsc-coupled-power-domains.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
endscenario
scenario fast
    create_clock -name clk -period 3.0 [get_ports a]
endscenario
"#,
    );
    let upf = TempFile::new(
        "mmsc-coupled-power-domains.upf",
        r#"
upf_version 2.1
set_design_top m
create_power_domain DOMAIN_A -elements {u1_and2}
create_power_domain DOMAIN_B -elements {u2_or2}
create_supply_net vdda -domain DOMAIN_A
create_supply_net vssa -domain DOMAIN_A
create_supply_net vddb -domain DOMAIN_B
create_supply_net vssb -domain DOMAIN_B
set_domain_supply_net DOMAIN_A -primary_power_net vdda -primary_ground_net vssa
set_domain_supply_net DOMAIN_B -primary_power_net vddb -primary_ground_net vssb
create_power_mode active
set_power_mode_domain_activity active DOMAIN_A 1.0
set_power_mode_domain_activity active DOMAIN_B 0.5
set_active_power_mode active
"#,
    );
    let domains = TempFile::new(
        "mmsc-coupled-power-domains.mesh",
        r#"
supply DOMAIN_A 0.8
tap 0 0 0 vdd
end
supply DOMAIN_B 0.65
tap 0 0 2 vdd
end
"#,
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--mmsc",
        mmsc.path(),
        "--physical-timing",
        "--post-route-power",
        "--all-power-modes",
        "--upf",
        upf.path(),
        "--multilayer-power-mesh",
        "--power-mesh-domains",
        domains.path(),
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multi-corner UPF power-mode IR report:")
            && report.contains("scenario 'slow'")
            && report.contains("scenario 'fast'")
            && report.contains("coupled_multilayer_mesh=true")
            && report.contains("domain 'DOMAIN_A' V=0.800000")
            && report.contains("domain 'DOMAIN_B' V=0.650000"),
        "{report}"
    );
}

#[test]
fn flow_accepts_mmsc_import_def_and_exports_best() {
    let input = TempFile::new(
        "flow-mmsc-import.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let imported = TempFile::new("flow-mmsc-import.in.def", "");
    let (export_ok, export_report) = run_edu(&["pnr", input.path(), "--def", imported.path()]);
    assert!(export_ok, "{export_report}");
    let mmsc = TempFile::new(
        "flow-mmsc-import.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_output_delay 0.3 -clock clk [get_ports y]
    set_scenario_property post_route.voltage_v 0.90
    set_scenario_property post_route.vdd_ohm_per_site 0.05
    set_scenario_property post_route.gnd_ohm_per_site 0.05
    set_scenario_property post_route.domain.TOP.voltage_v 0.85
    set_scenario_property post_route.domain.TOP.vdd_ohm_per_site 0.07
endscenario
scenario fast
    create_clock -name clk -period 3.0 [get_ports a]
    set_output_delay 0.3 -clock clk [get_ports y]
endscenario
"#,
    );
    let lef = TempFile::new("flow-mmsc-import.lef", "");
    let def = TempFile::new("flow-mmsc-import.out.def", "");
    let spef = TempFile::new("flow-mmsc-import.spef", "");
    let verilog = TempFile::new("flow-mmsc-import.best.v", "");
    let upf = TempFile::new(
        "flow-mmsc-import.upf",
        r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_port vdd -domain TOP
create_supply_port vss -domain TOP
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_on_resistance sw 10.0
set_power_switch_off_leakage sw 1e-9
create_power_mode active
set_power_mode_switch active sw on
create_power_mode sleep
set_power_mode_switch sleep sw off
set_active_power_mode active
"#,
    );
    let slow_sdf = TempFile::new("flow-mmsc-import.slow.sdf", "");
    let fast_sdf = TempFile::new("flow-mmsc-import.fast.sdf", "");
    let sdf_template = slow_sdf.path().replace(".slow.sdf", ".{corner}.sdf");
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--mmsc",
        mmsc.path(),
        "--physical-timing",
        "--post-route-power",
        "--upf",
        upf.path(),
        "--all-power-modes",
        "--power-mesh",
        "--import-def",
        imported.path(),
        "--timing-ripgup",
        "1",
        "--lef",
        lef.path(),
        "--def",
        def.path(),
        "--extract-spef",
        spef.path(),
        "--write-verilog",
        verilog.path(),
        "--write-sdf",
        &sdf_template,
    ]);
    assert!(ok, "{report}");
    let lef_text = fs::read_to_string(lef.path()).expect("read best MMMC LEF");
    let def_text = fs::read_to_string(def.path()).expect("read best MMMC DEF");
    let spef_text = fs::read_to_string(spef.path()).expect("read best MMMC SPEF");
    let verilog_text = fs::read_to_string(verilog.path()).expect("read best MMMC Verilog");
    let slow_sdf_text = fs::read_to_string(slow_sdf.path()).expect("read best MMMC slow SDF");
    let fast_sdf_text = fs::read_to_string(fast_sdf.path()).expect("read best MMMC fast SDF");
    let parsed_def = eda_api::parse_physical_def(&def_text).expect("parse best MMMC DEF");
    let parsed_spef = eda_api::parse_parasitics(&spef_text).expect("parse best MMMC SPEF");
    let parsed_slow_sdf =
        eda_api::parse_delay_file(&slow_sdf_text).expect("parse best MMMC slow SDF");
    let parsed_fast_sdf =
        eda_api::parse_delay_file(&fast_sdf_text).expect("parse best MMMC fast SDF");
    let reparsed_flow =
        eda_api::run_flow(&verilog_text, Some("m")).expect("verify best MMMC Verilog");
    assert!(
        report.contains("Multi-corner physical timing flow:")
            && report.contains("scenario 'slow'")
            && report.contains("scenario 'fast'")
            && report.contains("MMMC SDF exports: slow=")
            && report.contains(", fast=")
            && report.contains("Best physical Verilog export:")
            && report.contains("Multi-corner UPF power-mode IR report:")
            && report.contains("UPF IR report")
            && report.contains("DOMAIN 'TOP'")
            && report.contains("power mode 'active'")
            && report.contains("power mode 'sleep'")
            && report.contains("Power-mesh IR report:")
            && report.contains("worst drop 'slow' / 'active'")
            && report.contains("scenario 'slow'")
            && report.contains("scenario 'fast'")
            && reparsed_flow.equivalent
            && lef_text.contains("MACRO ")
            && parsed_def.tracks_per_channel > 0
            && !parsed_spef.nets.is_empty()
            && !parsed_slow_sdf.interconnects.is_empty()
            && !parsed_fast_sdf.interconnects.is_empty(),
        "{report}\n{lef_text}\n{def_text}\n{spef_text}\n{verilog_text}"
    );
}

#[test]
fn waiver_export_requires_a_waiver_source() {
    let input = TempFile::new(
        "waiver-export-source.v",
        "module m(input a, output y); assign y = ~a; endmodule",
    );
    let sdc = TempFile::new(
        "waiver-export-source.sdc",
        "create_clock -name clk -period 0.2 [get_ports a]",
    );
    let output = TempFile::new("waiver-export-source.waivers", "");
    let (ok, report) = run_edu(&[
        "sta",
        input.path(),
        "--sdc",
        sdc.path(),
        "--write-waivers",
        output.path(),
    ]);
    assert!(!ok, "{report}");
    assert!(
        report.contains("waiver export requires --waivers or --waiver-audit"),
        "{report}"
    );
}

#[test]
fn sta_rejects_conflicting_waiver_inputs() {
    let input = TempFile::new(
        "conflict.v",
        "module m(input a, output y); assign y = ~a; endmodule",
    );
    let sdc = TempFile::new(
        "conflict.sdc",
        "create_clock -name clk -period 0.2 [get_ports a]",
    );
    let waiver = TempFile::new("conflict.waivers", "waiver x clk y -1\n");
    let audit = TempFile::new(
        "conflict.audit",
        "audit 1 added 1 \"x\"\nwaiver x clk y -1\nend\n",
    );
    let (ok, report) = run_edu(&[
        "sta",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waivers",
        waiver.path(),
        "--waiver-audit",
        audit.path(),
    ]);
    assert!(!ok, "{report}");
    assert!(
        report.contains("choose either --waivers or --waiver-audit"),
        "{report}"
    );
}

#[test]
fn pnr_accepts_waiver_aware_timing_routing() {
    let input = TempFile::new(
        "pnr-waiver.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let sdc = TempFile::new(
        "pnr-waiver.sdc",
        r#"
create_clock -name clk -period 0.1 [get_ports a]
set_output_delay 0.1 -clock clk [get_ports y]
"#,
    );
    let waivers = TempFile::new(
        "pnr-waiver.waivers",
        r#"waiver "physical" "clk" "y" "-10.0"
kind output
"#,
    );
    let (ok, report) = run_edu(&[
        "pnr",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waivers",
        waivers.path(),
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Timing violation report for 'clk'")
            && report.contains("waived violations: 1")
            && report.contains("Routing report"),
        "{report}"
    );
}

#[test]
fn flow_accepts_physical_timing_waivers() {
    let input = TempFile::new(
        "flow-waiver.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let sdc = TempFile::new(
        "flow-waiver.sdc",
        r#"
create_clock -name clk -period 0.1 [get_ports a]
set_output_delay 0.1 -clock clk [get_ports y]
"#,
    );
    let waivers = TempFile::new(
        "flow-waiver.waivers",
        r#"waiver "physical" "clk" "y" "-10.0"
kind output
"#,
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waivers",
        waivers.path(),
        "--physical-timing",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Physical timing flow:")
            && report.contains("constrained slack:")
            && report.contains("best iteration"),
        "{report}"
    );
}

#[test]
fn flow_accepts_mmsc_physical_timing() {
    let input = TempFile::new(
        "flow-mmsc.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let mmsc = TempFile::new(
        "flow-mmsc.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_output_delay 0.3 -clock clk [get_ports y]
endscenario
scenario fast
    create_clock -name clk -period 3.0 [get_ports a]
    set_output_delay 0.3 -clock clk [get_ports y]
endscenario
"#,
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--mmsc",
        mmsc.path(),
        "--physical-timing",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multi-corner physical timing flow:")
            && report.contains("scenario 'slow'")
            && report.contains("scenario 'fast'")
            && report.contains("common HPWL"),
        "{report}"
    );
}

#[test]
fn flow_accepts_mmsc_timing_driven_cts() {
    let input = TempFile::new(
        "flow-mmsc-cts.v",
        "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
        endmodule",
    );
    let mmsc = TempFile::new(
        "flow-mmsc-cts.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 10.0 [get_ports clk]
    set_input_delay 0.5 -clock clk [get_ports d]
endscenario
scenario fast
    create_clock -name clk -period 8.0 [get_ports clk]
    set_input_delay 0.4 -clock clk [get_ports d]
endscenario
"#,
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--mmsc",
        mmsc.path(),
        "--physical-timing",
        "--timing-driven-cts",
        "--cts-target-skew",
        "0.2",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multi-corner physical timing flow:")
            && report.contains("Timing-driven CTS search:")
            && report.contains("target MET")
            && report.contains("scenario 'slow'")
            && report.contains("scenario 'fast'"),
        "{report}"
    );
}

#[test]
fn flow_accepts_mmsc_antenna_repair() {
    let input = TempFile::new(
        "flow-mmsc-antenna.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let mmsc = TempFile::new(
        "flow-mmsc-antenna.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_output_delay 0.3 -clock clk [get_ports y]
endscenario
scenario fast
    create_clock -name clk -period 3.0 [get_ports a]
    set_output_delay 0.3 -clock clk [get_ports y]
endscenario
"#,
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--mmsc",
        mmsc.path(),
        "--physical-timing",
        "--repair-antennas",
        "--antenna-max-diodes",
        "2",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multi-corner physical timing flow:")
            && report.contains("antenna diodes 0, antenna warnings 0"),
        "{report}"
    );
}

#[test]
fn flow_accepts_and_requires_antenna_repair() {
    let input = TempFile::new(
        "flow-antenna.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--physical-timing",
        "--repair-antennas",
        "--antenna-max-diodes",
        "2",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("antenna diodes 0, antenna warnings 0")
            && report.contains("Physical timing flow:"),
        "{report}"
    );

    let (rejected, rejected_report) = run_edu(&["flow", input.path(), "--repair-antennas"]);
    assert!(!rejected, "{rejected_report}");
    assert!(
        rejected_report.contains("--repair-antennas requires --physical-timing"),
        "{rejected_report}"
    );

    let (limit_rejected, limit_report) = run_edu(&[
        "flow",
        input.path(),
        "--physical-timing",
        "--antenna-max-diodes",
        "2",
    ]);
    assert!(!limit_rejected, "{limit_report}");
    assert!(
        limit_report.contains("--antenna-max-diodes requires --repair-antennas"),
        "{limit_report}"
    );
}

#[test]
fn cli_imports_def_physical_views() {
    let input = TempFile::new(
        "import-def.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let exported = TempFile::new("import-def-output.def", "");
    let (export_ok, export_report) = run_edu(&["pnr", input.path(), "--def", exported.path()]);
    assert!(export_ok, "{export_report}");
    let def_text = fs::read_to_string(exported.path()).expect("read exported DEF");
    assert!(
        def_text.contains("TRACKS ") && def_text.contains("COMPONENTS"),
        "{def_text}"
    );

    let (drc_ok, drc_report) = run_edu(&["drc", input.path(), "--import-def", exported.path()]);
    assert!(drc_ok, "{drc_report}");
    assert!(
        drc_report.contains("DRC report: 0 error(s)") && drc_report.contains("0 warning(s)"),
        "{drc_report}"
    );

    let (pnr_ok, pnr_report) = run_edu(&[
        "pnr",
        input.path(),
        "--import-def",
        exported.path(),
        "--maze",
    ]);
    assert!(pnr_ok, "{pnr_report}");
    assert!(
        pnr_report.contains("PnR report") && pnr_report.contains("Routing report"),
        "{pnr_report}"
    );

    let (flow_ok, flow_report) = run_edu(&[
        "flow",
        input.path(),
        "--import-def",
        exported.path(),
        "--physical-timing",
        "--timing-ripgup",
        "1",
    ]);
    assert!(flow_ok, "{flow_report}");
    assert!(
        flow_report.contains("Physical timing flow:") && flow_report.contains("best iteration"),
        "{flow_report}"
    );

    let (rejected, rejected_report) =
        run_edu(&["flow", input.path(), "--import-def", exported.path()]);
    assert!(!rejected, "{rejected_report}");
    assert!(
        rejected_report.contains("--import-def requires --physical-timing"),
        "{rejected_report}"
    );
}

#[test]
fn cts_accepts_timing_driven_search() {
    let input = TempFile::new(
        "timing-cts.v",
        "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
        endmodule",
    );
    let (ok, report) = run_edu(&[
        "cts",
        input.path(),
        "--timing-driven-cts",
        "--cts-target-skew",
        "1.0",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Timing-driven CTS search:")
            && report.contains("selected 2")
            && report.contains("target MET"),
        "{report}"
    );

    let (rejected, rejected_report) = run_edu(&["cts", input.path(), "--cts-target-skew", "1.0"]);
    assert!(!rejected, "{rejected_report}");
    assert!(
        rejected_report.contains("--cts-target-skew requires --timing-driven-cts"),
        "{rejected_report}"
    );
}

#[test]
fn flow_accepts_timing_driven_cts() {
    let input = TempFile::new(
        "flow-cts.v",
        "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
        endmodule",
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--physical-timing",
        "--timing-driven-cts",
        "--cts-target-skew",
        "0.2",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Physical timing flow:")
            && report.contains("Timing-driven CTS search:")
            && report.contains("target MET"),
        "{report}"
    );

    let (rejected, rejected_report) = run_edu(&["flow", input.path(), "--timing-driven-cts"]);
    assert!(!rejected, "{rejected_report}");
    assert!(
        rejected_report.contains("--timing-driven-cts in flow requires --physical-timing"),
        "{rejected_report}"
    );
}

#[test]
fn flow_rejects_waivers_without_physical_timing() {
    let input = TempFile::new(
        "flow-waiver-required.v",
        "module m(input a, output y); assign y = ~a; endmodule",
    );
    let sdc = TempFile::new(
        "flow-waiver-required.sdc",
        "create_clock -name clk -period 0.1 [get_ports a]",
    );
    let waivers = TempFile::new("flow-waiver-required.waivers", "waiver x clk y -1\n");
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waivers",
        waivers.path(),
    ]);
    assert!(!ok, "{report}");
    assert!(
        report.contains("flow waivers require --physical-timing"),
        "{report}"
    );
}

#[test]
fn sta_paths_include_hierarchy_grouping() {
    let input = TempFile::new(
        "sta-hierarchy-paths.v",
        "module leaf(input a, b, output n1, n2);
            assign n1 = a & b;
            assign n2 = ~(a | b);
         endmodule
         module sub(input a, b, output y);
            wire n1, n2;
            leaf u_leaf(.a(a), .b(b), .n1(n1), .n2(n2));
            assign y = n1 & n2;
         endmodule
         module top(input a, b, output y);
            sub u1(.a(a), .b(b), .y(y));
         endmodule",
    );
    let (ok, report) = run_edu(&["sta", input.path(), "--top", "top", "--paths", "2"]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Timing path report:")
            && report.contains("Hierarchical timing path report:")
            && report.contains("instance 'top.u1.u_leaf'"),
        "{report}"
    );
}
