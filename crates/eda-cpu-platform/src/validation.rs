use crate::i8086::{I8086Machine, I8086Stop, REG_AX};
use crate::rv32i::{Rv32iMachine, StopReason};
use crate::SparseMemory;

#[derive(Clone, Debug)]
pub struct ValidationResult {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

pub fn rv32i_smoke_validation() -> Vec<ValidationResult> {
    vec![
        validate_case(
            "rv32i_arithmetic_edge",
            &[0x800000b7, 0x7ff00113, 0x002081b3, 0x00100073],
            |cpu| cpu.regs[3] == 0x8000_07ff,
        ),
        validate_case(
            "rv32i_signed_unsigned_compare",
            &[0x800000b7, 0x00100113, 0x0020a1b3, 0x0020b233, 0x00100073],
            |cpu| cpu.regs[3] == 1 && cpu.regs[4] == 0,
        ),
        validate_case(
            "rv32i_negative_load_store_immediates",
            &[
                0x00400093, // addi x1, zero, 4
                0xfff00113, // addi x2, zero, -1
                0xfe208fa3, // sb x2, -1(x1)
                0xfff08183, // lb x3, -1(x1)
                0x00100073, // ebreak
            ],
            |cpu| cpu.regs[3] == 0xffff_ffff,
        ),
    ]
}

pub fn i8086_smoke_validation() -> Vec<ValidationResult> {
    vec![
        validate_i8086_case(
            "i8086_register_and_rotate",
            &[0xb8, 0x34, 0x12, 0xd1, 0xc0, 0xf4],
            |cpu| cpu.regs[REG_AX] == 0x2468,
        ),
        validate_i8086_case(
            "i8086_rep_movsb",
            &[
                0xb8, 0x00, 0x02, 0x89, 0xc6, 0xbf, 0x03, 0x00, 0xb9, 0x03, 0x00, 0xfc, 0xf3, 0xa4,
                0xa0, 0x03, 0x00, 0xf4,
            ],
            |cpu| cpu.memory.read_u8(5) == b'C',
        ),
        validate_i8086_case(
            "i8086_push_pop_dec_flow",
            &[0xb8, 0x2a, 0x00, 0x50, 0x58, 0x48, 0xf4],
            |cpu| cpu.regs[REG_AX] == 41,
        ),
        validate_i8086_case(
            "i8086_or_xor_logic",
            &[0xb0, 0xf0, 0x0c, 0x0f, 0x34, 0xff, 0xf4],
            |cpu| cpu.regs[REG_AX] as u8 == 0,
        ),
        validate_i8086_case(
            "i8086_rcl_through_carry",
            &[0xb0, 0x80, 0xf9, 0xd0, 0xd0, 0xf4],
            |cpu| cpu.regs[REG_AX] as u8 == 1,
        ),
    ]
}

pub fn platform_validation() -> Vec<ValidationResult> {
    let mut results = rv32i_smoke_validation();
    results.extend(i8086_smoke_validation());
    results
}

fn validate_case(
    name: &str,
    instructions: &[u32],
    predicate: impl Fn(&Rv32iMachine) -> bool,
) -> ValidationResult {
    let mut memory = SparseMemory::new();
    for (index, instruction) in instructions.iter().enumerate() {
        memory.write_le_u32((index as u32) * 4, *instruction);
    }
    let mut cpu = Rv32iMachine::baremetal(0, memory);
    let stop = cpu.run(instructions.len() as u64 + 16);
    let passed = stop == StopReason::Ebreak && predicate(&cpu);
    ValidationResult {
        name: name.to_string(),
        passed,
        detail: format!(
            "stop={stop:?} x1={} x2={} x3={} x4={} mem3={:#x} trap={:?}",
            cpu.regs[1],
            cpu.regs[2],
            cpu.regs[3],
            cpu.regs[4],
            cpu.memory.read_u8(3),
            cpu.trap
        ),
    }
}

fn validate_i8086_case(
    name: &str,
    program: &[u8],
    predicate: impl Fn(&I8086Machine) -> bool,
) -> ValidationResult {
    let mut cpu = I8086Machine::com(program, "").expect("COM fixture");
    cpu.memory.write_slice(0x200, b"ABC");
    let stop = cpu.run(program.len() as u64 + 64);
    let passed = stop == I8086Stop::Halt && predicate(&cpu);
    ValidationResult {
        name: name.to_string(),
        passed,
        detail: format!(
            "stop={stop:?} ax={:#06x} si={:#x} di={:#x} cx={:#x} src={:#x} mem={:?} output={:?}",
            cpu.regs[REG_AX],
            cpu.regs[6],
            cpu.regs[7],
            cpu.regs[1],
            cpu.memory.read_u8(0x200),
            [
                cpu.memory.read_u8(3),
                cpu.memory.read_u8(4),
                cpu.memory.read_u8(5)
            ],
            cpu.output
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_smoke_cases_pass() {
        for result in platform_validation() {
            assert!(result.passed, "{}: {}", result.name, result.detail);
        }
    }
}
