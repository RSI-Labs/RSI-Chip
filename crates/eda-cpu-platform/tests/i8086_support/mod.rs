use eda_cpu_platform::SparseMemory;

const ES: usize = 0;
const CS: usize = 1;
const SS: usize = 2;
const DS: usize = 3;

const CF: u16 = 1 << 0;
const PF: u16 = 1 << 2;
const AF: u16 = 1 << 3;
const ZF: u16 = 1 << 6;
const SF: u16 = 1 << 7;
const IF: u16 = 1 << 9;
const DF: u16 = 1 << 10;
const OF: u16 = 1 << 11;

const MEMORY_SIZE: u32 = 0x10_0000;

#[derive(Clone, Copy)]
enum Location {
    Register(u8, u8),
    Memory(usize, u16),
}

/// A small, independent reference model for the 8086 subset exercised by the
/// repository's machine-code differential suite.
///
/// This model intentionally uses a flat one-megabyte byte vector and separate
/// helper functions; it does not call `I8086Machine` or share its lowering code.
pub struct ReferenceI8086 {
    memory: Vec<u8>,
    registers: [u16; 8],
    segments: [u16; 4],
    instruction_pointer: u16,
    flags: u16,
    halted: bool,
    retired: u64,
}

impl ReferenceI8086 {
    pub fn com(initial_memory: &SparseMemory) -> Self {
        let mut memory = Vec::with_capacity(MEMORY_SIZE as usize);
        for address in 0..MEMORY_SIZE {
            memory.push(initial_memory.read_u8(address));
        }
        let mut registers = [0; 8];
        registers[4] = 0xfffe;
        Self {
            memory,
            registers,
            segments: [0; 4],
            instruction_pointer: 0x0100,
            flags: IF,
            halted: false,
            retired: 0,
        }
    }

    pub fn run(&mut self, max_instructions: u64) -> Result<(), String> {
        for _ in 0..max_instructions {
            if self.halted {
                return Ok(());
            }
            self.step()?;
        }
        if self.halted {
            Ok(())
        } else {
            Err("reference model exceeded its instruction budget".into())
        }
    }

    pub fn registers(&self) -> [u16; 8] {
        self.registers
    }

    pub fn segments(&self) -> [u16; 4] {
        self.segments
    }

    pub fn instruction_pointer(&self) -> u16 {
        self.instruction_pointer
    }

    pub fn flags(&self) -> u16 {
        self.flags
    }

    pub fn retired_instructions(&self) -> u64 {
        self.retired
    }

    pub fn memory_byte(&self, address: u32) -> u8 {
        self.memory[(address & 0xff_ffff) as usize]
    }

