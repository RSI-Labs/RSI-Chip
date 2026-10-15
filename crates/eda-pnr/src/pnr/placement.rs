use super::types::*;
use eda_core::netlist::*;

/// Width of a cell in placement sites.
pub(super) fn width_of(nl: &Netlist, ci: usize) -> usize {
    let cell = nl.cell(ci);
    match &cell.kind {
        CellKind::Lib(id) => nl.library().map(|l| l.get(*id).width_sites).unwrap_or(2),
        CellKind::Const(_) => 1,
        _ => 2,
    }
}

/// Deterministic row-packing placement.
pub fn place(nl: &Netlist) -> Result<Placement, String> {
    place_with_blockages(nl, &[])
}

/// Deterministic row-packing placement around hard placement obstructions.
///
/// Cells are considered in topological order and placed in the earliest legal
/// contiguous free sites. Additional rows are added only when no existing row
/// can contain a cell.
pub fn place_with_blockages(
    nl: &Netlist,
    blockages: &[PlacementBlockage],
) -> Result<Placement, String> {
    let order = nl
        .topo_cells()
        .ok_or_else(|| "cannot place: combinational loop".to_string())?;
    let widths: Vec<usize> = (0..nl.cell_count()).map(|ci| width_of(nl, ci)).collect();
    let mut occupied: Vec<Vec<bool>> = Vec::new();
    for blockage in blockages {
        blockage.validate()?;
        if occupied.len() <= blockage.row {
            occupied.resize(blockage.row + 1, vec![false; SITES_PER_ROW]);
        }
        for (site, occupied_site) in occupied[blockage.row]
            .iter_mut()
            .enumerate()
            .take(blockage.end_site + 1)
            .skip(blockage.start_site)
        {
            if *occupied_site {
                return Err(format!(
                    "placement blockages overlap at row {} site {}",
                    blockage.row, site
                ));
            }
            *occupied_site = true;
        }
    }

    let mut cell_pos = vec![Pos { row: 0, site: 0 }; nl.cell_count()];
    let mut rows = occupied.len().max(1);
    for ci in order {
        let w = widths[ci].max(1);
        if w > SITES_PER_ROW {
            return Err(format!(
                "cell '{}' width {} exceeds {} sites per row",
                nl.cell(ci).name,
                w,
                SITES_PER_ROW
            ));
        }
        let mut allocated = None;
        'rows: for row in 0..=occupied.len() {
            if row == occupied.len() {
                occupied.push(vec![false; SITES_PER_ROW]);
                rows = rows.max(row + 1);
            }
            for site in 0..=(SITES_PER_ROW - w) {
                if (site..site + w).all(|slot| !occupied[row][slot]) {
                    allocated = Some((row, site));
                    break 'rows;
                }
            }
        }
        let (row, site) = allocated.expect("a fresh row always has capacity");
        occupied[row][site..site + w].fill(true);
        cell_pos[ci] = Pos { row, site };
    }
    Ok(Placement {
        rows,
        cell_pos,
        cell_width: widths,
        blockages: blockages.to_vec(),
    })
}

/// Weighted HPWL using timing/criticality weights supplied by the caller.
pub fn weighted_hpwl(
    nl: &Netlist,
    p: &Placement,
    options: &TimingDrivenPlacementOptions,
) -> Result<f64, String> {
    options.validate()?;
    let mut total = 0.0;
    for ni in 0..nl.net_count() {
        let net = nl.net(ni);
        let mut pins: Vec<(f64, f64)> = Vec::new();
        let push_cell = |ci: usize, pins: &mut Vec<(f64, f64)>| {
            if let Some(pos) = p.cell_pos.get(ci) {
                pins.push((center_x(p, ci), pos.row as f64));
            }
        };
        for &driver in &net.drivers {
            push_cell(driver, &mut pins);
        }
        for &sink in &net.sinks {
            push_cell(sink, &mut pins);
        }
        let is_port = nl.ports.iter().any(|port| port.bits.contains(&ni));
        if is_port {
            if let Some(&(x, y)) = pins.first() {
                let _ = y;
                pins.push((x, 0.0));
            }
        }
        if pins.len() >= 2 {
            let xs = pins.iter().map(|pin| pin.0).collect::<Vec<_>>();
            let ys = pins.iter().map(|pin| pin.1).collect::<Vec<_>>();
            let width = xs.iter().cloned().fold(f64::MIN, f64::max)
                - xs.iter().cloned().fold(f64::MAX, f64::min);
            let height = ys.iter().cloned().fold(f64::MIN, f64::max)
                - ys.iter().cloned().fold(f64::MAX, f64::min);
            total += (width.max(0.0) + height.max(0.0)) * options.weight(ni);
        }
    }
    if !total.is_finite() {
        return Err("weighted HPWL is not finite".into());
    }
    Ok(total)
}

