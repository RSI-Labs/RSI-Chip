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
