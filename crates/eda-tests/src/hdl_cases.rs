//! Data-driven HDL frontend, elaboration, class, property, and UVM cases.

use crate::CaseList;

mod arrays;
mod classes;
mod functions;
mod interfaces;

pub fn register(c: &mut CaseList) {
    arrays::register(c);
    functions::register(c);
    interfaces::register(c);
    classes::register(c);
}
