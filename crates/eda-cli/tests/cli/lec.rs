//! End-to-end tests for the `lec` command: RTL-vs-mapped and two-source LEC.

use super::common::{run_edu, TempFile};

const COUNTER: &str = r#"
module counter8 (
    input  wire       clk,
    input  wire       rst,
    input  wire       en,
    input  wire       up,
    input  wire       load,
    input  wire [7:0] load_value,
    output reg  [7:0] count,
    output wire       carry,
    output wire       zero
);
    assign zero  = (count == 8'd0);
    assign carry = up ? (en && (count == 8'hff)) : (en && (count == 8'h00));

    always @(posedge clk) begin
        if (rst)
            count <= 8'd0;
        else if (load)
            count <= load_value;
        else if (en)
            count <= up ? (count + 8'd1) : (count - 8'd1);
    end
endmodule
"#;

fn counter_bug() -> String {
    COUNTER.replace("count + 8'd1", "count + 8'd2")
}

#[test]
fn lec_command_reports_rtl_vs_mapped_equivalent() {
    let src = TempFile::new("lec-counter.v", COUNTER);
    let (ok, output) = run_edu(&["lec", src.path(), "--top", "counter8"]);
    assert!(ok, "{output}");
    assert!(output.contains("EQUIVALENT"), "{output}");
    assert!(output.contains("reference:"), "{output}");
    assert!(output.contains("mapped:"), "{output}");
}

#[test]
fn lec_command_two_source_reports_equivalent() {
    let src = TempFile::new("lec-gold.v", COUNTER);
    let reference = TempFile::new("lec-gold-ref.v", COUNTER);
    let (ok, output) = run_edu(&[
        "lec",
        src.path(),
        "--top",
        "counter8",
        "--reference",
        reference.path(),
        "--reference-top",
        "counter8",
    ]);
    assert!(ok, "{output}");
    assert!(output.contains("EQUIVALENT"), "{output}");
    assert!(output.contains("implementation:"), "{output}");
}

#[test]
fn lec_command_two_source_reports_counterexample() {
    let src = TempFile::new("lec-good.v", COUNTER);
    let reference = TempFile::new("lec-bug.v", &counter_bug());
    let (ok, output) = run_edu(&[
        "lec",
        src.path(),
        "--top",
        "counter8",
        "--reference",
        reference.path(),
        "--reference-top",
        "counter8",
    ]);
    assert!(ok, "{output}");
    assert!(output.contains("NOT EQUIVALENT"), "{output}");
    assert!(output.contains("counterexample:"), "{output}");
    assert!(output.contains("count["), "{output}");
}

#[test]
fn lec_reference_flag_requires_lec_command() {
    let src = TempFile::new("lec-misuse.v", COUNTER);
    let (ok, output) = run_edu(&[
        "synth",
        src.path(),
        "--top",
        "counter8",
        "--reference",
        src.path(),
    ]);
    assert!(!ok, "{output}");
    assert!(
        output.contains("--reference requires the lec command"),
        "{output}"
    );
}

#[test]
fn lec_command_accepts_builtin_and_kissat_solvers() {
    let src = TempFile::new("lec-solvers.v", COUNTER);
    for solver in ["native", "kissat"] {
        let (ok, output) = run_edu(&["lec", src.path(), "--top", "counter8", "--solver", solver]);
        assert!(ok, "{solver}: {output}");
        assert!(output.contains("EQUIVALENT"), "{solver}: {output}");
    }
}

#[test]
fn lec_command_accepts_external_solvers_when_installed() {
    let src = TempFile::new("lec-external.v", COUNTER);
    for solver in ["z3", "cvc5"] {
        let (ok, output) = run_edu(&["lec", src.path(), "--top", "counter8", "--solver", solver]);
        if !ok && output.contains("unavailable") {
            eprintln!("skipping {solver}: not installed");
            continue;
        }
        assert!(ok, "{solver}: {output}");
        assert!(output.contains("EQUIVALENT"), "{solver}: {output}");
    }
}

#[test]
fn lec_command_cross_check_reports_every_engine() {
    let src = TempFile::new("lec-cross.v", COUNTER);
    let (ok, output) = run_edu(&[
        "lec",
        src.path(),
        "--top",
        "counter8",
        "--cross-check",
        "kissat,native",
    ]);
    assert!(ok, "{output}");
    assert!(output.contains("EQUIVALENT"), "{output}");
    assert!(output.contains("kissat: equivalent"), "{output}");
    assert!(output.contains("native: equivalent"), "{output}");
}

#[test]
fn lec_rejects_solver_flag_on_non_lec_commands() {
    let src = TempFile::new("lec-solver-misuse.v", COUNTER);
    let (ok, output) = run_edu(&["synth", src.path(), "--top", "counter8", "--solver", "z3"]);
    assert!(!ok, "{output}");
    assert!(
        output.contains("--solver requires the lec or equiv command"),
        "{output}"
    );
}

#[test]
fn equiv_command_accepts_solver_flag() {
    let src = TempFile::new("equiv-solver.v", COUNTER);
    let (ok, output) = run_edu(&[
        "equiv",
        src.path(),
        "--top",
        "counter8",
        "--solver",
        "native",
    ]);
    assert!(ok, "{output}");
    assert!(
        output.contains("EQUIVALENT (sat-backend: native)"),
        "{output}"
    );
}

#[test]
fn lec_command_strict_mode_reports_no_internal_cuts() {
    let src = TempFile::new("lec-strict.v", COUNTER);
    let (ok, output) = run_edu(&["lec", src.path(), "--top", "counter8", "--strict"]);
    assert!(ok, "{output}");
    assert!(output.contains("strict LEC: no internal cuts"), "{output}");
    assert!(output.contains("proved all"), "{output}");
    assert!(output.contains("0 internal cuts"), "{output}");
    assert!(output.contains("EQUIVALENT"), "{output}");
}
