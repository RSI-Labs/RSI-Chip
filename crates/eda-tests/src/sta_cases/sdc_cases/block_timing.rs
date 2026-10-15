//! SDC/MMMC child regression domain.

use super::super::*;

pub(super) fn register_block_timing(c: &mut CaseList) {
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
}
