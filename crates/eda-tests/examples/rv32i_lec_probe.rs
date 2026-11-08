//! RV32I full-core and signoff-core LEC probe.
//!
//! Reports RTL-vs-mapped equivalence plus an independent slang-vs-eda-hdl
//! full-core equivalence check, with exact netlist sizes so the "10k-gate"
//! scope is explicit.

use eda_core::library::CellLibrary;
use eda_slang::{PreprocessOptions, SlangFrontend};

const FULL_CORE: &str = include_str!("../../../platform/rtl/rv32i_full_core.sv");
const SIGNOFF_CORE: &str = include_str!("../../../rv32i/rtl/rv32i_core.v");

fn main() {
    for (label, source, top) in [
        ("rv32i_full_core", FULL_CORE, "rv32i_full_core"),
        ("rv32i_core", SIGNOFF_CORE, "rv32i_core"),
    ] {
        let result = eda_api::check_rtl_equivalence(source, Some(top))
            .unwrap_or_else(|error| panic!("{label} RTL-vs-mapped LEC: {error}"));
        println!(
            "{label}: rtl-vs-mapped equivalent={} reference_cells={} mapped_cells={} notes={:?}",
            result.equivalent, result.gold_stats.cells, result.mapped_stats.cells, result.notes
        );

        let frontend = SlangFrontend::discover().expect("slang runtime");
        let slang = match frontend.compile_source(source, Some(top), &PreprocessOptions::default())
        {
            Ok(design) => design,
            Err(error) => {
                println!("{label}: independent slang lowering unsupported: {error}");
                continue;
            }
        };
        let hdl = eda_hdl::compile(source, Some(top)).expect("eda-hdl lowering");
        let reference = eda_synth::synthesize_rtl_reference(&slang).expect("slang reference");
        let mapped = eda_synth::synthesize_with_library(
            &hdl,
            &eda_synth::SynthOptions::default(),
            &CellLibrary::std(),
        )
        .expect("eda-hdl mapped")
        .netlist;
        let independent = eda_formal::check_equivalent(&reference, &mapped)
            .unwrap_or_else(|error| panic!("{label} independent LEC: {error}"));
        println!(
            "{label}: independent slang-vs-hdl equivalent={} slang_cells={} hdl_cells={} notes={:?}",
            independent.equivalent,
            reference.stats().cells,
            mapped.stats().cells,
            independent.notes
        );
    }
}