    pub fn step(&mut self) -> Result<(), String> {
        let mut override_segment = None;
        let opcode = loop {
            let byte = self.fetch8();
            match byte {
                0x26 => override_segment = Some(ES),
                0x2e => override_segment = Some(CS),
                0x36 => override_segment = Some(SS),
                0x3e => override_segment = Some(DS),
                other => break other,
            }
        };
        match opcode {
            0x00..=0x3f if opcode & 0x07 < 0x06 => {
                let width = if opcode & 1 == 0 { 8 } else { 16 };
                let operation = opcode >> 3;
                let variant = opcode & 0x07;
                let (destination, left, right) = if variant <= 3 {
                    let (left, right) = self.modrm_pair(override_segment);
                    let destination = if variant & 0x02 != 0 { right } else { left };
                    (
                        destination,
                        self.read(left, width)?,
                        self.read(right, width)?,
                    )
                } else {
                    let immediate = if width == 8 {
                        u16::from(self.fetch8())
                    } else {
                        self.fetch16()
                    };
                    (
                        Location::Register(0, 0),
                        self.read(Location::Register(0, 0), width)?,
                        immediate,
                    )
                };
                let result = self.alu(operation, left, right, width)?;
                if operation != 7 {
                    self.write(destination, width, result)?;
                }
            }
            0x80..=0x83 => {
                let width = if opcode == 0x81 || opcode == 0x83 {
                    16
                } else {
                    8
                };
                let (destination, operation) = self.modrm(override_segment);
                let immediate = match opcode {
                    0x81 => self.fetch16(),
                    0x83 => (self.fetch8() as i8) as u16,
                    _ => u16::from(self.fetch8()),
                };
                let result =
                    self.alu(operation, self.read(destination, width)?, immediate, width)?;
                if operation != 7 {
                    self.write(destination, width, result)?;
                }
            }
            0x88..=0x8b => {
                let width = if opcode & 1 == 0 { 8 } else { 16 };
                let (memory, register) = self.modrm(override_segment);
                if opcode & 0x02 == 0 {
                    let value = self.read(Location::Register(register, 0), width)?;
                    self.write(memory, width, value)?;
                } else {
                    let value = self.read(memory, width)?;
                    self.write(Location::Register(register, 0), width, value)?;
                }
            }
            0x50..=0x57 => {
                let value = self.read(Location::Register(opcode & 7, 0), 16)?;
                self.push(value)?;
            }
            0x58..=0x5f => {
                let value = self.pop()?;
                self.write(Location::Register(opcode & 7, 0), 16, value)?;
            }
            0x70..=0x7f => {
                let displacement = (self.fetch8() as i8) as u16;
                if self.condition(opcode & 0x0f) {
                    self.instruction_pointer = self.instruction_pointer.wrapping_add(displacement);
                }
            }
            0x90 => {}
            0x9c => {
                let flags = self.flags;
                self.push(flags)?;
            }
            0x9d => {
                self.flags = self.pop()?;
            }
            0x8e => {
                let (source, register) = self.modrm(override_segment);
                self.segments[(register & 3) as usize] = self.read(source, 16)?;
            }
            0xa0 => {
                let address = self.fetch16();
                let segment = override_segment.unwrap_or(DS);
                let value = self.read(Location::Memory(segment, address), 8)?;
                self.write(Location::Register(0, 8), 8, value)?;
            }
            0xa1 => {
                let address = self.fetch16();
                let segment = override_segment.unwrap_or(DS);
                let value = self.read(Location::Memory(segment, address), 16)?;
                self.write(Location::Register(0, 16), 16, value)?;
            }
            0xa2 => {
                let address = self.fetch16();
                let segment = override_segment.unwrap_or(DS);
                let value = self.read(Location::Register(0, 8), 8)?;
                self.write(Location::Memory(segment, address), 8, value)?;
            }
            0xa3 => {
                let address = self.fetch16();
                let segment = override_segment.unwrap_or(DS);
                let value = self.read(Location::Register(0, 16), 16)?;
                self.write(Location::Memory(segment, address), 16, value)?;
            }
            0xb0..=0xbf => {
                let width = if opcode < 0xb8 { 8 } else { 16 };
                let register = opcode & 7;
                let value = if width == 8 {
                    u16::from(self.fetch8())
                } else {
                    self.fetch16()
                };
                self.write(Location::Register(register, width), width, value)?;
            }
            0xc2 => {
                let adjustment = self.fetch16();
                self.instruction_pointer = self.pop()?;
                self.registers[4] = self.registers[4].wrapping_add(adjustment);
            }
            0xc3 => {
                self.instruction_pointer = self.pop()?;
            }
            0x9f => {
                let low = self.flags as u8;
                self.write(Location::Register(4, 8), 8, u16::from(low))?;
            }
            0xc0 | 0xc1 | 0xd2 | 0xd3 => {
                let width = if opcode == 0xc0 || opcode == 0xd2 {
                    8
                } else {
                    16
                };
                let (destination, operation) = self.modrm(override_segment);
                let count = if opcode <= 0xc1 {
                    u16::from(self.fetch8())
                } else {
                    self.registers[1]
                };
                let value = self.read(destination, width)?;
                let result = self.shift(operation, value, count, width);
                self.write(destination, width, result)?;
            }
            0xf5 => self.set_flag(CF, !self.flag(CF)),
            0xf8 => self.set_flag(CF, false),
            0xf9 => self.set_flag(CF, true),
            0xfa => self.set_flag(IF, false),
            0xfb => self.set_flag(IF, true),
            0xfc => self.set_flag(DF, false),
            0xfd => self.set_flag(DF, true),
            0xf6 | 0xf7 => {
                let width = if opcode == 0xf6 { 8 } else { 16 };
                let (operand, operation) = self.modrm(override_segment);
                self.unary_group(operand, operation, width)?;
            }
            0xe0..=0xe3 => {
                let displacement = (self.fetch8() as i8) as u16;
                let old_cx = self.registers[1];
                self.registers[1] = old_cx.wrapping_sub(1);
                let branch = match opcode {
                    0xe0 => old_cx != 0 && !self.flag(ZF),
                    0xe1 => old_cx != 0 && self.flag(ZF),
                    0xe2 => old_cx != 0,
                    _ => self.registers[1] == 0,
                };
                if branch {
                    self.instruction_pointer = self.instruction_pointer.wrapping_add(displacement);
                }
            }
            0xe8 => {
                let displacement = self.fetch16();
                let return_ip = self.instruction_pointer;
                self.push(return_ip)?;
                self.instruction_pointer = self.instruction_pointer.wrapping_add(displacement);
            }
            0xe9 => {
                let displacement = self.fetch16();
                self.instruction_pointer = self.instruction_pointer.wrapping_add(displacement);
            }
            0xeb => {
                let displacement = (self.fetch8() as i8) as u16;
                self.instruction_pointer = self.instruction_pointer.wrapping_add(displacement);
            }
            0xf4 => {
                self.halted = true;
            }
            other => {
                return Err(format!(
                    "reference model opcode {other:#04x} is unsupported"
                ))
            }
        }

        if !self.halted {
            self.retired += 1;
        }
        Ok(())
    }

