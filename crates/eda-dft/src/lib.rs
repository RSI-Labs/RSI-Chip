//! # eda-dft — scan-chain insertion and scan-chain validation.
//!
//! Scan insertion wraps each mapped flop's data input in a function-input /
//! scan-input mux. In functional mode (`scan_en=0`) the original D value is
//! selected. In shift mode (`scan_en=1`) the previous flop Q, or the top-level
//! `scan_in`, is selected.

use eda_core::netlist::{Cell, CellKind, Dir, NetId, Netlist};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanCell {
    /// Cell ID in the returned scan netlist.
    pub cell: usize,
    pub name: String,
    pub q_net: NetId,
    pub functional_d_net: NetId,
    pub scan_d_net: NetId,
    pub mux_cell: usize,
}

#[derive(Clone, Debug)]
pub struct ScanChainResult {
    pub netlist: Netlist,
    pub chain: Vec<ScanCell>,
    pub scan_en: NetId,
    pub scan_in: NetId,
    pub scan_out: NetId,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DftReport {
    pub sequential_cells: usize,
    pub scan_cells: usize,
    pub scan_chains: usize,
    pub combinational_cells: usize,
    pub unscanned_sequential: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ScanOptions {
    pub chain_name: String,
    pub scan_en_port: String,
    pub scan_in_port: String,
    pub scan_out_port: String,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            chain_name: "chain0".into(),
            scan_en_port: "scan_en".into(),
            scan_in_port: "scan_in".into(),
            scan_out_port: "scan_out".into(),
        }
    }
}

/// Insert one serial scan chain into a mapped sequential netlist.
pub fn insert_scan_chain(nl: &Netlist, options: &ScanOptions) -> Result<ScanChainResult, String> {
    let lib = nl
        .library()
        .cloned()
        .ok_or_else(|| "scan insertion requires a library-backed netlist".to_string())?;
    let mux_id = lib
        .find("MUX2")
        .ok_or_else(|| "library has no MUX2 cell for scan insertion".to_string())?;
    if lib.get(mux_id).seq.is_some() {
        return Err("MUX2 is unexpectedly sequential".into());
    }
    for port_name in [
        &options.scan_en_port,
        &options.scan_in_port,
        &options.scan_out_port,
    ] {
        if port_name.is_empty() || nl.port_by_name(port_name).is_some() {
            return Err(format!(
                "scan port name '{}' is empty or already used",
                port_name
            ));
        }
    }

    let mut out = nl.clone();
    out.set_library(lib.clone());
    let scan_en = out.add_net(options.scan_en_port.clone());
    let scan_in = out.add_net(options.scan_in_port.clone());
    let scan_out = out.add_net(options.scan_out_port.clone());
    out.add_port(options.scan_en_port.clone(), Dir::In, vec![scan_en]);
    out.add_port(options.scan_in_port.clone(), Dir::In, vec![scan_in]);
    out.add_port(options.scan_out_port.clone(), Dir::Out, vec![scan_out]);

    let sequential: Vec<usize> = (0..out.cell_count())
        .filter(|&ci| out.cell_is_sequential(&out.cell(ci).kind))
        .collect();
    if sequential.is_empty() {
        return Err("cannot insert scan chain: netlist has no sequential cells".into());
    }

    let mut chain = Vec::new();
    let mut previous_q = scan_in;
    for ci in sequential {
        let old_cell = out.cell(ci).clone();
        if old_cell.inputs.len() < 2 {
            return Err(format!(
                "sequential cell '{}' has no data input",
                old_cell.name
            ));
        }
        let functional_d = old_cell.inputs[1];
        let q = old_cell.output;
        let mux_out = out.add_net(format!("{}_si", old_cell.name));
        let mux_name = format!("{}_scan_mux", old_cell.name);
        let mux_cell = out.add_cell(Cell {
            name: mux_name,
            kind: CellKind::Lib(mux_id),
            // MUX2 semantics: sel, in0, in1; sel=1 selects in1.
            inputs: vec![scan_en, functional_d, previous_q],
            output: mux_out,
        });
        let mut scanned = old_cell.clone();
        scanned.inputs[1] = mux_out;
        out.replace_cell(ci, scanned);
        chain.push(ScanCell {
            cell: ci,
            name: old_cell.name,
            q_net: q,
            functional_d_net: functional_d,
            scan_d_net: mux_out,
            mux_cell,
        });
        previous_q = q;
    }

    // Connect the final flop Q to the scan output with a library BUF. This
    // keeps every output port driven by a cell and preserves an unambiguous
    // observation point even when Q also feeds functional logic.
    let last_q = previous_q;
    if last_q != scan_in {
        let buf_id = lib
            .find("BUF")
            .ok_or_else(|| "library has no BUF cell for scan output".to_string())?;
        out.add_cell(Cell {
            name: format!("{}_scan_out_buf", options.chain_name),
            kind: CellKind::Lib(buf_id),
            inputs: vec![last_q],
            output: scan_out,
        });
    }

    check_scan_structure(&out, &chain, scan_en, scan_in, scan_out)?;
    Ok(ScanChainResult {
        netlist: out,
        chain,
        scan_en,
        scan_in,
        scan_out,
    })
}

