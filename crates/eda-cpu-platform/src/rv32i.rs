use crate::elf::{load_elf, ElfError};
use crate::SparseMemory;
use std::collections::HashMap;

pub const CSR_MSTATUS: u32 = 0x300;
pub const CSR_MISA: u32 = 0x301;
pub const CSR_MIE: u32 = 0x304;
pub const CSR_MTVEC: u32 = 0x305;
pub const CSR_MSCRATCH: u32 = 0x340;
pub const CSR_MEPC: u32 = 0x341;
pub const CSR_MCAUSE: u32 = 0x342;
pub const CSR_MTVAL: u32 = 0x343;
pub const CSR_MIP: u32 = 0x344;
pub const CSR_CYCLE: u32 = 0xc00;
pub const CSR_TIME: u32 = 0xc01;
pub const CSR_INSTRET: u32 = 0xc02;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    Running,
    Ebreak,
    Fault,
    Exit(i32),
    MaxSteps,
}

#[derive(Clone, Debug)]
pub struct Trap {
    pub pc: u32,
    pub cause: u32,
    pub value: u32,
}

#[derive(Clone, Debug)]
pub struct Rv32iMachine {
    pub memory: SparseMemory,
    pub regs: [u32; 32],
    pub pc: u32,
    pub csrs: HashMap<u32, u32>,
    pub cycle: u64,
    pub instructions_retired: u64,
    pub stop: StopReason,
    pub trap: Option<Trap>,
    pub linux_abi: bool,
    pub output: Vec<u8>,
    program_break: u32,
}

impl Rv32iMachine {
    pub fn baremetal(entry: u32, memory: SparseMemory) -> Self {
        Self::with_mode(entry, memory, false)
    }

    pub fn linux(entry: u32, memory: SparseMemory) -> Self {
        Self::with_mode(entry, memory, true)
    }

    fn with_mode(entry: u32, memory: SparseMemory, linux_abi: bool) -> Self {
        let mut machine = Self {
            memory,
            regs: [0; 32],
            pc: entry,
            csrs: HashMap::new(),
            cycle: 0,
            instructions_retired: 0,
            stop: StopReason::Running,
            trap: None,
            linux_abi,
            output: Vec::new(),
            program_break: 0,
        };
        machine.csrs.insert(CSR_MISA, 0x4000_0000); // RV32, no extensions in this model
        machine
    }

    pub fn load_elf(bytes: &[u8], args: &[String]) -> Result<Self, ElfError> {
        let mut memory = SparseMemory::new();
        let image = load_elf(&mut memory, bytes)?;
        let mut machine = Self::linux(image.entry as u32, memory);
        machine.initialize_linux_stack(args);
        machine.program_break = image
            .segments
            .iter()
            .map(|segment| (segment.vaddr + segment.memory_size) as u32)
            .max()
            .unwrap_or(0)
            .max(0x1000);
        Ok(machine)
    }

    fn initialize_linux_stack(&mut self, args: &[String]) {
        let initial_sp = 0xffff_e000_u32;
        let mut string_addresses = Vec::with_capacity(args.len());
        for argument in args.iter().rev() {
            let bytes = argument.as_bytes();
            let start = initial_sp.wrapping_sub(bytes.len() as u32).wrapping_sub(1) & !3;
            self.memory.write_slice(start, bytes);
            self.memory
                .write_u8(start.wrapping_add(bytes.len() as u32), 0);
            string_addresses.push(start);
        }
        string_addresses.reverse();

        let mut sp = initial_sp & !0xf;
        // AT_NULL and two zero terminators. This is sufficient for freestanding
        // _start implementations that walk argc/argv directly.
        for _ in 0..4 {
            sp = sp.wrapping_sub(4);
            self.memory.write_le_u32(sp, 0);
        }
        for address in string_addresses.iter().rev() {
            sp = sp.wrapping_sub(4);
            self.memory.write_le_u32(sp, *address);
        }
        sp = sp.wrapping_sub(4);
        self.memory.write_le_u32(sp, args.len() as u32);
        self.regs[2] = sp;
    }

    pub fn run(&mut self, max_instructions: u64) -> StopReason {
        let start = self.instructions_retired;
        while self.stop == StopReason::Running
            && self.instructions_retired - start < max_instructions
        {
            self.step();
        }
        if self.stop == StopReason::Running {
            self.stop = StopReason::MaxSteps;
        }
        self.stop
    }

