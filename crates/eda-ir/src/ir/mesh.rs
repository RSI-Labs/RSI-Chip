//! Single-layer static IR mesh analysis and current solving.

use super::types::*;
use super::validation::validate_f64;
use eda_core::netlist::Netlist;
use eda_pnr::{Placement, SITES_PER_ROW};
use eda_power::PowerReport;

pub fn analyze(
    nl: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    options: &IrOptions,
) -> Result<IrResult, String> {
    if placement.cell_pos.len() != nl.cell_count() || placement.cell_width.len() != nl.cell_count()
    {
        return Err("IR analysis placement/netlist cell count mismatch".into());
    }
    if power.cells.len() != nl.cell_count() {
        return Err("IR analysis power/netlist cell count mismatch".into());
    }
    validate_f64("voltage", options.voltage_v, true)?;
    validate_f64("VDD resistance", options.vdd_ohm_per_site, true)?;
    validate_f64("GND resistance", options.gnd_ohm_per_site, true)?;
    validate_f64("maximum drop", options.max_total_drop_v, true)?;
    validate_f64("maximum rail current", options.max_rail_current_a, true)?;

    // Leakage is reported only as a global estimate. Distribute it by cell
    // width, which is also how the power estimator modeled leakage.
    let total_sites: usize = placement.cell_width.iter().sum();
    let total_sites = total_sites.max(1);
    let mut cell_power = vec![0.0_f64; nl.cell_count()];
    for cp in &power.cells {
        if cp.cell >= nl.cell_count() {
            return Err(format!(
                "power report cell index {} is out of range",
                cp.cell
            ));
        }
        let leakage_share =
            power.static_power_uw * placement.cell_width[cp.cell] as f64 / total_sites as f64;
        cell_power[cp.cell] = cp.dynamic_power_uw + leakage_share;
    }

    // P[µW] = V[V] * I[µA]; convert µA to A with 1e6.
    let total_power_uw: f64 = cell_power.iter().sum();
    let total_current_a = total_power_uw / options.voltage_v / 1_000_000.0;
    let mut rows = Vec::new();
    let mut cells = Vec::with_capacity(nl.cell_count());
    let mut worst_drop = 0.0_f64;
    let mut worst_row = None;

    for row in 0..placement.rows.max(1) {
        let mut row_current = 0.0_f64;
        let mut vdd_drop = 0.0_f64;
        let mut gnd_drop = 0.0_f64;
        let mut row_cells = Vec::new();
        for (ci, cell_power_uw) in cell_power.iter().enumerate() {
            if placement.cell_pos[ci].row != row {
                continue;
            }
            let start = placement.cell_pos[ci].site;
            let width = placement.cell_width[ci].max(1);
            let end = (start + width).min(SITES_PER_ROW);
            let current_a = cell_power_uw / options.voltage_v / 1_000_000.0;
            row_current += current_a;

            // A current at x on a length-L rail splits between the two feeds:
            // x/L flows from the left and (L-x)/L from the right. The local
            // drop contribution is therefore I*R*x*(L-x)/L. This is exact for
            // a one-dimensional rail with both ends tied to ideal supplies.
            let center = (start + end) as f64 / 2.0;
            let length = SITES_PER_ROW as f64;
            let position = center.clamp(0.0, length);
            let factor = position * (length - position) / length;
            let local_vdd = current_a * options.vdd_ohm_per_site * factor;
            let local_gnd = current_a * options.gnd_ohm_per_site * factor;
            vdd_drop += local_vdd;
            gnd_drop += local_gnd;
            row_cells.push(IrCell {
                cell: ci,
                name: nl.cell(ci).name.clone(),
                row,
                site_start: start,
                site_end: end,
                current_ma: current_a * 1000.0,
                local_drop_v: local_vdd + local_gnd,
            });
        }

        let total_drop = vdd_drop + gnd_drop;
        // Each end carries at most approximately half of the rail current for
        // uniformly distributed load. Use the full current as a conservative
        // EM bound, so thin/degraded rails are not missed.
        let em_current = row_current;
        let em_margin = if options.max_rail_current_a == 0.0 {
            0.0
        } else {
            options.max_rail_current_a - em_current
        };
        if total_drop > worst_drop {
            worst_drop = total_drop;
            worst_row = Some(row);
        }
        row_cells.sort_by_key(|cell| cell.cell);
        cells.extend(row_cells);
        rows.push(IrRow {
            row,
            total_current_a: row_current,
            vdd_worst_drop_v: vdd_drop,
            gnd_worst_drop_v: gnd_drop,
            total_drop_v: total_drop,
            em_margin,
        });
    }

    let mut findings = Vec::new();
    if worst_drop > options.max_total_drop_v {
        findings.push(IrFinding {
            code: "IR-DROP".into(),
            severity: IrSeverity::Error,
            row: worst_row,
            message: format!(
                "worst IR drop {:.6} V exceeds {:.6} V",
                worst_drop, options.max_total_drop_v
            ),
        });
    }
    for row in &rows {
        if row.total_current_a > options.max_rail_current_a {
            findings.push(IrFinding {
                code: "EM-CURRENT".into(),
                severity: IrSeverity::Error,
                row: Some(row.row),
                message: format!(
                    "row {} current {:.6} A exceeds EM limit {:.6} A",
                    row.row, row.total_current_a, options.max_rail_current_a
                ),
            });
        } else if row.em_margin < options.max_rail_current_a * 0.1 {
            findings.push(IrFinding {
                code: "EM-MARGIN".into(),
                severity: IrSeverity::Warning,
                row: Some(row.row),
                message: format!(
                    "row {} has less than 10% EM margin ({:.6} A remaining)",
                    row.row, row.em_margin
                ),
            });
        }
    }

    Ok(IrResult {
        voltage_v: options.voltage_v,
        total_power_uw,
        total_current_a,
        worst_drop_v: worst_drop,
        worst_row,
        rows,
        cells,
        findings,
    })
}

