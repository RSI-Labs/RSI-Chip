//! SystemVerilog class, composition, string, and polymorphism cases.

mod classes;
mod composition;
mod polymorphism;
mod shared;
mod strings;

pub(super) use shared::expect_net;

use crate::CaseList;

pub(super) fn register(c: &mut CaseList) {
    classes::register(c);
    composition::register(c);
    strings::register(c);
    polymorphism::register(c);
}
