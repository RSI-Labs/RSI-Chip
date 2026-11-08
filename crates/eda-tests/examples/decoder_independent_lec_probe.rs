//! Independent frontend LEC on the i8086 ModRM decoder.

use eda_core::library::CellLibrary;
use eda_slang::{PreprocessOptions, SlangFrontend};

fn main() {
    let source = include_str!("../../../platform/rtl/i8086_modrm_decoder.sv");
    let frontend = SlangFrontend::discover().expect("discover slang");
    let slang_design = frontend
        .compile_source(
            source,
            Some("i8086_modrm_decoder"),
            &PreprocessOptions::default(),
        )
        .expect("slang direct lowering");
    let hdl_design =
        eda_hdl::compile(source, Some("i8086_modrm_decoder")).expect("eda-hdl lowering");

    let reference = eda_synth::synthesize_rtl_reference(&slang_design).expect("slang reference");
    let mapped = eda_synth::synthesize_with_library(
        &hdl_design,
        &eda_synth::SynthOptions::default(),
        &CellLibrary::std(),
    )
    .expect("eda-hdl mapped netlist")
    .netlist;

    let result = eda_formal::check_equivalent(&reference, &mapped).expect("LEC");
    println!(
        "equivalent={} slang_cells={} hdl_cells={} notes={:?}",
        result.equivalent,
        reference.stats().cells,
        mapped.stats().cells,
        result.notes
    );
}
