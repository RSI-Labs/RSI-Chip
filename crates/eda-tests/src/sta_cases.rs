//! Static timing analysis integration cases split by constraint and analysis domain.

use crate::CaseList;
use eda_core::ir::Design;
use eda_core::library::CellLibrary;
use eda_core::netlist::{Cell, CellKind, Dir, NetId, Netlist};
use std::sync::{Arc, OnceLock};

mod exceptions_cases;
mod incremental_cases;
mod liberty_cases;
mod paths_cases;
mod quality_cases;
mod sdc_cases;

pub fn register(c: &mut CaseList) {
    liberty_cases::register(c);
    paths_cases::register(c);
    exceptions_cases::register(c);
    incremental_cases::register(c);
    quality_cases::register(c);
    sdc_cases::register(c);
}

fn cache(src: &'static str) -> &'static Design {
    static DESIGNS: OnceLock<
        std::sync::Mutex<std::collections::HashMap<&'static str, &'static Design>>,
    > = OnceLock::new();
    let m = DESIGNS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut g = m.lock().unwrap();
    g.entry(src)
        .or_insert_with(|| Box::leak(Box::new(eda_hdl::compile(src, None).expect("compile"))))
}

fn synth_cached(src: &'static str) -> &'static Netlist {
    static NETS: OnceLock<
        std::sync::Mutex<std::collections::HashMap<&'static str, &'static Netlist>>,
    > = OnceLock::new();
    let m = NETS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut g = m.lock().unwrap();
    g.entry(src).or_insert_with(|| {
        Box::leak(Box::new(
            eda_synth::synthesize(
                cache(src),
                &eda_synth::SynthOptions {
                    optimize: true,
                    map_lib: true,
                },
            )
            .expect("synth")
            .netlist,
        ))
    })
}

fn nldm_transition_chain() -> Result<(Netlist, NetId, NetId), String> {
    let source = r#"
library(nldm_transition) {
    time_unit : "1ns";
    cell (INV) {
        area : 1;
        pin (A) {
            direction : input;
            capacitance : 0.01;
            timing () {
                related_pin : "A";
                cell_rise(nldm_transition) {
                    index_1("0.0, 0.2");
                    index_2("0.0, 0.02");
                    values("0.10, 0.12", "0.14, 0.18");
                }
                cell_fall(nldm_transition) {
                    index_1("0.0, 0.2");
                    index_2("0.0, 0.02");
                    values("0.08, 0.09", "0.10, 0.11");
                }
                rise_transition(nldm_transition) {
                    index_1("0.0, 0.2");
                    index_2("0.0, 0.02");
                    values("0.04, 0.06", "0.08, 0.10");
                }
                fall_transition(nldm_transition) {
                    index_1("0.0, 0.2");
                    index_2("0.0, 0.02");
                    values("0.03, 0.04", "0.05, 0.06");
                }
            }
        }
        pin (Y) { direction : output; function : "!A"; }
    }
}
"#;
    let library = Arc::new(eda_api::parse_liberty(source)?);
    let mut nl = Netlist::new("nldm_transition_chain");
    nl.set_library(library.clone());
    let a = nl.add_net("a");
    let n = nl.add_net("n");
    let y = nl.add_net("y");
    let inv = library
        .find("INV")
        .ok_or_else(|| "missing transition INV".to_string())?;
    nl.add_cell(Cell {
        name: "u1".into(),
        kind: CellKind::Lib(inv),
        inputs: vec![a],
        output: n,
    });
    nl.add_cell(Cell {
        name: "u2".into(),
        kind: CellKind::Lib(inv),
        inputs: vec![n],
        output: y,
    });
    nl.add_port("a", Dir::In, vec![a]);
    nl.add_port("y", Dir::Out, vec![y]);
    nl.add_net("dangling");
    Ok((nl, n, y))
}

fn constraint_lut_netlist() -> Result<Netlist, String> {
    let source = r#"
library(constraint_luts) {
    time_unit : "1ns";
    cell (INV) {
        area : 1;
        pin (A) {
            direction : input;
            capacitance : 0.01;
            timing () {
                related_pin : "A";
                cell_rise(inv_template) {
                    index_1("0.0, 0.2");
                    index_2("0.0, 0.02");
                    values("0.10, 0.12", "0.14, 0.18");
                }
                cell_fall(inv_template) {
                    index_1("0.0, 0.2");
                    index_2("0.0, 0.02");
                    values("0.08, 0.09", "0.10, 0.11");
                }
                rise_transition(inv_template) {
                    index_1("0.0, 0.2");
                    index_2("0.0, 0.02");
                    values("0.04, 0.06", "0.08, 0.10");
                }
                fall_transition(inv_template) {
                    index_1("0.0, 0.2");
                    index_2("0.0, 0.02");
                    values("0.03, 0.04", "0.05, 0.06");
                }
            }
        }
        pin (Y) { direction : output; function : "!A"; }
    }
    cell (DFF) {
        area : 5;
        ff (IQ, IQN) { clocked_on : "CLK"; next_state : "D"; }
        pin (D) {
            direction : input;
            capacitance : 0.01;
            timing () {
                related_pin : "CLK";
                timing_type : setup_rising;
                rise_constraint(setup_template) {
                    index_1("0.0, 0.1");
                    index_2("0.0, 0.1");
                    values("0.20, 0.24", "0.28, 0.32");
                }
                fall_constraint(setup_template) {
                    index_1("0.0, 0.1");
                    index_2("0.0, 0.1");
                    values("0.18, 0.22", "0.26, 0.30");
                }
            }
            timing () {
                related_pin : "CLK";
                timing_type : hold_rising;
                rise_constraint(hold_template) {
                    index_1("0.0, 0.1");
                    index_2("0.0, 0.1");
                    values("0.02, 0.04", "0.06, 0.08");
                }
                fall_constraint(hold_template) {
                    index_1("0.0, 0.1");
                    index_2("0.0, 0.1");
                    values("0.01, 0.02", "0.03, 0.04");
                }
            }
        }
        pin (CLK) { direction : input; capacitance : 0.02; }
        pin (Q) {
            direction : output;
            timing () {
                related_pin : "CLK";
                timing_type : rising_edge;
                cell_rise(values("0.10"));
            }
        }
    }
}
"#;
    let library = Arc::new(eda_api::parse_liberty(source)?);
    let mut nl = Netlist::new("constraint_lut_chain");
    nl.set_library(library.clone());
    let clk = nl.add_net("clk");
    let clk_gated = nl.add_net("clk_gated");
    let d = nl.add_net("d");
    let q1 = nl.add_net("q1");
    let data = nl.add_net("data");
    let q2 = nl.add_net("q2");
    let inv = library
        .find("INV")
        .ok_or_else(|| "missing INV".to_string())?;
    let dff = library
        .find("DFF")
        .ok_or_else(|| "missing DFF".to_string())?;
    nl.add_cell(Cell {
        name: "clk_inv".into(),
        kind: CellKind::Lib(inv),
        inputs: vec![clk],
        output: clk_gated,
    });
    nl.add_cell(Cell {
        name: "launch".into(),
        kind: CellKind::Lib(dff),
        inputs: vec![clk, d],
        output: q1,
    });
    nl.add_cell(Cell {
        name: "data_inv".into(),
        kind: CellKind::Lib(inv),
        inputs: vec![q1],
        output: data,
    });
    nl.add_cell(Cell {
        name: "capture".into(),
        kind: CellKind::Lib(dff),
        inputs: vec![clk_gated, data],
        output: q2,
    });
    nl.add_port("clk", Dir::In, vec![clk]);
    nl.add_port("d", Dir::In, vec![d]);
    nl.add_port("q", Dir::Out, vec![q2]);
    Ok(nl)
}

fn constraint_lut_cached() -> &'static Netlist {
    static NETLIST: OnceLock<Netlist> = OnceLock::new();
    NETLIST.get_or_init(|| constraint_lut_netlist().expect("constraint LUT netlist"))
}

fn recovery_removal_lut_netlist() -> Result<Netlist, String> {
    let source = r#"
library(async_constraint_luts) {
    time_unit : "1ns";
    cell (INV) {
        area : 1;
        pin (A) {
            direction : input;
            capacitance : 0.01;
            timing () {
                related_pin : "A";
                cell_rise(async_template) {
                    index_1("0.0, 0.2");
                    index_2("0.0, 0.02");
                    values("0.10, 0.12", "0.14, 0.18");
                }
                cell_fall(async_template) {
                    index_1("0.0, 0.2");
                    index_2("0.0, 0.02");
                    values("0.08, 0.09", "0.10, 0.11");
                }
                rise_transition(async_template) {
                    index_1("0.0, 0.2");
                    index_2("0.0, 0.02");
                    values("0.04, 0.06", "0.08, 0.10");
                }
                fall_transition(async_template) {
                    index_1("0.0, 0.2");
                    index_2("0.0, 0.02");
                    values("0.03, 0.04", "0.05, 0.06");
                }
            }
        }
        pin (Y) { direction : output; function : "!A"; }
    }
    cell (DFFR) {
        area : 5;
        ff (IQ, IQN) {
            clocked_on : "CLK";
            next_state : "D";
            clear : "RST";
        }
        pin (RST) {
            direction : input;
            capacitance : 0.02;
            timing () {
                related_pin : "CLK";
                timing_type : recovery_rising;
                rise_constraint(recovery_template) {
                    index_1("0.0, 0.1");
                    index_2("0.0, 0.1");
                    values("0.30, 0.34", "0.38, 0.42");
                }
                fall_constraint(recovery_template) {
                    index_1("0.0, 0.1");
                    index_2("0.0, 0.1");
                    values("0.29, 0.33", "0.37, 0.41");
                }
            }
            timing () {
                related_pin : "CLK";
                timing_type : removal_rising;
                rise_constraint(removal_template) {
                    index_1("0.0, 0.1");
                    index_2("0.0, 0.1");
                    values("0.10, 0.14", "0.18, 0.22");
                }
                fall_constraint(removal_template) {
                    index_1("0.0, 0.1");
                    index_2("0.0, 0.1");
                    values("0.09, 0.13", "0.17, 0.21");
                }
            }
        }
        pin (CLK) { direction : input; capacitance : 0.02; }
        pin (D) { direction : input; capacitance : 0.01; }
        pin (Q) {
            direction : output;
            timing () {
                related_pin : "CLK";
                timing_type : rising_edge;
                cell_rise(values("0.10"));
            }
        }
    }
}
"#;
    let library = Arc::new(eda_api::parse_liberty(source)?);
    let mut nl = Netlist::new("async_constraint_luts");
    nl.set_library(library.clone());
    let clk = nl.add_net("clk");
    let clk_gated = nl.add_net("clk_gated");
    let rst = nl.add_net("rst");
    let rst_gated = nl.add_net("rst_gated");
    let d = nl.add_net("d");
    let q = nl.add_net("q");
    let inv = library
        .find("INV")
        .ok_or_else(|| "missing INV".to_string())?;
    let dffr = library
        .find("DFFR")
        .ok_or_else(|| "missing DFFR".to_string())?;
    nl.add_cell(Cell {
        name: "clk_inv".into(),
        kind: CellKind::Lib(inv),
        inputs: vec![clk],
        output: clk_gated,
    });
    nl.add_cell(Cell {
        name: "rst_inv".into(),
        kind: CellKind::Lib(inv),
        inputs: vec![rst],
        output: rst_gated,
    });
    nl.add_cell(Cell {
        name: "capture".into(),
        kind: CellKind::Lib(dffr),
        inputs: vec![clk_gated, d, rst_gated],
        output: q,
    });
    nl.add_port("clk", Dir::In, vec![clk]);
    nl.add_port("rst", Dir::In, vec![rst]);
    nl.add_port("d", Dir::In, vec![d]);
    nl.add_port("q", Dir::Out, vec![q]);
    Ok(nl)
}

fn recovery_removal_lut_cached() -> &'static Netlist {
    static NETLIST: OnceLock<Netlist> = OnceLock::new();
    NETLIST.get_or_init(|| recovery_removal_lut_netlist().expect("recovery/removal LUT netlist"))
}
