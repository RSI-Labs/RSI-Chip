//! Focused probe for the RTL-reference LEC entry point.

fn main() {
    let source = format!(
        "{}\n{}\n{}{}",
        include_str!("../../../platform/rtl/i8086_prefix_decoder.sv"),
        include_str!("../../../platform/rtl/i8086_opcode_decoder.sv"),
        include_str!("../../../platform/rtl/i8086_modrm_decoder.sv"),
        include_str!("../../../platform/rtl/i8086_full_core.sv"),
    );
    let route = eda_api::RouteOptions::new(4096).expect("route options");
    let flow = eda_api::run_flow_with_rtl_reference_lec(
        &source,
        Some("i8086_full_core"),
        &eda_api::CellLibrary::std(),
        &route,
        &eda_api::FlowCancellation::new(),
    )
    .expect("RTL-reference LEC flow");
    println!(
        "equivalent={} gold_cells={} mapped_cells={} drc_errors={} delay_ns={}",
        flow.equivalent,
        flow.gold.stats().cells,
        flow.stats.cells,
        flow.drc
            .violations
            .iter()
            .filter(|violation| violation.severity == eda_pnr::DrcSeverity::Error)
            .count(),
        flow.timing.critical_path_delay
    );
}
