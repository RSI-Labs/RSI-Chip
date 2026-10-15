//! Shared SPICE circuit and result types.

mod analysis;
mod circuit;
mod devices;
mod results;

pub use analysis::*;
pub use devices::*;
pub use results::*;

pub const GROUND_ALIASES: [&str; 3] = ["0", "gnd", "GND"];
pub const SPICE_NOMINAL_TEMPERATURE_K: f64 = 300.0;
