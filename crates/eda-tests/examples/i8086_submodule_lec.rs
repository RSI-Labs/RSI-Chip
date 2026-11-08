//! Independent slang-vs-hdl LEC for each i8086 submodule.

use eda_core::library::CellLibrary;
use eda_slang::{PreprocessOptions, SlangFrontend};

fn check(label: &str, source: &str, top: &str, backend: eda_formal::SatBackend) {
    let frontend = SlangFrontend::discover().expect("slang");
    let slang = frontend
        .compile_source(source, Some(top), &PreprocessOptions::default())
        .expect("slang");
    let hdl = eda_hdl::compile(source, Some(top)).expect("hdl");
    let slang_ref = eda_synth::synthesize_rtl_reference(&slang).expect("slang ref");
    let hdl_map = eda_synth::synthesize_with_library(
        &hdl,
        &eda_synth::SynthOptions::default(),
        &CellLibrary::std(),
    )
    .expect("hdl map")
    .netlist;
    let result = eda_formal::check_equivalent_with_backend(&slang_ref, &hdl_map, backend)
        .unwrap_or_else(|error| panic!("{label} LEC with {}: {error}", backend.as_str()));
    println!(
        "{label}: backend={} equivalent={} slang_cells={} hdl_cells={} notes={:?}",
        backend.as_str(),
        result.equivalent,
        slang_ref.stats().cells,
        hdl_map.stats().cells,
        result.notes
    );
}

fn main() {
    let backend = std::env::args()
        .nth(1)
        .as_deref()
        .and_then(eda_formal::SatBackend::parse)
        .unwrap_or(eda_formal::SatBackend::Kissat);
    let prefix = include_str!("../../../platform/rtl/i8086_prefix_decoder.sv");
    let opcode = include_str!("../../../platform/rtl/i8086_opcode_decoder.sv");
    let modrm = include_str!("../../../platform/rtl/i8086_modrm_decoder.sv");
    check("prefix_decoder", prefix, "i8086_prefix_decoder", backend);
    check("opcode_decoder", opcode, "i8086_opcode_decoder", backend);
    check("modrm_decoder", modrm, "i8086_modrm_decoder", backend);
}
