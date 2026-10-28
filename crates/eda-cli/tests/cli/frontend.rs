use super::common::{run_edu, TempFile};

#[test]
fn slang_info_reports_overlapping_continuous_drivers() {
    if std::env::var_os("SLANG_BIN").is_none() {
        return;
    }
    let input = TempFile::new(
        "lint-drivers.sv",
        r#"
module lint_drivers(input a, b, output [3:0] y);
assign y[1:0] = {b, a};
assign y[3:2] = {b, a};
assign y[2] = a;
endmodule
"#,
    );
    let (success, output) = run_edu(&["slang-info", input.path(), "--top", "lint_drivers"]);
    assert!(success, "unexpected CLI output: {output}");
    assert!(output.contains("lint findings: 1"), "{output}");
    assert!(
        output.contains("MultipleContinuousDrivers: 'y' has 2 continuous drivers"),
        "{output}"
    );
}

#[test]
fn slang_info_reports_continuous_assignment_cycles() {
    if std::env::var_os("SLANG_BIN").is_none() {
        return;
    }
    let input = TempFile::new(
        "combinational-loop.sv",
        r#"
module combinational_loop(input a, output x, y);
wire p;
assign p = a;
assign x = p;
assign y = x;
assign x = y;
endmodule
"#,
    );
    let (success, output) = run_edu(&["slang-info", input.path(), "--top", "combinational_loop"]);
    assert!(success, "unexpected CLI output: {output}");
    assert!(
        output.contains("CombinationalLoop: 'x' has 2 nodes in a continuous cycle"),
        "{output}"
    );
}

#[test]
fn slang_info_reports_incomplete_constant_cases() {
    if std::env::var_os("SLANG_BIN").is_none() {
        return;
    }
    let input = TempFile::new(
        "incomplete-case.sv",
        r#"
module incomplete_case(input [1:0] selector, output reg y);
always @* begin
  case (selector)
    2'd0: y = 1'b0;
    2'd1: y = 1'b1;
  endcase
end
endmodule
"#,
    );
    let (success, output) = run_edu(&["slang-info", input.path(), "--top", "incomplete_case"]);
    assert!(success, "unexpected CLI output: {output}");
    assert!(
        output.contains("IncompleteCase: 'selector' leaves 2 selector value(s) unhandled"),
        "{output}"
    );
}

#[test]
fn slang_info_reports_latch_risks() {
    if std::env::var_os("SLANG_BIN").is_none() {
        return;
    }
    let input = TempFile::new(
        "latch-risk.sv",
        r#"
module latch_risk(input en, d, output reg q);
always @* begin
  if (en) q = d;
end
endmodule
"#,
    );
    let (success, output) = run_edu(&["slang-info", input.path(), "--top", "latch_risk"]);
    assert!(success, "unexpected CLI output: {output}");
    assert!(
        output.contains("LatchRisk: 'q' has 1 conditionally assigned bit(s)"),
        "{output}"
    );
}

#[test]
fn slang_info_reports_unused_user_names() {
    if std::env::var_os("SLANG_BIN").is_none() {
        return;
    }
    let input = TempFile::new(
        "unused-name.sv",
        r#"
module unused_name(output y);
wire unused;
wire used = 1'b1;
assign y = used;
endmodule
"#,
    );
    let (success, output) = run_edu(&["slang-info", input.path(), "--top", "unused_name"]);
    assert!(success, "unexpected CLI output: {output}");
    assert!(
        output.contains("UnusedName: 'unused' declares 1 unused bit(s)"),
        "{output}"
    );
}

#[test]
fn slang_info_reports_process_combinational_loops() {
    if std::env::var_os("SLANG_BIN").is_none() {
        return;
    }
    let input = TempFile::new(
        "process-loop.sv",
        r#"
module process_loop(output reg a, output reg b);
always @* begin
  a = b;
  b = a;
end
endmodule
"#,
    );
    let (success, output) = run_edu(&["slang-info", input.path(), "--top", "process_loop"]);
    assert!(success, "unexpected CLI output: {output}");
    assert!(
        output.contains("CombinationalLoop: 'b' has 1 nodes in a continuous cycle"),
        "{output}"
    );
}

#[test]
fn slang_info_reports_assignment_width_mismatches() {
    if std::env::var_os("SLANG_BIN").is_none() {
        return;
    }
    let input = TempFile::new(
        "width-mismatch.sv",
        r#"
module width_mismatch(output [3:0] y, output [3:0] z);
wire one_bit = 1'b1;
assign y = one_bit;
assign z = 4'b0000;
endmodule
"#,
    );
    let (success, output) = run_edu(&["slang-info", input.path(), "--top", "width_mismatch"]);
    assert!(success, "unexpected CLI output: {output}");
    assert!(
        output.contains("WidthMismatch: 'y' destination/source widths differ by 3 bit(s)"),
        "{output}"
    );

    let explicit = TempFile::new(
        "explicit-width.sv",
        r#"
module explicit_width(output [3:0] y, input logic one_bit);
assign y = 4'(one_bit);
endmodule
"#,
    );
    let (success, output) = run_edu(&["slang-info", explicit.path(), "--top", "explicit_width"]);
    assert!(success, "unexpected CLI output: {output}");
    assert!(
        !output.contains("WidthMismatch: 'y'"),
        "explicit cast was reported as implicit: {output}"
    );
}

