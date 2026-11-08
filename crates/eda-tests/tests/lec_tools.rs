//! LEC tooling regression: two-source RTL LEC, RTL-vs-external-netlist LEC,
//! and direct `Design`-to-`Design` equivalence, including negative cases.

const COUNTER: &str = include_str!("../../../platform/rtl/blocks/counter8.sv");
const I8086_PREFIX: &str = include_str!("../../../platform/rtl/i8086_prefix_decoder.sv");
const I8086_OPCODE: &str = include_str!("../../../platform/rtl/i8086_opcode_decoder.sv");
const I8086_MODRM: &str = include_str!("../../../platform/rtl/i8086_modrm_decoder.sv");
const SHIFT: &str = include_str!("../../../platform/rtl/blocks/shift_register32.sv");
const ALU: &str = include_str!("../../../platform/rtl/blocks/alu8.sv");
const UART: &str = include_str!("../../../platform/rtl/blocks/uart_tx.sv");
const SPI: &str = include_str!("../../../platform/rtl/blocks/spi_master.sv");
const PWM: &str = include_str!("../../../platform/rtl/blocks/pwm.sv");
const LFSR: &str = include_str!("../../../platform/rtl/blocks/lfsr8.sv");

// Same behavior as counter8, written with a case statement and an explicit
// increment expression: a refactor that must remain formally equivalent.
const COUNTER_REFACTOR: &str = r#"
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
    assign zero  = ~(|count);
    assign carry = en & (up ? &count : ~|count);

    always @(posedge clk) begin
        if (rst) begin
            count <= 8'd0;
        end else if (load) begin
            count <= load_value;
        end else if (en) begin
            case (up)
                1'b1: count <= count + 8'd1;
                default: count <= count - 8'd1;
            endcase
        end
    end
endmodule
"#;

// Deliberately wrong: increments by two. LEC must find a counterexample.
const COUNTER_BUG: &str = r#"
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
            count <= up ? (count + 8'd2) : (count - 8'd2);
    end
endmodule
"#;

#[test]
fn lec_rtl_vs_mapped_counter_is_equivalent() {
    let result = eda_api::check_rtl_equivalence(COUNTER, Some("counter8")).expect("LEC");
    assert!(
        result.equivalent,
        "RTL-vs-mapped LEC failed: notes={:?} cex={:?}",
        result.notes, result.counterexample
    );
    assert!(result.gold_stats.cells > 0);
    assert!(result.mapped_stats.cells > 0);
}

#[test]
fn lec_two_rtl_sources_refactor_is_equivalent() {
    let result = eda_api::check_rtl_equivalence_between(
        COUNTER,
        Some("counter8"),
        COUNTER_REFACTOR,
        Some("counter8"),
    )
    .expect("two-source LEC");
    assert!(
        result.equivalent,
        "refactor mistakenly reported non-equivalent: notes={:?} cex={:?}",
        result.notes, result.counterexample
    );
    assert!(result.reference_stats.cells > 0);
    assert!(result.implementation_stats.cells > 0);
}

#[test]
fn lec_two_rtl_sources_detects_increment_by_two_bug() {
    let result = eda_api::check_rtl_equivalence_between(
        COUNTER,
        Some("counter8"),
        COUNTER_BUG,
        Some("counter8"),
    )
    .expect("two-source LEC");
    assert!(
        !result.equivalent,
        "LEC failed to detect the increment-by-two bug"
    );
    assert!(
        !result.counterexample.is_empty(),
        "non-equivalence must include a counterexample: notes={:?}",
        result.notes
    );
}

#[test]
fn lec_external_netlist_against_rtl_reports_difference() {
    // Map the buggy counter through the normal flow, then prove the good RTL
    // against that external (mapped) netlist.
    let implementation = eda_api::run_flow(COUNTER_BUG, Some("counter8"))
        .expect("flow")
        .netlist;
    let result =
        eda_api::check_rtl_vs_netlist_equivalence(COUNTER, Some("counter8"), &implementation)
            .expect("RTL-vs-netlist LEC");
    assert!(
        !result.equivalent,
        "RTL-vs-netlist LEC failed to detect the mapped bug"
    );
    assert!(!result.counterexample.is_empty());
}

#[test]
fn lec_external_netlist_against_rtl_accepts_correct_netlist() {
    let implementation = eda_api::run_flow(COUNTER, Some("counter8"))
        .expect("flow")
        .netlist;
    let result =
        eda_api::check_rtl_vs_netlist_equivalence(COUNTER, Some("counter8"), &implementation)
            .expect("RTL-vs-netlist LEC");
    assert!(
        result.equivalent,
        "correct mapped netlist reported non-equivalent: notes={:?} cex={:?}",
        result.notes, result.counterexample
    );
}

#[test]
fn lec_design_to_design_api_is_usable_by_external_frontends() {
    let reference = eda_api::compile(COUNTER, Some("counter8")).expect("compile reference");
    let implementation =
        eda_api::compile(COUNTER_REFACTOR, Some("counter8")).expect("compile implementation");
    let result = eda_api::check_design_equivalence(&reference, &implementation).expect("LEC");
    assert!(
        result.equivalent,
        "Design-to-Design LEC reported non-equivalent: notes={:?} cex={:?}",
        result.notes, result.counterexample
    );
}

