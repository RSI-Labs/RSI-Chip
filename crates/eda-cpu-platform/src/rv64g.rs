//! A user-level RV64G instruction-set simulator.
//!
//! Implements RV64I, M, A, F, D, Zicsr, and Zifencei semantics for a single
//! hart. It is the golden reference for the RV64 RTL work and can run flat
//! RV64G test images compiled with LLVM's RISC-V backend.
//!
//! Platform MMIO matches the RV32I platform runner:
//!   0x1000_0000 console TX (byte)
//!   0x1000_0004 shutdown (word; low bits are the exit status)
//!   0x1000_0008 console status (bit 0 ready)
//!
//! The Linux-like `ecall` ABI is also available:
//!   a7=64  write(fd=a0, buf=a1, len=a2)
//!   a7=93  exit(code=a0)

use std::collections::HashMap;

const PAGE_SHIFT: u32 = 12;
const PAGE_SIZE: usize = 1 << PAGE_SHIFT;

/// Sparse 64-bit byte memory used by the RV64G model.
#[derive(Clone, Debug, Default)]
pub struct Rv64Memory {
    pages: HashMap<u64, Box<[u8; PAGE_SIZE]>>,
}

impl Rv64Memory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn read_u8(&self, address: u64) -> u8 {
        self.pages
            .get(&(address >> PAGE_SHIFT))
            .map_or(0, |page| page[(address as usize) & (PAGE_SIZE - 1)])
    }

    pub fn write_u8(&mut self, address: u64, value: u8) {
        let page = self
            .pages
            .entry(address >> PAGE_SHIFT)
            .or_insert_with(|| Box::new([0; PAGE_SIZE]));
        page[(address as usize) & (PAGE_SIZE - 1)] = value;
    }

    pub fn read_le_u16(&self, address: u64) -> u16 {
        u16::from_le_bytes([self.read_u8(address), self.read_u8(address.wrapping_add(1))])
    }

    pub fn read_le_u32(&self, address: u64) -> u32 {
        u32::from_le_bytes([
            self.read_u8(address),
            self.read_u8(address.wrapping_add(1)),
            self.read_u8(address.wrapping_add(2)),
            self.read_u8(address.wrapping_add(3)),
        ])
    }

    pub fn read_le_u64(&self, address: u64) -> u64 {
        u64::from_le_bytes([
            self.read_u8(address),
            self.read_u8(address.wrapping_add(1)),
            self.read_u8(address.wrapping_add(2)),
            self.read_u8(address.wrapping_add(3)),
            self.read_u8(address.wrapping_add(4)),
            self.read_u8(address.wrapping_add(5)),
            self.read_u8(address.wrapping_add(6)),
            self.read_u8(address.wrapping_add(7)),
        ])
    }

    pub fn write_le_u16(&mut self, address: u64, value: u16) {
        for (index, byte) in value.to_le_bytes().iter().enumerate() {
            self.write_u8(address.wrapping_add(index as u64), *byte);
        }
    }

    pub fn write_le_u32(&mut self, address: u64, value: u32) {
        for (index, byte) in value.to_le_bytes().iter().enumerate() {
            self.write_u8(address.wrapping_add(index as u64), *byte);
        }
    }

    pub fn write_le_u64(&mut self, address: u64, value: u64) {
        for (index, byte) in value.to_le_bytes().iter().enumerate() {
            self.write_u8(address.wrapping_add(index as u64), *byte);
        }
    }

    pub fn write_slice(&mut self, mut address: u64, bytes: &[u8]) {
        for byte in bytes {
            self.write_u8(address, *byte);
            address = address.wrapping_add(1);
        }
    }

    pub fn fill(&mut self, start: u64, length: u64, value: u8) {
        for offset in 0..length {
            self.write_u8(start.wrapping_add(offset), value);
        }
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rv64Stop {
    Running,
    Exit(i32),
    Ebreak,
    Fault,
    MaxSteps,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rv64Trap {
    pub pc: u64,
    pub cause: u64,
    pub value: u64,
}

#[derive(Clone, Debug)]
pub struct Rv64gMachine {
    pub memory: Rv64Memory,
    pub regs: [u64; 32],
    pub fregs: [u64; 32],
    pub pc: u64,
    pub csrs: HashMap<u16, u64>,
    pub fcsr: u64,
    pub cycle: u64,
    pub instructions_retired: u64,
    pub stop: Rv64Stop,
    pub trap: Option<Rv64Trap>,
    pub output: Vec<u8>,
    reservation: Option<u64>,
}

const MMIO_UART_TX: u64 = 0x1000_0000;
const MMIO_SHUTDOWN: u64 = 0x1000_0004;
const MMIO_UART_STATUS: u64 = 0x1000_0008;

impl Rv64gMachine {
    pub fn baremetal(entry: u64, memory: Rv64Memory) -> Self {
        let mut csrs = HashMap::new();
        // RV64, IMAFD.
        let misa = (2u64 << 62) | (1 << 8) | (1 << 12) | (1 << 0) | (1 << 5) | (1 << 3);
        csrs.insert(0x301, misa);
        csrs.insert(0x300, 0); // mstatus
        csrs.insert(0x305, 0); // mtvec
        csrs.insert(0x340, 0); // mscratch
        csrs.insert(0x341, 0); // mepc
        csrs.insert(0x342, 0); // mcause
        csrs.insert(0x343, 0); // mtval
        csrs.insert(0xf14, 0); // mhartid
        Self {
            memory,
            regs: [0; 32],
            fregs: [0; 32],
            pc: entry,
            csrs,
            fcsr: 0,
            cycle: 0,
            instructions_retired: 0,
            stop: Rv64Stop::Running,
            trap: None,
            output: Vec::new(),
            reservation: None,
        }
    }

    pub fn load_flat(entry: u64, bytes: &[u8]) -> Self {
        let mut memory = Rv64Memory::new();
        memory.write_slice(entry, bytes);
        Self::baremetal(entry, memory)
    }

    pub fn run(&mut self, max_instructions: u64) -> Rv64Stop {
        for _ in 0..max_instructions {
            if self.stop != Rv64Stop::Running {
                return self.stop;
            }
            self.step();
        }
        if self.stop == Rv64Stop::Running {
            self.stop = Rv64Stop::MaxSteps;
        }
        self.stop
    }

    pub fn step(&mut self) -> Rv64Stop {
        if self.stop != Rv64Stop::Running {
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
        let mut fwriteback = None;

        match opcode {
            0x37 => writeback = Some(sign_extend_32(instruction & 0xffff_f000)),
            0x17 => {
                writeback = Some(
                    self.pc
                        .wrapping_add(sign_extend_32(instruction & 0xffff_f000)),
                )
            }
            0x6f => {
                let immediate = jump_immediate(instruction);
                writeback = Some(next_sequential);
                next_pc = self.pc.wrapping_add(immediate);
            }
            0x67 => {
                let immediate = sign_extend_12(instruction >> 20);
                let target = a.wrapping_add(immediate) & !1;
                writeback = Some(next_sequential);
                next_pc = target;
            }
            0x63 => {
                let immediate = branch_immediate(instruction);
                let taken = match funct3 {
                    0 => a == b,
                    1 => a != b,
                    4 => (a as i64) < (b as i64),
                    5 => (a as i64) >= (b as i64),
                    6 => a < b,
                    7 => a >= b,
                    _ => return self.illegal(instruction),
                };
                if taken {
                    next_pc = self.pc.wrapping_add(immediate);
                }
            }
            0x03 => {
                let address = a.wrapping_add(sign_extend_12(instruction >> 20));
                let (value, write) = if address == MMIO_UART_STATUS {
                    (1, true)
                } else if address == MMIO_UART_TX || address == MMIO_SHUTDOWN {
                    (0, true)
                } else {
                    match funct3 {
                        0 => (sign_extend_8(self.memory.read_u8(address)), true),
                        1 => (sign_extend_16(self.memory.read_le_u16(address)), true),
                        2 => (sign_extend_32(self.memory.read_le_u32(address)), true),
                        3 => (self.memory.read_le_u64(address), true),
                        4 => (u64::from(self.memory.read_u8(address)), true),
                        5 => (u64::from(self.memory.read_le_u16(address)), true),
                        6 => (u64::from(self.memory.read_le_u32(address)), true),
                        _ => (0, false),
                    }
                };
                if !write {
                    return self.illegal(instruction);
                }
                writeback = Some(value);
            }
            0x23 => {
                let address = a.wrapping_add(sign_extend_12(
                    (instruction >> 25) << 5 | ((instruction >> 7) & 0x1f),
                ));
                if address == MMIO_UART_TX {
                    self.output.push(b as u8);
                } else if address == MMIO_SHUTDOWN {
                    self.stop = Rv64Stop::Exit(b as i32);
                    return self.stop;
                } else {
                    if self.reservation == Some(address) {
                        self.reservation = None;
                    }
                    match funct3 {
                        0 => self.memory.write_u8(address, b as u8),
                        1 => self.memory.write_le_u16(address, b as u16),
                        2 => self.memory.write_le_u32(address, b as u32),
                        3 => self.memory.write_le_u64(address, b),
                        _ => return self.illegal(instruction),
                    }
                }
            }
            0x13 => {
                let immediate = sign_extend_12(instruction >> 20);
                let shamt = (instruction >> 20) & 0x3f;
                let value = match funct3 {
                    0 => a.wrapping_add(immediate),
                    2 => ((a as i64) < (immediate as i64)) as u64,
                    3 => (a < immediate) as u64,
                    4 => a ^ immediate,
                    6 => a | immediate,
                    7 => a & immediate,
                    1 => {
                        if funct7 >> 1 != 0 {
                            return self.illegal(instruction);
                        }
                        a << shamt
                    }
                    5 => match funct7 >> 1 {
                        0 => a >> shamt,
                        0x10 => ((a as i64) >> shamt) as u64,
                        _ => return self.illegal(instruction),
                    },
                    _ => return self.illegal(instruction),
                };
                writeback = Some(value);
            }
            0x1b => {
                let immediate = sign_extend_12(instruction >> 20);
                let shamt = (instruction >> 20) & 0x1f;
                let value = match funct3 {
                    0 => sign_extend_32((a as u32).wrapping_add(immediate as u32)),
                    1 => {
                        if funct7 != 0 {
                            return self.illegal(instruction);
                        }
                        sign_extend_32((a as u32) << shamt)
                    }
                    5 => match funct7 {
                        0 => sign_extend_32((a as u32) >> shamt),
                        0x20 => sign_extend_32(((a as i32) >> shamt) as u32),
                        _ => return self.illegal(instruction),
                    },
                    _ => return self.illegal(instruction),
                };
                writeback = Some(value);
            }
            0x33 => {
                let value = match (funct7, funct3) {
                    (0x00, 0) => a.wrapping_add(b),
                    (0x20, 0) => a.wrapping_sub(b),
                    (0x00, 1) => a << (b & 63),
                    (0x00, 2) => ((a as i64) < (b as i64)) as u64,
                    (0x00, 3) => (a < b) as u64,
                    (0x00, 4) => a ^ b,
                    (0x00, 5) => a >> (b & 63),
                    (0x20, 5) => ((a as i64) >> (b & 63)) as u64,
                    (0x00, 6) => a | b,
                    (0x00, 7) => a & b,
                    (0x01, 0) => a.wrapping_mul(b),
                    (0x01, 1) => ((a as i64 as i128).wrapping_mul(b as i64 as i128) >> 64) as u64,
                    (0x01, 2) => ((a as i64 as i128).wrapping_mul(b as i128) >> 64) as u64,
                    (0x01, 3) => ((a as u128).wrapping_mul(b as u128) >> 64) as u64,
                    (0x01, 4) => div_signed(a, b),
                    (0x01, 5) => div_unsigned(a, b),
                    (0x01, 6) => rem_signed(a, b),
                    (0x01, 7) => rem_unsigned(a, b),
                    _ => return self.illegal(instruction),
                };
                writeback = Some(value);
            }
            0x3b => {
                let value = match (funct7, funct3) {
                    (0x00, 0) => sign_extend_32((a as u32).wrapping_add(b as u32)),
                    (0x20, 0) => sign_extend_32((a as u32).wrapping_sub(b as u32)),
                    (0x00, 1) => sign_extend_32((a as u32) << (b & 31)),
                    (0x00, 5) => sign_extend_32((a as u32) >> (b & 31)),
                    (0x20, 5) => sign_extend_32(((a as i32) >> (b & 31)) as u32),
                    (0x01, 0) => sign_extend_32((a as u32).wrapping_mul(b as u32)),
                    (0x01, 4) => sign_extend_32(div_signed_32(a, b)),
                    (0x01, 5) => sign_extend_32(div_unsigned_32(a, b)),
                    (0x01, 6) => sign_extend_32(rem_signed_32(a, b)),
                    (0x01, 7) => sign_extend_32(rem_unsigned_32(a, b)),
                    _ => return self.illegal(instruction),
                };
                writeback = Some(value);
            }
            0x2f => {
                let address = a;
                let width = match funct3 {
                    2 => 4,
                    3 => 8,
                    _ => return self.illegal(instruction),
                };
                let funct5 = instruction >> 27;
                match funct5 {
                    0x02 => {
                        let value = if width == 4 {
                            sign_extend_32(self.memory.read_le_u32(address))
                        } else {
                            self.memory.read_le_u64(address)
                        };
                        self.reservation = Some(address);
                        writeback = Some(value);
                    }
                    0x03 => {
                        if self.reservation == Some(address) {
                            self.reservation = None;
                            if width == 4 {
                                self.memory.write_le_u32(address, b as u32);
                            } else {
                                self.memory.write_le_u64(address, b);
                            }
                            writeback = Some(0);
                        } else {
                            self.reservation = None;
                            writeback = Some(1);
                        }
                    }
                    _ => {
                        let old = if width == 4 {
                            u64::from(self.memory.read_le_u32(address))
                        } else {
                            self.memory.read_le_u64(address)
                        };
                        let (old_sext, op_b) = if width == 4 {
                            (sign_extend_32(old as u32), sign_extend_32(b as u32))
                        } else {
                            (old, b)
                        };
                        let new_value = match funct5 {
                            0x01 => b,
                            0x00 => old_sext.wrapping_add(op_b),
                            0x04 => old_sext ^ op_b,
                            0x0c => old_sext & op_b,
                            0x08 => old_sext | op_b,
                            0x10 => {
                                if (old_sext as i64) < (op_b as i64) {
                                    old_sext
                                } else {
                                    op_b
                                }
                            }
                            0x14 => {
                                if (old_sext as i64) > (op_b as i64) {
                                    old_sext
                                } else {
                                    op_b
                                }
                            }
                            0x18 => {
                                if old_sext < op_b {
                                    old_sext
                                } else {
                                    op_b
                                }
                            }
                            0x1c => {
                                if old_sext > op_b {
                                    old_sext
                                } else {
                                    op_b
                                }
                            }
                            _ => return self.illegal(instruction),
                        };
                        if self.reservation == Some(address) {
                            self.reservation = None;
                        }
                        if width == 4 {
                            self.memory.write_le_u32(address, new_value as u32);
                        } else {
                            self.memory.write_le_u64(address, new_value);
                        }
                        writeback = Some(if width == 4 {
                            sign_extend_32(old as u32)
                        } else {
                            old
                        });
                    }
                }
            }
            0x07 => {
                let address = a.wrapping_add(sign_extend_12(instruction >> 20));
                match funct3 {
                    2 => fwriteback = Some(nan_box_f32(self.memory.read_le_u32(address))),
                    3 => fwriteback = Some(self.memory.read_le_u64(address)),
                    _ => return self.illegal(instruction),
                }
            }
            0x27 => {
                let address = a.wrapping_add(sign_extend_12(
                    (instruction >> 25) << 5 | ((instruction >> 7) & 0x1f),
                ));
                let bits = self.fregs[rs2];
                match funct3 {
                    2 => self.memory.write_le_u32(address, bits as u32),
                    3 => self.memory.write_le_u64(address, bits),
                    _ => return self.illegal(instruction),
                }
            }
            0x53 => {
                if let Some(value) = self.execute_fp(instruction) {
                    if let Some(bits) = value.fp {
                        fwriteback = Some(bits);
                    } else if let Some(value) = value.int {
                        writeback = Some(value);
                    }
                } else {
                    return self.illegal(instruction);
                }
            }
            0x43 | 0x47 | 0x4b | 0x4f => {
                if let Some(value) = self.execute_fma(instruction) {
                    fwriteback = Some(value);
                } else {
                    return self.illegal(instruction);
                }
            }
            0x0f => {
                if funct3 > 1 {
                    return self.illegal(instruction);
                }
            }
            0x73 => {
                if funct3 == 0 {
                    match instruction >> 20 {
                        0 => {
                            let number = self.regs[17];
                            match number {
                                93 | 94 => {
                                    self.stop = Rv64Stop::Exit(self.regs[10] as i32);
                                    return self.stop;
                                }
                                64 => {
                                    let address = self.regs[11];
                                    let length = self.regs[12] as usize;
                                    for offset in 0..length {
                                        self.output.push(
                                            self.memory
                                                .read_u8(address.wrapping_add(offset as u64)),
                                        );
                                    }
                                    self.regs[10] = length as u64;
                                }
                                _ => {
                                    self.regs[10] = u64::MAX - 37;
                                }
                            }
                        }
                        1 => self.stop = Rv64Stop::Ebreak,
                        _ => return self.illegal(instruction),
                    }
                } else if matches!(funct3, 1 | 2 | 3 | 5 | 6 | 7) {
                    let address = (instruction >> 20) as u16;
                    let old = match self.read_csr(address) {
                        Some(value) => value,
                        None => return self.illegal(instruction),
                    };
                    let source = if funct3 & 4 != 0 {
                        u64::from((instruction >> 15) & 0x1f)
                    } else {
                        a
                    };
                    let write = match funct3 & 3 {
                        1 => Some(source),
                        2 => Some(old | source),
                        3 => Some(old & !source),
                        _ => None,
                    };
                    let do_write = funct3 & 3 == 1 || source != 0;
                    if do_write {
                        if let Some(value) = write {
                            if !self.write_csr(address, value) {
                                return self.illegal(instruction);
                            }
                        }
                    }
                    writeback = Some(old);
                } else {
                    return self.illegal(instruction);
                }
            }
            _ => return self.illegal(instruction),
        }

        if let Some(value) = writeback {
            if rd != 0 {
                self.regs[rd] = value;
            }
        }
        if let Some(bits) = fwriteback {
            self.fregs[rd] = bits;
        }
        self.pc = next_pc;
        self.instructions_retired = self.instructions_retired.wrapping_add(1);
        self.stop
    }

    fn illegal(&mut self, instruction: u32) -> Rv64Stop {
        self.trap = Some(Rv64Trap {
            pc: self.pc,
            cause: 2,
            value: u64::from(instruction),
        });
        self.csrs.insert(0x341, self.pc);
        self.csrs.insert(0x342, 2);
        self.csrs.insert(0x343, u64::from(instruction));
        self.stop = Rv64Stop::Fault;
        self.stop
    }

    fn read_csr(&mut self, address: u16) -> Option<u64> {
        Some(match address {
            0x001 => self.fcsr & 0x1f,
            0x002 => (self.fcsr >> 5) & 7,
            0x003 => self.fcsr & 0xff,
            0xc00 => self.cycle,
            0xc80 => self.cycle >> 32,
            0xc02 => self.instructions_retired,
            0xc82 => self.instructions_retired >> 32,
            other => *self.csrs.get(&other).unwrap_or(&0),
        })
    }

    fn write_csr(&mut self, address: u16, value: u64) -> bool {
        match address {
            0x001 => self.fcsr = (self.fcsr & !0x1f) | (value & 0x1f),
            0x002 => self.fcsr = (self.fcsr & !(7 << 5)) | ((value & 7) << 5),
            0x003 => self.fcsr = value & 0xff,
            0xc00 | 0xc80 | 0xc02 | 0xc82 => {}
            0x301 => {}
            other => {
                self.csrs.insert(other, value);
            }
        }
        true
    }

    fn fp_flags(&mut self, flags: u64) {
        self.fcsr |= flags & 0x1f;
    }

    fn execute_fp(&mut self, instruction: u32) -> Option<FpResult> {
        let funct7 = instruction >> 25;
        let rs2 = ((instruction >> 20) & 0x1f) as usize;
        let rs1 = ((instruction >> 15) & 0x1f) as usize;
        let funct3 = (instruction >> 12) & 7;
        let a = self.fregs[rs1];
        let b = self.fregs[rs2];
        let result = match funct7 {
            0x00 => self.fp_binary_s(a, b, |x, y| x + y, false),
            0x01 => self.fp_binary_d(a, b, |x, y| x + y, false),
            0x04 => self.fp_binary_s(a, b, |x, y| x - y, false),
            0x05 => self.fp_binary_d(a, b, |x, y| x - y, false),
            0x08 => self.fp_binary_s(a, b, |x, y| x * y, false),
            0x09 => self.fp_binary_d(a, b, |x, y| x * y, false),
            0x0c => self.fp_binary_s(a, b, |x, y| x / y, true),
            0x0d => self.fp_binary_d(a, b, |x, y| x / y, true),
            0x10 | 0x11 => {
                let single = funct7 == 0x10;
                let (sign_a, sign_b) = if single {
                    ((a >> 31) & 1, (b >> 31) & 1)
                } else {
                    ((a >> 63) & 1, (b >> 63) & 1)
                };
                let sign = match funct3 {
                    0 => sign_b,
                    1 => sign_b ^ 1,
                    2 => sign_a ^ sign_b,
                    _ => return None,
                };
                let bits = if single {
                    nan_box_f32(((a as u32) & 0x7fff_ffff) | ((sign as u32) << 31))
                } else {
                    (a & 0x7fff_ffff_ffff_ffff) | (sign << 63)
                };
                FpResult {
                    fp: Some(bits),
                    int: None,
                }
            }
            0x14 | 0x15 => {
                // FMIN/FMAX share funct7; funct3 selects min (0) or max (1).
                let single = funct7 == 0x14;
                let maximum = match funct3 {
                    0 => false,
                    1 => true,
                    _ => return None,
                };
                if single {
                    let result = if maximum {
                        fmax32(f32_of(a), f32_of(b))
                    } else {
                        fmin32(f32_of(a), f32_of(b))
                    };
                    FpResult {
                        fp: Some(nan_box_f32(result.to_bits())),
                        int: None,
                    }
                } else {
                    let result = if maximum {
                        fmax64(f64_of(a), f64_of(b))
                    } else {
                        fmin64(f64_of(a), f64_of(b))
                    };
                    FpResult {
                        fp: Some(result.to_bits()),
                        int: None,
                    }
                }
            }
            0x2c => {
                let value = f32_of(a);
                if value.is_nan() || (value < 0.0) {
                    self.fp_flags(1 << 4);
                }
                FpResult {
                    fp: Some(nan_box_f32(value.sqrt().to_bits())),
                    int: None,
                }
            }
            0x2d => {
                let value = f64_of(a);
                if value.is_nan() || (value < 0.0) {
                    self.fp_flags(1 << 4);
                }
                FpResult {
                    fp: Some(value.sqrt().to_bits()),
                    int: None,
                }
            }
            0x20 => {
                let value = f64_of(a) as f32;
                FpResult {
                    fp: Some(nan_box_f32(value.to_bits())),
                    int: None,
                }
            }
            0x21 => {
                let value = f64::from(f32_of(a));
                FpResult {
                    fp: Some(value.to_bits()),
                    int: None,
                }
            }
            0x50 | 0x51 => {
                let single = funct7 == 0x50;
                let result = if single {
                    let (x, y) = (f32_of(a), f32_of(b));
                    match funct3 {
                        0 => (x == y) as u64,
                        1 => (!x.is_nan() && !y.is_nan() && x < y) as u64,
                        2 => (!x.is_nan() && !y.is_nan() && x <= y) as u64,
                        _ => return None,
                    }
                } else {
                    let (x, y) = (f64_of(a), f64_of(b));
                    match funct3 {
                        0 => (x == y) as u64,
                        1 => (!x.is_nan() && !y.is_nan() && x < y) as u64,
                        2 => (!x.is_nan() && !y.is_nan() && x <= y) as u64,
                        _ => return None,
                    }
                };
                FpResult {
                    fp: None,
                    int: Some(result),
                }
            }
            0x60 | 0x61 => {
                let single = funct7 == 0x60;
                let (value, width, unsigned) = match rs2 {
                    0 => (
                        if single {
                            f64::from(f32_of(a))
                        } else {
                            f64_of(a)
                        },
                        32,
                        false,
                    ),
                    1 => (
                        if single {
                            f64::from(f32_of(a))
                        } else {
                            f64_of(a)
                        },
                        32,
                        true,
                    ),
                    2 => (
                        if single {
                            f64::from(f32_of(a))
                        } else {
                            f64_of(a)
                        },
                        64,
                        false,
                    ),
                    3 => (
                        if single {
                            f64::from(f32_of(a))
                        } else {
                            f64_of(a)
                        },
                        64,
                        true,
                    ),
                    _ => return None,
                };
                let (converted, flags) = float_to_int_flags(value, width, unsigned);
                self.fp_flags(flags);
                FpResult {
                    fp: None,
                    int: Some(converted),
                }
            }
            0x68 | 0x69 => {
                let double = funct7 == 0x69;
                let value = self.regs[rs1];
                let (converted, integer_unsigned, width) = match rs2 {
                    0 => (value as i32 as f64, false, 32),
                    1 => (value as u32 as f64, true, 32),
                    2 => (value as i64 as f64, false, 64),
                    3 => (value as f64, true, 64),
                    _ => return None,
                };
                let _ = (integer_unsigned, width);
                let bits = if double {
                    converted.to_bits()
                } else {
                    nan_box_f32((converted as f32).to_bits())
                };
                FpResult {
                    fp: Some(bits),
                    int: None,
                }
            }
            0x70 | 0x71 => {
                let single = funct7 == 0x70;
                let bits = if single { a as u32 as u64 } else { a };
                let value = match funct3 {
                    0 => {
                        if single {
                            sign_extend_32(bits as u32)
                        } else {
                            bits
                        }
                    }
                    1 => fclass(bits, if single { 32 } else { 64 }),
                    _ => return None,
                };
                FpResult {
                    fp: None,
                    int: Some(value),
                }
            }
            0x78 | 0x79 => {
                // FMV.W.X / FMV.D.X take their source from the integer
                // register file (rs1), not from a floating-point register.
                let source = self.regs[rs1];
                let bits = if funct7 == 0x78 {
                    nan_box_f32(source as u32)
                } else {
                    source
                };
                FpResult {
                    fp: Some(bits),
                    int: None,
                }
            }
            _ => return None,
        };
        Some(result)
    }

    fn fp_binary_s(
        &mut self,
        a: u64,
        b: u64,
        op: impl Fn(f32, f32) -> f32,
        divide: bool,
    ) -> FpResult {
        let (x, y) = (f32_of(a), f32_of(b));
        if divide && y == 0.0 {
            self.fp_flags(if x == 0.0 { 1 << 4 } else { 1 << 3 });
        }
        let result = op(x, y);
        if result.is_infinite() && x.is_finite() && y.is_finite() {
            self.fp_flags(1 << 2);
        }
        if result.is_nan() && !x.is_nan() && !y.is_nan() {
            self.fp_flags(1 << 4);
        }
        FpResult {
            fp: Some(nan_box_f32(result.to_bits())),
            int: None,
        }
    }

    fn fp_binary_d(
        &mut self,
        a: u64,
        b: u64,
        op: impl Fn(f64, f64) -> f64,
        divide: bool,
    ) -> FpResult {
        let (x, y) = (f64_of(a), f64_of(b));
        if divide && y == 0.0 {
            self.fp_flags(if x == 0.0 { 1 << 4 } else { 1 << 3 });
        }
        let result = op(x, y);
        if result.is_infinite() && x.is_finite() && y.is_finite() {
            self.fp_flags(1 << 2);
        }
        if result.is_nan() && !x.is_nan() && !y.is_nan() {
            self.fp_flags(1 << 4);
        }
        FpResult {
            fp: Some(result.to_bits()),
            int: None,
        }
    }

    fn execute_fma(&mut self, instruction: u32) -> Option<u64> {
        let funct7 = instruction >> 25;
        let rs3 = ((instruction >> 27) & 0x1f) as usize;
        let rs2 = ((instruction >> 20) & 0x1f) as usize;
        let rs1 = ((instruction >> 15) & 0x1f) as usize;
        let format = funct7 & 0x3;
        if format > 1 {
            return None;
        }
        let single = format == 0;
        let a = self.fregs[rs1];
        let b = self.fregs[rs2];
        let c = self.fregs[rs3];
        // FMA sign is encoded in the opcode: FMADD/FMSUB/FNMSUB/FNMADD.
        let opcode = instruction & 0x7f;
        let negate_product = matches!(opcode, 0x4b | 0x4f);
        let negate_addend = matches!(opcode, 0x47 | 0x4f);
        if single {
            let (x, y) = (f32_of(a), f32_of(b));
            let addend = f32_of(c);
            let x = if negate_product { -x } else { x };
            let addend = if negate_addend { -addend } else { addend };
            Some(nan_box_f32(x.mul_add(y, addend).to_bits()))
        } else {
            let (x, y) = (f64_of(a), f64_of(b));
            let addend = f64_of(c);
            let x = if negate_product { -x } else { x };
            let addend = if negate_addend { -addend } else { addend };
            Some(x.mul_add(y, addend).to_bits())
        }
    }
}

struct FpResult {
    fp: Option<u64>,
    int: Option<u64>,
}

fn sign_extend_8(value: u8) -> u64 {
    (value as i8 as i64) as u64
}
fn sign_extend_16(value: u16) -> u64 {
    (value as i16 as i64) as u64
}
fn sign_extend_32(value: u32) -> u64 {
    (value as i32 as i64) as u64
}
fn sign_extend_12(value: u32) -> u64 {
    // The immediate occupies bits 11:0; sign-extend from bit 11 (not bit 31).
    sign_extend(u64::from(value & 0xfff), 11)
}
fn sign_extend(value: u64, bit: u32) -> u64 {
    let shift = 63 - bit;
    (((value << shift) as i64) >> shift) as u64
}
fn jump_immediate(instruction: u32) -> u64 {
    sign_extend(
        (((instruction >> 31) & 1) << 20
            | ((instruction >> 12) & 0xff) << 12
            | ((instruction >> 20) & 1) << 11
            | ((instruction >> 21) & 0x3ff) << 1) as u64,
        20,
    )
}
fn branch_immediate(instruction: u32) -> u64 {
    sign_extend(
        (((instruction >> 31) & 1) << 12
            | ((instruction >> 7) & 1) << 11
            | ((instruction >> 25) & 0x3f) << 5
            | ((instruction >> 8) & 0xf) << 1) as u64,
        12,
    )
}
fn div_signed(a: u64, b: u64) -> u64 {
    let (a, b) = (a as i64, b as i64);
    if b == 0 {
        u64::MAX
    } else if a == i64::MIN && b == -1 {
        a as u64
    } else {
        (a / b) as u64
    }
}
fn div_unsigned(a: u64, b: u64) -> u64 {
    if b == 0 {
        u64::MAX
    } else {
        a / b
    }
}
fn rem_signed(a: u64, b: u64) -> u64 {
    let (a, b) = (a as i64, b as i64);
    if b == 0 {
        a as u64
    } else if a == i64::MIN && b == -1 {
        0
    } else {
        (a % b) as u64
    }
}
fn rem_unsigned(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        a % b
    }
}
fn div_signed_32(a: u64, b: u64) -> u32 {
    let (a, b) = (a as u32 as i32, b as u32 as i32);
    if b == 0 {
        u32::MAX
    } else if a == i32::MIN && b == -1 {
        a as u32
    } else {
        (a / b) as u32
    }
}
fn div_unsigned_32(a: u64, b: u64) -> u32 {
    let (a, b) = (a as u32, b as u32);
    if b == 0 {
        u32::MAX
    } else {
        a / b
    }
}
fn rem_signed_32(a: u64, b: u64) -> u32 {
    let (a, b) = (a as u32 as i32, b as u32 as i32);
    if b == 0 {
        a as u32
    } else if a == i32::MIN && b == -1 {
        0
    } else {
        (a % b) as u32
    }
}
fn rem_unsigned_32(a: u64, b: u64) -> u32 {
    let (a, b) = (a as u32, b as u32);
    if b == 0 {
        a
    } else {
        a % b
    }
}
fn nan_box_f32(bits: u32) -> u64 {
    0xffff_ffff_0000_0000 | u64::from(bits)
}
fn f32_of(bits: u64) -> f32 {
    if bits >> 32 != 0xffff_ffff {
        f32::NAN
    } else {
        f32::from_bits(bits as u32)
    }
}
fn f64_of(bits: u64) -> f64 {
    f64::from_bits(bits)
}
fn fmin32(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::NAN
    } else if a == 0.0 && b == 0.0 {
        if a.is_sign_negative() {
            a
        } else {
            b
        }
    } else if a < b {
        a
    } else {
        b
    }
}
fn fmax32(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::NAN
    } else if a == 0.0 && b == 0.0 {
        if a.is_sign_positive() {
            a
        } else {
            b
        }
    } else if a > b {
        a
    } else {
        b
    }
}
fn fmin64(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a == 0.0 && b == 0.0 {
        if a.is_sign_negative() {
            a
        } else {
            b
        }
    } else if a < b {
        a
    } else {
        b
    }
}
fn fmax64(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a == 0.0 && b == 0.0 {
        if a.is_sign_positive() {
            a
        } else {
            b
        }
    } else if a > b {
        a
    } else {
        b
    }
}
fn float_to_int_flags(value: f64, width: u32, unsigned: bool) -> (u64, u64) {
    const NV: u64 = 1 << 4;
    const NX: u64 = 1;
    let max_unsigned = if width == 32 {
        u64::from(u32::MAX)
    } else {
        u64::MAX
    };
    let max_signed = if width == 32 {
        i32::MAX as i64 as u64
    } else {
        i64::MAX as u64
    };
    let min_signed = if width == 32 {
        i32::MIN as i64 as u64
    } else {
        i64::MIN as u64
    };
    if value.is_nan() {
        return (if unsigned { max_unsigned } else { max_signed }, NV);
    }
    if unsigned {
        if value < 0.0 {
            return (0, NV);
        }
        let limit = if width == 32 {
            4_294_967_296.0
        } else {
            18_446_744_073_709_551_616.0
        };
        if value >= limit {
            return (max_unsigned, NV);
        }
        let truncated = value.trunc();
        let result = truncated as u64;
        return (result, if truncated != value { NX } else { 0 });
    }
    let limit = if width == 32 {
        2_147_483_648.0
    } else {
        9_223_372_036_854_775_808.0
    };
    if value >= limit {
        return (max_signed, NV);
    }
    if value < -limit {
        return (min_signed, NV);
    }
    let truncated = value.trunc();
    let result = if width == 32 {
        truncated as i32 as i64 as u64
    } else {
        truncated as i64 as u64
    };
    (result, if truncated != value { NX } else { 0 })
}

