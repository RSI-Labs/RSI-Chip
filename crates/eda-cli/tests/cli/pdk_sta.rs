//! CLI integration-test domain.

use super::common::{run_edu, TempFile};
use std::fs;

#[test]
fn pdk_signoff_reports_correlation_and_implementation_checks() {
    let input = TempFile::new(
        "pdk-input.v",
        "module top(input a, output y); assign y = ~a; endmodule",
    );
    let pdk = TempFile::new(
        "input.pdk",
        r#"pdk test-pdk
version 1.2
layer met1 routing pitch_um 0.34 min_width_um 0.14 min_spacing_um 0.14
corner tt process TT voltage_v 1.8 temperature_c 25
correlate nmos VTO nominal 0.45 golden 0.452 tolerance 0.01
signoff model-correlation
signoff equivalence
signoff timing
signoff drc
signoff physical-database
.end
"#,
    );
    let (ok, report) = run_edu(&["pdk", input.path(), "--top", "top", "--pdk", pdk.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Model correlation nmos.VTO")
            && report.contains("PDK corner result tt")
            && report.contains("Check equivalence: PASS")
            && report.contains("Check physical-database: PASS")
            && report.contains("Overall signoff: PASS"),
        "{report}"
    );
}

#[test]
fn sta_accepts_mmsc_waiver_audit_file() {
    let input = TempFile::new(
        "mmsc-input.v",
        "module inv(input a, output y); assign y = ~a; endmodule
         module sub(input a, output y); inv u_leaf(.a(a), .y(y)); endmodule
         module top(input a, output y1, y2);
            sub u1(.a(a), .y(y1));
            sub u2(.a(a), .y(y2));
         endmodule",
    );
    let mmsc = TempFile::new(
        "input.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_output_delay 0.2 -clock clk [get_ports y1]
    set_output_delay 0.2 -clock clk [get_ports y2]
endscenario
scenario fast
    create_clock -name clk -period 0.2 [get_ports a]
    set_output_delay 0.2 -clock clk [get_ports y1]
    set_output_delay 0.2 -clock clk [get_ports y2]
endscenario
"#,
    );
    let audit = TempFile::new(
        "waiver-audit.audit",
        r#"
audit 1 added 1 "engineer"
waiver "fast-only" "fast" "*" "-10.0"
instance "top.u1.u_leaf"
end
"#,
    );
    let (ok, report) = run_edu(&[
        "sta",
        input.path(),
        "--top",
        "top",
        "--mmsc",
        mmsc.path(),
        "--waiver-audit",
        audit.path(),
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multi-corner timing violation report")
            && report.contains("Timing waiver audit: 1 event(s)")
            && report.contains("waived violations: 1")
            && report.contains("Timing waiver audit hierarchy distribution: 1 instance scope(s)")
            && report.contains("'top.u1.u_leaf': events=1, added=1, removed=0, active=1"),
        "{report}"
    );
}

#[test]
fn sta_accepts_min_pulse_waiver_file() {
    let input = TempFile::new(
        "min-pulse.v",
        "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
        endmodule",
    );
    let sdc = TempFile::new(
        "min-pulse.sdc",
        r#"
create_clock -name clk -period 4.0 -fall 0.1 [get_ports clk]
set_input_delay 1.0 -clock clk [get_ports d]
"#,
    );
    let waivers = TempFile::new(
        "min-pulse.waivers",
        r#"waiver "short-clock" "clk" "*" "-1.0"
kind min_pulse_high
owner "timing"
"#,
    );
    let (ok, report) = run_edu(&[
        "sta",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waivers",
        waivers.path(),
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("min pulse high violation") && report.contains("waived violations: 2"),
        "{report}"
    );
}

#[test]
fn sta_accepts_explicit_delay_limit_waiver_file() {
    let input = TempFile::new(
        "delay-limit.v",
        "module m(input a, b, output y); assign y = a & b; endmodule",
    );
    let sdc = TempFile::new(
        "delay-limit.sdc",
        r#"
create_clock -name clk -period 4.0 [get_ports clk]
set_max_delay 0.5 -from [get_ports a] -to [get_ports y]
set_min_delay 1.5 -from [get_ports a] -to [get_ports y]
"#,
    );
    let waivers = TempFile::new(
        "delay-limit.waivers",
        r#"waiver "accepted-max" "clk" "y" "-1.0"
kind max_delay
owner "timing"
"#,
    );
    let (ok, report) = run_edu(&[
        "sta",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waivers",
        waivers.path(),
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("min_delay clk:min_delay:y") && report.contains("waived violations: 1"),
        "{report}"
    );
}

#[test]
fn sta_exports_waiver_audit_and_active_waivers() {
    let input = TempFile::new(
        "waiver-export.v",
        "module m(input a, output y); assign y = ~a; endmodule",
    );
    let sdc = TempFile::new(
        "waiver-export.sdc",
        "create_clock -name clk -period 0.2 [get_ports a]",
    );
    let source = TempFile::new(
        "waiver-export.waivers",
        r#"waiver "accepted" "clk" "y" "-1.0"
kind output
owner "timing"
"#,
    );
    let audit_output = TempFile::new("waiver-export.audit", "");
    let waiver_output = TempFile::new("waiver-export-round.waivers", "");

    let (first_ok, first_report) = run_edu(&[
        "sta",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waivers",
        source.path(),
        "--write-waiver-audit",
        audit_output.path(),
    ]);
    assert!(first_ok, "{first_report}");
    let audit_text = fs::read_to_string(audit_output.path()).expect("read exported waiver audit");
    let audit = eda_api::parse_timing_waiver_audit(&audit_text).expect("parse exported audit");
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].waiver_id, "accepted");

    let (second_ok, second_report) = run_edu(&[
        "sta",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waiver-audit",
        audit_output.path(),
        "--write-waivers",
        waiver_output.path(),
    ]);
    assert!(second_ok, "{second_report}");
    let waiver_text =
        fs::read_to_string(waiver_output.path()).expect("read exported active waivers");
    let waivers =
        eda_api::parse_timing_violation_waivers(&waiver_text).expect("parse exported waivers");
    assert_eq!(waivers.len(), 1);
    assert_eq!(waivers[0].id, "accepted");
    assert_eq!(waivers[0].kind, Some(eda_api::TimingViolationKind::Output));
}
