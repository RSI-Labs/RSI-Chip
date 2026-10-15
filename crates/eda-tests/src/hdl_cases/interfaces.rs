//! Interface structure and property integration cases split by domain.

use crate::CaseList;

mod properties;
mod structure;

pub(super) fn register(c: &mut CaseList) {
    structure::register(c);
    properties::register(c);
}