#[test]
fn lec_design_to_design_detects_difference() {
    let reference = eda_api::compile(COUNTER, Some("counter8")).expect("compile reference");
    let implementation = eda_api::compile(COUNTER_BUG, Some("counter8")).expect("compile bug");
    let result = eda_api::check_design_equivalence(&reference, &implementation).expect("LEC");
    assert!(!result.equivalent, "Design-to-Design LEC missed the bug");
    assert!(!result.counterexample.is_empty());
}

#[test]
fn lec_refactored_mapped_netlist_matches_golden_rtl() {
    // Map the refactored RTL through the normal standard-cell flow and prove
    // it against the golden RTL reference: crosses both the source rewrite and
    // the synthesis/mapping step.
    let implementation = eda_api::run_flow(COUNTER_REFACTOR, Some("counter8"))
        .expect("flow")
        .netlist;
    let result =
        eda_api::check_rtl_vs_netlist_equivalence(COUNTER, Some("counter8"), &implementation)
            .expect("RTL-vs-mapped LEC");
    assert!(
        result.equivalent,
        "refactored mapped netlist reported non-equivalent: notes={:?} cex={:?}",
        result.notes, result.counterexample
    );
}

fn available_solvers() -> Vec<eda_api::SatBackend> {
    [
        eda_api::SatBackend::Native,
        eda_api::SatBackend::Kissat,
        eda_api::SatBackend::Z3,
        eda_api::SatBackend::Cvc5,
    ]
    .into_iter()
    .filter(|backend| backend.is_available())
    .collect()
}

#[test]
fn lec_each_available_solver_proves_and_refutes_counter() {
    let solvers = available_solvers();
    assert!(solvers.contains(&eda_api::SatBackend::Kissat));
    for solver in solvers {
        let equivalent =
            eda_api::check_rtl_equivalence_with_solver(COUNTER, Some("counter8"), solver)
                .unwrap_or_else(|error| panic!("{}: {error}", solver.as_str()));
        assert!(
            equivalent.equivalent,
            "{} reported the counter as non-equivalent: {:?}",
            solver.as_str(),
            equivalent.notes
        );
        let distinguished = eda_api::check_rtl_equivalence_between_with_solver(
            COUNTER,
            Some("counter8"),
            COUNTER_BUG,
            Some("counter8"),
            solver,
        )
        .unwrap_or_else(|error| panic!("{}: {error}", solver.as_str()));
        assert!(
            !distinguished.equivalent,
            "{} missed the increment-by-two bug",
            solver.as_str()
        );
        assert!(
            !distinguished.counterexample.is_empty(),
            "{} produced no counterexample",
            solver.as_str()
        );
        eprintln!(
            "{}: equivalent + non-equivalent counterexample OK",
            solver.as_str()
        );
    }
}

#[test]
fn lec_cross_check_requires_engine_agreement() {
    let solvers = available_solvers();
    let result = eda_api::check_rtl_equivalence_cross_checked(COUNTER, Some("counter8"), &solvers)
        .expect("cross-checked LEC");
    assert!(result.equivalent, "{:?}", result.notes);
    for solver in &solvers {
        assert!(
            result
                .notes
                .iter()
                .any(|note| note.starts_with(solver.as_str())),
            "cross-check report is missing {}: {:?}",
            solver.as_str(),
            result.notes
        );
    }
    eprintln!("cross-checked LEC engines: {:?}", solvers);
}

#[test]
fn all_reference_blocks_lec_agree_across_available_solvers() {
    if cfg!(debug_assertions) {
        eprintln!("skipping multi-solver block LEC in debug builds");
        return;
    }
    let blocks = [
        ("counter8", COUNTER),
        ("shift_register32", SHIFT),
        ("alu8", ALU),
        ("uart_tx", UART),
        ("spi_master", SPI),
        ("pwm", PWM),
        ("lfsr8", LFSR),
    ];
    let solvers = available_solvers();
    for (top, source) in blocks {
        for &solver in &solvers {
            let result = eda_api::check_rtl_equivalence_with_solver(source, Some(top), solver)
                .unwrap_or_else(|error| panic!("{top} with {}: {error}", solver.as_str()));
            assert!(
                result.equivalent,
                "{top} with {}: {:?}",
                solver.as_str(),
                result.notes
            );
        }
        eprintln!(
            "{top}: equivalent under {:?}",
            solvers.iter().map(|s| s.as_str()).collect::<Vec<_>>()
        );
    }
}

#[test]
fn i8086_decoders_lec_agree_across_available_solvers() {
    if cfg!(debug_assertions) {
        eprintln!("skipping multi-solver i8086 decoder LEC in debug builds");
        return;
    }
    let solvers = available_solvers();
    for (top, source) in [
        ("i8086_prefix_decoder", I8086_PREFIX),
        ("i8086_opcode_decoder", I8086_OPCODE),
        ("i8086_modrm_decoder", I8086_MODRM),
    ] {
        let design = eda_api::compile(source, Some(top)).expect("compile decoder");
        let reference = eda_synth::synthesize_rtl_reference(&design).expect("RTL reference");
        let mapped = eda_synth::synthesize_with_library(
            &design,
            &eda_synth::SynthOptions::default(),
            &eda_core::library::CellLibrary::std(),
        )
        .expect("mapped decoder")
        .netlist;
        let result = eda_api::check_netlists_cross_checked(&reference, &mapped, &solvers)
            .unwrap_or_else(|error| panic!("{top}: {error}"));
        assert!(result.equivalent, "{top}: {:?}", result.notes);
        eprintln!(
            "{top}: equivalent under {:?} ({:?})",
            solvers.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            result.notes
        );
    }
}
