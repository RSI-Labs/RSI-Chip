//! A practical SPICE netlist subset parser/writer.

mod core;
mod elements;
mod models;
mod utility;
mod writer;

pub use core::parse_spice;
pub use models::{
    parse_resistor_layer_models, parse_statistical_model, write_resistor_layer_models,
    write_statistical_model,
};
pub use utility::parse_number;
pub use writer::{format_number, write_spice};
