//! Formal property implementation module.

use eda_assert::{AssertionKind, BoolExpr, Property};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyBmcStatus {
    /// No counterexample exists in the bounded horizon.
    BoundedPass,
    /// A complete violating temporal window was found.
    Violated,
    /// A cover property was satisfied within the bounded horizon.
    Satisfied,
    /// A cover property had no complete satisfying witness in the horizon.
    NotSatisfied,
    /// The SVA assumptions are inconsistent, so the target result is vacuous.
    AssumptionsUnsat,
}

#[derive(Clone, Debug)]
pub struct PropertyBmcResult {
    pub assertion: String,
    pub kind: AssertionKind,
    pub property: Property,
    pub disable: Option<BoolExpr>,
    pub clock: Option<String>,
    pub status: PropertyBmcStatus,
    /// Per-frame primary-input assignments.
    pub trace: Option<Vec<HashMap<String, u64>>>,
    /// Per-frame values for every scalar net, including state and outputs.
    pub net_trace: Option<Vec<HashMap<String, u64>>>,
    /// Number of antecedent cycles considered.
    pub depth_checked: usize,
    /// Total SAT frames, including consequence lookahead.
    pub frames_checked: usize,
    /// Frame containing a true antecedent whose window failed.
    pub trigger_frame: Option<usize>,
    /// Number of consecutive antecedent samples selected for a repeated
    /// antecedent witness or counterexample.
    pub repeat_count: Option<usize>,
    /// Consequence frame selected for a repeated or nonconsecutive witness.
    pub end_frame: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PropertyReplayResult {
    pub frames_verified: usize,
    pub combinational_cells_checked: usize,
    pub transitions_verified: usize,
    pub property_violation_reproduced: bool,
}

impl PropertyBmcResult {
    pub fn violated(&self) -> bool {
        self.status == PropertyBmcStatus::Violated
    }

    pub fn covered(&self) -> bool {
        self.status == PropertyBmcStatus::Satisfied
    }
}
