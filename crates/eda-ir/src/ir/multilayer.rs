//! Multilayer power-mesh solving, currents, EM lifetime, and reports.

use super::types::*;
use eda_core::netlist::Netlist;
use eda_pnr::{Placement, SITES_PER_ROW};
use eda_power::PowerReport;
use std::collections::{BTreeMap, BTreeSet};

fn validate_multilayer_power_mesh_options(
    options: &MultilayerPowerMeshOptions,
) -> Result<(), String> {
    if options.rows == 0 || options.columns == 0 {
        return Err("multilayer power mesh requires positive row and column counts".into());
    }
    if options.layers.is_empty() {
        return Err("multilayer power mesh requires at least one layer".into());
    }
    let mut layer_names = BTreeSet::new();
    for (index, layer) in options.layers.iter().enumerate() {
        if layer.name.trim().is_empty() {
            return Err(format!("power mesh layer {} name cannot be empty", index));
        }
        if !layer_names.insert(layer.name.as_str()) {
            return Err(format!("duplicate power mesh layer '{}'", layer.name));
        }
        for (name, value) in [
            (
                "VDD horizontal resistance",
                layer.vdd_horizontal_ohm_per_segment,
            ),
            (
                "VDD vertical resistance",
                layer.vdd_vertical_ohm_per_segment,
            ),
            (
                "GND horizontal resistance",
                layer.gnd_horizontal_ohm_per_segment,
            ),
            (
                "GND vertical resistance",
                layer.gnd_vertical_ohm_per_segment,
            ),
            (
                "horizontal cross-section",
                layer.horizontal_cross_section_um2,
            ),
            ("vertical cross-section", layer.vertical_cross_section_um2),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!(
                    "power mesh layer '{}' has invalid {} {}",
                    layer.name, name, value
                ));
            }
        }
    }
    for (index, via) in options.vias.iter().enumerate() {
        if via.row >= options.rows
            || via.column >= options.columns
            || via.from_layer >= options.layers.len()
            || via.to_layer >= options.layers.len()
        {
            return Err(format!(
                "power mesh via {} location is outside the mesh",
                index
            ));
        }
        if via.from_layer == via.to_layer {
            return Err(format!(
                "power mesh via {} must connect distinct layers",
                index
            ));
        }
        for (name, value) in [
            ("VDD resistance", via.vdd_resistance_ohm),
            ("GND resistance", via.gnd_resistance_ohm),
            ("area", via.area_um2),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!(
                    "power mesh via {} has invalid {} {}",
                    index, name, value
                ));
            }
        }
    }
    for (index, layer) in options.cell_layers.iter().enumerate() {
        if *layer >= options.layers.len() {
            return Err(format!(
                "power mesh cell layer mapping {} references undefined layer {}",
                index, layer
            ));
        }
    }
    for (index, tap) in options.taps.iter().enumerate() {
        if tap.layer >= options.layers.len()
            || tap.row >= options.rows
            || tap.column >= options.columns
        {
            return Err(format!(
                "power mesh tap {} location is outside the mesh",
                index
            ));
        }
        if !tap.vdd && !tap.gnd {
            return Err(format!("power mesh tap {} must enable VDD or GND", index));
        }
    }
    let mut domain_names = BTreeSet::new();
    let mut has_domain_vdd = false;
    let mut has_domain_gnd = false;
    for (index, domain) in options.power_domains.iter().enumerate() {
        if domain.name.trim().is_empty() {
            return Err(format!("power mesh domain {} name cannot be empty", index));
        }
        if !domain_names.insert(domain.name.as_str()) {
            return Err(format!("duplicate power mesh domain '{}'", domain.name));
        }
        if !domain.voltage_v.is_finite() || domain.voltage_v <= 0.0 {
            return Err(format!(
                "power mesh domain '{}' has invalid voltage {}",
                domain.name, domain.voltage_v
            ));
        }
        if domain.taps.is_empty() {
            return Err(format!(
                "power mesh domain '{}' requires at least one tap",
                domain.name
            ));
        }
        if !domain.taps.iter().any(|tap| tap.vdd) {
            return Err(format!(
                "power mesh domain '{}' requires at least one VDD tap",
                domain.name
            ));
        }
        for (tap_index, tap) in domain.taps.iter().enumerate() {
            if tap.layer >= options.layers.len()
                || tap.row >= options.rows
                || tap.column >= options.columns
            {
                return Err(format!(
                    "power mesh domain '{}' tap {} location is outside the mesh",
                    domain.name, tap_index
                ));
            }
            if !tap.vdd && !tap.gnd {
                return Err(format!(
                    "power mesh domain '{}' tap {} must enable VDD or GND",
                    domain.name, tap_index
                ));
            }
            has_domain_vdd |= tap.vdd;
            has_domain_gnd |= tap.gnd;
        }
    }
    for (index, domain) in options.cell_power_domains.iter().enumerate() {
        if !domain_names.contains(domain.as_str()) {
            return Err(format!(
                "power mesh cell domain mapping {} references undefined domain '{}'",
                index, domain
            ));
        }
    }
    for (name, value) in [
        ("voltage", options.voltage_v),
        ("maximum drop", options.max_total_drop_v),
        ("maximum node current", options.max_node_current_a),
        ("maximum segment current", options.max_segment_current_a),
        ("tolerance", options.tolerance_v),
    ] {
        if !value.is_finite() || value <= 0.0 {
            return Err(format!("invalid multilayer power mesh {} {}", name, value));
        }
    }
    if options.max_iterations == 0 {
        return Err("multilayer power mesh requires a positive iteration limit".into());
    }
    validate_power_mesh_em_options(&options.em)?;
    if !options.use_boundary_taps {
        if !options.taps.iter().any(|tap| tap.vdd) && !has_domain_vdd {
            return Err("multilayer power mesh requires at least one VDD tap".into());
        }
        if !options.taps.iter().any(|tap| tap.gnd) && !has_domain_gnd {
            return Err("multilayer power mesh requires at least one GND tap".into());
        }
    }
    Ok(())
}