#[test]
fn slang_info_reports_implicit_signedness_mismatches() {
    if std::env::var_os("SLANG_BIN").is_none() {
        return;
    }
    let input = TempFile::new(
        "signedness-mismatch.sv",
        r#"
module signed_child(input logic signed [7:0] x);
endmodule

module signed_output_child(output logic signed [7:0] q);
assign q = 8'sh00;
endmodule

module signedness_mismatch(input logic [7:0] a, output logic signed [7:0] y);
wire logic [7:0] unsigned_source = 8'h00;
wire logic [7:0] unsigned_sink;
assign y = unsigned_source;
signed_child child(.x(a));
signed_output_child source(.q(unsigned_sink));
endmodule
"#,
    );
    let (success, output) = run_edu(&["slang-info", input.path(), "--top", "signedness_mismatch"]);
    assert!(success, "unexpected CLI output: {output}");
    assert!(
        output.contains("SignednessMismatch: 'y' destination/source signedness differs"),
        "{output}"
    );
    assert!(
        output.contains("SignednessMismatch: 'child.x' destination/source signedness differs"),
        "{output}"
    );
    assert!(
        output
            .contains("SignednessMismatch: 'unsigned_sink' destination/source signedness differs"),
        "{output}"
    );

    let explicit = TempFile::new(
        "explicit-signedness.sv",
        r#"
module explicit_signedness(input logic [7:0] a, output logic signed [7:0] y);
assign y = signed'(a);
endmodule
"#,
    );
    let (success, output) = run_edu(&[
        "slang-info",
        explicit.path(),
        "--top",
        "explicit_signedness",
    ]);
    assert!(success, "unexpected CLI output: {output}");
    assert!(
        !output.contains("SignednessMismatch: 'y'"),
        "explicit signed cast was reported as implicit: {output}"
    );
}

#[test]
fn slang_info_applies_lint_policy_waivers_and_severities() {
    if std::env::var_os("SLANG_BIN").is_none() {
        return;
    }
    let input = TempFile::new(
        "lint-policy.sv",
        r#"
module lint_policy(output [3:0] y);
wire unused;
wire one_bit = 1'b1;
assign y = one_bit;
endmodule
"#,
    );
    let policy = TempFile::new(
        "lint-policy.txt",
        "waive UnusedName unused owner=reviewer expires=2030-01-31 reason=reviewed dead logic\nseverity WidthMismatch Error\n",
    );
    let audit = TempFile::new("lint-audit.jsonl", "");
    let (success, output) = run_edu(&[
        "slang-info",
        input.path(),
        "--top",
        "lint_policy",
        "--lint-policy",
        policy.path(),
        "--lint-as-of",
        "2030-01-01",
        "--write-lint-audit",
        audit.path(),
    ]);
    assert!(success, "unexpected CLI output: {output}");
    assert!(
        output.contains("lint findings: 1") && output.contains("lint waived: 1"),
        "{output}"
    );
    assert!(
        output.contains("[Error] WidthMismatch: 'y' destination/source widths differ by 3 bit(s)"),
        "{output}"
    );
    assert!(
        output.contains(
            "Waived UnusedName: 'unused' owner: reviewer expiration: 2030-01-31 reason: reviewed dead logic"
        ),
        "{output}"
    );
    let audit_text = std::fs::read_to_string(audit.path()).expect("read lint audit");
    assert!(
        audit_text.starts_with(
            "{\"action\":\"AuditHeader\",\"design\":\"lint_policy\",\"top\":\"lint_policy\",\"policy\":"
        ),
        "{audit_text}"
    );
    assert!(
        audit_text.contains(
            "\"owner\":\"reviewer\",\"expiration\":\"2030-01-31\",\"reason\":\"reviewed dead logic\""
        ),
        "{audit_text}"
    );
    assert!(
        audit_text.contains("\"asOf\":\"2030-01-01\""),
        "{audit_text}"
    );

    let missing_value = run_edu(&[
        "slang-info",
        input.path(),
        "--top",
        "lint_policy",
        "--lint-policy",
    ]);
    assert!(
        !missing_value.0,
        "missing policy path must fail: {}",
        missing_value.1
    );

    let missing_audit_value = run_edu(&[
        "slang-info",
        input.path(),
        "--top",
        "lint_policy",
        "--write-lint-audit",
    ]);
    assert!(
        !missing_audit_value.0,
        "missing audit path must fail: {}",
        missing_audit_value.1
    );

    let expired = run_edu(&[
        "slang-info",
        input.path(),
        "--top",
        "lint_policy",
        "--lint-policy",
        policy.path(),
        "--lint-as-of",
        "2030-02-01",
    ]);
    assert!(expired.0, "unexpected expired-waiver output: {}", expired.1);
    assert!(
        expired.1.contains("lint findings: 2") && !expired.1.contains("lint waived:"),
        "{}",
        expired.1
    );

    let invalid_date = run_edu(&[
        "slang-info",
        input.path(),
        "--top",
        "lint_policy",
        "--lint-as-of",
        "2030-02-30",
    ]);
    assert!(
        !invalid_date.0,
        "invalid lint as-of date must fail: {}",
        invalid_date.1
    );
}

#[test]
fn slang_info_reports_unconnected_instance_ports() {
    if std::env::var_os("SLANG_BIN").is_none() {
        return;
    }
    let input = TempFile::new(
        "unconnected-ports.sv",
        r#"
module ports_child(input logic a, b, output logic c, d);
assign c = a;
assign d = b;
endmodule

module unconnected_ports(input logic a, output logic c);
ports_child child(.a(a), .c(c));
endmodule
"#,
    );
    let (success, output) = run_edu(&["slang-info", input.path(), "--top", "unconnected_ports"]);
    assert!(success, "unexpected CLI output: {output}");
    assert!(
        output.contains("UnconnectedPort: 'child.b' leaves 1 pin bit(s) unconnected"),
        "{output}"
    );
    assert!(
        output.contains("UnconnectedPort: 'child.d' leaves 1 pin bit(s) unconnected"),
        "{output}"
    );
}
