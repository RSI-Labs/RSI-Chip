//! Constrained paths, multicycles, delay limits, through selectors, and case analysis.

use super::sdc_cases::{hierarchical_through_chain_netlist, through_chain_netlist};
use super::*;

pub(super) fn register(c: &mut CaseList) {
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
            && report.contains("multicycle 2.000000 (setup+hold), from <any> to q1")
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
}