fn validate_power_mesh_em_options(options: &PowerMeshEmOptions) -> Result<(), String> {
    if !options.enabled {
        return Ok(());
    }
    for (name, value) in [
        ("temperature", options.temperature_k),
        ("reference temperature", options.reference_temperature_k),
        (
            "reference current density",
            options.reference_current_density_a_per_um2,
        ),
        ("reference lifetime", options.reference_lifetime_hours),
        ("activation energy", options.activation_energy_ev),
        ("minimum lifetime", options.min_lifetime_hours),
    ] {
        if !value.is_finite() || value <= 0.0 {
            return Err(format!("invalid power mesh EM {} {}", name, value));
        }
    }
    if !options.current_density_exponent.is_finite() || options.current_density_exponent <= 0.0 {
        return Err(format!(
            "invalid power mesh EM current density exponent {}",
            options.current_density_exponent
        ));
    }
    Ok(())
}

/// Estimate relative EM lifetime using Black's equation.
///
/// The result is normalized to a user-supplied lifetime at a reference current
/// density and temperature. It is suitable for relative what-if analysis, not
/// as a foundry-qualified absolute lifetime signoff value.
pub fn estimate_power_mesh_em_lifetime(
    current_density_a_per_um2: f64,
    options: &PowerMeshEmOptions,
) -> Result<f64, String> {
    validate_power_mesh_em_options(options)?;
    if !current_density_a_per_um2.is_finite() || current_density_a_per_um2 < 0.0 {
        return Err(format!(
            "invalid EM current density {} A/um^2",
            current_density_a_per_um2
        ));
    }
    if current_density_a_per_um2 == 0.0 {
        return Ok(f64::INFINITY);
    }
    const BOLTZMANN_EV_PER_K: f64 = 8.617_333_262_145e-5;
    let density_ratio = options.reference_current_density_a_per_um2 / current_density_a_per_um2;
    let temperature_ratio = ((options.activation_energy_ev / BOLTZMANN_EV_PER_K)
        * (1.0 / options.temperature_k - 1.0 / options.reference_temperature_k))
        .exp();
    Ok(options.reference_lifetime_hours
        * density_ratio.powf(options.current_density_exponent)
        * temperature_ratio)
}

