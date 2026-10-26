use crate::SparseMemory;

pub const REG_AX: usize = 0;
pub const REG_CX: usize = 1;
pub const REG_DX: usize = 2;
pub const REG_BX: usize = 3;
pub const REG_SP: usize = 4;
pub const REG_BP: usize = 5;
pub const REG_SI: usize = 6;
pub const REG_DI: usize = 7;

pub const SEG_ES: usize = 0;
pub const SEG_CS: usize = 1;
pub const SEG_SS: usize = 2;
pub const SEG_DS: usize = 3;

pub const FLAG_CF: u16 = 1 << 0;
pub const FLAG_PF: u16 = 1 << 2;
pub const FLAG_AF: u16 = 1 << 4;
pub const FLAG_ZF: u16 = 1 << 6;
pub const FLAG_SF: u16 = 1 << 7;
pub const FLAG_TF: u16 = 1 << 8;
pub const FLAG_IF: u16 = 1 << 9;
pub const FLAG_DF: u16 = 1 << 10;
pub const FLAG_OF: u16 = 1 << 11;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum I8086Stop {
    Running,
    Halt,
    Interrupt(u8),
    DivideOverflow,
    InvalidOpcode(u8),
    MaxInstructions,
}

#[derive(Clone, Debug)]
pub struct I8086Machine {
    pub memory: SparseMemory,
    pub regs: [u16; 8],
    pub segments: [u16; 4],
    pub ip: u16,
    pub flags: u16,
    pub stop: I8086Stop,
    pub instructions_retired: u64,
    pub output: Vec<u8>,
    pub exit_code: Option<i32>,
    io_ports: [u8; 0x10000],
}

#[derive(Clone, Copy, Debug)]
enum Operand {
    Reg8(usize),
    Reg16(usize),
    Immediate(u16),
    Memory { segment: usize, offset: u16 },
}

impl I8086Machine {
    pub fn com(program: &[u8], command_tail: &str) -> Result<Self, String> {
        if program.len() > 0xff00 {
            return Err("COM image exceeds one 64 KiB segment".into());
        }
        let mut memory = SparseMemory::new();
        memory.write_slice(0x100, program);
        let bytes = command_tail.as_bytes();
        if bytes.len() > 126 {
            return Err("command tail exceeds 126 bytes".into());
        }
        memory.write_u8(0x80, bytes.len() as u8);
        memory.write_slice(0x81, bytes);
        memory.write_u8(0x81 + bytes.len() as u32, 0x0d);
        memory.write_le_u16(0, 0x20cd); // INT 20h
        let mut machine = Self::new(memory);
        machine.ip = 0x100;
        machine.regs[REG_SP] = 0xfffe;
        machine.write_word_phys(0xfffe, 0);
        machine.flags = FLAG_IF;
        Ok(machine)
    }

    pub fn mz(program: &[u8], load_segment: u16) -> Result<Self, String> {
        if program.len() < 0x1e || (program[0] != b'M' || program[1] != b'Z') {
            return Err("not an MZ executable".into());
        }
        let word =
            |offset: usize| -> u16 { u16::from_le_bytes([program[offset], program[offset + 1]]) };
        let last_page = word(2);
        let pages = word(4);
        let image_len = if last_page == 0 {
            pages as usize * 512
        } else {
            (pages as usize - 1) * 512 + last_page as usize
        };
        let relocation_count = word(6) as usize;
        let header_paragraphs = word(8) as usize;
        let header_len = header_paragraphs * 16;
        if image_len > program.len() || header_len > image_len {
            return Err("MZ lengths exceed file".into());
        }
        let mut memory = SparseMemory::new();
        let image_base = (load_segment as u32) << 4;
        memory.write_slice(image_base, &program[..image_len]);
        let relocation_offset = word(0x18) as usize;
        for index in 0..relocation_count {
            let offset = relocation_offset + index * 4;
            if offset + 4 > header_len {
                return Err("MZ relocation exceeds header".into());
            }
            let item_offset = word(offset);
            let item_segment = word(offset + 2);
            let address = image_base
                .wrapping_add(((item_segment as u32) << 4) + item_offset as u32)
                & 0xff_ffff;
            let value = memory.read_le_u16(address).wrapping_add(load_segment);
            memory.write_le_u16(address, value);
        }
        let mut machine = Self::new(memory);
        machine.segments[SEG_CS] = load_segment
            .wrapping_add(header_paragraphs as u16)
            .wrapping_add(word(0x16));
        machine.ip = word(0x14);
        machine.segments[SEG_SS] = load_segment
            .wrapping_add(header_paragraphs as u16)
            .wrapping_add(word(0x0e));
        machine.regs[REG_SP] = word(0x10);
        machine.segments[SEG_DS] = load_segment;
        machine.segments[SEG_ES] = load_segment;
        machine.flags = FLAG_IF;
        Ok(machine)
    }

    fn new(memory: SparseMemory) -> Self {
        Self {
            memory,
            regs: [0; 8],
            segments: [0; 4],
            ip: 0,
            flags: 0,
            stop: I8086Stop::Running,
            instructions_retired: 0,
            output: Vec::new(),
            exit_code: None,
            io_ports: [0; 0x10000],
        }
    }

    pub fn run(&mut self, max_instructions: u64) -> I8086Stop {
        let start = self.instructions_retired;
        while self.stop == I8086Stop::Running
            && self.instructions_retired - start < max_instructions
        {
            self.step();
        }
        if self.stop == I8086Stop::Running {
            self.stop = I8086Stop::MaxInstructions;
        }
        self.stop
    }

    pub fn step(&mut self) -> I8086Stop {
        if self.stop != I8086Stop::Running {
            return self.stop;
        }
        let mut segment_override: Option<usize> = None;
        let mut repeat: Option<u8> = None;
        let mut prefix = true;
        while prefix {
            match self.fetch_u8() {
                0xf0 => {} // LOCK: no bus arbitration in the sequential model
                0xf2 => repeat = Some(0xf2),
                0xf3 => repeat = Some(0xf3),
                0x26 => segment_override = Some(SEG_ES),
                0x2e => segment_override = Some(SEG_CS),
                0x36 => segment_override = Some(SEG_SS),
                0x3e => segment_override = Some(SEG_DS),
                _ => {
                    self.ip = self.ip.wrapping_sub(1);
                    prefix = false;
                }
            }
        }
        let opcode = self.fetch_u8();
        let _ = self.execute(opcode, segment_override, repeat);
        if self.stop == I8086Stop::Running {
            self.instructions_retired = self.instructions_retired.wrapping_add(1);
        }
        self.stop
    }

    fn fetch_u8(&mut self) -> u8 {
        let value = self.memory.read_u8(self.physical(SEG_CS, self.ip));
        self.ip = self.ip.wrapping_add(1);
        value
    }