fn validate_power_mesh_options(options: &PowerMeshOptions) -> Result<(), String> {
    if options.rows == 0 || options.columns == 0 {
        return Err("power mesh requires positive row and column counts".into());
    }
    let fields = [
        ("voltage", options.voltage_v),
        (
            "VDD horizontal resistance",
            options.vdd_horizontal_ohm_per_segment,
        ),
        (
            "VDD vertical resistance",
            options.vdd_vertical_ohm_per_segment,
        ),
        (
            "GND horizontal resistance",
            options.gnd_horizontal_ohm_per_segment,
        ),
        (
            "GND vertical resistance",
            options.gnd_vertical_ohm_per_segment,
        ),
        ("maximum drop", options.max_total_drop_v),
        ("maximum node current", options.max_node_current_a),
        ("maximum segment current", options.max_segment_current_a),
        ("tolerance", options.tolerance_v),
    ];
    for (name, value) in fields {
        if !value.is_finite() || value <= 0.0 {
            return Err(format!("invalid power mesh {} {}", name, value));
        }
    }
    if options.max_iterations == 0 {
        return Err("power mesh requires a positive iteration limit".into());
    }
    for (index, tap) in options.taps.iter().enumerate() {
        if tap.row >= options.rows || tap.column >= options.columns {
            return Err(format!(
                "power mesh tap {} location ({},{}) is outside {}x{} mesh",
                index, tap.row, tap.column, options.rows, options.columns
            ));
        }
        if !tap.vdd && !tap.gnd {
            return Err(format!("power mesh tap {} must enable VDD or GND", index));
        }
    }
    if !options.use_boundary_taps {
        if !options.taps.iter().any(|tap| tap.vdd) {
            return Err("power mesh requires at least one VDD tap".into());
        }
        if !options.taps.iter().any(|tap| tap.gnd) {
            return Err("power mesh requires at least one GND tap".into());
        }
    }
    Ok(())
}