fn power_mesh_segment_em_values(
    current_a: f64,
    area_um2: f64,
    options: &MultilayerPowerMeshOptions,
) -> Result<(f64, f64), String> {
    let current_density = current_a / area_um2;
    let lifetime = if options.em.enabled {
        estimate_power_mesh_em_lifetime(current_density, &options.em)?
    } else {
        f64::INFINITY
    };
    Ok((current_density, lifetime))
}

#[allow(clippy::needless_range_loop)]
fn multilayer_tap_mask(
    options: &MultilayerPowerMeshOptions,
    select_vdd: bool,
) -> Vec<Vec<Vec<bool>>> {
    let mut taps = vec![vec![vec![false; options.columns]; options.rows]; options.layers.len()];
    if options.use_boundary_taps {
        for layer in 0..options.layers.len() {
            for row in 0..options.rows {
                for column in 0..options.columns {
                    if row == 0
                        || row + 1 == options.rows
                        || column == 0
                        || column + 1 == options.columns
                    {
                        taps[layer][row][column] = true;
                    }
                }
            }
        }
    }
    for tap in &options.taps {
        if select_vdd {
            taps[tap.layer][tap.row][tap.column] |= tap.vdd;
        } else {
            taps[tap.layer][tap.row][tap.column] |= tap.gnd;
        }
    }
    taps
}

fn multilayer_fixed_voltages(
    options: &MultilayerPowerMeshOptions,
    network: PowerMeshNetwork,
) -> Result<Vec<Vec<Vec<Option<f64>>>>, String> {
    let taps = multilayer_tap_mask(options, network == PowerMeshNetwork::Vdd);
    let global_voltage = if network == PowerMeshNetwork::Vdd {
        options.voltage_v
    } else {
        0.0
    };
    let mut fixed =
        vec![vec![vec![None::<f64>; options.columns]; options.rows]; options.layers.len()];
    for layer in 0..options.layers.len() {
        for row in 0..options.rows {
            for column in 0..options.columns {
                if taps[layer][row][column] {
                    fixed[layer][row][column] = Some(global_voltage);
                }
            }
        }
    }
    for domain in &options.power_domains {
        for tap in &domain.taps {
            let enabled = if network == PowerMeshNetwork::Vdd {
                tap.vdd
            } else {
                tap.gnd
            };
            if !enabled {
                continue;
            }
            let voltage = if network == PowerMeshNetwork::Vdd {
                domain.voltage_v
            } else {
                0.0
            };
            let existing = &mut fixed[tap.layer][tap.row][tap.column];
            match *existing {
                Some(current) if (current - voltage).abs() > 1e-12 => {
                    return Err(format!(
                        "multilayer power mesh location ({},{},{}) has conflicting {} fixed voltages {} and {}",
                        tap.layer,
                        tap.row,
                        tap.column,
                        if network == PowerMeshNetwork::Vdd { "VDD" } else { "GND" },
                        current,
                        voltage
                    ));
                }
                _ => *existing = Some(voltage),
            }
        }
    }
    Ok(fixed)
}

type MultilayerVoltage = Vec<Vec<Vec<f64>>>;
type MultilayerMeshSolve = (MultilayerVoltage, usize, f64);