/// Optimize a legal starting placement by weighted-HPWL local search.
///
/// The optimizer tries legal single-cell moves and pairwise position swaps.
/// A trial is accepted only when it reduces the caller-supplied timing
/// objective, so critical nets can dominate the placement decision.
pub fn place_timing_driven(
    nl: &Netlist,
    options: &TimingDrivenPlacementOptions,
) -> Result<TimingDrivenPlacementResult, String> {
    let initial = place(nl)?;
    optimize_placement_timing_driven(nl, initial, options)
}

pub fn optimize_placement_timing_driven(
    nl: &Netlist,
    initial: Placement,
    options: &TimingDrivenPlacementOptions,
) -> Result<TimingDrivenPlacementResult, String> {
    options.validate()?;
    for net in options.net_weights.keys() {
        if *net >= nl.net_count() {
            return Err(format!(
                "timing-driven weight references net {} but netlist has {} nets",
                net,
                nl.net_count()
            ));
        }
    }
    check_legal(nl, &initial)?;

    let mut current = initial.clone();
    let initial_hpwl = hpwl(nl, &initial);
    let initial_weighted = weighted_hpwl(nl, &initial, options)?;
    let mut best_cost = initial_weighted;
    let mut improving_moves = 0usize;
    let mut passes = 0usize;

    for pass in 0..options.max_passes {
        let mut improved = false;

        // Move one cell at a time. check_legal rejects overlaps, while the
        // surrounding search retains the previous position on rejection.
        for ci in 0..nl.cell_count() {
            let original = current.cell_pos[ci];
            let mut best_pos = original;
            let width = current.cell_width[ci].max(1);
            for row in 0..current.rows {
                for site in 0..=(SITES_PER_ROW.saturating_sub(width)) {
                    let candidate_pos = Pos { row, site };
                    if candidate_pos == original {
                        continue;
                    }
                    current.cell_pos[ci] = candidate_pos;
                    let cost = match (
                        weighted_hpwl(nl, &current, options),
                        check_legal(nl, &current),
                    ) {
                        (Ok(cost), Ok(())) => cost,
                        _ => {
                            current.cell_pos[ci] = original;
                            continue;
                        }
                    };
                    if cost < best_cost - 1e-12 {
                        best_cost = cost;
                        best_pos = candidate_pos;
                        improving_moves += 1;
                        improved = true;
                    } else {
                        current.cell_pos[ci] = original;
                    }
                }
            }
            current.cell_pos[ci] = best_pos;
        }

        // Swap cell positions. Equal-width swaps always preserve legality;
        // unequal-width swaps are accepted only when the resulting rows do
        // not overlap.
        for left in 0..nl.cell_count() {
            for right in (left + 1)..nl.cell_count() {
                let original_left = current.cell_pos[left];
                let original_right = current.cell_pos[right];
                if original_left == original_right {
                    continue;
                }
                current.cell_pos[left] = original_right;
                current.cell_pos[right] = original_left;
                let cost = match (
                    weighted_hpwl(nl, &current, options),
                    check_legal(nl, &current),
                ) {
                    (Ok(cost), Ok(())) => cost,
                    _ => {
                        current.cell_pos[left] = original_left;
                        current.cell_pos[right] = original_right;
                        continue;
                    }
                };
                if cost < best_cost - 1e-12 {
                    best_cost = cost;
                    improving_moves += 1;
                    improved = true;
                } else {
                    current.cell_pos[left] = original_left;
                    current.cell_pos[right] = original_right;
                }
            }
        }

        passes = pass + 1;
        if !improved {
            break;
        }
    }

    let final_hpwl = hpwl(nl, &current);
    let final_weighted = weighted_hpwl(nl, &current, options)?;
    if final_weighted > initial_weighted + 1e-12 {
        return Err("timing-driven optimizer worsened its objective".into());
    }
    check_legal(nl, &current)?;
    Ok(TimingDrivenPlacementResult {
        placement: current,
        initial_hpwl,
        final_hpwl,
        initial_weighted_hpwl: initial_weighted,
        final_weighted_hpwl: final_weighted,
        passes,
        improving_moves,
    })
}