    pub fn step(&mut self) -> StopReason {
        if self.stop != StopReason::Running {
            return self.stop;
        }
        self.cycle = self.cycle.wrapping_add(1);
        let instruction = self.memory.read_le_u32(self.pc);
        let opcode = instruction & 0x7f;
        let rd = ((instruction >> 7) & 0x1f) as usize;
        let funct3 = (instruction >> 12) & 7;
        let rs1 = ((instruction >> 15) & 0x1f) as usize;
        let rs2 = ((instruction >> 20) & 0x1f) as usize;
        let funct7 = instruction >> 25;
        let a = self.regs[rs1];
        let b = self.regs[rs2];
        let next_sequential = self.pc.wrapping_add(4);
        let mut next_pc = next_sequential;
        let mut writeback = None;

        match opcode {
            0x37 => writeback = Some(instruction & 0xffff_f000),
            0x17 => writeback = Some(self.pc.wrapping_add(instruction & 0xffff_f000)),
            0x6f => {
                let immediate = jump_immediate(instruction);
                if rd != 0 {
                    self.regs[rd] = next_sequential;
                }
                next_pc = self.pc.wrapping_add(immediate);
            }
            0x67 => {
                if funct3 != 0 || (instruction >> 26) != 0 {
                    self.take_trap(instruction);
                    return self.finish_trap();
                }
                let immediate = immediate_i(instruction);
                let target = a.wrapping_add(immediate) & !1;
                if rd != 0 {
                    self.regs[rd] = next_sequential;
                }
                next_pc = target;
            }
            0x63 => {
                let immediate = branch_immediate(instruction);
                let taken = match funct3 {
                    0 => a == b,
                    1 => a != b,
                    4 => (a as i32) < (b as i32),
                    5 => (a as i32) >= (b as i32),
                    6 => a < b,
                    7 => a >= b,
                    _ => {
                        self.take_trap(instruction);
                        return self.finish_trap();
                    }
                };
                if taken {
                    next_pc = self.pc.wrapping_add(immediate);
                }
            }
            0x03 => {
                let immediate = immediate_i(instruction);
                let address = a.wrapping_add(immediate);
                let value = match funct3 {
                    0 => sign_extend_u8(self.memory.read_u8(address)),
                    1 => sign_extend_u16(self.memory.read_le_u16(address)),
                    2 => self.memory.read_le_u32(address),
                    4 => self.memory.read_u8(address) as u32,
                    5 => self.memory.read_le_u16(address) as u32,
                    _ => {
                        self.take_trap(instruction);
                        return self.finish_trap();
                    }
                };
                writeback = Some(value);
            }
            0x23 => {
                let immediate = store_immediate(instruction);
                let address = a.wrapping_add(immediate);
                match funct3 {
                    0 => self.memory.write_u8(address, b as u8),
                    1 => self.memory.write_le_u16(address, b as u16),
                    2 => self.memory.write_le_u32(address, b),
                    _ => {
                        self.take_trap(instruction);
                        return self.finish_trap();
                    }
                }
            }
            0x13 => {
                let immediate = immediate_i(instruction);
                let shamt = immediate & 0x1f;
                let value = match (funct3, instruction >> 26) {
                    (0, _) => a.wrapping_add(immediate),
                    (1, 0) => {
                        if instruction >> 27 != 0 {
                            self.take_trap(instruction);
                            return self.finish_trap();
                        }
                        a << shamt
                    }
                    (2, _) => ((a as i32) < (immediate as i32)) as u32,
                    (3, _) => (a < immediate) as u32,
                    (4, _) => a ^ immediate,
                    (5, 0) => a >> shamt,
                    (5, 0x20) => ((a as i32) >> shamt) as u32,
                    (6, _) => a | immediate,
                    (7, _) => a & immediate,
                    _ => {
                        self.take_trap(instruction);
                        return self.finish_trap();
                    }
                };
                writeback = Some(value);
            }
            0x33 => {
                let value = match (funct3, funct7) {
                    (0, 0x00) => a.wrapping_add(b),
                    (0, 0x20) => a.wrapping_sub(b),
                    (1, 0) => a << (b & 31),
                    (2, 0) => ((a as i32) < (b as i32)) as u32,
                    (3, 0) => (a < b) as u32,
                    (4, 0) => a ^ b,
                    (5, 0) => a >> (b & 31),
                    (5, 0x20) => ((a as i32) >> (b & 31)) as u32,
                    (6, 0) => a | b,
                    (7, 0) => a & b,
                    _ => {
                        self.take_trap(instruction);
                        return self.finish_trap();
                    }
                };
                writeback = Some(value);
            }
            0x0f => {
                // FENCE and FENCE.I preserve single-hart memory ordering in
                // this sequential golden model.
                if funct3 > 1 || instruction >> 20 != 0 {
                    self.take_trap(instruction);
                    return self.finish_trap();
                }
            }
            0x73 => {
                let immediate = instruction >> 20;
                match (funct3, immediate, rd, rs1) {
                    (0, 0, 0, 0) => {
                        let ecall_target = self.handle_ecall();
                        if self.stop != StopReason::Running {
                            return self.stop;
                        }
                        if let Some(target) = ecall_target {
                            self.pc = target;
                            self.instructions_retired = self.instructions_retired.wrapping_add(1);
                            return self.stop;
                        }
                    }
                    (0, 1, 0, 0) => {
                        self.stop = StopReason::Ebreak;
                    }
                    (0, 0x302, 0, 0) => {
                        let mstatus = *self.csrs.get(&CSR_MSTATUS).unwrap_or(&0);
                        self.csrs.insert(CSR_MSTATUS, (mstatus | 0x80) & !0x8);
                        next_pc = *self.csrs.get(&CSR_MEPC).unwrap_or(&0);
                    }
                    (0, 0x105, _, _) => {}
                    (1..=3 | 5..=7, _, _, _) => {
                        let address = immediate;
                        let Some(old) = self.read_csr_or_trap(address) else {
                            self.take_trap(instruction);
                            return self.finish_trap();
                        };
                        let source = if funct3 >= 5 { rs1 as u32 } else { a };
                        let value = match funct3 & 3 {
                            1 => source,
                            2 => old | source,
                            _ => old & !source,
                        };
                        if !self.write_csr_or_trap(address, value) {
                            self.take_trap(instruction);
                            return self.finish_trap();
                        }
                        writeback = Some(old);
                    }
                    _ => {
                        self.take_trap(instruction);
                        return self.finish_trap();
                    }
                }
            }
            _ => {
                self.take_trap(instruction);
                return self.finish_trap();
            }
        }

        if let Some(value) = writeback {
            if rd != 0 {
                self.regs[rd] = value;
            }
        }
        self.pc = next_pc;
        self.instructions_retired = self.instructions_retired.wrapping_add(1);
        self.stop
    }

