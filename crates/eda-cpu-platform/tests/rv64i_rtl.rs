//! RV64I RTL platform tests: run the committed conformance images on
//! `platform/rtl/rv64i_full_core.sv` and compare against the RV64G ISS.
//!
//! The RTL process is the architectural executor; the host supplies sparse
//! memory over the external buses and services the same MMIO console and
//! shutdown addresses as the RV32I platform.

use eda_cpu_platform::rv64g::{Rv64Stop, Rv64gMachine};
use eda_cpu_platform::systemverilog::Rv64iPlatformRunner;

const RV64I: &[u8] = include_bytes!("../../../platform/programs/rv64g/rv64i.bin");
const RV64CSR: &[u8] = include_bytes!("../../../platform/programs/rv64g/rv64csr.bin");
const RV64M: &[u8] = include_bytes!("../../../platform/programs/rv64g/rv64m.bin");
const RV64A: &[u8] = include_bytes!("../../../platform/programs/rv64g/rv64a.bin");
const RV64F_OPS: &[u8] = include_bytes!("../../../platform/programs/rv64g/rv64f_ops.bin");
const RV64EXIT: &[u8] = include_bytes!("../../../platform/programs/rv64g/rv64exit.bin");

fn run_iss(image: &[u8]) -> Rv64gMachine {
    let mut machine = Rv64gMachine::load_flat(0, image);
    let stop = machine.run(200_000);
    assert_eq!(stop, Rv64Stop::Exit(0), "ISS did not exit cleanly");
    machine
}

#[test]
fn rv64i_integer_image_matches_iss() {
    if cfg!(debug_assertions) {
        eprintln!("skipping RV64I RTL run in debug builds");
        return;
    }
    let rtl = eda_cpu_platform::systemverilog::run_rv64i_platform(RV64I, 2_000_000)
        .expect("RV64I RTL run");
    assert_eq!(rtl.exit_code, Some(0), "RTL exit code");
    assert!(!rtl.trap, "RTL reported a fatal trap");
    assert!(!rtl.halted, "RTL halted without an MMIO shutdown");
    assert!(rtl.instructions > 0 && rtl.cycles > 0);
    let iss = run_iss(RV64I);
    assert_eq!(rtl.regs, iss.regs, "RTL/ISS register mismatch");
    assert_eq!(rtl.console, iss.output, "RTL/ISS console mismatch");
}

#[test]
fn rv64csr_image_matches_iss() {
    if cfg!(debug_assertions) {
        eprintln!("skipping RV64 CSR RTL run in debug builds");
        return;
    }
    let rtl = eda_cpu_platform::systemverilog::run_rv64i_platform(RV64CSR, 2_000_000)
        .expect("RV64 CSR RTL run");
    assert_eq!(rtl.exit_code, Some(0), "RTL exit code");
    assert!(!rtl.trap, "RTL reported a fatal trap");
    let iss = run_iss(RV64CSR);
    assert_eq!(rtl.regs, iss.regs, "RTL/ISS register mismatch");
}

#[test]
fn rv64m_multiply_divide_image_matches_iss() {
    if cfg!(debug_assertions) {
        eprintln!("skipping RV64M RTL run in debug builds");
        return;
    }
    let rtl =
        eda_cpu_platform::systemverilog::run_rv64i_platform(RV64M, 200_000).expect("RV64M RTL run");
    assert_eq!(rtl.exit_code, Some(0), "RTL exit code");
    assert!(!rtl.trap, "RTL reported a fatal trap");
    assert!(
        rtl.cycles > 64,
        "M instructions should take multiple cycles"
    );
    let mut iss = Rv64gMachine::load_flat(0, RV64M);
    assert_eq!(iss.run(200_000), Rv64Stop::Exit(0));
    assert_eq!(rtl.regs, iss.regs, "RV64M RTL/ISS register mismatch");
}

#[test]
fn rv64a_atomic_image_matches_iss() {
    if cfg!(debug_assertions) {
        eprintln!("skipping RV64A RTL run in debug builds");
        return;
    }
    let rtl =
        eda_cpu_platform::systemverilog::run_rv64i_platform(RV64A, 200_000).expect("RV64A RTL run");
    assert_eq!(rtl.exit_code, Some(0), "RTL exit code");
    assert!(!rtl.trap, "RTL reported a fatal trap");
    let mut iss = Rv64gMachine::load_flat(0, RV64A);
    assert_eq!(iss.run(200_000), Rv64Stop::Exit(0));
    assert_eq!(rtl.regs, iss.regs, "RV64A RTL/ISS register mismatch");
}

#[test]
fn rv64f_non_arithmetic_image_matches_iss() {
    if cfg!(debug_assertions) {
        eprintln!("skipping RV64F subset RTL run in debug builds");
        return;
    }
    let rtl = eda_cpu_platform::systemverilog::run_rv64i_platform(RV64F_OPS, 200_000)
        .expect("RV64F subset RTL run");
    assert_eq!(rtl.exit_code, Some(0), "RTL exit code");
    assert!(!rtl.trap, "RTL reported a fatal trap");
    let mut iss = Rv64gMachine::load_flat(0, RV64F_OPS);
    assert_eq!(iss.run(200_000), Rv64Stop::Exit(0));
    assert_eq!(rtl.regs, iss.regs, "RV64F subset RTL/ISS register mismatch");
}

#[test]
fn rv64_ecall_with_zero_mtvec_reports_a0_as_exit_code() {
    if cfg!(debug_assertions) {
        eprintln!("skipping RV64 trap RTL run in debug builds");
        return;
    }
    let rtl = eda_cpu_platform::systemverilog::run_rv64i_platform(RV64EXIT, 10_000)
        .expect("RV64 ECALL RTL run");
    assert_eq!(rtl.exit_code, Some(7), "ecall exit code");
    assert!(rtl.halted, "ecall should halt when mtvec is zero");
    assert_eq!(rtl.trap_cause, 11, "ecall mcause");
    // The runner turns a documented ECALL stop into a clean process exit.
    assert!(!rtl.trap);

    let mut iss = Rv64gMachine::load_flat(0, RV64EXIT);
    assert_eq!(iss.run(100), Rv64Stop::Exit(7));
}

#[test]
fn rv64_rtl_run_can_emit_a_vcd_waveform() {
    if cfg!(debug_assertions) {
        eprintln!("skipping RV64 VCD run in debug builds");
        return;
    }
    let mut memory = eda_cpu_platform::rv64g::Rv64Memory::new();
    memory.write_slice(0, RV64CSR);
    let result = Rv64iPlatformRunner::new()
        .expect("compile RV64I platform")
        .run_binary(0, memory, 10_000, true)
        .expect("RV64I RTL run with VCD");
    assert_eq!(result.exit_code, Some(0));
    let vcd = result.vcd.expect("VCD requested");
    assert!(vcd.contains("$enddefinitions"), "malformed VCD");
    assert!(vcd.contains("$var"), "VCD has no variables");
}