    fn fetch_u16(&mut self) -> u16 {
        let low = self.fetch_u8() as u16;
        let high = self.fetch_u8() as u16;
        (high << 8) | low
    }

    fn physical(&self, segment: usize, offset: u16) -> u32 {
        (((self.segments[segment] as u32) << 4).wrapping_add(offset as u32)) & 0xff_ffff
    }

    fn read_u8(&self, segment: usize, offset: u16) -> u8 {
        self.memory.read_u8(self.physical(segment, offset))
    }

    fn read_u16(&self, segment: usize, offset: u16) -> u16 {
        self.memory.read_le_u16(self.physical(segment, offset))
    }

    fn write_u8(&mut self, segment: usize, offset: u16, value: u8) {
        let address = self.physical(segment, offset);
        self.memory.write_u8(address, value);
    }

    fn write_u16(&mut self, segment: usize, offset: u16, value: u16) {
        let address = self.physical(segment, offset);
        self.memory.write_le_u16(address, value);
    }

    fn write_word_phys(&mut self, address: u32, value: u16) {
        self.memory.write_le_u16(address & 0xff_ffff, value);
    }

    fn push_u16(&mut self, value: u16) {
        self.regs[REG_SP] = self.regs[REG_SP].wrapping_sub(2);
        let address = self.physical(SEG_SS, self.regs[REG_SP]);
        self.memory.write_le_u16(address, value);
    }

    fn pop_u16(&mut self) -> u16 {
        let value = self.read_u16(SEG_SS, self.regs[REG_SP]);
        self.regs[REG_SP] = self.regs[REG_SP].wrapping_add(2);
        value
    }

    fn reg8(&self, index: usize) -> u8 {
        if index >= 4 {
            (self.regs[index & 3] >> 8) as u8
        } else {
            self.regs[index & 3] as u8
        }
    }

    fn set_reg8(&mut self, index: usize, value: u8) {
        if index >= 4 {
            self.regs[index & 3] = (self.regs[index & 3] & 0x00ff) | ((value as u16) << 8);
        } else {
            self.regs[index & 3] = (self.regs[index & 3] & 0xff00) | value as u16;
        }
    }

    fn alu(&mut self, operation: u8, a: u16, b: u16, width: u8) -> u16 {
        let mask = if width == 8 { 0x00ff } else { 0xffff };
        let sign = 1 << (width - 1);
        let a = a & mask;
        let b = b & mask;
        let (raw, add_carry) = match operation {
            0 | 2 => a.overflowing_add(b),
            1 => (a | b, false),
            4 => (a & b, false),
            6 => (a ^ b, false),
            3 | 5 | 7 => a.overflowing_sub(b),
            _ => (a, false),
        };
        let carry_in = u16::from(self.get_flag(FLAG_CF));
        let result = match operation {
            2 => raw.wrapping_add(carry_in),
            // SBB subtracts both the operand and the incoming carry.
            3 => raw.wrapping_sub(carry_in),
            _ => raw,
        } & mask;
        let carry = match operation {
            0 => u32::from(a) + u32::from(b) > u32::from(mask),
            2 => u32::from(a) + u32::from(b) + u32::from(carry_in) > u32::from(mask),
            3 => u64::from(a) < u64::from(b) + u64::from(carry_in),
            7 => add_carry,
            5 => i32::from(a) - i32::from(b) < 0,
            _ => false,
        };
        let overflow = match operation {
            0 | 2 => (!(a ^ b) & (a ^ result) & sign) != 0,
            3 | 5 | 7 => ((a ^ b) & (a ^ result) & sign) != 0,
            _ => false,
        };
        self.set_flag(FLAG_CF, carry);
        self.set_flag(FLAG_OF, overflow);
        self.set_flag(FLAG_SF, result & sign != 0);
        self.set_flag(FLAG_ZF, result == 0);
        self.set_flag(FLAG_PF, parity(result as u8));
        self.set_flag(
            FLAG_AF,
            matches!(operation, 0 | 2 | 3 | 5 | 7) && (a ^ b ^ result) & 0x10 != 0,
        );
        result
    }

