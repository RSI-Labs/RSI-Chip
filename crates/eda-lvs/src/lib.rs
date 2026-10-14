//! # eda-lvs — layout-versus-schematic structural checks.
//!
//! LVS compares two flat netlists as labelled bipartite graphs. Reference and
//! layout instance/net names are deliberately ignored; devices are matched by
//! stable type, pin-role connectivity, and primary-port labels. The module
//! also reportsERC-style multiple-driver shorts and undriven/unloaded opens.

use eda_core::netlist::{CellKind, Dir, NetId, Netlist};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LvsCellMatch {
    pub reference_cell: String,
    pub layout_cell: String,
    pub device_type: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LvsNetMatch {
    pub reference_net: String,
    pub layout_net: String,
}

#[derive(Clone, Debug)]
pub struct LvsResult {
    pub matches: bool,
    pub notes: Vec<String>,
    pub cell_matches: Vec<LvsCellMatch>,
    pub net_matches: Vec<LvsNetMatch>,
    pub shorted_nets: Vec<String>,
    pub open_nets: Vec<String>,
}

pub fn check_lvs(reference: &Netlist, layout: &Netlist) -> Result<LvsResult, String> {
    let mut notes = Vec::new();
    let mut result = LvsResult {
        matches: true,
        notes: Vec::new(),
        cell_matches: Vec::new(),
        net_matches: Vec::new(),
        shorted_nets: Vec::new(),
        open_nets: Vec::new(),
    };

    if !ports_compatible(reference, layout, &mut notes)? {
        result.matches = false;
        result.notes = notes;
        return Ok(result);
    }

    let mut reference_types = device_type_counts(reference)?;
    let layout_types = device_type_counts(layout)?;
    let mut type_mismatches = Vec::new();
    let mut keys: BTreeSet<String> = reference_types.keys().cloned().collect();
    keys.extend(layout_types.keys().cloned());
    for key in keys {
        let a = reference_types.remove(&key).unwrap_or(0);
        let b = layout_types.get(&key).copied().unwrap_or(0);
        if a != b {
            type_mismatches.push(format!("{}: reference {} vs layout {}", key, a, b));
        }
    }
    if !type_mismatches.is_empty() {
        notes.push(format!(
            "device count mismatch: {}",
            type_mismatches.join("; ")
        ));
        result.matches = false;
    }

    result.shorted_nets = find_shorts(reference, "reference")?;
    let layout_shorts = find_shorts(layout, "layout")?;
    result.shorted_nets.extend(layout_shorts);
    if !result.shorted_nets.is_empty() {
        result.matches = false;
        notes.push(format!(
            "{} net(s) have multiple drivers",
            result.shorted_nets.len()
        ));
    }

    result.open_nets = find_opens(reference, "reference")?;
    let layout_opens = find_opens(layout, "layout")?;
    result.open_nets.extend(layout_opens);
    if !result.open_nets.is_empty() {
        result.matches = false;
        notes.push(format!("{} net(s) are open", result.open_nets.len()));
    }

    let reference_sigs = refined_signatures(reference)?;
    let layout_sigs = refined_signatures(layout)?;
    if signature_counts(&reference_sigs.cells) != signature_counts(&layout_sigs.cells)
        || signature_counts(&reference_sigs.nets) != signature_counts(&layout_sigs.nets)
    {
        result.matches = false;
        notes.push("structural connectivity signatures differ".into());
        let reference_cell_sigs: BTreeSet<_> = reference_sigs.cells.values().cloned().collect();
        let layout_cell_sigs: BTreeSet<_> = layout_sigs.cells.values().cloned().collect();
        for sig in layout_cell_sigs.difference(&reference_cell_sigs).take(8) {
            notes.push(format!("layout-only cell signature {}", sig));
        }
        for sig in reference_cell_sigs.difference(&layout_cell_sigs).take(8) {
            notes.push(format!("reference-only cell signature {}", sig));
        }
    }

    result.cell_matches = map_cells(reference, layout, &reference_sigs.cells, &layout_sigs.cells);
    result.net_matches = map_nets(reference, layout, &reference_sigs.nets, &layout_sigs.nets);
    result.notes = notes;
    Ok(result)
}

/// Human-readable LVS/ERC summary.
pub fn report(result: &LvsResult) -> String {
    let mut out = format!(
        "LVS report: {}\n  matched devices : {}\n  matched nets    : {}\n  shorts          : {}\n  opens           : {}\n",
        if result.matches { "MATCH" } else { "MISMATCH" },
        result.cell_matches.len(),
        result.net_matches.len(),
        result.shorted_nets.len(),
        result.open_nets.len()
    );
    for note in &result.notes {
        out.push_str(&format!("  note: {note}\n"));
    }
    for short in result.shorted_nets.iter().take(20) {
        out.push_str(&format!("  short: {short}\n"));
    }
    for open in result.open_nets.iter().take(20) {
        out.push_str(&format!("  open: {open}\n"));
    }
    out
}

fn ports_compatible(
    reference: &Netlist,
    layout: &Netlist,
    notes: &mut Vec<String>,
) -> Result<bool, String> {
    if reference.ports.len() != layout.ports.len() {
        notes.push(format!(
            "port count differs: reference {} vs layout {}",
            reference.ports.len(),
            layout.ports.len()
        ));
        return Ok(false);
    }
    let mut layout_ports: HashMap<&str, &eda_core::netlist::Port> = layout
        .ports
        .iter()
        .map(|port| (port.name.as_str(), port))
        .collect();
    for port in &reference.ports {
        let Some(other) = layout_ports.remove(port.name.as_str()) else {
            notes.push(format!("layout has no port '{}'", port.name));
            return Ok(false);
        };
        if port.dir != other.dir {
            notes.push(format!(
                "port '{}' direction differs: {:?} vs {:?}",
                port.name, port.dir, other.dir
            ));
            return Ok(false);
        }
        if port.bits.len() != other.bits.len() {
            notes.push(format!(
                "port '{}' width differs: {} vs {}",
                port.name,
                port.bits.len(),
                other.bits.len()
            ));
            return Ok(false);
        }
    }
    Ok(true)
}

fn device_type_counts(nl: &Netlist) -> Result<BTreeMap<String, usize>, String> {
    let mut out = BTreeMap::new();
    for ci in 0..nl.cell_count() {
        *out.entry(device_type(nl, ci)?).or_insert(0) += 1;
    }
    Ok(out)
}

fn device_type(nl: &Netlist, ci: usize) -> Result<String, String> {
    let cell = nl.cell(ci);
    Ok(match &cell.kind {
        CellKind::Lib(id) => {
            let lib = nl
                .library()
                .ok_or_else(|| format!("cell '{}' has no attached library", cell.name))?;
            let def = lib.get(*id);
            if def.seq.is_some() {
                format!("LIB:{}:SEQ", def.name)
            } else {
                format!("LIB:{}:COMB", def.name)
            }
        }
        CellKind::Const(value) => format!("CONST:{value:?}"),
        CellKind::Dff { posedge } => format!("DFF:POSEDGE:{}", *posedge),
        CellKind::DffR {
            posedge,
            reset_high,
        } => format!("DFFR:POSEDGE:{}:RESET_HIGH:{}", *posedge, *reset_high),
        kind => format!("{kind:?}:IN{}", cell.inputs.len()),
    })
}

fn find_shorts(nl: &Netlist, side: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for ni in 0..nl.net_count() {
        let net = nl.net(ni);
        if net.drivers.len() > 1 {
            out.push(format!(
                "{}.{} has {} drivers",
                side,
                net.name,
                net.drivers.len()
            ));
        }
    }
    Ok(out)
}

fn find_opens(nl: &Netlist, side: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let port_nets: HashMap<NetId, (String, Dir)> = nl
        .ports
        .iter()
        .flat_map(|port| {
            port.bits
                .iter()
                .enumerate()
                .map(move |(bit, &net)| (net, (format!("{}[{}]", port.name, bit), port.dir)))
        })
        .collect();
    for ni in 0..nl.net_count() {
        let net = nl.net(ni);
        let label = match port_nets.get(&ni) {
            Some((name, _)) => name.clone(),
            None => net.name.clone(),
        };
        if let Some((_, Dir::Out)) = port_nets.get(&ni) {
            if net.drivers.is_empty() {
                out.push(format!("{}.{} output port is undriven", side, label));
            }
        }
        if let Some((_, Dir::In)) = port_nets.get(&ni) {
            if net.sinks.is_empty() {
                out.push(format!("{}.{} input port is unloaded", side, label));
            }
        }
        if !port_nets.contains_key(&ni) && (net.drivers.is_empty() || net.sinks.is_empty()) {
            out.push(format!("{}.{} internal net is dangling", side, label));
        }
    }
    Ok(out)
}

#[derive(Debug)]
struct Signatures {
    cells: HashMap<usize, String>,
    nets: HashMap<usize, String>,
}

fn refined_signatures(nl: &Netlist) -> Result<Signatures, String> {
    let mut cells = HashMap::new();
    for ci in 0..nl.cell_count() {
        cells.insert(ci, digest(&[&device_type(nl, ci)?]));
    }
    let mut nets = HashMap::new();
    let mut port_labels: HashMap<NetId, Vec<String>> = HashMap::new();
    for port in &nl.ports {
        for (bit, &net) in port.bits.iter().enumerate() {
            port_labels
                .entry(net)
                .or_default()
                .push(format!("PORT:{}:{}:{:?}", port.name, bit, port.dir));
        }
    }
    for ni in 0..nl.net_count() {
        let mut labels = port_labels.remove(&ni).unwrap_or_default();
        labels.sort();
        let mut parts = vec!["NET".to_string()];
        parts.extend(labels);
        nets.insert(
            ni,
            digest(&parts.iter().map(String::as_str).collect::<Vec<_>>()),
        );
    }

    for _ in 0..32 {
        let mut next_cells = HashMap::new();
        for ci in 0..nl.cell_count() {
            let cell = nl.cell(ci);
            let mut parts = vec![
                cells[&ci].clone(),
                "OUT".to_string(),
                nets[&cell.output].clone(),
            ];
            for (pin, &input) in cell.inputs.iter().enumerate() {
                parts.push(format!("IN{pin}={}", nets[&input]));
            }
            parts.sort();
            next_cells.insert(
                ci,
                digest(&parts.iter().map(String::as_str).collect::<Vec<_>>()),
            );
        }

        let mut next_nets = HashMap::new();
        for ni in 0..nl.net_count() {
            let net = nl.net(ni);
            let mut parts = vec![nets[&ni].clone()];
            for &driver in &net.drivers {
                parts.push(format!(
                    "DRIVER:{}={}",
                    output_pin_name(nl, driver)?,
                    cells[&driver]
                ));
            }
            for &sink in &net.sinks {
                for (pin, input) in nl.cell(sink).inputs.iter().enumerate() {
                    if *input == ni {
                        parts.push(format!(
                            "SINK:{}:{}={}",
                            input_pin_name(nl, sink, pin)?,
                            pin,
                            cells[&sink]
                        ));
                    }
                }
            }
            parts.sort();
            next_nets.insert(
                ni,
                digest(&parts.iter().map(String::as_str).collect::<Vec<_>>()),
            );
        }

        let unchanged =
            signatures_equal(&cells, &next_cells) && signatures_equal(&nets, &next_nets);
        cells = next_cells;
        nets = next_nets;
        if unchanged {
            break;
        }
    }

    Ok(Signatures { cells, nets })
}

fn signature_counts(signatures: &HashMap<usize, String>) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for signature in signatures.values() {
        *out.entry(signature.clone()).or_insert(0) += 1;
    }
    out
}