fn fclass(bits: u64, width: u32) -> u64 {
    let (sign, exponent, fraction, quiet, max_exponent) = if width == 32 {
        (
            (bits >> 31) & 1,
            (bits >> 23) & 0xff,
            bits & 0x7f_ffff,
            (bits >> 22) & 1,
            0xff,
        )
    } else {
        (
            (bits >> 63) & 1,
            (bits >> 52) & 0x7ff,
            bits & 0xf_ffff_ffff_ffff,
            (bits >> 51) & 1,
            0x7ff,
        )
    };
    if exponent == max_exponent {
        if fraction == 0 {
            // Bit 0: -inf, bit 7: +inf.
            return 1 << if sign == 1 { 0 } else { 7 };
        }
        // Bit 8: signaling NaN, bit 9: quiet NaN (sign is ignored).
        return 1 << if quiet == 1 { 9 } else { 8 };
    }
    if exponent == 0 {
        if fraction == 0 {
            // Bit 3: -zero, bit 4: +zero.
            return 1 << if sign == 1 { 3 } else { 4 };
        }
        // Bit 2: -subnormal, bit 5: +subnormal.
        return 1 << if sign == 1 { 2 } else { 5 };
    }
    // Bit 1: -normal, bit 6: +normal.
    1 << if sign == 1 { 1 } else { 6 }
}