    fn fetch8(&mut self) -> u8 {
        let value = self.memory[self.physical(CS, self.instruction_pointer) as usize];
        self.instruction_pointer = self.instruction_pointer.wrapping_add(1);
        value
    }

    fn fetch16(&mut self) -> u16 {
        let low = u16::from(self.fetch8());
        let high = u16::from(self.fetch8());
        (high << 8) | low
    }

    fn push(&mut self, value: u16) -> Result<(), String> {
        self.registers[4] = self.registers[4].wrapping_sub(2);
        let segment = SS;
        let offset = self.registers[4];
        self.write(Location::Memory(segment, offset), 16, value)
    }

    fn pop(&mut self) -> Result<u16, String> {
        let segment = SS;
        let offset = self.registers[4];
        let value = self.read(Location::Memory(segment, offset), 16)?;
        self.registers[4] = self.registers[4].wrapping_add(2);
        Ok(value)
    }

    fn condition(&self, code: u8) -> bool {
        match code {
            0 => self.flag(OF),
            1 => !self.flag(OF),
            2 => self.flag(CF),
            3 => !self.flag(CF),
            4 => self.flag(ZF),
            5 => !self.flag(ZF),
            6 => self.flag(CF) || self.flag(ZF),
            7 => !self.flag(CF) && !self.flag(ZF),
            8 => self.flag(SF),
            9 => !self.flag(SF),
            0xa => self.flag(PF),
            0xb => !self.flag(PF),
            0xc => self.flag(SF) != self.flag(OF),
            0xd => self.flag(SF) == self.flag(OF),
            0xe => self.flag(ZF) || self.flag(SF) != self.flag(OF),
            _ => !self.flag(ZF) && self.flag(SF) == self.flag(OF),
        }
    }

    fn modrm_pair(&mut self, override_segment: Option<usize>) -> (Location, Location) {
        let (memory, register) = self.modrm(override_segment);
        let register_location = Location::Register(register, 0);
        (memory, register_location)
    }