#[allow(clippy::too_many_arguments)]
#[allow(clippy::needless_range_loop)]
fn solve_multilayer_power_mesh_network(
    options: &MultilayerPowerMeshOptions,
    network: PowerMeshNetwork,
    injected_current: &[Vec<Vec<f64>>],
    initial_voltage_v: f64,
    current_sign: f64,
    fixed_voltage: &[Vec<Vec<Option<f64>>>],
) -> Result<MultilayerMeshSolve, String> {
    let mut voltage =
        vec![vec![vec![initial_voltage_v; options.columns]; options.rows]; options.layers.len()];
    let mut iterations = 0;
    let mut final_delta = f64::INFINITY;
    while iterations < options.max_iterations {
        let mut max_delta = 0.0_f64;
        for layer in 0..options.layers.len() {
            for row in 0..options.rows {
                for column in 0..options.columns {
                    if let Some(fixed) = fixed_voltage[layer][row][column] {
                        voltage[layer][row][column] = fixed;
                        continue;
                    }
                    let mut conductance_sum = 0.0_f64;
                    let mut neighbor_voltage_sum = 0.0_f64;
                    let layer_resistance = &options.layers[layer];
                    let (horizontal, vertical) = match network {
                        PowerMeshNetwork::Vdd => (
                            layer_resistance.vdd_horizontal_ohm_per_segment,
                            layer_resistance.vdd_vertical_ohm_per_segment,
                        ),
                        PowerMeshNetwork::Gnd => (
                            layer_resistance.gnd_horizontal_ohm_per_segment,
                            layer_resistance.gnd_vertical_ohm_per_segment,
                        ),
                    };
                    if row > 0 {
                        let conductance = 1.0 / vertical;
                        conductance_sum += conductance;
                        neighbor_voltage_sum += conductance * voltage[layer][row - 1][column];
                    }
                    if row + 1 < options.rows {
                        let conductance = 1.0 / vertical;
                        conductance_sum += conductance;
                        neighbor_voltage_sum += conductance * voltage[layer][row + 1][column];
                    }
                    if column > 0 {
                        let conductance = 1.0 / horizontal;
                        conductance_sum += conductance;
                        neighbor_voltage_sum += conductance * voltage[layer][row][column - 1];
                    }
                    if column + 1 < options.columns {
                        let conductance = 1.0 / horizontal;
                        conductance_sum += conductance;
                        neighbor_voltage_sum += conductance * voltage[layer][row][column + 1];
                    }
                    for via in &options.vias {
                        let resistance = match network {
                            PowerMeshNetwork::Vdd => via.vdd_resistance_ohm,
                            PowerMeshNetwork::Gnd => via.gnd_resistance_ohm,
                        };
                        if via.row == row
                            && via.column == column
                            && (via.from_layer == layer || via.to_layer == layer)
                        {
                            let other_layer = if via.from_layer == layer {
                                via.to_layer
                            } else {
                                via.from_layer
                            };
                            let conductance = 1.0 / resistance;
                            conductance_sum += conductance;
                            neighbor_voltage_sum += conductance * voltage[other_layer][row][column];
                        }
                    }
                    if conductance_sum <= f64::EPSILON {
                        return Err(format!(
                            "multilayer power mesh node layer {} ({},{}) is disconnected",
                            layer, row, column
                        ));
                    }
                    let updated = (neighbor_voltage_sum
                        + current_sign * injected_current[layer][row][column])
                        / conductance_sum;
                    let delta = (updated - voltage[layer][row][column]).abs();
                    max_delta = max_delta.max(delta);
                    voltage[layer][row][column] = updated;
                }
            }
        }
        iterations += 1;
        final_delta = max_delta;
        if max_delta <= options.tolerance_v {
            return Ok((voltage, iterations, final_delta));
        }
    }
    Err(format!(
        "multilayer power mesh solver did not converge: final delta {:.6e} V after {} iterations",
        final_delta, iterations
    ))
}

