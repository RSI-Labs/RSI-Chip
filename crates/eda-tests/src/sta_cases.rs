//! Static timing analysis cases (exercising the eda-sta crate).

use crate::CaseList;
use eda_core::ir::Design;
use eda_core::library::CellLibrary;
use eda_core::netlist::{Cell, CellKind, Dir, NetId, Netlist};
use std::sync::{Arc, OnceLock};

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

pub fn register(c: &mut CaseList) {
    register_sdc(c);
    signal_quality(c);
    sdc_design_quality_rules(c);
    sdc_scoped_quality_rules(c);
    sdc_transition_load_rules(c);
    sdc_clock_transition_rules(c);
    sdc_clock_transition_min_max_checks(c);
    c.add("api_indexed_liberty_delay_uses_output_load", || {
        let source = r#"
library(nldm) {
    time_unit : "1ns";
    cell (INV) {
        area : 1;
        pin (A) {
            direction : input;
            capacitance : 0.01;
            timing () {
                related_pin : "A";
                cell_rise(nldm) {
                    index_1("0.0, 0.2");
                    index_2("0.0, 0.02");
                    values("0.10, 0.12", "0.14, 0.18");
                }
                cell_fall(nldm) {
                    index_1("0.0, 0.2");
                    index_2("0.0, 0.02");
                    values("0.08, 0.09", "0.10, 0.11");
                }
            }
        }
        pin (Y) { direction : output; function : "!A"; }
    }
}
"#;
        let library = Arc::new(eda_api::parse_liberty(source)?);
        let mut nl = Netlist::new("nldm_chain");
        nl.set_library(library.clone());
        let a = nl.add_net("a");
        let n = nl.add_net("n");
        let y = nl.add_net("y");
        let inv = library.find("INV").ok_or("missing indexed INV")?;
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
        let timing = eda_api::analyze_timing(&nl)?;
        let paths = eda_api::timing_paths(&nl, &timing, 1);
        let Some(path) = paths.first() else {
            return Err(eda_api::timing_report(&nl, &timing));
        };
        // The first arc is evaluated at zero input transition and sees its
        // 0.01 pF fanout load. Without a transition table, its output slew
        // falls back to the arc delay (0.11 ns), which is then used as the
        // second arc's indexed input-transition coordinate.
        if (timing.critical_path_delay - 0.232).abs() > 1e-12
            || path.arcs.len() != 3
            || (path.arcs[1].delay_ns - 0.11).abs() > 1e-12
            || (path.arcs[2].delay_ns - 0.122).abs() > 1e-12
        {
            Err(format!(
                "critical={:?} path={path:?} report={}",
                timing.critical_path_delay,
                eda_api::timing_report(&nl, &timing)
            ))
        } else {
            Ok(())
        }
    });
    c.add(
        "api_liberty_transition_propagation_updates_nldm_delay",
        || {
            let (nl, n, y) = nldm_transition_chain()?;
            let timing = eda_api::analyze_timing(&nl)?;
            let paths = eda_api::timing_paths(&nl, &timing, 1);
            let Some(path) = paths.first() else {
                return Err(eda_api::timing_report(&nl, &timing));
            };
            // The first stage drives 0.01 pF at zero input transition. Its worst
            // rise transition is 0.05 ns. That transition selects the 0.11 ns
            // interpolated delay for the unloaded second stage.
            let n_transition = timing.net_transition(n).ok_or("missing n transition")?;
            let y_transition = timing.net_transition(y).ok_or("missing y transition")?;
            let dangling = nl
                .net_by_name("dangling")
                .and_then(|net| timing.net_transition(net));
            if (n_transition - 0.05).abs() > 1e-12
                || (y_transition - 0.05).abs() > 1e-12
                || dangling.is_some()
                || (timing.critical_path_delay - 0.22).abs() > 1e-12
                || path.arcs.len() != 3
                || (path.arcs[1].delay_ns - 0.11).abs() > 1e-12
                || (path.arcs[2].delay_ns - 0.11).abs() > 1e-12
            {
                Err(format!(
                "critical={:?} n_slew={n_transition} y_slew={y_transition} path={path:?} report={}",
                timing.critical_path_delay,
                eda_api::timing_report(&nl, &timing)
            ))
            } else {
                Ok(())
            }
        },
    );
    c.add(
        "api_liberty_ps_nldm_preserves_capacitance_load_axis",
        || {
            let source = r#"
library(ps_nldm) {
    time_unit : "1ps";
    cell (INV) {
        area : 1;
        pin (A) {
            direction : input;
            capacitance : 0.01;
            timing () {
                related_pin : "A";
                cell_rise(ps_template) {
                    index_1("0, 200");
                    index_2("0, 0.02");
                    values("100, 120", "140, 180");
                }
                rise_transition(ps_template) {
                    index_1("0, 200");
                    index_2("0, 0.02");
                    values("40, 60", "80, 100");
                }
            }
        }
        pin (Y) { direction : output; function : "!A"; }
    }
}
"#;
            let library = Arc::new(eda_api::parse_liberty(source)?);
            let mut nl = Netlist::new("ps_nldm_chain");
            nl.set_library(library.clone());
            let a = nl.add_net("a");
            let n = nl.add_net("n");
            let y = nl.add_net("y");
            let inv = library.find("INV").ok_or("missing ps INV")?;
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
            let timing = eda_api::analyze_timing(&nl)?;
            let n_transition = timing.net_transition(n).ok_or("missing ps n slew")?;
            // The first load is 0.01 pF in both libraries. Only the time values
            // and time axes convert from ps to ns; the capacitance axis must not.
            if (n_transition - 0.05).abs() > 1e-12
                || (timing.critical_path_delay - 0.22).abs() > 1e-12
            {
                Err(format!(
                    "critical={} n_slew={n_transition}",
                    timing.critical_path_delay
                ))
            } else {
                Ok(())
            }
        },
    );
    c.add("api_liberty_ff_capacitance_unit_scales_output_load", || {
        let source = r#"
library(ff_nldm) {
    time_unit : "1ns";
    capacitance_unit : "1ff";
    cell (INV) {
        area : 1;
        pin (A) {
            direction : input;
            capacitance : 10;
            timing () {
                related_pin : "A";
                cell_rise(ff_template) {
                    index_1("0.0, 0.2");
                    index_2("0, 20");
                    values("0.10, 0.12", "0.14, 0.18");
                }
                rise_transition(ff_template) {
                    index_1("0.0, 0.2");
                    index_2("0, 20");
                    values("0.04, 0.06", "0.08, 0.10");
                }
            }
        }
        pin (Y) { direction : output; function : "!A"; }
    }
}
"#;
        let library = Arc::new(eda_api::parse_liberty(source)?);
        let mut nl = Netlist::new("ff_nldm_chain");
        nl.set_library(library.clone());
        let a = nl.add_net("a");
        let n = nl.add_net("n");
        let y = nl.add_net("y");
        let inv = library.find("INV").ok_or("missing ff INV")?;
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
        let timing = eda_api::analyze_timing(&nl)?;
        let n_transition = timing.net_transition(n).ok_or("missing ff n slew")?;
        // 10 ff and 20 ff normalize to 0.01 pF and 0.02 pF. The first
        // stage therefore sees the same 0.01 pF load as the ns/pf model.
        if (n_transition - 0.05).abs() > 1e-12 || (timing.critical_path_delay - 0.22).abs() > 1e-12
        {
            Err(format!(
                "critical={} n_slew={n_transition}",
                timing.critical_path_delay
            ))
        } else {
            Ok(())
        }
    });
    c.add(
        "api_liberty_capacitive_load_unit_scales_output_load",
        || {
            let source = r#"
library(standard_cap_nldm) {
    time_unit : "1ns";
    capacitive_load_unit (1, ff);
    cell (INV) {
        area : 1;
        pin (A) {
            direction : input;
            capacitance : 10;
            timing () {
                related_pin : "A";
                cell_rise(standard_cap_template) {
                    index_1("0.0, 0.2");
                    index_2("0, 20");
                    values("0.10, 0.12", "0.14, 0.18");
                }
                rise_transition(standard_cap_template) {
                    index_1("0.0, 0.2");
                    index_2("0, 20");
                    values("0.04, 0.06", "0.08, 0.10");
                }
            }
        }
        pin (Y) { direction : output; function : "!A"; }
    }
}
"#;
            let library = Arc::new(eda_api::parse_liberty(source)?);
            let mut nl = Netlist::new("standard_cap_nldm_chain");
            nl.set_library(library.clone());
            let a = nl.add_net("a");
            let n = nl.add_net("n");
            let y = nl.add_net("y");
            let inv = library.find("INV").ok_or("missing standard-cap INV")?;
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
            let timing = eda_api::analyze_timing(&nl)?;
            let n_transition = timing
                .net_transition(n)
                .ok_or("missing standard-cap n slew")?;
            if (n_transition - 0.05).abs() > 1e-12
                || (timing.critical_path_delay - 0.22).abs() > 1e-12
            {
                Err(format!(
                    "critical={} n_slew={n_transition}",
                    timing.critical_path_delay
                ))
            } else {
                Ok(())
            }
        },
    );
    c.add(
        "api_incremental_liberty_transition_update_matches_full",
        || {
            let (nl, n, y) = nldm_transition_chain()?;
            let old = eda_sta::parse_sdc(
                "create_clock -name clk -period 4.0 [get_ports a]",
            )?;
            let mut new = old.clone();
            new.set_pin_timing_derate("u1", "A", None, Some(2.0))?;
            let previous = eda_sta::analyze_with_constraints(&nl, &old)?;
            let incremental = eda_sta::update_timing_for_timing_derates(
                &nl,
                &previous,
                &new,
                ["u1"],
                &[],
            )?;
            let full = eda_sta::analyze_with_constraints(&nl, &new)?;
            let n_transition = incremental
                .timing
                .net_transition(n)
                .ok_or("incremental n transition missing")?;
            let y_transition = incremental
                .timing
                .net_transition(y)
                .ok_or("incremental y transition missing")?;
            if incremental.recomputed_cells.contains("u1")
                && incremental.recomputed_cells.contains("u2")
                && (n_transition - 0.10).abs() < 1e-12
                && (y_transition - 0.06).abs() < 1e-12
                && incremental.timing.net_transition(n) == full.net_transition(n)
                && incremental.timing.net_transition(y) == full.net_transition(y)
                && (incremental.timing.critical_path_delay - full.critical_path_delay).abs()
                    < 1e-12
                && (incremental.timing.critical_path_delay - 0.34).abs() < 1e-12
            {
                Ok(())
            } else {
                Err(format!(
                    "critical={:?}, full={:?}, n_slew={n_transition}, y_slew={y_transition}, cells={:?}",
                    incremental.timing.critical_path_delay,
                    full.critical_path_delay,
                    incremental.recomputed_cells
                ))
            }
        },
    );
    c.add(
        "api_liberty_constraint_luts_use_data_and_clock_transitions",
        || {
            let nl = constraint_lut_netlist()?;
            let timing = eda_api::analyze_timing(&nl)?;
            let data = nl.net_by_name("data").ok_or("missing data net")?;
            let clk_gated = nl
                .net_by_name("clk_gated")
                .ok_or("missing gated clock net")?;
            let data_transition = timing
                .net_transition(data)
                .ok_or("missing data transition")?;
            let clock_transition = timing
                .net_transition(clk_gated)
                .ok_or("missing clock transition")?;
            let capture_hold = timing
                .hold_timing_paths(&nl, 0.0, 0)
                .into_iter()
                .find(|path| {
                    matches!(
                        path.path.endpoint,
                        eda_api::TimingPathEndpoint::SequentialCell(ref name)
                            if name == "capture"
                    )
                })
                .ok_or("missing capture hold path")?;

            let constraints = eda_api::parse_timing_constraints(
                "create_clock -name clk -period 1.0 [get_ports clk]",
            )?;
            let constrained =
                eda_api::constrained_timing(&nl, &constraints)?;
            // The data inverter sees the launch flop's 0.10 ns slew and the
            // capture clock inverter sees a 0.02 pF clock-pin load. Those two
            // propagated slews (0.07/0.06 ns) select both constraint LUT axes.
            if (data_transition - 0.07).abs() > 1e-12
                || (clock_transition - 0.06).abs() > 1e-12
                || (timing.reg2reg_critical - 0.515).abs() > 1e-12
                || (capture_hold.hold_ns - 0.06).abs() > 1e-12
                || (constrained.reg2reg_slack - 0.485).abs() > 1e-12
                || (constrained.reg2reg_hold_slack - 0.175).abs() > 1e-12
            {
                Err(format!(
                    "data_slew={data_transition} clock_slew={clock_transition} reg2reg={} hold={} setup_slack={} hold_slack={}",
                    timing.reg2reg_critical,
                    capture_hold.hold_ns,
                    constrained.reg2reg_slack,
                    constrained.reg2reg_hold_slack
                ))
            } else {
                Ok(())
            }
        },
    );
    c.add(
        "api_liberty_recovery_removal_luts_use_reset_and_clock_transitions",
        || {
            let nl = recovery_removal_lut_netlist()?;
            let timing = eda_api::analyze_timing(&nl)?;
            let reset_gated = nl
                .net_by_name("rst_gated")
                .ok_or("missing gated reset net")?;
            let clock_gated = nl
                .net_by_name("clk_gated")
                .ok_or("missing gated clock net")?;
            let reset_transition = timing
                .net_transition(reset_gated)
                .ok_or("missing reset transition")?;
            let clock_transition = timing
                .net_transition(clock_gated)
                .ok_or("missing clock transition")?;
            let constraints = eda_api::parse_timing_constraints(
                r#"
                create_clock -name clk -period 2.0 [get_ports clk]
                set_input_delay 0.20 -clock clk [get_ports rst]
            "#,
            )?;
            let constrained =
                eda_api::constrained_timing(&nl, &constraints)?;
            if (reset_transition - 0.06).abs() > 1e-12
                || (clock_transition - 0.06).abs() > 1e-12
                || (constrained.recovery_slack - 1.308).abs() > 1e-12
                || (constrained.removal_slack - 0.148).abs() > 1e-12
                || (constrained.async_reset_slack - 0.148).abs() > 1e-12
            {
                Err(format!(
                    "reset_slew={reset_transition} clock_slew={clock_transition} recovery={} removal={}",
                    constrained.recovery_slack, constrained.removal_slack
                ))
            } else {
                Ok(())
            }
        },
    );
    c.add(
        "api_sdf_recovery_removal_annotation_overrides_liberty_luts",
        || {
            let nl = recovery_removal_lut_netlist()?;
            let sdf_text = r#"
(DELAYFILE (SDFVERSION "3.0") (DESIGN "async_constraint_luts") (TIMESCALE 1ns)
  (CELL (CELLTYPE "DFFR") (INSTANCE capture)
    (TIMINGCHECK
      (RECOVERY RST CLK (0.44::0.44))
      (REMOVAL RST CLK (0.20::0.20))
    )
  )
)
"#;
            let annotations = eda_api::parse_delay_file(sdf_text)?;
            let annotated = eda_api::back_annotate(&nl, &annotations)?;
            let constraints = eda_api::parse_timing_constraints(
                r#"
                create_clock -name clk -period 2.0 [get_ports clk]
                set_input_delay 0.20 -clock clk [get_ports rst]
            "#,
            )?;
            let timing = eda_api::constrained_timing(&annotated, &constraints)?;
            if (timing.recovery_slack - 1.24).abs() > 1e-12
                || (timing.removal_slack - 0.12).abs() > 1e-12
                || (timing.async_reset_slack - 0.12).abs() > 1e-12
            {
                Err(format!(
                    "recovery={} removal={}",
                    timing.recovery_slack, timing.removal_slack
                ))
            } else {
                Ok(())
            }
        },
    );
    c.add(
        "api_sdf_constraint_annotation_overrides_liberty_luts",
        || {
            let nl = constraint_lut_netlist()?;
            let sdf_text = r#"
(DELAYFILE (SDFVERSION "3.0") (DESIGN "constraint_lut_chain") (TIMESCALE 1ns)
  (CELL (CELLTYPE "DFF") (INSTANCE capture)
    (TIMINGCHECK
      (SETUP D CLK (0.400::0.400))
      (HOLD D CLK (0.100::0.100))
    )
  )
)
"#;
            let annotations = eda_api::parse_delay_file(sdf_text)?;
            let annotated = eda_api::back_annotate(&nl, &annotations)?;
            let timing = eda_api::analyze_timing(&annotated)?;
            let capture_hold = timing
                .hold_timing_paths(&annotated, 0.0, 0)
                .into_iter()
                .find(|path| {
                    matches!(
                        path.path.endpoint,
                        eda_api::TimingPathEndpoint::SequentialCell(ref name)
                            if name == "capture"
                    )
                })
                .ok_or("missing annotated capture hold path")?;
            // SDF timing checks are back-annotations and therefore supersede the
            // original Liberty constraint LUTs for the annotated instance.
            if (timing.reg2reg_critical - 0.635).abs() > 1e-12
                || (capture_hold.hold_ns - 0.100).abs() > 1e-12
            {
                Err(format!(
                    "reg2reg={} hold={}",
                    timing.reg2reg_critical, capture_hold.hold_ns
                ))
            } else {
                Ok(())
            }
        },
    );
    // --- Structural timing on hand-built netlists ---
    c.add("sta_inv_chain_delay", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("chain");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let mut prev = a;
        for i in 0..3 {
            let out = nl.add_net(format!("n{}", i));
            nl.add_cell(Cell {
                name: format!("u{}", i),
                kind: CellKind::Lib(lib.find("INV").unwrap()),
                inputs: vec![prev],
                output: out,
            });
            prev = out;
        }
        let y = prev;
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y", Dir::Out, vec![y]);
        let r = eda_sta::analyze(&nl)?;
        // 3 INV @ 0.4ns = 1.2ns combinational worst path.
        if (r.critical_path_delay - 1.2).abs() > 1e-9 {
            Err(format!("critical = {}, want 1.2", r.critical_path_delay))
        } else {
            Ok(())
        }
    });
    c.add("sta_and_or_two_levels", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("two");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let n1 = nl.add_net("n1");
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
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let r = eda_sta::analyze(&nl)?;
        if (r.critical_path_delay - (1.0 + 0.4)).abs() > 1e-9 {
            Err(format!("critical = {}, want 1.4", r.critical_path_delay))
        } else {
            Ok(())
        }
    });
    // Register-to-register path: clk->q + comb + setup.
    c.add("sta_reg2reg", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("r2r");
        nl.set_library(lib.clone());
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let q = nl.add_net("q");
        let w = nl.add_net("w");
        let q2 = nl.add_net("q2");
        nl.add_cell(Cell {
            name: "ff1".into(),
            kind: CellKind::Lib(lib.find("DFF").unwrap()),
            inputs: vec![clk, d],
            output: q,
        });
        nl.add_cell(Cell {
            name: "inv".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![q],
            output: w,
        });
        nl.add_cell(Cell {
            name: "ff2".into(),
            kind: CellKind::Lib(lib.find("DFF").unwrap()),
            inputs: vec![clk, w],
            output: q2,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("q", Dir::Out, vec![q]);
        nl.add_port("q2", Dir::Out, vec![q2]);
        let r = eda_sta::analyze(&nl)?;
        let want = 1.2 + 0.4 + 0.6; // clk->q + inv + setup
        if (r.reg2reg_critical - want).abs() > 1e-9 {
            Err(format!(
                "reg2reg = {}, want {}; in2reg={}; critical={}; arrivals={:?}",
                r.reg2reg_critical,
                want,
                r.in2reg_critical,
                r.critical_path_delay,
                eda_sta::arrival_by_name(&nl, &r)
            ))
        } else {
            Ok(())
        }
    });
    c.add("sta_hold_min_arrival", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("hold");
        nl.set_library(lib.clone());
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let q = nl.add_net("q");
        let w = nl.add_net("w");
        let q2 = nl.add_net("q2");
        let dff = lib.find("DFF").unwrap();
        nl.add_cell(Cell {
            name: "ff1".into(),
            kind: CellKind::Lib(dff),
            inputs: vec![clk, d],
            output: q,
        });
        nl.add_cell(Cell {
            name: "inv".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![q],
            output: w,
        });
        nl.add_cell(Cell {
            name: "ff2".into(),
            kind: CellKind::Lib(dff),
            inputs: vec![clk, w],
            output: q2,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("q", Dir::Out, vec![q]);
        nl.add_port("q2", Dir::Out, vec![q2]);
        let r = eda_sta::analyze(&nl)?;
        if (r.reg2reg_hold_critical - 1.6).abs() > 1e-9 {
            Err(format!(
                "reg2reg hold = {}, want 1.6",
                r.reg2reg_hold_critical
            ))
        } else if (r.in2reg_hold_critical - 0.0).abs() > 1e-9 || r.hold_endpoint_list().len() != 2 {
            Err(format!(
                "in2reg hold={}, endpoints={}",
                r.in2reg_hold_critical,
                r.hold_endpoint_list().len()
            ))
        } else {
            Ok(())
        }
    });
    // Slack at a given period.
    c.add("sta_slack", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("slack");
        nl.set_library(lib.clone());
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let q = nl.add_net("q");
        let n1 = nl.add_net("n1");
        let n2 = nl.add_net("n2");
        nl.add_cell(Cell {
            name: "ff1".into(),
            kind: CellKind::Lib(lib.find("DFF").unwrap()),
            inputs: vec![clk, d],
            output: q,
        });
        nl.add_cell(Cell {
            name: "i1".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![q],
            output: n1,
        });
        nl.add_cell(Cell {
            name: "i2".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![n1],
            output: n2,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("q", Dir::Out, vec![q]);
        let r = eda_sta::analyze(&nl)?;
        let period = 5.0;
        let slack = eda_sta::worst_slack(&r, period);
        let want = period - (1.2 + 0.8); // tail is unconnected: comb from q through 2 inv
        if (slack - want).abs() > 1e-9 {
            Err(format!("slack = {}, want {}", slack, want))
        } else {
            Ok(())
        }
    });
    // Synthesized designs produce finite, positive timing.
    for (name, src) in [
        ("add4", "module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule"),
        ("mux", "module m(input s, a, b, output y); assign y = s ? b : a; endmodule"),
        ("counter", "module m(input clk, rst, output reg [3:0] q); always @(posedge clk or posedge rst) if (rst) q <= 4'd0; else q <= q + 4'd1; endmodule"),
    ] {
        c.add(format!("sta_synth_{}", name), move || {
            let nl = synth_cached(src);
            let r = eda_sta::analyze(nl)?;
            if !r.critical_path_delay.is_finite() || r.critical_path_delay <= 0.0 {
                Err(format!("bad critical path {}", r.critical_path_delay))
            } else {
                Ok(())
            }
        });
    }
    // Critical path reports the right cell count/sequence for a chain.
    c.add("sta_critical_path_cells", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("chain2");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let n1 = nl.add_net("n1");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![a],
            output: n1,
        });
        nl.add_cell(Cell {
            name: "u2".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![n1],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y", Dir::Out, vec![y]);
        let r = eda_sta::analyze(&nl)?;
        let path = r.critical_path_cells();
        if path.len() != 2 {
            Err(format!("path has {} cells, want 2", path.len()))
        } else {
            Ok(())
        }
    });

    c.add("sta_timing_paths_reconstruct_cells_and_nets", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("path_chain");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let n1 = nl.add_net("n1");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![a],
            output: n1,
        });
        nl.add_cell(Cell {
            name: "u2".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![n1],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y", Dir::Out, vec![y]);
        let r = eda_sta::analyze(&nl)?;
        let paths = r.timing_paths(&nl, 0);
        let report = eda_sta::timing_path_report(&nl, &paths);
        if paths.len() == 1
            && paths[0].kind == eda_sta::TimingPathKind::InputToOutput
            && paths[0].launch == eda_sta::TimingLaunch::PrimaryInput("a".into())
            && paths[0].cells.len() == 2
            && paths[0].nets.len() == 3
            && report.contains("cells: u1 -> u2")
            && report.contains("nets: a -> n1 -> y")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("sta_timing_paths_include_incremental_arcs", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("arc_chain");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let mut prev = a;
        for index in 0..3 {
            let out = nl.add_net(format!("n{}", index));
            nl.add_cell(Cell {
                name: format!("u{}", index),
                kind: CellKind::Lib(lib.find("INV").unwrap()),
                inputs: vec![prev],
                output: out,
            });
            prev = out;
        }
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y", Dir::Out, vec![prev]);
        let timing = eda_sta::analyze(&nl)?;
        let paths = timing.timing_paths(&nl, 1);
        let report = eda_sta::timing_path_report(&nl, &paths);
        let Some(path) = paths.first() else {
            return Err("no timing path".into());
        };
        let launch_ok = path.arcs.first().is_some_and(|arc| {
            arc.kind == eda_sta::TimingArcKind::PrimaryInputLaunch
                && arc.cell.is_none()
                && arc.from_net == Some(a)
                && arc.to_net == a
                && arc.from_arrival_ns.is_none()
                && (arc.to_arrival_ns - 0.0).abs() < 1e-12
                && (arc.delay_ns - 0.0).abs() < 1e-12
        });
        let combinational_ok = path.arcs.len() == 4
            && path.arcs[1..]
                .iter()
                .zip([0.4, 0.8, 1.2])
                .all(|(arc, arrival)| {
                    arc.kind == eda_sta::TimingArcKind::Combinational
                        && arc.cell.is_some()
                        && arc.from_arrival_ns.is_some()
                        && (arc.to_arrival_ns - arrival).abs() < 1e-12
                        && (arc.delay_ns - 0.4).abs() < 1e-12
                });
        let arc_sum = path.arcs.iter().map(|arc| arc.delay_ns).sum::<f64>();
        if launch_ok
            && combinational_ok
            && (arc_sum - path.arrival_ns).abs() < 1e-12
            && report.contains("input a -> a: launch delay 0.000000 ns")
            && report.contains("u0/a -> u0/y: combinational 0.400000 ns")
        {
            Ok(())
        } else {
            Err(format!(
                "sum={arc_sum}, arrival={}\n{}",
                path.arrival_ns, report
            ))
        }
    });

    c.add(
        "sta_timing_path_arcs_include_sdc_input_and_clk_launch",
        || {
            let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= ~q1;
             endmodule";
            let nl = synth_cached(src);
            let constraints = eda_sta::parse_sdc(
                r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 1.0 -clock clk [get_ports d]
        "#,
            )?;
            let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
            let paths = eda_sta::constrained_timing_paths(nl, &timing, 0);
            let report = eda_sta::constrained_timing_path_report(nl, &paths);
            let input_launch = paths.iter().any(|path| {
                path.path.launch == eda_sta::TimingLaunch::PrimaryInput("d".into())
                    && path.path.arcs.first().is_some_and(|arc| {
                        arc.kind == eda_sta::TimingArcKind::PrimaryInputLaunch
                            && (arc.delay_ns - 1.0).abs() < 1e-12
                            && (arc.to_arrival_ns - 1.0).abs() < 1e-12
                    })
            });
            let clock_launch = paths.iter().any(|path| {
                matches!(path.path.launch, eda_sta::TimingLaunch::SequentialCell(_))
                    && path.path.arcs.first().is_some_and(|arc| {
                        arc.kind == eda_sta::TimingArcKind::ClockToOutput
                            && (arc.delay_ns - 1.2).abs() < 1e-12
                            && (arc.to_arrival_ns - 1.2).abs() < 1e-12
                    })
                    && path.path.arcs.get(1).is_some_and(|arc| {
                        arc.kind == eda_sta::TimingArcKind::Combinational
                            && (arc.delay_ns - 0.4).abs() < 1e-12
                            && (arc.to_arrival_ns - 1.6).abs() < 1e-12
                    })
            });
            if input_launch
                && clock_launch
                && report.contains("launch delay 1.000000 ns")
                && report.contains("clock-to-output 1.200000 ns")
            {
                Ok(())
            } else {
                Err(report)
            }
        },
    );

    c.add("sta_timing_path_arcs_reflect_sdf_annotation", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("sdf_arcs");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let n0 = nl.add_net("n0");
        let y = nl.add_net("y");
        for (name, input, output) in [("u0", a, n0), ("u1", n0, y)] {
            nl.add_cell(Cell {
                name: name.into(),
                kind: CellKind::Lib(lib.find("INV").unwrap()),
                inputs: vec![input],
                output,
            });
        }
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y", Dir::Out, vec![y]);
        let sdf = eda_sdf::parse_sdf(
            r#"
            (DELAYFILE
              (SDFVERSION "3.0")
              (DESIGN "sdf_arcs")
              (TIMESCALE 1ns)
              (CELL (CELLTYPE "INV") (INSTANCE u0)
                (DELAY (ABSOLUTE (IOPATH A Y (0.8))))
              )
              (CELL (CELLTYPE "INV") (INSTANCE u1)
                (DELAY (ABSOLUTE (IOPATH A Y (0.9))))
              )
            )
            "#,
        )?;
        let annotated = eda_sdf::annotate_sdf(&nl, &sdf)?;
        let timing = eda_sta::analyze(&annotated)?;
        let path = timing
            .timing_paths(&annotated, 1)
            .into_iter()
            .next()
            .ok_or("no annotated path")?;
        if path.arcs.len() == 3
            && (path.arcs[1].delay_ns - 0.8).abs() < 1e-12
            && (path.arcs[2].delay_ns - 0.9).abs() < 1e-12
            && (path.arrival_ns - 1.7).abs() < 1e-12
        {
            Ok(())
        } else {
            Err(format!(
                "delays={},{}, arrival={}",
                path.arcs[1].delay_ns, path.arcs[2].delay_ns, path.arrival_ns
            ))
        }
    });

    c.add("sta_timing_path_arcs_reflect_cell_derate", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("derated_arc");
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
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_timing_derate -early 0.9 -late 1.1
            set_timing_derate -cell_delay -early 0.5 -late 2.0 [get_cells u1]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let paths = eda_sta::constrained_timing_paths(&nl, &timing, 1);
        let Some(path) = paths.first() else {
            return Err("no derated path".into());
        };
        if path.path.arcs.len() == 2
            && (path.path.arcs[0].delay_ns - 0.22).abs() < 1e-12
            && (path.path.arcs[1].delay_ns - 2.0).abs() < 1e-12
            && (path.path.arrival_ns - 2.22).abs() < 1e-12
        {
            Ok(())
        } else {
            Err(format!(
                "launch={}, arc={}, arrival={}",
                path.path.arcs[0].delay_ns, path.path.arcs[1].delay_ns, path.path.arrival_ns
            ))
        }
    });

    c.add("sta_timing_path_arcs_select_derated_critical_pin", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("pin_derated_arc");
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
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_input_delay 0.1 -clock clk [get_ports a]
            set_input_delay 0.1 -clock clk [get_ports b]
            set_timing_derate -cell_delay -late 2.0 [get_pins u1/a]
            set_timing_derate -cell_delay -late 1.0 [get_pins u1/b]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let paths = eda_sta::constrained_timing_paths(&nl, &timing, 1);
        let report = eda_sta::constrained_timing_path_report(&nl, &paths);
        let Some(path) = paths.first() else {
            return Err("no pin-derated path".into());
        };
        if path.path.arcs.len() == 2
            && path.path.arcs[1].from_net == Some(a)
            && (path.path.arcs[1].delay_ns - 2.0).abs() < 1e-12
            && report.contains("u1/a -> u1/y: combinational 2.000000 ns")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("sta_constrained_timing_paths_sort_by_slack", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 1.0 -clock clk [get_ports d]
            set_clock_uncertainty 0.1 -setup [get_clocks clk]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
        let paths = eda_sta::constrained_timing_paths(nl, &timing, 0);
        let report = eda_sta::constrained_timing_path_report(nl, &paths);
        if paths.len() >= 3
            && paths
                .windows(2)
                .all(|pair| pair[0].slack_ns <= pair[1].slack_ns)
            && paths.iter().any(|path| {
                matches!(
                    path.path.endpoint,
                    eda_sta::TimingPathEndpoint::SequentialCell(_)
                )
            })
            && report.contains("slack")
            && report.contains("required")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add(
        "sta_constrained_timing_paths_include_clock_metadata",
        || {
            let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
            let nl = synth_cached(src);
            let constraints = eda_sta::parse_sdc(
                r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_clock_latency -source 0.1 [get_clocks clk]
            set_clock_latency 0.2 [get_clocks clk]
        "#,
            )?;
            let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
            let paths = eda_sta::constrained_timing_paths(nl, &timing, 0);
            let report = eda_sta::constrained_timing_path_report(nl, &paths);
            let sequential_ok = paths.iter().any(|path| {
                matches!(path.path.launch, eda_sta::TimingLaunch::SequentialCell(_))
                    && matches!(
                        path.path.endpoint,
                        eda_sta::TimingPathEndpoint::SequentialCell(_)
                    )
                    && path.clocks.launch.edge == eda_sta::TimingClockEdgeKind::Rise
                    && path.clocks.capture.edge == eda_sta::TimingClockEdgeKind::Rise
                    && !path.clocks.launch.virtual_reference
                    && !path.clocks.capture.virtual_reference
                    && (path.clocks.edge_separation_ns - 4.0).abs() < 1e-12
                    && (path.clocks.insertion_skew_ns - 0.0).abs() < 1e-12
            });
            let input_ok = paths.iter().any(|path| {
                matches!(path.path.launch, eda_sta::TimingLaunch::PrimaryInput(_))
                    && matches!(
                        path.path.endpoint,
                        eda_sta::TimingPathEndpoint::SequentialCell(_)
                    )
                    && path.clocks.launch.virtual_reference
                    && !path.clocks.capture.virtual_reference
                    && (path.clocks.edge_separation_ns - 4.2).abs() < 1e-12
                    && (path.clocks.insertion_skew_ns - 0.2).abs() < 1e-12
            });
            if sequential_ok
                && input_ok
                && report.contains("launch: clk rise at 0.000000 ns")
                && report.contains("capture: clk rise at 0.000000 ns")
                && report.contains("edge separation 4.200000 ns, insertion skew 0.200000 ns")
            {
                Ok(())
            } else {
                Err(report)
            }
        },
    );

    c.add("sta_multicycle_clock_metadata_extends_capture_edge", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_multicycle_path 2 -to [get_ports q1]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
        let paths = eda_sta::constrained_timing_paths(nl, &timing, 0);
        let report = eda_sta::constrained_timing_path_report(nl, &paths);
        let endpoint_net = nl.net_by_name("q1").ok_or("missing q1")?;
        let multiplier = timing
            .path_cycle_multipliers
            .get(nl.net(endpoint_net).name.as_str())
            .copied()
            .unwrap_or(1.0);
        let endpoint = paths
            .iter()
            .find(|path| {
                matches!(
                    path.path.endpoint,
                    eda_sta::TimingPathEndpoint::SequentialCell(_)
                ) && path.path.nets.last() == Some(&endpoint_net)
            })
            .ok_or("missing multicycle endpoint")?;
        if (multiplier - 2.0).abs() < 1e-12
            && endpoint.exceptions.len() == 1
            && endpoint.exceptions[0].multicycle == Some(2.0)
            && (endpoint.clocks.edge_separation_ns - 8.0).abs() < 1e-12
            && (endpoint.clocks.capture.absolute_time_ns - 8.0).abs() < 1e-12
            && report.contains("multicycle 2.000000, from <any> to q1")
        {
            Ok(())
        } else {
            Err(format!(
                "multiplier={multiplier}, separation={}, capture={}",
                endpoint.clocks.edge_separation_ns, endpoint.clocks.capture.absolute_time_ns
            ))
        }
    });

    c.add("sta_pin_false_path_matches_capture_data_pin", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_false_path -to [get_pins u2_dff/d]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
        let paths = eda_sta::constrained_timing_paths(nl, &timing, 0);
        let excluded = eda_sta::excluded_timing_paths(nl, &timing);
        let capture_excluded = excluded.iter().any(|path| {
            path.check == eda_sta::ExcludedTimingCheck::Setup
                && matches!(
                    path.path.endpoint,
                    eda_sta::TimingPathEndpoint::SequentialCell(_)
                )
        });
        let output_kept = paths.iter().any(|path| {
            path.path.endpoint == eda_sta::TimingPathEndpoint::OutputPort("q1".into())
                && path.exceptions.is_empty()
        });
        let capture_kept = paths.iter().any(|path| {
            matches!(
                path.path.endpoint,
                eda_sta::TimingPathEndpoint::SequentialCell(_)
            ) && path
                .path
                .nets
                .last()
                .is_some_and(|&net| nl.net(net).name == "q1")
        });
        if !capture_kept
            && capture_excluded
            && output_kept
            && timing.path_exceptions.values().flatten().any(|exception| {
                exception.to_object.as_ref().is_some_and(|object| {
                    object.kind == eda_sta::TimingObjectKind::Pin && object.name == "u2_dff/d"
                })
            })
        {
            Ok(())
        } else {
            Err(eda_sta::excluded_timing_path_report(nl, &excluded))
        }
    });

    c.add(
        "sta_pin_multicycle_scopes_capture_pin_not_output_port",
        || {
            let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
            let nl = synth_cached(src);
            let constraints = eda_sta::parse_sdc(
                r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_multicycle_path 2 -to [get_pins u2_dff/d]
        "#,
            )?;
            let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
            let paths = eda_sta::constrained_timing_paths(nl, &timing, 0);
            let capture = paths
                .iter()
                .find(|path| {
                    matches!(
                        path.path.endpoint,
                        eda_sta::TimingPathEndpoint::SequentialCell(_)
                    ) && path
                        .path
                        .nets
                        .last()
                        .is_some_and(|&net| nl.net(net).name == "q1")
                })
                .ok_or("missing capture path")?;
            let output = paths
                .iter()
                .find(|path| {
                    path.path.endpoint == eda_sta::TimingPathEndpoint::OutputPort("q1".into())
                })
                .ok_or("missing output path")?;
            if capture.exceptions.len() == 1
                && capture.exceptions[0].multicycle == Some(2.0)
                && (capture.clocks.edge_separation_ns - 8.0).abs() < 1e-12
                && output.exceptions.is_empty()
                && (output.clocks.edge_separation_ns - 4.0).abs() < 1e-12
            {
                Ok(())
            } else {
                Err(format!(
                    "capture={}, output={}",
                    capture.clocks.edge_separation_ns, output.clocks.edge_separation_ns
                ))
            }
        },
    );

    c.add("sta_pin_delay_limits_attach_to_matching_path", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_max_delay 2.0 -to [get_pins u2_dff/d]
            set_min_delay 1.0 -to [get_pins u2_dff/d]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
        let paths = eda_sta::constrained_timing_paths(nl, &timing, 0);
        let report = eda_sta::constrained_timing_path_report(nl, &paths);
        let capture = paths
            .iter()
            .find(|path| {
                matches!(
                    path.path.endpoint,
                    eda_sta::TimingPathEndpoint::SequentialCell(_)
                ) && path
                    .path
                    .nets
                    .last()
                    .is_some_and(|&net| nl.net(net).name == "q1")
            })
            .ok_or("missing capture path")?;
        let output = paths
            .iter()
            .find(|path| path.path.endpoint == eda_sta::TimingPathEndpoint::OutputPort("q1".into()))
            .ok_or("missing output path")?;
        if (timing.max_delay_slack - 0.8).abs() < 1e-12
            && (timing.min_delay_slack - 0.2).abs() < 1e-12
            && capture.delay_limits.len() == 1
            && (capture.delay_limits[0].limit.limit - 2.0).abs() < 1e-12
            && (capture.delay_limits[0].slack_ns - 0.8).abs() < 1e-12
            && (capture.slack_ns - 0.8).abs() < 1e-12
            && output.delay_limits.is_empty()
            && (output.slack_ns - 2.8).abs() < 1e-12
            && report.contains("max 2.000000 ns, slack 0.800000 ns")
        {
            Ok(())
        } else {
            Err(format!(
                "max={}, min={}, capture slack={}, output slack={}\n{}",
                timing.max_delay_slack,
                timing.min_delay_slack,
                capture.slack_ns,
                output.slack_ns,
                report
            ))
        }
    });

    c.add("sta_pin_min_delay_limit_attaches_to_hold_path", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_min_delay 1.0 -to [get_pins u2_dff/d]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
        let paths = eda_sta::constrained_hold_timing_paths(nl, &timing, 0);
        let report = eda_sta::constrained_hold_timing_path_report(nl, &paths);
        let capture = paths
            .iter()
            .find(|path| {
                matches!(
                    path.path.path.launch,
                    eda_sta::TimingLaunch::SequentialCell(_)
                )
            })
            .ok_or("missing sequential hold path")?;
        if capture.delay_limits.len() == 1
            && !capture.delay_limits[0].maximum
            && (capture.delay_limits[0].limit.limit - 1.0).abs() < 1e-12
            && (capture.delay_limits[0].slack_ns - 0.2).abs() < 1e-12
            && report.contains("min 1.000000 ns, slack 0.200000 ns")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("sta_ordered_through_false_path_matches_path_order", || {
        let nl = through_chain_netlist();
        let ordered = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_false_path -through [get_pins u0/y] -through [get_pins u1/y]
        "#,
        )?;
        let ordered_timing = eda_sta::analyze_with_sdc(&nl, &ordered)?;
        let ordered_paths = eda_sta::constrained_timing_paths(&nl, &ordered_timing, 0);
        let ordered_excluded = eda_sta::excluded_timing_paths(&nl, &ordered_timing);
        let ordered_report = eda_sta::excluded_timing_path_report(&nl, &ordered_excluded);

        let reversed = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_false_path -through [get_pins u1/y] -through [get_pins u0/y]
        "#,
        )?;
        let reversed_timing = eda_sta::analyze_with_sdc(&nl, &reversed)?;
        let reversed_paths = eda_sta::constrained_timing_paths(&nl, &reversed_timing, 0);
        let reversed_excluded = eda_sta::excluded_timing_paths(&nl, &reversed_timing);

        let object_groups_ok = ordered
            .false_paths
            .first()
            .and_then(|exception| {
                exception
                    .through_groups
                    .first()
                    .and_then(|group| group.first())
                    .map(|object| {
                        object.kind == eda_sta::TimingObjectKind::Pin && object.name == "u0/y"
                    })
            })
            .unwrap_or(false);
        if ordered_paths.is_empty()
            && ordered_excluded.len() == 1
            && object_groups_ok
            && ordered_report.contains("through u0/y -> u1/y")
            && reversed_paths.len() == 1
            && reversed_excluded.is_empty()
        {
            Ok(())
        } else {
            Err(format!(
                "ordered paths={}, excluded={}; reversed paths={}, excluded={}\n{}",
                ordered_paths.len(),
                ordered_excluded.len(),
                reversed_paths.len(),
                reversed_excluded.len(),
                ordered_report
            ))
        }
    });

    c.add("sta_through_max_delay_uses_typed_path_objects", || {
        let nl = through_chain_netlist();
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_max_delay 1.0 -through [get_pins u1/y] -to [get_ports y]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let paths = eda_sta::constrained_timing_paths(&nl, &timing, 0);
        let report = eda_sta::constrained_timing_path_report(&nl, &paths);
        let Some(path) = paths.first() else {
            return Err("through-chain path missing".into());
        };
        if (timing.max_delay_slack + 0.2).abs() < 1e-12
            && path.delay_limits.len() == 1
            && path.delay_limits[0].limit.through_groups.len() == 1
            && path.delay_limits[0].limit.through_groups[0][0].name == "u1/y"
            && (path.delay_limits[0].slack_ns + 0.2).abs() < 1e-12
            && report.contains("through u1/y")
        {
            Ok(())
        } else {
            Err(format!(
                "max={}, path limits={}\n{}",
                timing.max_delay_slack,
                path.delay_limits.len(),
                report
            ))
        }
    });

    c.add("sta_wildcard_through_false_path_respects_hierarchy", || {
        let nl = hierarchical_through_chain_netlist();
        let matching = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_false_path -through [get_pins top/u*/y]
        "#,
        )?;
        let matching_timing = eda_sta::analyze_with_sdc(&nl, &matching)?;
        let matching_paths = eda_sta::constrained_timing_paths(&nl, &matching_timing, 0);
        let matching_excluded = eda_sta::excluded_timing_paths(&nl, &matching_timing);

        let nonmatching = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_false_path -through [get_pins u*/y]
        "#,
        )?;
        let nonmatching_timing = eda_sta::analyze_with_sdc(&nl, &nonmatching)?;
        let nonmatching_paths = eda_sta::constrained_timing_paths(&nl, &nonmatching_timing, 0);
        let nonmatching_excluded = eda_sta::excluded_timing_paths(&nl, &nonmatching_timing);

        let pattern = matching
            .false_paths
            .first()
            .and_then(|exception| exception.through_groups.first())
            .and_then(|group| group.first())
            .ok_or("missing through pattern")?;
        if pattern.is_pattern()
            && pattern.matches_name("top/u1/y")
            && !pattern.matches_name("u1/y")
            && matching_paths.is_empty()
            && matching_excluded.len() == 1
            && nonmatching_paths.len() == 1
            && nonmatching_excluded.is_empty()
        {
            Ok(())
        } else {
            Err(format!(
                "matching paths={} excluded={}, nonmatching paths={} excluded={}",
                matching_paths.len(),
                matching_excluded.len(),
                nonmatching_paths.len(),
                nonmatching_excluded.len()
            ))
        }
    });

    c.add(
        "sta_recursive_hierarchy_wildcard_matches_nested_objects",
        || {
            let nl = hierarchical_through_chain_netlist();
            let constraints = eda_sta::parse_sdc(
                r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_false_path -through [get_pins **/u1/y]
        "#,
            )?;
            let timing = eda_sta::analyze_with_sdc(&nl, &constraints)?;
            let paths = eda_sta::constrained_timing_paths(&nl, &timing, 0);
            let excluded = eda_sta::excluded_timing_paths(&nl, &timing);
            let pattern = eda_sta::TimingObjectPattern::new("**/u1/y");
            if pattern.is_pattern()
                && pattern.matches("top/u1/y")
                && pattern.matches("a/b/c/u1/y")
                && !pattern.matches("top/u1/x")
                && paths.is_empty()
                && excluded.len() == 1
            {
                Ok(())
            } else {
                Err(eda_sta::excluded_timing_path_report(&nl, &excluded))
            }
        },
    );

    c.add("sta_wildcard_max_delay_matches_typed_endpoint", || {
        let nl = hierarchical_through_chain_netlist();
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_max_delay 1.0 -through [get_pins top/u?/y] -to [get_ports *]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let paths = eda_sta::constrained_timing_paths(&nl, &timing, 0);
        let Some(path) = paths.first() else {
            return Err("wildcard max-delay path missing".into());
        };
        let through_ok = constraints
            .max_delay_paths
            .first()
            .and_then(|limit| limit.through_groups.first())
            .and_then(|group| group.first())
            .map(|object| object.is_pattern() && object.matches_name("top/u1/y"))
            .unwrap_or(false);
        if through_ok
            && constraints
                .max_delay_paths
                .first()
                .and_then(|limit| limit.to_object.as_ref())
                .is_some_and(|object| object.is_pattern() && object.matches_name("y"))
            && (timing.max_delay_slack + 0.2).abs() < 1e-12
            && path.delay_limits.len() == 1
            && (path.delay_limits[0].slack_ns + 0.2).abs() < 1e-12
        {
            Ok(())
        } else {
            Err(format!("max={}", timing.max_delay_slack))
        }
    });

    c.add("sta_disable_timing_removes_only_selected_input_arc", || {
        let nl = sdc_netlist();
        let cell = nl
            .cells()
            .first()
            .ok_or_else(|| "synthesized netlist has no cells".to_string())?
            .name
            .clone();
        let constraints = eda_sta::parse_sdc(&format!(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_disable_timing -from [get_pins {cell}/a] -to [get_pins {cell}/y]
        "#
        ))?;
        let timing = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let y = nl.net_by_name("y").ok_or("missing y")?;
        let paths = timing.base.timing_paths(&nl, 0);
        if timing.base.disabled_timing_arcs == 1
            && timing.base.net_arrival(y) == Some(1.0)
            && paths.len() == 1
            && paths[0].launch == eda_sta::TimingLaunch::PrimaryInput("b".into())
            && paths[0].arcs.len() == 2
        {
            Ok(())
        } else {
            Err(format!(
                "disabled={}, y={:?}, paths={}",
                timing.base.disabled_timing_arcs,
                timing.base.net_arrival(y),
                paths.len()
            ))
        }
    });

    c.add("sta_disable_timing_removes_clk_to_q_launch", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_disable_timing -from [get_pins u1_dff/clk] -to [get_pins u1_dff/q]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
        let paths = eda_sta::constrained_timing_paths(nl, &timing, 0);
        let sequential_to_u2 = paths.iter().any(|path| {
            matches!(
                path.path.endpoint,
                eda_sta::TimingPathEndpoint::SequentialCell(_)
            ) && path
                .path
                .nets
                .last()
                .is_some_and(|&net| nl.net(net).name == "q1")
        });
        let output_q1 = paths
            .iter()
            .any(|path| path.path.endpoint == eda_sta::TimingPathEndpoint::OutputPort("q1".into()));
        if timing.base.disabled_timing_arcs == 1 && !sequential_to_u2 && !output_q1 {
            Ok(())
        } else {
            Err(format!(
                "disabled={}, paths={}",
                timing.base.disabled_timing_arcs,
                paths.len()
            ))
        }
    });

    c.add("sta_disable_timing_removes_selected_capture_check", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_disable_timing -to [get_pins u2_dff/d]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
        let paths = eda_sta::constrained_timing_paths(nl, &timing, 0);
        let capture_kept = paths.iter().any(|path| {
            matches!(
                path.path.endpoint,
                eda_sta::TimingPathEndpoint::SequentialCell(_)
            ) && path
                .path
                .nets
                .last()
                .is_some_and(|&net| nl.net(net).name == "q1")
        });
        let output_q1_kept = paths
            .iter()
            .any(|path| path.path.endpoint == eda_sta::TimingPathEndpoint::OutputPort("q1".into()));
        if timing.base.disabled_timing_arcs == 1 && !capture_kept && output_q1_kept {
            Ok(())
        } else {
            Err(format!(
                "disabled={}, paths={}",
                timing.base.disabled_timing_arcs,
                paths.len()
            ))
        }
    });

    c.add("sta_disable_timing_wildcard_matches_multiple_cells", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_disable_timing [get_pins *_dff/q]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
        let paths = timing.base.timing_paths(nl, 0);
        if timing.base.disabled_timing_arcs == 2
            && paths
                .iter()
                .all(|path| matches!(path.launch, eda_sta::TimingLaunch::PrimaryInput(_)))
        {
            Ok(())
        } else {
            Err(format!(
                "disabled={}, paths={}",
                timing.base.disabled_timing_arcs,
                paths.len()
            ))
        }
    });

    c.add("sta_case_analysis_dominating_and_removes_path", || {
        let nl = sdc_netlist();
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_case_analysis 0 [get_ports a]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let y = nl.net_by_name("y").ok_or("missing y")?;
        if timing.base.case_analysis_nets.len() == 2
            && !timing.base.case_analysis_nets["a"]
            && !timing.base.case_analysis_nets["y"]
            && timing.base.net_arrival(y).is_none()
            && timing.base.timing_paths(&nl, 0).is_empty()
            && eda_sta::constrained_report(&timing).contains("case constants: 2")
        {
            Ok(())
        } else {
            Err(eda_sta::constrained_report(&timing))
        }
    });

    c.add(
        "sta_case_analysis_propagates_through_inverter_chain",
        || {
            let nl = through_chain_netlist();
            let constraints = eda_sta::parse_sdc(
                r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_case_analysis 1 [get_ports a]
        "#,
            )?;
            let timing = eda_sta::analyze_with_sdc(&nl, &constraints)?;
            let y = nl.net_by_name("y").ok_or("missing y")?;
            if timing.base.case_analysis_nets.len() == 4
                && !timing.base.case_analysis_nets["n0"]
                && timing.base.case_analysis_nets["n1"]
                && !timing.base.case_analysis_nets["y"]
                && timing.base.net_arrival(y).is_none()
                && timing.base.timing_paths(&nl, 0).is_empty()
            {
                Ok(())
            } else {
                Err(format!("constants={:?}", timing.base.case_analysis_nets))
            }
        },
    );

    c.add(
        "sta_case_analysis_on_flop_output_removes_downstream_path",
        || {
            let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
            let nl = synth_cached(src);
            let constraints = eda_sta::parse_sdc(
                r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_case_analysis 0 [get_pins u1_dff/q]
        "#,
            )?;
            let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
            let paths = timing.base.timing_paths(nl, 0);
            let downstream_capture = paths.iter().any(|path| {
                matches!(
                    path.endpoint,
                    eda_sta::TimingPathEndpoint::SequentialCell(_)
                ) && path
                    .nets
                    .last()
                    .is_some_and(|&net| nl.net(net).name == "q1")
            });
            let q1_output = paths
                .iter()
                .any(|path| path.endpoint == eda_sta::TimingPathEndpoint::OutputPort("q1".into()));
            if timing.base.case_analysis_nets.contains_key("q1")
                && !timing.base.case_analysis_nets["q1"]
                && !downstream_capture
                && !q1_output
            {
                Ok(())
            } else {
                Err(format!("constants={:?}", timing.base.case_analysis_nets))
            }
        },
    );

    c.add("sta_case_analysis_wildcard_and_conflict_checks", || {
        let nl = through_chain_netlist();
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_case_analysis 1 [get_ports *]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let conflicting = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_case_analysis 1 [get_ports a]
            set_case_analysis 0 [get_nets a]
        "#,
        )?;
        if timing.base.case_analysis_nets.contains_key("a")
            && timing.base.case_analysis_nets["a"]
            && matches!(
                eda_sta::analyze_with_sdc(&nl, &conflicting),
                Err(error) if error.contains("conflicting set_case_analysis")
            )
        {
            Ok(())
        } else {
            Err("wildcard/conflict case analysis behavior mismatch".into())
        }
    });

    c.add(
        "sta_case_analysis_constant_clock_disables_sequential_timing",
        || {
            let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
            let nl = synth_cached(src);
            let constraints = eda_sta::parse_sdc(
                r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_case_analysis 1 [get_ports clk]
        "#,
            )?;
            let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
            let paths = timing.base.timing_paths(nl, 0);
            if timing.base.case_analysis_nets.contains_key("clk")
                && paths.is_empty()
                && timing.min_pulse_width_slack.is_infinite()
                && timing.reg2reg_slack.is_infinite()
                && timing.in2reg_slack.is_infinite()
            {
                Ok(())
            } else {
                Err(eda_sta::constrained_report(&timing))
            }
        },
    );

    c.add(
        "sta_incremental_input_delay_updates_only_affected_cone",
        || {
            let nl = incremental_branch_netlist();
            let old = eda_sta::parse_sdc(
                r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_input_delay 0.3 -clock clk [get_ports b]
        "#,
            )?;
            let mut new = old.clone();
            new.set_input_delay(eda_sta::PortDelay {
                port: "a".into(),
                clock: "clk".into(),
                delay: 1.0,
            })?;

            let previous = eda_sta::analyze_with_constraints(&nl, &old)?;
            let incremental = eda_sta::update_timing_for_input_delays(&nl, &previous, &new, ["a"])?;
            let full = eda_sta::analyze_with_constraints(&nl, &new)?;

            let y = nl.net_by_name("y").ok_or("missing y")?;
            let z = nl.net_by_name("z").ok_or("missing z")?;
            let unaffected_old = previous.net_arrival(z);
            if incremental.recomputed_nets.len() == 4
                && incremental.recomputed_cells.len() == 3
                && incremental.total_cells == 6
                && !incremental.recomputed_nets.contains("b")
                && !incremental.recomputed_nets.contains("z")
                && incremental.timing.net_arrival(y) == full.net_arrival(y)
                && incremental.timing.net_min_arrival(y) == full.net_min_arrival(y)
                && incremental.timing.net_arrival(z) == unaffected_old
                && (incremental.timing.critical_path_delay - 2.2).abs() < 1e-12
            {
                Ok(())
            } else {
                Err(format!(
                    "nets={:?}, cells={:?}, y={:?}, z={:?}",
                    incremental.recomputed_nets,
                    incremental.recomputed_cells,
                    incremental.timing.net_arrival(y),
                    incremental.timing.net_arrival(z)
                ))
            }
        },
    );

    c.add(
        "sta_incremental_input_delay_rebuilds_sequential_endpoints",
        || {
            let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
            let nl = synth_cached(src);
            let old = eda_sta::parse_sdc(
                r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports d]
        "#,
            )?;
            let mut new = old.clone();
            new.set_input_delay(eda_sta::PortDelay {
                port: "d".into(),
                clock: "clk".into(),
                delay: 1.1,
            })?;

            let previous = eda_sta::analyze_with_constraints(nl, &old)?;
            let incremental = eda_sta::update_timing_for_input_delays(nl, &previous, &new, ["d"])?;
            let full = eda_sta::analyze_with_constraints(nl, &new)?;
            let incremental_paths = incremental.timing.timing_paths(nl, 0);
            let full_paths = full.timing_paths(nl, 0);
            let same_arrivals =
                incremental_paths
                    .iter()
                    .zip(full_paths.iter())
                    .all(|(left, right)| {
                        left.endpoint == right.endpoint
                            && (left.arrival_ns - right.arrival_ns).abs() < 1e-12
                    });
            if incremental.timing.setup_endpoint_list().len() == full.setup_endpoint_list().len()
                && incremental.timing.hold_endpoint_list().len() == full.hold_endpoint_list().len()
                && (incremental.timing.in2reg_critical - full.in2reg_critical).abs() < 1e-12
                && (incremental.timing.in2reg_hold_critical - full.in2reg_hold_critical).abs()
                    < 1e-12
                && incremental_paths.len() == full_paths.len()
                && same_arrivals
            {
                Ok(())
            } else {
                Err(format!(
                    "incremental in2reg={}, full={}",
                    incremental.timing.in2reg_critical, full.in2reg_critical
                ))
            }
        },
    );

    c.add(
        "sta_incremental_cell_derate_updates_downstream_cone",
        || {
            let nl = incremental_branch_netlist();
            let old = eda_sta::parse_sdc("create_clock -name clk -period 4 [get_ports a]")?;
            let mut new = old.clone();
            new.set_cell_timing_derate("a_u1", Some(0.5), Some(2.0))?;

            let previous = eda_sta::analyze_with_constraints(&nl, &old)?;
            let incremental =
                eda_sta::update_timing_for_timing_derates(&nl, &previous, &new, ["a_u1"], &[])?;
            let full = eda_sta::analyze_with_constraints(&nl, &new)?;

            let y = nl.net_by_name("y").ok_or("missing y")?;
            let z = nl.net_by_name("z").ok_or("missing z")?;
            if incremental.recomputed_nets.len() == 2
                && incremental.recomputed_cells.len() == 2
                && incremental.recomputed_cells.contains("a_u1")
                && incremental.recomputed_cells.contains("a_u2")
                && !incremental.recomputed_cells.contains("a_u0")
                && incremental.timing.net_arrival(y) == full.net_arrival(y)
                && incremental.timing.net_min_arrival(y) == full.net_min_arrival(y)
                && incremental.timing.net_arrival(z) == previous.net_arrival(z)
                && (incremental.timing.net_arrival(y).unwrap_or(f64::NAN) - 1.6).abs() < 1e-12
            {
                Ok(())
            } else {
                Err(format!(
                    "nets={:?}, cells={:?}, y={:?}",
                    incremental.recomputed_nets,
                    incremental.recomputed_cells,
                    incremental.timing.net_arrival(y)
                ))
            }
        },
    );

    c.add(
        "sta_incremental_pin_derate_updates_sequential_launch",
        || {
            let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
            let nl = synth_cached(src);
            let old = eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports clk]")?;
            let mut new = old.clone();
            new.set_pin_timing_derate("u1_dff", "q", Some(0.5), Some(2.0))?;

            let previous = eda_sta::analyze_with_constraints(nl, &old)?;
            let pin = eda_sta::TimingPin::new("u1_dff", "q");
            let incremental = eda_sta::update_timing_for_timing_derates(
                nl,
                &previous,
                &new,
                Vec::<&str>::new(),
                &[pin],
            )?;
            let full = eda_sta::analyze_with_constraints(nl, &new)?;
            let incremental_paths = incremental.timing.timing_paths(nl, 0);
            let full_paths = full.timing_paths(nl, 0);
            let same_arrivals =
                incremental_paths
                    .iter()
                    .zip(full_paths.iter())
                    .all(|(left, right)| {
                        left.endpoint == right.endpoint
                            && (left.arrival_ns - right.arrival_ns).abs() < 1e-12
                    });
            if incremental.recomputed_cells.contains("u1_dff")
                && (incremental.timing.reg2reg_critical - full.reg2reg_critical).abs() < 1e-12
                && (incremental.timing.reg2reg_hold_critical - full.reg2reg_hold_critical).abs()
                    < 1e-12
                && incremental_paths.len() == full_paths.len()
                && same_arrivals
            {
                Ok(())
            } else {
                Err(format!(
                    "reg2reg={}, full={}",
                    incremental.timing.reg2reg_critical, full.reg2reg_critical
                ))
            }
        },
    );

    c.add(
        "sta_incremental_sdf_arc_update_matches_full_analysis",
        || {
            let old_nl = through_chain_netlist();
            let annotated = eda_sdf::annotate_sdf(
                &old_nl,
                &eda_sdf::parse_sdf(
                    r#"
                (DELAYFILE
                  (SDFVERSION "3.0")
                  (DESIGN "through_chain")
                  (TIMESCALE 1ns)
                  (CELL (CELLTYPE "INV") (INSTANCE u1)
                    (DELAY (ABSOLUTE (IOPATH A Y (1.0))))
                  )
                )
            "#,
                )?,
            )?;
            let constraints = eda_sta::parse_sdc("create_clock -name clk -period 4 [get_ports a]")?;
            let previous = eda_sta::analyze_with_constraints(&old_nl, &constraints)?;
            let incremental = eda_sta::update_timing_for_timing_derates(
                &annotated,
                &previous,
                &constraints,
                ["u1"],
                &[],
            )?;
            let full = eda_sta::analyze_with_constraints(&annotated, &constraints)?;
            let y = annotated.net_by_name("y").ok_or("missing y")?;
            if incremental.recomputed_cells.len() == 2
                && incremental.recomputed_cells.contains("u1")
                && incremental.recomputed_cells.contains("u2")
                && incremental.timing.net_arrival(y) == full.net_arrival(y)
                && incremental.timing.net_min_arrival(y) == full.net_min_arrival(y)
                && (incremental.timing.net_arrival(y).unwrap_or(f64::NAN) - 1.8).abs() < 1e-12
            {
                Ok(())
            } else {
                Err(format!(
                    "cells={:?}, y={:?}, full={:?}",
                    incremental.recomputed_cells,
                    incremental.timing.net_arrival(y),
                    full.net_arrival(y)
                ))
            }
        },
    );

    c.add(
        "sta_topology_incremental_update_after_buffer_insertion",
        || {
            let old = incremental_branch_netlist();
            let mut new = old.clone();
            let eco_net = new.add_net("a_eco_net");
            let target = new
                .cells()
                .iter()
                .position(|cell| cell.name == "a_u2")
                .ok_or("missing a_u2")?;
            let mut patched = new.cell(target).clone();
            patched.inputs[0] = eco_net;
            new.replace_cell(target, patched);
            let library = new.library().ok_or("missing library")?.clone();
            new.add_cell(Cell {
                name: "a_eco_buf".into(),
                kind: CellKind::Lib(library.find("BUF").unwrap()),
                inputs: vec![new.net_by_name("a_n1").ok_or("missing a_n1")?],
                output: eco_net,
            });

            let diff = eda_sta::diff_timing_topology(&old, &new);
            let constraints = eda_sta::parse_sdc("create_clock -name clk -period 4 [get_ports a]")?;
            let previous = eda_sta::analyze_with_constraints(&old, &constraints)?;
            let incremental =
                eda_sta::update_timing_for_topology(&old, &previous, &new, &constraints)?;
            let full = eda_sta::analyze_with_constraints(&new, &constraints)?;
            let y = new.net_by_name("y").ok_or("missing y")?;
            let z = new.net_by_name("z").ok_or("missing z")?;
            if diff.added_cells.contains("a_eco_buf")
                && diff.added_nets.contains("a_eco_net")
                && diff.changed_cells.contains("a_u2")
                && incremental.recomputed_cells.len() == 2
                && incremental.recomputed_cells.contains("a_eco_buf")
                && incremental.recomputed_cells.contains("a_u2")
                && !incremental.recomputed_cells.contains("a_u0")
                && incremental.timing.net_arrival(y) == full.net_arrival(y)
                && incremental.timing.net_min_arrival(y) == full.net_min_arrival(y)
                && incremental.timing.net_arrival(z) == previous.net_arrival(z)
                && (incremental.timing.net_arrival(y).unwrap_or(f64::NAN) - 2.0).abs() < 1e-12
            {
                Ok(())
            } else {
                Err(format!(
                    "diff={diff:?}, cells={:?}, y={:?}",
                    incremental.recomputed_cells,
                    incremental.timing.net_arrival(y)
                ))
            }
        },
    );

    c.add(
        "sta_topology_incremental_update_after_cell_replacement",
        || {
            let old = incremental_branch_netlist();
            let mut new = old.clone();
            let target = new
                .cells()
                .iter()
                .position(|cell| cell.name == "a_u1")
                .ok_or("missing a_u1")?;
            let mut patched = new.cell(target).clone();
            let library = new.library().ok_or("missing library")?.clone();
            patched.kind = CellKind::Lib(library.find("BUF").unwrap());
            new.replace_cell(target, patched);

            let diff = eda_sta::diff_timing_topology(&old, &new);
            let constraints = eda_sta::parse_sdc("create_clock -name clk -period 4 [get_ports a]")?;
            let previous = eda_sta::analyze_with_constraints(&old, &constraints)?;
            let incremental =
                eda_sta::update_timing_for_topology(&old, &previous, &new, &constraints)?;
            let full = eda_sta::analyze_with_constraints(&new, &constraints)?;
            let y = new.net_by_name("y").ok_or("missing y")?;
            if diff.changed_cells.contains("a_u1")
                && incremental.recomputed_cells.len() == 2
                && incremental.recomputed_cells.contains("a_u1")
                && incremental.recomputed_cells.contains("a_u2")
                && !incremental.recomputed_cells.contains("a_u0")
                && incremental.timing.net_arrival(y) == full.net_arrival(y)
                && (incremental.timing.net_arrival(y).unwrap_or(f64::NAN) - 1.6).abs() < 1e-12
            {
                Ok(())
            } else {
                Err(format!(
                    "diff={diff:?}, cells={:?}, y={:?}",
                    incremental.recomputed_cells,
                    incremental.timing.net_arrival(y)
                ))
            }
        },
    );

    c.add(
        "sta_constraint_incremental_input_delay_matches_full_analysis",
        || {
            let nl = through_chain_netlist();
            let old = eda_sta::parse_sdc(
                r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_input_delay 0.2 -clock clk [get_ports a]
        "#,
            )?;
            let mut new = old.clone();
            new.set_input_delay(eda_sta::PortDelay {
                port: "a".into(),
                clock: "clk".into(),
                delay: 1.0,
            })?;
            let previous = eda_sta::analyze_with_constraints(&nl, &old)?;
            let updated = eda_sta::update_timing_for_constraints(&nl, &previous, &old, &new)?;
            let full = eda_sta::analyze_with_sdc(&nl, &new)?;
            if updated.diff.changed_input_ports.contains("a")
                && !updated.full_base_recompute
                && updated.recomputed_cells.len() == 3
                && updated.timing.critical_path_delay == full.base.critical_path_delay
                && updated.constrained.worst_slack == full.worst_slack
            {
                Ok(())
            } else {
                Err(format!(
                    "cells={}, slack={}, full={}",
                    updated.recomputed_cells.len(),
                    updated.constrained.worst_slack,
                    full.worst_slack
                ))
            }
        },
    );

    c.add("sta_constraint_exception_reuses_base_arrivals", || {
        let nl = through_chain_netlist();
        let old = eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports a]")?;
        let new = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_false_path -to [get_ports y]
        "#,
        )?;
        let previous = eda_sta::analyze_with_constraints(&nl, &old)?;
        let updated = eda_sta::update_timing_for_constraints(&nl, &previous, &old, &new)?;
        if updated.diff.timing_exceptions_changed
            && !updated.diff.base_recompute_required
            && updated.recomputed_nets.is_empty()
            && updated.timing.critical_path_delay == previous.critical_path_delay
            && updated.constrained.output_slack.is_infinite()
        {
            Ok(())
        } else {
            Err(format!(
                "base={}, cells={}",
                updated.full_base_recompute,
                updated.recomputed_cells.len()
            ))
        }
    });

    c.add(
        "sta_constraint_case_analysis_change_forces_full_recompute",
        || {
            let nl = through_chain_netlist();
            let old = eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports a]")?;
            let new = eda_sta::parse_sdc(
                r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_case_analysis 1 [get_ports a]
        "#,
            )?;
            let previous = eda_sta::analyze_with_constraints(&nl, &old)?;
            let updated = eda_sta::update_timing_for_constraints(&nl, &previous, &old, &new)?;
            let full = eda_sta::analyze_with_sdc(&nl, &new)?;
            if updated.diff.case_analysis_changed
                && updated.full_base_recompute
                && updated.recomputed_cells.len() == nl.cell_count()
                && updated.timing.case_analysis_nets == full.base.case_analysis_nets
                && updated.timing.critical_path_delay == 0.0
            {
                Ok(())
            } else {
                Err(format!(
                    "full={}, cells={}, constants={:?}",
                    updated.full_base_recompute,
                    updated.recomputed_cells.len(),
                    updated.timing.case_analysis_nets
                ))
            }
        },
    );

    c.add(
        "sta_constraint_disable_timing_change_forces_full_recompute",
        || {
            let nl = through_chain_netlist();
            let old = eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports a]")?;
            let new = eda_sta::parse_sdc(
                r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_disable_timing -from [get_pins u1/a] -to [get_pins u1/y]
        "#,
            )?;
            let previous = eda_sta::analyze_with_constraints(&nl, &old)?;
            let updated = eda_sta::update_timing_for_constraints(&nl, &previous, &old, &new)?;
            let full = eda_sta::analyze_with_sdc(&nl, &new)?;
            if updated.diff.disabled_timing_changed
                && updated.full_base_recompute
                && updated.timing.disabled_timing_arcs == full.base.disabled_timing_arcs
                && updated.timing.critical_path_delay == full.base.critical_path_delay
            {
                Ok(())
            } else {
                Err(format!(
                    "full={}, disabled={}, full-disabled={}",
                    updated.full_base_recompute,
                    updated.timing.disabled_timing_arcs,
                    full.base.disabled_timing_arcs
                ))
            }
        },
    );

    c.add("sta_incremental_timing_rejects_bad_update_objects", || {
        let nl = incremental_branch_netlist();
        let constraints = eda_sta::parse_sdc("create_clock -name clk -period 4 [get_ports a]")?;
        let previous = eda_sta::analyze_with_constraints(&nl, &constraints)?;
        let unknown =
            eda_sta::update_timing_for_input_delays(&nl, &previous, &constraints, ["missing"]);
        let output = eda_sta::update_timing_for_input_delays(&nl, &previous, &constraints, ["y"]);
        let empty = eda_sta::update_timing_for_input_delays(
            &nl,
            &previous,
            &constraints,
            Vec::<&str>::new(),
        );
        let missing_cell = eda_sta::update_timing_for_timing_derates(
            &nl,
            &previous,
            &constraints,
            ["missing"],
            &[],
        );
        let missing_pin = eda_sta::update_timing_for_timing_derates(
            &nl,
            &previous,
            &constraints,
            Vec::<&str>::new(),
            &[eda_sta::TimingPin::new("a_u0", "missing")],
        );
        if matches!(unknown, Err(error) if error.contains("not found"))
            && matches!(output, Err(error) if error.contains("not an input"))
            && matches!(empty, Err(error) if error.contains("at least one"))
            && matches!(missing_cell, Err(error) if error.contains("cell 'missing' not found"))
            && matches!(missing_pin, Err(error) if error.contains("pin 'a_u0/missing' not found"))
        {
            Ok(())
        } else {
            Err("incremental STA object validation mismatch".into())
        }
    });

    c.add("sta_hold_timing_paths_reconstruct_early_paths", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let timing = eda_sta::analyze(nl)?;
        let paths = timing.hold_timing_paths(nl, 0.05, 0);
        let report = eda_sta::hold_timing_path_report(nl, &paths);
        if paths.len() == 2
            && paths
                .windows(2)
                .all(|pair| pair[0].slack_ns <= pair[1].slack_ns)
            && matches!(paths[0].path.launch, eda_sta::TimingLaunch::PrimaryInput(_))
            && matches!(
                paths[1].path.launch,
                eda_sta::TimingLaunch::SequentialCell(_)
            )
            && report.contains("Hold timing path report")
            && report.contains("hold slack")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("sta_hold_timing_path_arcs_use_min_arrivals", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let timing = eda_sta::analyze(nl)?;
        let paths = timing.hold_timing_paths(nl, 0.05, 0);
        let report = eda_sta::hold_timing_path_report(nl, &paths);
        let Some(hold) = paths
            .iter()
            .find(|path| matches!(path.path.launch, eda_sta::TimingLaunch::SequentialCell(_)))
        else {
            return Err("no sequential hold path".into());
        };
        let launch = hold.path.arcs.first().ok_or("no hold launch arc")?;
        let sum = hold.path.arcs.iter().map(|arc| arc.delay_ns).sum::<f64>();
        if launch.kind == eda_sta::TimingArcKind::ClockToOutput
            && (launch.delay_ns - 1.2).abs() < 1e-12
            && (sum - hold.path.arrival_ns).abs() < 1e-12
            && report.contains("hold slack")
            && report.contains("clock-to-output 1.200000 ns")
        {
            Ok(())
        } else {
            Err(format!(
                "sum={sum}, arrival={}\n{}",
                hold.path.arrival_ns, report
            ))
        }
    });

    c.add(
        "sta_constrained_hold_paths_include_same_edge_metadata",
        || {
            let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
            let nl = synth_cached(src);
            let constraints = eda_sta::parse_sdc(
                r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_clock_latency -source 0.1 [get_clocks clk]
            set_clock_latency 0.2 [get_clocks clk]
        "#,
            )?;
            let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
            let paths = eda_sta::constrained_hold_timing_paths(nl, &timing, 0);
            let report = eda_sta::constrained_hold_timing_path_report(nl, &paths);
            let sequential_ok = paths.iter().any(|path| {
                matches!(
                    path.path.path.launch,
                    eda_sta::TimingLaunch::SequentialCell(_)
                ) && (path.clocks.edge_separation_ns - 0.0).abs() < 1e-12
                    && (path.clocks.insertion_skew_ns - 0.0).abs() < 1e-12
            });
            let input_ok = paths.iter().any(|path| {
                matches!(
                    path.path.path.launch,
                    eda_sta::TimingLaunch::PrimaryInput(_)
                ) && (path.clocks.edge_separation_ns - 0.2).abs() < 1e-12
                    && (path.clocks.insertion_skew_ns - 0.2).abs() < 1e-12
            });
            if sequential_ok && input_ok && report.contains("edge separation 0.000000 ns") {
                Ok(())
            } else {
                Err(report)
            }
        },
    );

    c.add("sta_reset_timing_paths_recover_checks", || {
        let src: &'static str = "module m(input clk, d, rst, output reg q);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 1'b0; else q <= d;
             endmodule";
        let nl = synth_cached(src);
        let timing = eda_sta::analyze(nl)?;
        let paths = timing.reset_timing_paths(nl, 0);
        let report = eda_sta::reset_timing_path_report(nl, &paths);
        if paths.len() == 1
            && paths[0].recovery.arrival_ns == paths[0].removal.arrival_ns
            && paths[0].recovery.nets == paths[0].removal.nets
            && matches!(
                paths[0].recovery.launch,
                eda_sta::TimingLaunch::PrimaryInput(_)
            )
            && report.contains("recovery arrival")
            && report.contains("removal arrival")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("sta_reset_timing_paths_include_late_and_early_arcs", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("reset_arcs");
        nl.set_library(lib.clone());
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let rst = nl.add_net("rst");
        let rst_n0 = nl.add_net("rst_n0");
        let rst_n1 = nl.add_net("rst_n1");
        let q = nl.add_net("q");
        nl.add_cell(Cell {
            name: "reset_u0".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![rst],
            output: rst_n0,
        });
        nl.add_cell(Cell {
            name: "reset_u1".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![rst_n0],
            output: rst_n1,
        });
        nl.add_cell(Cell {
            name: "capture".into(),
            kind: CellKind::Lib(lib.find("DFFR").unwrap()),
            inputs: vec![clk, d, rst_n1],
            output: q,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("rst", Dir::In, vec![rst]);
        nl.add_port("q", Dir::Out, vec![q]);
        let timing = eda_sta::analyze(&nl)?;
        let paths = timing.reset_timing_paths(&nl, 1);
        let report = eda_sta::reset_timing_path_report(&nl, &paths);
        let Some(reset) = paths.first() else {
            return Err("no reset endpoint".into());
        };
        fn reset_arcs_ok(path: &eda_sta::TimingPath) -> bool {
            path.arcs.len() == 3
                && path.arcs[0].kind == eda_sta::TimingArcKind::PrimaryInputLaunch
                && path.arcs[1..].iter().all(|arc| {
                    arc.kind == eda_sta::TimingArcKind::Combinational
                        && (arc.delay_ns - 0.4).abs() < 1e-12
                })
                && (path.arcs.iter().map(|arc| arc.delay_ns).sum::<f64>() - path.arrival_ns).abs()
                    < 1e-12
        }
        if reset_arcs_ok(&reset.recovery)
            && reset_arcs_ok(&reset.removal)
            && reset
                .recovery
                .cells
                .first()
                .is_some_and(|&cell| nl.cell(cell).name == "reset_u0")
            && report.contains("recovery arrival 0.800000 ns")
            && report.contains("removal arrival 0.800000 ns")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("sta_constrained_reset_paths_include_clock_metadata", || {
        let src: &'static str = "module m(input clk, d, rst, output reg q);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 1'b0; else q <= d;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_clock_latency -source 0.1 [get_clocks clk]
            set_clock_latency 0.2 [get_clocks clk]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
        let paths = eda_sta::constrained_reset_timing_paths(nl, &timing, 0);
        let report = eda_sta::constrained_reset_timing_path_report(nl, &paths);
        let recovery = paths
            .iter()
            .find(|path| path.kind == eda_sta::ResetCheckKind::Recovery)
            .ok_or("missing recovery metadata")?;
        let removal = paths
            .iter()
            .find(|path| path.kind == eda_sta::ResetCheckKind::Removal)
            .ok_or("missing removal metadata")?;
        if (recovery.clocks.edge_separation_ns - 4.2).abs() < 1e-12
            && (recovery.clocks.insertion_skew_ns - 0.2).abs() < 1e-12
            && (removal.clocks.edge_separation_ns - 0.2).abs() < 1e-12
            && (removal.clocks.insertion_skew_ns - 0.2).abs() < 1e-12
            && report.contains("edge separation 4.200000 ns, insertion skew 0.200000 ns")
            && report.contains("edge separation 0.200000 ns, insertion skew 0.200000 ns")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("sta_false_path_reports_excluded_reset_checks", || {
        let src: &'static str = "module m(input clk, d, rst, output reg q);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 1'b0; else q <= d;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_false_path -to [get_ports rst]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
        let excluded = eda_sta::excluded_timing_paths(nl, &timing);
        let report = eda_sta::excluded_timing_path_report(nl, &excluded);
        if excluded.len() == 2
            && excluded
                .iter()
                .any(|path| path.check == eda_sta::ExcludedTimingCheck::Recovery)
            && excluded
                .iter()
                .any(|path| path.check == eda_sta::ExcludedTimingCheck::Removal)
            && report.contains("recovery flop")
            && report.contains("removal flop")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
}

fn signal_quality(c: &mut CaseList) {
    let src: &'static str = "module m(input a, b, c, output y); assign y = (a & b) & c; endmodule";
    c.add("sta_signal_quality_clean", move || {
        let nl = synth_cached(src);
        let result = eda_sta::analyze_quality(
            nl,
            &eda_sta::QualityOptions {
                max_transition_ns: Some(10.0),
                max_capacitance_pf: Some(10.0),
                max_fanout: Some(16),
                ..Default::default()
            },
        )?;
        if result.has_violations() {
            Err(eda_sta::quality_report(&result))
        } else if result.transitions_checked == 0 || result.capacitances_checked == 0 {
            Err(format!(
                "checks transition={} capacitance={}",
                result.transitions_checked, result.capacitances_checked
            ))
        } else {
            Ok(())
        }
    });

    c.add("sta_signal_quality_reports_violations", move || {
        let nl = synth_cached(src);
        let mut extra = std::collections::BTreeMap::new();
        let internal = ["n0", "g_n1", "w"]
            .iter()
            .find(|name| nl.net_by_name(name).is_some())
            .copied()
            .unwrap_or("y");
        extra.insert(internal.to_string(), 0.2);
        let result = eda_sta::analyze_quality(
            nl,
            &eda_sta::QualityOptions {
                max_transition_ns: Some(0.2),
                max_capacitance_pf: Some(0.1),
                max_fanout: Some(16),
                extra_capacitance_pf: extra,
                ..Default::default()
            },
        )?;
        let kinds = result
            .violations
            .iter()
            .map(|violation| violation.kind)
            .collect::<Vec<_>>();
        if !kinds.contains(&eda_sta::QualityKind::MaxTransition)
            || !kinds.contains(&eda_sta::QualityKind::MaxCapacitance)
        {
            Err(eda_sta::quality_report(&result))
        } else if !eda_sta::quality_report(&result).contains("TRANS-001")
            || !eda_sta::quality_report(&result).contains("CAP-001")
        {
            Err("quality report omitted violation codes".into())
        } else {
            Ok(())
        }
    });

    c.add("sta_signal_quality_rejects_bad_options", || {
        let options = eda_sta::QualityOptions {
            input_transition_ns: f64::NAN,
            ..Default::default()
        };
        match eda_sta::analyze_quality(
            synth_cached("module m(input a, output y); assign y = a; endmodule"),
            &options,
        ) {
            Err(e) if e.contains("input transition") => Ok(()),
            other => Err(format!("unexpected result: {:?}", other.is_ok())),
        }
    });
}

fn sdc_design_quality_rules(c: &mut CaseList) {
    let src: &'static str = "module m(input a, b, c, output y); assign y = (a & b) & c; endmodule";
    for index in 0_usize..100 {
        c.add(format!("sta_sdc_design_quality_{index:03}"), move || {
            let transition_limit = if index % 2 == 0 { 0.01 } else { 5.0 };
            let capacitance_limit = if index % 3 == 0 { 1e-9 } else { 5.0 };
            let fanout_limit = 1 + index % 16;
            let sdc = format!(
                "set_max_transition {transition_limit}\n\
                     set_max_capacitance {capacitance_limit}\n\
                     set_max_fanout {fanout_limit}"
            );
            let constraints = eda_api::parse_timing_constraints(&sdc)?;
            if constraints.max_transition_ns != Some(transition_limit)
                || constraints.max_capacitance_pf != Some(capacitance_limit)
                || constraints.max_fanout != Some(fanout_limit)
            {
                return Err(format!(
                    "parsed limits transition={:?} capacitance={:?} fanout={:?}",
                    constraints.max_transition_ns,
                    constraints.max_capacitance_pf,
                    constraints.max_fanout
                ));
            }
            let options = eda_api::quality_options_from_sdc(&constraints);
            options.validate()?;
            if options.max_transition_ns != Some(transition_limit)
                || options.max_capacitance_pf != Some(capacitance_limit)
                || options.max_fanout != Some(fanout_limit)
            {
                return Err("SDC quality options were not preserved".into());
            }
            let diff = eda_sta::diff_timing_constraints(
                &eda_sta::TimingConstraints::default(),
                &constraints,
            );
            if !diff.design_rules_changed || !diff.quality_recompute_required {
                return Err("SDC design-rule diff did not request quality recompute".into());
            }

            let nl = synth_cached(src);
            let mut analysis_options = options.clone();
            if capacitance_limit == 1e-9 {
                let internal = ["n0", "g_n1", "w"]
                    .iter()
                    .find(|name| nl.net_by_name(name).is_some())
                    .copied()
                    .unwrap_or("y");
                analysis_options
                    .extra_capacitance_pf
                    .insert(internal.to_string(), 0.1);
            }
            let result = eda_api::analyze_signal_quality(nl, &analysis_options)?;
            let has_transition_violation = result
                .violations
                .iter()
                .any(|violation| violation.kind == eda_sta::QualityKind::MaxTransition);
            let has_capacitance_violation = result
                .violations
                .iter()
                .any(|violation| violation.kind == eda_sta::QualityKind::MaxCapacitance);
            if has_transition_violation != (transition_limit == 0.01)
                || has_capacitance_violation != (capacitance_limit == 1e-9)
            {
                return Err(format!(
                    "transition violation={has_transition_violation}, \
                         capacitance violation={has_capacitance_violation}"
                ));
            }
            Ok(())
        });
    }
}

fn sdc_scoped_quality_rules(c: &mut CaseList) {
    let src: &'static str = "module m(input a, b, output y0, y1, y2); \
                             wire n = a & b; \
                             assign y0 = ~n; assign y1 = ~n; assign y2 = ~n; \
                             endmodule";
    for index in 0_usize..100 {
        c.add(format!("sta_sdc_scoped_quality_{index:03}"), move || {
            let variant = index % 4;
            let (accessor, target, kind) = match variant {
                0 => ("get_ports", "y0", eda_sta::TimingObjectKind::Port),
                1 => ("get_nets", "y0", eda_sta::TimingObjectKind::Net),
                2 => ("get_cells", "*", eda_sta::TimingObjectKind::Cell),
                _ => ("get_pins", "*/y", eda_sta::TimingObjectKind::Pin),
            };
            let tight_transition = index % 2 == 0;
            let transition_limit = if tight_transition { 0.001 } else { 10.0 };
            let capacitance_limit = if index % 3 == 0 { 0.001 } else { 10.0 };
            let fanout_limit = if index % 5 == 0 { 1 } else { 16 };
            let sdc = format!(
                "set_max_transition 10\n\
                     set_max_transition {transition_limit} [{accessor} {target}]\n\
                     set_max_capacitance 10\n\
                     set_max_capacitance {capacitance_limit} [{accessor} {target}]\n\
                     set_max_fanout 16\n\
                     set_max_fanout {fanout_limit} [get_nets *]"
            );
            let constraints = eda_api::parse_timing_constraints(&sdc)?;
            if constraints.max_transition_ns != Some(10.0)
                || constraints.max_capacitance_pf != Some(10.0)
                || constraints.max_fanout != Some(16)
            {
                return Err("design-wide quality fallback was not retained".into());
            }
            if constraints.scoped_max_transition_ns.len() != 1
                || constraints.scoped_max_transition_ns[0].object.kind != kind
                || constraints.scoped_max_transition_ns[0].object.name != target
                || constraints.scoped_max_transition_ns[0].limit != transition_limit
            {
                return Err(format!(
                    "scoped transition rules={:?}",
                    constraints.scoped_max_transition_ns
                ));
            }
            if constraints.scoped_max_capacitance_pf.len() != 1
                || constraints.scoped_max_capacitance_pf[0].limit != capacitance_limit
            {
                return Err(format!(
                    "scoped capacitance rules={:?}",
                    constraints.scoped_max_capacitance_pf
                ));
            }
            if constraints.scoped_max_fanout.len() != 1
                || constraints.scoped_max_fanout[0].limit != fanout_limit as f64
            {
                return Err(format!(
                    "scoped fanout rules={:?}",
                    constraints.scoped_max_fanout
                ));
            }

            let diff = eda_sta::diff_timing_constraints(
                &eda_sta::TimingConstraints::default(),
                &constraints,
            );
            if !diff.design_rules_changed || !diff.quality_recompute_required {
                return Err("scoped quality diff did not request recompute".into());
            }
            let mut options = eda_api::quality_options_from_sdc(&constraints);
            options.extra_capacitance_pf.insert("y0".to_string(), 0.1);
            options.validate()?;
            let result = eda_api::analyze_signal_quality(synth_cached(src), &options)?;
            let transition_hit = result
                .violations
                .iter()
                .any(|violation| violation.kind == eda_sta::QualityKind::MaxTransition);
            let capacitance_hit = result
                .violations
                .iter()
                .any(|violation| violation.kind == eda_sta::QualityKind::MaxCapacitance);
            let fanout_hit = result
                .violations
                .iter()
                .any(|violation| violation.kind == eda_sta::QualityKind::MaxFanout);
            if transition_hit != tight_transition
                || capacitance_hit != (capacitance_limit == 0.001)
                || fanout_hit != (fanout_limit == 1)
            {
                return Err(format!(
                    "transition={transition_hit} capacitance={capacitance_hit} \
                         fanout={fanout_hit}"
                ));
            }
            Ok(())
        });
    }
}

fn sdc_transition_load_rules(c: &mut CaseList) {
    let src: &'static str = "module m(input a, b, output y); assign y = a & b; endmodule";
    for index in 0_usize..100 {
        c.add(format!("sta_sdc_transition_load_{index:03}"), move || {
            let transition_variant = index % 4;
            let (transition_accessor, transition_target, transition_kind) = match transition_variant
            {
                0 => ("get_ports", "a", eda_sta::TimingObjectKind::Port),
                1 => ("get_nets", "a", eda_sta::TimingObjectKind::Net),
                2 => ("get_pins", "*/a", eda_sta::TimingObjectKind::Pin),
                _ => ("get_ports", "*", eda_sta::TimingObjectKind::Port),
            };
            let input_transition = if index % 2 == 0 { 0.01 } else { 0.50 };
            let load_variant = index % 3;
            let (load_accessor, load_target, load_kind) = match load_variant {
                0 => ("get_ports", "y", eda_sta::TimingObjectKind::Port),
                1 => ("get_nets", "y", eda_sta::TimingObjectKind::Net),
                _ => ("get_pins", "*/y", eda_sta::TimingObjectKind::Pin),
            };
            let load = if index % 5 == 0 { 0.02 } else { 0.0 };
            let sdc = format!(
                "set_max_transition 1.05\n\
                     set_input_transition {input_transition} \
                     [{transition_accessor} {transition_target}]\n\
                     set_max_capacitance 0.01\n\
                     set_load {load} [{load_accessor} {load_target}]"
            );
            let constraints = eda_api::parse_timing_constraints(&sdc)?;
            if constraints.input_transitions.len() != 1
                || constraints.input_transitions[0].object.kind != transition_kind
                || constraints.input_transitions[0].object.name != transition_target
                || constraints.input_transitions[0].limit != input_transition
            {
                return Err(format!(
                    "input transition rules={:?}",
                    constraints.input_transitions
                ));
            }
            if constraints.load_capacitances.len() != 1
                || constraints.load_capacitances[0].object.kind != load_kind
                || constraints.load_capacitances[0].object.name != load_target
                || constraints.load_capacitances[0].limit != load
            {
                return Err(format!("load rules={:?}", constraints.load_capacitances));
            }
            let diff = eda_sta::diff_timing_constraints(
                &eda_sta::TimingConstraints::default(),
                &constraints,
            );
            if !diff.input_transitions_changed
                || !diff.loads_changed
                || !diff.design_rules_changed
                || !diff.quality_recompute_required
            {
                return Err("transition/load diff did not request quality recompute".into());
            }

            let options = eda_api::quality_options_from_sdc(&constraints);
            options.validate()?;
            let result = eda_api::analyze_signal_quality(synth_cached(src), &options)?;
            let transition_violation = result
                .violations
                .iter()
                .any(|violation| violation.kind == eda_sta::QualityKind::MaxTransition);
            let load_violation = result
                .violations
                .iter()
                .any(|violation| violation.kind == eda_sta::QualityKind::MaxCapacitance);
            if transition_violation != (input_transition == 0.50)
                || load_violation != (load == 0.02)
            {
                return Err(format!(
                    "transition={transition_violation}, load={load_violation}"
                ));
            }
            Ok(())
        });
    }
}

fn sdc_clock_transition_rules(c: &mut CaseList) {
    let src: &'static str = "module m(input clk, divclk, d, output reg q); \
                             wire gclk = ~clk; \
                             always @(posedge gclk) q <= d; \
                             endmodule";
    for index in 0_usize..100 {
        c.add(format!("sta_sdc_clock_transition_{index:03}"), move || {
            let target_root = index % 2 == 0;
            let target = if target_root { "root" } else { "div" };
            let transition = 0.10 + (index % 10) as f64 / 100.0;
            let flag_variant = index % 5;
            let (flags, rise_set, fall_set, minimum_set, maximum_set) = match flag_variant {
                0 => (String::new(), true, true, true, true),
                1 => ("-rise ".to_string(), true, false, true, true),
                2 => ("-fall ".to_string(), false, true, true, true),
                3 => ("-min ".to_string(), true, true, true, false),
                _ => ("-max ".to_string(), true, true, false, true),
            };
            let sdc = format!(
                "create_clock -name root -period 4.0 [get_ports clk]\n\
                     create_generated_clock -name div -source [get_clocks root] \
                     -divide_by 2 -ports [get_ports divclk]\n\
                     set_clock_transition {flags}{transition} [get_clocks {target}]"
            );
            let constraints = eda_api::parse_timing_constraints(&sdc)?;
            let clock = &constraints.clocks[target];
            let expected_rise = (rise_set && maximum_set).then_some(transition);
            let expected_fall = (fall_set && maximum_set).then_some(transition);
            let expected_min_rise = (rise_set && minimum_set).then_some(transition);
            let expected_min_fall = (fall_set && minimum_set).then_some(transition);
            if clock.rise_transition_ns != expected_rise.unwrap_or(0.0)
                || clock.fall_transition_ns != expected_fall.unwrap_or(0.0)
                || clock.min_rise_transition_ns != expected_min_rise.unwrap_or(0.0)
                || clock.min_fall_transition_ns != expected_min_fall.unwrap_or(0.0)
            {
                return Err(format!(
                    "transition rise={} fall={} min-rise={} min-fall={}",
                    clock.rise_transition_ns,
                    clock.fall_transition_ns,
                    clock.min_rise_transition_ns,
                    clock.min_fall_transition_ns
                ));
            }
            let diff = eda_sta::diff_timing_constraints(
                &eda_sta::TimingConstraints::default(),
                &constraints,
            );
            if !diff.changed_clocks.contains(target) {
                return Err("clock transition change was not diffed".into());
            }

            let nl = synth_cached(src);
            let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
            let target_net = if target_root { "clk" } else { "divclk" };
            let net = nl
                .net_by_name(target_net)
                .ok_or_else(|| format!("missing clock net {target_net}"))?;
            let expected_maximum = if maximum_set { transition } else { 0.0 };
            let expected_minimum = if minimum_set { transition } else { 0.0 };
            if (timing.base.net_transition(net).unwrap_or(f64::NAN) - expected_maximum).abs()
                > 1e-12
                || (timing.base.net_min_transition(net).unwrap_or(f64::NAN) - expected_minimum)
                    .abs()
                    > 1e-12
            {
                return Err(format!(
                    "clock transition={:?}/{:?}, expected {expected_maximum}/{expected_minimum}",
                    timing.base.net_transition(net),
                    timing.base.net_min_transition(net)
                ));
            }
            if target_root {
                let gated = nl
                    .net_by_name("gclk")
                    .ok_or("missing buffered clock net gclk")?;
                let expected_gated = expected_maximum + 0.4;
                if (timing.base.net_transition(gated).unwrap_or(f64::NAN) - expected_gated).abs()
                    > 1e-12
                {
                    return Err(format!(
                        "gated transition={:?}, expected {expected_gated}",
                        timing.base.net_transition(gated)
                    ));
                }
            }
            Ok(())
        });
    }
}

fn sdc_clock_transition_min_max_checks(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sta_sdc_clock_minmax_{index:03}"), move || {
            let minimum = 0.01 + (index % 4) as f64 / 100.0;
            let maximum = 0.05 + (index % 4) as f64 / 100.0;
            let use_recovery = index % 4 == 3;
            let full_sdc = if use_recovery {
                format!(
                    "create_clock -name clk -period 2.0 [get_ports clk]\n\
                         set_input_delay 0.20 -clock clk [get_ports rst]\n\
                         set_clock_transition -min {minimum} [get_clocks clk]\n\
                         set_clock_transition -max {maximum} [get_clocks clk]"
                )
            } else {
                format!(
                    "create_clock -name clk -period 1.0 [get_ports clk]\n\
                         set_clock_transition -min {minimum} [get_clocks clk]\n\
                         set_clock_transition -max {maximum} [get_clocks clk]"
                )
            };
            let max_only_sdc = full_sdc.replace(
                &format!("set_clock_transition -min {minimum} [get_clocks clk]\n"),
                "",
            );
            let min_only_sdc = full_sdc.replace(
                &format!("set_clock_transition -max {maximum} [get_clocks clk]"),
                "set_clock_transition -max 0.0 [get_clocks clk]",
            );
            let full_constraints = eda_api::parse_timing_constraints(&full_sdc)?;
            let max_constraints = eda_api::parse_timing_constraints(&max_only_sdc)?;
            let min_constraints = eda_api::parse_timing_constraints(&min_only_sdc)?;

            if use_recovery {
                let nl = recovery_removal_lut_cached();
                let full = eda_api::constrained_timing(nl, &full_constraints)?;
                let max_reference = eda_api::constrained_timing(nl, &max_constraints)?;
                let min_reference = eda_api::constrained_timing(nl, &min_constraints)?;
                if (full.recovery_slack - max_reference.recovery_slack).abs() > 1e-12 {
                    return Err(format!(
                        "recovery full={} max_ref={}",
                        full.recovery_slack, max_reference.recovery_slack
                    ));
                }
                if (full.removal_slack - min_reference.removal_slack).abs() > 1e-12 {
                    return Err(format!(
                        "removal full={} min_ref={}",
                        full.removal_slack, min_reference.removal_slack
                    ));
                }
            } else {
                let nl = constraint_lut_cached();
                let full = eda_api::constrained_timing(nl, &full_constraints)?;
                let max_reference = eda_api::constrained_timing(nl, &max_constraints)?;
                let min_reference = eda_api::constrained_timing(nl, &min_constraints)?;
                let clk = nl.net_by_name("clk").ok_or("missing clock net")?;
                if (full.base.net_transition(clk).unwrap_or(f64::NAN) - maximum).abs() > 1e-12
                    || (full.base.net_min_transition(clk).unwrap_or(f64::NAN) - minimum).abs()
                        > 1e-12
                {
                    return Err("clock min/max transitions were not both propagated".into());
                }
                if (full.reg2reg_slack - max_reference.reg2reg_slack).abs() > 1e-12 {
                    return Err(format!(
                        "setup full={} max_ref={}",
                        full.reg2reg_slack, max_reference.reg2reg_slack
                    ));
                }
                if (full.reg2reg_hold_slack - min_reference.reg2reg_hold_slack).abs() > 1e-12 {
                    return Err(format!(
                        "hold full={} min_ref={}",
                        full.reg2reg_hold_slack, min_reference.reg2reg_hold_slack
                    ));
                }
            }
            Ok(())
        });
    }
}

fn sdc_netlist() -> Netlist {
    let src: &'static str = "module m(input a, b, output y); assign y = a & b; endmodule";
    let d = eda_hdl::compile(src, None).unwrap();
    eda_synth::synthesize(
        &d,
        &eda_synth::SynthOptions {
            optimize: true,
            map_lib: true,
        },
    )
    .unwrap()
    .netlist
}

fn through_chain_netlist() -> Netlist {
    let lib = Arc::new(CellLibrary::std());
    let mut nl = Netlist::new("through_chain");
    nl.set_library(lib.clone());
    let a = nl.add_net("a");
    let n0 = nl.add_net("n0");
    let n1 = nl.add_net("n1");
    let y = nl.add_net("y");
    for (name, input, output) in [("u0", a, n0), ("u1", n0, n1), ("u2", n1, y)] {
        nl.add_cell(Cell {
            name: name.into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![input],
            output,
        });
    }
    nl.add_port("a", Dir::In, vec![a]);
    nl.add_port("y", Dir::Out, vec![y]);
    nl
}

fn hierarchical_through_chain_netlist() -> Netlist {
    let lib = Arc::new(CellLibrary::std());
    let mut nl = Netlist::new("hier_through_chain");
    nl.set_library(lib.clone());
    let a = nl.add_net("a");
    let n0 = nl.add_net("n0");
    let n1 = nl.add_net("n1");
    let y = nl.add_net("y");
    for (name, input, output) in [("top/u0", a, n0), ("top/u1", n0, n1), ("top/u2", n1, y)] {
        nl.add_cell(Cell {
            name: name.into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![input],
            output,
        });
    }
    nl.add_port("a", Dir::In, vec![a]);
    nl.add_port("y", Dir::Out, vec![y]);
    nl
}

fn incremental_branch_netlist() -> Netlist {
    let lib = Arc::new(CellLibrary::std());
    let mut nl = Netlist::new("incremental_branches");
    nl.set_library(lib.clone());
    let a = nl.add_net("a");
    let b = nl.add_net("b");
    let a_n0 = nl.add_net("a_n0");
    let a_n1 = nl.add_net("a_n1");
    let b_n0 = nl.add_net("b_n0");
    let b_n1 = nl.add_net("b_n1");
    let y = nl.add_net("y");
    let z = nl.add_net("z");
    for (name, input, output) in [
        ("a_u0", a, a_n0),
        ("a_u1", a_n0, a_n1),
        ("a_u2", a_n1, y),
        ("b_u0", b, b_n0),
        ("b_u1", b_n0, b_n1),
        ("b_u2", b_n1, z),
    ] {
        nl.add_cell(Cell {
            name: name.into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![input],
            output,
        });
    }
    nl.add_port("a", Dir::In, vec![a]);
    nl.add_port("b", Dir::In, vec![b]);
    nl.add_port("y", Dir::Out, vec![y]);
    nl.add_port("z", Dir::Out, vec![z]);
    nl
}

fn register_sdc(c: &mut CaseList) {
    c.add("sdc_parse_common_commands", || {
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name core -period 4.0 [get_ports clk]
            set_clock_uncertainty 0.1 -setup [get_clocks core]
            set_input_delay 0.3 -clock core [get_ports a]
            set_output_delay 0.4 -clock core [get_ports y]
            set_false_path -from [get_ports b] -to [get_ports y]
            set_multicycle_path 2 -from [get_ports a] -to [get_ports y]
            set_timing_derate -early 0.9 -late 1.1
        "#,
        )?;
        if constraints.clocks.len() != 1
            || constraints.input_delays.len() != 1
            || constraints.output_delays.len() != 1
            || constraints.false_paths.len() != 1
            || constraints.multicycle_paths.len() != 1
            || constraints.timing_derate.early != 0.9
            || constraints.timing_derate.late != 1.1
        {
            Err("parsed SDC object counts".into())
        } else {
            Ok(())
        }
    });

    c.add("sdc_input_output_delay_slack", || {
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_clock_uncertainty 0.1 -setup [get_clocks clk]
            set_input_delay 0.3 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
        "#,
        )?;
        let nl = sdc_netlist();
        let r = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        if (r.output_slack - 0.2).abs() > 1e-9 {
            Err(format!("output slack {}", r.output_slack))
        } else {
            Ok(())
        }
    });
    c.add("sdc_timing_derate_scales_arrivals_and_report", || {
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_timing_derate -early 0.8 -late 1.2
        "#,
        )?;
        let nl = sdc_netlist();
        let r = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let report = eda_sta::constrained_report(&r);
        if (r.output_slack - 2.16).abs() > 1e-9
            || (r.early_derate - 0.8).abs() > 1e-9
            || (r.late_derate - 1.2).abs() > 1e-9
            || !report.contains("early derate 0.800")
            || !report.contains("late derate 1.200")
        {
            Err(format!("slack={} report={}", r.output_slack, report))
        } else {
            Ok(())
        }
    });
    c.add("sdc_cell_timing_derate_overrides_global", || {
        let nl = sdc_netlist();
        let cell = nl
            .cells()
            .first()
            .ok_or_else(|| "synthesized netlist has no cells".to_string())?
            .name
            .clone();
        let source = format!(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_timing_derate -early 0.9 -late 1.1
            set_timing_derate -cell_delay -early 0.5 -late 2.0 [get_cells {}]
        "#,
            cell
        );
        let constraints = eda_sta::parse_sdc(&source)?;
        let r = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let report = eda_sta::constrained_report(&r);
        if (r.output_slack - 1.38).abs() > 1e-9
            || r.cell_derates.len() != 1
            || (r.cell_derates[&cell].late - 2.0).abs() > 1e-9
            || !report.contains("cell derates   : 1")
        {
            Err(format!("slack={} report={}", r.output_slack, report))
        } else {
            Ok(())
        }
    });
    c.add("sdc_cell_timing_derate_rejects_bad_objects", || {
        if eda_sta::parse_sdc("set_timing_derate -early 1 [get_nets a]").is_ok()
            || eda_sta::parse_sdc(
                "set_timing_derate -cell_delay -cell_check -early 1 [get_cells u1]",
            )
            .is_ok()
        {
            Err("unsupported cell derate object accepted".into())
        } else {
            Ok(())
        }
    });
    c.add("sdc_pin_timing_derate_overrides_cell_arc", || {
        let nl = sdc_netlist();
        let cell = nl
            .cells()
            .first()
            .ok_or_else(|| "synthesized netlist has no cells".to_string())?
            .name
            .clone();
        let source = format!(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_timing_derate -cell_delay -late 1.2 [get_cells {}]
            set_timing_derate -cell_delay -early 0.5 -late 2.0 [get_pins {}/a]
        "#,
            cell, cell
        );
        let constraints = eda_sta::parse_sdc(&source)?;
        let r = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let report = eda_sta::constrained_report(&r);
        let pin = eda_sta::TimingPin::new(cell, "a");
        if (r.output_slack - 1.4).abs() > 1e-9
            || r.pin_derates.len() != 1
            || (r.pin_derates[&pin].late - 2.0).abs() > 1e-9
            || !report.contains("pin derates   : 1")
        {
            Err(format!("slack={} report={}", r.output_slack, report))
        } else {
            Ok(())
        }
    });
    c.add("sdc_pin_timing_derate_rejects_bad_objects", || {
        if eda_sta::parse_sdc("set_timing_derate -early 1 [get_pins u1]").is_ok() {
            Err("malformed pin derate object accepted".into())
        } else {
            Ok(())
        }
    });
    c.add("sdc_pin_check_derate_overrides_cell_check", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let capture = nl
            .cells()
            .last()
            .ok_or_else(|| "netlist has no cells".to_string())?
            .name
            .clone();
        let constraints = eda_sta::parse_sdc(&format!(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_timing_derate -cell_check -early 0.8 -late 1.2
            set_timing_derate -cell_check -early 0.7 -late 1.3 [get_cells {}]
            set_timing_derate -cell_check -early 0.5 -late 2.0 [get_pins {}/d]
        "#,
            capture, capture
        ))?;
        let r = eda_sta::analyze_with_sdc(nl, &constraints)?;
        let pin = eda_sta::TimingPin::new(capture, "d");
        if (r.reg2reg_slack - 1.6).abs() > 1e-9
            || (r.reg2reg_hold_slack - 1.1).abs() > 1e-9
            || r.pin_check_derates.len() != 1
            || (r.pin_check_derates[&pin].late - 2.0).abs() > 1e-9
        {
            Err(format!(
                "setup={} hold={}",
                r.reg2reg_slack, r.reg2reg_hold_slack
            ))
        } else {
            Ok(())
        }
    });
    c.add("sdc_output_pin_timing_derate_scales_all_arcs", || {
        let nl = sdc_netlist();
        let cell = nl
            .cells()
            .first()
            .ok_or_else(|| "synthesized netlist has no cells".to_string())?
            .name
            .clone();
        let source = format!(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_input_delay 0.3 -clock clk [get_ports b]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_timing_derate -late 2.0 [get_pins {}/y]
        "#,
            cell
        );
        let constraints = eda_sta::parse_sdc(&source)?;
        let r = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let pin = eda_sta::TimingPin::new(cell, "y");
        if (r.output_slack - 1.3).abs() > 1e-9
            || r.pin_derates.len() != 1
            || (r.pin_derates[&pin].late - 2.0).abs() > 1e-9
        {
            Err(format!("slack={}", r.output_slack))
        } else {
            Ok(())
        }
    });
    c.add("sdc_cell_check_derate_scales_sequential_checks", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let capture = nl
            .cells()
            .last()
            .ok_or_else(|| "netlist has no cells".to_string())?
            .name
            .clone();
        let source = format!(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_timing_derate -cell_check -early 0.5 -late 2.0 [get_cells {}]
        "#,
            capture
        );
        let constraints = eda_sta::parse_sdc(&source)?;
        let r = eda_sta::analyze_with_sdc(nl, &constraints)?;
        let report = eda_sta::constrained_report(&r);
        if (r.reg2reg_slack - 1.6).abs() > 1e-9
            || r.cell_check_derates.len() != 1
            || (r.cell_check_derates[&capture].late - 2.0).abs() > 1e-9
            || !report.contains("check derates  : 1")
        {
            Err(format!("reg2reg={} report={}", r.reg2reg_slack, report))
        } else {
            Ok(())
        }
    });
    c.add("sdc_cell_check_derate_scales_min_pulse_width", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_timing_derate -cell_check -early 0.5 -late 2.0
        "#,
        )?;
        let r = eda_sta::analyze_with_sdc(nl, &constraints)?;
        if (r.reg2reg_slack - 1.6).abs() > 1e-9
            || (r.min_pulse_high_slack - 1.5).abs() > 1e-9
            || (r.min_pulse_low_slack - 1.5).abs() > 1e-9
            || (r.min_pulse_width_slack - 1.5).abs() > 1e-9
        {
            Err(format!(
                "setup={} high={} low={}",
                r.reg2reg_slack, r.min_pulse_high_slack, r.min_pulse_low_slack
            ))
        } else {
            Ok(())
        }
    });
    c.add("sdc_pin_check_derate_scales_min_pulse_width", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let cells = nl
            .cells()
            .iter()
            .map(|cell| cell.name.clone())
            .collect::<Vec<_>>();
        if cells.len() < 2 {
            return Err("expected two sequential cells".into());
        }
        let constraints = eda_sta::parse_sdc(&format!(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_timing_derate -cell_check -late 2.0 [get_pins {0}/clk]
            set_timing_derate -cell_check -late 2.0 [get_pins {1}/clk]
        "#,
            cells[0], cells[1]
        ))?;
        let r = eda_sta::analyze_with_sdc(nl, &constraints)?;
        if (r.min_pulse_high_slack - 1.5).abs() > 1e-9
            || (r.min_pulse_low_slack - 1.5).abs() > 1e-9
            || r.pin_check_derates.len() != 2
        {
            Err(format!(
                "high={} low={}",
                r.min_pulse_high_slack, r.min_pulse_low_slack
            ))
        } else {
            Ok(())
        }
    });
    c.add("sdc_cell_check_derate_scales_recovery_removal", || {
        let src: &'static str = "module m(input clk, d, rst, output reg q);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 1'b0; else q <= d;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 1.2 -clock clk [get_ports rst]
            set_clock_uncertainty 0.1 -setup [get_clocks clk]
            set_clock_uncertainty 0.05 -hold [get_clocks clk]
            set_timing_derate -cell_check -early 0.5 -late 2.0
        "#,
        )?;
        let r = eda_sta::analyze_with_sdc(nl, &constraints)?;
        if (r.recovery_slack - 2.1).abs() > 1e-9
            || (r.removal_slack - 1.075).abs() > 1e-9
            || (r.async_reset_slack - 1.075).abs() > 1e-9
        {
            Err(format!(
                "recovery={} removal={} async={}",
                r.recovery_slack, r.removal_slack, r.async_reset_slack
            ))
        } else {
            Ok(())
        }
    });
    c.add("sdc_pin_check_derate_scales_recovery_removal", || {
        let src: &'static str = "module m(input clk, d, rst, output reg q);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 1'b0; else q <= d;
             endmodule";
        let nl = synth_cached(src);
        let cell = nl
            .cells()
            .last()
            .ok_or_else(|| "netlist has no cells".to_string())?
            .name
            .clone();
        let constraints = eda_sta::parse_sdc(&format!(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 1.2 -clock clk [get_ports rst]
            set_clock_uncertainty 0.1 -setup [get_clocks clk]
            set_clock_uncertainty 0.05 -hold [get_clocks clk]
            set_timing_derate -cell_check -early 0.5 -late 2.0 [get_pins {}/rst]
        "#,
            cell
        ))?;
        let r = eda_sta::analyze_with_sdc(nl, &constraints)?;
        if (r.recovery_slack - 2.1).abs() > 1e-9
            || (r.removal_slack - 1.075).abs() > 1e-9
            || r.pin_check_derates.len() != 1
        {
            Err(format!(
                "recovery={} removal={}",
                r.recovery_slack, r.removal_slack
            ))
        } else {
            Ok(())
        }
    });
    c.add("sdc_port_timing_derate_scales_external_delays", || {
        let nl = sdc_netlist();
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_timing_derate -early 0.5 -late 2.0 [get_ports a]
            set_timing_derate -late 2.0 [get_ports y]
        "#,
        )?;
        let r = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let report = eda_sta::constrained_report(&r);
        if (r.output_slack - 1.8).abs() > 1e-9
            || r.port_derates.len() != 2
            || (r.port_derates["a"].early - 0.5).abs() > 1e-9
            || (r.port_derates["y"].late - 2.0).abs() > 1e-9
            || !report.contains("port derates   : 2")
        {
            Err(format!("slack={} report={}", r.output_slack, report))
        } else {
            Ok(())
        }
    });
    c.add("sdc_port_timing_derate_rejects_bad_objects", || {
        if eda_sta::parse_sdc("set_timing_derate -cell_delay -late 2 [get_ports a]").is_ok()
            || eda_sta::parse_sdc("set_timing_derate -late 2 [get_nets n1]").is_ok()
        {
            Err("unsupported port derate object accepted".into())
        } else {
            Ok(())
        }
    });
    c.add("sta_multicorner_aggregates_worst_scenario", || {
        let nl = sdc_netlist();
        let slow = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
        "#,
        )?;
        let fast = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
        "#,
        )?;
        let result = eda_sta::analyze_timing_scenarios([
            eda_sta::TimingScenario::new("slow", &nl, &slow),
            eda_sta::TimingScenario::new("fast", &nl, &fast),
        ])?;
        let report = eda_sta::multi_corner_report(&result);
        if result.corners.len() != 2
            || result.worst_slack_corner != 1
            || result.worst_scenario() != Some("fast")
            || (result.corners[0].timing.output_slack - 2.4).abs() > 1e-9
            || (result.corners[1].timing.output_slack - 0.4).abs() > 1e-9
            || !report.contains("scenario 'fast' (WORST)")
        {
            Err(report)
        } else {
            Ok(())
        }
    });
    c.add("sta_multicorner_timing_paths_merge_setup_endpoints", || {
        let nl = sdc_netlist();
        let slow = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
        "#,
        )?;
        let fast = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
        "#,
        )?;
        let views = [
            eda_sta::TimingScenario::new("slow", &nl, &slow),
            eda_sta::TimingScenario::new("fast", &nl, &fast),
        ];
        let result = eda_sta::analyze_timing_scenarios(views)?;
        let paths = eda_sta::multi_corner_setup_timing_paths(&result, views, 0)?;
        let report = eda_sta::multi_corner_setup_timing_path_report(&paths);
        if paths.len() == 1
            && paths[0].endpoint == eda_sta::TimingPathEndpoint::OutputPort("y".into())
            && paths[0].scenario == "fast"
            && paths[0].corner_slacks.len() == 2
            && paths[0]
                .corner_slacks
                .iter()
                .all(|slack| slack.slack_ns.is_some())
            && report.contains("Multi-corner setup timing path report: 1 endpoint(s)")
            && report.contains("output y: worst scenario 'fast'")
            && report.contains("scenario slack:")
            && report.contains("arcs:")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add(
        "sta_multicorner_incremental_update_reuses_unchanged_corner",
        || {
            let nl = sdc_netlist();
            let slow = eda_sta::parse_sdc(
                r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
        "#,
            )?;
            let fast = eda_sta::parse_sdc(
                r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
        "#,
            )?;
            let old = vec![
                eda_sta::TimingScenarioDefinition::new("slow", slow.clone()),
                eda_sta::TimingScenarioDefinition::new("fast", fast.clone()),
            ];
            let mut new_slow_constraints = slow.clone();
            new_slow_constraints.set_input_delay(eda_sta::PortDelay {
                port: "a".into(),
                clock: "clk".into(),
                delay: 0.8,
            })?;
            let new = vec![
                eda_sta::TimingScenarioDefinition::new("slow", new_slow_constraints),
                eda_sta::TimingScenarioDefinition::new("fast", fast),
            ];

            let previous = eda_sta::analyze_timing_scenario_definitions(&nl, &old)?;
            let diff = eda_sta::diff_timing_scenarios(&old, &new)?;
            let updated =
                eda_sta::update_timing_scenarios_incrementally(&nl, &previous, &old, &new)?;
            let full = eda_sta::analyze_timing_scenario_definitions(&nl, &new)?;
            let same_slacks =
                updated
                    .result
                    .corners
                    .iter()
                    .zip(full.corners.iter())
                    .all(|(left, right)| {
                        left.scenario == right.scenario
                            && (left.timing.worst_slack - right.timing.worst_slack).abs() < 1e-12
                    });
            if diff.changed.contains("slow")
                && diff.constraints["slow"].changed_input_ports.contains("a")
                && updated.reused.contains("fast")
                && updated.updates.len() == 1
                && updated.updates["slow"].recomputed_cells.len() == 1
                && updated.result.corners.len() == 2
                && same_slacks
            {
                Ok(())
            } else {
                Err(eda_sta::multi_corner_report(&updated.result))
            }
        },
    );
    c.add(
        "sta_multicorner_incremental_update_adds_and_removes_scenarios",
        || {
            let nl = sdc_netlist();
            let slow = eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports clk]")?;
            let fast = eda_sta::parse_sdc("create_clock -name clk -period 2.0 [get_ports clk]")?;
            let old = vec![eda_sta::TimingScenarioDefinition::new("slow", slow)];
            let new = vec![eda_sta::TimingScenarioDefinition::new("fast", fast)];
            let previous = eda_sta::analyze_timing_scenario_definitions(&nl, &old)?;
            let diff = eda_sta::diff_timing_scenarios(&old, &new)?;
            let updated =
                eda_sta::update_timing_scenarios_incrementally(&nl, &previous, &old, &new)?;
            if diff.removed.contains("slow")
                && diff.added.contains("fast")
                && updated.result.corners.len() == 1
                && updated.result.corners[0].scenario == "fast"
                && updated.reused.is_empty()
                && updated.updates.is_empty()
            {
                Ok(())
            } else {
                Err(eda_sta::multi_corner_report(&updated.result))
            }
        },
    );
    c.add(
        "sta_multicorner_topology_incremental_updates_one_netlist_view",
        || {
            let old_nl = through_chain_netlist();
            let annotated = eda_sdf::annotate_sdf(
                &old_nl,
                &eda_sdf::parse_sdf(
                    r#"
                (DELAYFILE
                  (SDFVERSION "3.0")
                  (DESIGN "through_chain")
                  (TIMESCALE 1ns)
                  (CELL (CELLTYPE "INV") (INSTANCE u1)
                    (DELAY (ABSOLUTE (IOPATH A Y (1.0))))
                  )
                )
            "#,
                )?,
            )?;
            let slow = eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports a]")?;
            let fast = eda_sta::parse_sdc("create_clock -name clk -period 2.0 [get_ports a]")?;
            let old_views = [
                eda_sta::TimingScenario::new("slow", &old_nl, &slow),
                eda_sta::TimingScenario::new("fast", &old_nl, &fast),
            ];
            let new_views = [
                eda_sta::TimingScenario::new("slow", &old_nl, &slow),
                eda_sta::TimingScenario::new("fast", &annotated, &fast),
            ];
            let previous = eda_sta::analyze_timing_scenarios(old_views)?;
            let diff = eda_sta::diff_timing_scenario_sets(&old_views, &new_views)?;
            let updated = eda_sta::update_timing_scenarios_with_topology_incrementally(
                &previous, &old_views, &new_views,
            )?;
            let full = eda_sta::analyze_timing_scenarios(new_views)?;
            let same =
                updated
                    .result
                    .corners
                    .iter()
                    .zip(full.corners.iter())
                    .all(|(left, right)| {
                        left.scenario == right.scenario
                            && (left.timing.worst_slack - right.timing.worst_slack).abs() < 1e-12
                    });
            let fast_update = updated.updates.get("fast").ok_or("missing fast update")?;
            if diff.changed_netlists.contains("fast")
                && !diff.changed_netlists.contains("slow")
                && updated.reused.contains("slow")
                && fast_update.kind == eda_sta::MultiCornerScenarioUpdateKind::TopologyIncremental
                && fast_update
                    .topology
                    .as_ref()
                    .is_some_and(|topology| topology.diff.changed_cells.contains("u1"))
                && same
            {
                Ok(())
            } else {
                Err(eda_sta::multi_corner_report(&updated.result))
            }
        },
    );

    c.add(
        "sta_multicorner_topology_and_constraint_incremental_update",
        || {
            let old_nl = through_chain_netlist();
            let annotated = eda_sdf::annotate_sdf(
                &old_nl,
                &eda_sdf::parse_sdf(
                    r#"
                (DELAYFILE
                  (SDFVERSION "3.0")
                  (DESIGN "through_chain")
                  (TIMESCALE 1ns)
                  (CELL (CELLTYPE "INV") (INSTANCE u2)
                    (DELAY (ABSOLUTE (IOPATH A Y (1.2))))
                  )
                )
            "#,
                )?,
            )?;
            let old_constraints =
                eda_sta::parse_sdc("create_clock -name clk -period 2.0 [get_ports a]")?;
            let mut new_constraints = old_constraints.clone();
            new_constraints.set_input_delay(eda_sta::PortDelay {
                port: "a".into(),
                clock: "clk".into(),
                delay: 0.4,
            })?;
            let old_view = eda_sta::TimingScenario::new("fast", &old_nl, &old_constraints);
            let new_view = eda_sta::TimingScenario::new("fast", &annotated, &new_constraints);
            let previous = eda_sta::analyze_timing_scenarios([old_view])?;
            let updated = eda_sta::update_timing_scenarios_with_topology_incrementally(
                &previous,
                &[old_view],
                &[new_view],
            )?;
            let full = eda_sta::analyze_timing_scenarios([new_view])?;
            let update = updated.updates.get("fast").ok_or("missing update")?;
            if update.kind
                == eda_sta::MultiCornerScenarioUpdateKind::TopologyAndConstraintIncremental
                && update.topology.is_some()
                && update.constraint.is_some()
                && (updated.result.corners[0].timing.worst_slack
                    - full.corners[0].timing.worst_slack)
                    .abs()
                    < 1e-12
            {
                Ok(())
            } else {
                Err(eda_sta::multi_corner_report(&updated.result))
            }
        },
    );

    c.add("sta_violation_store_tracks_lifecycle", || {
        let nl = through_chain_netlist();
        let failing = eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
        let worse = eda_sta::parse_sdc("create_clock -name clk -period 0.5 [get_ports a]")?;
        let passing = eda_sta::parse_sdc("create_clock -name clk -period 2.0 [get_ports a]")?;

        let failing_timing = eda_sta::analyze_with_sdc(&nl, &failing)?;
        let mut store = eda_sta::TimingViolationStore::from_timing("func", &nl, &failing_timing)?;
        let violation_id = "func:output:y".to_string();
        let worse_timing = eda_sta::analyze_with_sdc(&nl, &worse)?;
        let degraded = store.update(&nl, &worse_timing)?;
        let passing_timing = eda_sta::analyze_with_sdc(&nl, &passing)?;
        let removed = store.update(&nl, &passing_timing)?;
        if store.generation == 3
            && store.is_empty()
            && degraded.degraded.contains_key(&violation_id)
            && (degraded.degraded[&violation_id].old_slack_ns + 0.2).abs() < 1e-12
            && (degraded.degraded[&violation_id].new_slack_ns + 0.7).abs() < 1e-12
            && removed.removed.contains_key(&violation_id)
            && eda_sta::timing_violation_report(&store, Some(&removed))
                .contains("lifecycle: added=0, removed=1")
        {
            Ok(())
        } else {
            Err("violation lifecycle mismatch".into())
        }
    });

    c.add("sta_violation_store_survives_eco_with_stable_id", || {
        let old = through_chain_netlist();
        let mut new = old.clone();
        let eco_net = new.add_net("eco_n1");
        let target = new
            .cells()
            .iter()
            .position(|cell| cell.name == "u2")
            .ok_or("missing u2")?;
        let mut patched = new.cell(target).clone();
        patched.inputs[0] = eco_net;
        new.replace_cell(target, patched);
        let library = new.library().ok_or("missing library")?.clone();
        new.add_cell(Cell {
            name: "eco_buf".into(),
            kind: CellKind::Lib(library.find("BUF").unwrap()),
            inputs: vec![new.net_by_name("n0").ok_or("missing n0")?],
            output: eco_net,
        });
        let constraints = eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
        let old_timing = eda_sta::analyze_with_sdc(&old, &constraints)?;
        let new_timing = eda_sta::analyze_with_sdc(&new, &constraints)?;
        let mut store = eda_sta::TimingViolationStore::from_timing("eco", &old, &old_timing)?;
        let diff = store.update(&new, &new_timing)?;
        let id = "eco:output:y".to_string();
        if store.violations().contains_key(&id)
            && diff.degraded.contains_key(&id)
            && (new_timing.worst_slack - old_timing.worst_slack).abs() > 1e-12
            && store.violations()[&id]
                .cells
                .iter()
                .any(|cell| cell == "eco_buf")
        {
            Ok(())
        } else {
            Err(eda_sta::timing_violation_report(&store, Some(&diff)))
        }
    });

    c.add("sta_violation_store_tracks_hold_and_reset_checks", || {
        let src: &'static str = "module m(input clk, d, rst, output reg q);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 1'b0; else q <= d;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_clock_uncertainty 0.3 -hold [get_clocks clk]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
        let store = eda_sta::TimingViolationStore::from_timing("checks", nl, &timing)?;
        let hold_id = "checks:hold:u1_dffr".to_string();
        let removal_id = "checks:removal:u1_dffr".to_string();
        if store.violations().contains_key(&hold_id)
            && store.violations()[&hold_id].kind == eda_sta::TimingViolationKind::Hold
            && (store.violations()[&hold_id].slack_ns + 0.5).abs() < 1e-12
            && store.violations().contains_key(&removal_id)
            && store.violations()[&removal_id].kind == eda_sta::TimingViolationKind::Removal
            && (store.violations()[&removal_id].slack_ns + 0.45).abs() < 1e-12
        {
            Ok(())
        } else {
            Err(eda_sta::timing_violation_report(&store, None))
        }
    });

    c.add(
        "sta_multicorner_violation_registry_aggregates_worst_corner",
        || {
            let nl = through_chain_netlist();
            let slow = eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports a]")?;
            let fast = eda_sta::parse_sdc("create_clock -name clk -period 0.5 [get_ports a]")?;
            let fast_fixed =
                eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports a]")?;
            let slow_view = eda_sta::TimingScenario::new("slow", &nl, &slow);
            let fast_view = eda_sta::TimingScenario::new("fast", &nl, &fast);
            let mut registry =
                eda_sta::TimingViolationRegistry::from_scenarios([slow_view, fast_view])?;
            let key = "output:y".to_string();
            let initial_summary = registry
                .summaries()
                .get(&key)
                .ok_or("missing summary")?
                .clone();
            let updated = registry.replace_scenarios([
                slow_view,
                eda_sta::TimingScenario::new("fast", &nl, &fast_fixed),
            ])?;
            if registry.generation() == 2
                && registry.stores().len() == 2
                && registry.stores()["fast"].is_empty()
                && initial_summary.worst_scenario == "fast"
                && (initial_summary.worst_slack_ns + 0.7).abs() < 1e-12
                && initial_summary.scenarios.len() == 1
                && updated.cross_corner.removed.contains_key(&key)
                && updated.scenario_diffs["fast"]
                    .removed
                    .contains_key("fast:output:y")
                && eda_sta::timing_violation_registry_report(&registry, Some(&updated))
                    .contains("cross-corner: added=0, removed=1")
            {
                Ok(())
            } else {
                Err(eda_sta::timing_violation_registry_report(
                    &registry,
                    Some(&updated),
                ))
            }
        },
    );

    c.add(
        "sta_multicorner_registry_distributes_violations_by_hierarchy",
        || {
            let src: &'static str = "module inv(input a, output y); assign y = ~a; endmodule
                module sub(input a, output y); inv u_leaf(.a(a), .y(y)); endmodule
                module top(input a, output y1, y2);
                    sub u1(.a(a), .y(y1));
                    sub u2(.a(a), .y(y2));
                endmodule";
            let nl = synth_cached(src);
            let slow = eda_sta::parse_sdc(
                r#"
                create_clock -name clk -period 2.0 [get_ports a]
                set_output_delay 0.2 -clock clk [get_ports y1]
                set_output_delay 0.2 -clock clk [get_ports y2]
            "#,
            )?;
            let fast = eda_sta::parse_sdc(
                r#"
                create_clock -name clk -period 0.1 [get_ports a]
                set_output_delay 0.2 -clock clk [get_ports y1]
                set_output_delay 0.2 -clock clk [get_ports y2]
            "#,
            )?;
            let registry = eda_sta::TimingViolationRegistry::from_scenarios([
                eda_sta::TimingScenario::new("slow", nl, &slow),
                eda_sta::TimingScenario::new("fast", nl, &fast),
            ])?;
            let first = registry
                .summaries()
                .get("output:y1")
                .ok_or("missing y1 cross-corner summary")?;
            let second = registry
                .summaries()
                .get("output:y2")
                .ok_or("missing y2 cross-corner summary")?;
            let first_hierarchy = Some("top.u1.u_leaf".to_string());
            let second_hierarchy = Some("top.u2.u_leaf".to_string());
            let first_owner = first
                .hierarchy_summary
                .get(&first_hierarchy)
                .ok_or("missing y1 hierarchy summary")?;
            let second_owner = second
                .hierarchy_summary
                .get(&second_hierarchy)
                .ok_or("missing y2 hierarchy summary")?;
            let registry_hierarchies = registry.hierarchy_summaries();
            if registry.stores()["slow"].is_empty()
                && registry.stores()["fast"].len() == 2
                && first.scenarios.len() == 1
                && second.scenarios.len() == 1
                && first_owner.worst_scenario == "fast"
                && second_owner.worst_scenario == "fast"
                && first_owner.violations == 1
                && second_owner.violations == 1
                && first_owner
                    .kinds
                    .contains_key(&eda_sta::TimingViolationKind::Output)
                && registry_hierarchies.len() == 2
                && registry_hierarchies.contains_key(&first_hierarchy)
                && registry_hierarchies.contains_key(&second_hierarchy)
            {
                let report = eda_sta::timing_violation_registry_report(&registry, None);
                if report.contains("hierarchy distribution:")
                    && report.contains("'top.u1.u_leaf': violations=1")
                    && report.contains("'top.u2.u_leaf': violations=1")
                {
                    Ok(())
                } else {
                    Err(report)
                }
            } else {
                Err(eda_sta::timing_violation_registry_report(&registry, None))
            }
        },
    );

    c.add("sta_block_timing_model_extracts_boundary_paths", || {
        let src: &'static str = "module boundary(input clk, d, output y);
                reg q;
                always @(posedge clk) q <= d;
                assign y = ~q;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_input_delay 0.3 -clock clk [get_ports d]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_clock_uncertainty 0.1 -setup [get_clocks clk]
            set_clock_uncertainty 0.05 -hold [get_clocks clk]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
        let model = eda_sta::block_timing_model(nl, &timing)?;
        let setup_paths = eda_sta::constrained_timing_paths(nl, &timing, 0);
        let hold_paths = eda_sta::constrained_hold_timing_paths(nl, &timing, 0);
        let min_paths = timing.base.min_timing_paths(nl, 0);
        let expected_input_setup = setup_paths
            .iter()
            .filter(|path| {
                matches!(&path.path.launch, eda_sta::TimingLaunch::PrimaryInput(name) if name == "d")
            })
            .map(|path| (path.required_ns, path.slack_ns))
            .reduce(|left, right| (left.0.min(right.0), left.1.min(right.1)))
            .ok_or("missing input setup boundary path")?;
        let expected_input_hold = hold_paths
            .iter()
            .filter(|path| {
                matches!(&path.path.path.launch, eda_sta::TimingLaunch::PrimaryInput(name) if name == "d")
            })
            .map(|path| (path.required_ns, path.slack_ns))
            .reduce(|left, right| (left.0.max(right.0), left.1.min(right.1)))
            .ok_or("missing input hold boundary path")?;
        let expected_output = setup_paths
            .iter()
            .filter(|path| {
                matches!(&path.path.endpoint, eda_sta::TimingPathEndpoint::OutputPort(name) if name == "y")
            })
            .map(|path| (path.path.arrival_ns, path.slack_ns))
            .reduce(|left, right| (left.0.max(right.0), left.1.min(right.1)))
            .ok_or("missing output boundary path")?;
        let expected_early_output = min_paths
            .iter()
            .filter(|path| {
                matches!(&path.endpoint, eda_sta::TimingPathEndpoint::OutputPort(name) if name == "y")
            })
            .map(|path| path.arrival_ns)
            .reduce(f64::min)
            .ok_or("missing output early boundary path")?;
        let input = model
            .ports
            .get("d")
            .ok_or("block model omitted input d")?;
        let output = model
            .ports
            .get("y")
            .ok_or("block model omitted output y")?;
        let report = eda_sta::block_timing_model_report(&model);
        if model.module == "boundary"
            && model.clock == "clk"
            && (model.period_ns - 2.0).abs() < 1e-12
            && model.internal_paths == 0
            && input.clock.as_deref() == Some("clk")
            && input.direction == eda_sta::BlockTimingDirection::Input
            && (input.late_required_ns.unwrap() - expected_input_setup.0).abs() < 1e-12
            && (input.setup_slack_ns.unwrap() - expected_input_setup.1).abs() < 1e-12
            && (input.early_required_ns.unwrap() - expected_input_hold.0).abs() < 1e-12
            && (input.hold_slack_ns.unwrap() - expected_input_hold.1).abs() < 1e-12
            && output.clock.as_deref() == Some("clk")
            && output.direction == eda_sta::BlockTimingDirection::Output
            && (output.late_arrival_ns.unwrap() - expected_output.0).abs() < 1e-12
            && (output.early_arrival_ns.unwrap() - expected_early_output).abs() < 1e-12
            && (output.setup_slack_ns.unwrap() - expected_output.1).abs() < 1e-12
            && report.contains("Block timing model for 'boundary'")
            && report.contains("input 'd'")
            && report.contains("output 'y'")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("sta_block_timing_model_serialization_round_trip", || {
        let model = eda_sta::BlockTimingModel {
            module: "sub\\block".into(),
            clock: "clk".into(),
            period_ns: 2.5,
            ports: [(
                "d".to_string(),
                eda_sta::BlockTimingPort {
                    name: "d".into(),
                    direction: eda_sta::BlockTimingDirection::Input,
                    clock: Some("clk".into()),
                    late_arrival_ns: None,
                    early_arrival_ns: None,
                    late_required_ns: Some(1.25),
                    setup_slack_ns: Some(0.25),
                    early_required_ns: Some(0.05),
                    hold_slack_ns: Some(0.1),
                    paths: 2,
                },
            )]
            .into_iter()
            .collect(),
            internal_paths: 1,
        };
        let text = eda_sta::write_block_timing_model(&model)?;
        let parsed = eda_sta::parse_block_timing_model(&text)?;
        let malformed = eda_sta::parse_block_timing_model("block m clk nan\ninternal_paths 0\n");
        if parsed == model
            && text.contains(r#"block "sub\\block" "clk" 2.5"#)
            && text.contains("port input \"d\" \"clk\" - - 1.25 0.25 0.05 0.1 2")
            && text.ends_with("internal_paths 1\n")
            && matches!(malformed, Err(error) if error.contains("must be finite"))
        {
            Ok(())
        } else {
            Err(format!("{}\n{:?}", text, parsed))
        }
    });

    c.add("sta_block_timing_composition_checks_interfaces", || {
        let producer = eda_sta::BlockTimingModel {
            module: "producer".into(),
            clock: "clk".into(),
            period_ns: 2.0,
            ports: [
                (
                    "clk".into(),
                    eda_sta::BlockTimingPort {
                        name: "clk".into(),
                        direction: eda_sta::BlockTimingDirection::Input,
                        clock: None,
                        late_arrival_ns: None,
                        early_arrival_ns: None,
                        late_required_ns: None,
                        setup_slack_ns: None,
                        early_required_ns: None,
                        hold_slack_ns: None,
                        paths: 0,
                    },
                ),
                (
                    "y".into(),
                    eda_sta::BlockTimingPort {
                        name: "y".into(),
                        direction: eda_sta::BlockTimingDirection::Output,
                        clock: Some("clk".into()),
                        late_arrival_ns: Some(1.0),
                        early_arrival_ns: Some(0.8),
                        late_required_ns: None,
                        setup_slack_ns: Some(0.5),
                        early_required_ns: None,
                        hold_slack_ns: None,
                        paths: 1,
                    },
                ),
            ]
            .into_iter()
            .collect(),
            internal_paths: 1,
        };
        let consumer = eda_sta::BlockTimingModel {
            module: "consumer".into(),
            clock: "clk".into(),
            period_ns: 2.0,
            ports: [
                (
                    "clk".into(),
                    eda_sta::BlockTimingPort {
                        name: "clk".into(),
                        direction: eda_sta::BlockTimingDirection::Input,
                        clock: None,
                        late_arrival_ns: None,
                        early_arrival_ns: None,
                        late_required_ns: None,
                        setup_slack_ns: None,
                        early_required_ns: None,
                        hold_slack_ns: None,
                        paths: 0,
                    },
                ),
                (
                    "d".into(),
                    eda_sta::BlockTimingPort {
                        name: "d".into(),
                        direction: eda_sta::BlockTimingDirection::Input,
                        clock: Some("clk".into()),
                        late_arrival_ns: None,
                        early_arrival_ns: None,
                        late_required_ns: Some(1.2),
                        setup_slack_ns: Some(0.3),
                        early_required_ns: Some(0.85),
                        hold_slack_ns: Some(0.2),
                        paths: 1,
                    },
                ),
                (
                    "q".into(),
                    eda_sta::BlockTimingPort {
                        name: "q".into(),
                        direction: eda_sta::BlockTimingDirection::Output,
                        clock: Some("clk".into()),
                        late_arrival_ns: Some(1.4),
                        early_arrival_ns: Some(1.1),
                        late_required_ns: None,
                        setup_slack_ns: Some(0.4),
                        early_required_ns: None,
                        hold_slack_ns: None,
                        paths: 1,
                    },
                ),
            ]
            .into_iter()
            .collect(),
            internal_paths: 1,
        };
        let models = [producer, consumer];
        let instances = [
            eda_sta::BlockTimingCompositionInstance::new("u1", "producer"),
            eda_sta::BlockTimingCompositionInstance::new("u2", "consumer"),
        ];
        let connections = [eda_sta::BlockTimingCompositionConnection::new(
            "u1", "y", "u2", "d", 0.1,
        )];
        let result = eda_sta::compose_block_timing_models(&models, &instances, &connections)?;
        let report = eda_sta::block_timing_composition_report(&result);
        let duplicate = [connections[0].clone(), connections[0].clone()];
        let mut violating = models.clone();
        violating[1]
            .ports
            .get_mut("d")
            .ok_or("missing violating consumer port")?
            .early_required_ns = Some(1.0);
        let violating_result =
            eda_sta::compose_block_timing_models(&violating, &instances, &connections)?;
        if result.clock == "clk"
            && result.checks.len() == 1
            && (result.checks[0].arrival_ns - 1.1).abs() < 1e-12
            && (result.checks[0].slack_ns - 0.1).abs() < 1e-12
            && result.hold_checks.len() == 1
            && (result.hold_checks[0].arrival_ns - 0.9).abs() < 1e-12
            && (result.hold_checks[0].slack_ns - 0.05).abs() < 1e-12
            && result.is_clean()
            && result.internal_paths == 2
            && result.unconnected_inputs.is_empty()
            && result.unconnected_outputs == vec!["u2:q".to_string()]
            && (result.worst_slack_ns.unwrap() - 0.1).abs() < 1e-12
            && (result.worst_hold_slack_ns.unwrap() - 0.05).abs() < 1e-12
            && report.contains("u1:y -> u2:d")
            && report.contains("setup u1:y -> u2:d")
            && report.contains("hold u1:y -> u2:d")
            && report.contains("worst setup interface slack: 0.100000 ns")
            && report.contains("worst hold interface slack: 0.050000 ns")
            && matches!(
                eda_sta::compose_block_timing_models(&models, &instances, &duplicate),
                Err(error) if error.contains("multiple drivers")
            )
            && !violating_result.is_clean()
            && (violating_result.worst_hold_slack_ns.unwrap() + 0.1).abs() < 1e-12
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("sta_block_timing_composition_maps_clock_names", || {
        let producer_text = r#"
            block "producer" "pclk" 2
            port input "pclk" "-" - - - - - - 0
            port output "y" "pclk" 1.0 0.8 - 0.5 - - 1
            internal_paths 1
        "#;
        let consumer_text = r#"
            block "consumer" "dclk" 2
            port input "dclk" "-" - - - - - - 0
            port input "d" "dclk" - - 1.2 0.3 0.85 0.2 1
            port output "q" "dclk" 1.4 1.1 - 0.4 - - 1
            internal_paths 1
        "#;
        let models = [
            eda_sta::parse_block_timing_model(producer_text)?,
            eda_sta::parse_block_timing_model(consumer_text)?,
        ];
        let instances = [
            eda_sta::BlockTimingCompositionInstance::new("u1", "producer"),
            eda_sta::BlockTimingCompositionInstance::new("u2", "consumer"),
        ];
        let connections = [eda_sta::BlockTimingCompositionConnection::new(
            "u1", "y", "u2", "d", 0.1,
        )];
        let clock_maps = [
            eda_sta::BlockTimingClockMap::new("u1").with_clock("pclk", "clk"),
            eda_sta::BlockTimingClockMap::new("u2").with_clock("dclk", "clk"),
        ];
        let result = eda_sta::compose_block_timing_models_with_clock_maps(
            &models,
            &instances,
            &connections,
            &clock_maps,
        )?;
        let report = eda_sta::block_timing_composition_report(&result);
        let bad_maps = [
            eda_sta::BlockTimingClockMap::new("u1").with_clock("wrong", "clk"),
            clock_maps[1].clone(),
        ];
        if result.clock == "clk"
            && result.clock_maps == clock_maps
            && result.checks.len() == 1
            && result.hold_checks.len() == 1
            && result.is_clean()
            && report.contains("clock map u1: pclk -> clk")
            && report.contains("clock map u2: dclk -> clk")
            && matches!(
                eda_sta::compose_block_timing_models(&models, &instances, &connections),
                Err(error) if error.contains("uses clock 'dclk', expected 'pclk'")
            )
            && matches!(
                eda_sta::compose_block_timing_models_with_clock_maps(
                    &models,
                    &instances,
                    &connections,
                    &bad_maps
                ),
                Err(error) if error.contains("references unknown local clock 'wrong'")
            )
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("sta_block_timing_composition_supports_clock_domains", || {
        let producer_text = r#"
            block "producer" "pclk" 4
            port input "pclk" "-" - - - - - - 0
            port output "y" "pclk" 1.0 0.7 - 0.5 - - 1
            internal_paths 1
        "#;
        let consumer_text = r#"
            block "consumer" "dclk" 6
            port input "dclk" "-" - - - - - - 0
            port input "d" "dclk" - - 1.2 0.3 0.4 0.2 1
            internal_paths 1
        "#;
        let models = [
            eda_sta::parse_block_timing_model(producer_text)?,
            eda_sta::parse_block_timing_model(consumer_text)?,
        ];
        let instances = [
            eda_sta::BlockTimingCompositionInstance::new("u1", "producer"),
            eda_sta::BlockTimingCompositionInstance::new("u2", "consumer"),
        ];
        let connections = [eda_sta::BlockTimingCompositionConnection::new(
            "u1", "y", "u2", "d", 0.1,
        )];
        let clock_maps = [
            eda_sta::BlockTimingClockMap::new("u1").with_clock("pclk", "source_clk"),
            eda_sta::BlockTimingClockMap::new("u2").with_clock("dclk", "capture_clk"),
        ];
        let clocks = [
            eda_sta::BlockTimingClock::new("source_clk", 4.0, 0.0),
            eda_sta::BlockTimingClock::new("capture_clk", 6.0, 0.2),
        ];
        let relationships = [eda_sta::BlockTimingClockRelationship::new(
            "source_clk",
            "capture_clk",
            0.2,
        )];
        let result = eda_sta::compose_block_timing_models_with_clock_domains(
            &models,
            &instances,
            &connections,
            &clock_maps,
            &clocks,
            &relationships,
        )?;
        let report = eda_sta::block_timing_composition_report(&result);
        if result.clocks == clocks
            && result.clock_relationships == relationships
            && result.checks.len() == 1
            && result.checks[0].source_clock == "source_clk"
            && result.checks[0].destination_clock == "capture_clk"
            && (result.checks[0].edge_separation_ns - 0.2).abs() < 1e-12
            && (result.checks[0].required_ns - 1.4).abs() < 1e-12
            && (result.checks[0].slack_ns - 0.3).abs() < 1e-12
            && result.hold_checks.len() == 1
            && (result.hold_checks[0].required_ns - 0.6).abs() < 1e-12
            && (result.hold_checks[0].slack_ns - 0.2).abs() < 1e-12
            && result.is_clean()
            && report.contains("clock 'capture_clk': period 6.000000 ns, phase 0.200000 ns")
            && report.contains("clock relationship source_clk -> capture_clk")
            && matches!(
                eda_sta::compose_block_timing_models_with_clock_domains(
                    &models,
                    &instances,
                    &connections,
                    &clock_maps,
                    &clocks,
                    &[],
                ),
                Err(error) if error.contains("has no timed 'source_clk' -> 'capture_clk' clock relationship")
            )
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("sta_timing_violation_waivers_filter_and_reappear", || {
        let nl = through_chain_netlist();
        let failing = eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
        let worse = eda_sta::parse_sdc("create_clock -name clk -period 0.5 [get_ports a]")?;
        let timing = eda_sta::analyze_with_sdc(&nl, &failing)?;
        let mut store = eda_sta::TimingViolationStore::from_timing("func", &nl, &timing)?;
        let waiver = eda_sta::TimingViolationWaiver::new("small-output", "func", "y", -0.21)?
            .with_kind(eda_sta::TimingViolationKind::Output)
            .with_owner("timing")
            .with_note("accepted early mode");
        store.add_waiver(waiver)?;
        let waived_ok = store.is_empty()
            && store.violations().len() == 1
            && store.waived_len() == 1
            && store.waived_violations().get("func:output:y") == Some(&"small-output".to_string());

        let worse_timing = eda_sta::analyze_with_sdc(&nl, &worse)?;
        store.update(&nl, &worse_timing)?;
        let threshold_ok = store.len() == 1
            && store.waived_len() == 0
            && (store.violations()["func:output:y"].slack_ns + 0.7).abs() < 1e-12;

        store.remove_waiver("small-output")?;
        let temporary = eda_sta::TimingViolationWaiver::new("temporary-output", "func", "y", -1.0)?
            .expires_after(3);
        store.add_waiver(temporary)?;
        store.update(&nl, &worse_timing)?;
        let temporary_valid = store.is_empty() && store.waived_len() == 1;
        store.update(&nl, &worse_timing)?;
        let temporary_expired = store.len() == 1 && store.waived_len() == 0;
        store.remove_waiver("temporary-output")?;
        if waived_ok
            && threshold_ok
            && temporary_valid
            && temporary_expired
            && store.len() == 1
            && store.waivers().is_empty()
            && eda_sta::timing_violation_report(&store, None).contains("waived violations: 0")
        {
            Ok(())
        } else {
            Err(eda_sta::timing_violation_report(&store, None))
        }
    });

    c.add(
        "sta_multicorner_violation_waiver_applies_by_scenario",
        || {
            let nl = through_chain_netlist();
            let slow = eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports a]")?;
            let fast = eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
            let views = [
                eda_sta::TimingScenario::new("slow", &nl, &slow),
                eda_sta::TimingScenario::new("fast", &nl, &fast),
            ];
            let mut registry = eda_sta::TimingViolationRegistry::from_scenarios(views)?;
            let waiver = eda_sta::TimingViolationWaiver::new("fast-accepted", "fast", "y", -0.21)?
                .with_owner("frontend");
            registry.add_waiver(waiver)?;
            let waived_ok = registry.is_empty()
                && registry.stores()["fast"].violations().len() == 1
                && registry.stores()["fast"].waived_len() == 1
                && registry.summaries().is_empty();
            registry.remove_waiver("fast-accepted")?;
            if waived_ok
                && registry.len() == 1
                && registry.summaries().contains_key("output:y")
                && registry.waivers().is_empty()
                && eda_sta::timing_violation_registry_report(&registry, None)
                    .contains("waived violations: 0")
            {
                Ok(())
            } else {
                Err(eda_sta::timing_violation_registry_report(&registry, None))
            }
        },
    );

    c.add("sta_timing_waivers_serialize_and_parse_round_trip", || {
        let waivers = vec![
            eda_sta::TimingViolationWaiver::new("all", "func*", "**/y", -0.21)?
                .with_kind(eda_sta::TimingViolationKind::Output)
                .with_owner("timing\\owner")
                .with_note("reviewed\nlater")
                .expires_after(12),
            eda_sta::TimingViolationWaiver::new("hold-any", "*", "top/**", -0.05)?
                .with_kind(eda_sta::TimingViolationKind::Hold),
        ];
        let text = eda_sta::write_timing_violation_waivers(&waivers)?;
        let parsed = eda_sta::parse_timing_violation_waivers(&text)?;
        let malformed = eda_sta::parse_timing_violation_waivers(
            "waiver same func y -1\nwaiver same func y -2\n",
        );
        if parsed == waivers
            && text.contains("waiver \"all\" \"func*\" \"**/y\" \"-0.21\"")
            && text.contains("owner \"timing\\\\owner\"")
            && text.contains("note \"reviewed\\nlater\"")
            && matches!(malformed, Err(error) if error.contains("duplicate timing violation waiver"))
        {
            Ok(())
        } else {
            Err(text)
        }
    });

    c.add("sta_timing_waiver_audit_records_actor_and_snapshot", || {
        let nl = through_chain_netlist();
        let constraints = eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
        let timing = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let mut store = eda_sta::TimingViolationStore::from_timing("func", &nl, &timing)?;
        let waiver =
            eda_sta::TimingViolationWaiver::new("audited", "func", "y", -1.0)?.with_owner("timing");
        store.add_waiver_with_actor(waiver.clone(), Some("engineer".into()))?;
        store.remove_waiver_with_actor("audited", Some("reviewer".into()))?;
        let audit = store.audit();
        if audit.len() == 2
            && audit[0].sequence == 1
            && audit[0].action == eda_sta::TimingWaiverAuditAction::Added
            && audit[0].changed_by.as_deref() == Some("engineer")
            && audit[0].waiver == waiver
            && audit[1].sequence == 2
            && audit[1].action == eda_sta::TimingWaiverAuditAction::Removed
            && audit[1].changed_by.as_deref() == Some("reviewer")
            && audit[1].waiver == waiver
            && eda_sta::timing_waiver_audit_report(audit)
                .contains("#1 added 'audited' at generation 1 by engineer")
        {
            Ok(())
        } else {
            Err(eda_sta::timing_waiver_audit_report(audit))
        }
    });

    c.add("sta_timing_waiver_audit_groups_by_hierarchy_scope", || {
        let nl = through_chain_netlist();
        let fast = eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
        let mut registry =
            eda_sta::TimingViolationRegistry::from_scenarios([eda_sta::TimingScenario::new(
                "fast", &nl, &fast,
            )])?;
        let first = eda_sta::TimingViolationWaiver::new("u1", "fast", "y", -1.0)?
            .with_instance_pattern("top.u1.**");
        let second = eda_sta::TimingViolationWaiver::new("u2", "fast", "y", -1.0)?
            .with_instance_pattern("top.u2");
        let unscoped =
            eda_sta::TimingViolationWaiver::new("any", "fast", "y", -1.0)?.with_owner("timing");
        registry.add_waiver_with_actor(first, Some("frontend".into()))?;
        registry.add_waiver_with_actor(second, Some("frontend".into()))?;
        registry.add_waiver_with_actor(unscoped, Some("lead".into()))?;
        registry.remove_waiver_with_actor("u1", Some("reviewer".into()))?;
        let summaries = eda_sta::timing_waiver_audit_hierarchy_summaries(registry.audit())?;
        let first_key = Some("top.u1.**".to_string());
        let second_key = Some("top.u2".to_string());
        let first_summary = summaries
            .get(&first_key)
            .ok_or("missing u1 audit hierarchy summary")?;
        let second_summary = summaries
            .get(&second_key)
            .ok_or("missing u2 audit hierarchy summary")?;
        let unscoped_summary = summaries
            .get(&None)
            .ok_or("missing unscoped audit hierarchy summary")?;
        let report = eda_sta::timing_waiver_audit_hierarchy_report(registry.audit())?;
        if summaries.len() == 3
            && registry.audit().len() == 4
            && first_summary.events == 2
            && first_summary.added_events == 1
            && first_summary.removed_events == 1
            && first_summary.active_waiver_ids.is_empty()
            && first_summary.latest_generation == 4
            && second_summary.events == 1
            && second_summary.active_waiver_ids.contains("u2")
            && unscoped_summary.active_waiver_ids.contains("any")
            && report.contains("'top.u1.**': events=2, added=1, removed=1, active=0")
            && report.contains("'top.u2': events=1, added=1, removed=0, active=1")
            && report.contains("'(unscoped)': events=1, added=1, removed=0, active=1")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("sta_multicorner_waiver_registry_keeps_global_audit", || {
        let nl = through_chain_netlist();
        let slow = eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports a]")?;
        let fast = eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
        let mut registry = eda_sta::TimingViolationRegistry::from_scenarios([
            eda_sta::TimingScenario::new("slow", &nl, &slow),
            eda_sta::TimingScenario::new("fast", &nl, &fast),
        ])?;
        let waiver =
            eda_sta::TimingViolationWaiver::new("global", "*", "y", -1.0)?.with_owner("timing");
        registry.add_waiver_with_actor(waiver.clone(), Some("lead".into()))?;
        registry.remove_waiver_with_actor("global", Some("lead".into()))?;
        let audit = registry.audit();
        if audit.len() == 2
            && audit[0].action == eda_sta::TimingWaiverAuditAction::Added
            && audit[1].action == eda_sta::TimingWaiverAuditAction::Removed
            && audit
                .iter()
                .all(|entry| entry.changed_by.as_deref() == Some("lead") && entry.waiver == waiver)
        {
            Ok(())
        } else {
            Err(eda_sta::timing_waiver_audit_report(audit))
        }
    });

    c.add("sta_timing_waiver_audit_serializes_and_parses_round_trip", || {
        let waiver = eda_sta::TimingViolationWaiver::new("restore", "func*", "**/y", -0.21)?
            .with_kind(eda_sta::TimingViolationKind::Output)
            .with_owner("timing")
            .with_note("approved\nsession")
            .expires_after(9);
        let removed = eda_sta::TimingViolationWaiver::new("old", "func", "y", -1.0)?;
        let entries = vec![
            eda_sta::TimingWaiverAuditEntry {
                sequence: 1,
                action: eda_sta::TimingWaiverAuditAction::Added,
                waiver_id: "old".into(),
                generation: 1,
                changed_by: Some("engineer".into()),
                waiver: removed,
            },
            eda_sta::TimingWaiverAuditEntry {
                sequence: 2,
                action: eda_sta::TimingWaiverAuditAction::Removed,
                waiver_id: "old".into(),
                generation: 1,
                changed_by: None,
                waiver: eda_sta::TimingViolationWaiver::new("old", "func", "y", -1.0)?,
            },
            eda_sta::TimingWaiverAuditEntry {
                sequence: 3,
                action: eda_sta::TimingWaiverAuditAction::Added,
                waiver_id: "restore".into(),
                generation: 2,
                changed_by: Some("reviewer".into()),
                waiver,
            },
        ];
        let text = eda_sta::write_timing_waiver_audit(&entries)?;
        let parsed = eda_sta::parse_timing_waiver_audit(&text)?;
        let malformed = eda_sta::parse_timing_waiver_audit(
            "audit 2 added 1 \"x\"\nend\n",
        );
        if parsed == entries
            && text.contains("audit 1 added 1 \"engineer\"")
            && text.contains("audit 2 removed 1 \"\"")
            && text.contains("audit 3 added 2 \"reviewer\"")
            && matches!(malformed, Err(error) if error.contains("audit block must contain exactly one waiver"))
        {
            Ok(())
        } else {
            Err(text)
        }
    });

    c.add("sta_timing_waiver_audit_restores_store_state", || {
        let nl = through_chain_netlist();
        let constraints =
            eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
        let timing = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let source = eda_sta::TimingViolationStore::from_timing("func", &nl, &timing)?;
        let waiver = eda_sta::TimingViolationWaiver::new("restore", "func", "y", -1.0)?
            .with_owner("timing");
        let entries = vec![eda_sta::TimingWaiverAuditEntry {
            sequence: 1,
            action: eda_sta::TimingWaiverAuditAction::Added,
            waiver_id: "restore".into(),
            generation: 1,
            changed_by: Some("engineer".into()),
            waiver,
        }];
        let mut restored = source.clone();
        restored.restore_waiver_audit(&entries)?;
        let future = vec![eda_sta::TimingWaiverAuditEntry {
            sequence: 1,
            action: eda_sta::TimingWaiverAuditAction::Added,
            waiver_id: "future".into(),
            generation: 2,
            changed_by: None,
            waiver: eda_sta::TimingViolationWaiver::new("future", "func", "y", -1.0)?,
        }];
        if restored.generation == source.generation
            && restored.audit() == entries.as_slice()
            && restored.waivers().len() == 1
            && restored.is_empty()
            && restored.violations().len() == 1
            && matches!(restored.restore_waiver_audit(&future), Err(error) if error.contains("newer than store generation"))
        {
            Ok(())
        } else {
            Err(eda_sta::timing_violation_report(&restored, None))
        }
    });

    c.add(
        "sta_multicorner_waiver_audit_restores_registry_state",
        || {
            let nl = through_chain_netlist();
            let slow = eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports a]")?;
            let fast = eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
            let mut registry = eda_sta::TimingViolationRegistry::from_scenarios([
                eda_sta::TimingScenario::new("slow", &nl, &slow),
                eda_sta::TimingScenario::new("fast", &nl, &fast),
            ])?;
            let waiver = eda_sta::TimingViolationWaiver::new("fast", "fast", "y", -0.21)?;
            let entries = vec![eda_sta::TimingWaiverAuditEntry {
                sequence: 1,
                action: eda_sta::TimingWaiverAuditAction::Added,
                waiver_id: "fast".into(),
                generation: registry.generation(),
                changed_by: Some("lead".into()),
                waiver,
            }];
            registry.restore_waiver_audit(&entries)?;
            if registry.audit() == entries.as_slice()
                && registry.waivers().len() == 1
                && registry.is_empty()
                && registry.stores()["fast"].audit() == entries.as_slice()
                && registry.stores()["slow"].audit() == entries.as_slice()
                && registry.summaries().is_empty()
            {
                Ok(())
            } else {
                Err(eda_sta::timing_violation_registry_report(&registry, None))
            }
        },
    );

    c.add("sta_multicorner_timing_paths_merge_hold_endpoints", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let slow = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_clock_uncertainty 0.0 -hold [get_clocks clk]
        "#,
        )?;
        let fast = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_clock_uncertainty 0.1 -hold [get_clocks clk]
        "#,
        )?;
        let views = [
            eda_sta::TimingScenario::new("slow", nl, &slow),
            eda_sta::TimingScenario::new("fast", nl, &fast),
        ];
        let result = eda_sta::analyze_timing_scenarios(views)?;
        let paths = eda_sta::multi_corner_hold_timing_paths(&result, views, 0)?;
        let report = eda_sta::multi_corner_hold_timing_path_report(&paths);
        if paths.len() == 2
            && paths
                .windows(2)
                .all(|pair| pair[0].path.slack_ns <= pair[1].path.slack_ns)
            && paths
                .iter()
                .all(|path| path.scenario == "fast" && path.corner_slacks.len() == 2)
            && report.contains("Multi-corner hold timing path report: 2 endpoint(s)")
            && report.contains("worst scenario 'fast'")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("sta_multicorner_timing_paths_merge_reset_checks", || {
        let src: &'static str = "module m(input clk, d, rst, output reg q);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 1'b0; else q <= d;
             endmodule";
        let nl = synth_cached(src);
        let slow = eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports clk]")?;
        let fast = eda_sta::parse_sdc("create_clock -name clk -period 2.0 [get_ports clk]")?;
        let views = [
            eda_sta::TimingScenario::new("slow", nl, &slow),
            eda_sta::TimingScenario::new("fast", nl, &fast),
        ];
        let result = eda_sta::analyze_timing_scenarios(views)?;
        let paths = eda_sta::multi_corner_reset_timing_paths(&result, views, 0)?;
        let report = eda_sta::multi_corner_reset_timing_path_report(&paths);
        if paths.len() == 2
            && paths
                .iter()
                .filter(|path| path.check == eda_sta::ResetCheckKind::Recovery)
                .all(|path| path.scenario == "fast")
            && report.contains("Multi-corner reset timing path report: 2 endpoint/check(s)")
            && report.contains("recovery")
            && report.contains("removal")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("sta_multicorner_rejects_invalid_scenarios", || {
        let nl = sdc_netlist();
        let constraints = eda_sta::parse_sdc("create_clock -name clk -period 1 [get_ports clk]")?;
        if eda_sta::analyze_timing_scenarios(Vec::<eda_sta::TimingScenario>::new()).is_ok()
            || eda_sta::analyze_timing_scenarios([
                eda_sta::TimingScenario::new("same", &nl, &constraints),
                eda_sta::TimingScenario::new("same", &nl, &constraints),
            ])
            .is_ok()
        {
            Err("invalid multi-corner input accepted".into())
        } else {
            Ok(())
        }
    });
    c.add("sta_multicorner_inline_manifest_parses_and_runs", || {
        let definitions = eda_sta::parse_timing_scenarios(
            r#"
            scenario slow
                create_clock -name clk -period 4.0 [get_ports clk]
                set_input_delay 0.2 -clock clk [get_ports a]
            endscenario
            scenario fast
                create_clock -name clk -period 2.0 [get_ports clk]
                set_input_delay 0.2 -clock clk [get_ports a]
            endscenario
        "#,
        )?;
        let nl = sdc_netlist();
        let result = eda_sta::analyze_timing_scenario_definitions(&nl, &definitions)?;
        if definitions.len() == 2
            && definitions[0].constraints.clocks["clk"].period == 4.0
            && result.worst_scenario() == Some("fast")
        {
            Ok(())
        } else {
            Err(eda_sta::multi_corner_report(&result))
        }
    });
    c.add(
        "sta_multicorner_manifest_rejects_malformed_sections",
        || {
            let bad = [
                "// no scenarios",
                "scenario slow\ncreate_clock -name c -period 1 [get_ports c]",
                "scenario slow\nendscenario extra",
                "scenario slow\nendscenario\nscenario slow\nendscenario",
            ];
            if bad
                .iter()
                .all(|source| eda_sta::parse_timing_scenarios(source).is_err())
            {
                Ok(())
            } else {
                Err("malformed MMMC manifest accepted".into())
            }
        },
    );
    c.add("sdc_hold_checks_can_dominate_worst_slack", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.8 -clock clk [get_ports d]
            set_clock_uncertainty 0.10 -setup [get_clocks clk]
            set_clock_uncertainty 0.15 -hold [get_clocks clk]
        "#,
        )?;
        let r = eda_sta::analyze_with_sdc(nl, &constraints)?;
        // Input hold: 0.8 - 0.2 cell hold - 0.15 uncertainty.
        let want_in2reg = 0.45;
        // Reg hold: 1.2 clk->q - 0.2 hold - 0.15 uncertainty.
        let want_reg2reg = 0.85;
        if (r.in2reg_hold_slack - want_in2reg).abs() > 1e-9
            || (r.reg2reg_hold_slack - want_reg2reg).abs() > 1e-9
            || (r.hold_slack - want_in2reg).abs() > 1e-9
            || (r.worst_slack - want_in2reg).abs() > 1e-9
        {
            Err(format!(
                "hold in2reg={} reg2reg={} total={} worst={}",
                r.in2reg_hold_slack, r.reg2reg_hold_slack, r.hold_slack, r.worst_slack
            ))
        } else if !eda_sta::constrained_report(&r).contains("hold in-to-reg") {
            Err("report omitted hold results".into())
        } else {
            Ok(())
        }
    });
    c.add("sdc_max_and_min_delay_limits", || {
        let src: &'static str = "module m(input a, b, output y); assign y = a & b; endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_max_delay 1.1 -from [get_ports a] -to [get_ports y]
            set_min_delay 0.4 -from [get_ports a] -to [get_ports y]
        "#,
        )?;
        let r = eda_sta::analyze_with_sdc(nl, &constraints)?;
        if (r.max_delay_slack - 0.1).abs() > 1e-9 {
            Err(format!("max delay slack {}", r.max_delay_slack))
        } else if (r.min_delay_slack - 0.6).abs() > 1e-9 {
            Err(format!("min delay slack {}", r.min_delay_slack))
        } else if (r.worst_slack - 0.1).abs() > 1e-9 {
            Err(format!("worst slack {}", r.worst_slack))
        } else if !eda_sta::constrained_report(&r).contains("max delay")
            || !eda_sta::constrained_report(&r).contains("min delay")
        {
            Err("report omitted path-delay limits".into())
        } else {
            Ok(())
        }
    });
    c.add("sdc_min_pulse_width", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 -fall 0.3 [get_ports clk]
        "#,
        )?;
        let r = eda_sta::analyze_with_sdc(nl, &constraints)?;
        if (r.min_pulse_high_slack - 0.05).abs() > 1e-9
            || (r.min_pulse_low_slack - 3.45).abs() > 1e-9
            || (r.min_pulse_width_slack - 0.05).abs() > 1e-9
        {
            Err(format!(
                "high={} low={} worst={}",
                r.min_pulse_high_slack, r.min_pulse_low_slack, r.min_pulse_width_slack
            ))
        } else if !eda_sta::constrained_report(&r).contains("min pulse high") {
            Err("report omitted min-pulse results".into())
        } else {
            Ok(())
        }
    });
    c.add("sdc_recovery_removal", || {
        let src: &'static str = "module m(input clk, d, rst, output reg q);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 1'b0; else q <= d;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 1.2 -clock clk [get_ports rst]
            set_clock_uncertainty 0.10 -setup [get_clocks clk]
            set_clock_uncertainty 0.05 -hold [get_clocks clk]
        "#,
        )?;
        let r = eda_sta::analyze_with_sdc(nl, &constraints)?;
        if (r.recovery_slack - 2.4).abs() > 1e-9
            || (r.removal_slack - 1.0).abs() > 1e-9
            || (r.async_reset_slack - 1.0).abs() > 1e-9
        {
            Err(format!(
                "recovery={} removal={} async={}",
                r.recovery_slack, r.removal_slack, r.async_reset_slack
            ))
        } else if !eda_sta::constrained_report(&r).contains("recovery")
            || !eda_sta::constrained_report(&r).contains("removal")
        {
            Err("report omitted async-reset checks".into())
        } else {
            Ok(())
        }
    });
    c.add("sdc_clock_latency", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_clock_latency -source 0.7 [get_clocks clk]
            set_clock_latency 0.3 [get_clocks clk]
            set_input_delay 1.0 -clock clk [get_ports d]
        "#,
        )?;
        let r = eda_sta::analyze_with_sdc(nl, &constraints)?;
        if (r.reg2reg_slack - 2.2).abs() > 1e-9
            || (r.in2reg_slack - 2.7).abs() > 1e-9
            || (r.output_slack - 1.8).abs() > 1e-9
        {
            Err(format!(
                "reg2reg={} in2reg={} output={}",
                r.reg2reg_slack, r.in2reg_slack, r.output_slack
            ))
        } else if !eda_sta::constrained_report(&r).contains("source latency 0.700 ns")
            || !eda_sta::constrained_report(&r).contains("network latency 0.300 ns")
        {
            Err("report omitted clock latency".into())
        } else {
            Ok(())
        }
    });

    c.add("sdc_false_path_endpoint", || {
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_input_delay 0.3 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_false_path -from [get_ports a] -to [get_ports y]
        "#,
        )?;
        let nl = sdc_netlist();
        let r = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        if r.output_slack.is_infinite() && r.exceptions_applied == 1 {
            Ok(())
        } else {
            Err(format!(
                "slack={}, exceptions={}",
                r.output_slack, r.exceptions_applied
            ))
        }
    });

    c.add("sta_false_path_reports_excluded_path_objects", || {
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_input_delay 0.3 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_false_path -from [get_ports a] -to [get_ports y]
        "#,
        )?;
        let nl = sdc_netlist();
        let timing = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let excluded = eda_sta::excluded_timing_paths(&nl, &timing);
        let report = eda_sta::excluded_timing_path_report(&nl, &excluded);
        if excluded.len() == 1
            && excluded[0].check == eda_sta::ExcludedTimingCheck::Setup
            && excluded[0].path.endpoint == eda_sta::TimingPathEndpoint::OutputPort("y".into())
            && excluded[0].exception.from.as_deref() == Some("a")
            && excluded[0].exception.to.as_deref() == Some("y")
            && excluded[0].exception.multicycle.is_none()
            && report.contains("setup output y excluded by false path from a to y")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("sdc_multicycle_path", || {
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_input_delay 0.3 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_multicycle_path 2 -from [get_ports a] -to [get_ports y]
        "#,
        )?;
        let nl = sdc_netlist();
        let r = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        if (r.output_slack - 2.3).abs() > 1e-9 {
            Err(format!("multicycle slack {}", r.output_slack))
        } else {
            Ok(())
        }
    });

    c.add("sdc_invalid_clock_is_rejected", || {
        if eda_sta::parse_sdc("set_input_delay 1 -clock missing [get_ports a]").is_ok() {
            Err("unknown clock accepted".into())
        } else {
            Ok(())
        }
    });

    for factor in 1_u32..=100 {
        c.add(
            format!("sdc_generated_clock_divide_{factor:03}"),
            move || {
                let source = format!(
                    "create_clock -name core -period 10.0 [get_ports clk]\n\
                 create_generated_clock -name gen -source [get_clocks core] \
                 -divide_by {factor} -ports [get_ports div]"
                );
                let constraints = eda_sta::parse_sdc(&source)?;
                let generated = constraints
                    .clocks
                    .get("gen")
                    .ok_or("generated clock missing")?;
                if generated.source_clock.as_deref() != Some("core")
                    || generated.divide_by != Some(factor as f64)
                    || (generated.period - 10.0 * factor as f64).abs() > 1e-9
                    || generated.ports != ["div".to_string()]
                {
                    Err(format!("factor={} generated={:?}", factor, generated))
                } else {
                    Ok(())
                }
            },
        );
    }

    for index in 0_u32..100 {
        c.add(format!("sdc_clock_group_{index:03}"), move || {
            let (flag, expected) = match index % 3 {
                0 => ("-asynchronous", eda_sta::ClockGroupKind::Asynchronous),
                1 => (
                    "-logically_exclusive",
                    eda_sta::ClockGroupKind::LogicallyExclusive,
                ),
                _ => (
                    "-physically_exclusive",
                    eda_sta::ClockGroupKind::PhysicallyExclusive,
                ),
            };
            let source = format!(
                "create_clock -name core -period 4 [get_ports core_clk]\n\
                 create_clock -name io -period 6 [get_ports io_clk]\n\
                 create_clock -name scan -period 8 [get_ports scan_clk]\n\
                 set_clock_groups -name rel_{index} {flag} \
                 -group [get_clocks core] -group [get_clocks io scan]"
            );
            let constraints = eda_sta::parse_sdc(&source)?;
            if constraints.clock_groups.len() != 1
                || constraints.clock_groups[0].name != format!("rel_{index}")
                || constraints.clock_groups[0].kind != expected
                || constraints.clock_group_kind("core", "io") != Some(expected)
                || constraints.clock_group_kind("core", "scan") != Some(expected)
                || constraints.clock_group_kind("io", "scan").is_some()
            {
                Err(format!(
                    "index={} groups={:?}",
                    index, constraints.clock_groups
                ))
            } else {
                Ok(())
            }
        });
    }
}
