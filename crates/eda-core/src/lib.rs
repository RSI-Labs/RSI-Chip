//! # eda-core
//!
//! Shared foundation for the whole EDA toolchain:
//! - [`logic`]: four-valued logic (0/1/X/Z) and multi-bit words with Verilog semantics.
//! - [`netlist`]: gate-level netlist IR used by synthesis, formal, STA and PnR.
//! - [`ir`]: elaborated RTL IR (the output of the HDL frontend).
//! - [`library`]: standard-cell library model (function, area, timing arcs).

pub mod cancellation;
pub mod ir;
pub mod liberty;
pub mod library;
pub mod logic;
pub mod memory_file;
pub mod netlist;
pub mod to_design;
