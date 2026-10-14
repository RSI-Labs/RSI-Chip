//! # eda-pnr — placement and routing estimates
//!
//! Row-based placement in standard-cell site grid (1 row = 1 site height):
//! cells are packed left-to-right in topological order, wrapping to new rows.
//! Wire estimates use HPWL (half-perimeter wire length) over net bounding
//! boxes of cell centers.

use eda_core::cancellation::FlowCancellation;
use eda_core::netlist::{CellKind, Netlist};
use std::collections::HashMap;

pub mod any_angle;
pub mod any_angle_route;
pub mod layout;
pub mod layout_browser;
pub mod physical;
pub use layout_browser::{LayoutBrowserServer, LayoutBrowserService};
pub use physical::{
    parse_def, physical_from_def, write_def, write_lef, Def, DefComponent, DefNet,
    DefPlacementBlockage, DefRoute, DefRouteBlockage, DefVia,
};

/// Sites per placement row.
pub const SITES_PER_ROW: usize = 32;
/// Global-routing tracks available in each horizontal/vertical channel.
pub const TRACKS_PER_CHANNEL: usize = 8;
/// Conservative antenna threshold in routing-length units per connected pin.
pub const MAX_ANTENNA_RATIO: f64 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pos {
    pub row: usize,
    pub site: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub rows: usize,
    /// Indexed by cell id; every cell has one slot.
    pub cell_pos: Vec<Pos>,
    /// Cell widths in sites (mirrors library widths).
    pub cell_width: Vec<usize>,
    /// Hard placement obstructions retained for legalization and DRC.
    pub blockages: Vec<PlacementBlockage>,
}

/// A rectangular hard placement obstruction in one row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementBlockage {
    pub name: String,
    pub row: usize,
    pub start_site: usize,
    pub end_site: usize,
}

impl PlacementBlockage {
    pub fn new(
        name: impl Into<String>,
        row: usize,
        start_site: usize,
        end_site: usize,
    ) -> Result<Self, String> {
        let out = Self {
            name: name.into(),
            row,
            start_site,
            end_site,
        };
        out.validate()?;
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("placement blockage name cannot be empty".into());
        }
        if self.start_site > self.end_site || self.end_site >= SITES_PER_ROW {
            return Err(format!(
                "placement blockage '{}' spans sites {}..{} outside a {}-site row",
                self.name, self.start_site, self.end_site, SITES_PER_ROW
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub net: usize,
    pub orientation: Orientation,
    /// Horizontal channel row for horizontal segments; x column for vertical
    /// segments.
    pub channel: usize,
    pub start: usize,
    pub end: usize,
    pub track: usize,
}

impl Segment {
    pub fn length(&self) -> usize {
        self.end.saturating_sub(self.start)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Routing {
    pub segments: Vec<Segment>,
    pub vias: usize,
    /// Explicit layer-change locations. Each via connects a horizontal and a
    /// vertical segment of the same net at its grid coordinate.
    pub via_locations: Vec<Via>,
    pub tracks_per_channel: usize,
    /// Channels that exhausted their track capacity.
    pub congested_channels: Vec<(Orientation, usize)>,
    /// Hard routing blockages retained for DRC and physical audit.
    pub blocked_channels: Vec<RouteChannelBlockage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Via {
    pub net: usize,
    pub x: usize,
    pub y: usize,
}

/// A hard blockage covering one complete global-routing channel.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RouteChannelBlockage {
    pub name: String,
    pub orientation: Orientation,
    pub channel: usize,
}

impl RouteChannelBlockage {
    pub fn new(
        name: impl Into<String>,
        orientation: Orientation,
        channel: usize,
    ) -> Result<Self, String> {
        let out = Self {
            name: name.into(),
            orientation,
            channel,
        };
        out.validate()?;
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("routing blockage name cannot be empty".into());
        }
        Ok(())
    }
}

/// Global-routing capacity and congestion-policy options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteOptions {
    pub tracks_per_channel: usize,
    /// Keep partial routing and report congested channels instead of failing
    /// immediately. DRC subsequently reports the resulting opens.
    pub allow_congestion: bool,
    pub blocked_channels: Vec<RouteChannelBlockage>,
}

/// Connectivity weights for timing-driven placement.
///
/// Nets absent from `net_weights` receive `default_net_weight`. Higher
/// weights give a net more influence over the weighted-HPWL objective.
#[derive(Clone, Debug)]
pub struct TimingDrivenPlacementOptions {
    pub net_weights: HashMap<usize, f64>,
    pub default_net_weight: f64,
    pub max_passes: usize,
}

impl Default for TimingDrivenPlacementOptions {
    fn default() -> Self {
        Self {
            net_weights: HashMap::new(),
            default_net_weight: 1.0,
            max_passes: 4,
        }
    }
}

impl TimingDrivenPlacementOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_passes == 0 {
            return Err("timing-driven placement requires at least one pass".into());
        }
        if !self.default_net_weight.is_finite() || self.default_net_weight < 0.0 {
            return Err("default timing-driven net weight must be finite and non-negative".into());
        }
        for (net, weight) in &self.net_weights {
            if !weight.is_finite() || *weight < 0.0 {
                return Err(format!("invalid timing-driven weight for net {}", net));
            }
        }
        Ok(())
    }

    fn weight(&self, net: usize) -> f64 {
        self.net_weights
            .get(&net)
            .copied()
            .unwrap_or(self.default_net_weight)
    }
}

#[derive(Clone, Debug)]
pub struct TimingDrivenPlacementResult {
    pub placement: Placement,
    pub initial_hpwl: f64,
    pub final_hpwl: f64,
    pub initial_weighted_hpwl: f64,
    pub final_weighted_hpwl: f64,
    pub passes: usize,
    pub improving_moves: usize,
}

/// Timing/criticality weights for negotiated-congestion routing.
///
/// Connections are rerouted in descending weight order, so critical nets get
/// first use of scarce direct channels while low-priority nets detour.
#[derive(Clone, Debug)]
pub struct TimingDrivenRoutingOptions {
    pub net_weights: HashMap<usize, f64>,
    pub default_net_weight: f64,
}

impl Default for TimingDrivenRoutingOptions {
    fn default() -> Self {
        Self {
            net_weights: HashMap::new(),
            default_net_weight: 1.0,
        }
    }
}

impl TimingDrivenRoutingOptions {
    pub fn validate(&self, net_count: usize) -> Result<(), String> {
        if !self.default_net_weight.is_finite() || self.default_net_weight < 0.0 {
            return Err("default routing weight must be finite and non-negative".into());
        }
        for (net, weight) in &self.net_weights {
            if *net >= net_count {
                return Err(format!(
                    "routing weight references net {} but netlist has {} nets",
                    net, net_count
                ));
            }
            if !weight.is_finite() || *weight < 0.0 {
                return Err(format!("invalid routing weight for net {}", net));
            }
        }
        Ok(())
    }

