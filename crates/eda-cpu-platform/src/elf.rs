use crate::SparseMemory;

pub const EM_RISCV: u16 = 243;
pub const EM_386: u16 = 3;
pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;
pub const PT_LOAD: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElfClass {
    Elf32,
    Elf64,
}

#[derive(Clone, Debug)]
pub struct ElfImage {
    pub class: ElfClass,
    pub little_endian: bool,
    pub machine: u16,
    pub entry: u64,
    pub image_base: u64,
    pub segments: Vec<ElfSegment>,
}

#[derive(Clone, Debug)]
pub struct ElfSegment {
    pub vaddr: u64,
    pub file_offset: usize,
    pub file_size: usize,
    pub memory_size: u64,
}

#[derive(Debug)]
pub struct ElfError(pub String);

impl std::fmt::Display for ElfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ElfError {}

pub fn parse_elf(bytes: &[u8]) -> Result<ElfImage, ElfError> {
    if bytes.len() < 52 || &bytes[0..4] != b"\x7fELF" {
        return Err(ElfError("not an ELF image".into()));
    }
    let class = match bytes[4] {
        1 => ElfClass::Elf32,
        2 => ElfClass::Elf64,
        value => return Err(ElfError(format!("unsupported ELF class {value}"))),
    };
    let little_endian = match bytes[5] {
        1 => true,
        2 => false,
        value => return Err(ElfError(format!("unsupported ELF endian {value}"))),
    };
    if !little_endian {
        return Err(ElfError("big-endian ELF images are not supported".into()));
    }

    let (entry, phoff, phentsize, phnum, machine, r#type) = match class {
        ElfClass::Elf32 => {
            if bytes.len() < 52 {
                return Err(ElfError("truncated ELF32 header".into()));
            }
            (
                le_u32(bytes, 24)? as u64,
                le_u32(bytes, 28)? as usize,
                le_u16(bytes, 42)? as usize,
                le_u16(bytes, 44)? as usize,
                le_u16(bytes, 18)?,
                le_u16(bytes, 16)?,
            )
        }
        ElfClass::Elf64 => {
            if bytes.len() < 64 {
                return Err(ElfError("truncated ELF64 header".into()));
            }
            (
                le_u64(bytes, 24)?,
                le_u64(bytes, 32)? as usize,
                le_u16(bytes, 54)? as usize,
                le_u16(bytes, 56)? as usize,
                le_u16(bytes, 18)?,
                le_u16(bytes, 16)?,
            )
        }
    };

    if phentsize == 0 || phnum == 0 {
        return Err(ElfError("ELF has no program-header table".into()));
    }
    let mut segments = Vec::with_capacity(phnum);
    for index in 0..phnum {
        let offset = phoff
            .checked_add(index * phentsize)
            .ok_or_else(|| ElfError("program-header offset overflow".into()))?;
        if offset + phentsize > bytes.len() {
            return Err(ElfError("truncated program header".into()));
        }
        let program = &bytes[offset..offset + phentsize];
        let (kind, file_offset, vaddr, file_size, memory_size) = match class {
            ElfClass::Elf32 => {
                let kind = le_u32(program, 0)?;
                let offset = le_u32(program, 4)? as usize;
                let vaddr = le_u32(program, 8)? as u64;
                let file_size = le_u32(program, 16)? as usize;
                let memory_size = le_u32(program, 20)? as u64;
                (kind, offset, vaddr, file_size, memory_size)
            }
            ElfClass::Elf64 => {
                let kind = le_u32(program, 0)?;
                let offset = le_u64(program, 8)? as usize;
                let vaddr = le_u64(program, 16)?;
                let file_size = le_u64(program, 32)? as usize;
                let memory_size = le_u64(program, 40)?;
                (kind, offset, vaddr, file_size, memory_size)
            }
        };
        if kind != PT_LOAD {
            continue;
        }
        if file_offset
            .checked_add(file_size)
            .is_none_or(|end| end > bytes.len())
        {
            return Err(ElfError("PT_LOAD segment exceeds file".into()));
        }
        segments.push(ElfSegment {
            vaddr,
            file_offset,
            file_size,
            memory_size,
        });
    }
    if segments.is_empty() {
        return Err(ElfError("ELF has no PT_LOAD segments".into()));
    }
    let image_base = if r#type == ET_DYN {
        0x4000_0000
    } else {
        segments
            .iter()
            .map(|segment| segment.vaddr)
            .min()
            .unwrap_or(0)
    };
    Ok(ElfImage {
        class,
        little_endian,
        machine,
        entry,
        image_base,
        segments,
    })
}

pub fn load_elf(memory: &mut SparseMemory, bytes: &[u8]) -> Result<ElfImage, ElfError> {
    let image = parse_elf(bytes)?;
    if image.machine != EM_RISCV {
        return Err(ElfError(format!(
            "expected ELF machine RISC-V ({EM_RISCV}), got {}",
            image.machine
        )));
    }
    if matches!(image.class, ElfClass::Elf64) {
        return Err(ElfError("RV32I platform requires ELF32 images".into()));
    }
    let bias = if image.entry >= image.image_base {
        0
    } else {
        image.image_base
    };
    for segment in &image.segments {
        let start = segment.vaddr.wrapping_add(bias) as u32;
        let bytes = &bytes[segment.file_offset..segment.file_offset + segment.file_size];
        memory.write_slice(start, bytes);
        if segment.memory_size > segment.file_size as u64 {
            let zero_start = start.wrapping_add(segment.file_size as u32);
            let zero_len = (segment.memory_size - segment.file_size as u64) as u32;
            memory.fill(zero_start, zero_len, 0);
        }
    }
    Ok(ElfImage {
        entry: image.entry.wrapping_add(bias),
        ..image
    })
}

fn le_u16(bytes: &[u8], offset: usize) -> Result<u16, ElfError> {
    bytes
        .get(offset..offset + 2)
        .map(|value| u16::from_le_bytes([value[0], value[1]]))
        .ok_or_else(|| ElfError("truncated 16-bit ELF field".into()))
}

fn le_u32(bytes: &[u8], offset: usize) -> Result<u32, ElfError> {
    bytes
        .get(offset..offset + 4)
        .map(|value| u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
        .ok_or_else(|| ElfError("truncated 32-bit ELF field".into()))
}

fn le_u64(bytes: &[u8], offset: usize) -> Result<u64, ElfError> {
    bytes
        .get(offset..offset + 8)
        .map(|value| {
            u64::from_le_bytes([
                value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
            ])
        })
        .ok_or_else(|| ElfError("truncated 64-bit ELF field".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_elf() {
        assert!(parse_elf(b"not elf").is_err());
    }
}
