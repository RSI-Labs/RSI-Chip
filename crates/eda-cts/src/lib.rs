//! # eda-cts — balanced clock-tree synthesis.
//!
//! Clock domains are inferred from sequential cells, clock buffers/inverters
//! are traced to their root, and each domain receives a balanced buffer tree
//! with bounded fanout and equal logical depth.

use eda_core::netlist::{Cell, CellKind, NetId, Netlist};
use eda_pnr::Placement;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockSink {
    pub cell: usize,
    pub clock_pin: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClockTree {
    pub domain: String,
    pub root: NetId,
    pub sinks: Vec<ClockSink>,
    pub buffer_cells: Vec<usize>,
    pub levels: usize,
    pub branch_factor: usize,
    pub max_fanout: usize,
    pub estimated_insertion_delay_ns: f64,
    pub estimated_logical_skew_ns: f64,
    pub estimated_wire_delay_ns: f64,
    pub estimated_physical_skew_ns: f64,
    pub estimated_skew_ns: f64,
    pub placement_aware: bool,
}

#[derive(Clone, Debug)]
pub struct ClockTreeResult {
    pub netlist: Netlist,
    pub trees: Vec<ClockTree>,
    /// Legal placement for the post-CTS netlist. Present only for
    /// placement-aware CTS.
    pub physical_placement: Option<Placement>,
    pub buffers_inserted: usize,
    pub original_cells: usize,
    pub original_nets: usize,
}

/// Result of searching placement-aware CTS branch factors for a skew target.
#[derive(Clone, Debug)]
pub struct TimingDrivenCtsResult {
    pub cts: ClockTreeResult,
    pub selected_branch_factor: usize,
    pub searched_branch_factors: Vec<usize>,
    pub best_estimated_skew_ns: f64,
    pub target_met: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct CtsOptions {
    pub branch_factor: usize,
    /// Conservative RC delay per placement site/row unit.
    pub wire_delay_ns_per_unit: f64,
}

/// Options for skew-targeted, placement-aware clock-tree search.
#[derive(Clone, Copy, Debug)]
pub struct TimingDrivenCtsOptions {
    pub minimum_branch_factor: usize,
    pub maximum_branch_factor: usize,
    pub wire_delay_ns_per_unit: f64,
    /// Desired upper bound on estimated physical skew in nanoseconds.
    /// Once candidates meet this bound, fewer buffer levels are preferred.
    pub target_skew_ns: f64,
}

impl Default for TimingDrivenCtsOptions {
    fn default() -> Self {
        Self {
            minimum_branch_factor: 2,
            maximum_branch_factor: 16,
            wire_delay_ns_per_unit: 0.01,
            target_skew_ns: 0.05,
        }
    }
}

impl TimingDrivenCtsOptions {
    pub fn new(
        minimum_branch_factor: usize,
        maximum_branch_factor: usize,
        wire_delay_ns_per_unit: f64,
        target_skew_ns: f64,
    ) -> Result<Self, String> {
        let options = Self {
            minimum_branch_factor,
            maximum_branch_factor,
            wire_delay_ns_per_unit,
            target_skew_ns,
        };
        options.validate()?;
        Ok(options)
    }

    fn validate(&self) -> Result<(), String> {
        if !(2..=16).contains(&self.minimum_branch_factor) {
            return Err("timing-driven CTS minimum branch factor must be between 2 and 16".into());
        }
        if self.maximum_branch_factor < self.minimum_branch_factor
            || self.maximum_branch_factor > 16
        {
            return Err(
                "timing-driven CTS maximum branch factor must be between the minimum and 16".into(),
            );
        }
        if !self.wire_delay_ns_per_unit.is_finite() || self.wire_delay_ns_per_unit < 0.0 {
            return Err(
                "timing-driven CTS wire delay per unit must be finite and nonnegative".into(),
            );
        }
        if !self.target_skew_ns.is_finite() || self.target_skew_ns < 0.0 {
            return Err("timing-driven CTS skew target must be finite and nonnegative".into());
        }
        Ok(())
    }
}

impl Default for CtsOptions {
    fn default() -> Self {
        CtsOptions {
            branch_factor: 4,
            wire_delay_ns_per_unit: 0.01,
        }
    }
}

impl CtsOptions {
    pub fn new(branch_factor: usize, wire_delay_ns_per_unit: f64) -> Result<Self, String> {
        let options = Self {
            branch_factor,
            wire_delay_ns_per_unit,
        };
        options.validate()?;
        Ok(options)
    }
}

pub fn synthesize(nl: &Netlist, options: &CtsOptions) -> Result<ClockTreeResult, String> {
    synthesize_impl(nl, None, options)
}

/// Build clock trees while using physical sink locations to cluster leaf
/// branches and estimate wire-induced skew.
pub fn synthesize_with_placement(
    nl: &Netlist,
    placement: &Placement,
    options: &CtsOptions,
) -> Result<ClockTreeResult, String> {
    synthesize_impl(nl, Some(placement), options)
}

/// Search placement-aware clock-tree branch factors against a skew target.
///
/// Candidates that meet the target are ordered by fewer inserted buffers,
/// lower insertion delay, and finally lower estimated skew. If no candidate
/// meets the target, the lowest-skew result wins. The search therefore avoids
/// paying extra tree depth for insignificant skew improvements.
pub fn synthesize_timing_driven_with_placement(
    nl: &Netlist,
    placement: &Placement,
    options: &TimingDrivenCtsOptions,
) -> Result<TimingDrivenCtsResult, String> {
    options.validate()?;
    let mut best: Option<(usize, ClockTreeResult)> = None;
    let mut searched = Vec::new();
    for branch in options.minimum_branch_factor..=options.maximum_branch_factor {
        searched.push(branch);
        let candidate = synthesize_with_placement(
            nl,
            placement,
            &CtsOptions {
                branch_factor: branch,
                wire_delay_ns_per_unit: options.wire_delay_ns_per_unit,
            },
        )?;
        let replace = match &best {
            None => true,
            Some((_, current)) => {
                timing_driven_cts_better(&candidate, current, options.target_skew_ns)
            }
        };
        if replace {
            best = Some((branch, candidate));
        }
    }
    let (selected_branch_factor, cts) =
        best.ok_or_else(|| "timing-driven CTS searched no branch factors".to_string())?;
    let best_estimated_skew_ns = cts
        .trees
        .iter()
        .map(|tree| tree.estimated_skew_ns)
        .fold(f64::NEG_INFINITY, f64::max);
    Ok(TimingDrivenCtsResult {
        target_met: best_estimated_skew_ns <= options.target_skew_ns,
        best_estimated_skew_ns,
        selected_branch_factor,
        searched_branch_factors: searched,
        cts,
    })
}

fn timing_driven_cts_better(
    candidate: &ClockTreeResult,
    current: &ClockTreeResult,
    target_skew_ns: f64,
) -> bool {
    let candidate_skew = worst_skew(candidate);
    let current_skew = worst_skew(current);
    let candidate_meets = candidate_skew <= target_skew_ns;
    let current_meets = current_skew <= target_skew_ns;
    if candidate_meets != current_meets {
        return candidate_meets;
    }
    if candidate.buffers_inserted != current.buffers_inserted {
        return candidate.buffers_inserted < current.buffers_inserted;
    }
    if candidate_skew != current_skew {
        return candidate_skew < current_skew;
    }
    candidate
        .trees
        .iter()
        .map(|tree| tree.estimated_insertion_delay_ns)
        .sum::<f64>()
        < current
            .trees
            .iter()
            .map(|tree| tree.estimated_insertion_delay_ns)
            .sum::<f64>()
}

fn worst_skew(result: &ClockTreeResult) -> f64 {
    result
        .trees
        .iter()
        .map(|tree| tree.estimated_skew_ns)
        .fold(f64::NEG_INFINITY, f64::max)
}

fn synthesize_impl(
    nl: &Netlist,
    placement: Option<&Placement>,
    options: &CtsOptions,
) -> Result<ClockTreeResult, String> {
    options.validate()?;
    if let Some(placement) = placement {
        if placement.cell_pos.len() != nl.cell_count() {
            return Err(format!(
                "CTS placement has {} cells but netlist has {}",
                placement.cell_pos.len(),
                nl.cell_count()
            ));
        }
        eda_pnr::check_legal(nl, placement)?;
    }
    let mut out = nl.clone();
    let mut domains: BTreeMap<NetId, (String, Vec<ClockSink>)> = BTreeMap::new();
    for ci in 0..out.cell_count() {
        if !out.cell_is_sequential(&out.cell(ci).kind) {
            continue;
        }
        let clock_pin = clock_pin(&out, ci)?;
        let clock_net = out.cell(ci).inputs[clock_pin];
        let root = clock_root(&out, clock_net)?;
        let name = clock_name(&out, root);
        domains
            .entry(root)
            .or_insert_with(|| (name, Vec::new()))
            .1
            .push(ClockSink {
                cell: ci,
                clock_pin,
            });
    }
    if domains.is_empty() {
        return Err("CTS requires at least one sequential cell".into());
    }

    let original_cells = out.cell_count();
    let original_nets = out.net_count();
    let mut trees = Vec::new();
    for (root, (domain, mut sinks)) in domains {
        let placement_aware = placement.is_some();
        if let Some(placement) = placement {
            sinks.sort_unstable_by_key(|sink| {
                let pos = &placement.cell_pos[sink.cell];
                (pos.row, placement.cell_width[sink.cell] + pos.site)
            });
        } else {
            sinks.sort_unstable_by_key(|sink| sink.cell);
        }
        let sink_count = sinks.len();
        let levels = required_levels(sink_count, options.branch_factor).max(1);
        let prefix = format!("cts_{}_l{}", sanitize(&domain), levels);
        let mut buffer_cells = Vec::new();
        let mut fanout = 0usize;
        let buffer_kind = buffer_kind(&out)?;
        let mut sink_nets = BTreeMap::new();
        let leaf = build_level(
            &mut out,
            root,
            &sinks,
            levels,
            options.branch_factor,
            &prefix,
            &buffer_kind,
            &mut buffer_cells,
            &mut fanout,
            &mut sink_nets,
        )?;
        let _ = leaf;
        for (index, sink) in sinks.iter().enumerate() {
            let target = sink_nets[&index];
            let mut cell = out.cell(sink.cell).clone();
            cell.inputs[sink.clock_pin] = target;
            out.replace_cell(sink.cell, cell);
        }
        let delay = buffer_delay(&out, &buffer_kind);
        let (wire_delay, physical_skew) = physical_metrics(
            nl,
            placement,
            root,
            &sinks,
            levels,
            options.branch_factor,
            options.wire_delay_ns_per_unit,
        );
        trees.push(ClockTree {
            domain,
            root,
            sinks,
            buffer_cells,
            levels,
            branch_factor: options.branch_factor,
            max_fanout: fanout,
            estimated_logical_skew_ns: 0.0,
            estimated_insertion_delay_ns: levels as f64 * delay + wire_delay,
            estimated_wire_delay_ns: wire_delay,
            estimated_physical_skew_ns: physical_skew,
            estimated_skew_ns: physical_skew,
            placement_aware,
        });
    }

    validate(&out, &trees)?;
    let physical_placement = placement
        .map(|original| eda_pnr::extend_placement(&out, original))
        .transpose()?;
    if let Some(placement) = &physical_placement {
        eda_pnr::check_legal(&out, placement)?;
    }
    let buffers_inserted = out.cell_count() - original_cells;
    Ok(ClockTreeResult {
        netlist: out,
        trees,
        physical_placement,
        buffers_inserted,
        original_cells,
        original_nets,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_level(
    out: &mut Netlist,
    input: NetId,
    sinks: &[ClockSink],
    level: usize,
    branch: usize,
    prefix: &str,
    buffer_kind: &CellKind,
    buffers: &mut Vec<usize>,
    max_fanout: &mut usize,
    sink_nets: &mut BTreeMap<usize, NetId>,
) -> Result<NetId, String> {
    if level == 0 || sinks.is_empty() {
        for (index, _) in sinks.iter().enumerate() {
            sink_nets.insert(index, input);
        }
        return Ok(input);
    }
    let net_name = format!("{}_b{}", prefix, buffers.len());
    let output = out.add_net(net_name);
    let cell_name = format!("{}_buf{}", prefix, buffers.len());
    let ci = out.add_cell(Cell {
        name: cell_name,
        kind: buffer_kind.clone(),
        inputs: vec![input],
        output,
    });
    buffers.push(ci);
    if level == 1 {
        *max_fanout = (*max_fanout).max(sinks.len());
        for (index, _) in sinks.iter().enumerate() {
            sink_nets.insert(index, output);
        }
        return Ok(output);
    }
    let groups = divide(sinks, branch);
    *max_fanout = (*max_fanout).max(groups.len());
    for (group_index, group) in groups.iter().enumerate() {
        if group.is_empty() {
            continue;
        }
        let child_prefix = format!("{}_g{}", prefix, group_index);
        let base = sink_nets.len();
        let mut child_map = BTreeMap::new();
        let child = build_level(
            out,
            output,
            group,
            level - 1,
            branch,
            &child_prefix,
            buffer_kind,
            buffers,
            max_fanout,
            &mut child_map,
        )?;
        let _ = child;
        for (local, global) in group.iter().map(|sink| sink.cell).enumerate() {
            let _ = global;
            // Preserve global sink indices supplied by the caller.
            if let Some((original_index, _)) = sinks
                .iter()
                .enumerate()
                .find(|(_, candidate)| candidate.cell == group[local].cell)
            {
                sink_nets.insert(original_index, child_map[&local]);
            }
        }
        let _ = base;
    }
    Ok(output)
}

fn divide(sinks: &[ClockSink], groups: usize) -> Vec<Vec<ClockSink>> {
    let groups = groups.max(1);
    let size = sinks.len().div_ceil(groups);
    let mut out = Vec::new();
    let mut start = 0;
    while start < sinks.len() {
        let end = (start + size).min(sinks.len());
        out.push(sinks[start..end].to_vec());
        start = end;
    }
    while out.len() < groups {
        out.push(Vec::new());
    }
    out
}

fn required_levels(sinks: usize, branch: usize) -> usize {
    let mut levels = 0usize;
    let mut capacity = 1usize;
    while capacity < sinks {
        capacity = capacity.saturating_mul(branch);
        levels += 1;
    }
    levels
}

impl CtsOptions {
    fn validate(&self) -> Result<(), String> {
        if !(2..=16).contains(&self.branch_factor) {
            return Err("CTS branch factor must be between 2 and 16".into());
        }
        if !self.wire_delay_ns_per_unit.is_finite() || self.wire_delay_ns_per_unit < 0.0 {
            return Err("CTS wire delay per unit must be finite and nonnegative".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Location {
    x: f64,
    y: f64,
}

fn cell_location(_nl: &Netlist, placement: &Placement, ci: usize) -> Location {
    let pos = &placement.cell_pos[ci];
    Location {
        x: pos.site as f64 + placement.cell_width[ci] as f64 / 2.0,
        y: pos.row as f64,
    }
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_unstable_by(|a, b| a.total_cmp(b));
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

fn root_location(
    nl: &Netlist,
    placement: &Placement,
    root: NetId,
    sinks: &[ClockSink],
) -> Location {
    if let Some(&driver) = nl.net(root).drivers.first() {
        return cell_location(nl, placement, driver);
    }
    let mut xs: Vec<f64> = sinks
        .iter()
        .map(|sink| cell_location(nl, placement, sink.cell).x)
        .collect();
    Location {
        x: median(&mut xs),
        y: 0.0,
    }
}

fn leaf_groups(sinks: &[ClockSink], levels: usize, branch: usize) -> Vec<Vec<ClockSink>> {
    if levels <= 1 {
        return vec![sinks.to_vec()];
    }
    divide(sinks, branch)
        .into_iter()
        .flat_map(|group| leaf_groups(&group, levels - 1, branch))
        .filter(|group| !group.is_empty())
        .collect()
}

fn manhattan(a: Location, b: Location) -> f64 {
    (a.x - b.x).abs() + (a.y - b.y).abs()
}

fn physical_metrics(
    nl: &Netlist,
    placement: Option<&Placement>,
    root: NetId,
    sinks: &[ClockSink],
    levels: usize,
    branch: usize,
    delay_per_unit: f64,
) -> (f64, f64) {
    let Some(placement) = placement else {
        return (0.0, 0.0);
    };
    let root = root_location(nl, placement, root, sinks);
    let mut delays = Vec::with_capacity(sinks.len());
    for group in leaf_groups(sinks, levels, branch) {
        let mut xs: Vec<f64> = group
            .iter()
            .map(|sink| cell_location(nl, placement, sink.cell).x)
            .collect();
        let mut ys: Vec<f64> = group
            .iter()
            .map(|sink| cell_location(nl, placement, sink.cell).y)
            .collect();
        let center = Location {
            x: median(&mut xs),
            y: median(&mut ys),
        };
        for sink in &group {
            let sink_location = cell_location(nl, placement, sink.cell);
            delays.push(
                (manhattan(root, center) + manhattan(center, sink_location)) * delay_per_unit,
            );
        }
    }
    let min = delays.iter().copied().fold(f64::INFINITY, f64::min);
    let max = delays.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (max, (max - min).max(0.0))
}

fn clock_pin(nl: &Netlist, ci: usize) -> Result<usize, String> {
    let cell = nl.cell(ci);
    match &cell.kind {
        CellKind::Dff { .. } | CellKind::DffR { .. } => Ok(0),
        CellKind::Lib(id) => {
            let lib = nl
                .library()
                .ok_or_else(|| format!("cell '{}' has no attached library", cell.name))?;
            let seq = lib
                .get(*id)
                .seq
                .clone()
                .ok_or_else(|| format!("cell '{}' is not sequential", cell.name))?;
            Ok(seq.clk_pin)
        }
        _ => Err(format!("cell '{}' is not sequential", cell.name)),
    }
}

fn clock_root(nl: &Netlist, start: NetId) -> Result<NetId, String> {
    let mut stack = vec![start];
    let mut seen = BTreeSet::new();
    while let Some(net) = stack.pop() {
        if !seen.insert(net) {
            continue;
        }
        let drivers = nl.net(net).drivers.clone();
        if drivers.is_empty() {
            return Ok(net);
        }
        let mut traversed = false;
        for driver in drivers {
            let cell = nl.cell(driver);
            if nl.cell_is_sequential(&cell.kind) {
                return Ok(net);
            }
            let single_input_buffer = match &cell.kind {
                CellKind::Buf | CellKind::Not => true,
                CellKind::Lib(id) => nl
                    .library()
                    .map(|lib| {
                        let def = lib.get(*id);
                        def.seq.is_none()
                            && def.inputs.len() == 1
                            && def
                                .func
                                .eval(&[eda_core::logic::LogicValue::One])
                                .as_bool()
                                .is_some()
                    })
                    .unwrap_or(false),
                _ => false,
            };
            if single_input_buffer && cell.inputs.len() == 1 {
                stack.push(cell.inputs[0]);
                traversed = true;
            }
        }
        if !traversed {
            return Ok(net);
        }
    }
    Ok(start)
}

fn clock_name(nl: &Netlist, root: NetId) -> String {
    if let Some(port) = nl.ports.iter().find(|port| port.bits.contains(&root)) {
        return port.name.clone();
    }
    nl.net(root).name.clone()
}

fn buffer_kind(nl: &Netlist) -> Result<CellKind, String> {
    let Some(lib) = nl.library() else {
        return Ok(CellKind::Buf);
    };
    let id = lib
        .find("BUF")
        .ok_or_else(|| "library has no BUF cell for CTS".to_string())?;
    Ok(CellKind::Lib(id))
}

fn buffer_delay(nl: &Netlist, kind: &CellKind) -> f64 {
    if let CellKind::Lib(id) = kind {
        if let Some(lib) = nl.library() {
            return lib
                .get(*id)
                .arcs
                .iter()
                .map(|(_, delay)| *delay)
                .fold(0.0, f64::max);
        }
    }
    0.5
}

fn validate(nl: &Netlist, trees: &[ClockTree]) -> Result<(), String> {
    nl.topo_cells()
        .ok_or_else(|| "CTS netlist contains a combinational loop".to_string())?;
    for tree in trees {
        if tree.sinks.is_empty() {
            return Err(format!("clock domain '{}' has no sinks", tree.domain));
        }
        if tree.max_fanout > tree.branch_factor {
            return Err(format!(
                "clock domain '{}' fanout {} exceeds branch factor {}",
                tree.domain, tree.max_fanout, tree.branch_factor
            ));
        }
        for buffer in &tree.buffer_cells {
            let cell = nl.cell(*buffer);
            if nl.cell_is_sequential(&cell.kind) {
                return Err(format!("CTS buffer '{}' is sequential", cell.name));
            }
            if nl.net(cell.output).drivers.len() != 1 {
                return Err(format!(
                    "CTS buffer '{}' output has multiple drivers",
                    cell.name
                ));
            }
        }
        for sink in &tree.sinks {
            let target = nl.cell(sink.cell).inputs[sink.clock_pin];
            if target == tree.root && !tree.buffer_cells.is_empty() {
                return Err(format!(
                    "clock sink '{}' was not connected to CTS tree",
                    nl.cell(sink.cell).name
                ));
            }
        }
    }
    Ok(())
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn report(result: &ClockTreeResult) -> String {
    let mut out = format!(
        "CTS report: {} clock domain(s), {} buffer(s) inserted\n",
        result.trees.len(),
        result.buffers_inserted
    );
    if let Some(placement) = &result.physical_placement {
        out.push_str(&format!("  Physical placement: {} rows\n", placement.rows));
    }
    for tree in &result.trees {
        out.push_str(&format!(
            "  CLOCK {} sinks={} levels={} branch={} max_fanout={} insertion={:.3}ns wire={:.3}ns skew={:.3}ns physical_skew={:.3}ns placement={}\n",
            tree.domain,
            tree.sinks.len(),
            tree.levels,
            tree.branch_factor,
            tree.max_fanout,
            tree.estimated_insertion_delay_ns,
            tree.estimated_wire_delay_ns,
            tree.estimated_skew_ns,
            tree.estimated_physical_skew_ns,
            if tree.placement_aware { "aware" } else { "logical" }
        ));
    }
    out
}

/// Render a timing-driven CTS search result and its selected implementation.
pub fn timing_driven_report(result: &TimingDrivenCtsResult) -> String {
    let mut out = format!(
        "Timing-driven CTS search: branch factors {}..{}, selected {}, worst skew {:.6} ns, target {}\n",
        result.searched_branch_factors.first().copied().unwrap_or(0),
        result.searched_branch_factors.last().copied().unwrap_or(0),
        result.selected_branch_factor,
        result.best_estimated_skew_ns,
        if result.target_met { "MET" } else { "MISSED" }
    );
    out.push_str(&report(&result.cts));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_branch_factor() {
        let design = eda_hdl::compile(
            "module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule",
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
        assert!(synthesize(
            &nl,
            &CtsOptions {
                branch_factor: 1,
                wire_delay_ns_per_unit: 0.01
            }
        )
        .is_err());
    }

    #[test]
    fn timing_driven_search_selects_valid_branch_factor() {
        let design = eda_hdl::compile(
            "module m(input clk, d, output reg q);
                always @(posedge clk) q <= d;
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
        let placement = eda_pnr::place(&nl).unwrap();
        let result = synthesize_timing_driven_with_placement(
            &nl,
            &placement,
            &TimingDrivenCtsOptions::default(),
        )
        .unwrap();
        assert_eq!(result.searched_branch_factors, (2..=16).collect::<Vec<_>>());
        assert_eq!(
            result.selected_branch_factor,
            result.cts.trees[0].branch_factor
        );
        assert!(result.target_met);
        assert!(timing_driven_report(&result).contains("target MET"));
    }
}