type PowerMeshSolve = (Vec<Vec<f64>>, usize, f64);

#[allow(clippy::too_many_arguments)]
fn solve_power_mesh_network(
    rows: usize,
    columns: usize,
    injected_current: &[Vec<f64>],
    fixed_voltage_v: f64,
    horizontal_ohm: f64,
    vertical_ohm: f64,
    current_sign: f64,
    taps: &[Vec<bool>],
    max_iterations: usize,
    tolerance_v: f64,
) -> Result<PowerMeshSolve, String> {
    let mut voltage = vec![vec![fixed_voltage_v; columns]; rows];
    let conductance_horizontal = 1.0 / horizontal_ohm;
    let conductance_vertical = 1.0 / vertical_ohm;
    let mut iterations = 0;
    let mut final_delta = f64::INFINITY;

    while iterations < max_iterations {
        let mut max_delta = 0.0_f64;
        for row in 0..rows {
            for column in 0..columns {
                if taps[row][column] {
                    voltage[row][column] = fixed_voltage_v;
                    continue;
                }

                let mut conductance_sum = 0.0;
                let mut neighbor_voltage_sum = 0.0;
                if row > 0 {
                    conductance_sum += conductance_vertical;
                    neighbor_voltage_sum += conductance_vertical * voltage[row - 1][column];
                }
                if row + 1 < rows {
                    conductance_sum += conductance_vertical;
                    neighbor_voltage_sum += conductance_vertical * voltage[row + 1][column];
                }
                if column > 0 {
                    conductance_sum += conductance_horizontal;
                    neighbor_voltage_sum += conductance_horizontal * voltage[row][column - 1];
                }
                if column + 1 < columns {
                    conductance_sum += conductance_horizontal;
                    neighbor_voltage_sum += conductance_horizontal * voltage[row][column + 1];
                }
                let updated = (neighbor_voltage_sum + current_sign * injected_current[row][column])
                    / conductance_sum;
                let delta = (updated - voltage[row][column]).abs();
                max_delta = max_delta.max(delta);
                voltage[row][column] = updated;
            }
        }
        iterations += 1;
        final_delta = max_delta;
        if max_delta <= tolerance_v {
            return Ok((voltage, iterations, final_delta));
        }
    }
    Err(format!(
        "power mesh solver did not converge: final delta {:.6e} V after {} iterations",
        final_delta, iterations
    ))
}

fn power_mesh_tap_mask(options: &PowerMeshOptions, select_vdd: bool) -> Vec<Vec<bool>> {
    let mut taps = vec![vec![false; options.columns]; options.rows];
    if options.use_boundary_taps {
        for (row, tap_row) in taps.iter_mut().enumerate() {
            for (column, tap) in tap_row.iter_mut().enumerate() {
                if row == 0
                    || row + 1 == options.rows
                    || column == 0
                    || column + 1 == options.columns
                {
                    *tap = true;
                }
            }
        }
    }
    for tap in &options.taps {
        if select_vdd {
            taps[tap.row][tap.column] |= tap.vdd;
        } else {
            taps[tap.row][tap.column] |= tap.gnd;
        }
    }
    taps
}

/// Solve a two-network VDD/GND power mesh with configurable supply taps.
///
/// Cell currents are placed on the nearest mesh node. The VDD and GND networks
/// are solved independently using a finite-difference Gauss-Seidel nodal
/// equation. Total local drop is the VDD voltage loss plus local ground bounce.
pub fn analyze_mesh(
    nl: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    options: &PowerMeshOptions,
) -> Result<PowerMeshResult, String> {
    if placement.cell_pos.len() != nl.cell_count() || placement.cell_width.len() != nl.cell_count()
    {
        return Err("power mesh placement/netlist cell count mismatch".into());
    }
    if power.cells.len() != nl.cell_count() {
        return Err("power mesh power/netlist cell count mismatch".into());
    }
    validate_power_mesh_options(options)?;

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
    analyze_mesh_currents(nl, placement, &cell_current, total_power_uw, options)
}

