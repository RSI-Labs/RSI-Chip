//! # eda-ir — static power-grid IR-drop and electromigration analysis.
//!
//! Cell currents are derived from a switching-power report and distributed on
//! their placed sites. VDD/GND rails are modeled as resistive segments fed
//! from both ends, giving a physically ordered static voltage-drop estimate
//! without requiring a full nodal linear solver.

mod ir;

pub use ir::*;