    fn handle_ecall(&mut self) -> Option<u32> {
        if !self.linux_abi {
            self.csrs.insert(CSR_MEPC, self.pc);
            self.csrs.insert(CSR_MCAUSE, 11);
            self.csrs.insert(CSR_MTVAL, 0);
            let trap_vector = *self.csrs.get(&CSR_MTVEC).unwrap_or(&0) & !3;
            if trap_vector == 0 {
                self.stop = StopReason::Fault;
                self.trap = Some(Trap {
                    pc: self.pc,
                    cause: 11,
                    value: 0,
                });
            }
            return Some(trap_vector);
        }

        let number = self.regs[17];
        match number {
            93 | 94 => {
                self.stop = StopReason::Exit(self.regs[10] as i32);
                None
            }
            64 => {
                let fd = self.regs[10] as i32;
                let address = self.regs[11];
                let length = self.regs[12] as usize;
                if fd == 1 || fd == 2 {
                    for offset in 0..length {
                        self.output
                            .push(self.memory.read_u8(address.wrapping_add(offset as u32)));
                    }
                    self.regs[10] = length as u32;
                } else {
                    self.regs[10] = u32::MAX; // -EBADF
                }
                None
            }
            214 => {
                let requested = self.regs[10];
                if requested != 0 {
                    self.program_break = self.program_break.max(requested);
                }
                self.regs[10] = self.program_break;
                None
            }
            222 => {
                let address = self.regs[10];
                let length = self.regs[11];
                self.memory.fill(address, length, 0);
                self.regs[10] = address;
                None
            }
            _ => {
                // Unsupported Linux syscalls are reported as -ENOSYS rather
                // than silently succeeding; this makes compiled-program
                // failures observable.
                self.regs[10] = u32::MAX - 37;
                None
            }
        }
    }

