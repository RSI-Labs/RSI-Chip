//! Full-circuit LEC for every synthesizable 8086 and RV32I circuit.
//!
//! For each circuit this runs:
//! 1. RTL-derived reference vs standard-cell-mapped netlist LEC.
//! 2. Independent slang-vs-eda-hdl LEC when the slang runtime is available.
//!
//! Testbenches (`*_tb*`, `*_runtime_tb*`) are intentionally excluded: they
//! contain clocks/initial blocks and are not synthesizable circuits.
//!
//! Run in release: the largest SoC proof is ~110k mapped gates.

use eda_core::library::CellLibrary;
use eda_slang::{PreprocessOptions, SlangFrontend};

const RV_ISA_DECODER: &str = include_str!("../../../platform/rtl/rv32i_isa_decoder.sv");
const RV_FULL_CORE: &str = include_str!("../../../platform/rtl/rv32i_full_core.sv");
const RV_SOC: &str = include_str!("../../../platform/rtl/rv32i_full_soc_16.sv");
const RV_SIGNOFF_CORE: &str = include_str!("../../../rv32i/rtl/rv32i_core.v");
const I8086_PREFIX: &str = include_str!("../../../platform/rtl/i8086_prefix_decoder.sv");
const I8086_OPCODE: &str = include_str!("../../../platform/rtl/i8086_opcode_decoder.sv");
const I8086_MODRM: &str = include_str!("../../../platform/rtl/i8086_modrm_decoder.sv");
const I8086_FULL_CORE: &str = include_str!("../../../platform/rtl/i8086_full_core.sv");
const I8086_SOC: &str = include_str!("../../../platform/rtl/i8086_full_soc_64.sv");
const CPU8086_PARTS: &str = concat!(
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
);
const CPU8086_SYS: &str = include_str!("../../../cpu8086/rtl/sys8086.v");

fn check(label: &str, source: &str, top: &str) {
    let result = eda_api::check_rtl_equivalence(source, Some(top))
        .unwrap_or_else(|error| panic!("{label} RTL-vs-mapped LEC: {error}"));
    assert!(
        result.equivalent,
        "{label} RTL-vs-mapped LEC failed: notes={:?} cex={:?}",
        result.notes, result.counterexample
    );
    eprintln!(
        "{label}: rtl-vs-mapped equivalent=true reference_cells={} mapped_cells={} notes={:?}",
        result.gold_stats.cells, result.mapped_stats.cells, result.notes
    );

    let Some(frontend) = SlangFrontend::discover_optional() else {
        eprintln!("{label}: slang unavailable; skipping independent frontend LEC");
        return;
    };
    let slang = match frontend.compile_source(source, Some(top), &PreprocessOptions::default()) {
        Ok(design) => design,
        Err(error) => panic!("{label} independent slang lowering: {error}"),
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
    assert!(
        independent.equivalent,
        "{label} independent LEC failed: notes={:?} cex={:?}",
        independent.notes, independent.counterexample
    );
    eprintln!(
        "{label}: independent slang-vs-hdl equivalent=true slang_cells={} hdl_cells={} notes={:?}",
        reference.stats().cells,
        mapped.stats().cells,
        independent.notes
    );
}

#[test]
fn all_synthesizable_8086_and_rv32i_circuits_lec() {
    if cfg!(debug_assertions) {
        eprintln!("skipping full-circuit LEC in debug builds");
        return;
    }
    check("rv32i_isa_decoder", RV_ISA_DECODER, "rv32i_isa_decoder");
    check("rv32i_full_core", RV_FULL_CORE, "rv32i_full_core");
    check("rv32i_core", RV_SIGNOFF_CORE, "rv32i_core");
    check(
        "rv32i_full_soc_16",
        &format!("{RV_FULL_CORE}\n{RV_SOC}"),
        "rv32i_full_soc_16",
    );

    check("i8086_prefix_decoder", I8086_PREFIX, "i8086_prefix_decoder");
    check("i8086_opcode_decoder", I8086_OPCODE, "i8086_opcode_decoder");
    check("i8086_modrm_decoder", I8086_MODRM, "i8086_modrm_decoder");
    let i8086_core = format!("{I8086_PREFIX}\n{I8086_OPCODE}\n{I8086_MODRM}\n{I8086_FULL_CORE}");
    check("i8086_full_core", &i8086_core, "i8086_full_core");
    check(
        "i8086_full_soc_64",
        &format!("{i8086_core}\n{I8086_SOC}"),
        "i8086_full_soc_64",
    );

    check("cpu8086", CPU8086_PARTS, "cpu8086");
    check(
        "sys8086",
        &format!("{CPU8086_PARTS}\n{CPU8086_SYS}"),
        "sys8086",
    );
}

#[test]
fn strict_no_cut_lec_on_cpu8086_system() {
    if cfg!(debug_assertions) {
        eprintln!("skipping strict LEC in debug builds");
        return;
    }
    let source = format!("{CPU8086_PARTS}\n{CPU8086_SYS}");
    let design = eda_api::compile(&source, Some("sys8086")).expect("compile sys8086");
    let reference = eda_synth::synthesize_rtl_reference(&design).expect("reference");
    let mapped = eda_synth::synthesize_with_library(
        &design,
        &eda_synth::SynthOptions::default(),
        &CellLibrary::std(),
    )
    .expect("mapped")
    .netlist;
    let result =
        eda_api::with_lec_strict(true, || eda_formal::check_equivalent(&reference, &mapped))
            .expect("strict LEC");
    assert!(result.equivalent, "{:?}", result.notes);
    assert!(result
        .notes
        .iter()
        .any(|note| note.contains("0 internal cuts")));
    assert!(result.notes.iter().any(|note| note.contains("proved all")));
    eprintln!("strict sys8086 notes: {:?}", result.notes);
}
