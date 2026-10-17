//! Sparse-memory macro planning and technology-independent candidate choice.

use eda_core::ir::{Design, MemoryByteEnable, MemoryKind, MemoryTemplate, MemoryWriteMask};

/// Technology-independent macro candidate.
///
/// This is not a vendor cell selection. The thresholds deliberately separate
/// small structures that can reasonably remain distributed logic from larger
/// structures that require block-RAM/ROM mapping or explicit partitioning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryMacroCandidate {
    DistributedRom,
    BlockRom,
    DistributedRam,
    BlockRam,
    NoStoragePort,
    PartitionRequired,
    UnsupportedAsyncLargeMemory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryMacroPlan {
    pub memory: usize,
    pub name: String,
    pub kind: MemoryKind,
    pub template: MemoryTemplate,
    pub element_width: usize,
    pub bank_count: u64,
    pub bank_depth: u64,
    pub total_bits: u128,
    pub write_mask: Option<MemoryWriteMask>,
    pub byte_enable: Option<MemoryByteEnable>,
    pub candidate: MemoryMacroCandidate,
    pub rationale: String,
}

pub fn plan_memories(design: &Design) -> Vec<MemoryMacroPlan> {
    design
        .memories
        .iter()
        .enumerate()
        .map(|(memory_index, memory)| {
            let total_bits =
                u128::from(memory.flat_length) * u128::try_from(memory.element_width).unwrap_or(0);
            let candidate = macro_candidate(memory.template, total_bits);
            let rationale = match candidate {
                MemoryMacroCandidate::DistributedRom => {
                    "small read-only structure can remain distributed logic"
                }
                MemoryMacroCandidate::BlockRom => {
                    "synchronous read timing and size favor block-ROM mapping"
                }
                MemoryMacroCandidate::DistributedRam => {
                    "small runtime-writable structure can map to distributed/LUT RAM"
                }
                MemoryMacroCandidate::BlockRam => {
                    "synchronous write timing and size favor block-RAM mapping"
                }
                MemoryMacroCandidate::NoStoragePort => {
                    "no hardware storage access was inferred for this object"
                }
                MemoryMacroCandidate::PartitionRequired => {
                    "mixed timing requires explicit port/bank partitioning"
                }
                MemoryMacroCandidate::UnsupportedAsyncLargeMemory => {
                    "large asynchronous access is not covered by the current macro model"
                }
            };
            MemoryMacroPlan {
                memory: memory_index,
                name: memory.name.clone(),
                kind: memory.kind,
                template: memory.template,
                element_width: memory.element_width,
                bank_count: memory.banking.count,
                bank_depth: memory.banking.depth,
                total_bits,
                write_mask: memory.write_mask.clone(),
                byte_enable: memory.byte_enable.clone(),
                candidate,
                rationale: rationale.to_string(),
            }
        })
        .collect()
}

pub(crate) fn macro_candidate(template: MemoryTemplate, total_bits: u128) -> MemoryMacroCandidate {
    const DISTRIBUTED_ROM_LIMIT: u128 = 2048;
    const DISTRIBUTED_RAM_LIMIT: u128 = 4096;
    match template {
        MemoryTemplate::Unmapped => MemoryMacroCandidate::NoStoragePort,
        MemoryTemplate::AsyncRom => {
            if total_bits <= DISTRIBUTED_ROM_LIMIT {
                MemoryMacroCandidate::DistributedRom
            } else {
                MemoryMacroCandidate::UnsupportedAsyncLargeMemory
            }
        }
        MemoryTemplate::SyncRom => {
            if total_bits <= DISTRIBUTED_ROM_LIMIT {
                MemoryMacroCandidate::DistributedRom
            } else {
                MemoryMacroCandidate::BlockRom
            }
        }
        MemoryTemplate::AsyncRam => {
            if total_bits <= DISTRIBUTED_RAM_LIMIT {
                MemoryMacroCandidate::DistributedRam
            } else {
                MemoryMacroCandidate::UnsupportedAsyncLargeMemory
            }
        }
        MemoryTemplate::SyncRam => {
            if total_bits <= DISTRIBUTED_RAM_LIMIT {
                MemoryMacroCandidate::DistributedRam
            } else {
                MemoryMacroCandidate::BlockRam
            }
        }
        MemoryTemplate::MixedRom | MemoryTemplate::MixedRam => {
            MemoryMacroCandidate::PartitionRequired
        }
    }
}