    fn read_csr_or_trap(&self, address: u32) -> Option<u32> {
        match address {
            CSR_MSTATUS | CSR_MISA | CSR_MIE | CSR_MTVEC | CSR_MSCRATCH | CSR_MEPC | CSR_MCAUSE
            | CSR_MTVAL | CSR_MIP => Some(*self.csrs.get(&address).unwrap_or(&0)),
            CSR_CYCLE => Some(self.cycle as u32),
            CSR_TIME => Some(self.cycle as u32),
            CSR_INSTRET => Some(self.instructions_retired as u32),
            _ => None,
        }
    }

    fn write_csr_or_trap(&mut self, address: u32, value: u32) -> bool {
        match address {
            CSR_MSTATUS | CSR_MIE | CSR_MTVEC | CSR_MSCRATCH | CSR_MEPC | CSR_MCAUSE
            | CSR_MTVAL | CSR_MIP => {
                self.csrs.insert(address, value);
                true
            }
            CSR_MISA | CSR_CYCLE | CSR_TIME | CSR_INSTRET => false,
            _ => false,
        }
    }

    fn take_trap(&mut self, instruction: u32) {
        self.trap = Some(Trap {
            pc: self.pc,
            cause: 2,
            value: instruction,
        });
        self.csrs.insert(CSR_MEPC, self.pc);
        self.csrs.insert(CSR_MCAUSE, 2);
        self.csrs.insert(CSR_MTVAL, instruction);
        let vector = *self.csrs.get(&CSR_MTVEC).unwrap_or(&0) & !3;
        if vector == 0 {
            self.stop = StopReason::Fault;
        } else {
            self.pc = vector;
        }
    }

    fn finish_trap(&mut self) -> StopReason {
        self.instructions_retired = self.instructions_retired.wrapping_add(1);
        self.stop
    }
}

fn sign_extend(value: u32, sign_bit_index: u32) -> u32 {
    if (value >> sign_bit_index) & 1 == 1 {
        value | (!0 << (sign_bit_index + 1))
    } else {
        value
    }
}

fn immediate_i(instruction: u32) -> u32 {
    sign_extend((instruction >> 20) & 0xfff, 11)
}

fn store_immediate(instruction: u32) -> u32 {
    let value = ((instruction >> 25) << 5) | ((instruction >> 7) & 0x1f);
    sign_extend(value, 11)
}

fn branch_immediate(instruction: u32) -> u32 {
    let value = ((instruction >> 31) & 1) << 12
        | ((instruction >> 7) & 1) << 11
        | ((instruction >> 25) & 0x3f) << 5
        | ((instruction >> 8) & 0xf) << 1;
    sign_extend(value, 11)
}

fn jump_immediate(instruction: u32) -> u32 {
    let value = ((instruction >> 31) & 1) << 20
        | ((instruction >> 12) & 0xff) << 12
        | ((instruction >> 20) & 1) << 11
        | ((instruction >> 21) & 0x3ff) << 1;
    sign_extend(value, 20)
}

fn sign_extend_u8(value: u8) -> u32 {
    value as i8 as i32 as u32
}

fn sign_extend_u16(value: u16) -> u32 {
    value as i16 as i32 as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immediate_helpers_sign_extend() {
        assert_eq!(immediate_i(0xfff08183), 0xffff_ffff);
        assert_eq!(store_immediate(0xfe208fa3), 0xffff_ffff);
    }

    #[test]
    fn arithmetic_and_control_flow_execute() {
        let mut memory = SparseMemory::new();
        let instructions: [u32; 5] = [
            0x00a00093, // addi x1, x0, 10
            0x00b00113, // addi x2, x0, 11
            0x002081b3, // add x3, x1, x2
            0x00100113, // addi x2, x0, 1
            0x00100073, // ebreak
        ];
        for (index, instruction) in instructions.iter().enumerate() {
            memory.write_le_u32((index as u32) * 4, *instruction);
        }
        let mut cpu = Rv32iMachine::baremetal(0, memory);
        assert_eq!(cpu.run(100), StopReason::Ebreak);
        assert_eq!(cpu.regs[3], 21);
    }

    #[test]
    fn negative_store_and_load_immediates_sign_extend() {
        let mut memory = SparseMemory::new();
        let instructions = [
            0x00400093u32,
            0xfff00113,
            0xfe208fa3,
            0xfff08183,
            0x00100073,
        ];
        for (i, instr) in instructions.iter().enumerate() {
            memory.write_le_u32(i as u32 * 4, *instr);
        }
        let mut cpu = Rv32iMachine::baremetal(0, memory);
        for _ in 0..5 {
            cpu.step();
        }
        assert_eq!(cpu.regs[3], 0xffffffff);
    }
}