    fn weight(&self, net: usize) -> f64 {
        self.net_weights
            .get(&net)
            .copied()
            .unwrap_or(self.default_net_weight)
    }
}

impl Default for RouteOptions {
    fn default() -> Self {
        Self {
            tracks_per_channel: TRACKS_PER_CHANNEL,
            allow_congestion: false,
            blocked_channels: Vec::new(),
        }
    }
}

impl RouteOptions {
    pub fn new(tracks_per_channel: usize) -> Result<Self, String> {
        if tracks_per_channel == 0 {
            return Err("routing requires at least one track per channel".into());
        }
        Ok(Self {
            tracks_per_channel,
            allow_congestion: false,
            blocked_channels: Vec::new(),
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.tracks_per_channel == 0 {
            return Err("routing requires at least one track per channel".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for blockage in &self.blocked_channels {
            blockage.validate()?;
            if !seen.insert((blockage.orientation, blockage.channel)) {
                return Err(format!(
                    "duplicate routing blockage for {:?} channel {}",
                    blockage.orientation, blockage.channel
                ));
            }
        }
        Ok(())
    }

    pub fn with_blocked_channel(
        mut self,
        name: impl Into<String>,
        orientation: Orientation,
        channel: usize,
    ) -> Result<Self, String> {
        let blockage = RouteChannelBlockage::new(name, orientation, channel)?;
        self.blocked_channels.push(blockage);
        self.validate()?;
        Ok(self)
    }

    fn blocked_set(&self) -> std::collections::BTreeSet<(Orientation, usize)> {
        self.blocked_channels
            .iter()
            .map(|blockage| (blockage.orientation, blockage.channel))
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DrcSeverity {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrcViolation {
    pub code: String,
    pub severity: DrcSeverity,
    pub message: String,
    pub cell: Option<usize>,
    pub net: Option<usize>,
    pub row: Option<usize>,
    pub site: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct DrcResult {
    pub violations: Vec<DrcViolation>,
}

impl DrcResult {
    pub fn has_errors(&self) -> bool {
        self.violations
            .iter()
            .any(|v| v.severity == DrcSeverity::Error)
    }

    pub fn has_warnings(&self) -> bool {
        self.violations
            .iter()
            .any(|v| v.severity == DrcSeverity::Warning)
    }

    pub fn error_count(&self) -> usize {
        self.violations
            .iter()
            .filter(|v| v.severity == DrcSeverity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.violations
            .iter()
            .filter(|v| v.severity == DrcSeverity::Warning)
            .count()
    }
}

impl Routing {
    pub fn wirelength(&self) -> usize {
        self.segments.iter().map(Segment::length).sum()
    }

    pub fn utilization(&self) -> f64 {
        let mut occupancy: HashMap<(Orientation, usize), Vec<(usize, i32)>> = HashMap::new();
        for seg in &self.segments {
            occupancy
                .entry((seg.orientation, seg.channel))
                .or_default()
                .extend([(seg.start, 1), (seg.end + 1, -1)]);
        }
        occupancy
            .values()
            .map(|events| {
                let mut sorted = events.clone();
                sorted.sort_unstable();
                let mut active = 0_i32;
                let mut peak = 0_i32;
                for (_, delta) in sorted {
                    active += delta;
                    peak = peak.max(active);
                }
                peak as f64
            })
            .fold(0.0_f64, f64::max)
            / self.tracks_per_channel as f64
    }
}

/// Width of a cell in placement sites.
fn width_of(nl: &Netlist, ci: usize) -> usize {
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
fn center_x(p: &Placement, ci: usize) -> f64 {
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

fn integer_pin_x(p: &Placement, ci: usize) -> usize {
    center_x(p, ci)
        .round()
        .clamp(0.0, SITES_PER_ROW as f64)
        .min(SITES_PER_ROW as f64) as usize
}

fn net_pins(nl: &Netlist, p: &Placement, ni: usize) -> Vec<(usize, usize)> {
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

#[derive(Default)]
struct TrackAllocator {
    used: TrackUsageMap,
    blocked: std::collections::BTreeSet<(Orientation, usize)>,
}

type TrackKey = (Orientation, usize, usize);
type TrackRange = (usize, usize, usize);
type TrackUsageMap = HashMap<TrackKey, Vec<TrackRange>>;
type RouteShape = (Orientation, usize, usize, usize);
type TrackAssignment = (Orientation, usize, usize, usize, usize);

impl TrackAllocator {
    fn with_blockages(blocked: std::collections::BTreeSet<(Orientation, usize)>) -> Self {
        Self {
            used: HashMap::new(),
            blocked,
        }
    }

    fn is_blocked(&self, orientation: Orientation, channel: usize) -> bool {
        self.blocked.contains(&(orientation, channel))
    }

    fn is_free(
        &self,
        orientation: Orientation,
        channel: usize,
        start: usize,
        end: usize,
        net: usize,
        track: usize,
    ) -> bool {
        if self.is_blocked(orientation, channel) {
            return false;
        }
        self.used
            .get(&(orientation, channel, track))
            .is_none_or(|ranges| {
                !ranges
                    .iter()
                    .any(|&(s, e, n)| n != net && start <= e && end >= s)
            })
    }

    fn allocate_shape(
        &mut self,
        shape: &[RouteShape],
        net: usize,
        capacity: usize,
    ) -> Option<Vec<TrackAssignment>> {
        let mut assignment = Vec::with_capacity(shape.len());
        for &(orientation, channel, start, end) in shape {
            let track = (0..capacity)
                .find(|&track| self.is_free(orientation, channel, start, end, net, track))?;
            assignment.push((orientation, channel, start, end, track));
        }
        for &(orientation, channel, start, end, track) in &assignment {
            self.used
                .entry((orientation, channel, track))
                .or_default()
                .push((start, end, net));
        }
        Some(assignment)
    }

    fn reserve(
        &mut self,
        orientation: Orientation,
        channel: usize,
        start: usize,
        end: usize,
        track: usize,
        net: usize,
    ) {
        self.used
            .entry((orientation, channel, track))
            .or_default()
            .push((start, end, net));
    }
}

/// Deterministic L-shaped global routing over the placement channel grid.
/// Each driver-to-sink connection uses one horizontal and one vertical
/// track segment; the greedy allocator avoids overlap in a channel/track.
pub fn route(nl: &Netlist, p: &Placement) -> Result<Routing, String> {
    route_with_options_cancellable(nl, p, &RouteOptions::default(), &FlowCancellation::new())
}

/// Capacity-aware deterministic global routing.
///
/// For each two-pin connection the router considers both L-shaped bend
/// choices before declaring a channel congested. This avoids many artificial
/// conflicts caused by always assigning the horizontal segment in the source
/// row and the vertical segment in the sink column.
pub fn route_with_options(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
) -> Result<Routing, String> {
    route_with_options_cancellable(nl, p, options, &FlowCancellation::new())
}

pub fn route_with_options_cancellable(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
    cancellation: &FlowCancellation,
) -> Result<Routing, String> {
    if cancellation.is_cancelled() {
        return Err("global routing cancelled".into());
    }
    check_legal(nl, p)?;
    options.validate()?;
    let mut allocator = TrackAllocator::with_blockages(options.blocked_set());
    let mut segments: Vec<Segment> = Vec::new();
    let mut congested = Vec::new();
    let mut vias = 0usize;
    let mut via_locations = Vec::new();

    for ni in 0..nl.net_count() {
        if cancellation.is_cancelled() {
            return Err("global routing cancelled".into());
        }
        let pins = net_pins(nl, p, ni);
        if pins.len() < 2 {
            continue;
        }
        let source = pins[0];
        for &sink in &pins[1..] {
            if cancellation.is_cancelled() {
                return Err("global routing cancelled".into());
            }
            let shapes = vec![
                vec![
                    (
                        Orientation::Horizontal,
                        source.1,
                        source.0.min(sink.0),
                        source.0.max(sink.0),
                    ),
                    (
                        Orientation::Vertical,
                        sink.0,
                        source.1.min(sink.1),
                        source.1.max(sink.1),
                    ),
                ],
                vec![
                    (
                        Orientation::Vertical,
                        source.0,
                        source.1.min(sink.1),
                        source.1.max(sink.1),
                    ),
                    (
                        Orientation::Horizontal,
                        sink.1,
                        sink.0.min(source.0),
                        sink.0.max(source.0),
                    ),
                ],
            ];
            let mut routed = false;
            for shape in shapes {
                // Zero-length components are not physical segments.
                let shape: Vec<_> = shape
                    .into_iter()
                    .filter(|&(_, _, start, end)| start != end)
                    .collect();
                if shape.is_empty() {
                    routed = true;
                    break;
                }
                if let Some(assignment) =
                    allocator.allocate_shape(&shape, ni, options.tracks_per_channel)
                {
                    for (orientation, channel, start, end, track) in assignment {
                        segments.push(Segment {
                            net: ni,
                            orientation,
                            channel,
                            start,
                            end,
                            track,
                        });
                    }
                    if shape.len() == 2 {
                        vias += 1;
                        if let Some(via) = shape_via(&segments, ni) {
                            via_locations.push(via);
                        }
                    }
                    routed = true;
                    break;
                }
            }
            if !routed {
                // Record every channel needed by the preferred L-shape. DRC
                // will identify the actual opened pin groups when partial
                // routing is requested.
                if source.0 != sink.0 {
                    congested.push((Orientation::Horizontal, source.1));
                    congested.push((Orientation::Horizontal, sink.1));
                }
                if source.1 != sink.1 {
                    congested.push((Orientation::Vertical, source.0));
                    congested.push((Orientation::Vertical, sink.0));
                }
            }
        }
    }

    congested.sort_unstable();
    congested.dedup();
    if !congested.is_empty() {
        if options.allow_congestion {
            return Ok(Routing {
                segments,
                vias,
                via_locations,
                tracks_per_channel: options.tracks_per_channel,
                congested_channels: congested,
                blocked_channels: options.blocked_channels.clone(),
            });
        }
        return Err(format!(
            "global routing congestion in {} channel(s): {:?}",
            congested.len(),
            congested
        ));
    }
    Ok(Routing {
        segments,
        vias,
        via_locations,
        tracks_per_channel: options.tracks_per_channel,
        congested_channels: congested,
        blocked_channels: options.blocked_channels.clone(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct MazeNode {
    x: usize,
    y: usize,
    last_orientation: Option<Orientation>,
    last_track: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
struct MazeEdge {
    to: (usize, usize),
    orientation: Orientation,
    channel: usize,
    start: usize,
    end: usize,
    track: usize,
}

#[derive(Clone, Debug)]
struct MazeSearchStep {
    node: MazeNode,
    previous: Option<usize>,
    edge: Option<MazeEdge>,
    cost: usize,
}

fn maze_path(
    allocator: &TrackAllocator,
    capacity: usize,
    source: (usize, usize),
    sink: (usize, usize),
    max_x: usize,
    max_y: usize,
    cancellation: &FlowCancellation,
) -> Result<Option<Vec<MazeEdge>>, String> {
    if source == sink {
        return Ok(Some(Vec::new()));
    }

    #[derive(Eq, PartialEq, Ord, PartialOrd, Hash)]
    struct QueueKey {
        priority: usize,
        step: usize,
    }
    // BinaryHeap is a max-heap; store keys as Reverse so lower f-cost wins.
    let mut queue: std::collections::BinaryHeap<std::cmp::Reverse<QueueKey>> =
        std::collections::BinaryHeap::new();
    let start = MazeNode {
        x: source.0,
        y: source.1,
        last_orientation: None,
        last_track: None,
    };
    let mut steps = vec![MazeSearchStep {
        node: start,
        previous: None,
        edge: None,
        cost: 0,
    }];
    let mut best = HashMap::from([(start, 0usize)]);
    let estimate = |node: MazeNode| -> usize { node.x.abs_diff(sink.0) + node.y.abs_diff(sink.1) };
    queue.push(std::cmp::Reverse(QueueKey {
        priority: estimate(start),
        step: 0,
    }));

    let mut target_step = None;
    let mut expansions = 0usize;
    while let Some(std::cmp::Reverse(item)) = queue.pop() {
        expansions += 1;
        if expansions.is_multiple_of(128) && cancellation.is_cancelled() {
            return Err("maze routing cancelled".into());
        }
        let current_index = item.step;
        let current = steps[current_index].clone();
        if current.cost > best.get(&current.node).copied().unwrap_or(usize::MAX) {
            continue;
        }
        if current.node.x == sink.0 && current.node.y == sink.1 {
            target_step = Some(current_index);
            break;
        }

        let mut neighbors = Vec::with_capacity(4);
        if current.node.x > 0 {
            neighbors.push((
                current.node.x - 1,
                current.node.y,
                Orientation::Horizontal,
                current.node.y,
            ));
        }
        if current.node.x < max_x {
            neighbors.push((
                current.node.x + 1,
                current.node.y,
                Orientation::Horizontal,
                current.node.y,
            ));
        }
        if current.node.y > 0 {
            neighbors.push((
                current.node.x,
                current.node.y - 1,
                Orientation::Vertical,
                current.node.x,
            ));
        }
        if current.node.y < max_y {
            neighbors.push((
                current.node.x,
                current.node.y + 1,
                Orientation::Vertical,
                current.node.x,
            ));
        }

        for (next_x, next_y, orientation, channel) in neighbors {
            let (start, end) = match orientation {
                Orientation::Horizontal => (current.node.x.min(next_x), current.node.x.max(next_x)),
                Orientation::Vertical => (current.node.y.min(next_y), current.node.y.max(next_y)),
            };
            let tracks = match (current.node.last_orientation, current.node.last_track) {
                (Some(previous), Some(track)) if previous == orientation => vec![track],
                _ => (0..capacity).collect::<Vec<_>>(),
            };
            for track in tracks {
                if !allocator.is_free(orientation, channel, start, end, usize::MAX, track) {
                    continue;
                }
                let next_node = MazeNode {
                    x: next_x,
                    y: next_y,
                    last_orientation: Some(orientation),
                    last_track: Some(track),
                };
                let via_penalty = usize::from(
                    current
                        .node
                        .last_orientation
                        .is_some_and(|previous| previous != orientation),
                ) * 2;
                let next_cost = current.cost + 1 + via_penalty;
                if next_cost >= best.get(&next_node).copied().unwrap_or(usize::MAX) {
                    continue;
                }
                best.insert(next_node, next_cost);
                let edge = MazeEdge {
                    to: (next_x, next_y),
                    orientation,
                    channel,
                    start,
                    end,
                    track,
                };
                steps.push(MazeSearchStep {
                    node: next_node,
                    previous: Some(current_index),
                    edge: Some(edge),
                    cost: next_cost,
                });
                let step = steps.len() - 1;
                queue.push(std::cmp::Reverse(QueueKey {
                    priority: next_cost + estimate(next_node),
                    step,
                }));
            }
        }
    }

    let Some(mut index) = target_step else {
        return Ok(None);
    };
    let mut edges = Vec::new();
    while let Some(edge) = steps[index].edge {
        edges.push(edge);
        let Some(previous) = steps[index].previous else {
            return Err("maze search traceback lost its root".into());
        };
        index = previous;
    }
    edges.reverse();
    Ok(Some(edges))
}

/// Detailed maze routing on the placement channel grid.
///
/// Unlike the two-L-shape global router, this router can take arbitrary
/// rectilinear detours. It reserves previously routed intervals and charges a
/// small bend penalty, allowing later nets to escape local channel conflicts.
pub fn route_maze(nl: &Netlist, p: &Placement) -> Result<Routing, String> {
    route_maze_with_options_cancellable(nl, p, &RouteOptions::default(), &FlowCancellation::new())
}

pub fn route_maze_with_options(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
) -> Result<Routing, String> {
    route_maze_with_options_cancellable(nl, p, options, &FlowCancellation::new())
}

pub fn route_maze_with_options_cancellable(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
    cancellation: &FlowCancellation,
) -> Result<Routing, String> {
    if cancellation.is_cancelled() {
        return Err("maze routing cancelled".into());
    }
    check_legal(nl, p)?;
    options.validate()?;
    let mut allocator = TrackAllocator::with_blockages(options.blocked_set());
    let mut segments: Vec<Segment> = Vec::new();
    let mut via_locations = Vec::new();
    let mut congested = Vec::new();

    for ni in 0..nl.net_count() {
        if cancellation.is_cancelled() {
            return Err("maze routing cancelled".into());
        }
        let pins = net_pins(nl, p, ni);
        if pins.len() < 2 {
            continue;
        }
        let source = pins[0];
        for &sink in &pins[1..] {
            if cancellation.is_cancelled() {
                return Err("maze routing cancelled".into());
            }
            let edges = maze_path(
                &allocator,
                options.tracks_per_channel,
                source,
                sink,
                SITES_PER_ROW,
                p.rows,
                cancellation,
            )?;
            let Some(edges) = edges else {
                if source.0 != sink.0 {
                    congested.push((Orientation::Horizontal, source.1));
                    congested.push((Orientation::Horizontal, sink.1));
                }
                if source.1 != sink.1 {
                    congested.push((Orientation::Vertical, source.0));
                    congested.push((Orientation::Vertical, sink.0));
                }
                continue;
            };
            if edges.is_empty() {
                continue;
            }

            for edge in &edges {
                allocator.reserve(
                    edge.orientation,
                    edge.channel,
                    edge.start,
                    edge.end,
                    edge.track,
                    ni,
                );
            }

            for edge in &edges {
                let mut merged = false;
                if let Some(last) = segments.last_mut() {
                    if last.net == ni
                        && last.orientation == edge.orientation
                        && last.channel == edge.channel
                        && last.track == edge.track
                        && (last.end == edge.start || edge.end == last.start)
                    {
                        last.start = last.start.min(edge.start);
                        last.end = last.end.max(edge.end);
                        merged = true;
                    }
                }
                if !merged {
                    segments.push(Segment {
                        net: ni,
                        orientation: edge.orientation,
                        channel: edge.channel,
                        start: edge.start,
                        end: edge.end,
                        track: edge.track,
                    });
                }
            }

            for pair in edges.windows(2) {
                if pair[0].orientation != pair[1].orientation {
                    let (x, y) = pair[0].to;
                    via_locations.push(Via { net: ni, x, y });
                }
            }
        }
    }

    congested.sort_unstable();
    congested.dedup();
    if !congested.is_empty() && !options.allow_congestion {
        return Err(format!(
            "maze routing congestion in {} channel(s): {:?}",
            congested.len(),
            congested
        ));
    }
    let vias = via_locations.len();
    let routing = Routing {
        segments,
        vias,
        via_locations,
        tracks_per_channel: options.tracks_per_channel,
        congested_channels: congested,
        blocked_channels: options.blocked_channels.clone(),
    };
    check_routing(&routing)?;
    Ok(routing)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MazeConnection {
    net: usize,
    source: (usize, usize),
    sink: (usize, usize),
}

type MazeResource = (Orientation, usize, usize, usize, usize);

fn maze_resource(edge: &MazeEdge) -> MazeResource {
    (
        edge.orientation,
        edge.channel,
        edge.track,
        edge.start,
        edge.end,
    )
}

fn insert_maze_path(
    occupancy: &mut HashMap<MazeResource, std::collections::BTreeSet<usize>>,
    path_index: usize,
    path: &[MazeEdge],
) {
    for edge in path {
        occupancy
            .entry(maze_resource(edge))
            .or_default()
            .insert(path_index);
    }
}

fn remove_maze_path(
    occupancy: &mut HashMap<MazeResource, std::collections::BTreeSet<usize>>,
    path_index: usize,
    path: &[MazeEdge],
) {
    for edge in path {
        if let Some(paths) = occupancy.get_mut(&maze_resource(edge)) {
            paths.remove(&path_index);
            if paths.is_empty() {
                occupancy.remove(&maze_resource(edge));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn negotiated_maze_path(
    occupancy: &HashMap<MazeResource, std::collections::BTreeSet<usize>>,
    history: &HashMap<MazeResource, usize>,
    blocked: &std::collections::BTreeSet<(Orientation, usize)>,
    paths: &[Option<Vec<MazeEdge>>],
    connections: &[MazeConnection],
    path_index: usize,
    capacity: usize,
    max_x: usize,
    max_y: usize,
    cancellation: &FlowCancellation,
) -> Result<Option<Vec<MazeEdge>>, String> {
    let connection = connections[path_index];
    let source = connection.source;
    let sink = connection.sink;
    if source == sink {
        return Ok(Some(Vec::new()));
    }

    #[derive(Eq, PartialEq, Ord, PartialOrd, Hash)]
    struct QueueKey {
        priority: usize,
        step: usize,
    }

    const PRESENT_CONGESTION_PENALTY: usize = 8;
    let mut queue: std::collections::BinaryHeap<std::cmp::Reverse<QueueKey>> =
        std::collections::BinaryHeap::new();
    let start = MazeNode {
        x: source.0,
        y: source.1,
        last_orientation: None,
        last_track: None,
    };
    let mut steps = vec![MazeSearchStep {
        node: start,
        previous: None,
        edge: None,
        cost: 0,
    }];
    let mut best = HashMap::from([(start, 0usize)]);
    let estimate = |node: MazeNode| -> usize { node.x.abs_diff(sink.0) + node.y.abs_diff(sink.1) };
    queue.push(std::cmp::Reverse(QueueKey {
        priority: estimate(start),
        step: 0,
    }));

    let mut target_step = None;
    let mut expansions = 0usize;
    while let Some(std::cmp::Reverse(item)) = queue.pop() {
        expansions += 1;
        if expansions.is_multiple_of(128) && cancellation.is_cancelled() {
            return Err("negotiated maze routing cancelled".into());
        }
        let current_index = item.step;
        let current = steps[current_index].clone();
        if current.cost > best.get(&current.node).copied().unwrap_or(usize::MAX) {
            continue;
        }
        if current.node.x == sink.0 && current.node.y == sink.1 {
            target_step = Some(current_index);
            break;
        }

        let mut neighbors = Vec::with_capacity(4);
        if current.node.x > 0 {
            neighbors.push((
                current.node.x - 1,
                current.node.y,
                Orientation::Horizontal,
                current.node.y,
            ));
        }
        if current.node.x < max_x {
            neighbors.push((
                current.node.x + 1,
                current.node.y,
                Orientation::Horizontal,
                current.node.y,
            ));
        }
        if current.node.y > 0 {
            neighbors.push((
                current.node.x,
                current.node.y - 1,
                Orientation::Vertical,
                current.node.x,
            ));
        }
        if current.node.y < max_y {
            neighbors.push((
                current.node.x,
                current.node.y + 1,
                Orientation::Vertical,
                current.node.x,
            ));
        }

        for (next_x, next_y, orientation, channel) in neighbors {
            if blocked.contains(&(orientation, channel)) {
                continue;
            }
            let (start, end) = match orientation {
                Orientation::Horizontal => (current.node.x.min(next_x), current.node.x.max(next_x)),
                Orientation::Vertical => (current.node.y.min(next_y), current.node.y.max(next_y)),
            };
            let tracks = match (current.node.last_orientation, current.node.last_track) {
                (Some(previous), Some(track)) if previous == orientation => vec![track],
                _ => (0..capacity).collect::<Vec<_>>(),
            };
            for track in tracks {
                let edge = MazeEdge {
                    to: (next_x, next_y),
                    orientation,
                    channel,
                    start,
                    end,
                    track,
                };
                let resource = maze_resource(&edge);
                let foreign_nets = occupancy
                    .get(&resource)
                    .map(|path_ids| {
                        path_ids
                            .iter()
                            .filter(|&&other| {
                                other != path_index
                                    && paths
                                        .get(other)
                                        .and_then(|path| path.as_ref())
                                        .is_some_and(|_| connections[other].net != connection.net)
                            })
                            .count()
                    })
                    .unwrap_or(0);
                let history_penalty = history.get(&resource).copied().unwrap_or(0);
                let via_penalty = usize::from(
                    current
                        .node
                        .last_orientation
                        .is_some_and(|previous| previous != orientation),
                ) * 2;
                let next_cost = current.cost
                    + 1
                    + via_penalty
                    + foreign_nets * PRESENT_CONGESTION_PENALTY
                    + history_penalty;

                let next_node = MazeNode {
                    x: next_x,
                    y: next_y,
                    last_orientation: Some(orientation),
                    last_track: Some(track),
                };
                if next_cost >= best.get(&next_node).copied().unwrap_or(usize::MAX) {
                    continue;
                }
                best.insert(next_node, next_cost);
                steps.push(MazeSearchStep {
                    node: next_node,
                    previous: Some(current_index),
                    edge: Some(edge),
                    cost: next_cost,
                });
                let step = steps.len() - 1;
                queue.push(std::cmp::Reverse(QueueKey {
                    priority: next_cost + estimate(next_node),
                    step,
                }));
            }
        }
    }

    let Some(mut index) = target_step else {
        return Ok(None);
    };
    let mut edges = Vec::new();
    while let Some(edge) = steps[index].edge {
        edges.push(edge);
        let Some(previous) = steps[index].previous else {
            return Err("negotiated maze search traceback lost its root".into());
        };
        index = previous;
    }
    edges.reverse();
    Ok(Some(edges))
}

/// Result of negotiated-congestion maze routing.
#[derive(Clone, Debug)]
pub struct MazeRipupResult {
    pub routing: Routing,
    pub passes: usize,
    pub historical_conflicts: usize,
}

/// Result of timing-prioritized negotiated routing.
#[derive(Clone, Debug)]
pub struct TimingDrivenRipupResult {
    pub ripup: MazeRipupResult,
    /// Weighted wirelength of the neutral-priority baseline route.
    pub initial_weighted_wirelength: f64,
    /// Weighted wirelength after timing-prioritized rip-up/reroute.
    pub final_weighted_wirelength: f64,
}

/// Negotiated-congestion rip-up and reroute.
///
/// Every pass removes one two-pin route at a time and reroutes it with a soft
/// congestion penalty. Unlike the strictly sequential maze router, competing
/// routes may temporarily overlap; resources used by multiple nets accrue
/// history cost. Later passes therefore move repeat offenders to detours.
pub fn route_maze_ripgup(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
    max_passes: usize,
) -> Result<MazeRipupResult, String> {
    route_maze_ripgup_impl(nl, p, options, max_passes, None, &FlowCancellation::new())
}

pub fn route_maze_ripgup_cancellable(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
    max_passes: usize,
    cancellation: &FlowCancellation,
) -> Result<MazeRipupResult, String> {
    route_maze_ripgup_impl(nl, p, options, max_passes, None, cancellation)
}

fn route_maze_ripgup_impl(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
    max_passes: usize,
    timing: Option<&TimingDrivenRoutingOptions>,
    cancellation: &FlowCancellation,
) -> Result<MazeRipupResult, String> {
    if cancellation.is_cancelled() {
        return Err("rip-up routing cancelled".into());
    }
    check_legal(nl, p)?;
    options.validate()?;
    if max_passes == 0 {
        return Err("rip-up routing requires at least one pass".into());
    }
    if let Some(timing) = timing {
        timing.validate(nl.net_count())?;
    }

    let mut connections = Vec::new();
    for ni in 0..nl.net_count() {
        if cancellation.is_cancelled() {
            return Err("rip-up routing cancelled".into());
        }
        let pins = net_pins(nl, p, ni);
        if pins.len() < 2 {
            continue;
        }
        let source = pins[0];
        for &sink in &pins[1..] {
            connections.push(MazeConnection {
                net: ni,
                source,
                sink,
            });
        }
    }
    if let Some(timing) = timing {
        connections.sort_by(|left, right| {
            timing
                .weight(right.net)
                .total_cmp(&timing.weight(left.net))
                .then_with(|| left.net.cmp(&right.net))
        });
    }
    if connections.is_empty() {
        let routing = Routing {
            segments: Vec::new(),
            vias: 0,
            via_locations: Vec::new(),
            tracks_per_channel: options.tracks_per_channel,
            congested_channels: Vec::new(),
            blocked_channels: options.blocked_channels.clone(),
        };
        return Ok(MazeRipupResult {
            routing,
            passes: 0,
            historical_conflicts: 0,
        });
    }

    let mut paths: Vec<Option<Vec<MazeEdge>>> = vec![None; connections.len()];
    let blocked = options.blocked_set();
    let mut occupancy = HashMap::new();
    let mut history: HashMap<MazeResource, usize> = HashMap::new();
    let mut historical_conflicts = 0usize;
    let mut passes = 0usize;
    let mut unresolved_conflicts = 0usize;

    for pass in 0..max_passes {
        passes = pass + 1;
        for path_index in 0..connections.len() {
            if cancellation.is_cancelled() {
                return Err("rip-up routing cancelled".into());
            }
            if let Some(old_path) = paths[path_index].as_ref() {
                remove_maze_path(&mut occupancy, path_index, old_path);
            }
            let new_path = negotiated_maze_path(
                &occupancy,
                &history,
                &blocked,
                &paths,
                &connections,
                path_index,
                options.tracks_per_channel,
                SITES_PER_ROW,
                p.rows,
                cancellation,
            )?
            .ok_or_else(|| "negotiated maze routing failed to find a path".to_string())?;
            insert_maze_path(&mut occupancy, path_index, &new_path);
            paths[path_index] = Some(new_path);
        }

        let mut conflicts = Vec::new();
        for (resource, path_ids) in &occupancy {
            if cancellation.is_cancelled() {
                return Err("rip-up routing cancelled".into());
            }
            let mut nets = std::collections::BTreeSet::new();
            for &path_index in path_ids {
                nets.insert(connections[path_index].net);
            }
            if nets.len() > 1 {
                conflicts.push((*resource, nets.len() - 1));
            }
        }
        unresolved_conflicts = conflicts.len();
        if conflicts.is_empty() {
            break;
        }
        historical_conflicts += conflicts.len();
        for (resource, excess) in conflicts {
            *history.entry(resource).or_default() += excess;
        }
    }

    if unresolved_conflicts > 0 {
        return Err(format!(
            "negotiated maze routing did not converge in {} pass(es)",
            passes
        ));
    }

    let mut segments: Vec<Segment> = Vec::new();
    let mut via_locations = Vec::new();
    for (path_index, path) in paths.iter().enumerate() {
        if cancellation.is_cancelled() {
            return Err("rip-up routing cancelled".into());
        }
        let Some(path) = path.as_ref() else {
            continue;
        };
        let net = connections[path_index].net;
        for edge in path {
            let mut merged = false;
            if let Some(last) = segments.last_mut() {
                if last.net == net
                    && last.orientation == edge.orientation
                    && last.channel == edge.channel
                    && last.track == edge.track
                    && (last.end == edge.start || edge.end == last.start)
                {
                    last.start = last.start.min(edge.start);
                    last.end = last.end.max(edge.end);
                    merged = true;
                }
            }
            if !merged {
                segments.push(Segment {
                    net,
                    orientation: edge.orientation,
                    channel: edge.channel,
                    start: edge.start,
                    end: edge.end,
                    track: edge.track,
                });
            }
        }
        for pair in path.windows(2) {
            if pair[0].orientation != pair[1].orientation {
                let (x, y) = pair[0].to;
                via_locations.push(Via { net, x, y });
            }
        }
    }
    let vias = via_locations.len();
    let routing = Routing {
        segments,
        vias,
        via_locations,
        tracks_per_channel: options.tracks_per_channel,
        congested_channels: Vec::new(),
        blocked_channels: options.blocked_channels.clone(),
    };
    let routing = match check_routing(&routing) {
        Ok(()) => routing,
        Err(_) => strict_priority_maze_routing(nl, p, options, &connections, cancellation)?,
    };
    Ok(MazeRipupResult {
        routing,
        passes,
        historical_conflicts,
    })
}

/// Conflict-free fallback for negotiated routing.
///
/// Negotiated congestion uses exact-edge history keys. Two partial-edge
/// reservations on the same track can therefore overlap even when every
/// complete-edge conflict has converged. The fallback preserves the caller's
/// connection priority while allocating full overlapping intervals strictly.
fn strict_priority_maze_routing(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
    connections: &[MazeConnection],
    cancellation: &FlowCancellation,
) -> Result<Routing, String> {
    if cancellation.is_cancelled() {
        return Err("maze routing cancelled".into());
    }
    let mut allocator = TrackAllocator::with_blockages(options.blocked_set());
    let mut segments: Vec<Segment> = Vec::new();
    let mut via_locations = Vec::new();
    for connection in connections {
        if cancellation.is_cancelled() {
            return Err("maze routing cancelled".into());
        }
        let edges = maze_path(
            &allocator,
            options.tracks_per_channel,
            connection.source,
            connection.sink,
            SITES_PER_ROW,
            p.rows,
            cancellation,
        )?
        .ok_or_else(|| {
            format!(
                "strict priority maze routing failed for net '{}'",
                nl.net(connection.net).name
            )
        })?;
        if edges.is_empty() {
            continue;
        }
        for edge in &edges {
            allocator.reserve(
                edge.orientation,
                edge.channel,
                edge.start,
                edge.end,
                edge.track,
                connection.net,
            );
        }
        for edge in &edges {
            let mut merged = false;
            if let Some(last) = segments.last_mut() {
                if last.net == connection.net
                    && last.orientation == edge.orientation
                    && last.channel == edge.channel
                    && last.track == edge.track
                    && (last.end == edge.start || edge.end == last.start)
                {
                    last.start = last.start.min(edge.start);
                    last.end = last.end.max(edge.end);
                    merged = true;
                }
            }
            if !merged {
                segments.push(Segment {
                    net: connection.net,
                    orientation: edge.orientation,
                    channel: edge.channel,
                    start: edge.start,
                    end: edge.end,
                    track: edge.track,
                });
            }
        }
        for pair in edges.windows(2) {
            if pair[0].orientation != pair[1].orientation {
                via_locations.push(Via {
                    net: connection.net,
                    x: pair[0].to.0,
                    y: pair[0].to.1,
                });
            }
        }
    }
    let routing = Routing {
        segments,
        vias: via_locations.len(),
        via_locations,
        tracks_per_channel: options.tracks_per_channel,
        congested_channels: Vec::new(),
        blocked_channels: options.blocked_channels.clone(),
    };
    check_routing(&routing)?;
    Ok(routing)
}

fn weighted_routing_wirelength(routing: &Routing, timing: &TimingDrivenRoutingOptions) -> f64 {
    routing
        .segments
        .iter()
        .map(|segment| segment.length() as f64 * timing.weight(segment.net))
        .sum()
}

/// Timing-prioritized negotiated-congestion rip-up/reroute.
///
/// The baseline uses neutral net priority. The optimized run reroutes
/// high-weight nets first and reports the weighted wirelength change.
pub fn route_maze_timing_ripgup(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
    timing: &TimingDrivenRoutingOptions,
    max_passes: usize,
) -> Result<TimingDrivenRipupResult, String> {
    route_maze_timing_ripgup_cancellable(
        nl,
        p,
        options,
        timing,
        max_passes,
        &FlowCancellation::new(),
    )
}

pub fn route_maze_timing_ripgup_cancellable(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
    timing: &TimingDrivenRoutingOptions,
    max_passes: usize,
    cancellation: &FlowCancellation,
) -> Result<TimingDrivenRipupResult, String> {
    timing.validate(nl.net_count())?;
    let baseline = route_maze_ripgup_impl(nl, p, options, max_passes, None, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("timing-driven rip-up routing cancelled".into());
    }
    let optimized = route_maze_ripgup_impl(nl, p, options, max_passes, Some(timing), cancellation)?;
    let initial = weighted_routing_wirelength(&baseline.routing, timing);
    let final_weight = weighted_routing_wirelength(&optimized.routing, timing);
    Ok(TimingDrivenRipupResult {
        ripup: optimized,
        initial_weighted_wirelength: initial,
        final_weighted_wirelength: final_weight,
    })
}

pub fn timing_driven_ripgup_report(result: &TimingDrivenRipupResult) -> String {
    format!(
        "Timing-driven rip-up report: {} pass(es), {} historical conflict(s)\n  weighted wirelength : {:.3} -> {:.3}\n",
        result.ripup.passes,
        result.ripup.historical_conflicts,
        result.initial_weighted_wirelength,
        result.final_weighted_wirelength
    )
}

fn shape_via(segments: &[Segment], net: usize) -> Option<Via> {
    let (a, b) = (
        segments.last()?,
        segments.get(segments.len().checked_sub(2)?)?,
    );
    let horizontal = match (a.orientation, b.orientation) {
        (Orientation::Horizontal, Orientation::Vertical) => a,
        (Orientation::Vertical, Orientation::Horizontal) => b,
        _ => return None,
    };
    let vertical = if std::ptr::eq(a, horizontal) { b } else { a };
    (horizontal.net == net && vertical.net == net).then_some(Via {
        net,
        x: vertical.channel,
        y: horizontal.channel,
    })
}

/// Validate channel ranges, track bounds, and cross-net track exclusivity.
pub fn check_routing(r: &Routing) -> Result<(), String> {
    if r.tracks_per_channel == 0 {
        return Err("routing has zero tracks per channel".into());
    }
    let mut used: TrackUsageMap = HashMap::new();
    let mut blocked = std::collections::BTreeSet::new();
    for blockage in &r.blocked_channels {
        blockage.validate()?;
        if !blocked.insert((blockage.orientation, blockage.channel)) {
            return Err(format!(
                "duplicate routing blockage for {:?} channel {}",
                blockage.orientation, blockage.channel
            ));
        }
    }
    for seg in &r.segments {
        if seg.start > seg.end {
            return Err(format!("reversed segment {:?}", seg));
        }
        if seg.track >= r.tracks_per_channel {
            return Err(format!("track {} out of range {:?}", seg.track, seg));
        }
        if blocked.contains(&(seg.orientation, seg.channel)) {
            return Err(format!(
                "segment {:?} crosses hard routing blockage {:?} channel {}",
                seg, seg.orientation, seg.channel
            ));
        }
        let ranges = used
            .entry((seg.orientation, seg.channel, seg.track))
            .or_default();
        if ranges
            .iter()
            .any(|&(s, e, net)| net != seg.net && seg.start <= e && seg.end >= s)
        {
            return Err(format!("track collision {:?}", seg));
        }
        ranges.push((seg.start, seg.end, seg.net));
    }
    for via in &r.via_locations {
        let horizontal = r.segments.iter().any(|seg| {
            seg.net == via.net
                && seg.orientation == Orientation::Horizontal
                && point_on_segment((via.x, via.y), seg)
        });
        let vertical = r.segments.iter().any(|seg| {
            seg.net == via.net
                && seg.orientation == Orientation::Vertical
                && point_on_segment((via.x, via.y), seg)
        });
        if !horizontal || !vertical {
            return Err(format!("via {:?} does not connect its net segments", via));
        }
    }
    Ok(())
}

/// Text summary of global routing.
pub fn routing_report(r: &Routing) -> String {
    let mut out = format!(
        "Routing report: {} segments, {} wirelength units, {} vias, {} tracks/channel\n",
        r.segments.len(),
        r.wirelength(),
        r.vias,
        r.tracks_per_channel
    );
    if !r.blocked_channels.is_empty() {
        out.push_str(&format!("  hard blockages: {}\n", r.blocked_channels.len()));
    }
    out
}

/// Run placement/routing design-rule checks.
///
/// Errors cover illegal placement, invalid track allocation, out-of-bounds
/// channels, cross-net shorts, and disconnected multi-pin nets. Warnings
/// cover conservative antenna-ratio violations.
pub fn drc(nl: &Netlist, p: &Placement, r: &Routing) -> Result<DrcResult, String> {
    let mut out = DrcResult::default();

    if p.cell_width.len() != nl.cell_count() || p.cell_pos.len() != nl.cell_count() {
        out.violations.push(DrcViolation {
            code: "PLACE-001".into(),
            severity: DrcSeverity::Error,
            message: format!(
                "placement width/vector length does not match {} cells",
                nl.cell_count()
            ),
            cell: None,
            net: None,
            row: None,
            site: None,
        });
    }
    if let Err(e) = check_legal(nl, p) {
        out.violations.push(DrcViolation {
            code: "PLACE-002".into(),
            severity: DrcSeverity::Error,
            message: e,
            cell: None,
            net: None,
            row: None,
            site: None,
        });
    }
    if let Err(e) = check_routing(r) {
        out.violations.push(DrcViolation {
            code: "ROUTE-001".into(),
            severity: DrcSeverity::Error,
            message: e,
            cell: None,
            net: None,
            row: None,
            site: None,
        });
    }

    for seg in &r.segments {
        if seg.net >= nl.net_count() {
            out.violations.push(DrcViolation {
                code: "ROUTE-002".into(),
                severity: DrcSeverity::Error,
                message: format!("segment references unknown net {}", seg.net),
                cell: None,
                net: Some(seg.net),
                row: if seg.orientation == Orientation::Horizontal {
                    Some(seg.channel)
                } else {
                    None
                },
                site: Some(seg.channel),
            });
            continue;
        }

        let max_channel = match seg.orientation {
            Orientation::Horizontal => p.rows,
            Orientation::Vertical => SITES_PER_ROW,
        };
        let max_range = match seg.orientation {
            Orientation::Horizontal => SITES_PER_ROW,
            Orientation::Vertical => p.rows,
        };
        if seg.channel > max_channel || seg.end > max_range {
            out.violations.push(DrcViolation {
                code: "ROUTE-003".into(),
                severity: DrcSeverity::Error,
                message: format!(
                    "segment for net '{}' outside die bounds",
                    nl.net(seg.net).name
                ),
                cell: None,
                net: Some(seg.net),
                row: if seg.orientation == Orientation::Horizontal {
                    Some(seg.channel)
                } else {
                    None
                },
                site: Some(seg.channel),
            });
        }
    }

    for via in &r.via_locations {
        if via.net >= nl.net_count() {
            out.violations.push(DrcViolation {
                code: "VIA-001".into(),
                severity: DrcSeverity::Error,
                message: format!("via references unknown net {}", via.net),
                cell: None,
                net: Some(via.net),
                row: Some(via.y),
                site: Some(via.x),
            });
        }
        if via.x > SITES_PER_ROW || via.y > p.rows {
            out.violations.push(DrcViolation {
                code: "VIA-002".into(),
                severity: DrcSeverity::Error,
                message: format!("via {:?} outside die bounds", via),
                cell: None,
                net: Some(via.net),
                row: Some(via.y),
                site: Some(via.x),
            });
        }
    }

    for ni in 0..nl.net_count() {
        let pins = net_pins(nl, p, ni);
        let segments: Vec<Segment> = r
            .segments
            .iter()
            .copied()
            .filter(|seg| seg.net == ni)
            .collect();
        let net_vias: Vec<Via> = r
            .via_locations
            .iter()
            .copied()
            .filter(|via| via.net == ni)
            .collect();
        let components = connected_pin_components(&pins, &segments, &net_vias);

        if pins.len() >= 2 && components > 1 {
            out.violations.push(DrcViolation {
                code: "OPEN-001".into(),
                severity: DrcSeverity::Error,
                message: format!(
                    "net '{}' has {} physically disconnected pin group(s)",
                    nl.net(ni).name,
                    components
                ),
                cell: None,
                net: Some(ni),
                row: None,
                site: None,
            });
        }

        let pin_count = pins.len().max(1);
        let ratio = segments.iter().map(Segment::length).sum::<usize>() as f64 / pin_count as f64;
        if ratio > MAX_ANTENNA_RATIO {
            out.violations.push(DrcViolation {
                code: "ANT-001".into(),
                severity: DrcSeverity::Warning,
                message: format!(
                    "net '{}' antenna ratio {:.2} exceeds {:.2}",
                    nl.net(ni).name,
                    ratio,
                    MAX_ANTENNA_RATIO
                ),
                cell: None,
                net: Some(ni),
                row: None,
                site: None,
            });
        }

        if !segments.is_empty() && pins.is_empty() {
            out.violations.push(DrcViolation {
                code: "DANGLE-001".into(),
                severity: DrcSeverity::Error,
                message: format!("net '{}' has routing but no physical pins", nl.net(ni).name),
                cell: None,
                net: Some(ni),
                row: None,
                site: None,
            });
        }
    }

    out.violations.sort_by(|a, b| {
        (a.severity, a.code.clone(), a.message.clone()).cmp(&(
            b.severity,
            b.code.clone(),
            b.message.clone(),
        ))
    });
    Ok(out)
}

fn point_on_segment(point: (usize, usize), seg: &Segment) -> bool {
    match seg.orientation {
        Orientation::Horizontal => {
            point.1 == seg.channel && point.0 >= seg.start && point.0 <= seg.end
        }
        Orientation::Vertical => {
            point.0 == seg.channel && point.1 >= seg.start && point.1 <= seg.end
        }
    }
}

fn segments_touch(a: &Segment, b: &Segment) -> bool {
    match (a.orientation, b.orientation) {
        (Orientation::Horizontal, Orientation::Vertical) => {
            b.channel >= a.start && b.channel <= a.end && a.channel >= b.start && a.channel <= b.end
        }
        (Orientation::Vertical, Orientation::Horizontal) => segments_touch(b, a),
        (Orientation::Horizontal, Orientation::Horizontal) => {
            a.channel == b.channel && a.track == b.track && a.start <= b.end && b.start <= a.end
        }
        (Orientation::Vertical, Orientation::Vertical) => {
            a.channel == b.channel && a.track == b.track && a.start <= b.end && b.start <= a.end
        }
    }
}

fn connected_pin_components(pins: &[(usize, usize)], segments: &[Segment], vias: &[Via]) -> usize {
    if pins.is_empty() {
        return 0;
    }
    let mut parent: Vec<usize> = (0..pins.len() + segments.len()).collect();

    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let (a, b) = (find(parent, a), find(parent, b));
        if a != b {
            parent[a] = b;
        }
    }

    for i in 0..pins.len() {
        for j in i + 1..pins.len() {
            if pins[i] == pins[j] {
                union(&mut parent, i, j);
            }
        }
        for (si, seg) in segments.iter().enumerate() {
            if point_on_segment(pins[i], seg) {
                union(&mut parent, i, pins.len() + si);
            }
        }
    }
    for i in 0..segments.len() {
        for j in i + 1..segments.len() {
            let a = &segments[i];
            let b = &segments[j];
            let connected = if a.orientation == b.orientation {
                a.net == b.net && segments_touch(a, b)
            } else {
                a.net == b.net
                    && segments_touch(a, b)
                    && vias.iter().any(|via| {
                        via.net == a.net
                            && point_on_segment((via.x, via.y), a)
                            && point_on_segment((via.x, via.y), b)
                    })
            };
            if connected {
                union(&mut parent, pins.len() + i, pins.len() + j);
            }
        }
    }

    let mut roots = std::collections::HashSet::new();
    for i in 0..pins.len() {
        roots.insert(find(&mut parent, i));
    }
    roots.len()
}

/// Human-readable DRC summary.
pub fn drc_report(r: &DrcResult) -> String {
    let mut s = format!(
        "DRC report: {} error(s), {} warning(s)\n",
        r.error_count(),
        r.warning_count()
    );
    for v in &r.violations {
        s.push_str(&format!(
            "  {}: {} [{}]{}\n",
            v.code,
            v.message,
            if v.severity == DrcSeverity::Error {
                "error"
            } else {
                "warning"
            },
            v.net.map(|n| format!(" net={}", n)).unwrap_or_default()
        ));
    }
    s
}

/// Text summary of the placement.
pub fn report(nl: &Netlist, p: &Placement) -> String {
    let used: usize = p.cell_width.iter().sum();
    let mut out = format!(
        "PnR report for '{}': {} cells, {} rows x {} sites, utilization {:.1}%\n",
        nl.name,
        nl.cell_count(),
        p.rows,
        SITES_PER_ROW,
        100.0 * used as f64 / (p.rows * SITES_PER_ROW) as f64
    );
    if !p.blockages.is_empty() {
        out.push_str(&format!(
            "  hard placement blockages: {}\n",
            p.blockages.len()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::library::CellLibrary;
    use eda_core::netlist::{Cell, Dir};
    use std::sync::Arc;

    fn congested_routing_fixture() -> (Netlist, Placement) {
        let mut nl = Netlist::new("congested");
        let n0 = nl.add_net("n0");
        let n1 = nl.add_net("n1");
        let n2 = nl.add_net("n2");
        let n3 = nl.add_net("n3");
        nl.add_cell(Cell {
            name: "d0".into(),
            kind: CellKind::Not,
            inputs: vec![],
            output: n0,
        });
        nl.add_cell(Cell {
            name: "d1".into(),
            kind: CellKind::Not,
            inputs: vec![],
            output: n1,
        });
        nl.add_cell(Cell {
            name: "s0".into(),
            kind: CellKind::Not,
            inputs: vec![n0],
            output: n2,
        });
        nl.add_cell(Cell {
            name: "s1".into(),
            kind: CellKind::Not,
            inputs: vec![n1],
            output: n3,
        });
        let placement = Placement {
            rows: 1,
            cell_pos: vec![
                Pos { row: 0, site: 0 },
                Pos { row: 0, site: 4 },
                Pos { row: 0, site: 8 },
                Pos { row: 0, site: 12 },
            ],
            cell_width: vec![2, 2, 2, 2],
            blockages: Vec::new(),
        };
        (nl, placement)
    }

    #[test]
    fn simple_place_legal() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("p");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let p = place(&nl).unwrap();
        check_legal(&nl, &p).unwrap();
        assert!(hpwl(&nl, &p) >= 0.0);
    }

    #[test]
    fn placement_avoids_hard_blockages() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("placement-blockage");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);

        let blockage = PlacementBlockage::new("macro", 0, 0, 2).unwrap();
        let placement = place_with_blockages(&nl, std::slice::from_ref(&blockage)).unwrap();
        assert_eq!(placement.blockages, vec![blockage.clone()]);
        check_legal(&nl, &placement).unwrap();
        assert!(placement
            .cell_pos
            .iter()
            .any(|position| { position.row > 0 || position.site > blockage.end_site }));
        assert!(report(&nl, &placement).contains("hard placement blockages: 1"));

        let mut illegal = placement.clone();
        illegal.cell_pos[0] = Pos {
            row: 0,
            site: blockage.start_site,
        };
        assert!(check_legal(&nl, &illegal).is_err());

        let duplicate = PlacementBlockage::new("duplicate", 0, 1, 4).unwrap();
        assert!(place_with_blockages(&nl, &[blockage, duplicate]).is_err());
        assert!(PlacementBlockage::new("", 0, 0, 2).is_err());
        assert!(PlacementBlockage::new("bad", 0, 3, 2).is_err());
    }

    #[test]
    fn maze_router_avoids_hard_channel_blockage() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("blocked");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let p = place(&nl).unwrap();

        let baseline = route_maze(&nl, &p).unwrap();
        assert!(baseline.segments.iter().any(|segment| {
            segment.orientation == Orientation::Vertical && segment.channel == 2
        }));

        let options = RouteOptions::new(4)
            .unwrap()
            .with_blocked_channel("keepout0", Orientation::Vertical, 2)
            .unwrap();
        assert!(options
            .clone()
            .with_blocked_channel("keepout0-duplicate", Orientation::Vertical, 2)
            .is_err());
        let routed = route_maze_with_options(&nl, &p, &options).unwrap();
        assert!(!routed.segments.is_empty());
        assert_eq!(routed.blocked_channels.len(), 1);
        assert!(routed.segments.iter().all(|segment| {
            !(segment.orientation == Orientation::Vertical && segment.channel == 2)
        }));
        check_routing(&routed).unwrap();
        assert!(routing_report(&routed).contains("hard blockages: 1"));

        let mut illegal = routed.clone();
        illegal.segments.push(Segment {
            net: 0,
            orientation: Orientation::Vertical,
            channel: 2,
            start: 0,
            end: 1,
            track: 0,
        });
        assert!(check_routing(&illegal).is_err());
    }

    #[test]
    fn extend_placement_legalizes_appended_cells() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("extend");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        let original = place(&nl).unwrap();
        let n = nl.add_net("n");
        nl.add_cell(Cell {
            name: "u2".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![y],
            output: n,
        });
        let extended = extend_placement(&nl, &original).unwrap();
        assert_eq!(extended.cell_pos.len(), nl.cell_count());
        assert_eq!(extended.cell_width.len(), nl.cell_count());
        check_legal(&nl, &extended).unwrap();

        let mut short = original.clone();
        short.cell_pos.push(original.cell_pos[0]);
        short.cell_pos.push(original.cell_pos[0]);
        assert!(extend_placement(&nl, &short).is_err());
    }

    #[test]
    fn timing_driven_placement_prioritizes_critical_net() {
        let mut nl = Netlist::new("critical_place");
        let mut sources = Vec::new();
        let mut sinks = Vec::new();
        for index in 0..3 {
            sources.push(nl.add_net(format!("src{}", index)));
        }
        for index in 0..3 {
            sinks.push(nl.add_net(format!("sink{}", index)));
        }
        for (index, source) in sources.iter().enumerate() {
            nl.add_cell(Cell {
                name: format!("d{}", index),
                kind: CellKind::Not,
                inputs: Vec::new(),
                output: *source,
            });
        }
        for index in 0..3 {
            nl.add_cell(Cell {
                name: format!("s{}", index),
                kind: CellKind::Not,
                inputs: vec![sources[index]],
                output: sinks[index],
            });
        }

        let options = TimingDrivenPlacementOptions {
            net_weights: HashMap::from([(sources[0], 100.0)]),
            default_net_weight: 1.0,
            max_passes: 4,
        };
        let result = place_timing_driven(&nl, &options).unwrap();
        check_legal(&nl, &result.placement).unwrap();
        assert!(result.improving_moves > 0);
        assert!(result.final_weighted_hpwl < result.initial_weighted_hpwl);
        assert!(result.final_weighted_hpwl <= result.initial_weighted_hpwl);
        assert!(timing_driven_placement_report(&result).contains("weighted HPWL"));

        let d0 = nl
            .cells()
            .iter()
            .position(|cell| cell.name == "d0")
            .unwrap();
        let s0 = nl
            .cells()
            .iter()
            .position(|cell| cell.name == "s0")
            .unwrap();
        let distance = |cell: usize| result.placement.cell_pos[cell].site;
        assert!(distance(s0) - distance(d0) <= 2);

        let mut invalid = options.clone();
        invalid.net_weights.insert(nl.net_count(), 1.0);
        assert!(place_timing_driven(&nl, &invalid).is_err());
    }

    #[test]
    fn global_routes_are_legal() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("route");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let n1 = nl.add_net("n1");
        let n2 = nl.add_net("n2");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: n1,
        });
        nl.add_cell(Cell {
            name: "u2".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![n1],
            output: n2,
        });
        nl.add_cell(Cell {
            name: "u3".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![n2, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let placement = place(&nl).unwrap();
        let routing = route(&nl, &placement).unwrap();
        check_routing(&routing).unwrap();
        assert!(!routing.segments.is_empty());
        assert!(routing.wirelength() > 0);
    }

    #[test]
    fn routing_capacity_is_configurable() {
        let (nl, placement) = congested_routing_fixture();
        let options = RouteOptions::new(2).unwrap();
        let routing = route_with_options(&nl, &placement, &options).unwrap();
        check_routing(&routing).unwrap();
        assert_eq!(routing.tracks_per_channel, 2);
        assert!(routing.congested_channels.is_empty());
        // Two different nets overlap in row channel 0 and use both tracks.
        assert!((routing.utilization() - 1.0).abs() < 1e-9);

        let one_track = RouteOptions::new(1).unwrap();
        assert!(route_with_options(&nl, &placement, &one_track).is_err());
    }

    #[test]
    fn maze_router_detours_instead_of_failing_on_local_conflict() {
        let (nl, placement) = congested_routing_fixture();
        let one_track = RouteOptions::new(1).unwrap();
        assert!(route_with_options(&nl, &placement, &one_track).is_err());

        let routing = route_maze_with_options(&nl, &placement, &one_track).unwrap();
        check_routing(&routing).unwrap();
        assert!(routing.congested_channels.is_empty());
        assert!(routing.vias >= 2);
        let drc_result = drc(&nl, &placement, &routing).unwrap();
        assert!(!drc_result.has_errors(), "{:?}", drc_result.violations);
        assert!(routing
            .segments
            .iter()
            .any(|segment| segment.orientation == Orientation::Horizontal
                && segment.channel == placement.rows));
    }

    #[test]
    fn negotiated_ripgup_router_converges_to_legal_solution() {
        let (nl, placement) = congested_routing_fixture();
        let options = RouteOptions::new(1).unwrap();
        let result = route_maze_ripgup(&nl, &placement, &options, 8).unwrap();
        check_routing(&result.routing).unwrap();
        assert_eq!(result.routing.congested_channels.len(), 0);
        assert!(result.passes >= 1);
        let drc_result = drc(&nl, &placement, &result.routing).unwrap();
        assert!(!drc_result.has_errors(), "{:?}", drc_result.violations);

        assert!(route_maze_ripgup(&nl, &placement, &options, 0).is_err());
    }

    #[test]
    fn routers_honor_cooperative_cancellation() {
        let (nl, placement) = congested_routing_fixture();
        let options = RouteOptions::new(1).unwrap();
        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            route_with_options_cancellable(&nl, &placement, &options, &cancellation).unwrap_err(),
            "global routing cancelled"
        );
        assert_eq!(
            route_maze_with_options_cancellable(&nl, &placement, &options, &cancellation)
                .unwrap_err(),
            "maze routing cancelled"
        );
        assert_eq!(
            route_maze_ripgup_cancellable(&nl, &placement, &options, 8, &cancellation).unwrap_err(),
            "rip-up routing cancelled"
        );
        let timing = TimingDrivenRoutingOptions {
            net_weights: HashMap::from([(1, 10.0)]),
            default_net_weight: 1.0,
        };
        assert_eq!(
            route_maze_timing_ripgup_cancellable(
                &nl,
                &placement,
                &options,
                &timing,
                8,
                &cancellation
            )
            .unwrap_err(),
            "rip-up routing cancelled"
        );
    }

    #[test]
    fn timing_driven_ripgup_prioritizes_critical_net() {
        let (nl, placement) = congested_routing_fixture();
        let options = RouteOptions::new(1).unwrap();
        let timing = TimingDrivenRoutingOptions {
            net_weights: HashMap::from([(1, 10.0)]),
            default_net_weight: 1.0,
        };
        let result = route_maze_timing_ripgup(&nl, &placement, &options, &timing, 8).unwrap();
        check_routing(&result.ripup.routing).unwrap();
        assert!(
            result.final_weighted_wirelength < result.initial_weighted_wirelength,
            "{:?}",
            result
        );
        assert!(timing_driven_ripgup_report(&result).contains("weighted wirelength"));

        let critical_horizontal_rows: std::collections::BTreeSet<_> = result
            .ripup
            .routing
            .segments
            .iter()
            .filter(|segment| segment.net == 1 && segment.orientation == Orientation::Horizontal)
            .map(|segment| segment.channel)
            .collect();
        assert!(
            critical_horizontal_rows.contains(&0),
            "{:?}",
            result.ripup.routing
        );

        let mut invalid = timing.clone();
        invalid.net_weights.insert(nl.net_count(), 1.0);
        assert!(route_maze_timing_ripgup(&nl, &placement, &options, &invalid, 8).is_err());
    }

    #[test]
    fn layer_changes_have_explicit_vias() {
        let design = eda_hdl::compile(
            "module m(input clk, rst, output reg [3:0] q);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 4'd0; else q <= q + 4'd1;
             endmodule",
            None,
        )
        .unwrap();
        let nl = eda_synth::synthesize(
            &design,
            &eda_synth::SynthOptions {
                optimize: true,
                map_lib: true,
            },
        )
        .unwrap()
        .netlist;
        let placement = place(&nl).unwrap();
        let mut routing = route(&nl, &placement).unwrap();
        assert!(!routing.via_locations.is_empty());
        assert_eq!(routing.vias, routing.via_locations.len());
        check_routing(&routing).unwrap();

        routing.via_locations.pop();
        routing.vias = routing.via_locations.len();
        assert!(check_routing(&routing).is_ok());
        let result = drc(&nl, &placement, &routing).unwrap();
        assert!(result.has_errors(), "{}", drc_report(&result));
        assert!(result.violations.iter().any(|v| v.code == "OPEN-001"));
    }

    #[test]
    fn partial_congested_routing_is_drc_diagnosable() {
        let (nl, placement) = congested_routing_fixture();
        let options = RouteOptions {
            tracks_per_channel: 1,
            allow_congestion: true,
            blocked_channels: Vec::new(),
        };
        let routing = route_with_options(&nl, &placement, &options).unwrap();
        assert_eq!(
            routing.congested_channels,
            vec![(Orientation::Horizontal, 0)]
        );
        check_routing(&routing).unwrap();
        let result = drc(&nl, &placement, &routing).unwrap();
        assert!(result.has_errors());
        assert!(result.violations.iter().any(|v| v.code == "OPEN-001"));
    }

    #[test]
    fn drc_accepts_generated_layout() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("drc_clean");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("XOR2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let placement = place(&nl).unwrap();
        let routing = route(&nl, &placement).unwrap();
        let result = drc(&nl, &placement, &routing).unwrap();
        assert!(!result.has_errors(), "{}", drc_report(&result));
    }

    #[test]
    fn drc_rejects_bad_track() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("drc_clean");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let placement = place(&nl).unwrap();
        let mut routing = route(&nl, &placement).unwrap();
        routing.segments[0].track = routing.tracks_per_channel + 10;
        let result = drc(&nl, &placement, &routing).unwrap();
        assert!(result.has_errors());
        assert!(result.violations.iter().any(|v| v.code == "ROUTE-001"));
    }

    #[test]
    fn drc_detects_open_net() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("drc_clean");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let placement = place(&nl).unwrap();
        let mut routing = route(&nl, &placement).unwrap();
        routing.segments.clear();
        let result = drc(&nl, &placement, &routing).unwrap();
        assert!(result.violations.iter().any(|v| v.code == "OPEN-001"));
    }

    #[test]
    fn drc_detects_unknown_routed_net() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("drc_clean");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let placement = place(&nl).unwrap();
        let mut routing = route(&nl, &placement).unwrap();
        routing.segments[0].net = nl.net_count() + 100;
        let result = drc(&nl, &placement, &routing).unwrap();
        assert!(result.violations.iter().any(|v| v.code == "ROUTE-002"));
    }
}
