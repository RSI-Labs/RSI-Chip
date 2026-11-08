//! Independent frontend LEC probe: slang and eda-hdl lower the same i8086 RTL.

use eda_core::library::CellLibrary;
use eda_slang::{PreprocessOptions, SlangFrontend};

fn main() {
    let source = format!(
        "{}\n{}\n{}{}",
        include_str!("../../../platform/rtl/i8086_prefix_decoder.sv"),
        include_str!("../../../platform/rtl/i8086_opcode_decoder.sv"),
        include_str!("../../../platform/rtl/i8086_modrm_decoder.sv"),
        include_str!("../../../platform/rtl/i8086_full_core.sv"),
    );
    let frontend = SlangFrontend::discover().expect("discover slang");
    let slang_design = frontend
        .compile_source(
            &source,
            Some("i8086_full_core"),
            &PreprocessOptions::default(),
        )
        .expect("slang direct lowering");
    let hdl_design = eda_hdl::compile(&source, Some("i8086_full_core")).expect("eda-hdl lowering");

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
