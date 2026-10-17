//! Macro threshold decision tests.

use super::super::super::*;
use crate::memory_plan::macro_candidate;
use eda_core::ir::MemoryTemplate;

#[test]
fn memory_macro_thresholds_separate_distributed_and_block_candidates() {
    assert_eq!(
        macro_candidate(MemoryTemplate::SyncRom, 2048),
        MemoryMacroCandidate::DistributedRom
    );
    assert_eq!(
        macro_candidate(MemoryTemplate::SyncRom, 2049),
        MemoryMacroCandidate::BlockRom
    );
    assert_eq!(
        macro_candidate(MemoryTemplate::SyncRam, 4096),
        MemoryMacroCandidate::DistributedRam
    );
    assert_eq!(
        macro_candidate(MemoryTemplate::SyncRam, 4097),
        MemoryMacroCandidate::BlockRam
    );
    assert_eq!(
        macro_candidate(MemoryTemplate::AsyncRom, 2049),
        MemoryMacroCandidate::UnsupportedAsyncLargeMemory
    );
    assert_eq!(
        macro_candidate(MemoryTemplate::MixedRam, 1),
        MemoryMacroCandidate::PartitionRequired
    );
}
