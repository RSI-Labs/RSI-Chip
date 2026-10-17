//! Distributed-RAM lowering.

use crate::memory_model::{
    constant_word, part_value_index, DistributedRamPartTarget, DistributedRamWrite,
    DistributedRamWritePart,
};
use crate::memory_plan::{macro_candidate, MemoryMacroCandidate};
use crate::Synth;
use eda_core::ir::{BinOp, Expr, MemoryKind, Sens, Stmt};
use eda_core::logic::LogicWord;
use eda_core::netlist::CellKind;

impl<'a> Synth<'a> {
    pub(crate) fn lower_distributed_ram_processes(&mut self) -> Result<(), String> {
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
            if !self.is_distributed_ram(write.memory) {
                continue;
            }
            if self.ram_state.contains_key(&write.memory) {
                return Err(format!(
                    "memory '{}' has multiple clocked write processes, which distributed-RAM lowering does not support",
                    self.design.memories[write.memory].name
                ));
            }
            self.lower_distributed_ram_process(clock, posedge, write)?;
            self.skipped_processes.insert(process_index);
        }
        Ok(())
    }

    pub(crate) fn distributed_ram_write(&self, statement: &Stmt) -> Option<DistributedRamWrite> {
        match statement {
            Stmt::Block(statements) if statements.len() == 1 => {
                self.distributed_ram_write(&statements[0])
            }
            Stmt::MemoryBl {
                memory, indexes, e, ..
            }
            | Stmt::MemoryNBl {
                memory, indexes, e, ..
            } => Some(DistributedRamWrite {
                memory: *memory,
                condition: None,
                indexes: indexes.clone(),
                value: (*e).clone(),
                part: None,
            }),
            Stmt::MemoryBlPart {
                memory,
                indexes,
                start,
                width,
                ascending,
                e,
                ..
            }
            | Stmt::MemoryNBlPart {
                memory,
                indexes,
                start,
                width,
                ascending,
                e,
                ..
            } => Some(DistributedRamWrite {
                memory: *memory,
                condition: None,
                indexes: indexes.clone(),
                value: (*e).clone(),
                part: Some(DistributedRamWritePart {
                    start: (**start).clone(),
                    width: *width,
                    ascending: *ascending,
                }),
            }),
            Stmt::If { c, t, f: None, .. } => {
                let write = self.distributed_ram_write(t)?;
                write.condition.as_ref().map(|_| None).unwrap_or_else(|| {
                    Some(DistributedRamWrite {
                        condition: Some((*c).clone()),
                        ..write
                    })
                })
            }
            _ => None,
        }
    }

    pub(crate) fn is_distributed_ram(&self, memory_index: usize) -> bool {
        let Some(memory) = self.design.memories.get(memory_index) else {
            return false;
        };
        memory.kind == MemoryKind::Ram
            && macro_candidate(
                memory.template,
                u128::from(memory.flat_length) * u128::try_from(memory.element_width).unwrap_or(0),
            ) == MemoryMacroCandidate::DistributedRam
    }

    pub(crate) fn lower_distributed_ram_process(
        &mut self,
        clock_flat: usize,
        posedge: bool,
        write: DistributedRamWrite,
    ) -> Result<(), String> {
        let memory = self.design.memories[write.memory].clone();
        let clock = self.flat2net_of(clock_flat)?;
        let state = (0..memory.flat_length)
            .map(|_| {
                (0..memory.element_width)
                    .map(|_| self.new_tmp("ram_q"))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        self.ram_state.insert(write.memory, state);

        self.reset_sym();
        let condition = match &write.condition {
            Some(condition) => Some(self.blast(condition, 1)?[0]),
            None => None,
        };
        let value_width = write
            .part
            .as_ref()
            .map(|part| part.width)
            .unwrap_or(memory.element_width);
        let value = self.blast(&write.value, value_width)?;
        let part_targets = match &write.part {
            Some(part) => Some(self.distributed_ram_part_targets(&memory, part)?),
            None => None,
        };
        let old_state = self
            .ram_state
            .get(&write.memory)
            .cloned()
            .unwrap_or_default();
        let mut next_state = Vec::with_capacity(memory.flat_length as usize);
        for (physical_index, old_bits) in old_state.iter().enumerate() {
            let physical_index = physical_index as u64;
            let mut select = self.memory_address_select(&memory, physical_index, &write.indexes)?;
            if let Some(condition) = condition {
                select = self.combine(CellKind::And, select, condition);
            }
            let mut updated = Vec::with_capacity(memory.element_width);
            for (bit, old) in old_bits.iter().enumerate() {
                let mut next = *old;
                match &part_targets {
                    None => {
                        next = self.mux(select, value[bit], next);
                    }
                    Some(targets) => {
                        for target in targets {
                            let Some(value_index) = part_value_index(&memory, target, bit) else {
                                continue;
                            };
                            let bit_select = match target.start_select {
                                Some(start_select) => {
                                    self.combine(CellKind::And, select, start_select)
                                }
                                None => select,
                            };
                            next = self.mux(bit_select, value[value_index], next);
                        }
                    }
                }
                self.add_cell("dxff", CellKind::Dff { posedge }, vec![clock, next], *old);
                updated.push(*old);
            }
            next_state.push(updated);
        }
        self.ram_state.insert(write.memory, next_state);
        self.lowered_memories.insert(write.memory);
        Ok(())
    }

    pub(crate) fn distributed_ram_part_targets(
        &mut self,
        memory: &eda_core::ir::MemoryDecl,
        part: &DistributedRamWritePart,
    ) -> Result<Vec<DistributedRamPartTarget>, String> {
        if part.width == 0 || part.width > memory.element_width {
            return Err(format!(
                "memory '{}' has an invalid distributed-RAM part-select width {}",
                memory.name, part.width
            ));
        }
        if let Some(start) = constant_word(&part.start)
            .and_then(|word| word.to_u64())
            .map(|start| start as i64)
        {
            let (low, high) = memory
                .element_part_offsets(start, part.width, part.ascending)
                .ok_or_else(|| {
                    format!(
                        "memory '{}' distributed-RAM part select start {start} is out of range",
                        memory.name
                    )
                })?;
            return Ok(vec![DistributedRamPartTarget {
                low,
                high,
                start_select: None,
            }]);
        }

        let start_width = part.start.width().max(1);
        let lower = memory.element_msb.min(memory.element_lsb);
        let upper = memory.element_msb.max(memory.element_lsb);
        let first = if part.ascending {
            lower
        } else {
            lower + part.width as i64 - 1
        };
        let last = if part.ascending {
            upper - part.width as i64 + 1
        } else {
            upper
        };
        let mut targets: Vec<DistributedRamPartTarget> = Vec::new();
        for logical_start in first..=last {
            let Some((low, high)) =
                memory.element_part_offsets(logical_start, part.width, part.ascending)
            else {
                continue;
            };
            if targets.iter().any(|target| target.low == low) {
                continue;
            }
            let target = Expr::Const(LogicWord::from_u64(start_width, logical_start as u64));
            let equal = self.blast_bin(BinOp::Eq, &part.start, &target)?;
            let Some(equal) = equal.first() else {
                return Err(format!(
                    "memory '{}' distributed-RAM part-select comparison produced no select",
                    memory.name
                ));
            };
            targets.push(DistributedRamPartTarget {
                low,
                high,
                start_select: Some(*equal),
            });
        }
        if targets.is_empty() {
            return Err(format!(
                "memory '{}' distributed-RAM part select has no legal starts",
                memory.name
            ));
        }
        Ok(targets)
    }
}

impl<'a> Synth<'a> {}
