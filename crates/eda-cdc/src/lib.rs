//! # eda-cdc — clock-domain crossing analysis.
//!
//! The analyzer infers clock roots from sequential cells, propagates clock
//! roots through simple clock buffers/inverters, and traces data paths between
//! launch/capture flops. Two-flop destination synchronizers are recognized.

use eda_core::netlist::{CellKind, NetId, Netlist};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockDomain {
    pub name: String,
    pub root_net: NetId,
    pub root_name: String,
    pub clock_nets: Vec<NetId>,
    pub sequential_cells: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdcPath {
    pub source_cell: Option<usize>,
    pub source_name: String,
    /// Deepest post-synthesis hierarchy instance owning the launch flop.
    pub source_hierarchy: Option<String>,
    pub source_clock: String,
    pub destination_cell: usize,
    pub destination_name: String,
    /// Deepest post-synthesis hierarchy instance owning the capture flop.
    pub destination_hierarchy: Option<String>,
    pub destination_clock: String,
    pub destination_pin: String,
    pub combinational_cells: Vec<usize>,
    /// Unique hierarchy instances crossed in launch-to-capture order.
    pub hierarchy_instances: Vec<String>,
    pub synchronized: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdcFinding {
    pub kind: String,
    pub severity: Severity,
    pub path: CdcPath,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct CdcResult {
    pub clocks: Vec<ClockDomain>,
    pub crossings: Vec<CdcPath>,
    pub findings: Vec<CdcFinding>,
}

/// CDC crossing counts grouped by stable hierarchy-instance path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CdcHierarchySummary {
    pub instance: Option<String>,
    pub source_crossings: usize,
    pub destination_crossings: usize,
    pub synchronized: usize,
    pub unsynchronized: usize,
}

impl CdcResult {
    pub fn has_errors(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == Severity::Error)
    }

    pub fn clean(&self) -> bool {
        self.findings
            .iter()
            .all(|finding| finding.severity == Severity::Info)
    }

    /// Aggregate crossings by the hierarchy instances launching and capturing them.
    pub fn hierarchy_summaries(&self) -> BTreeMap<Option<String>, CdcHierarchySummary> {
        let mut out = BTreeMap::new();
        for path in &self.crossings {
            let source_key = path.source_hierarchy.clone();
            let destination_key = path.destination_hierarchy.clone();
            let source = out
                .entry(source_key)
                .or_insert_with(|| CdcHierarchySummary {
                    instance: path.source_hierarchy.clone(),
                    ..Default::default()
                });
            source.source_crossings += 1;
            if path.synchronized {
                source.synchronized += 1;
            } else {
                source.unsynchronized += 1;
            }

            if path.destination_hierarchy != path.source_hierarchy {
                let destination =
                    out.entry(destination_key)
                        .or_insert_with(|| CdcHierarchySummary {
                            instance: path.destination_hierarchy.clone(),
                            ..Default::default()
                        });
                destination.destination_crossings += 1;
                if path.synchronized {
                    destination.synchronized += 1;
                } else {
                    destination.unsynchronized += 1;
                }
            } else {
                source.destination_crossings += 1;
            }
        }
        out
    }
}

#[derive(Clone, Copy, Debug)]
struct SequentialPins {
    clock: usize,
    data: usize,
}

#[derive(Clone, Debug)]
struct DomainInfo {
    name: String,
    root: NetId,
    clock_nets: BTreeSet<NetId>,
    cells: BTreeSet<usize>,
}

pub fn analyze(nl: &Netlist) -> Result<CdcResult, String> {
    let mut domains: BTreeMap<String, DomainInfo> = BTreeMap::new();
    let mut cell_domain: BTreeMap<usize, String> = BTreeMap::new();
    let mut sequential = BTreeMap::<usize, SequentialPins>::new();

    for ci in 0..nl.cell_count() {
        if !nl.cell_is_sequential(&nl.cell(ci).kind) {
            continue;
        }
        let pins = sequential_pins(nl, ci)?;
        sequential.insert(ci, pins);
        let clock_net = nl.cell(ci).inputs[pins.clock];
        let root = clock_root(nl, clock_net)?;
        let name = clock_name(nl, root);
        let domain = domains.entry(name.clone()).or_insert_with(|| DomainInfo {
            name: name.clone(),
            root,
            clock_nets: BTreeSet::new(),
            cells: BTreeSet::new(),
        });
        domain.clock_nets.insert(clock_net);
        domain.cells.insert(ci);
        cell_domain.insert(ci, name);
    }

    let clocks: Vec<ClockDomain> = domains
        .values()
        .map(|domain| ClockDomain {
            name: domain.name.clone(),
            root_net: domain.root,
            root_name: nl.net(domain.root).name.clone(),
            clock_nets: domain.clock_nets.iter().copied().collect(),
            sequential_cells: domain.cells.iter().copied().collect(),
        })
        .collect();

    if domains.len() <= 1 {
        return Ok(CdcResult {
            clocks,
            crossings: Vec::new(),
            findings: Vec::new(),
        });
    }

    let synchronizer_first_stages = find_synchronizer_first_stages(nl, &sequential);
    let mut crossings = Vec::new();

    for &source in sequential.keys() {
        let source_domain = cell_domain[&source].clone();
        let output = nl.cell(source).output;
        let starts = vec![(output, Vec::new())];
        let paths = trace_to_sequential(nl, starts)?;
        for (destination, input_index, comb_cells) in paths {
            if destination == source {
                continue;
            }
            let destination_domain = cell_domain[&destination].clone();
            if destination_domain == source_domain {
                continue;
            }
            let pins = sequential[&destination];
            if input_index != pins.data {
                // Asynchronous reset crossings are reported separately.
                continue;
            }
            let synchronized = synchronizer_first_stages.contains(&destination);
            let source_hierarchy = nl
                .cell_hierarchy_instance(source)
                .map(|instance| instance.path.clone());
            let destination_hierarchy = nl
                .cell_hierarchy_instance(destination)
                .map(|instance| instance.path.clone());
            let mut hierarchy_instances = Vec::new();
            for cell in std::iter::once(source)
                .chain(comb_cells.iter().copied())
                .chain(std::iter::once(destination))
            {
                if let Some(instance) = nl.cell_hierarchy_instance(cell) {
                    if !hierarchy_instances.contains(&instance.path) {
                        hierarchy_instances.push(instance.path.clone());
                    }
                }
            }
            crossings.push(CdcPath {
                source_cell: Some(source),
                source_name: nl.cell(source).name.clone(),
                source_hierarchy,
                source_clock: source_domain.clone(),
                destination_cell: destination,
                destination_name: nl.cell(destination).name.clone(),
                destination_hierarchy,
                destination_clock: destination_domain.clone(),
                destination_pin: data_pin_name(nl, destination)?,
                combinational_cells: comb_cells,
                hierarchy_instances,
                synchronized,
            });
        }
    }

    crossings.sort_by(|a, b| {
        (
            &a.source_clock,
            &a.source_name,
            &a.destination_clock,
            &a.destination_name,
        )
            .cmp(&(
                &b.source_clock,
                &b.source_name,
                &b.destination_clock,
                &b.destination_name,
            ))
    });

    let mut findings = Vec::new();
    for path in &crossings {
        let (kind, severity, message) = if path.synchronized {
            (
                "synchronized-crossing",
                Severity::Info,
                format!(
                    "{} -> {} is synchronized by a two-flop destination stage",
                    path.source_name, path.destination_name
                ),
            )
        } else {
            (
                "unsynchronized-crossing",
                Severity::Error,
                format!(
                    "{} in clock '{}' drives {}:{} in clock '{}' without a destination synchronizer",
                    path.source_name,
                    path.source_clock,
                    path.destination_name,
                    path.destination_pin,
                    path.destination_clock
                ),
            )
        };
        findings.push(CdcFinding {
            kind: kind.into(),
            severity,
            path: path.clone(),
            message,
        });
    }

    // Multiple unsynchronized destination captures from one source bus indicate
    // possible bit skew. Group indexed flop outputs by their vector base name.
    let mut by_source: BTreeMap<(String, String), Vec<&CdcPath>> = BTreeMap::new();
    for path in &crossings {
        let Some(source_cell) = path.source_cell else {
            continue;
        };
        let source_net = nl.cell(source_cell).output;
        let base = vector_base(&nl.net(source_net).name);
        by_source
            .entry((path.source_clock.clone(), base))
            .or_default()
            .push(path);
    }
    for paths in by_source.values() {
        let unsynchronized: Vec<_> = paths
            .iter()
            .filter(|path| !path.synchronized)
            .copied()
            .collect();
        let destinations: BTreeSet<_> = unsynchronized
            .iter()
            .map(|path| path.destination_cell)
            .collect();
        if destinations.len() > 1 {
            let first = unsynchronized[0];
            findings.push(CdcFinding {
                kind: "multi-bit-crossing".into(),
                severity: Severity::Error,
                path: first.clone(),
                message: format!(
                    "source bus '{}' fans out to {} unsynchronized destination flops; bits may skew",
                    first.source_name,
                    destinations.len()
                ),
            });
        }
    }

    Ok(CdcResult {
        clocks,
        crossings,
        findings,
    })
}

fn vector_base(name: &str) -> String {
    match name.rfind('[') {
        Some(position) => name[..position].to_string(),
        None => name.to_string(),
    }
}

fn sequential_pins(nl: &Netlist, ci: usize) -> Result<SequentialPins, String> {
    let cell = nl.cell(ci);
    match &cell.kind {
        CellKind::Dff { .. } | CellKind::DffR { .. } => Ok(SequentialPins { clock: 0, data: 1 }),
        CellKind::Lib(id) => {
            let lib = nl
                .library()
                .ok_or_else(|| format!("cell '{}' has no attached library", cell.name))?;
            let seq = lib
                .get(*id)
                .seq
                .clone()
                .ok_or_else(|| format!("cell '{}' is not sequential", cell.name))?;
            Ok(SequentialPins {
                clock: seq.clk_pin,
                data: 1,
            })
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
        let mut found_input_driver = false;
        for driver in drivers {
            let cell = nl.cell(driver);
            if nl.cell_is_sequential(&cell.kind) {
                return Ok(net);
            }
            if matches!(cell.kind, CellKind::Buf | CellKind::Not) {
                stack.extend(cell.inputs.iter().copied());
                found_input_driver = true;
            } else if let CellKind::Lib(id) = &cell.kind {
                let is_comb = nl
                    .library()
                    .map(|lib| lib.get(*id).seq.is_none())
                    .unwrap_or(false);
                if is_comb && cell.inputs.len() == 1 {
                    stack.push(cell.inputs[0]);
                    found_input_driver = true;
                }
            }
        }
        if !found_input_driver {
            return Ok(net);
        }
    }
    Ok(start)
}

fn clock_name(nl: &Netlist, root: NetId) -> String {
    let net = nl.net(root);
    if let Some(port) = nl.ports.iter().find(|port| port.bits.contains(&root)) {
        return port.name.clone();
    }
    net.name.clone()
}

fn data_pin_name(nl: &Netlist, ci: usize) -> Result<String, String> {
    let pins = sequential_pins(nl, ci)?;
    match &nl.cell(ci).kind {
        CellKind::Lib(id) => {
            let lib = nl
                .library()
                .ok_or_else(|| format!("cell '{}' has no library", nl.cell(ci).name))?;
            lib.get(*id)
                .inputs
                .get(pins.data)
                .cloned()
                .ok_or_else(|| format!("cell '{}' has no data pin", nl.cell(ci).name))
        }
        _ => Ok("D".into()),
    }
}

fn find_synchronizer_first_stages(
    nl: &Netlist,
    sequential: &BTreeMap<usize, SequentialPins>,
) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    for &first in sequential.keys() {
        let q = nl.cell(first).output;
        let sinks = nl.net(q).sinks.clone();
        for sink in sinks {
            if sink == first || !nl.cell_is_sequential(&nl.cell(sink).kind) {
                continue;
            }
            let Some(pins) = sequential.get(&sink) else {
                continue;
            };
            if nl.cell(sink).inputs.get(pins.data) == Some(&q) && nl.net(q).sinks.len() == 1 {
                out.insert(first);
            }
        }
    }
    out
}

fn trace_to_sequential(
    nl: &Netlist,
    starts: Vec<(NetId, Vec<usize>)>,
) -> Result<Vec<(usize, usize, Vec<usize>)>, String> {
    let mut out = Vec::new();
    let mut stack = starts;
    let mut visited = BTreeSet::new();
    while let Some((net, path)) = stack.pop() {
        if !visited.insert((net, path.clone())) {
            continue;
        }
        if path.len() > nl.cell_count() {
            return Err("CDC traversal exceeded netlist size".into());
        }
        for &sink in &nl.net(net).sinks {
            if nl.cell_is_sequential(&nl.cell(sink).kind) {
                for (input_index, &input) in nl.cell(sink).inputs.iter().enumerate() {
                    if input == net {
                        out.push((sink, input_index, path.clone()));
                    }
                }
                continue;
            }
            let mut next_path = path.clone();
            next_path.push(sink);
            stack.push((nl.cell(sink).output, next_path));
        }
    }
    out.sort_by_key(|(cell, pin, path)| (*cell, *pin, path.clone()));
    out.dedup();
    Ok(out)
}

/// Human-readable CDC report.
pub fn report(result: &CdcResult) -> String {
    let mut out = format!(
        "CDC report: {} clock domain(s), {} crossing(s), {} finding(s)\n",
        result.clocks.len(),
        result.crossings.len(),
        result.findings.len()
    );
    for clock in &result.clocks {
        out.push_str(&format!(
            "  CLOCK {} flops={} root={}\n",
            clock.name,
            clock.sequential_cells.len(),
            clock.root_name
        ));
    }
    for finding in &result.findings {
        let severity = match finding.severity {
            Severity::Info => "INFO",
            Severity::Warning => "WARN",
            Severity::Error => "ERROR",
        };
        out.push_str(&format!(
            "  {} [{}] {}\n",
            severity, finding.kind, finding.message
        ));
        let path = &finding.path;
        let source = path.source_hierarchy.as_deref().unwrap_or("(top)");
        let destination = path.destination_hierarchy.as_deref().unwrap_or("(top)");
        out.push_str(&format!(
            "    hierarchy: {} -> {} [{}]\n",
            source,
            destination,
            if path.hierarchy_instances.is_empty() {
                "(top)".to_string()
            } else {
                path.hierarchy_instances.join(" -> ")
            }
        ));
    }
    let hierarchy_summaries = result.hierarchy_summaries();
    if !hierarchy_summaries.is_empty() {
        out.push_str("  hierarchy distribution:\n");
        for summary in hierarchy_summaries.values() {
            out.push_str(&format!(
                "    '{}': source={}, destination={}, synchronized={}, unsynchronized={}\n",
                summary.instance.as_deref().unwrap_or("(top)"),
                summary.source_crossings,
                summary.destination_crossings,
                summary.synchronized,
                summary.unsynchronized
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::netlist::{Cell, Dir};

    fn add_flop(nl: &mut Netlist, name: &str, clock: NetId, data: NetId, output: NetId) {
        nl.add_cell(Cell {
            name: name.to_string(),
            kind: CellKind::Dff { posedge: true },
            inputs: vec![clock, data],
            output,
        });
    }

    fn add_ports(nl: &mut Netlist, names: &[&str]) {
        for name in names {
            let net = nl.net_by_name(name).unwrap_or_else(|| nl.add_net(*name));
            nl.add_port(*name, Dir::In, vec![net]);
        }
    }

    #[test]
    fn single_clock_domain_has_no_crossing() {
        let mut nl = Netlist::new("single_clock");
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let q = nl.add_net("q");
        let r = nl.add_net("r");
        add_flop(&mut nl, "q_reg", clk, d, q);
        add_flop(&mut nl, "r_reg", clk, q, r);
        add_ports(&mut nl, &["clk", "d"]);
        nl.add_port("q", Dir::Out, vec![q]);
        nl.add_port("r", Dir::Out, vec![r]);

        let result = analyze(&nl).unwrap();
        assert_eq!(result.clocks.len(), 1);
        assert!(result.crossings.is_empty());
        assert!(result.clean());
    }

    #[test]
    fn two_flop_synchronizer_is_recognized() {
        let mut nl = Netlist::new("synchronizer");
        let clk_a = nl.add_net("clk_a");
        let clk_b = nl.add_net("clk_b");
        let d = nl.add_net("d");
        let a_q = nl.add_net("a_q");
        let sync0 = nl.add_net("sync0");
        let sync1 = nl.add_net("sync1");
        add_flop(&mut nl, "a_q_reg", clk_a, d, a_q);
        add_flop(&mut nl, "sync0_reg", clk_b, a_q, sync0);
        add_flop(&mut nl, "sync1_reg", clk_b, sync0, sync1);
        add_ports(&mut nl, &["clk_a", "clk_b", "d"]);
        nl.add_port("a_q", Dir::Out, vec![a_q]);
        nl.add_port("sync0", Dir::Out, vec![sync0]);
        nl.add_port("sync1", Dir::Out, vec![sync1]);

        let result = analyze(&nl).unwrap();
        assert_eq!(result.clocks.len(), 2);
        assert_eq!(result.crossings.len(), 1);
        assert!(result.crossings[0].synchronized);
        assert!(result.clean());
    }

    #[test]
    fn unsynchronized_crossing_is_an_error() {
        let mut nl = Netlist::new("unsynchronized");
        let clk_a = nl.add_net("clk_a");
        let clk_b = nl.add_net("clk_b");
        let d = nl.add_net("d");
        let a_q = nl.add_net("a_q");
        let b_q = nl.add_net("b_q");
        add_flop(&mut nl, "a_q_reg", clk_a, d, a_q);
        add_flop(&mut nl, "b_q_reg", clk_b, a_q, b_q);
        add_ports(&mut nl, &["clk_a", "clk_b", "d"]);

        let result = analyze(&nl).unwrap();
        assert!(result.has_errors());
        assert!(result
            .findings
            .iter()
            .any(|finding| finding.kind == "unsynchronized-crossing"));
        assert!(report(&result).contains("unsynchronized-crossing"));
    }

    #[test]
    fn clock_inverter_preserves_one_domain() {
        let mut nl = Netlist::new("clock_inverter");
        let clk = nl.add_net("clk");
        let not_clk = nl.add_net("not_clk");
        let d = nl.add_net("d");
        let q = nl.add_net("q");
        let r = nl.add_net("r");
        nl.add_cell(Cell {
            name: "clk_inv".into(),
            kind: CellKind::Not,
            inputs: vec![clk],
            output: not_clk,
        });
        add_flop(&mut nl, "q_reg", clk, d, q);
        add_flop(&mut nl, "r_reg", not_clk, q, r);
        add_ports(&mut nl, &["clk", "d"]);
        nl.add_port("q", Dir::Out, vec![q]);
        nl.add_port("r", Dir::Out, vec![r]);

        let result = analyze(&nl).unwrap();
        assert_eq!(result.clocks.len(), 1);
        assert!(result.crossings.is_empty());
        assert!(result.clean());
    }
}
