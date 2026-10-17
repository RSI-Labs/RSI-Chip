//! Distributed/block-ROM read macro lowering.

use crate::memory_model::physical_to_logical;
use crate::memory_plan::{macro_candidate, MemoryMacroCandidate};
use crate::Synth;
use eda_core::ir::{BinOp, Expr, MemoryKind};
use eda_core::logic::{LogicValue, LogicWord};
use eda_core::netlist::{CellKind, NetId};

impl<'a> Synth<'a> {
    pub(crate) fn blast_distributed_rom(
        &mut self,
        memory_index: usize,
        indexes: &[Expr],
    ) -> Result<Vec<NetId>, String> {
        let memory = self
            .design
            .memories
            .get(memory_index)
            .ok_or_else(|| format!("memory read refers to missing memory {memory_index}"))?
            .clone();
        let total_bits =
            u128::from(memory.flat_length) * u128::try_from(memory.element_width).unwrap_or(0);
        if memory.kind == MemoryKind::Ram {
            if macro_candidate(memory.template, total_bits) == MemoryMacroCandidate::BlockRam {
                let Some(state) = self.block_ram_state.get(&memory_index).cloned() else {
                    return Err(format!(
                        "memory '{}' has no lowered BlockRam write port",
                        memory.name
                    ));
                };
                if indexes.len() != 1 {
                    return Err(format!(
                        "memory '{}' requires exactly one BlockRam read index, got {}",
                        memory.name,
                        indexes.len()
                    ));
                }
                let mut address = Vec::with_capacity(state.address_width);
                for index in indexes {
                    address.extend(self.blast(index, state.address_width)?);
                }
                if address.len() != state.address_width {
                    return Err(format!(
                        "memory '{}' BlockRam read address width mismatch",
                        memory.name
                    ));
                }
                let mut output = Vec::with_capacity(memory.element_width);
                for bit_index in 0..memory.element_width {
                    let out = self.new_tmp("block_ram_q");
                    self.add_cell(
                        "block_ram_r",
                        CellKind::BlockRamRead {
                            macro_id: memory_index,
                            address_width: state.address_width,
                            bit_index,
                            initial: state.initial.clone(),
                        },
                        address.clone(),
                        out,
                    );
                    output.push(out);
                }
                return Ok(output);
            }
            if macro_candidate(memory.template, total_bits) != MemoryMacroCandidate::DistributedRam
            {
                return Err(format!(
                    "memory '{}' requires a writable block macro, which is not implemented",
                    memory.name
                ));
            }
            let Some(state) = self.ram_state.get(&memory_index).cloned() else {
                return Err(format!(
                    "memory '{}' has no lowered distributed-RAM state",
                    memory.name
                ));
            };
            let mut output = (0..memory.element_width)
                .map(|_| self.const_net(LogicValue::X))
                .collect::<Vec<_>>();
            for (physical_index, value) in state.iter().enumerate() {
                let select = self.memory_address_select(&memory, physical_index as u64, indexes)?;
                for (bit, new_value) in output.iter_mut().zip(value.iter()) {
                    *bit = self.mux(select, *new_value, *bit);
                }
            }
            return Ok(output);
        }
        if macro_candidate(memory.template, total_bits) == MemoryMacroCandidate::BlockRom {
            if indexes.len() != 1 {
                return Err(format!(
                    "memory '{}' requires exactly one BlockRom address index, got {}",
                    memory.name,
                    indexes.len()
                ));
            }
            if memory.dimensions.len() != 1
                || memory.dimensions[0].base != 0
                || memory.dimensions[0].step != 1
                || memory.dimensions[0].length as u64 != memory.flat_length
            {
                return Err(format!(
                    "memory '{}' uses non-normalized indexing and cannot yet map to one BlockRom address port",
                    memory.name
                ));
            }
            let flat_length = usize::try_from(memory.flat_length).map_err(|_| {
                format!(
                    "memory '{}' is too large for the current BlockRom model",
                    memory.name
                )
            })?;
            if flat_length == 0 {
                return Err(format!(
                    "memory '{}' has an empty BlockRom address space",
                    memory.name
                ));
            }
            let contents = self
                .rom_contents
                .get(&memory_index)
                .cloned()
                .ok_or_else(|| format!("memory '{}' has no ROM contents", memory.name))?;
            if !contents.deterministic {
                return Err(format!(
                    "memory '{}' has conditional ROM initialization and cannot map to BlockRom",
                    memory.name
                ));
            }
            let mut words = vec![LogicWord::xes(memory.element_width); memory.flat_length as usize];
            for (logical, word) in contents.values {
                if logical.len() != 1 {
                    return Err(format!(
                        "memory '{}' has invalid BlockRom index rank {}",
                        memory.name,
                        logical.len()
                    ));
                }
                let Some(logical_index) = logical.first().copied() else {
                    return Err(format!("memory '{}' has invalid ROM index", memory.name));
                };
                let Some(physical) = memory.physical_index(&[logical_index]) else {
                    return Err(format!(
                        "memory '{}' has out-of-range ROM index",
                        memory.name
                    ));
                };
                words[physical as usize] = word;
            }
            let contents = std::sync::Arc::new(words);
            let address_width = flat_length.max(2).next_power_of_two().trailing_zeros() as usize;
            if address_width == 0 || address_width > u32::BITS as usize {
                return Err(format!(
                    "memory '{}' has an invalid BlockRom address width",
                    memory.name
                ));
            }
            let mut address = Vec::with_capacity(address_width);
            for index in indexes {
                address.extend(self.blast(index, address_width)?);
            }
            if address.len() != address_width {
                return Err(format!(
                    "memory '{}' BlockRom address width mismatch",
                    memory.name
                ));
            }
            let mut output = Vec::with_capacity(memory.element_width);
            for bit_index in 0..memory.element_width {
                let out = self.new_tmp("block_rom_q");
                self.add_cell(
                    "block_rom",
                    CellKind::BlockRom {
                        address_width,
                        bit_index,
                        contents: contents.clone(),
                    },
                    address.clone(),
                    out,
                );
                output.push(out);
            }
            self.lowered_memories.insert(memory_index);
            return Ok(output);
        }
        if macro_candidate(memory.template, total_bits) != MemoryMacroCandidate::DistributedRom {
            return Err(format!(
                "memory '{}' requires block macro synthesis, which is not implemented",
                memory.name
            ));
        }
        let contents = self
            .rom_contents
            .get(&memory_index)
            .ok_or_else(|| format!("memory '{}' has no deterministic ROM contents", memory.name))?;
        if !contents.deterministic {
            return Err(format!(
                "memory '{}' has conditional or unsupported initialization and cannot lower to distributed ROM",
                memory.name
            ));
        }
        let contents = contents.values.clone();

        self.lowered_memories.insert(memory_index);
        let mut output = (0..memory.element_width)
            .map(|_| self.const_net(LogicValue::X))
            .collect::<Vec<_>>();
        for physical_index in 0..memory.flat_length {
            let logical = physical_to_logical(&memory, physical_index)
                .ok_or_else(|| format!("memory '{}' has invalid banking geometry", memory.name))?;
            let select = self.memory_address_select(&memory, physical_index, indexes)?;
            let value = contents
                .get(&logical)
                .cloned()
                .unwrap_or_else(|| LogicWord::xes(memory.element_width));
            for (bit, value_bit) in output.iter_mut().zip(value.bits.iter()) {
                let constant = self.const_net(*value_bit);
                *bit = self.mux(select, constant, *bit);
            }
        }
        Ok(output)
    }

    pub(crate) fn memory_address_select(
        &mut self,
        memory: &eda_core::ir::MemoryDecl,
        physical_index: u64,
        indexes: &[Expr],
    ) -> Result<NetId, String> {
        let logical = physical_to_logical(memory, physical_index)
            .ok_or_else(|| format!("memory '{}' has invalid banking geometry", memory.name))?;
        let mut select = None;
        for (index, logical_index) in indexes.iter().zip(logical.iter()) {
            let width = index.width().max(1);
            let target = Expr::Const(LogicWord::from_u64(width, *logical_index as u64));
            let equal = self.blast_bin(BinOp::Eq, index, &target)?;
            let Some(equal) = equal.first() else {
                return Err(format!(
                    "memory '{}' address comparison produced no select bit",
                    memory.name
                ));
            };
            select = Some(match select {
                Some(previous) => self.combine(CellKind::And, previous, *equal),
                None => *equal,
            });
        }
        select.ok_or_else(|| format!("memory '{}' read has no index", memory.name))
    }
}
