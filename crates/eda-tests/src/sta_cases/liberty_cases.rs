//! Liberty timing/constraint LUT regression cases.

use super::*;

pub(super) fn register(c: &mut CaseList) {
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
}
