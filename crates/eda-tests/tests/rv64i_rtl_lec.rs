//! Full-circuit LEC for the RV64I core.
//!
//! Runs the same two independent obligations used for the RV32I/i8086 cores:
//! 1. RTL-derived reference versus standard-cell-mapped netlist.
//! 2. Independent slang frontend versus eda-hdl frontend.

use eda_core::library::CellLibrary;
use eda_slang::{PreprocessOptions, SlangFrontend};

const RV64I_CORE: &str = include_str!("../../../platform/rtl/rv64i_full_core.sv");

fn check_rtl_vs_mapped() {
    if cfg!(debug_assertions) {
        eprintln!("skipping RV64I LEC in debug builds");
        return;
    }
    // The localized-correspondence proof of the 168k-cell RV64IMA netlist can
    // hit a hard SAT obligation, so this regression uses the strict parallel
    // engine: every flop next-state function and output bit is proven over its
    // full combinational cone with no internal cuts.
    let result = eda_api::with_lec_strict(true, || {
        eda_api::check_rtl_equivalence(RV64I_CORE, Some("rv64i_full_core"))
    })
    .expect("RV64I RTL-vs-mapped strict LEC");
    assert!(
        result.equivalent,
        "RV64I RTL-vs-mapped LEC failed: notes={:?} cex={:?}",
        result.notes, result.counterexample
    );
    eprintln!(
        "rv64i_full_core: rtl-vs-mapped equivalent=true reference_cells={} mapped_cells={} notes={:?}",
        result.gold_stats.cells, result.mapped_stats.cells, result.notes
    );
}

fn check_independent_frontends() {
    if cfg!(debug_assertions) {
        eprintln!("skipping RV64I independent LEC in debug builds");
        return;
    }
    let Some(frontend) = SlangFrontend::discover_optional() else {
        eprintln!("skipping RV64I independent LEC: slang runtime not found");
        return;
    };
    let slang = frontend
        .compile_source(
            RV64I_CORE,
            Some("rv64i_full_core"),
            &PreprocessOptions::default(),
        )
        .unwrap_or_else(|error| panic!("RV64I slang lowering: {error}"));
    let hdl =
        eda_hdl::compile(RV64I_CORE, Some("rv64i_full_core")).expect("RV64I eda-hdl lowering");
    let reference = eda_synth::synthesize_rtl_reference(&slang).expect("slang reference");
    let mapped = eda_synth::synthesize_with_library(
        &hdl,
        &eda_synth::SynthOptions::default(),
        &CellLibrary::std(),
    )
    .expect("eda-hdl mapped")
    .netlist;
    let result = eda_formal::check_equivalent(&reference, &mapped).expect("RV64I independent LEC");
    assert!(
        result.equivalent,
        "RV64I independent slang-vs-hdl LEC failed: notes={:?} cex={:?}",
        result.notes, result.counterexample
    );
    eprintln!(
        "rv64i_full_core: independent slang-vs-hdl equivalent=true slang_cells={} hdl_cells={} notes={:?}",
        reference.stats().cells,
        mapped.stats().cells,
        result.notes
    );
}

#[test]
fn rv64i_full_core_rtl_vs_mapped_lec() {
    check_rtl_vs_mapped();
}

#[test]
fn rv64i_full_core_independent_frontend_lec() {
    check_independent_frontends();
}
