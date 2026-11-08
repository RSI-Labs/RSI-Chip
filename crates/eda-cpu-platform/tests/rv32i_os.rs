//! End-to-end MiniOS-RV32I regression on the event-driven RTL simulator.
//!
//! Runs `platform/os/rv32i/minios.bin` (built from start.S + trap.S +
//! kernel.c + user.S) on the SystemVerilog RV32I core with the platform
//! runner's memory-mapped console/shutdown devices.

use eda_cpu_platform::systemverilog::Rv32iPlatformRunner;
use eda_cpu_platform::SparseMemory;

const MINIOS: &[u8] = include_bytes!("../../../platform/os/rv32i/minios.bin");

#[test]
fn minios_boots_and_runs_the_user_asm_program() {
    if cfg!(debug_assertions) {
        eprintln!("skipping MiniOS RTL run in debug builds");
        return;
    }
    let result = eda_cpu_platform::systemverilog::run_rv32i_platform(MINIOS, 200_000)
        .expect("run MiniOS on RV32I RTL");
    let text = String::from_utf8_lossy(&result.console);
    assert!(
        text.contains("MiniOS-RV32I: boot"),
        "missing boot banner: {text:?}"
    );
    assert!(
        text.contains("MiniOS-RV32I: console ready"),
        "missing console banner: {text:?}"
    );
    assert!(
        text.contains("MiniOS-RV32I: launching user asm program at 0x1000"),
        "missing launch banner: {text:?}"
    );
    assert!(
        text.contains("user-asm: hello from the RV32I user program"),
        "user asm banner missing: {text:?}"
    );
    assert!(
        text.contains("user-asm: sum(1..10) = 55"),
        "user asm computation missing: {text:?}"
    );
    assert!(
        text.contains("MiniOS: shutdown code 0"),
        "clean shutdown missing: {text:?}"
    );
    assert_eq!(result.exit_code, Some(0), "OS exit code");
    assert!(result.instructions > 0, "no retired instructions");
    assert!(result.cycles > 0, "no simulated cycles");
    assert!(
        !result.halted,
        "MiniOS should stop through the shutdown MMIO"
    );
}

#[test]
fn minios_run_can_emit_a_vcd_waveform() {
    if cfg!(debug_assertions) {
        eprintln!("skipping MiniOS VCD run in debug builds");
        return;
    }
    let mut memory = SparseMemory::new();
    memory.write_slice(0, MINIOS);
    let result = Rv32iPlatformRunner::new()
        .expect("compile RV32I platform")
        .run_binary(0, memory, 200_000, true)
        .expect("run MiniOS with VCD");
    assert_eq!(result.exit_code, Some(0));
    let vcd = result.vcd.expect("VCD requested");
    assert!(vcd.contains("$enddefinitions"), "malformed VCD");
    assert!(vcd.contains("$var"), "VCD has no variables");
    assert!(vcd.len() > 1024, "VCD unexpectedly small");
}