fn signatures_equal(a: &HashMap<usize, String>, b: &HashMap<usize, String>) -> bool {
    let mut av: Vec<_> = a.values().cloned().collect();
    let mut bv: Vec<_> = b.values().cloned().collect();
    av.sort();
    bv.sort();
    av == bv
}

fn output_pin_name(nl: &Netlist, ci: usize) -> Result<String, String> {
    let cell = nl.cell(ci);
    if let CellKind::Lib(id) = &cell.kind {
        let lib = nl
            .library()
            .ok_or_else(|| format!("cell '{}' has no library", cell.name))?;
        return Ok(lib.get(*id).output.clone());
    }
    Ok("Q".to_string())
}

fn input_pin_name(nl: &Netlist, ci: usize, index: usize) -> Result<String, String> {
    let cell = nl.cell(ci);
    if let CellKind::Lib(id) = &cell.kind {
        let lib = nl
            .library()
            .ok_or_else(|| format!("cell '{}' has no library", cell.name))?;
        return lib
            .get(*id)
            .inputs
            .get(index)
            .cloned()
            .ok_or_else(|| format!("cell '{}' has no input pin {}", cell.name, index));
    }
    Ok(index.to_string())
}

fn map_cells(
    reference: &Netlist,
    layout: &Netlist,
    reference_sigs: &HashMap<usize, String>,
    layout_sigs: &HashMap<usize, String>,
) -> Vec<LvsCellMatch> {
    pair_by_signature(reference_sigs, layout_sigs)
        .into_iter()
        .map(|(a, b)| LvsCellMatch {
            reference_cell: reference.cell(a).name.clone(),
            layout_cell: layout.cell(b).name.clone(),
            device_type: device_type(reference, a).unwrap_or_default(),
        })
        .collect()
}

