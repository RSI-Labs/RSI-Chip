//! Simulator unit tests split by language/runtime domain.

mod aggregates;
mod core;
mod functions;
mod properties;

use crate::Simulator;
use crate::MAX_STEPS_PER_TIME;
use eda_core::ir::Design;
use eda_core::logic::{LogicValue, LogicWord};
use eda_hdl::compile;

fn sim_for(src: &str) -> Simulator<'static> {
    let d: &'static Design = Box::leak(Box::new(compile(src, None).unwrap()));
    Simulator::new(d).unwrap()
}