/// Solve a layer/via-aware VDD/GND mesh from explicit cell currents.
#[allow(clippy::needless_range_loop)]
pub fn analyze_multilayer_mesh_currents(
    nl: &Netlist,
    placement: &Placement,
    cell_current_a: &[f64],
    total_power_uw: f64,
    options: &MultilayerPowerMeshOptions,
) -> Result<MultilayerPowerMeshResult, String> {
    if placement.cell_pos.len() != nl.cell_count() || placement.cell_width.len() != nl.cell_count()
    {
        return Err("multilayer power mesh placement/netlist cell count mismatch".into());
    }
    if cell_current_a.len() != nl.cell_count() {
        return Err("multilayer power mesh current/netlist cell count mismatch".into());
    }
    if !options.cell_layers.is_empty() && options.cell_layers.len() != nl.cell_count() {
        return Err("multilayer power mesh cell layer mapping count mismatch".into());
    }
    if !options.cell_power_domains.is_empty() && options.cell_power_domains.len() != nl.cell_count()
    {
        return Err("multilayer power mesh cell domain mapping count mismatch".into());
    }
    validate_multilayer_power_mesh_options(options)?;

    let domain_voltages = options
        .power_domains
        .iter()
        .map(|domain| (domain.name.as_str(), domain.voltage_v))
        .collect::<BTreeMap<_, _>>();
    let reference_vdd_v = options
        .power_domains
        .iter()
        .map(|domain| domain.voltage_v)
        .fold(options.voltage_v, f64::max);

    let mut injected_current =
        vec![vec![vec![0.0_f64; options.columns]; options.rows]; options.layers.len()];
    let mut cells = Vec::with_capacity(nl.cell_count());
    for cell_index in 0..nl.cell_count() {
        let position = &placement.cell_pos[cell_index];
        if position.row >= placement.rows {
            return Err(format!(
                "cell '{}' row {} is outside placement rows {}",
                nl.cell(cell_index).name,
                position.row,
                placement.rows
            ));
        }
        let physical_row = position.row as f64 + 0.5;
        let mesh_row =
            ((physical_row * options.rows as f64) / placement.rows.max(1) as f64).floor();
        let mesh_row = mesh_row.clamp(0.0, (options.rows - 1) as f64) as usize;
        let center_site =
            position.site as f64 + placement.cell_width[cell_index].max(1) as f64 / 2.0;
        let normalized_column = (center_site / SITES_PER_ROW as f64).clamp(0.0, 0.999999);
        let mesh_column = (normalized_column * options.columns as f64).floor();
        let mesh_column = mesh_column.clamp(0.0, (options.columns - 1) as f64) as usize;
        let layer = options.cell_layers.get(cell_index).copied().unwrap_or(0);
        let power_domain = options.cell_power_domains.get(cell_index).cloned();
        let supply_voltage_v = power_domain
            .as_deref()
            .and_then(|domain| domain_voltages.get(domain).copied());
        let current_a = cell_current_a[cell_index];
        if !current_a.is_finite() || current_a < 0.0 {
            return Err(format!(
                "cell '{}' has invalid multilayer power-mesh current {}",
                nl.cell(cell_index).name,
                current_a
            ));
        }
        injected_current[layer][mesh_row][mesh_column] += current_a;
        cells.push(MultilayerPowerMeshCell {
            cell: cell_index,
            name: nl.cell(cell_index).name.clone(),
            layer,
            mesh_row,
            mesh_column,
            current_a,
            local_total_drop_v: 0.0,
            power_domain,
            supply_voltage_v,
        });
    }

    let vdd_fixed = multilayer_fixed_voltages(options, PowerMeshNetwork::Vdd)?;
    let gnd_fixed = multilayer_fixed_voltages(options, PowerMeshNetwork::Gnd)?;
    let (vdd, vdd_iterations, vdd_delta) = solve_multilayer_power_mesh_network(
        options,
        PowerMeshNetwork::Vdd,
        &injected_current,
        reference_vdd_v,
        -1.0,
        &vdd_fixed,
    )?;
    let (gnd, gnd_iterations, gnd_delta) = solve_multilayer_power_mesh_network(
        options,
        PowerMeshNetwork::Gnd,
        &injected_current,
        0.0,
        1.0,
        &gnd_fixed,
    )?;

    let mut nodes = Vec::with_capacity(options.layers.len() * options.rows * options.columns);
    let mut worst_drop = 0.0_f64;
    let mut worst_node = None;
    for layer in 0..options.layers.len() {
        for row in 0..options.rows {
            for column in 0..options.columns {
                let vdd_drop = (reference_vdd_v - vdd[layer][row][column]).max(0.0);
                let gnd_drop = gnd[layer][row][column].max(0.0);
                let total_drop = vdd_drop + gnd_drop;
                if total_drop > worst_drop {
                    worst_drop = total_drop;
                    worst_node = Some((layer, row, column));
                }
                nodes.push(MultilayerPowerMeshNode {
                    layer,
                    layer_name: options.layers[layer].name.clone(),
                    row,
                    column,
                    vdd_v: vdd[layer][row][column],
                    gnd_v: gnd[layer][row][column],
                    vdd_drop_v: vdd_drop,
                    gnd_drop_v: gnd_drop,
                    total_drop_v: total_drop,
                    injected_current_a: injected_current[layer][row][column],
                });
            }
        }
    }
    let node_index = |layer: usize, row: usize, column: usize| {
        (layer * options.rows + row) * options.columns + column
    };
    for cell in &mut cells {
        let node = &nodes[node_index(cell.layer, cell.mesh_row, cell.mesh_column)];
        let reference = cell.supply_voltage_v.unwrap_or(options.voltage_v);
        let local_vdd_drop = (reference - node.vdd_v).max(0.0);
        cell.local_total_drop_v = local_vdd_drop + node.gnd_drop_v;
    }

    let mut segments = Vec::new();
    for (network, voltages) in [(PowerMeshNetwork::Vdd, &vdd), (PowerMeshNetwork::Gnd, &gnd)] {
        for (layer, layer_options) in options.layers.iter().enumerate() {
            let (horizontal, vertical) = match network {
                PowerMeshNetwork::Vdd => (
                    layer_options.vdd_horizontal_ohm_per_segment,
                    layer_options.vdd_vertical_ohm_per_segment,
                ),
                PowerMeshNetwork::Gnd => (
                    layer_options.gnd_horizontal_ohm_per_segment,
                    layer_options.gnd_vertical_ohm_per_segment,
                ),
            };
            for row in 0..options.rows {
                for column in 0..options.columns.saturating_sub(1) {
                    let voltage_drop =
                        voltages[layer][row][column] - voltages[layer][row][column + 1];
                    let current = (voltage_drop / horizontal).abs();
                    let (current_density, em_lifetime) = power_mesh_segment_em_values(
                        current,
                        layer_options.horizontal_cross_section_um2,
                        options,
                    )?;
                    segments.push(MultilayerPowerMeshSegment {
                        network,
                        orientation: PowerMeshSegmentOrientation::Horizontal,
                        from_layer: layer,
                        from_row: row,
                        from_column: column,
                        to_layer: layer,
                        to_row: row,
                        to_column: column + 1,
                        resistance_ohm: horizontal,
                        voltage_drop_v: voltage_drop,
                        current_a: current,
                        current_density_a_per_um2: current_density,
                        em_lifetime_hours: em_lifetime,
                    });
                }
            }
            for row in 0..options.rows.saturating_sub(1) {
                for column in 0..options.columns {
                    let voltage_drop =
                        voltages[layer][row][column] - voltages[layer][row + 1][column];
                    let current = (voltage_drop / vertical).abs();
                    let (current_density, em_lifetime) = power_mesh_segment_em_values(
                        current,
                        layer_options.vertical_cross_section_um2,
                        options,
                    )?;
                    segments.push(MultilayerPowerMeshSegment {
                        network,
                        orientation: PowerMeshSegmentOrientation::Vertical,
                        from_layer: layer,
                        from_row: row,
                        from_column: column,
                        to_layer: layer,
                        to_row: row + 1,
                        to_column: column,
                        resistance_ohm: vertical,
                        voltage_drop_v: voltage_drop,
                        current_a: current,
                        current_density_a_per_um2: current_density,
                        em_lifetime_hours: em_lifetime,
                    });
                }
            }
        }
        for via in &options.vias {
            let (from_layer, to_layer) = (via.from_layer, via.to_layer);
            let resistance = match network {
                PowerMeshNetwork::Vdd => via.vdd_resistance_ohm,
                PowerMeshNetwork::Gnd => via.gnd_resistance_ohm,
            };
            let voltage_drop =
                voltages[from_layer][via.row][via.column] - voltages[to_layer][via.row][via.column];
            let current = (voltage_drop / resistance).abs();
            let (current_density, em_lifetime) =
                power_mesh_segment_em_values(current, via.area_um2, options)?;
            segments.push(MultilayerPowerMeshSegment {
                network,
                orientation: PowerMeshSegmentOrientation::Via,
                from_layer,
                from_row: via.row,
                from_column: via.column,
                to_layer,
                to_row: via.row,
                to_column: via.column,
                resistance_ohm: resistance,
                voltage_drop_v: voltage_drop,
                current_a: current,
                current_density_a_per_um2: current_density,
                em_lifetime_hours: em_lifetime,
            });
        }
    }

    let total_current_a: f64 = cell_current_a.iter().sum();
    let worst_segment_current_a = segments
        .iter()
        .map(|segment| segment.current_a)
        .fold(0.0, f64::max);
    let worst_em_lifetime_hours = segments
        .iter()
        .map(|segment| segment.em_lifetime_hours)
        .reduce(f64::min)
        .unwrap_or(f64::INFINITY);
    let mut findings = Vec::new();
    if worst_drop > options.max_total_drop_v {
        findings.push(IrFinding {
            code: "MULTILAYER-MESH-IR-DROP".into(),
            severity: IrSeverity::Error,
            row: worst_node.map(|(_, row, _)| row),
            message: format!(
                "worst multilayer power-mesh drop {:.6} V at {:?} exceeds {:.6} V",
                worst_drop, worst_node, options.max_total_drop_v
            ),
        });
    }
    for node in &nodes {
        if node.injected_current_a > options.max_node_current_a {
            findings.push(IrFinding {
                code: "MULTILAYER-MESH-NODE-CURRENT".into(),
                severity: IrSeverity::Error,
                row: Some(node.row),
                message: format!(
                    "layer {} mesh node ({},{}) current {:.6} A exceeds {:.6} A",
                    node.layer_name,
                    node.row,
                    node.column,
                    node.injected_current_a,
                    options.max_node_current_a
                ),
            });
        }
    }
    for segment in &segments {
        if segment.current_a > options.max_segment_current_a {
            findings.push(IrFinding {
                code: "MULTILAYER-MESH-SEGMENT-CURRENT".into(),
                severity: IrSeverity::Error,
                row: Some(segment.from_row),
                message: format!(
                    "{:?} layer segment {:?} {} ({},{},{})-{} ({},{},{}) current {:.6} A exceeds {:.6} A",
                    segment.network,
                    segment.orientation,
                    options.layers[segment.from_layer].name,
                    segment.from_row,
                    segment.from_column,
                    segment.from_layer,
                    options.layers[segment.to_layer].name,
                    segment.to_row,
                    segment.to_column,
                    segment.to_layer,
                    segment.current_a,
                    options.max_segment_current_a
                ),
            });
        }
        if options.em.enabled
            && segment.em_lifetime_hours.is_finite()
            && segment.em_lifetime_hours < options.em.min_lifetime_hours
        {
            findings.push(IrFinding {
                code: "MULTILAYER-MESH-EM-LIFETIME".into(),
                severity: IrSeverity::Error,
                row: Some(segment.from_row),
                message: format!(
                    "{:?} {} segment {:?} ({},{},{})-{} ({},{},{}) current density {:.6} A/um^2 has EM lifetime {:.3e} h below {:.3e} h",
                    segment.network,
                    options.layers[segment.from_layer].name,
                    segment.orientation,
                    segment.from_row,
                    segment.from_column,
                    segment.from_layer,
                    options.layers[segment.to_layer].name,
                    segment.to_row,
                    segment.to_column,
                    segment.to_layer,
                    segment.current_density_a_per_um2,
                    segment.em_lifetime_hours,
                    options.em.min_lifetime_hours
                ),
            });
        }
    }

    let power_domain_results = options
        .power_domains
        .iter()
        .map(|supply| {
            let domain_cells = cells
                .iter()
                .filter(|cell| cell.power_domain.as_deref() == Some(supply.name.as_str()))
                .collect::<Vec<_>>();
            MultilayerPowerDomainResult {
                name: supply.name.clone(),
                voltage_v: supply.voltage_v,
                cells: domain_cells.len(),
                total_current_a: domain_cells.iter().map(|cell| cell.current_a).sum(),
                worst_local_drop_v: domain_cells
                    .iter()
                    .map(|cell| cell.local_total_drop_v)
                    .fold(0.0, f64::max),
            }
        })
        .collect::<Vec<_>>();

    Ok(MultilayerPowerMeshResult {
        voltage_v: options.voltage_v,
        total_power_uw,
        total_current_a,
        worst_drop_v: worst_drop,
        worst_node,
        worst_segment_current_a,
        worst_em_lifetime_hours,
        nodes,
        cells,
        segments,
        iterations: vdd_iterations.max(gnd_iterations),
        final_delta_v: vdd_delta.max(gnd_delta),
        findings,
        power_domains: power_domain_results,
    })
}

