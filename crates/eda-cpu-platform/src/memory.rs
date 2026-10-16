use std::collections::HashMap;

const PAGE_SHIFT: u32 = 12;
pub const PAGE_SIZE: u32 = 1 << PAGE_SHIFT;

#[derive(Clone, Debug, Default)]
pub struct SparseMemory {
    pages: HashMap<u32, Box<[u8; PAGE_SIZE as usize]>>,
}

impl SparseMemory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn read_u8(&self, address: u32) -> u8 {
        self.pages
            .get(&(address >> PAGE_SHIFT))
            .map_or(0, |page| page[(address & (PAGE_SIZE - 1)) as usize])
    }

    pub fn write_u8(&mut self, address: u32, value: u8) {
        let page_number = address >> PAGE_SHIFT;
        let offset = (address & (PAGE_SIZE - 1)) as usize;
        self.pages
            .entry(page_number)
            .or_insert_with(|| Box::new([0; PAGE_SIZE as usize]))[offset] = value;
    }

    pub fn write_slice(&mut self, mut address: u32, values: &[u8]) {
        for value in values {
            self.write_u8(address, *value);
            address = address.wrapping_add(1);
        }
    }

    pub fn read_le_u16(&self, address: u32) -> u16 {
        u16::from_le_bytes([self.read_u8(address), self.read_u8(address.wrapping_add(1))])
    }

    pub fn read_le_u32(&self, address: u32) -> u32 {
        u32::from_le_bytes([
            self.read_u8(address),
            self.read_u8(address.wrapping_add(1)),
            self.read_u8(address.wrapping_add(2)),
            self.read_u8(address.wrapping_add(3)),
        ])
    }

    pub fn write_le_u16(&mut self, address: u32, value: u16) {
        let bytes = value.to_le_bytes();
        self.write_u8(address, bytes[0]);
        self.write_u8(address.wrapping_add(1), bytes[1]);
    }

    pub fn write_le_u32(&mut self, address: u32, value: u32) {
        let bytes = value.to_le_bytes();
        self.write_u8(address, bytes[0]);
        self.write_u8(address.wrapping_add(1), bytes[1]);
        self.write_u8(address.wrapping_add(2), bytes[2]);
        self.write_u8(address.wrapping_add(3), bytes[3]);
    }

    pub fn fill(&mut self, start: u32, len: u32, value: u8) {
        for offset in 0..len {
            self.write_u8(start.wrapping_add(offset), value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_pages_wrap_and_default_to_zero() {
        let mut memory = SparseMemory::new();
        memory.write_le_u32(u32::MAX - 1, 0x1234_5678);
        assert_eq!(memory.read_u8(u32::MAX - 1), 0x78);
        assert_eq!(memory.read_u8(1), 0x12);
        assert_eq!(memory.read_u8(0x1000), 0);
    }
}
