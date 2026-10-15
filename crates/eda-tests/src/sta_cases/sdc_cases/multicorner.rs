//! SDC/MMMC child regression domain.

use super::super::*;
use super::{sdc_netlist, through_chain_netlist};

pub(super) fn register_multicorner(c: &mut CaseList) {
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
}
