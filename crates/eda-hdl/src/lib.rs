//! # eda-hdl — Verilog frontend
//!
//! Parses a synthesizable/simulatable subset of Verilog into an AST, then
//! elaborates the module hierarchy into a flat [`eda_core::ir::Design`].

pub mod ast;
pub mod elab;
pub mod lexer;
pub mod parser;

pub use elab::{compile, elaborate};
pub use parser::parse;
