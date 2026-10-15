//! SystemVerilog class, string, polymorphism, and UVM integration cases.

use crate::CaseList;

mod oop;
mod uvm_core;
mod uvm_resources;
mod uvm_sequences;

pub(super) fn register(c: &mut CaseList) {
    oop::register(c);
    uvm_core::register(c);
    uvm_sequences::register(c);
    uvm_resources::register(c);
}
