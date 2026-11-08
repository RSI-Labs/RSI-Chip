//! Independent frontend differential LEC for the full i8086 core.
//!
//! Lowers the same RTL through the independent slang frontend and the
//! in-house eda-hdl frontend, synthesizes both with the generic RTL
//! reference path, and proves the resulting netlists equivalent. This is a
//! common-mode check on the toolchain: a bug in one lowering cannot hide
//! behind the same lowering being used on both sides.
//!
//! Skipped when slang is not installed (`SLANG_BIN` or `PATH`), and skipped
//! in debug builds where the 240k-cell SAT proof is impractically slow.

use eda_core::library::CellLibrary;
use eda_slang::{PreprocessOptions, SlangFrontend};

const SOURCE: &str = concat!(
    include_str!("../../../platform/rtl/i8086_prefix_decoder.sv"),
    "\n",
    include_str!("../../../platform/rtl/i8086_opcode_decoder.sv"),
    "\n",
    include_str!("../../../platform/rtl/i8086_modrm_decoder.sv"),
    "\n",
    include_str!("../../../platform/rtl/i8086_full_core.sv"),
);

#[test]
fn i8086_full_core_independent_slang_vs_eda_hdl_lec() {
    if cfg!(debug_assertions) {
        eprintln!("skipping: run this proof in release");
        return;
    }
    let Some(frontend) = SlangFrontend::discover_optional() else {
        eprintln!("skipping: slang runtime not found");
        return;
    };
    let slang = frontend
        .compile_source(
            SOURCE,
            Some("i8086_full_core"),
            &PreprocessOptions::default(),
        )
        .expect("slang lowering");
    let hdl = eda_hdl::compile(SOURCE, Some("i8086_full_core")).expect("eda-hdl lowering");

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
        "independent slang-vs-eda-hdl LEC failed: notes={:?} counterexample={:?}",
        result.notes, result.counterexample
    );
}