/// Analyze sequential/scannable cell counts.
pub fn analyze(nl: &Netlist) -> Result<DftReport, String> {
    analyze_with_ports(nl, "scan_en", "scan_in", "scan_out")
}

/// Analyze scannability using explicit scan-control port names.
pub fn analyze_with_ports(
    nl: &Netlist,
    scan_en_port: &str,
    scan_in_port: &str,
    scan_out_port: &str,
) -> Result<DftReport, String> {
    let mut report = DftReport::default();
    let scan_ports = [scan_en_port, scan_in_port, scan_out_port]
        .iter()
        .all(|name| nl.port_by_name(name).is_some());
    for ci in 0..nl.cell_count() {
        let cell = nl.cell(ci);
        if nl.cell_is_sequential(&cell.kind) {
            report.sequential_cells += 1;
            if scan_ports {
                report.scan_cells += 1;
            } else {
                report.unscanned_sequential.push(cell.name.clone());
            }
        } else {
            report.combinational_cells += 1;
        }
    }
    report.scan_chains = usize::from(scan_ports && report.scan_cells > 0);
    Ok(report)
}

/// Structural scan-chain audit.
pub fn check_scan_structure(
    nl: &Netlist,
    chain: &[ScanCell],
    scan_en: NetId,
    scan_in: NetId,
    scan_out: NetId,
) -> Result<(), String> {
    if chain.is_empty() {
        return Err("scan chain is empty".into());
    }
    let Some(mux_id) = nl.library().and_then(|lib| lib.find("MUX2")) else {
        return Err("scan netlist has no MUX2 cell".into());
    };
    let mut previous_q = scan_in;
    for link in chain {
        let mux = nl.cell(link.mux_cell);
        if !matches!(mux.kind, CellKind::Lib(id) if id == mux_id) {
            return Err(format!("'{}' is not a MUX2", mux.name));
        }
        if mux.inputs != vec![scan_en, link.functional_d_net, previous_q] {
            return Err(format!("incorrect scan mux inputs on '{}'", mux.name));
        }
        if mux.output != link.scan_d_net {
            return Err(format!("scan mux '{}' output mismatch", mux.name));
        }
        let flop = nl.cell(link.cell);
        if !nl.cell_is_sequential(&flop.kind) || flop.inputs.get(1) != Some(&link.scan_d_net) {
            return Err(format!("flop '{}' is not scan connected", flop.name));
        }
        if flop.output != link.q_net {
            return Err(format!("flop '{}' Q connection mismatch", flop.name));
        }
        previous_q = link.q_net;
    }
    if nl.net(scan_out).drivers.is_empty() {
        return Err("scan_out is undriven".into());
    }
    Ok(())
}

pub fn report(result: &ScanChainResult) -> String {
    format!(
        "DFT scan report: chain length {}, scan_en={}, scan_in={}, scan_out={}\n",
        result.chain.len(),
        result.netlist.net(result.scan_en).name,
        result.netlist.net(result.scan_in).name,
        result.netlist.net(result.scan_out).name
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::library::CellLibrary;
    use std::sync::Arc;

    #[test]
    fn rejects_combinational_netlist() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("comb");
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
        assert!(insert_scan_chain(&nl, &ScanOptions::default()).is_err());
    }

    #[test]
    fn custom_scan_port_names_are_counted_as_scanned() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("seq");
        nl.set_library(lib.clone());
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let q = nl.add_net("q");
        nl.add_cell(Cell {
            name: "u_dff".into(),
            kind: CellKind::Lib(lib.find("DFF").unwrap()),
            inputs: vec![clk, d],
            output: q,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("q", Dir::Out, vec![q]);
        let options = ScanOptions {
            chain_name: "custom_chain".into(),
            scan_en_port: "test_en".into(),
            scan_in_port: "test_in".into(),
            scan_out_port: "test_out".into(),
        };
        let scan = insert_scan_chain(&nl, &options).unwrap();
        let default_view = analyze(&scan.netlist).unwrap();
        assert_eq!(default_view.scan_cells, 0);
        assert_eq!(default_view.unscanned_sequential, vec!["u_dff".to_string()]);
        let custom_view = analyze_with_ports(
            &scan.netlist,
            &options.scan_en_port,
            &options.scan_in_port,
            &options.scan_out_port,
        )
        .unwrap();
        assert_eq!(custom_view.sequential_cells, 1);
        assert_eq!(custom_view.scan_cells, 1);
        assert_eq!(custom_view.scan_chains, 1);
        assert!(custom_view.unscanned_sequential.is_empty());
    }
}
