use eda_cpu_platform::elf::parse_elf;
use eda_cpu_platform::rv32i::{Rv32iMachine, StopReason};

#[test]
fn loads_and_runs_a_minimal_rv32i_linux_elf() {
    let instructions: [u8; 12] = [
        0x13, 0x05, 0xa0, 0x02, // addi a0, zero, 42
        0x93, 0x08, 0xd0, 0x05, // addi a7, zero, 93
        0x73, 0x00, 0x00, 0x00, // ecall
    ];
    let elf = minimal_rv32_elf(&instructions, 0x1000);
    let parsed = parse_elf(&elf).expect("parse ELF");
    assert_eq!(parsed.entry, 0x1000);
    assert_eq!(parsed.segments.len(), 1);
    let mut machine = Rv32iMachine::load_elf(&elf, &[]).expect("load ELF");
    assert_eq!(machine.pc, 0x1000);
    assert_eq!(machine.run(100), StopReason::Exit(42));
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn minimal_rv32_elf(code: &[u8], entry: u32) -> Vec<u8> {
    let mut elf = Vec::new();
    elf.extend_from_slice(b"\x7fELF");
    elf.extend_from_slice(&[1, 1, 1, 0, 0]);
    elf.extend_from_slice(&[0; 7]);
    push_u16(&mut elf, 2); // ET_EXEC
    push_u16(&mut elf, 243); // EM_RISCV
    push_u32(&mut elf, 1);
    push_u32(&mut elf, entry);
    push_u32(&mut elf, 52); // program-header offset
    push_u32(&mut elf, 0); // section-header offset
    push_u32(&mut elf, 0); // flags
    push_u16(&mut elf, 52);
    push_u16(&mut elf, 32);
    push_u16(&mut elf, 1);
    push_u16(&mut elf, 0);
    push_u16(&mut elf, 0);
    push_u16(&mut elf, 0);

    let code_offset = 0x100_usize;
    push_u32(&mut elf, 1); // PT_LOAD
    push_u32(&mut elf, code_offset as u32); // file offset
    push_u32(&mut elf, entry); // vaddr
    push_u32(&mut elf, entry); // paddr
    push_u32(&mut elf, code.len() as u32);
    push_u32(&mut elf, code.len() as u32);
    push_u32(&mut elf, 5); // RX
    push_u32(&mut elf, 0x1000);
    elf.resize(code_offset, 0);
    elf.extend_from_slice(code);
    elf
}