    fn set_flag(&mut self, flag: u16, value: bool) {
        if value {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }

    fn get_flag(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }

    fn condition(&self, code: u8) -> bool {
        match code {
            0 => self.get_flag(FLAG_OF),
            1 => !self.get_flag(FLAG_OF),
            2 => self.get_flag(FLAG_CF),
            3 => !self.get_flag(FLAG_CF),
            4 => self.get_flag(FLAG_ZF),
            5 => !self.get_flag(FLAG_ZF),
            6 => self.get_flag(FLAG_CF) || self.get_flag(FLAG_ZF),
            7 => !self.get_flag(FLAG_CF) && !self.get_flag(FLAG_ZF),
            8 => self.get_flag(FLAG_SF),
            9 => !self.get_flag(FLAG_SF),
            0xa => self.get_flag(FLAG_PF),
            0xb => !self.get_flag(FLAG_PF),
            0xc => self.get_flag(FLAG_SF) != self.get_flag(FLAG_OF),
            0xd => self.get_flag(FLAG_SF) == self.get_flag(FLAG_OF),
            0xe => self.get_flag(FLAG_ZF) || self.get_flag(FLAG_SF) != self.get_flag(FLAG_OF),
            _ => !self.get_flag(FLAG_ZF) && self.get_flag(FLAG_SF) == self.get_flag(FLAG_OF),
        }
    }

    fn handle_interrupt(&mut self, vector: u8) {
        match vector {
            0x20 => {
                self.exit_code = Some(0);
                self.stop = I8086Stop::Halt;
                return;
            }
            0x21 if self.handle_dos21() => return,
            0x10 if (self.regs[REG_AX] >> 8) as u8 == 0x0e => {
                let character = self.regs[REG_AX] as u8;
                self.output.push(character);
                return;
            }
            _ => {}
        }
        let address = u32::from(vector) * 4;
        let target = self.memory.read_le_u16(address);
        let target_segment = self.memory.read_le_u16(address + 2);
        self.push_u16(self.flags);
        self.push_u16(self.segments[SEG_CS]);
        self.push_u16(self.ip);
        self.set_flag(FLAG_IF, false);
        self.set_flag(FLAG_TF, false);
        self.segments[SEG_CS] = target_segment;
        self.ip = target;
    }

    fn handle_dos21(&mut self) -> bool {
        let ah = (self.regs[REG_AX] >> 8) as u8;
        match ah {
            0x00 => {
                self.exit_code = Some(0);
                self.stop = I8086Stop::Halt;
            }
            0x02 | 0x06 => {
                let character = self.regs[REG_DX] as u8;
                self.output.push(character);
                self.set_reg8(0, character);
            }
            0x09 => {
                let mut offset = self.regs[REG_DX];
                loop {
                    let character = self.read_u8(SEG_DS, offset);
                    if character == b'$' {
                        break;
                    }
                    self.output.push(character);
                    offset = offset.wrapping_add(1);
                }
                self.set_reg8(0, b'$');
            }
            0x25 | 0x26 => {
                let vector = self.regs[REG_AX] as u8;
                let address = u32::from(vector) * 4;
                self.memory.write_le_u16(address, self.regs[REG_DX]);
                self.memory.write_le_u16(address + 2, self.segments[SEG_DS]);
            }
            0x30 => {
                self.regs[REG_AX] = 0x0005;
                self.regs[REG_BX] = 0;
                self.regs[REG_CX] = 0;
            }
            0x40 => {
                let handle = self.regs[REG_BX];
                let length = self.regs[REG_CX] as usize;
                let offset = self.regs[REG_DX];
                if handle == 1 || handle == 2 {
                    for index in 0..length {
                        self.output
                            .push(self.read_u8(SEG_DS, offset.wrapping_add(index as u16)));
                    }
                    self.regs[REG_AX] = length as u16;
                    self.set_flag(FLAG_CF, false);
                } else {
                    self.regs[REG_AX] = 6;
                    self.set_flag(FLAG_CF, true);
                }
            }
            0x4c => {
                self.exit_code = Some(self.regs[REG_AX] as u8 as i32);
                self.stop = I8086Stop::Halt;
            }
            _ => return false,
        }
        true
    }

    fn modrm(&mut self, override_segment: Option<usize>) -> (Operand, usize) {
        let byte = self.fetch_u8();
        let mode = byte >> 6;
        let register = ((byte >> 3) & 7) as usize;
        let rm = (byte & 7) as usize;
        if mode == 3 {
            return (Operand::Reg16(rm), register);
        }
        let default_segment = match (mode, rm) {
            (0, 2 | 3) | (_, 2 | 3) => SEG_SS,
            _ => SEG_DS,
        };
        let mut offset = match (mode, rm) {
            (0, 0) => self.regs[REG_BX].wrapping_add(self.regs[REG_SI]),
            (0, 1) => self.regs[REG_BX].wrapping_add(self.regs[REG_DI]),
            (0, 2) => self.regs[REG_BP].wrapping_add(self.regs[REG_SI]),
            (0, 3) => self.regs[REG_BP].wrapping_add(self.regs[REG_DI]),
            (0, 4) => self.regs[REG_SI],
            (0, 5) => self.regs[REG_DI],
            (0, 6) => self.fetch_u16(),
            (0, 7) => self.regs[REG_BX],
            (_, 0) => self.regs[REG_BX].wrapping_add(self.regs[REG_SI]),
            (_, 1) => self.regs[REG_BX].wrapping_add(self.regs[REG_DI]),
            (_, 2) => self.regs[REG_BP].wrapping_add(self.regs[REG_SI]),
            (_, 3) => self.regs[REG_BP].wrapping_add(self.regs[REG_DI]),
            (_, 4) => self.regs[REG_SI],
            (_, 5) => self.regs[REG_DI],
            (_, 6) => self.regs[REG_BP],
            _ => self.regs[REG_BX],
        };
        if mode == 1 {
            let displacement = self.fetch_u8() as i8 as i16 as u16;
            offset = offset.wrapping_add(displacement);
        } else if mode == 2 {
            offset = offset.wrapping_add(self.fetch_u16());
        }
        (
            Operand::Memory {
                segment: override_segment.unwrap_or(default_segment),
                offset,
            },
            register,
        )
    }

    fn read_operand(&self, operand: Operand, width: u8) -> u16 {
        match operand {
            Operand::Reg8(index) => self.reg8(index) as u16,
            Operand::Reg16(index) if width == 8 => self.reg8(index) as u16,
            Operand::Reg16(index) => self.regs[index],
            Operand::Immediate(value) => value,
            Operand::Memory { segment, offset } => {
                if width == 8 {
                    self.read_u8(segment, offset) as u16
                } else {
                    self.read_u16(segment, offset)
                }
            }
        }
    }

    fn write_operand(&mut self, operand: Operand, width: u8, value: u16) {
        match operand {
            Operand::Reg8(index) => self.set_reg8(index, value as u8),
            Operand::Reg16(index) if width == 8 => self.set_reg8(index, value as u8),
            Operand::Reg16(index) => self.regs[index] = value,
            Operand::Immediate(_) => {}
            Operand::Memory { segment, offset } => {
                if width == 8 {
                    self.write_u8(segment, offset, value as u8);
                } else {
                    self.write_u16(segment, offset, value);
                }
            }
        }
    }

    fn execute(
        &mut self,
        opcode: u8,
        segment_override: Option<usize>,
        _repeat: Option<u8>,
    ) -> I8086Stop {
        // Arithmetic opcodes occupy the regular pattern 00h..3Dh. Each group
        // has rm,r / r,rm / accumulator-immediate encodings.
        if opcode <= 0x3d && (opcode & 7) <= 5 && (opcode < 0x40) {
            let operation = (opcode >> 3) & 7;
            let width = if opcode & 1 == 0 { 8 } else { 16 };
            let variant = opcode & 7;
            let (destination, source) = match variant {
                0 => {
                    let (destination, register) = self.modrm(segment_override);
                    let source = if width == 8 {
                        Operand::Reg8(register)
                    } else {
                        Operand::Reg16(register)
                    };
                    (destination, source)
                }
                1 => {
                    // Bit 0 selects width; bit 1 selects direction. Both
                    // 00h/01h forms therefore write the r/m operand.
                    let (destination, register) = self.modrm(segment_override);
                    let source = if width == 8 {
                        Operand::Reg8(register)
                    } else {
                        Operand::Reg16(register)
                    };
                    (destination, source)
                }
                2 => {
                    let (source, register) = self.modrm(segment_override);
                    let destination = if width == 8 {
                        Operand::Reg8(register)
                    } else {
                        Operand::Reg16(register)
                    };
                    (destination, source)
                }
                3 => {
                    // Both 02h/03h forms write the register operand.
                    let (source, register) = self.modrm(segment_override);
                    let destination = if width == 8 {
                        Operand::Reg8(register)
                    } else {
                        Operand::Reg16(register)
                    };
                    (destination, source)
                }
                4 => (
                    if width == 8 {
                        Operand::Reg8(0)
                    } else {
                        Operand::Reg16(REG_AX)
                    },
                    Operand::Immediate(self.fetch_u8() as u16),
                ),
                _ => (
                    if width == 8 {
                        Operand::Reg8(0)
                    } else {
                        Operand::Reg16(REG_AX)
                    },
                    Operand::Immediate(self.fetch_u16()),
                ),
            };
            let a = self.read_operand(destination, width);
            let b = self.read_operand(source, width);
            let result = self.alu(operation, a, b, width);
            if operation != 7 {
                self.write_operand(destination, width, result);
            }
            return I8086Stop::Running;
        }

        match opcode {
            0x06 => self.push_u16(self.segments[SEG_ES]),
            0x07 => self.segments[SEG_ES] = self.pop_u16(),
            0x0e => self.push_u16(self.segments[SEG_CS]),
            0x16 => self.push_u16(self.segments[SEG_SS]),
            0x17 => self.segments[SEG_SS] = self.pop_u16(),
            0x1e => self.push_u16(self.segments[SEG_DS]),
            0x1f => self.segments[SEG_DS] = self.pop_u16(),
            0x27 => self.daa(),
            0x2f => self.das(),
            0x37 => self.aaa(),
            0x3f => self.aas(),
            0x40..=0x47 => {
                let register = (opcode & 7) as usize;
                let value = self.regs[register];
                self.regs[register] = self.inc_dec(value, 16, true);
            }
            0x48..=0x4f => {
                let register = (opcode & 7) as usize;
                let value = self.regs[register];
                self.regs[register] = self.inc_dec(value, 16, false);
            }
            0x50..=0x57 => {
                let value = self.regs[(opcode & 7) as usize];
                self.push_u16(value);
            }
            0x58..=0x5f => {
                let value = self.pop_u16();
                self.regs[(opcode & 7) as usize] = value;
            }
            0x70..=0x7f => {
                let displacement = self.fetch_u8() as i8 as i16 as u16;
                if self.condition(opcode & 0xf) {
                    self.ip = self.ip.wrapping_add(displacement);
                }
            }
            0x80..=0x83 => {
                let width = if opcode == 0x81 || opcode == 0x83 {
                    16
                } else {
                    8
                };
                let (destination, operation) = self.modrm(segment_override);
                let immediate = if opcode == 0x81 {
                    self.fetch_u16()
                } else if opcode == 0x83 {
                    (self.fetch_u8() as i8 as i16) as u16
                } else {
                    self.fetch_u8() as u16
                };
                let a = self.read_operand(destination, width);
                let result = self.alu(operation as u8, a, immediate, width);
                if operation != 7 {
                    self.write_operand(destination, width, result);
                }
            }
            0x84 | 0x85 => {
                let width = if opcode == 0x84 { 8 } else { 16 };
                let (left, register) = self.modrm(segment_override);
                let right = if width == 8 {
                    Operand::Reg8(register)
                } else {
                    Operand::Reg16(register)
                };
                self.test(
                    self.read_operand(left, width),
                    self.read_operand(right, width),
                    width,
                );
            }
            0x86 | 0x87 => {
                let width = if opcode == 0x86 { 8 } else { 16 };
                let (memory, register) = self.modrm(segment_override);
                let register_operand = if width == 8 {
                    Operand::Reg8(register)
                } else {
                    Operand::Reg16(register)
                };
                let a = self.read_operand(memory, width);
                let b = self.read_operand(register_operand, width);
                self.write_operand(memory, width, b);
                self.write_operand(register_operand, width, a);
            }
            0x88 | 0x89 => {
                let width = if opcode == 0x88 { 8 } else { 16 };
                let (destination, register) = self.modrm(segment_override);
                let source = if width == 8 {
                    Operand::Reg8(register)
                } else {
                    Operand::Reg16(register)
                };
                self.write_operand(destination, width, self.read_operand(source, width));
            }
            0x8a | 0x8b => {
                let width = if opcode == 0x8a { 8 } else { 16 };
                let (source, register) = self.modrm(segment_override);
                let destination = if width == 8 {
                    Operand::Reg8(register)
                } else {
                    Operand::Reg16(register)
                };
                self.write_operand(destination, width, self.read_operand(source, width));
            }
            0x8c => {
                let (destination, register) = self.modrm(segment_override);
                self.write_operand(destination, 16, self.segments[register & 3]);
            }
            0x8d => {
                let (memory, register) = self.modrm(segment_override);
                if let Operand::Memory { offset, .. } = memory {
                    self.regs[register] = offset;
                }
            }
            0x8e => {
                let (source, register) = self.modrm(segment_override);
                self.segments[register & 3] = self.read_operand(source, 16);
            }
            0x8f => {
                let (destination, _) = self.modrm(segment_override);
                let value = self.pop_u16();
                self.write_operand(destination, 16, value);
            }
            0x90 => {}
            0xf4 => {
                self.stop = I8086Stop::Halt;
                return self.stop;
            }
            0x91..=0x97 => {
                let register = (opcode & 7) as usize;
                self.regs.swap(REG_AX, register);
            }
            0x98 => {
                let al = self.regs[REG_AX] as u8;
                self.regs[REG_AX] = al as i8 as i16 as u16;
            }
            0x99 => {
                self.regs[REG_DX] = if self.regs[REG_AX] & 0x8000 != 0 {
                    0xffff
                } else {
                    0
                };
            }
            0x9c => self.push_u16(self.flags),
            0x9d => self.flags = self.pop_u16(),
            0x9e => {
                let ah = (self.regs[REG_AX] >> 8) as u8;
                self.flags = (self.flags & 0xff00) | ah as u16;
            }
            0x9f => {
                let low = self.flags as u8;
                self.set_reg8(4, low);
            }
            0xa0 => {
                let offset = self.fetch_u16();
                let segment = segment_override.unwrap_or(SEG_DS);
                let value = self.read_u8(segment, offset);
                self.set_reg8(0, value);
            }
            0xa1 => {
                let offset = self.fetch_u16();
                let segment = segment_override.unwrap_or(SEG_DS);
                let value = self.read_u16(segment, offset);
                self.regs[REG_AX] = value;
            }
            0xa2 => {
                let offset = self.fetch_u16();
                let segment = segment_override.unwrap_or(SEG_DS);
                let value = self.regs[REG_AX] as u8;
                self.write_u8(segment, offset, value);
            }
            0xa3 => {
                let offset = self.fetch_u16();
                let segment = segment_override.unwrap_or(SEG_DS);
                let value = self.regs[REG_AX];
                self.write_u16(segment, offset, value);
            }
            0xa4..=0xa7 | 0xaa..=0xaf => {
                self.string_operation(opcode, segment_override, _repeat);
            }
            0xb0..=0xb7 => {
                let value = self.fetch_u8();
                self.set_reg8((opcode & 7) as usize, value);
            }
            0xb8..=0xbf => {
                let value = self.fetch_u16();
                self.regs[(opcode & 7) as usize] = value;
            }
            0xc2 => {
                let adjustment = self.fetch_u16();
                self.ip = self.pop_u16();
                self.regs[REG_SP] = self.regs[REG_SP].wrapping_add(adjustment);
            }
            0xc3 => self.ip = self.pop_u16(),
            0xc4 | 0xc5 => {
                let (operand, register) = self.modrm(segment_override);
                let Operand::Memory { segment, offset } = operand else {
                    self.stop = I8086Stop::InvalidOpcode(opcode);
                    return self.stop;
                };
                self.regs[register] = self.read_u16(segment, offset);
                let next = self.read_u16(segment, offset.wrapping_add(2));
                self.segments[if opcode == 0xc4 { SEG_ES } else { SEG_DS }] = next;
            }
            0xc6 | 0xc7 => {
                let width = if opcode == 0xc6 { 8 } else { 16 };
                let (destination, _) = self.modrm(segment_override);
                let immediate = if width == 8 {
                    self.fetch_u8() as u16
                } else {
                    self.fetch_u16()
                };
                self.write_operand(destination, width, immediate);
            }
            0xca => {
                let adjustment = self.fetch_u16();
                self.ip = self.pop_u16();
                self.segments[SEG_CS] = self.pop_u16();
                self.regs[REG_SP] = self.regs[REG_SP].wrapping_add(adjustment);
            }
            0xcb => {
                self.ip = self.pop_u16();
                self.segments[SEG_CS] = self.pop_u16();
            }
            0xcc => {
                self.handle_interrupt(3);
                if self.stop != I8086Stop::Running {
                    return self.stop;
                }
            }
            0xcd => {
                let vector = self.fetch_u8();
                self.handle_interrupt(vector);
                if self.stop != I8086Stop::Running {
                    return self.stop;
                }
            }
            0xce => {
                if self.get_flag(FLAG_OF) {
                    self.handle_interrupt(4);
                    if self.stop != I8086Stop::Running {
                        return self.stop;
                    }
                }
            }
            0xcf => {
                self.ip = self.pop_u16();
                self.segments[SEG_CS] = self.pop_u16();
                self.flags = self.pop_u16();
            }
            0xc0 | 0xc1 | 0xd0..=0xd3 => {
                let width = if opcode & 1 == 0 { 8 } else { 16 };
                let (destination, operation) = self.modrm(segment_override);
                let count = if opcode == 0xc0 || opcode == 0xc1 {
                    u16::from(self.fetch_u8())
                } else if opcode < 0xd2 {
                    1
                } else {
                    self.regs[REG_CX] as u8 as u16
                };
                let value = self.read_operand(destination, width);
                let result = self.shift_rotate(operation as u8, value, count, width);
                self.write_operand(destination, width, result);
            }
            0xd4 => {
                let base = self.fetch_u8();
                if base == 0 {
                    self.stop = I8086Stop::DivideOverflow;
                    return self.stop;
                }
                let al = self.regs[REG_AX] as u8;
                self.set_reg8(4, al / base);
                self.set_reg8(0, al % base);
            }
            0xd5 => {
                let base = self.fetch_u8();
                let al = self.regs[REG_AX] as u8;
                let ah = (self.regs[REG_AX] >> 8) as u8;
                self.set_reg8(0, al.wrapping_add(ah.wrapping_mul(base)));
                self.set_reg8(4, 0);
            }
            0xd6 => {
                let al = if self.get_flag(FLAG_CF) { 0xff } else { 0 };
                self.set_reg8(0, al);
            }
            0xd7 => {
                let offset = self.regs[REG_BX].wrapping_add(self.regs[REG_AX] as u8 as u16);
                let value = self.read_u8(SEG_DS, offset);
                self.set_reg8(0, value);
            }
            0x9a => {
                let offset = self.fetch_u16();
                let segment = self.fetch_u16();
                self.push_u16(self.segments[SEG_CS]);
                self.push_u16(self.ip);
                self.segments[SEG_CS] = segment;
                self.ip = offset;
            }
            0xe0..=0xe3 => {
                let displacement = self.fetch_u8() as i8 as i16 as u16;
                self.regs[REG_CX] = self.regs[REG_CX].wrapping_sub(1);
                let branch = match opcode {
                    0xe0 => self.regs[REG_CX] != 0 && !self.get_flag(FLAG_ZF),
                    0xe1 => self.regs[REG_CX] != 0 && self.get_flag(FLAG_ZF),
                    0xe2 => self.regs[REG_CX] != 0,
                    _ => self.regs[REG_CX] == 0,
                };
                if branch {
                    self.ip = self.ip.wrapping_add(displacement);
                }
            }
            0xe4 => {
                let port = self.fetch_u8() as usize;
                self.set_reg8(0, self.io_ports[port]);
            }
            0xe5 => {
                let port = self.fetch_u8() as usize;
                self.regs[REG_AX] =
                    u16::from_le_bytes([self.io_ports[port], self.io_ports[port + 1]]);
            }
            0xe6 => {
                let port = self.fetch_u8() as usize;
                self.io_ports[port] = self.regs[REG_AX] as u8;
            }
            0xe7 => {
                let port = self.fetch_u8() as usize;
                let value = self.regs[REG_AX].to_le_bytes();
                self.io_ports[port] = value[0];
                self.io_ports[port + 1] = value[1];
            }
            0xe8 => {
                let displacement = self.fetch_u16();
                self.push_u16(self.ip);
                self.ip = self.ip.wrapping_add(displacement);
            }
            0xe9 => {
                let displacement = self.fetch_u16();
                self.ip = self.ip.wrapping_add(displacement);
            }
            0xea => {
                let offset = self.fetch_u16();
                let segment = self.fetch_u16();
                self.ip = offset;
                self.segments[SEG_CS] = segment;
            }
            0xeb => {
                let displacement = self.fetch_u8() as i8 as i16 as u16;
                self.ip = self.ip.wrapping_add(displacement);
            }
            0xec => {
                let port = self.regs[REG_DX] as usize;
                self.set_reg8(0, self.io_ports[port]);
            }
            0xed => {
                let port = self.regs[REG_DX] as usize;
                self.regs[REG_AX] =
                    u16::from_le_bytes([self.io_ports[port], self.io_ports[port + 1]]);
            }
            0xee => {
                let port = self.regs[REG_DX] as usize;
                self.io_ports[port] = self.regs[REG_AX] as u8;
            }
            0xef => {
                let port = self.regs[REG_DX] as usize;
                let value = self.regs[REG_AX].to_le_bytes();
                self.io_ports[port] = value[0];
                self.io_ports[port + 1] = value[1];
            }
            0x9b => {}
            0xd8..=0xdf => {
                // ESC instructions are accepted and issue the ModRM bus cycle
                // without modeling an attached 8087.
                let _ = self.modrm(segment_override);
            }
            0xf5 => {
                self.set_flag(FLAG_CF, !self.get_flag(FLAG_CF));
            }
            0xf8 => self.set_flag(FLAG_CF, false),
            0xf9 => self.set_flag(FLAG_CF, true),
            0xfa => self.set_flag(FLAG_IF, false),
            0xfb => self.set_flag(FLAG_IF, true),
            0xfc => self.set_flag(FLAG_DF, false),
            0xfd => self.set_flag(FLAG_DF, true),
            0xf6 | 0xf7 => {
                let width = if opcode == 0xf6 { 8 } else { 16 };
                let (operand, operation) = self.modrm(segment_override);
                let value = self.read_operand(operand, width);
                match operation {
                    0 | 1 => {
                        let immediate = if width == 8 {
                            self.fetch_u8() as u16
                        } else {
                            self.fetch_u16()
                        };
                        self.test(value, immediate, width);
                    }
                    2 => self.write_operand(operand, width, !value & width_mask(width)),
                    3 => {
                        // NEG is zero minus the operand; unlike SBB it does
                        // not consume the incoming carry flag.
                        let result = self.alu(5, 0, value, width);
                        self.write_operand(operand, width, result);
                    }
                    4 | 5 => {
                        if width == 8 {
                            let a = self.regs[REG_AX] as u8;
                            let product = if operation == 4 {
                                u16::from(a) * value
                            } else {
                                (a as i8 as i16 as i32 * (value as u8 as i8 as i32)) as u16
                            };
                            self.regs[REG_AX] = product;
                            let overflow = if operation == 4 {
                                product >> 8 != 0
                            } else {
                                product != (product as u8 as i8 as i16 as u16)
                            };
                            self.set_flag(FLAG_CF, overflow);
                            self.set_flag(FLAG_OF, overflow);
                        } else {
                            let dividend =
                                (u32::from(self.regs[REG_DX]) << 16) | u32::from(self.regs[REG_AX]);
                            let product = if operation == 4 {
                                dividend.wrapping_mul(u32::from(value))
                            } else {
                                ((self.regs[REG_AX] as i16 as i32)
                                    .wrapping_mul(value as i16 as i32))
                                    as u32
                            };
                            self.regs[REG_AX] = product as u16;
                            self.regs[REG_DX] = (product >> 16) as u16;
                            let overflow = self.regs[REG_DX] != 0;
                            self.set_flag(FLAG_CF, overflow);
                            self.set_flag(FLAG_OF, overflow);
                        }
                    }
                    _ => {
                        if self.divide(value, width, operation == 7) {
                            return self.stop;
                        }
                    }
                }
            }
            0xfe => {
                let (operand, operation) = self.modrm(segment_override);
                let value = self.read_operand(operand, 8);
                let result = self.inc_dec(value, 8, operation == 0);
                self.write_operand(operand, 8, result);
            }
            0xff => {
                let (operand, operation) = self.modrm(segment_override);
                match operation {
                    0 | 1 => {
                        let value = self.read_operand(operand, 16);
                        let result = self.inc_dec(value, 16, operation == 0);
                        self.write_operand(operand, 16, result);
                    }
                    2 => {
                        let target = self.read_operand(operand, 16);
                        self.push_u16(self.ip);
                        self.ip = target;
                    }
                    3 => {
                        let Operand::Memory { segment, offset } = operand else {
                            self.stop = I8086Stop::InvalidOpcode(opcode);
                            return self.stop;
                        };
                        let target = self.read_u16(segment, offset);
                        let target_segment = self.read_u16(segment, offset.wrapping_add(2));
                        self.push_u16(self.segments[SEG_CS]);
                        self.push_u16(self.ip);
                        self.ip = target;
                        self.segments[SEG_CS] = target_segment;
                    }
                    4 => self.ip = self.read_operand(operand, 16),
                    5 => {
                        let Operand::Memory { segment, offset } = operand else {
                            self.stop = I8086Stop::InvalidOpcode(opcode);
                            return self.stop;
                        };
                        self.ip = self.read_u16(segment, offset);
                        self.segments[SEG_CS] = self.read_u16(segment, offset.wrapping_add(2));
                    }
                    _ => {
                        let value = self.read_operand(operand, 16);
                        self.push_u16(value);
                    }
                }
            }
            _ => {
                self.stop = I8086Stop::InvalidOpcode(opcode);
                return self.stop;
            }
        }
        self.stop
    }

    fn inc_dec(&mut self, value: u16, width: u8, increment: bool) -> u16 {
        let carry = self.get_flag(FLAG_CF);
        let result = if increment {
            self.alu(0, value, 1, width)
        } else {
            self.alu(3, value, 1, width)
        };
        self.set_flag(FLAG_CF, carry);
        result
    }

    fn test(&mut self, a: u16, b: u16, width: u8) {
        let result = a & b;
        self.set_flag(FLAG_CF, false);
        self.set_flag(FLAG_OF, false);
        self.set_flag(FLAG_AF, false);
        self.set_flag(FLAG_SF, result & (1 << (width - 1)) != 0);
        self.set_flag(FLAG_ZF, result & width_mask(width) == 0);
        self.set_flag(FLAG_PF, parity(result as u8));
    }

    fn shift_rotate(&mut self, operation: u8, value: u16, count: u16, width: u8) -> u16 {
        let count = (count & 31) as u32;
        let mask = width_mask(width);
        let value = value & mask;
        if count == 0 {
            return value;
        }
        let (result, carry) = match operation {
            0 => {
                let mut result = value;
                let mut carry = false;
                for _ in 0..count {
                    carry = result & (1 << (width - 1)) != 0;
                    result = ((result << 1) | u16::from(result >> (width - 1) != 0)) & mask;
                }
                (result, carry)
            }
            1 => {
                let mut result = value;
                let mut carry = false;
                for _ in 0..count {
                    carry = result & 1 != 0;
                    result = (result >> 1) | ((result & 1) << (width - 1));
                }
                (result, carry)
            }
            2 => {
                let mut result = value;
                let mut carry = self.get_flag(FLAG_CF);
                for _ in 0..count {
                    let next_carry = result & (1 << (width - 1)) != 0;
                    result = ((result << 1) | u16::from(carry)) & mask;
                    carry = next_carry;
                }
                self.set_flag(FLAG_CF, carry);
                if count == 1 {
                    self.set_flag(FLAG_OF, (result & (1 << (width - 1)) != 0) != carry);
                }
                return result;
            }
            3 => {
                let mut result = value;
                let mut carry = self.get_flag(FLAG_CF);
                for _ in 0..count {
                    let next_carry = result & 1 != 0;
                    result = result >> 1 | (u16::from(carry) << (width - 1));
                    carry = next_carry;
                }
                self.set_flag(FLAG_CF, carry);
                if count == 1 {
                    self.set_flag(
                        FLAG_OF,
                        (result & (1 << (width - 1)) != 0) != (result & (1 << (width - 2)) != 0),
                    );
                }
                return result;
            }
            4 | 6 => {
                let result = ((value as u32) << count) as u16 & mask;
                let bits = u32::from(width);
                let carry = count <= bits && ((value >> (bits - count)) & 1) != 0;
                (result, carry)
            }
            5 => {
                let result = (value as u32 >> count) as u16 & mask;
                (result, (value as u32 >> (count - 1)) & 1 != 0)
            }
            7 => {
                let signed = if width == 8 {
                    value as u8 as i8 as i32
                } else {
                    value as i16 as i32
                };
                let result = ((signed >> count) as u16) & mask;
                let bits = u32::from(width);
                let carry = if count <= bits {
                    (value >> (count - 1)) & 1 != 0
                } else {
                    value & (1 << (bits - 1)) != 0
                };
                (result, carry)
            }
            _ => unreachable!("shift operation is masked to three bits"),
        };
        self.set_flag(FLAG_CF, carry);
        if count == 1 {
            let overflow = match operation {
                0 => (result & (1 << (width - 1)) != 0) != carry,
                1 => (result & (1 << (width - 1)) != 0) != (result & (1 << (width - 2)) != 0),
                4 | 6 => (result & (1 << (width - 1)) != 0) != carry,
                5 => value & (1 << (width - 1)) != 0,
                _ => false,
            };
            self.set_flag(FLAG_OF, overflow);
        }
        if operation <= 3 {
            return result;
        }
        self.set_flag(FLAG_SF, result & (1 << (width - 1)) != 0);
        self.set_flag(FLAG_ZF, result == 0);
        self.set_flag(FLAG_PF, parity(result as u8));
        result
    }

    fn string_operation(
        &mut self,
        opcode: u8,
        segment_override: Option<usize>,
        repeat: Option<u8>,
    ) {
        if repeat.is_some() && self.regs[REG_CX] == 0 {
            return;
        }
        let width = if opcode & 1 == 0 { 8 } else { 16 };
        let delta = if self.get_flag(FLAG_DF) {
            -(width as i16 / 8) as u16
        } else {
            width as u16 / 8
        };
        loop {
            match opcode {
                0xa4 | 0xa5 => {
                    let source_segment = segment_override.unwrap_or(SEG_DS);
                    let value = self.read_operand(
                        Operand::Memory {
                            segment: source_segment,
                            offset: self.regs[REG_SI],
                        },
                        width,
                    );
                    self.write_operand(
                        Operand::Memory {
                            segment: SEG_ES,
                            offset: self.regs[REG_DI],
                        },
                        width,
                        value,
                    );
                    self.regs[REG_SI] = self.regs[REG_SI].wrapping_add(delta);
                    self.regs[REG_DI] = self.regs[REG_DI].wrapping_add(delta);
                }
                0xa6 | 0xa7 => {
                    let a = self.read_u16(segment_override.unwrap_or(SEG_DS), self.regs[REG_SI]);
                    let b = self.read_u16(SEG_ES, self.regs[REG_DI]);
                    self.alu(7, a, b, width);
                    self.regs[REG_SI] = self.regs[REG_SI].wrapping_add(delta);
                    self.regs[REG_DI] = self.regs[REG_DI].wrapping_add(delta);
                }
                0xaa | 0xab => {
                    let offset = self.regs[REG_DI];
                    if width == 8 {
                        let value = self.regs[REG_AX] as u8;
                        self.write_u8(SEG_ES, offset, value);
                    } else {
                        let value = self.regs[REG_AX];
                        self.write_u16(SEG_ES, offset, value);
                    }
                    self.regs[REG_DI] = self.regs[REG_DI].wrapping_add(delta);
                }
                0xac | 0xad => {
                    let segment = segment_override.unwrap_or(SEG_DS);
                    let value = self.read_u16(segment, self.regs[REG_SI]);
                    self.regs[REG_AX] = if width == 8 { value & 0xff } else { value };
                    self.regs[REG_SI] = self.regs[REG_SI].wrapping_add(delta);
                }
                _ => {
                    let b = self.read_u16(SEG_ES, self.regs[REG_DI]);
                    self.alu(7, self.regs[REG_AX], b, width);
                    self.regs[REG_DI] = self.regs[REG_DI].wrapping_add(delta);
                }
            }
            let Some(prefix) = repeat else {
                break;
            };
            self.regs[REG_CX] = self.regs[REG_CX].wrapping_sub(1);
            if self.regs[REG_CX] == 0 {
                break;
            }
            let comparison = opcode == 0xa6 || opcode == 0xa7 || opcode == 0xae || opcode == 0xaf;
            if comparison {
                let continue_repeat = if prefix == 0xf3 {
                    self.get_flag(FLAG_ZF)
                } else {
                    !self.get_flag(FLAG_ZF)
                };
                if !continue_repeat {
                    break;
                }
            }
        }
    }

    fn divide(&mut self, divisor: u16, width: u8, signed: bool) -> bool {
        if width == 8 {
            let dividend = self.regs[REG_AX];
            if divisor == 0 {
                self.stop = I8086Stop::DivideOverflow;
                return true;
            }
            if signed {
                let quotient = dividend as i16 as i32 / (divisor as u8 as i8 as i32);
                let remainder = dividend as i16 as i32 % (divisor as u8 as i8 as i32);
                if !(-128..=127).contains(&quotient) {
                    self.stop = I8086Stop::DivideOverflow;
                    return true;
                }
                self.set_reg8(0, quotient as u8);
                self.set_reg8(4, remainder as u8);
            } else {
                let quotient = dividend / divisor;
                if quotient > 0xff {
                    self.stop = I8086Stop::DivideOverflow;
                    return true;
                }
                self.set_reg8(0, quotient as u8);
                self.set_reg8(4, (dividend % divisor) as u8);
            }
        } else {
            let dividend = (u32::from(self.regs[REG_DX]) << 16) | u32::from(self.regs[REG_AX]);
            if divisor == 0 {
                self.stop = I8086Stop::DivideOverflow;
                return true;
            }
            if signed {
                let quotient = dividend as i32 / (divisor as i16 as i32);
                let remainder = dividend as i32 % (divisor as i16 as i32);
                if !(-32768..=32767).contains(&quotient) {
                    self.stop = I8086Stop::DivideOverflow;
                    return true;
                }
                self.regs[REG_AX] = quotient as u16;
                self.regs[REG_DX] = remainder as u16;
            } else {
                let quotient = dividend / u32::from(divisor);
                if quotient > 0xffff {
                    self.stop = I8086Stop::DivideOverflow;
                    return true;
                }
                self.regs[REG_AX] = quotient as u16;
                self.regs[REG_DX] = (dividend % u32::from(divisor)) as u16;
            }
        }
        false
    }

    fn daa(&mut self) {
        let old_al = self.regs[REG_AX] as u8;
        let mut al = old_al;
        if al & 0x0f > 9 || self.get_flag(FLAG_AF) {
            al = al.wrapping_add(6);
            self.set_flag(FLAG_AF, true);
        } else {
            self.set_flag(FLAG_AF, false);
        }
        if old_al > 0x99 || self.get_flag(FLAG_CF) {
            al = al.wrapping_add(0x60);
            let ah = (self.regs[REG_AX] >> 8) as u8;
            self.set_flag(FLAG_CF, true);
            self.set_reg8(4, ah.wrapping_add(1));
        } else {
            self.set_flag(FLAG_CF, false);
        }
        self.set_reg8(0, al);
        self.set_flag(FLAG_ZF, al == 0);
        self.set_flag(FLAG_SF, al & 0x80 != 0);
        self.set_flag(FLAG_PF, parity(al));
    }

    fn das(&mut self) {
        let old_al = self.regs[REG_AX] as u8;
        let mut al = old_al;
        let old_af = self.get_flag(FLAG_AF);
        if al & 0x0f > 9 || old_af {
            al = al.wrapping_sub(6);
            self.set_flag(FLAG_AF, true);
        } else {
            self.set_flag(FLAG_AF, false);
        }
        if old_al > 0x99 || self.get_flag(FLAG_CF) {
            al = al.wrapping_sub(0x60);
            let ah = (self.regs[REG_AX] >> 8) as u8;
            self.set_flag(FLAG_CF, true);
            self.set_reg8(4, ah.wrapping_sub(1));
        } else {
            self.set_flag(FLAG_CF, false);
        }
        self.set_reg8(0, al);
        self.set_flag(FLAG_ZF, al == 0);
        self.set_flag(FLAG_SF, al & 0x80 != 0);
        self.set_flag(FLAG_PF, parity(al));
    }

    fn aaa(&mut self) {
        let should_adjust = self.regs[REG_AX] as u8 & 0x0f > 9 || self.get_flag(FLAG_AF);
        if should_adjust {
            self.regs[REG_AX] = self.regs[REG_AX].wrapping_add(0x106);
        }
        self.set_flag(FLAG_AF, should_adjust);
        self.set_flag(FLAG_CF, should_adjust);
        self.set_reg8(0, (self.regs[REG_AX] as u8) & 0x0f);
    }

    fn aas(&mut self) {
        let should_adjust = self.regs[REG_AX] as u8 & 0x0f > 9 || self.get_flag(FLAG_AF);
        if should_adjust {
            self.regs[REG_AX] = self.regs[REG_AX].wrapping_sub(0x106);
        }
        self.set_flag(FLAG_AF, should_adjust);
        self.set_flag(FLAG_CF, should_adjust);
        self.set_reg8(0, (self.regs[REG_AX] as u8) & 0x0f);
    }
}

fn width_mask(width: u8) -> u16 {
    if width == 8 {
        0x00ff
    } else {
        0xffff
    }
}

fn parity(value: u8) -> bool {
    value.count_ones().is_multiple_of(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn com_loader_sets_psp_and_initial_state() {
        let cpu = I8086Machine::com(&[0xb8, 0x2a, 0x00], " hello").unwrap();
        assert_eq!(cpu.ip, 0x100);
        assert_eq!(cpu.regs[REG_SP], 0xfffe);
        assert_eq!(cpu.memory.read_u8(0x80), 6);
        assert_eq!(cpu.memory.read_u8(0x81), b' ');
    }

    #[test]
    fn com_program_prints_and_exits_through_dos21() {
        let program: &[u8] = &[
            0xb4, 0x09, // mov ah, 9
            0xba, 0x0c, 0x01, // mov dx, message
            0xcd, 0x21, // int 21h
            0xb8, 0x00, 0x4c, // mov ax, 4c00h
            0xcd, 0x21, // int 21h
            b'H', b'e', b'l', b'l', b'o', b'$',
        ];
        let mut cpu = I8086Machine::com(program, "").unwrap();
        assert_eq!(cpu.run(100), I8086Stop::Halt);
        assert_eq!(cpu.output, b"Hello");
        assert_eq!(cpu.exit_code, Some(0));
    }

    #[test]
    fn add_sets_carry_and_overflow_edges() {
        let program: &[u8] = &[
            0xb0, 0xfe, // mov al, 0feh
            0x04, 0x02, // add al, 2
            0xf4, // hlt
        ];
        let mut cpu = I8086Machine::com(program, "").unwrap();
        assert_eq!(cpu.run(10), I8086Stop::Halt);
        assert_eq!(cpu.regs[REG_AX] as u8, 0);
        assert!(cpu.get_flag(FLAG_CF));
        assert!(!cpu.get_flag(FLAG_OF));
    }

    #[test]
    fn word_alu_direction_uses_rm_and_register_forms() {
        let program: &[u8] = &[
            0xb8, 0x01, 0x00, // mov ax, 1
            0xb9, 0x02, 0x00, // mov cx, 2
            0x01, 0xc1, // add cx, ax
            0xbb, 0x03, 0x00, // mov bx, 3
            0x03, 0xc3, // add ax, bx
            0xf4, // hlt
        ];
        let mut cpu = I8086Machine::com(program, "").unwrap();
        assert_eq!(cpu.run(10), I8086Stop::Halt);
        assert_eq!(cpu.regs[REG_AX], 4);
        assert_eq!(cpu.regs[REG_CX], 3);
        assert_eq!(cpu.regs[REG_BX], 3);
    }

    #[test]
    fn sbb_consumes_carry_but_neg_does_not() {
        let sbb_program: &[u8] = &[
            0xb8, 0x07, 0x00, // mov ax, 7
            0xbb, 0x07, 0x00, // mov bx, 7
            0xf9, // stc
            0x19, 0xc3, // sbb bx, ax
            0xf4, // hlt
        ];
        let mut cpu = I8086Machine::com(sbb_program, "").unwrap();
        assert_eq!(cpu.run(10), I8086Stop::Halt);
        assert_eq!(cpu.regs[REG_BX], 0xffff);
        assert!(cpu.get_flag(FLAG_CF));

        let neg_program: &[u8] = &[
            0xb8, 0x05, 0x00, // mov ax, 5
            0xf9, // stc
            0xf7, 0xd8, // neg ax
            0xf4, // hlt
        ];
        let mut cpu = I8086Machine::com(neg_program, "").unwrap();
        assert_eq!(cpu.run(10), I8086Stop::Halt);
        assert_eq!(cpu.regs[REG_AX], 0xfffb);
        assert!(cpu.get_flag(FLAG_CF));
    }
}
