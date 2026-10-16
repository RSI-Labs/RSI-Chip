//! A executable-centric CPU simulation and validation platform.
//!
//! The platform is intentionally independent from synthesis and netlist LEC.
//! Its role is to load real program images, execute them against an ISA-level
//! golden model, emit architectural traces, and provide directed/random
//! validation stimuli for the SystemVerilog CPU implementations.

pub mod elf;
pub mod i8086;
pub mod memory;
pub mod rv32i;
pub mod systemverilog;
pub mod validation;

pub use memory::SparseMemory;
