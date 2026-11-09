//! Strict full-circuit LEC probe: no internal cut points.
//!
//! Usage: `strict_full_lec_probe <rv32i_core|rv32i_full_core|i8086_full_core|rv32i_soc|i8086_soc>`
//! Environment: `EDA_LEC_BUDGET_SECS` watchdog (default 1800).

use eda_core::cancellation::FlowCancellation;
use eda_core::library::CellLibrary;

const RV_SIGNOFF: &str = include_str!("../../../rv32i/rtl/rv32i_core.v");
const RV_FULL: &str = include_str!("../../../platform/rtl/rv32i_full_core.sv");
const RV64I_FULL: &str = include_str!("../../../platform/rtl/rv64i_full_core.sv");
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

fn main() {
    let which = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "rv32i_core".into());
    let (source, top) = match which.as_str() {
        "rv32i_core" => (RV_SIGNOFF.to_string(), "rv32i_core"),
        "rv32i_full_core" => (RV_FULL.to_string(), "rv32i_full_core"),
        "rv64i_full_core" => (RV64I_FULL.to_string(), "rv64i_full_core"),
        "rv32i_soc" => (format!("{RV_FULL}\n{RV_SOC}"), "rv32i_full_soc_16"),
        "i8086_full_core" => (format!("{I8086_DECODERS}{I8086_FULL}"), "i8086_full_core"),
        "i8086_soc" => (
            format!("{I8086_DECODERS}{I8086_FULL}\n{I8086_SOC}"),
            "i8086_full_soc_64",
        ),
        "sys8086" => (CPU8086.to_string(), "sys8086"),
        other => panic!("unknown circuit '{other}'"),
    };
    let design = eda_hdl::compile(&source, Some(top)).expect("compile");
    let reference = eda_synth::synthesize_rtl_reference(&design).expect("reference");
    let mapped = eda_synth::synthesize_with_library(
        &design,
        &eda_synth::SynthOptions::default(),
        &CellLibrary::std(),
    )
    .expect("mapped")
    .netlist;
    let budget = std::env::var("EDA_LEC_BUDGET_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1800);
    let cancellation = FlowCancellation::new();
    let watchdog = {
        let cancellation = cancellation.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(budget));
            cancellation.cancel();
        })
    };
    let started = std::time::Instant::now();
    let result = eda_formal::with_lec_strict(true, || {
        eda_formal::check_equivalent_cancellable(&reference, &mapped, &cancellation)
    });
    watchdog.thread().unpark();
    match result {
        Ok(result) => {
            println!(
                "{which}: strict equivalent={} reference_cells={} mapped_cells={} elapsed={:.1}s notes={:?}",
                result.equivalent,
                reference.stats().cells,
                mapped.stats().cells,
                started.elapsed().as_secs_f64(),
                result.notes
            );
        }
        Err(error) => {
            println!(
                "{which}: strict LEC did not complete within {budget}s: {error} (elapsed {:.1}s)",
                started.elapsed().as_secs_f64()
            );
            std::process::exit(2);
        }
    }
}
