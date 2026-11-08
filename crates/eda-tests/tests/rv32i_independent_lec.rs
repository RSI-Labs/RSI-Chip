//! Independent frontend differential LEC for the RV32I full cores.
//!
//! Compares a slang-lowered RTL reference against an eda-hdl-lowered,
//! standard-cell-mapped netlist for both RV32I implementations:
//! - `platform/rtl/rv32i_full_core.sv` (platform/matrix core)
//! - `rv32i/rtl/rv32i_core.v` (PDK signoff core)
//!
//! Skipped when slang is unavailable, and in debug builds where the ~31k-37k
//! cell SAT proofs are impractically slow.

use eda_core::library::CellLibrary;
use eda_slang::{PreprocessOptions, SlangFrontend};

const FULL_CORE: &str = include_str!("../../../platform/rtl/rv32i_full_core.sv");
const SIGNOFF_CORE: &str = include_str!("../../../rv32i/rtl/rv32i_core.v");

fn check(label: &str, source: &str, top: &str) {
    if cfg!(debug_assertions) {
        eprintln!("skipping {label}: run this proof in release");
        return;
    }
    let Some(frontend) = SlangFrontend::discover_optional() else {
        eprintln!("skipping {label}: slang runtime not found");
        return;
    };
    let slang = frontend
        .compile_source(source, Some(top), &PreprocessOptions::default())
        .unwrap_or_else(|error| panic!("{label} slang lowering: {error}"));
    let hdl = eda_hdl::compile(source, Some(top)).expect("eda-hdl lowering");
    let reference = eda_synth::synthesize_rtl_reference(&slang).expect("slang reference synthesis");
    let mapped = eda_synth::synthesize_with_library(
        &hdl,
        &eda_synth::SynthOptions::default(),
        &CellLibrary::std(),
    )
    .expect("eda-hdl mapped synthesis")
    .netlist;
    let result = eda_formal::check_equivalent(&reference, &mapped).expect("LEC");
    assert!(
        result.equivalent,
        "{label} independent slang-vs-eda-hdl LEC failed: notes={:?} counterexample={:?}",
        result.notes, result.counterexample
    );
    eprintln!(
        "{label}: equivalent=true slang_cells={} hdl_cells={} notes={:?}",
        reference.stats().cells,
        mapped.stats().cells,
        result.notes
    );
}

#[test]
fn rv32i_full_core_independent_slang_vs_eda_hdl_lec() {
    check("rv32i_full_core", FULL_CORE, "rv32i_full_core");
}

#[test]
fn rv32i_signoff_core_independent_slang_vs_eda_hdl_lec() {
    check("rv32i_core", SIGNOFF_CORE, "rv32i_core");
}
