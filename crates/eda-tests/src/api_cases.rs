//! End-to-end API integration cases split by facade domain.

use crate::CaseList;

mod frontend_formal;
mod physical_import;
mod physical_timing;
mod timing_objects;
mod timing_sdc;

pub fn register(c: &mut CaseList) {
    frontend_formal::register(c);
    timing_sdc::register(c);
    timing_objects::register(c);
    physical_timing::register(c);
    physical_import::register(c);
}

fn scaled_timing_view(
    nl: &eda_core::netlist::Netlist,
    timing_scale: f64,
) -> Result<eda_core::netlist::Netlist, String> {
    use std::sync::Arc;

    let mut view = nl.clone();
    let library = view
        .library()
        .cloned()
        .ok_or_else(|| "physical timing view requires a library".to_string())?;
    let mut scaled = (*library).clone();
    for cell in &mut scaled.cells {
        for (_, delay) in &mut cell.arcs {
            *delay *= timing_scale;
        }
    }
    view.set_library(Arc::new(scaled));
    Ok(view)
}
