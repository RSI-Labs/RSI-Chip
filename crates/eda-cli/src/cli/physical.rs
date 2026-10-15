//! Power mesh, parasitic, and physical-view export helpers.

pub(crate) fn multilayer_mesh_options(rows: usize) -> eda_api::MultilayerPowerMeshOptions {
    let mut options = eda_api::MultilayerPowerMeshOptions {
        rows,
        ..Default::default()
    };
    options.use_boundary_taps = false;
    options.layers = vec![
        eda_api::PowerMeshLayer::new("M8", 0.02, 0.02, 0.02, 0.02),
        eda_api::PowerMeshLayer::new("M7", 0.03, 0.03, 0.03, 0.03),
    ];
    for row in 0..options.rows {
        for column in 0..options.columns {
            if row == 0 || row + 1 == options.rows || column == 0 || column + 1 == options.columns {
                options.taps.push(eda_api::PowerMeshLayerTap {
                    layer: 0,
                    row,
                    column,
                    vdd: true,
                    gnd: true,
                });
            }
            options
                .vias
                .push(eda_api::PowerMeshVia::new(row, column, 0, 1, 0.02, 0.02));
        }
    }
    options
}

pub(crate) fn coupled_multilayer_mesh_options(
    rows: usize,
    supplies: &[eda_api::MultilayerPowerDomainSupply],
) -> eda_api::MultilayerPowerMeshOptions {
    let mut options = multilayer_mesh_options(rows);
    for tap in &mut options.taps {
        tap.vdd = false;
    }
    options.power_domains = supplies.to_vec();
    options
}

pub(crate) fn write_extracted_spef(
    path: &str,
    netlist: &eda_core::netlist::Netlist,
    routing: &eda_pnr::Routing,
) -> Result<(), String> {
    let parasitics = eda_api::extract_interconnect_parasitics(
        netlist,
        routing,
        &eda_api::InterconnectModel::default(),
    )?;
    let text = eda_api::write_parasitics(&parasitics)?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn write_best_physical_timing_outputs(
    lef_out: Option<&String>,
    def_out: Option<&String>,
    spef_out: Option<&String>,
    sdf_out: Option<&String>,
    verilog_out: Option<&String>,
    netlist: &eda_core::netlist::Netlist,
    placement: &eda_api::Placement,
    routing: &eda_pnr::Routing,
    parasitics: &eda_api::Spef,
    sdf: &eda_api::Sdf,
) -> Result<(), String> {
    if let Some(path) = lef_out {
        let lef = eda_pnr::write_lef(netlist, placement)?;
        std::fs::write(path, lef).map_err(|e| e.to_string())?;
    }
    if let Some(path) = def_out {
        let def = eda_pnr::write_def(netlist, placement, routing)?;
        std::fs::write(path, def).map_err(|e| e.to_string())?;
    }
    if let Some(path) = spef_out {
        let text = eda_api::write_parasitics(parasitics)?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    if let Some(path) = sdf_out {
        let text = eda_api::write_delay_file(sdf)?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    if let Some(path) = verilog_out {
        let text = eda_api::export_verilog(netlist)?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    Ok(())
}