/// Solve a mesh from explicit per-cell currents.
///
/// This entry point lets UPF-aware callers account for domain-specific supply
/// voltages, switch state, and mode-dependent activity before injection.
#[allow(clippy::needless_range_loop)]
pub fn analyze_mesh_currents(
    nl: &Netlist,
    placement: &Placement,
    cell_current_a: &[f64],
    total_power_uw: f64,
    options: &PowerMeshOptions,
) -> Result<PowerMeshResult, String> {
    if placement.cell_pos.len() != nl.cell_count() || placement.cell_width.len() != nl.cell_count()
    {
        return Err("power mesh placement/netlist cell count mismatch".into());
    }
    if cell_current_a.len() != nl.cell_count() {
        return Err("power mesh current/netlist cell count mismatch".into());
    }
    validate_power_mesh_options(options)?;

    let mut injected_current = vec![vec![0.0_f64; options.columns]; options.rows];
    let mut cells = Vec::with_capacity(nl.cell_count());
    for (cell_index, current_a) in cell_current_a.iter().enumerate() {
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
        let mesh_row = ((physical_row * options.rows as f64) / placement.rows.max(1) as f64)
            .floor()
            .clamp(0.0, (options.rows - 1) as f64) as usize;
        let center_site =
            position.site as f64 + placement.cell_width[cell_index].max(1) as f64 / 2.0;
        let normalized_column = (center_site / SITES_PER_ROW as f64).clamp(0.0, 0.999999);
        let mesh_column = (normalized_column * options.columns as f64)
            .floor()
            .clamp(0.0, (options.columns - 1) as f64) as usize;

        let current_a = *current_a;
        if !current_a.is_finite() || current_a < 0.0 {
            return Err(format!(
                "cell '{}' has invalid power-mesh current {}",
                nl.cell(cell_index).name,
                current_a
            ));
        }
        injected_current[mesh_row][mesh_column] += current_a;
        cells.push(PowerMeshCell {
            cell: cell_index,
            name: nl.cell(cell_index).name.clone(),
            mesh_row,
            mesh_column,
            current_a,
            local_total_drop_v: 0.0,
        });
    }

    let vdd_taps = power_mesh_tap_mask(options, true);
    let gnd_taps = power_mesh_tap_mask(options, false);
    let (vdd, vdd_iterations, vdd_delta) = solve_power_mesh_network(
        options.rows,
        options.columns,
        &injected_current,
        options.voltage_v,
        options.vdd_horizontal_ohm_per_segment,
        options.vdd_vertical_ohm_per_segment,
        -1.0,
        &vdd_taps,
        options.max_iterations,
        options.tolerance_v,
    )?;
    let (gnd, gnd_iterations, gnd_delta) = solve_power_mesh_network(
        options.rows,
        options.columns,
        &injected_current,
        0.0,
        options.gnd_horizontal_ohm_per_segment,
        options.gnd_vertical_ohm_per_segment,
        1.0,
        &gnd_taps,
        options.max_iterations,
        options.tolerance_v,
    )?;

    let mut nodes = Vec::with_capacity(options.rows * options.columns);
    let mut worst_drop = 0.0_f64;
    let mut worst_node = None;
    for row in 0..options.rows {
        for column in 0..options.columns {
            let vdd_drop = (options.voltage_v - vdd[row][column]).max(0.0);
            let gnd_drop = gnd[row][column].max(0.0);
            let total_drop = vdd_drop + gnd_drop;
            if total_drop > worst_drop {
                worst_drop = total_drop;
                worst_node = Some((row, column));
            }
            nodes.push(PowerMeshNode {
                row,
                column,
                vdd_v: vdd[row][column],
                gnd_v: gnd[row][column],
                vdd_drop_v: vdd_drop,
                gnd_drop_v: gnd_drop,
                total_drop_v: total_drop,
                injected_current_a: injected_current[row][column],
            });
        }
    }
    for cell in &mut cells {
        let node = &nodes[cell.mesh_row * options.columns + cell.mesh_column];
        cell.local_total_drop_v = node.total_drop_v;
    }

    let mut segments = Vec::new();
    for (network, voltage, horizontal_ohm, vertical_ohm) in [
        (
            PowerMeshNetwork::Vdd,
            &vdd,
            options.vdd_horizontal_ohm_per_segment,
            options.vdd_vertical_ohm_per_segment,
        ),
        (
            PowerMeshNetwork::Gnd,
            &gnd,
            options.gnd_horizontal_ohm_per_segment,
            options.gnd_vertical_ohm_per_segment,
        ),
    ] {
        for row in 0..options.rows {
            for column in 0..options.columns.saturating_sub(1) {
                let voltage_drop = voltage[row][column] - voltage[row][column + 1];
                segments.push(PowerMeshSegment {
                    network,
                    orientation: PowerMeshSegmentOrientation::Horizontal,
                    from_row: row,
                    from_column: column,
                    to_row: row,
                    to_column: column + 1,
                    resistance_ohm: horizontal_ohm,
                    voltage_drop_v: voltage_drop,
                    current_a: (voltage_drop / horizontal_ohm).abs(),
                });
            }
        }
        for row in 0..options.rows.saturating_sub(1) {
            for column in 0..options.columns {
                let voltage_drop = voltage[row][column] - voltage[row + 1][column];
                segments.push(PowerMeshSegment {
                    network,
                    orientation: PowerMeshSegmentOrientation::Vertical,
                    from_row: row,
                    from_column: column,
                    to_row: row + 1,
                    to_column: column,
                    resistance_ohm: vertical_ohm,
                    voltage_drop_v: voltage_drop,
                    current_a: (voltage_drop / vertical_ohm).abs(),
                });
            }
        }
    }

    let total_current_a: f64 = cell_current_a.iter().sum();
    let mut findings = Vec::new();
    let worst_segment_current_a = segments
        .iter()
        .map(|segment| segment.current_a)
        .fold(0.0, f64::max);
    if worst_drop > options.max_total_drop_v {
        findings.push(IrFinding {
            code: "MESH-IR-DROP".into(),
            severity: IrSeverity::Error,
            row: worst_node.map(|(row, _)| row),
            message: format!(
                "worst power-mesh drop {:.6} V at {:?} exceeds {:.6} V",
                worst_drop, worst_node, options.max_total_drop_v
            ),
        });
    }
    for node in &nodes {
        if node.injected_current_a > options.max_node_current_a {
            findings.push(IrFinding {
                code: "MESH-NODE-CURRENT".into(),
                severity: IrSeverity::Error,
                row: Some(node.row),
                message: format!(
                    "mesh node ({},{}) current {:.6} A exceeds {:.6} A",
                    node.row, node.column, node.injected_current_a, options.max_node_current_a
                ),
            });
        }
    }
    for segment in &segments {
        if segment.current_a > options.max_segment_current_a {
            findings.push(IrFinding {
                code: "MESH-SEGMENT-CURRENT".into(),
                severity: IrSeverity::Error,
                row: Some(segment.from_row),
                message: format!(
                    "{:?} mesh segment {:?} ({},{})-({},{}) current {:.6} A exceeds {:.6} A",
                    segment.network,
                    segment.orientation,
                    segment.from_row,
                    segment.from_column,
                    segment.to_row,
                    segment.to_column,
                    segment.current_a,
                    options.max_segment_current_a
                ),
            });
        }
    }

    Ok(PowerMeshResult {
        voltage_v: options.voltage_v,
        total_power_uw,
        total_current_a,
        worst_drop_v: worst_drop,
        worst_node,
        worst_segment_current_a,
        nodes,
        cells,
        segments,
        iterations: vdd_iterations.max(gnd_iterations),
        final_delta_v: vdd_delta.max(gnd_delta),
        findings,
    })
}