pub fn timing_driven_placement_report(result: &TimingDrivenPlacementResult) -> String {
    format!(
        "Timing-driven placement report: {} move(s), {} pass(es)\n  HPWL           : {:.3} -> {:.3} sites\n  weighted HPWL  : {:.3} -> {:.3}\n",
        result.improving_moves,
        result.passes,
        result.initial_hpwl,
        result.final_hpwl,
        result.initial_weighted_hpwl,
        result.final_weighted_hpwl
    )
}

/// Extend a legal prefix placement to cover newly appended cells.
///
/// The input placement describes the first `original.cell_pos.len()` cells of
/// `nl`. New cells are placed in the earliest legal contiguous sites, adding
/// rows when necessary. This is useful after CTS, ECO, or other netlist
/// transformations append cells.
pub fn extend_placement(nl: &Netlist, original: &Placement) -> Result<Placement, String> {
    if original.cell_pos.len() != original.cell_width.len() {
        return Err("prefix placement position/width vectors differ".into());
    }
    if original.cell_pos.len() > nl.cell_count() {
        return Err(format!(
            "prefix placement has {} cells but netlist has {}",
            original.cell_pos.len(),
            nl.cell_count()
        ));
    }

    let mut occupied: Vec<Vec<bool>> = vec![vec![false; SITES_PER_ROW]; original.rows.max(1)];
    for blockage in &original.blockages {
        blockage.validate()?;
        if blockage.row >= occupied.len() {
            return Err(format!(
                "placement blockage '{}' row {} beyond {}",
                blockage.name,
                blockage.row,
                occupied.len()
            ));
        }
        for (site, occupied_site) in occupied[blockage.row]
            .iter_mut()
            .enumerate()
            .take(blockage.end_site + 1)
            .skip(blockage.start_site)
        {
            if *occupied_site {
                return Err(format!(
                    "placement blockage overlaps a prefix cell at row {} site {}",
                    blockage.row, site
                ));
            }
            *occupied_site = true;
        }
    }
    for (ci, pos) in original.cell_pos.iter().enumerate() {
        let width = original.cell_width[ci].max(1);
        if pos.row >= occupied.len() || pos.site + width > SITES_PER_ROW {
            return Err(format!("prefix cell {} has illegal placement", ci));
        }
        for (site, occupied_site) in occupied[pos.row]
            .iter_mut()
            .enumerate()
            .take(pos.site + width)
            .skip(pos.site)
        {
            if *occupied_site {
                return Err(format!(
                    "prefix cells overlap at row {} site {}",
                    pos.row, site
                ));
            }
            *occupied_site = true;
        }
    }

    let mut placement = original.clone();
    for ci in original.cell_pos.len()..nl.cell_count() {
        let width = width_of(nl, ci).max(1);
        if width > SITES_PER_ROW {
            return Err(format!(
                "cell '{}' width {} exceeds {} sites per row",
                nl.cell(ci).name,
                width,
                SITES_PER_ROW
            ));
        }
        let mut allocated = None;
        'rows: for row in 0..=occupied.len() {
            if row == occupied.len() {
                occupied.push(vec![false; SITES_PER_ROW]);
            }
            for site in 0..=(SITES_PER_ROW - width) {
                if (site..site + width).all(|slot| !occupied[row][slot]) {
                    allocated = Some((row, site));
                    break 'rows;
                }
            }
        }
        let (row, site) = allocated.expect("a fresh row always has space");
        occupied[row][site..site + width].fill(true);
        placement.cell_pos.push(Pos { row, site });
        placement.cell_width.push(width);
        placement.rows = placement.rows.max(row + 1);
    }
    check_legal(nl, &placement)?;
    Ok(placement)
}

/// Cell center x coordinate (site units).
pub(super) fn center_x(p: &Placement, ci: usize) -> f64 {
    let pos = p.cell_pos[ci];
    pos.site as f64 + p.cell_width[ci] as f64 / 2.0
}

