//! Block-RAM macro lowering.

use crate::memory_model::{BlockRamLowering, DistributedRamWrite};
use crate::memory_plan::{macro_candidate, MemoryMacroCandidate};
use crate::Synth;
use eda_core::ir::{MemoryKind, MemoryWriteMask, Sens};
use eda_core::logic::{LogicValue, LogicWord};
use eda_core::netlist::CellKind;
use std::sync::Arc;

impl<'a> Synth<'a> {
    pub(crate) fn lower_block_ram_processes(&mut self) -> Result<(), String> {
        for process_index in 0..self.design.processes.len() {
            let process = self.design.processes[process_index].clone();
            let (clock, posedge) = match &process.sens {
                Sens::Posedge(net) => (*net, true),
                Sens::Negedge(net) => (*net, false),
                _ => continue,
            };
            let Some(write) = self.distributed_ram_write(&process.body) else {
                continue;
            };
            if !self.is_block_ram(write.memory) {
                continue;
            }
            if self.block_ram_state.contains_key(&write.memory) {
                return Err(format!(
                    "memory '{}' has multiple clocked write ports, which the current BlockRam model does not support",
                    self.design.memories[write.memory].name
                ));
            }
            self.lower_block_ram_process(clock, posedge, write)?;
            self.skipped_processes.insert(process_index);
        }
        Ok(())
    }

    pub(crate) fn is_block_ram(&self, memory_index: usize) -> bool {
        let Some(memory) = self.design.memories.get(memory_index) else {
            return false;
        };
        memory.kind == MemoryKind::Ram
            && macro_candidate(
                memory.template,
                u128::from(memory.flat_length) * u128::try_from(memory.element_width).unwrap_or(0),
            ) == MemoryMacroCandidate::BlockRam
    }

    pub(crate) fn lower_block_ram_process(
        &mut self,
        clock_flat: usize,
        posedge: bool,
        write: DistributedRamWrite,
    ) -> Result<(), String> {
        let memory = self.design.memories[write.memory].clone();
        let flat_length = usize::try_from(memory.flat_length).map_err(|_| {
            format!(
                "memory '{}' is too large for the current BlockRam model",
                memory.name
            )
        })?;
        if flat_length == 0 {
            return Err(format!(
                "memory '{}' has an empty BlockRam address space",
                memory.name
            ));
        }
        if write.indexes.len() != 1
            || memory.dimensions.len() != 1
            || memory.dimensions[0].base != 0
            || memory.dimensions[0].step != 1
            || memory.dimensions[0].length as u64 != memory.flat_length
        {
            return Err(format!(
                "memory '{}' requires one normalized one-dimensional BlockRam address index",
                memory.name
            ));
        }
        if write.part.is_some()
            || !memory
                .write_mask
                .as_ref()
                .is_some_and(MemoryWriteMask::is_full)
            || memory.banking.count != 1
        {
            return Err(format!(
                "memory '{}' uses a partial-write or banked contract that the current BlockRam model does not support",
                memory.name
            ));
        }

        let address_width = flat_length.max(2).next_power_of_two().trailing_zeros() as usize;
        if address_width == 0 || address_width > u32::BITS as usize {
            return Err(format!(
                "memory '{}' has an invalid BlockRam address width",
                memory.name
            ));
        }
        let initial = self.block_ram_initial_contents(write.memory, flat_length)?;

        self.reset_sym();
        let clock = self.flat2net_of(clock_flat)?;
        let enable = match &write.condition {
            Some(condition) => self.blast(condition, 1)?[0],
            None => self.const_net(LogicValue::One),
        };
        let mut address = Vec::with_capacity(address_width);
        for index in &write.indexes {
            address.extend(self.blast(index, address_width)?);
        }
        if address.len() != address_width {
            return Err(format!(
                "memory '{}' BlockRam write address width mismatch",
                memory.name
            ));
        }
        let data = self.blast(&write.value, memory.element_width)?;
        if data.len() != memory.element_width {
            return Err(format!(
                "memory '{}' BlockRam write data width mismatch",
                memory.name
            ));
        }

        for (bit_index, data_bit) in data.into_iter().enumerate() {
            let state_bit = self.new_tmp("block_ram_w");
            let mut inputs = Vec::with_capacity(2 + address_width + 1);
            inputs.extend([clock, enable]);
            inputs.extend(address.iter().copied());
            inputs.push(data_bit);
            self.add_cell(
                "block_ram_w",
                CellKind::BlockRamWrite {
                    macro_id: write.memory,
                    address_width,
                    bit_index,
                    initial: initial.clone(),
                    posedge,
                },
                inputs,
                state_bit,
            );
        }
        self.block_ram_state.insert(
            write.memory,
            BlockRamLowering {
                address_width,
                initial,
            },
        );
        self.lowered_memories.insert(write.memory);
        Ok(())
    }

    pub(crate) fn block_ram_initial_contents(
        &self,
        memory_index: usize,
        flat_length: usize,
    ) -> Result<Arc<Vec<LogicWord>>, String> {
        let memory = &self.design.memories[memory_index];
        let Some(contents) = self.rom_contents.get(&memory_index) else {
            return Ok(Arc::new(vec![
                LogicWord::xes(memory.element_width);
                flat_length
            ]));
        };
        if !contents.deterministic {
            return Err(format!(
                "memory '{}' has conditional BlockRam initialization",
                memory.name
            ));
        }
        let mut words = vec![LogicWord::xes(memory.element_width); flat_length];
        for (logical, word) in &contents.values {
            let Some(&logical_index) = logical.first() else {
                return Err(format!(
                    "memory '{}' has invalid BlockRam index",
                    memory.name
                ));
            };
            if logical.len() != 1 {
                return Err(format!(
                    "memory '{}' has invalid BlockRam index rank {}",
                    memory.name,
                    logical.len()
                ));
            }
            let Some(physical) = memory.physical_index(&[logical_index]) else {
                return Err(format!(
                    "memory '{}' has an out-of-range BlockRam initialization index",
                    memory.name
                ));
            };
            words[physical as usize] = word.clone();
        }
        Ok(Arc::new(words))
    }
}
