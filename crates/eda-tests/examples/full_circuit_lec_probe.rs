//! Full-circuit LEC probe: SoCs and the cpu8086 subset system.
//!
//! Runs RTL-vs-mapped LEC and, when slang can lower the source, an independent
//! slang-vs-eda-hdl LEC. Every proof obligation (all flop next-state functions
//! and all output bits) is reported by the LEC engine notes.

use eda_core::library::CellLibrary;
use eda_slang::{PreprocessOptions, SlangFrontend};

const RV_FULL: &str = include_str!("../../../platform/rtl/rv32i_full_core.sv");
const RV_SOC: &str = include_str!("../../../platform/rtl/rv32i_full_soc_16.sv");
const I8086_DECODERS: &str = concat!(
    include_str!("../../../platform/rtl/i8086_prefix_decoder.sv"),
    "\n",
    include_str!("../../../platform/rtl/i8086_opcode_decoder.sv"),
    "\n",
    include_str!("../../../platform/rtl/i8086_modrm_decoder.sv"),
    "\n",
);
const I8086_FULL: &str = include_str!("../../../platform/rtl/i8086_full_core.sv");
const I8086_SOC: &str = include_str!("../../../platform/rtl/i8086_full_soc_64.sv");
const CPU8086: &str = concat!(
    include_str!("../../../cpu8086/rtl/primitives.v"),
    "\n",
    include_str!("../../../cpu8086/rtl/ripple_adder16.v"),
    "\n",
    include_str!("../../../cpu8086/rtl/alu16.v"),
    "\n",
    include_str!("../../../cpu8086/rtl/regfile16.v"),
    "\n",
    include_str!("../../../cpu8086/rtl/ram64x16.v"),
    "\n",
    include_str!("../../../cpu8086/rtl/cpu8086.v"),
    "\n",
    include_str!("../../../cpu8086/rtl/sys8086.v"),
);

fn check(label: &str, source: &str, top: &str) {
    let result = eda_api::check_rtl_equivalence(source, Some(top))
        .unwrap_or_else(|error| panic!("{label} RTL-vs-mapped LEC: {error}"));
    println!(
        "{label}: rtl-vs-mapped equivalent={} reference_cells={} mapped_cells={} notes={:?}",
        result.equivalent, result.gold_stats.cells, result.mapped_stats.cells, result.notes
    );
    assert!(result.equivalent, "{label} RTL-vs-mapped LEC failed");

    let Some(frontend) = SlangFrontend::discover_optional() else {
        println!("{label}: slang unavailable; skipping independent frontend LEC");
        return;
    };
    let slang = match frontend.compile_source(source, Some(top), &PreprocessOptions::default()) {
        Ok(design) => design,
        Err(error) => {
            println!("{label}: independent slang lowering unsupported: {error}");
            return;
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
    assert!(independent.equivalent, "{label} independent LEC failed");
}

fn main() {
    let selection = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    let run = |name: &str| selection == "all" || selection == name;
    if run("rv32i_full_soc_16") {
        check(
            "rv32i_full_soc_16",
            &format!("{RV_FULL}\n{RV_SOC}"),
            "rv32i_full_soc_16",
        );
    }
    if run("i8086_full_soc_64") {
        check(
            "i8086_full_soc_64",
            &format!("{I8086_DECODERS}{I8086_FULL}\n{I8086_SOC}"),
            "i8086_full_soc_64",
        );
    }
    if run("sys8086") {
        check("sys8086", CPU8086, "sys8086");
    }
}