/// Verify placement legality: rows in range, sites in range, no overlaps.
pub fn check_legal(nl: &Netlist, p: &Placement) -> Result<(), String> {
    if p.cell_pos.len() != nl.cell_count() {
        return Err(format!(
            "placed {} cells but netlist has {}",
            p.cell_pos.len(),
            nl.cell_count()
        ));
    }
    let mut occupied: Vec<Vec<bool>> = vec![vec![false; SITES_PER_ROW]; p.rows.max(1)];
    for blockage in &p.blockages {
        blockage.validate()?;
        if blockage.row >= occupied.len() {
            return Err(format!(
                "placement blockage '{}' row {} beyond {}",
                blockage.name,
                blockage.row,
                occupied.len()
            ));
        }
        for (site, occupied_site) in occupied[blockage.row]
            .iter_mut()
            .enumerate()
            .take(blockage.end_site + 1)
            .skip(blockage.start_site)
        {
            if *occupied_site {
                return Err(format!(
                    "cell or placement blockage overlaps at row {} site {}",
                    blockage.row, site
                ));
            }
            *occupied_site = true;
        }
    }
    for (ci, pos) in p.cell_pos.iter().enumerate() {
        let w = p.cell_width[ci].max(1);
        if pos.row >= occupied.len() {
            return Err(format!(
                "cell {} in row {} beyond {}",
                ci,
                pos.row,
                occupied.len()
            ));
        }
        if pos.site + w > SITES_PER_ROW {
            return Err(format!(
                "cell {} overflows row (site {} + {})",
                ci, pos.site, w
            ));
        }
        for (site, occupied_site) in occupied[pos.row]
            .iter_mut()
            .enumerate()
            .take(pos.site + w)
            .skip(pos.site)
        {
            if *occupied_site {
                return Err(format!("cells overlap at row {} site {}", pos.row, site));
            }
            *occupied_site = true;
        }
    }
    Ok(())
}

/// Half-perimeter wire length over all multi-pin nets (site units).
pub fn hpwl(nl: &Netlist, p: &Placement) -> f64 {
    let mut total = 0.0;
    for (ni, net) in nl.nets().iter().enumerate() {
        let mut pins: Vec<(f64, f64)> = Vec::new();
        let push_cell = |ci: usize, pins: &mut Vec<(f64, f64)>| {
            if let Some(pos) = p.cell_pos.get(ci) {
                pins.push((center_x(p, ci), pos.row as f64));
            }
        };
        for &d in &net.drivers {
            push_cell(d, &mut pins);
        }
        for &s in &net.sinks {
            push_cell(s, &mut pins);
        }
        // Primary input/output pins attach to the die edge at their net's
        // first consumer column (approximation).
        let is_port = nl.ports.iter().any(|pt| pt.bits.contains(&ni));
        if is_port {
            if let Some(&(x, y)) = pins.first() {
                let _ = y;
                pins.push((x, 0.0));
            }
        }
        if pins.len() >= 2 {
            let xs: Vec<f64> = pins.iter().map(|q| q.0).collect();
            let ys: Vec<f64> = pins.iter().map(|q| q.1).collect();
            let hx = xs.iter().cloned().fold(f64::MIN, f64::max)
                - xs.iter().cloned().fold(f64::MAX, f64::min);
            let hy = ys.iter().cloned().fold(f64::MIN, f64::max)
                - ys.iter().cloned().fold(f64::MAX, f64::min);
            total += hx.max(0.0) + hy.max(0.0);
        }
    }
    total
}

pub(super) fn integer_pin_x(p: &Placement, ci: usize) -> usize {
    center_x(p, ci)
        .round()
        .clamp(0.0, SITES_PER_ROW as f64)
        .min(SITES_PER_ROW as f64) as usize
}

pub(super) fn net_pins(nl: &Netlist, p: &Placement, ni: usize) -> Vec<(usize, usize)> {
    let net = nl.net(ni);
    let mut pins = Vec::new();
    let mut push = |ci: usize| {
        if let Some(pos) = p.cell_pos.get(ci) {
            let pin = (integer_pin_x(p, ci), pos.row);
            if !pins.contains(&pin) {
                pins.push(pin);
            }
        }
    };
    for &driver in &net.drivers {
        push(driver);
    }
    for &sink in &net.sinks {
        push(sink);
    }
    if let Some(port) = nl.ports.iter().find(|port| port.bits.contains(&ni)) {
        if let Some(&(x, _)) = pins.first() {
            let edge_row = if port.dir == eda_core::netlist::Dir::In {
                0
            } else {
                p.rows
            };
            let edge_pin = (x, edge_row);
            if !pins.contains(&edge_pin) {
                pins.push(edge_pin);
            }
        }
    }
    pins
}