    fn modrm(&mut self, override_segment: Option<usize>) -> (Location, u8) {
        let byte = self.fetch8();
        let mode = byte >> 6;
        let register = (byte >> 3) & 7;
        let rm = byte & 7;
        if mode == 3 {
            return (Location::Register(rm, 0), register);
        }

        let (offset, default_segment) = if mode == 0 && rm == 6 {
            (self.fetch16(), DS)
        } else {
            let base = match rm {
                0 => self.registers[3].wrapping_add(self.registers[6]),
                1 => self.registers[3].wrapping_add(self.registers[7]),
                2 => self.registers[5].wrapping_add(self.registers[6]),
                3 => self.registers[5].wrapping_add(self.registers[7]),
                4 => self.registers[6],
                5 => self.registers[7],
                6 => self.registers[5],
                _ => self.registers[3],
            };
            let segment = if rm == 2 || rm == 3 || rm == 6 {
                SS
            } else {
                DS
            };
            let displacement = match mode {
                1 => (self.fetch8() as i8) as u16,
                2 => self.fetch16(),
                _ => 0,
            };
            (base.wrapping_add(displacement), segment)
        };
        (
            Location::Memory(override_segment.unwrap_or(default_segment), offset),
            register,
        )
    }

    fn read(&self, location: Location, width: u8) -> Result<u16, String> {
        match location {
            Location::Register(index, encoded_width) => {
                let actual_width = if encoded_width == 0 {
                    width
                } else {
                    encoded_width
                };
                Ok(if actual_width == 8 {
                    u16::from(self.register8(index))
                } else {
                    self.registers[index as usize]
                })
            }
            Location::Memory(segment, offset) => {
                let address = self.physical(segment, offset) as usize;
                Ok(if width == 8 {
                    u16::from(self.memory[address])
                } else {
                    u16::from_le_bytes([self.memory[address], self.memory[address + 1]])
                })
            }
        }
    }

    fn write(&mut self, location: Location, width: u8, value: u16) -> Result<(), String> {
        match location {
            Location::Register(index, encoded_width) => {
                let actual_width = if encoded_width == 0 {
                    width
                } else {
                    encoded_width
                };
                if actual_width == 8 {
                    let high = index & 4 != 0;
                    let register = (index & 3) as usize;
                    self.registers[register] = if high {
                        (self.registers[register] & 0x00ff) | ((value & 0xff) << 8)
                    } else {
                        (self.registers[register] & 0xff00) | (value & 0xff)
                    };
                } else {
                    self.registers[index as usize] = value;
                }
                Ok(())
            }
            Location::Memory(segment, offset) => {
                let address = self.physical(segment, offset) as usize;
                if width == 8 {
                    self.memory[address] = value as u8;
                } else {
                    self.memory[address..address + 2].copy_from_slice(&value.to_le_bytes());
                }
                Ok(())
            }
        }
    }

    fn register8(&self, index: u8) -> u8 {
        let word = self.registers[(index & 3) as usize];
        if index & 4 != 0 {
            (word >> 8) as u8
        } else {
            word as u8
        }
    }

    fn physical(&self, segment: usize, offset: u16) -> u32 {
        ((u32::from(self.segments[segment]) << 4).wrapping_add(u32::from(offset))) & 0xff_ffff
    }

    fn flag(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }

