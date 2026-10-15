//! Signal quality, block timing, constrained path objects, exceptions, violations, and waivers.

use super::*;

pub(super) fn register(c: &mut CaseList) {
    c.add("api_signal_quality_analysis", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) & c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let mut extra = std::collections::BTreeMap::new();
        extra.insert("y".to_string(), 0.2);
        let result = eda_api::analyze_signal_quality(
            &flow.netlist,
            &eda_api::QualityOptions {
                max_transition_ns: Some(10.0),
                max_capacitance_pf: Some(0.1),
                extra_capacitance_pf: extra,
                ..Default::default()
            },
        )?;
        if result
            .violations
            .iter()
            .any(|v| v.kind == eda_api::QualityKind::MaxCapacitance)
        {
            Ok(())
        } else {
            Err(eda_api::signal_quality_report(&result))
        }
    });
    c.add("api_signal_quality_uses_spef_capacitance", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let parasitics = eda_api::ParasiticReport {
            design: "m".into(),
            net_count: 1,
            unmatched_netlist_nets: Vec::new(),
            nets: vec![eda_api::NetParasitics {
                net: "y".into(),
                grounded_cap_pf: 0.12,
                coupling_cap_pf: 0.04,
                resistance_ohm: 0.0,
                estimated_delay_ns: 0.0,
            }],
            total_grounded_cap_pf: 0.12,
            total_coupling_cap_pf: 0.04,
            total_resistance_ohm: 0.0,
            worst_delay_ns: 0.0,
        };
        let result = eda_api::analyze_signal_quality_with_parasitics(
            &flow.netlist,
            &parasitics,
            &eda_api::QualityOptions {
                max_transition_ns: Some(10.0),
                max_capacitance_pf: Some(0.13),
                ..Default::default()
            },
        )?;
        let cap = result
            .violations
            .iter()
            .find(|v| v.kind == eda_api::QualityKind::MaxCapacitance)
            .ok_or_else(|| eda_api::signal_quality_report(&result))?;
        // Effective load includes half the coupling capacitance.
        if (cap.actual - 0.14).abs() > 1e-12 {
            Err(format!("actual cap {}", cap.actual))
        } else {
            Ok(())
        }
    });
    c.add("api_sdc_constrained_timing", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let sdc = r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_clock_uncertainty 0.1 -setup [get_clocks clk]
            set_input_delay 0.3 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
        "#;
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(sdc)?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        if (timing.worst_slack - 0.2).abs() > 1e-9 {
            Err(format!("worst slack {}", timing.worst_slack))
        } else {
            Ok(())
        }
    });
    c.add("api_block_timing_model", || {
        let src = "module m(input a, output y); assign y = ~a; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 2.0 [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let model = eda_api::extract_block_timing_model(&flow.netlist, &timing)?;
        let report = eda_api::block_timing_model_report(&model);
        let output = model.ports.get("y").ok_or("block model omitted y")?;
        if model.module == "m"
            && model.clock == "clk"
            && model.ports.len() == 2
            && output.clock.as_deref() == Some("clk")
            && (output.late_arrival_ns.ok_or("output arrival missing")? - 0.4).abs() < 1e-12
            && (output.setup_slack_ns.ok_or("output slack missing")? - 1.2).abs() < 1e-12
            && report.contains("Block timing model for 'm'")
            && report.contains("output 'y': clock 'clk', paths=1")
            && report.contains("late_arrival=0.400000 ns")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_block_timing_model_serialization", || {
        let src = "module m(input clk, d, output y); reg q; always @(posedge clk) q <= d; assign y = q; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_input_delay 0.3 -clock clk [get_ports d]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let model =
            eda_api::extract_block_timing_model(&flow.netlist, &timing)?;
        let text = eda_api::write_block_timing_model(&model)?;
        let parsed = eda_api::parse_block_timing_model(&text)?;
        if parsed == model
            && text.contains(r#"block "m" "clk" 2.0"#)
            && text.contains("port input \"d\" \"clk\"")
            && text.contains("port output \"y\" \"clk\"")
            && matches!(
                eda_api::parse_block_timing_model("port input d clk\ninternal_paths 0\n"),
                Err(error) if error.contains("must begin with a header")
            )
        {
            Ok(())
        } else {
            Err(text)
        }
    });
    c.add("api_block_timing_composition", || {
        let producer_text = r#"
            block "producer" "clk" 2
            port input "clk" "-" - - - - - - 0
            port output "y" "clk" 1.0 0.8 - 0.5 - - 1
            internal_paths 1
        "#;
        let consumer_text = r#"
            block "consumer" "clk" 2
            port input "clk" "-" - - - - - - 0
            port input "d" "clk" - - 1.2 0.3 0.85 0.2 1
            port output "q" "clk" 1.4 1.1 - 0.4 - - 1
            internal_paths 1
        "#;
        let models = [
            eda_api::parse_block_timing_model(producer_text)?,
            eda_api::parse_block_timing_model(consumer_text)?,
        ];
        let instances = [
            eda_api::BlockTimingCompositionInstance::new("u1", "producer"),
            eda_api::BlockTimingCompositionInstance::new("u2", "consumer"),
        ];
        let connections = [eda_api::BlockTimingCompositionConnection::new(
            "u1", "y", "u2", "d", 0.1,
        )];
        let result = eda_api::compose_block_timing_models(&models, &instances, &connections)?;
        let report = eda_api::block_timing_composition_report(&result);
        let mut wrong_clock = models[0].clone();
        wrong_clock.clock = "other".into();
        let wrong_models = [wrong_clock, models[1].clone()];
        let wrong_error =
            match eda_api::compose_block_timing_models(&wrong_models, &instances, &connections) {
                Err(error) => error,
                Ok(_) => "wrong-clock composition unexpectedly succeeded".into(),
            };
        if result.is_clean()
            && result.checks.len() == 1
            && (result.checks[0].required_ns - 1.2).abs() < 1e-12
            && (result.checks[0].slack_ns - 0.1).abs() < 1e-12
            && result.hold_checks.len() == 1
            && (result.hold_checks[0].arrival_ns - 0.9).abs() < 1e-12
            && (result.hold_checks[0].slack_ns - 0.05).abs() < 1e-12
            && result.unconnected_outputs == vec!["u2:q".to_string()]
            && report.contains("Block timing composition:")
            && report.contains("setup u1:y -> u2:d")
            && report.contains("hold u1:y -> u2:d")
            && wrong_error.contains("block timing instance 'u2' uses clock 'clk', expected 'other'")
        {
            Ok(())
        } else {
            Err(format!("{}\n{}", report, wrong_error))
        }
    });
    c.add("api_block_timing_composition_clock_mapping", || {
        let producer = r#"
            block "producer" "pclk" 2
            port input "pclk" "-" - - - - - - 0
            port output "y" "pclk" 1.0 0.8 - 0.5 - - 1
            internal_paths 0
        "#;
        let consumer = r#"
            block "consumer" "dclk" 2
            port input "dclk" "-" - - - - - - 0
            port input "d" "dclk" - - 1.2 0.3 0.85 0.2 1
            internal_paths 0
        "#;
        let models = [
            eda_api::parse_block_timing_model(producer)?,
            eda_api::parse_block_timing_model(consumer)?,
        ];
        let instances = [
            eda_api::BlockTimingCompositionInstance::new("u1", "producer"),
            eda_api::BlockTimingCompositionInstance::new("u2", "consumer"),
        ];
        let connections = [eda_api::BlockTimingCompositionConnection::new(
            "u1", "y", "u2", "d", 0.1,
        )];
        let clock_maps = [
            eda_api::BlockTimingClockMap::new("u1").with_clock("pclk", "top_clk"),
            eda_api::BlockTimingClockMap::new("u2").with_clock("dclk", "top_clk"),
        ];
        let result = eda_api::compose_block_timing_models_with_clock_maps(
            &models,
            &instances,
            &connections,
            &clock_maps,
        )?;
        let report = eda_api::block_timing_composition_report(&result);
        if result.clock == "top_clk"
            && result.clock_maps.len() == 2
            && result.checks[0].clock == "top_clk -> top_clk"
            && result.hold_checks[0].clock == "top_clk -> top_clk"
            && result.is_clean()
            && report.contains("clock map u1: pclk -> top_clk")
            && report.contains("clock map u2: dclk -> top_clk")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_block_timing_composition_clock_domains", || {
        let producer = r#"
            block "producer" "pclk" 4
            port input "pclk" "-" - - - - - - 0
            port output "y" "pclk" 1.0 0.7 - 0.5 - - 1
            internal_paths 0
        "#;
        let consumer = r#"
            block "consumer" "dclk" 6
            port input "dclk" "-" - - - - - - 0
            port input "d" "dclk" - - 1.2 0.3 0.4 0.2 1
            internal_paths 0
        "#;
        let models = [
            eda_api::parse_block_timing_model(producer)?,
            eda_api::parse_block_timing_model(consumer)?,
        ];
        let instances = [
            eda_api::BlockTimingCompositionInstance::new("u1", "producer"),
            eda_api::BlockTimingCompositionInstance::new("u2", "consumer"),
        ];
        let connections = [eda_api::BlockTimingCompositionConnection::new(
            "u1", "y", "u2", "d", 0.1,
        )];
        let clock_maps = [
            eda_api::BlockTimingClockMap::new("u1").with_clock("pclk", "source_clk"),
            eda_api::BlockTimingClockMap::new("u2").with_clock("dclk", "capture_clk"),
        ];
        let clocks = [
            eda_api::BlockTimingClock::new("source_clk", 4.0, 0.0),
            eda_api::BlockTimingClock::new("capture_clk", 6.0, 0.2),
        ];
        let relationships = [eda_api::BlockTimingClockRelationship::new(
            "source_clk",
            "capture_clk",
            0.2,
        )];
        let result = eda_api::compose_block_timing_models_with_clock_domains(
            &models,
            &instances,
            &connections,
            &clock_maps,
            &clocks,
            &relationships,
        )?;
        let report = eda_api::block_timing_composition_report(&result);
        let wrong_period = [
            eda_api::BlockTimingClock::new("capture_clk", 5.0, 0.2),
            clocks[0].clone(),
        ];
        if result.is_clean()
            && result.checks[0].clock == "source_clk -> capture_clk"
            && result.clocks.len() == 2
            && result.clock_relationships.len() == 1
            && report.contains("clock 'source_clk': period 4.000000 ns")
            && report.contains("clock relationship source_clk -> capture_clk")
            && matches!(
                eda_api::compose_block_timing_models_with_clock_domains(
                    &models,
                    &instances,
                    &connections,
                    &clock_maps,
                    &wrong_period,
                    &relationships,
                ),
                Err(error) if error.contains("does not match clock 'capture_clk' period")
            )
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_constrained_timing_path_objects", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 1.0 -clock clk [get_ports d]
            set_clock_uncertainty 0.1 -setup [get_clocks clk]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let paths = eda_api::constrained_timing_paths(&flow.netlist, &timing, 2);
        let report = eda_api::constrained_timing_path_report(&flow.netlist, &paths);
        if paths.len() == 2
            && paths
                .windows(2)
                .all(|pair| pair[0].slack_ns <= pair[1].slack_ns)
            && paths
                .iter()
                .all(|path| path.required_ns >= path.path.arrival_ns)
            && report.contains("Constrained timing path report")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_constrained_timing_path_clock_metadata", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_clock_latency -source 0.1 [get_clocks clk]
            set_clock_latency 0.2 [get_clocks clk]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let paths = eda_api::constrained_timing_paths(&flow.netlist, &timing, 0);
        let report = eda_api::constrained_timing_path_report(&flow.netlist, &paths);
        if paths.iter().any(|path| {
            path.clocks.launch.clock == "clk"
                && path.clocks.capture.clock == "clk"
                && path.clocks.launch.source_latency_ns == timing.source_latency
                && path.clocks.capture.source_latency_ns == timing.source_latency
                && path.clocks.edge_separation_ns.is_finite()
                && path.clocks.insertion_skew_ns.is_finite()
        }) && report.contains("clocks:")
            && report.contains("insertion skew")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_false_path_excluded_timing_path_objects", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_false_path -from [get_ports a] -to [get_ports y]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let paths = eda_api::constrained_timing_paths(&flow.netlist, &timing, 0);
        let excluded = eda_api::excluded_timing_paths(&flow.netlist, &timing);
        let report = eda_api::excluded_timing_path_report(&flow.netlist, &excluded);
        if paths.is_empty()
            && excluded.len() == 1
            && excluded[0].check == eda_api::ExcludedTimingCheck::Setup
            && excluded[0].exception.from.as_deref() == Some("a")
            && report.contains("Excluded timing path report: 1 path(s)")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_timing_exception_preserves_typed_pin_object", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_multicycle_path 2 -to [get_pins u2_dff/d]
        "#,
        )?;
        if constraints.multicycle_paths.len() != 1 {
            return Err("multicycle path not parsed".into());
        }
        let object = constraints.multicycle_paths[0]
            .to_object
            .as_ref()
            .ok_or("typed object missing")?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let paths = eda_api::constrained_timing_paths(&flow.netlist, &timing, 0);
        let capture_has_exception = paths.iter().any(|path| {
            path.exceptions.len() == 1
                && path.exceptions[0]
                    .to_object
                    .as_ref()
                    .is_some_and(|path_object| path_object == object)
        });
        if object.kind == eda_api::TimingObjectKind::Pin
            && object.name == "u2_dff/d"
            && constraints.multicycle_paths[0].to.as_deref() == Some("u2_dff/d")
            && capture_has_exception
        {
            Ok(())
        } else {
            Err(format!("{:?}", object))
        }
    });
    c.add("api_pin_delay_limit_objects", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_max_delay 2.0 -to [get_pins u2_dff/d]
            set_min_delay 1.0 -to [get_pins u2_dff/d]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let paths = eda_api::constrained_timing_paths(&flow.netlist, &timing, 0);
        let report = eda_api::constrained_timing_path_report(&flow.netlist, &paths);
        if constraints.max_delay_paths.len() == 1
            && constraints.min_delay_paths.len() == 1
            && constraints.max_delay_paths[0]
                .to_object
                .as_ref()
                .is_some_and(|object| {
                    object.kind == eda_api::TimingObjectKind::Pin && object.name == "u2_dff/d"
                })
            && paths.iter().any(|path| {
                path.delay_limits.len() == 1
                    && path.delay_limits[0].maximum
                    && path.delay_limits[0].limit.limit == 2.0
            })
            && report.contains("delay limits:")
            && report.contains("max 2.000000 ns, slack 0.800000 ns")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_ordered_through_delay_limit_objects", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_max_delay 2.0 -through [get_pins u1_dff/q] -through [get_pins u2_dff/d]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let paths = eda_api::constrained_timing_paths(&flow.netlist, &timing, 0);
        let capture = paths
            .iter()
            .find(|path| {
                matches!(
                    path.path.endpoint,
                    eda_api::TimingPathEndpoint::SequentialCell(_)
                ) && path
                    .path
                    .nets
                    .last()
                    .is_some_and(|net| flow.netlist.net(*net).name == "q1")
            })
            .ok_or("missing capture path")?;
        let output = paths
            .iter()
            .find(|path| path.path.endpoint == eda_api::TimingPathEndpoint::OutputPort("q1".into()))
            .ok_or("missing output path")?;
        if constraints.max_delay_paths.len() == 1
            && constraints.max_delay_paths[0].through_groups.len() == 2
            && capture.delay_limits.len() == 1
            && (capture.delay_limits[0].slack_ns - 0.8).abs() < 1e-12
            && output.delay_limits.is_empty()
        {
            Ok(())
        } else {
            Err(format!(
                "capture limits={}, output limits={}",
                capture.delay_limits.len(),
                output.delay_limits.len()
            ))
        }
    });
    c.add("api_wildcard_timing_objects_match_hierarchy_levels", || {
        let literal = eda_api::TimingObjectPattern::new("top/u1");
        let local_wildcard = eda_api::TimingObjectPattern::new("top/u*");
        let recursive = eda_api::TimingObjectPattern::new("**/u1");
        let pin = eda_api::TimingObjectRef::new(eda_api::TimingObjectKind::Pin, "**/u1/y");
        if !literal.is_pattern()
            && literal.matches("top/u1")
            && local_wildcard.matches("top/u10")
            && !local_wildcard.matches("block/top/u10")
            && recursive.matches("block/top/u1")
            && pin.matches_name("a/b/u1/y")
            && !pin.matches_name("a/b/u1/x")
        {
            Ok(())
        } else {
            Err("hierarchical wildcard semantics mismatch".into())
        }
    });
    c.add("api_disable_timing_arc_objects", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_disable_timing -from [get_pins u1_dff/clk] -to [get_pins u1_dff/q]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let disabled = constraints
            .disabled_timing_arcs
            .first()
            .ok_or("missing disabled arc object")?;
        if disabled.from.as_ref().is_some_and(|object| {
            object.kind == eda_api::TimingObjectKind::Pin && object.name == "u1_dff/clk"
        }) && disabled.to.as_ref().is_some_and(|object| {
            object.kind == eda_api::TimingObjectKind::Pin && object.name == "u1_dff/q"
        }) && timing.base.disabled_timing_arcs == 1
            && eda_api::constrained_timing_report(&timing).contains("disabled arcs : 1")
        {
            Ok(())
        } else {
            Err(eda_api::constrained_timing_report(&timing))
        }
    });
    c.add("api_case_analysis_constant_propagation", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_case_analysis 0 [get_ports a]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let paths = eda_api::timing_paths(&flow.netlist, &timing.base, 0);
        let analysis = constraints
            .case_analyses
            .first()
            .ok_or("missing case analysis object")?;
        if !analysis.value
            && analysis.object.kind == eda_api::TimingObjectKind::Port
            && analysis.object.name == "a"
            && timing.base.case_analysis_nets.len() == 2
            && paths.is_empty()
            && eda_api::constrained_timing_report(&timing).contains("case constants: 2")
        {
            Ok(())
        } else {
            Err(eda_api::constrained_timing_report(&timing))
        }
    });
    c.add("api_incremental_timing_update", || {
        let src = "module chain(input a, b, output y);
            wire n;
            assign n = a & b;
            assign y = ~n;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let old = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_input_delay 0.2 -clock clk [get_ports a]
        "#,
        )?;
        let mut new = old.clone();
        new.set_input_delay(eda_api::PortDelay {
            port: "a".into(),
            clock: "clk".into(),
            delay: 1.0,
        })?;
        let previous = eda_api::constrained_timing(&flow.netlist, &old)?.base;
        let incremental =
            eda_api::update_timing_for_input_delays(&flow.netlist, &previous, &new, ["a"])?;
        let paths = eda_api::timing_paths(&flow.netlist, &incremental.timing, 0);
        if incremental.changed_ports.contains("a")
            && incremental.recomputed_cells.len() == 1
            && paths.len() == 1
            && (paths[0].arrival_ns - 1.7).abs() < 1e-12
            && paths[0].arcs.len() == 2
        {
            Ok(())
        } else {
            Err(format!(
                "cells={}, paths={}",
                incremental.recomputed_cells.len(),
                paths.len()
            ))
        }
    });
    c.add("api_incremental_timing_derate_update", || {
        let src = "module chain(input a, b, output y);
            wire n;
            assign n = a & b;
            assign y = ~n;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let old =
            eda_api::parse_timing_constraints("create_clock -name clk -period 4.0 [get_ports a]")?;
        let mut new = old.clone();
        new.set_pin_timing_derate("u1_nand2", "a", Some(1.0), Some(2.0))?;
        let previous = eda_api::constrained_timing(&flow.netlist, &old)?.base;
        let changed_pin = eda_api::TimingPin::new("u1_nand2", "a");
        let incremental = eda_api::update_timing_for_timing_derates(
            &flow.netlist,
            &previous,
            &new,
            Vec::<&str>::new(),
            &[changed_pin],
        )?;
        let full = eda_api::constrained_timing(&flow.netlist, &new)?.base;
        let paths = eda_api::timing_paths(&flow.netlist, &incremental.timing, 0);
        if incremental.recomputed_cells.contains("u1_nand2")
            && incremental.timing.critical_path_delay == full.critical_path_delay
            && paths.len() == 1
            && (paths[0].arrival_ns - 1.4).abs() < 1e-12
            && paths[0].arcs.len() == 2
        {
            Ok(())
        } else {
            Err(format!(
                "critical={}, full={}, paths={}",
                incremental.timing.critical_path_delay,
                full.critical_path_delay,
                paths.len()
            ))
        }
    });
    c.add("api_incremental_topology_update_after_eco", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let old_flow = eda_api::run_flow(src, None)?;
        let patch =
            eda_api::parse_eco_patch("eco timing; insert_buffer u1_and2 0 timing_buf timing_net;")?;
        let eco = eda_api::apply_eco_patch(&old_flow.netlist, &patch)?;
        let constraints =
            eda_api::parse_timing_constraints("create_clock -name clk -period 4.0 [get_ports a]")?;
        let previous = eda_api::constrained_timing(&old_flow.netlist, &constraints)?.base;
        let diff = eda_api::diff_timing_topology(&old_flow.netlist, &eco.netlist);
        let incremental = eda_api::update_timing_for_topology(
            &old_flow.netlist,
            &previous,
            &eco.netlist,
            &constraints,
        )?;
        let full = eda_api::constrained_timing(&eco.netlist, &constraints)?.base;
        let paths = eda_api::timing_paths(&eco.netlist, &incremental.timing, 0);
        if diff.added_cells.contains("timing_buf")
            && diff.added_nets.contains("timing_net")
            && incremental.recomputed_cells.contains("timing_buf")
            && incremental.recomputed_cells.contains("u1_and2")
            && incremental.timing.critical_path_delay == full.critical_path_delay
            && paths.len() == 1
            && (paths[0].arrival_ns - 1.5).abs() < 1e-12
        {
            Ok(())
        } else {
            Err(format!(
                "diff={diff:?}, cells={:?}, critical={}, full={}",
                incremental.recomputed_cells,
                incremental.timing.critical_path_delay,
                full.critical_path_delay
            ))
        }
    });
    c.add("api_incremental_constraint_update", || {
        let src = "module chain(input a, output y); assign y = ~a; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let old =
            eda_api::parse_timing_constraints("create_clock -name clk -period 4.0 [get_ports a]")?;
        let new = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_false_path -to [get_ports y]
        "#,
        )?;
        let previous = eda_api::constrained_timing(&flow.netlist, &old)?.base;
        let diff = eda_api::diff_timing_constraints(&old, &new);
        let updated = eda_api::update_timing_for_constraints(&flow.netlist, &previous, &old, &new)?;
        if diff.timing_exceptions_changed
            && !diff.base_recompute_required
            && updated.recomputed_cells.is_empty()
            && updated.timing.critical_path_delay == previous.critical_path_delay
            && updated.constrained.output_slack.is_infinite()
        {
            Ok(())
        } else {
            Err(format!(
                "base={}, cells={}, slack={}",
                updated.full_base_recompute,
                updated.recomputed_cells.len(),
                updated.constrained.output_slack
            ))
        }
    });
    c.add("api_timing_violation_store", || {
        let src = "module m(input a, output y); assign y = ~a; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let failing =
            eda_api::parse_timing_constraints("create_clock -name clk -period 0.2 [get_ports a]")?;
        let passing =
            eda_api::parse_timing_constraints("create_clock -name clk -period 4.0 [get_ports a]")?;
        let failing_timing = eda_api::constrained_timing(&flow.netlist, &failing)?;
        let mut store = eda_api::timing_violation_store("api", &flow.netlist, &failing_timing)?;
        let passing_timing = eda_api::constrained_timing(&flow.netlist, &passing)?;
        let diff = eda_api::update_timing_violations(&mut store, &flow.netlist, &passing_timing)?;
        let report = eda_api::timing_violation_report(&store, Some(&diff));
        if store.scenario == "api"
            && store.generation == 2
            && store.is_empty()
            && diff.removed.contains_key("api:output:y")
            && report.contains("Timing violation report for 'api'")
            && report.contains("lifecycle: added=0, removed=1")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_timing_violation_waivers", || {
        let src = "module m(input a, output y); assign y = ~a; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let failing =
            eda_api::parse_timing_constraints("create_clock -name clk -period 0.2 [get_ports a]")?;
        let timing = eda_api::constrained_timing(&flow.netlist, &failing)?;
        let mut store = eda_api::timing_violation_store("api", &flow.netlist, &timing)?;
        let waiver = eda_api::timing_violation_waiver("accepted-output", "api", "y", -1.0)?
            .with_kind(eda_api::TimingViolationKind::Output)
            .with_owner("timing")
            .with_note("reviewed");
        eda_api::add_timing_violation_waiver(&mut store, waiver)?;
        let waived_ok = store.is_empty()
            && store.violations().len() == 1
            && store.waived_violations().get("api:output:y")
                == Some(&"accepted-output".to_string());
        eda_api::remove_timing_violation_waiver(&mut store, "accepted-output")?;

        let slow =
            eda_api::parse_timing_constraints("create_clock -name clk -period 4.0 [get_ports a]")?;
        let fast =
            eda_api::parse_timing_constraints("create_clock -name clk -period 0.2 [get_ports a]")?;
        let mut registry = eda_api::timing_violation_registry([
            eda_api::TimingScenario::new("slow", &flow.netlist, &slow),
            eda_api::TimingScenario::new("fast", &flow.netlist, &fast),
        ])?;
        let registry_waiver = eda_api::timing_violation_waiver("fast-only", "fast", "y", -1.0)?;
        eda_api::add_timing_violation_registry_waiver(&mut registry, registry_waiver)?;
        let registry_waived = registry.is_empty() && registry.summaries().is_empty();
        eda_api::remove_timing_violation_registry_waiver(&mut registry, "fast-only")?;
        if waived_ok
            && registry_waived
            && registry.summaries().contains_key("output:y")
            && eda_api::timing_violation_registry_report(&registry, None)
                .contains("waived violations: 0")
        {
            Ok(())
        } else {
            Err("API waiver behavior mismatch".into())
        }
    });
    c.add("api_hierarchy_scoped_timing_violation_waiver", || {
        let src = "module inv(input a, output y); assign y = ~a; endmodule
            module sub(input a, output y); inv u_leaf(.a(a), .y(y)); endmodule
            module top(input a, output y1, y2);
                sub u1(.a(a), .y(y1));
                sub u2(.a(a), .y(y2));
            endmodule";
        let flow = eda_api::run_flow(src, Some("top"))?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 0.1 [get_ports a]
            set_output_delay 0.2 -clock clk [get_ports y1]
            set_output_delay 0.2 -clock clk [get_ports y2]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let mut store = eda_api::timing_violation_store("api", &flow.netlist, &timing)?;
        let first = store
            .violations()
            .get("api:output:y1")
            .ok_or("hierarchical flow omitted y1 violation")?;
        let second = store
            .violations()
            .get("api:output:y2")
            .ok_or("hierarchical flow omitted y2 violation")?;
        let first_hierarchy = first
            .hierarchy
            .clone()
            .ok_or("y1 violation omitted hierarchy")?;
        let second_hierarchy = second
            .hierarchy
            .clone()
            .ok_or("y2 violation omitted hierarchy")?;

        let waiver = eda_api::timing_violation_waiver("one-instance", "api", "*", -10.0)?
            .with_instance_pattern(first_hierarchy.clone());
        eda_api::add_timing_violation_waiver(&mut store, waiver.clone())?;
        let text = eda_api::write_timing_violation_waivers(&[waiver])?;
        let parsed = eda_api::parse_timing_violation_waivers(&text)?;
        let report = eda_api::timing_violation_report(&store, None);
        if first_hierarchy != second_hierarchy
            && store.waived_len() == 1
            && store.len() == 1
            && parsed.len() == 1
            && parsed[0].instance_pattern.as_deref() == Some(first_hierarchy.as_str())
            && report.contains("hierarchy: ")
        {
            Ok(())
        } else {
            Err(format!(
                "{first_hierarchy:?} {second_hierarchy:?}\n{report}\n{text}"
            ))
        }
    });
    c.add("api_multicorner_hierarchy_violation_summary", || {
        let src = "module inv(input a, output y); assign y = ~a; endmodule
            module sub(input a, output y); inv u_leaf(.a(a), .y(y)); endmodule
            module top(input a, output y1, y2);
                sub u1(.a(a), .y(y1));
                sub u2(.a(a), .y(y2));
            endmodule";
        let flow = eda_api::run_flow(src, Some("top"))?;
        let slow = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 2.0 [get_ports a]
            set_output_delay 0.2 -clock clk [get_ports y1]
            set_output_delay 0.2 -clock clk [get_ports y2]
        "#,
        )?;
        let fast = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 0.1 [get_ports a]
            set_output_delay 0.2 -clock clk [get_ports y1]
            set_output_delay 0.2 -clock clk [get_ports y2]
        "#,
        )?;
        let mut registry = eda_api::timing_violation_registry([
            eda_api::TimingScenario::new("slow", &flow.netlist, &slow),
            eda_api::TimingScenario::new("fast", &flow.netlist, &fast),
        ])?;
        let waiver = eda_api::timing_violation_waiver("accepted-u1", "*", "*", -10.0)?
            .with_instance_pattern("top.u1.u_leaf");
        eda_api::add_timing_violation_registry_waiver(&mut registry, waiver)?;
        let surviving = registry
            .summaries()
            .get("output:y2")
            .ok_or("hierarchy waiver removed surviving y2 summary")?;
        let owner = Some("top.u2.u_leaf".to_string());
        let surviving_owner = surviving
            .hierarchy_summary
            .get(&owner)
            .ok_or("surviving y2 hierarchy summary missing")?;
        let registry_hierarchies: std::collections::BTreeMap<
            Option<String>,
            eda_api::CrossCornerHierarchyViolationSummary,
        > = registry.hierarchy_summaries();
        let report = eda_api::timing_violation_registry_report(&registry, None);
        if registry.summaries().contains_key("output:y1")
            || !registry.summaries().contains_key("output:y2")
            || surviving_owner.worst_scenario != "fast"
            || surviving_owner.violations != 1
            || surviving_owner
                .kinds
                .get(&eda_api::TimingViolationKind::Output)
                .copied()
                != Some(1)
            || registry_hierarchies.len() != 1
            || !registry_hierarchies.contains_key(&owner)
            || !report.contains("hierarchy distribution:")
            || !report.contains("'top.u2.u_leaf': violations=1")
            || report.contains("'top.u1.u_leaf': violations=1")
        {
            Err(report)
        } else {
            Ok(())
        }
    });
    c.add("api_timing_waiver_serialization_and_audit", || {
        let src = "module m(input a, output y); assign y = ~a; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints =
            eda_api::parse_timing_constraints("create_clock -name clk -period 0.2 [get_ports a]")?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let waiver = eda_api::timing_violation_waiver("saved", "api", "y", -1.0)?
            .with_kind(eda_api::TimingViolationKind::Output)
            .with_owner("timing")
            .with_note("accepted by review")
            .expires_after(9);
        let text = eda_api::write_timing_violation_waivers(std::slice::from_ref(&waiver))?;
        let parsed = eda_api::parse_timing_violation_waivers(&text)?;
        let mut store = eda_api::timing_violation_store("api", &flow.netlist, &timing)?;
        eda_api::add_timing_violation_waiver_with_actor(
            &mut store,
            waiver.clone(),
            Some("engineer"),
        )?;
        let report = eda_api::timing_waiver_audit_report(store.audit());
        if parsed == vec![waiver.clone()]
            && text.contains("expires_after 9")
            && store.audit().len() == 1
            && store.audit()[0].changed_by.as_deref() == Some("engineer")
            && report.contains("#1 added 'saved' at generation 1 by engineer")
        {
            Ok(())
        } else {
            Err(format!("{}\n{}", text, report))
        }
    });
    c.add("api_timing_waiver_audit_hierarchy_summary", || {
        let text = r#"
            audit 1 added 1 "frontend"
            waiver "u1" "fast" "y" "-1.0"
            instance "top.u1.**"
            end
            audit 2 removed 2 "reviewer"
            waiver "u1" "fast" "y" "-1.0"
            instance "top.u1.**"
            end
            audit 3 added 2 "frontend"
            waiver "u2" "fast" "y" "-1.0"
            instance "top.u2"
            end
        "#;
        let entries = eda_api::parse_timing_waiver_audit(text)?;
        let summaries: std::collections::BTreeMap<
            Option<String>,
            eda_api::TimingWaiverAuditHierarchySummary,
        > = eda_api::timing_waiver_audit_hierarchy_summaries(&entries)?;
        let report = eda_api::timing_waiver_audit_hierarchy_report(&entries)?;
        let first_key = Some("top.u1.**".to_string());
        let second_key = Some("top.u2".to_string());
        let first = summaries
            .get(&first_key)
            .ok_or("missing first hierarchy summary")?;
        let second = summaries
            .get(&second_key)
            .ok_or("missing second hierarchy summary")?;
        if entries.len() == 3
            && summaries.len() == 2
            && first.events == 2
            && first.added_events == 1
            && first.removed_events == 1
            && first.active_waiver_ids.is_empty()
            && second.events == 1
            && second.active_waiver_ids.contains("u2")
            && report.contains("'top.u1.**': events=2, added=1, removed=1, active=0")
            && report.contains("'top.u2': events=1, added=1, removed=0, active=1")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_incremental_timing_violation_dirty_objects", || {
        let src = "module m(input a, b, output y, z);
            assign y = a & b;
            assign z = a | b;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let old_constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_max_delay 0.5 -from [get_ports a] -to [get_ports y]
            set_max_delay 0.5 -from [get_ports a] -to [get_ports z]
        "#,
        )?;
        let new_constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_max_delay 1.5 -from [get_ports a] -to [get_ports y]
            set_max_delay 0.5 -from [get_ports a] -to [get_ports z]
        "#,
        )?;
        let old_timing = eda_api::constrained_timing(&flow.netlist, &old_constraints)?;
        let new_timing = eda_api::constrained_timing(&flow.netlist, &new_constraints)?;
        let mut store = eda_api::timing_violation_store("api", &flow.netlist, &old_timing)?;
        let untouched_slack = store.violations()["api:max_delay:z"].slack_ns;
        let mut dirty = eda_api::TimingViolationDirtyObjects::default();
        dirty.add_net("y");
        let diff = eda_api::update_timing_violations_incremental(
            &mut store,
            &flow.netlist,
            &new_timing,
            &dirty,
        )?;
        let report = eda_api::timing_violation_report(&store, Some(&diff));
        let mut registry = eda_api::timing_violation_registry([
            eda_api::TimingScenario::new("api", &flow.netlist, &old_constraints),
            eda_api::TimingScenario::new("other", &flow.netlist, &old_constraints),
        ])?;
        eda_api::update_timing_violation_registry_scenario_incremental(
            &mut registry,
            eda_api::TimingScenario::new("api", &flow.netlist, &new_constraints),
            &dirty,
        )?;
        if store.violations().len() == 1
            && store.violations().contains_key("api:max_delay:z")
            && !store.violations().contains_key("api:max_delay:y")
            && store.violations()["api:max_delay:z"].slack_ns == untouched_slack
            && diff.removed.len() == 1
            && diff.added.is_empty()
            && registry.stores()["other"].violations().len() == 2
            && registry.stores()["api"].violations().len() == 1
            && report.contains("lifecycle: added=0, removed=1")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add(
        "api_timing_violation_dirty_objects_from_timing_diffs",
        || {
            let src = "module m(input a, b, output y, z);
            assign y = a & b;
            assign z = a | b;
         endmodule";
            let flow = eda_api::run_flow(src, None)?;
            let old_constraints = eda_api::parse_timing_constraints(
                r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_output_delay 4.0 -clock clk [get_ports y]
            set_output_delay 4.0 -clock clk [get_ports z]
        "#,
            )?;
            let new_constraints = eda_api::parse_timing_constraints(
                r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_output_delay 3.0 -clock clk [get_ports y]
            set_output_delay 4.0 -clock clk [get_ports z]
        "#,
            )?;
            let old_timing = eda_api::constrained_timing(&flow.netlist, &old_constraints)?;
            let new_timing = eda_api::constrained_timing(&flow.netlist, &new_constraints)?;
            let dirty = eda_api::timing_violation_dirty_objects_from_constraints(
                &old_constraints,
                &new_constraints,
            );
            let mut store = eda_api::timing_violation_store("api", &flow.netlist, &old_timing)?;
            let untouched_slack = store.violations()["api:output:z"].slack_ns;
            let diff = eda_api::update_timing_violations_incremental(
                &mut store,
                &flow.netlist,
                &new_timing,
                &dirty,
            )?;
            let mut topology = eda_api::TimingTopologyDiff::default();
            topology.changed_cells.insert("u1".into());
            topology.changed_nets.insert("y".into());
            let derived_topology = eda_api::timing_violation_dirty_objects_from_topology(&topology);
            if dirty.nets.contains("y")
                && !dirty.nets.contains("z")
                && dirty.clocks.is_empty()
                && !store.violations().contains_key("api:output:y")
                && store.violations()["api:output:z"].slack_ns == untouched_slack
                && diff.removed.len() == 1
                && derived_topology.cells.contains("u1")
                && derived_topology.nets.contains("y")
            {
                Ok(())
            } else {
                Err(eda_api::timing_violation_report(&store, Some(&diff)))
            }
        },
    );
    c.add("api_timing_waiver_audit_round_trip_and_restore", || {
        let src = "module m(input a, output y); assign y = ~a; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints =
            eda_api::parse_timing_constraints("create_clock -name clk -period 0.2 [get_ports a]")?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let mut source = eda_api::timing_violation_store("api", &flow.netlist, &timing)?;
        let waiver =
            eda_api::timing_violation_waiver("session", "api", "y", -1.0)?.with_owner("timing");
        eda_api::add_timing_violation_waiver_with_actor(&mut source, waiver, Some("engineer"))?;
        let text = eda_api::write_timing_waiver_audit(source.audit())?;
        let entries = eda_api::parse_timing_waiver_audit(&text)?;
        let mut restored = eda_api::timing_violation_store("api", &flow.netlist, &timing)?;
        eda_api::restore_timing_waiver_audit(&mut restored, &entries)?;
        if entries == source.audit()
            && restored.audit() == source.audit()
            && restored.waivers().len() == 1
            && restored.is_empty()
            && restored.violations().len() == 1
            && text.contains("audit 1 added 1 \"engineer\"")
        {
            Ok(())
        } else {
            Err(text)
        }
    });
    c.add("api_multicorner_violation_registry", || {
        let src = "module m(input a, output y); assign y = ~a; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let slow =
            eda_api::parse_timing_constraints("create_clock -name clk -period 4.0 [get_ports a]")?;
        let fast =
            eda_api::parse_timing_constraints("create_clock -name clk -period 0.2 [get_ports a]")?;
        let old_views = [
            eda_api::TimingScenario::new("slow", &flow.netlist, &slow),
            eda_api::TimingScenario::new("fast", &flow.netlist, &fast),
        ];
        let mut registry = eda_api::timing_violation_registry(old_views)?;
        let new_views = [
            eda_api::TimingScenario::new("slow", &flow.netlist, &slow),
            eda_api::TimingScenario::new("slow2", &flow.netlist, &slow),
        ];
        let update = eda_api::update_timing_violation_registry(&mut registry, new_views)?;
        let report = eda_api::timing_violation_registry_report(&registry, Some(&update));
        if registry.stores().len() == 2
            && registry.is_empty()
            && update.removed_scenarios.contains_key("fast")
            && update.added_scenarios.contains_key("slow2")
            && report.contains("Multi-corner timing violation report")
            && report.contains("scenarios: added=1, removed=1")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_hold_timing_path_objects", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let paths = eda_api::hold_timing_paths(&flow.netlist, &flow.timing, 0.05, 0);
        let report = eda_api::hold_timing_path_report(&flow.netlist, &paths);
        if paths.len() == 2
            && paths
                .iter()
                .all(|path| (path.path.arrival_ns - path.slack_ns - path.required_ns).abs() < 1e-12)
            && report.contains("Hold timing path report: 2 path(s)")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_constrained_hold_timing_path_objects", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 1.0 -clock clk [get_ports d]
            set_clock_uncertainty 0.05 -hold [get_clocks clk]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let paths = eda_api::constrained_hold_timing_paths(&flow.netlist, &timing, 0);
        let report = eda_api::constrained_hold_timing_path_report(&flow.netlist, &paths);
        if paths.len() == 2
            && paths
                .windows(2)
                .all(|pair| pair[0].slack_ns <= pair[1].slack_ns)
            && paths
                .iter()
                .all(|path| path.path.path.arrival_ns >= path.required_ns)
            && report.contains("Constrained hold timing path report")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
}
