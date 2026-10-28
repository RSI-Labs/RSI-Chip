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