    fn set_flag(&mut self, flag: u16, value: bool) {
        if value {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }

    fn alu(&mut self, operation: u8, left: u16, right: u16, width: u8) -> Result<u16, String> {
        let bits = u32::from(width);
        let mask: u32 = (1 << bits) - 1;
        let sign = 1 << (bits - 1);
        let a = u32::from(left) & mask;
        let b = u32::from(right) & mask;
        let carry_in = u32::from(self.flag(CF));

        let (unbounded, carry, overflow, auxiliary) = match operation {
            0 => {
                let sum = a + b;
                let result = sum & mask;
                (
                    result,
                    sum > mask,
                    (!(a ^ b) & (a ^ result) & sign) != 0,
                    (a ^ b ^ result) & 0x10 != 0,
                )
            }
            1 => {
                let result = a | b;
                (result, false, false, false)
            }
            2 => {
                let sum = a + b + carry_in;
                let result = sum & mask;
                (
                    result,
                    sum > mask,
                    (!(a ^ b) & (a ^ result) & sign) != 0,
                    (a ^ b ^ result) & 0x10 != 0,
                )
            }
            3 => {
                let difference = a.wrapping_sub(b).wrapping_sub(carry_in);
                let result = difference & mask;
                (
                    result,
                    u64::from(a) < u64::from(b) + u64::from(carry_in),
                    ((a ^ b) & (a ^ result) & sign) != 0,
                    (a ^ b ^ result) & 0x10 != 0,
                )
            }
            4 => {
                let result = a & b;
                (result, false, false, false)
            }
            5 | 7 => {
                let difference = a.wrapping_sub(b);
                let result = difference & mask;
                (
                    result,
                    a < b,
                    ((a ^ b) & (a ^ result) & sign) != 0,
                    (a ^ b ^ result) & 0x10 != 0,
                )
            }
            6 => {
                let result = a ^ b;
                (result, false, false, false)
            }
            other => return Err(format!("unsupported ALU operation {other}")),
        };
        let result = unbounded as u16;
        self.set_flag(CF, carry);
        self.set_flag(OF, overflow);
        self.set_flag(SF, result & sign as u16 != 0);
        self.set_flag(ZF, result & mask as u16 == 0);
        self.set_flag(PF, (result as u8).count_ones().is_multiple_of(2));
        self.set_flag(AF, auxiliary);
        Ok(result)
    }

    fn unary_group(&mut self, operand: Location, operation: u8, width: u8) -> Result<(), String> {
        let value = self.read(operand, width)?;
        match operation {
            0 | 1 => {
                let immediate = if width == 8 {
                    u16::from(self.fetch8())
                } else {
                    self.fetch16()
                };
                self.alu(4, value, immediate, width)?;
            }
            2 => self.write(operand, width, !value)?,
            3 => {
                let result = self.alu(5, 0, value, width)?;
                self.write(operand, width, result)?;
            }
            4 | 5 => {
                let bits = u32::from(width);
                if bits == 8 {
                    let (product, overflow) = if operation == 4 {
                        let product = u32::from(self.register8(0)) * u32::from(value);
                        (product, product > 0xff)
                    } else {
                        let product = i32::from(self.register8(0) as i8)
                            .wrapping_mul(i32::from(value as u8 as i8));
                        (product as u32, !(-128..=127).contains(&product))
                    };
                    self.write(Location::Register(0, 16), 16, product as u16)?;
                    self.set_flag(CF, overflow);
                    self.set_flag(OF, overflow);
                } else {
                    let dividend =
                        (u32::from(self.registers[2]) << 16) | u32::from(self.registers[0]);
                    let (product, overflow) = if operation == 4 {
                        let product = dividend.wrapping_mul(u32::from(value));
                        (product, product > 0xffff)
                    } else {
                        let product = i32::from(self.registers[0] as i16)
                            .wrapping_mul(i32::from(value as i16));
                        (product as u32, !(-32768..=32767).contains(&product))
                    };
                    self.registers[0] = product as u16;
                    self.registers[2] = (product >> 16) as u16;
                    self.set_flag(CF, overflow);
                    self.set_flag(OF, overflow);
                }
            }
            6 | 7 => self.divide(value, width, operation == 7)?,
            other => return Err(format!("unsupported unary operation {other}")),
        }
        Ok(())
    }

    fn divide(&mut self, divisor: u16, width: u8, signed: bool) -> Result<(), String> {
        if width == 8 {
            let dividend = self.registers[0];
            if divisor == 0 {
                return Err("reference divide by zero".into());
            }
            let (quotient, remainder) = if signed {
                let q = i32::from(dividend as i16) / i32::from(divisor as u8 as i8);
                let r = i32::from(dividend as i16) % i32::from(divisor as u8 as i8);
                if !(-128..=127).contains(&q) {
                    return Err("reference signed byte divide overflow".into());
                }
                (q as u8, r as u8)
            } else {
                ((dividend / divisor) as u8, (dividend % divisor) as u8)
            };
            self.write(Location::Register(0, 8), 8, u16::from(quotient))?;
            self.write(Location::Register(4, 8), 8, u16::from(remainder))?;
        } else {
            if divisor == 0 {
                return Err("reference divide by zero".into());
            }
            let dividend = (u32::from(self.registers[2]) << 16) | u32::from(self.registers[0]);
            let (quotient, remainder) = if signed {
                let q = i64::from(dividend as i32) / i64::from(divisor as i16);
                let r = i64::from(dividend as i32) % i64::from(divisor as i16);
                if !(-32768..=32767).contains(&q) {
                    return Err("reference signed word divide overflow".into());
                }
                (q as u16, r as u16)
            } else {
                (
                    (dividend / u32::from(divisor)) as u16,
                    (dividend % u32::from(divisor)) as u16,
                )
            };
            self.registers[0] = quotient;
            self.registers[2] = remainder;
        }
        Ok(())
    }

    fn shift(&mut self, operation: u8, value: u16, count: u16, width: u8) -> u16 {
        let count = (count & 31) as u32;
        let bits = u32::from(width);
        let mask: u32 = (1 << bits) - 1;
        let value = u32::from(value) & mask;
        if count == 0 {
            return value as u16;
        }

        let (result, carry) = match operation {
            0 => {
                let mut result = value;
                let mut carry = false;
                for _ in 0..count {
                    carry = result & (1 << (bits - 1)) != 0;
                    result = ((result << 1) | (result >> (bits - 1))) & mask;
                }
                (result, carry)
            }
            1 => {
                let mut result = value;
                let mut carry = false;
                for _ in 0..count {
                    carry = result & 1 != 0;
                    result = (result >> 1) | ((result & 1) << (bits - 1));
                }
                (result, carry)
            }
            2 => {
                let mut result = value;
                let mut carry = self.flag(CF);
                for _ in 0..count {
                    let next = result & (1 << (bits - 1)) != 0;
                    result = ((result << 1) | u32::from(carry)) & mask;
                    carry = next;
                }
                (result, carry)
            }
            3 => {
                let mut result = value;
                let mut carry = self.flag(CF);
                for _ in 0..count {
                    let next = result & 1 != 0;
                    result = (result >> 1) | (u32::from(carry) << (bits - 1));
                    carry = next;
                }
                (result, carry)
            }
            4 | 6 => {
                let result = (value << count) & mask;
                let carry = count <= bits && value & (1 << (bits - count)) != 0;
                (result, carry)
            }
            5 => {
                let result = value >> count;
                let carry = count <= bits && value & (1 << (count - 1)) != 0;
                (result, carry)
            }
            _ => {
                let signed = if width == 8 {
                    i32::from(value as u8 as i8)
                } else {
                    i32::from(value as u16 as i16)
                };
                let result = ((signed >> count) as u32) & mask;
                let carry = if count <= bits {
                    value & (1 << (count - 1)) != 0
                } else {
                    value & (1 << (bits - 1)) != 0
                };
                (result, carry)
            }
        };

        self.set_flag(CF, carry);
        if count == 1 {
            let overflow = match operation {
                0 | 2 | 4 | 6 => (result & (1 << (bits - 1)) != 0) != carry,
                1 | 3 => (result & (1 << (bits - 1)) != 0) != (result & (1 << (bits - 2)) != 0),
                5 => value & (1 << (bits - 1)) != 0,
                _ => false,
            };
            self.set_flag(OF, overflow);
        }
        let result = result as u16;
        // ROL/ROR/RCL/RCR define CF and count-one OF only. SF, ZF, and PF
        // remain undefined on 8086, so this reference leaves them unchanged.
        if operation <= 3 {
            return result;
        }
        self.set_flag(SF, result & (1 << (bits - 1)) as u16 != 0);
        self.set_flag(ZF, result & mask as u16 == 0);
        self.set_flag(PF, (result as u8).count_ones().is_multiple_of(2));
        result
    }
}
