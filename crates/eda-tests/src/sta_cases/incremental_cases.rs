//! Disabled arcs, incremental timing, hold/reset paths, and excluded checks.

use super::sdc_cases::{incremental_branch_netlist, sdc_netlist, through_chain_netlist};
use super::*;

pub(super) fn register(c: &mut CaseList) {
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
