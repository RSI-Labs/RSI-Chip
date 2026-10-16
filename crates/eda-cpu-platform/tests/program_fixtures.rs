use eda_cpu_platform::i8086::{I8086Machine, I8086Stop};
use eda_cpu_platform::rv32i::{Rv32iMachine, StopReason};

#[test]
fn checked_in_compiled_program_fixtures_run() {
    let mut i8086 = I8086Machine::com(include_bytes!("../../../platform/programs/hello.com"), "")
        .expect("load checked-in COM fixture");
    assert_eq!(i8086.run(100), I8086Stop::Halt);
    assert_eq!(i8086.output, b"Hello");

    let mut rv32i = Rv32iMachine::load_elf(
        include_bytes!("../../../platform/programs/rv32i_exit42.elf"),
        &[],
    )
    .expect("load checked-in ELF fixture");
    assert_eq!(rv32i.run(100), StopReason::Exit(42));
}

#[test]
fn checked_in_com_runs_on_systemverilog_core() {
    let program = include_bytes!("../../../platform/programs/hello.com");
    let mut golden = I8086Machine::com(program, "").expect("golden COM loader");
    golden.run(100);
    let rtl = eda_cpu_platform::systemverilog::run_i8086_com(program)
        .expect("SystemVerilog COM execution");
    assert_eq!(rtl.output, b"Hello");
    assert_eq!(rtl.output, golden.output);
    assert!(rtl.halted);
}

#[test]
fn checked_in_elf_runs_on_systemverilog_core() {
    let program = include_bytes!("../../../platform/programs/rv32i_exit42.elf");
    let mut golden = Rv32iMachine::load_elf(program, &[]).expect("golden ELF loader");
    assert_eq!(golden.run(100), StopReason::Exit(42));
    let rtl = eda_cpu_platform::systemverilog::run_rv32i_elf(program)
        .expect("SystemVerilog ELF execution");
    assert!(rtl.halted);
    assert_eq!(rtl.a0, golden.regs[10]);
}

#[test]
fn high_memory_com_runs_on_golden_and_systemverilog_models() {
    let program = include_bytes!("../../../platform/programs/high_memory.com");
    assert!(program.len() > 64);

    let mut golden = I8086Machine::com(program, "").expect("golden high-memory COM loader");
    assert_eq!(golden.run(100), I8086Stop::Halt);
    assert_eq!(golden.output, b"OK");

    let rtl = eda_cpu_platform::systemverilog::run_i8086_com(program)
        .expect("SystemVerilog high-memory COM execution");
    assert_eq!(rtl.output, b"OK");
    assert_eq!(rtl.output, golden.output);
    assert!(rtl.halted && !rtl.trap);
}

#[test]
fn high_memory_elf_runs_on_golden_and_systemverilog_models() {
    let program = include_bytes!("../../../platform/programs/rv32i_high_memory.elf");
    assert!(program.len() > 64);

    let mut golden = Rv32iMachine::load_elf(program, &[]).expect("golden high-memory ELF loader");
    assert_eq!(golden.run(100), StopReason::Exit(42));
    assert_eq!(golden.memory.read_le_u32(0x1084), 42);

    let rtl = eda_cpu_platform::systemverilog::run_rv32i_elf(program)
        .expect("SystemVerilog high-memory ELF execution");
    assert!(rtl.halted);
    assert_eq!(rtl.a0, 42);
    assert_eq!(rtl.a0, golden.regs[10]);
}
