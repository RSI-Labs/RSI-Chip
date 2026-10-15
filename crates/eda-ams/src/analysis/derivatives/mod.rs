//! Local and global sensitivity analysis split by numerical domain.

mod ac;
mod global;
mod local;
mod parameters;
mod transient;

pub use global::{global_sensitivity, global_sensitivity_cancellable};
pub use local::{derivative_analysis, sensitivity};
pub(super) use parameters::{
    perturbed_for_sensitivity, sensitivity_output, sensitivity_parameters, SensitivityParameter,
    SensitivityParameterKind,
};
