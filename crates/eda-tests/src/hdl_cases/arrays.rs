//! HDL array/indexed-select integration cases split by parser and runtime domain.

mod frontend;
mod indexed;
mod module_ports;
mod runtime_arrays;

use crate::CaseList;
use eda_core::ir::Dir;
use eda_core::logic::LogicWord;
use eda_core::to_design::netlist_to_design;
use eda_hdl::lexer::Lexer;
use eda_hdl::{compile, elaborate, parse};
use eda_sim::Simulator;
use eda_synth::{synthesize, SynthOptions};

pub(super) fn register(c: &mut CaseList) {
    frontend::register(c);
    indexed::register(c);
    runtime_arrays::register(c);
    module_ports::register(c);
}