fn map_nets(
    reference: &Netlist,
    layout: &Netlist,
    reference_sigs: &HashMap<usize, String>,
    layout_sigs: &HashMap<usize, String>,
) -> Vec<LvsNetMatch> {
    pair_by_signature(reference_sigs, layout_sigs)
        .into_iter()
        .map(|(a, b)| LvsNetMatch {
            reference_net: reference.net(a).name.clone(),
            layout_net: layout.net(b).name.clone(),
        })
        .collect()
}

fn pair_by_signature(
    reference: &HashMap<usize, String>,
    layout: &HashMap<usize, String>,
) -> Vec<(usize, usize)> {
    let mut by_signature: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (id, signature) in layout {
        by_signature
            .entry(signature.as_str())
            .or_default()
            .push(*id);
    }
    let mut out = Vec::new();
    for (id, signature) in reference {
        if let Some(layout_ids) = by_signature.get_mut(signature.as_str()) {
            layout_ids.sort_unstable();
            if let Some(layout_id) = layout_ids.first().copied() {
                layout_ids.remove(0);
                out.push((*id, layout_id));
            }
        }
    }
    out.sort_by_key(|(reference, _)| *reference);
    out
}

fn digest(parts: &[&str]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0x1f;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::library::CellLibrary;
    use eda_core::netlist::Cell;
    use std::sync::Arc;

    fn and_netlist(name: &str) -> Netlist {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new(name);
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
        nl
    }

    #[test]
    fn renamed_instances_and_internal_nets_match() {
        let reference = and_netlist("ref");
        let mut layout = and_netlist("layout");
        layout.cells_mut()[0].name = "renamed".into();
        let r = check_lvs(&reference, &layout).unwrap();
        assert!(r.matches, "{:?}", r.notes);
    }

    #[test]
    fn device_mismatch_is_reported() {
        let reference = and_netlist("ref");
        let mut layout = and_netlist("layout");
        let lib = Arc::new(CellLibrary::std());
        layout.cells_mut()[0].kind = CellKind::Lib(lib.find("OR2").unwrap());
        let r = check_lvs(&reference, &layout).unwrap();
        assert!(!r.matches);
        assert!(r
            .notes
            .iter()
            .any(|note| note.contains("device count mismatch")));
    }

    #[test]
    fn multiple_drivers_are_reported_as_shorts() {
        let mut reference = and_netlist("short");
        let y = reference.net_by_name("y").unwrap();
        reference.add_cell(Cell {
            name: "short_driver".into(),
            kind: CellKind::Const(eda_core::logic::LogicValue::One),
            inputs: Vec::new(),
            output: y,
        });
        let layout = reference.clone();
        let r = check_lvs(&reference, &layout).unwrap();
        assert!(!r.matches);
        assert_eq!(r.shorted_nets.len(), 2);
    }

    #[test]
    fn dangling_internal_net_is_reported_as_open() {
        let mut reference = and_netlist("open");
        reference.add_net("floating");
        let layout = reference.clone();
        let r = check_lvs(&reference, &layout).unwrap();
        assert!(!r.matches);
        assert_eq!(r.open_nets.len(), 2);
        assert!(r.open_nets.iter().all(|open| open.contains("floating")));
    }
}