/// Analyze a multilayer mesh from a switching-power report.
pub fn analyze_multilayer_mesh(
    nl: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    options: &MultilayerPowerMeshOptions,
) -> Result<MultilayerPowerMeshResult, String> {
    if power.cells.len() != nl.cell_count() {
        return Err("multilayer power mesh power/netlist cell count mismatch".into());
    }
    let total_sites: usize = placement.cell_width.iter().sum();
    let total_sites = total_sites.max(1);
    let mut cell_power = vec![0.0_f64; nl.cell_count()];
    for report_cell in &power.cells {
        if report_cell.cell >= nl.cell_count() {
            return Err(format!(
                "power report cell index {} is out of range",
                report_cell.cell
            ));
        }
        let leakage_share = power.static_power_uw * placement.cell_width[report_cell.cell] as f64
            / total_sites as f64;
        cell_power[report_cell.cell] = report_cell.dynamic_power_uw + leakage_share;
    }
    let cell_current = cell_power
        .iter()
        .map(|power_uw| power_uw / options.voltage_v / 1_000_000.0)
        .collect::<Vec<_>>();
    let total_power_uw: f64 = cell_power.iter().sum();
    analyze_multilayer_mesh_currents(nl, placement, &cell_current, total_power_uw, options)
}

/// Render a layer/via-aware power-mesh report.
pub fn multilayer_power_mesh_report(result: &MultilayerPowerMeshResult) -> String {
    let mut out = format!(
        "Multilayer power-mesh IR report: nodes={}, cells={}, segments={}, iterations={}, final_delta={:.6e} V\n",
        result.nodes.len(),
        result.cells.len(),
        result.segments.len(),
        result.iterations,
        result.final_delta_v
    );
    for node in result
        .nodes
        .iter()
        .filter(|node| node.injected_current_a > 0.0)
    {
        out.push_str(&format!(
            "  layer '{}' ({},{}): current {:.6} A, total drop {:.6} V\n",
            node.layer_name, node.row, node.column, node.injected_current_a, node.total_drop_v
        ));
    }
    for domain in &result.power_domains {
        out.push_str(&format!(
            "  domain '{}' V={:.6} cells={} I={:.9} A worst_local_drop={:.9} V\n",
            domain.name,
            domain.voltage_v,
            domain.cells,
            domain.total_current_a,
            domain.worst_local_drop_v
        ));
    }
    for segment in result
        .segments
        .iter()
        .filter(|segment| segment.orientation == PowerMeshSegmentOrientation::Via)
    {
        out.push_str(&format!(
            "  via {:?} ({},{}) {}->{}: current {:.6} A, J {:.6} A/um^2, EM {:.3e} h\n",
            segment.network,
            segment.from_row,
            segment.from_column,
            segment.from_layer,
            segment.to_layer,
            segment.current_a,
            segment.current_density_a_per_um2,
            segment.em_lifetime_hours
        ));
    }
    out.push_str(&format!(
        "  worst drop: {:.6} V at {:?}; worst segment current: {:.6} A; worst EM {:.3e} h; errors={}\n",
        result.worst_drop_v,
        result.worst_node,
        result.worst_segment_current_a,
        result.worst_em_lifetime_hours,
        result.has_errors()
    ));
    for finding in &result.findings {
        out.push_str(&format!("  {}: {}\n", finding.code, finding.message));
    }
    out
}
