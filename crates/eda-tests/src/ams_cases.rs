//! Data-driven analog/mixed-signal integration cases split by analysis domain.

use crate::CaseList;

mod basics;
mod bsim3;
mod dynamics;
mod optimization;
mod reference;
mod sensitivity;
mod statistical;

pub const AMS_CASE_COUNT: usize = 3110;

pub fn register(cases: &mut CaseList) {
    basics::register(cases);
    bsim3::register(cases);
    statistical::register(cases);
    dynamics::register(cases);
    sensitivity::register(cases);
    optimization::register(cases);
}
