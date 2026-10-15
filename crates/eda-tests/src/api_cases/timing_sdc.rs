//! Timing flows, SDC checks, derates, hold repair, MMMC, and incremental timing.

use super::*;

pub(super) fn register(c: &mut CaseList) {
    c.add("api_stats_flow", || {
        let src = "module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let st = &flow.stats;
        if st.seq != 1 {
            Err(format!("seq = {}", st.seq))
        } else {
            Ok(())
        }
    });
    c.add("api_timing_flow", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        if flow.timing.critical_path_delay <= 0.0 {
            Err("no timing".into())
        } else {
            Ok(())
        }
    });
    c.add("api_timing_path_objects", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let paths = eda_api::timing_paths(&flow.netlist, &flow.timing, 1);
        let report = eda_api::timing_path_report(&flow.netlist, &paths);
        if paths.len() == 1
            && paths[0].kind == eda_api::TimingPathKind::InputToOutput
            && matches!(paths[0].launch, eda_api::TimingLaunch::PrimaryInput(_))
            && paths[0].endpoint == eda_api::TimingPathEndpoint::OutputPort("y".into())
            && report.contains("Timing path report")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_timing_path_arc_objects", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let paths = eda_api::timing_paths(&flow.netlist, &flow.timing, 1);
        let report = eda_api::timing_path_report(&flow.netlist, &paths);
        let Some(path) = paths.first() else {
            return Err("no API timing path".into());
        };
        let arc_sum = path.arcs.iter().map(|arc| arc.delay_ns).sum::<f64>();
        if path.arcs.len() == 2
            && path.arcs[0].kind == eda_api::TimingArcKind::PrimaryInputLaunch
            && path.arcs[1].kind == eda_api::TimingArcKind::Combinational
            && path.arcs[1].cell.is_some()
            && (path.arcs[1].delay_ns - 1.0).abs() < 1e-12
            && (arc_sum - path.arrival_ns).abs() < 1e-12
            && report.contains("arcs:")
            && report.contains("combinational 1.000000 ns")
        {
            Ok(())
        } else {
            Err(format!(
                "sum={arc_sum}, arrival={}\n{}",
                path.arrival_ns, report
            ))
        }
    });
    c.add("api_hold_timing_analysis", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.8 -clock clk [get_ports d]
            set_clock_uncertainty 0.15 -hold [get_clocks clk]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let endpoints: &[eda_api::HoldEndpoint] = flow.timing.hold_endpoint_list();
        if timing.hold_slack <= 0.0 || endpoints.len() < 2 {
            Err(format!(
                "hold slack={} endpoints={}",
                timing.hold_slack,
                endpoints.len()
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_sdc_path_delay_limits", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_max_delay 1.1 -from [get_ports a] -to [get_ports y]
            set_min_delay 0.4 -from [get_ports a] -to [get_ports y]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        if (timing.max_delay_slack - 0.1).abs() > 1e-9
            || (timing.min_delay_slack - 0.6).abs() > 1e-9
        {
            Err(format!(
                "max={} min={}",
                timing.max_delay_slack, timing.min_delay_slack
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_sdc_min_pulse_width", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 -fall 0.3 [get_ports clk]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        if (timing.min_pulse_high_slack - 0.05).abs() > 1e-9
            || (timing.min_pulse_width_slack - 0.05).abs() > 1e-9
        {
            Err(format!(
                "high={} low={} worst={}",
                timing.min_pulse_high_slack,
                timing.min_pulse_low_slack,
                timing.min_pulse_width_slack
            ))
        } else {
            Ok(())
        }
    });

    c.add("api_timing_min_pulse_width_violations", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 -fall 0.1 [get_ports clk]
            set_input_delay 1.0 -clock clk [get_ports d]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let negative_high = timing
            .min_pulse_width_checks
            .iter()
            .filter(|check| {
                check.polarity == eda_api::MinPulsePolarity::High && check.slack_ns < 0.0
            })
            .collect::<Vec<_>>();
        let endpoint = negative_high
            .first()
            .map(|check| check.cell.as_str())
            .ok_or("no negative high-pulse check")?;
        let mut store = eda_api::timing_violation_store("sdc", &flow.netlist, &timing)?;
        let waiver = eda_api::timing_violation_waiver("pulse", "sdc", endpoint, -1.0)?
            .with_kind(eda_api::TimingViolationKind::MinPulseHigh);
        eda_api::add_timing_violation_waiver(&mut store, waiver.clone())?;
        let serialized = eda_api::write_timing_violation_waivers(&[waiver])?;
        let parsed = eda_api::parse_timing_violation_waivers(&serialized)?;
        if negative_high.len() == 2
            && (timing.min_pulse_width_slack + 0.15).abs() <= 1e-9
            && store.waived_len() == 1
            && parsed.first().and_then(|waiver| waiver.kind)
                == Some(eda_api::TimingViolationKind::MinPulseHigh)
            && eda_api::timing_violation_report(&store, None).contains("min_pulse_high")
        {
            Ok(())
        } else {
            Err(eda_api::timing_violation_report(&store, None))
        }
    });

    c.add("api_timing_explicit_delay_limit_violations", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_max_delay 0.5 -from [get_ports a] -to [get_ports y]
            set_min_delay 1.5 -from [get_ports a] -to [get_ports y]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let mut store = eda_api::timing_violation_store("sdc", &flow.netlist, &timing)?;
        let max_waiver = eda_api::timing_violation_waiver("max-only", "sdc", "y", -1.0)?
            .with_kind(eda_api::TimingViolationKind::MaxDelay);
        eda_api::add_timing_violation_waiver(&mut store, max_waiver.clone())?;
        let serialized = eda_api::write_timing_violation_waivers(&[max_waiver])?;
        let parsed = eda_api::parse_timing_violation_waivers(&serialized)?;
        if (timing.max_delay_slack + 0.5).abs() > 1e-9
            || (timing.min_delay_slack + 0.5).abs() > 1e-9
            || store.violations().len() != 2
            || store.waived_len() != 1
            || store.active_violations().len() != 1
            || parsed.first().and_then(|waiver| waiver.kind)
                != Some(eda_api::TimingViolationKind::MaxDelay)
        {
            Err(format!(
                "max={} min={}; {}",
                timing.max_delay_slack,
                timing.min_delay_slack,
                eda_api::timing_violation_report(&store, None)
            ))
        } else {
            Ok(())
        }
    });

    c.add("api_sdc_recovery_removal", || {
        let src = "module m(input clk, d, rst, output reg q);
            always @(posedge clk or posedge rst)
                if (rst) q <= 1'b0; else q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 1.2 -clock clk [get_ports rst]
            set_clock_uncertainty 0.10 -setup [get_clocks clk]
            set_clock_uncertainty 0.05 -hold [get_clocks clk]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let endpoints: &[eda_api::ResetEndpoint] = flow.timing.reset_endpoint_list();
        let reset_paths = eda_api::reset_timing_paths(&flow.netlist, &flow.timing, 0);
        let constrained_reset = eda_api::constrained_reset_timing_paths(&flow.netlist, &timing, 0);
        let reset_report =
            eda_api::constrained_reset_timing_path_report(&flow.netlist, &constrained_reset);
        if endpoints.len() != 1
            || (timing.recovery_slack - 2.4).abs() > 1e-9
            || (timing.removal_slack - 1.0).abs() > 1e-9
            || reset_paths.len() != 1
            || constrained_reset.len() != 2
            || constrained_reset
                .iter()
                .any(|path| !path.required_ns.is_finite())
            || !reset_report.contains("Constrained reset timing path report")
        {
            Err(format!(
                "endpoints={} recovery={} removal={}\n{}",
                endpoints.len(),
                timing.recovery_slack,
                timing.removal_slack,
                reset_report
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_sdc_clock_latency", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_clock_latency -source 0.7 [get_clocks clk]
            set_clock_latency 0.3 [get_clocks clk]
            set_input_delay 1.0 -clock clk [get_ports d]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        if (timing.reg2reg_slack - 2.2).abs() > 1e-9
            || (timing.in2reg_slack - 2.7).abs() > 1e-9
            || (timing.output_slack - 1.8).abs() > 1e-9
        {
            Err(format!(
                "reg2reg={} in2reg={} output={}",
                timing.reg2reg_slack, timing.in2reg_slack, timing.output_slack
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_sdc_timing_derate", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_timing_derate -early 0.8 -late 1.2
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        if (timing.output_slack - 2.16).abs() > 1e-9
            || (timing.early_derate - 0.8).abs() > 1e-9
            || (timing.late_derate - 1.2).abs() > 1e-9
        {
            Err(format!(
                "output={} early={} late={}",
                timing.output_slack, timing.early_derate, timing.late_derate
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_sdc_cell_timing_derate", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let cell = flow
            .netlist
            .cells()
            .first()
            .ok_or_else(|| "netlist has no cells".to_string())?
            .name
            .clone();
        let sdc = format!(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_timing_derate -early 0.9 -late 1.1
            set_timing_derate -cell_delay -late 2.0 [get_cells {}]
        "#,
            cell
        );
        let constraints = eda_api::parse_timing_constraints(&sdc)?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        if (timing.output_slack - 1.38).abs() <= 1e-9
            && timing.cell_derates.len() == 1
            && (timing.cell_derates[&cell].late - 2.0).abs() <= 1e-9
        {
            Ok(())
        } else {
            Err(format!(
                "slack={} derates={:?}",
                timing.output_slack, timing.cell_derates
            ))
        }
    });
    c.add("api_sdc_port_timing_derate", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_timing_derate -early 0.5 -late 2.0 [get_ports a]
            set_timing_derate -late 2.0 [get_ports y]
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        if (timing.output_slack - 1.8).abs() <= 1e-9
            && timing.port_derates.len() == 2
            && (timing.port_derates["a"].late - 2.0).abs() <= 1e-9
        {
            Ok(())
        } else {
            Err(format!(
                "slack={} derates={:?}",
                timing.output_slack, timing.port_derates
            ))
        }
    });
    c.add("api_sdc_pin_timing_derate", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let cell = flow
            .netlist
            .cells()
            .first()
            .ok_or_else(|| "netlist has no cells".to_string())?
            .name
            .clone();
        let sdc = format!(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_timing_derate -cell_delay -late 1.2 [get_cells {}]
            set_timing_derate -late 2.0 [get_pins {}/a]
        "#,
            cell, cell
        );
        let constraints = eda_api::parse_timing_constraints(&sdc)?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let pin = eda_api::TimingPin::new(cell, "a");
        if (timing.output_slack - 1.4).abs() <= 1e-9
            && timing.pin_derates.len() == 1
            && (timing.pin_derates[&pin].late - 2.0).abs() <= 1e-9
        {
            Ok(())
        } else {
            Err(format!(
                "slack={} derates={:?}",
                timing.output_slack, timing.pin_derates
            ))
        }
    });
    c.add("api_sdc_clk_to_q_pin_derate", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let launch = flow
            .netlist
            .cells()
            .first()
            .ok_or_else(|| "netlist has no sequential cells".to_string())?
            .name
            .clone();
        let constraints = eda_api::parse_timing_constraints(&format!(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_timing_derate -cell_delay -early 0.5 -late 2.0 [get_pins {}/q]
        "#,
            launch
        ))?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let pin = eda_api::TimingPin::new(launch, "q");
        if (timing.reg2reg_slack - 1.0).abs() <= 1e-9
            && (timing.reg2reg_hold_slack - 0.4).abs() <= 1e-9
            && timing.pin_derates.len() == 1
            && (timing.pin_derates[&pin].late - 2.0).abs() <= 1e-9
        {
            Ok(())
        } else {
            Err(format!(
                "setup={} hold={}",
                timing.reg2reg_slack, timing.reg2reg_hold_slack
            ))
        }
    });
    c.add("api_sdc_cell_check_derate", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_timing_derate -cell_check -early 0.5 -late 2.0
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        if (timing.reg2reg_slack - 1.6).abs() <= 1e-9
            && (timing.min_pulse_width_slack - 1.5).abs() <= 1e-9
            && (timing.early_check_derate - 0.5).abs() <= 1e-9
            && (timing.late_check_derate - 2.0).abs() <= 1e-9
        {
            Ok(())
        } else {
            Err(format!(
                "reg2reg={} pulse={} early={} late={}",
                timing.reg2reg_slack,
                timing.min_pulse_width_slack,
                timing.early_check_derate,
                timing.late_check_derate
            ))
        }
    });
    c.add("api_sdc_pin_check_derate", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let capture = flow
            .netlist
            .cells()
            .last()
            .ok_or_else(|| "netlist has no cells".to_string())?
            .name
            .clone();
        let constraints = eda_api::parse_timing_constraints(&format!(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_timing_derate -cell_check -early 0.5 -late 2.0 [get_pins {}/d]
        "#,
            capture
        ))?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let pin = eda_api::TimingPin::new(capture, "d");
        if (timing.reg2reg_slack - 1.6).abs() <= 1e-9
            && (timing.reg2reg_hold_slack - 1.1).abs() <= 1e-9
            && timing.pin_check_derates.len() == 1
            && (timing.pin_check_derates[&pin].early - 0.5).abs() <= 1e-9
        {
            Ok(())
        } else {
            Err(format!(
                "setup={} hold={}",
                timing.reg2reg_slack, timing.reg2reg_hold_slack
            ))
        }
    });
    c.add("api_repair_hold_violations_with_placement", || {
        let src = "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 8.0 [get_ports clk]
            set_input_delay 0.0 -clock clk [get_ports d]
            set_clock_uncertainty 1.2 -hold [get_clocks clk]
        "#,
        )?;
        let before = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let result = eda_api::repair_hold_violations_with_placement(
            &flow.netlist,
            &flow.placement,
            &constraints,
            &eda_api::HoldRepairOptions::default(),
        )?;
        let after = eda_api::constrained_timing(&result.netlist, &constraints)?;
        let report = eda_api::hold_repair_report(&result);
        let placement = result
            .physical_placement
            .as_ref()
            .ok_or("hold repair omitted physical placement")?;
        let routing = eda_api::route_with_options(
            &result.netlist,
            placement,
            &eda_api::RouteOptions::default(),
        )?;
        let drc = eda_api::physical_verification(&result.netlist, placement, &routing)?;
        if before.hold_slack >= 0.0
            || after.hold_slack < 0.0
            || !result.all_active_violations_repaired
            || result.buffers_inserted != 2
            || result.repairs.len() != 1
            || result.repairs[0].buffers_inserted != 2
            || result.repairs[0].old_slack_ns >= 0.0
            || result.repairs[0].new_slack_ns < 0.0
            || placement.cell_pos.len() != flow.netlist.cell_count() + 2
            || drc.has_errors()
            || !report.contains("Hold repair: 2 buffer(s), 1 endpoint(s), status PASS")
        {
            Err(format!(
                "before={} after={} buffers={} repairs={:?}",
                before.hold_slack, after.hold_slack, result.buffers_inserted, result.repairs
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_hold_repair_respects_waivers_and_buffer_limit", || {
        let src = "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 8.0 [get_ports clk]
            set_input_delay 0.0 -clock clk [get_ports d]
            set_clock_uncertainty 1.2 -hold [get_clocks clk]
        "#,
        )?;
        let waiver = eda_api::timing_violation_waiver("hold-accepted", "clk", "*", -10.0)?
            .with_kind(eda_api::TimingViolationKind::Hold);
        let waived = eda_api::repair_hold_violations(
            &flow.netlist,
            &constraints,
            &eda_api::HoldRepairOptions {
                waivers: vec![waiver],
                ..Default::default()
            },
        )?;
        let limited = eda_api::repair_hold_violations(
            &flow.netlist,
            &constraints,
            &eda_api::HoldRepairOptions {
                max_buffers_per_endpoint: 1,
                ..Default::default()
            },
        )?;
        let limited_timing = eda_api::constrained_timing(&limited.netlist, &constraints)?;
        if waived.buffers_inserted != 0
            || !waived.repairs.is_empty()
            || !waived.all_active_violations_repaired
            || waived.netlist.cell_count() != flow.netlist.cell_count()
            || limited.buffers_inserted != 1
            || limited.all_active_violations_repaired
            || limited_timing.hold_slack >= 0.0
        {
            Err(format!(
                "waived buffers={} limited buffers={} limited hold={}",
                waived.buffers_inserted, limited.buffers_inserted, limited_timing.hold_slack
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_multicorner_common_hold_repair", || {
        let src = "module m(input clk, d, output reg q, output y);
            always @(posedge clk) q <= d;
            assign y = ~d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let slow_view = scaled_timing_view(&flow.netlist, 2.0)?;
        let fast_view = scaled_timing_view(&flow.netlist, 0.5)?;
        let slow_constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 12.0 [get_ports clk]
            set_input_delay 0.0 -clock clk [get_ports d]
            set_clock_uncertainty 2.0 -hold [get_clocks clk]
        "#,
        )?;
        let fast_constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 10.0 [get_ports clk]
            set_input_delay 0.0 -clock clk [get_ports d]
            set_clock_uncertainty 1.2 -hold [get_clocks clk]
        "#,
        )?;
        let scenarios = vec![
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "slow",
                &slow_view,
                &slow_constraints,
                Vec::new(),
            ),
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "fast",
                &fast_view,
                &fast_constraints,
                Vec::new(),
            ),
        ];
        let placement = eda_pnr::Placement {
            rows: 4,
            cell_pos: vec![
                eda_pnr::Pos { row: 0, site: 0 },
                eda_pnr::Pos { row: 3, site: 26 },
            ],
            cell_width: flow.placement.cell_width.clone(),
            blockages: Vec::new(),
        };
        let result = eda_api::repair_multicorner_hold_violations(
            &scenarios,
            &placement,
            &eda_api::MultiCornerHoldRepairOptions::default(),
        )?;
        let report = eda_api::multi_corner_hold_repair_report(&result);
        let slow = result
            .scenarios
            .iter()
            .find(|scenario| scenario.scenario == "slow")
            .ok_or("missing slow repaired view")?;
        let fast = result
            .scenarios
            .iter()
            .find(|scenario| scenario.scenario == "fast")
            .ok_or("missing fast repaired view")?;
        if result.buffers_inserted == 0
            || result.repairs.len() != 1
            || !result.all_active_violations_repaired
            || !slow.all_active_violations_repaired
            || !fast.all_active_violations_repaired
            || slow.netlist.cell_count() != fast.netlist.cell_count()
            || result.physical_placement.cell_pos.len() != slow.netlist.cell_count()
            || eda_pnr::check_routing(&result.routing).is_err()
            || !report.contains("Multi-corner hold repair:")
            || !report.contains("status PASS")
            || !report.contains("scenario 'slow'")
            || !report.contains("scenario 'fast'")
        {
            Err(report)
        } else {
            Ok(())
        }
    });
    c.add("api_multicorner_physical_timing_flow_hold_repair", || {
        let src = "module m(input clk, d, output reg q, output y);
            always @(posedge clk) q <= d;
            assign y = ~d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let slow_view = scaled_timing_view(&flow.netlist, 2.0)?;
        let fast_view = scaled_timing_view(&flow.netlist, 0.5)?;
        let slow_constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 12.0 [get_ports clk]
            set_input_delay 0.0 -clock clk [get_ports d]
            set_clock_uncertainty 2.0 -hold [get_clocks clk]
        "#,
        )?;
        let fast_constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 10.0 [get_ports clk]
            set_input_delay 0.0 -clock clk [get_ports d]
            set_clock_uncertainty 1.2 -hold [get_clocks clk]
        "#,
        )?;
        let scenarios = vec![
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "slow",
                &slow_view,
                &slow_constraints,
                Vec::new(),
            ),
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "fast",
                &fast_view,
                &fast_constraints,
                Vec::new(),
            ),
        ];
        let placement = eda_pnr::Placement {
            rows: 4,
            cell_pos: vec![
                eda_pnr::Pos { row: 0, site: 0 },
                eda_pnr::Pos { row: 3, site: 26 },
            ],
            cell_width: flow.placement.cell_width.clone(),
            blockages: Vec::new(),
        };
        let result = eda_api::run_multi_corner_physical_timing_flow(
            &eda_api::MultiCornerPhysicalTimingFlowOptions {
                scenarios,
                initial_placement: Some(placement),
                hold_repair: Some(eda_api::MultiCornerHoldRepairOptions::default()),
                iterations: 0,
                ..Default::default()
            },
        )?;
        let report = eda_api::multi_corner_physical_timing_flow_report(&result);
        let hold = result
            .hold_repair
            .as_ref()
            .ok_or("MMMC physical flow omitted hold repair")?;
        let slow = &result.baseline.corners[0];
        let fast = &result.baseline.corners[1];
        if hold.buffers_inserted == 0
            || !hold.all_active_violations_repaired
            || slow.evaluation.annotated_netlist.cell_count()
                != fast.evaluation.annotated_netlist.cell_count()
            || slow.evaluation.annotated_netlist.cell_count() <= flow.netlist.cell_count()
            || result.baseline.placement.cell_pos.len()
                != slow.evaluation.annotated_netlist.cell_count()
            || slow.evaluation.drc.has_errors()
            || fast.evaluation.drc.has_errors()
            || eda_pnr::check_routing(&result.baseline.routing).is_err()
            || !report.contains("Multi-corner physical timing flow")
            || !report.contains("Multi-corner hold repair:")
            || !report.contains("status PASS")
        {
            Err(report)
        } else {
            Ok(())
        }
    });
    c.add(
        "api_multicorner_physical_timing_flow_repairs_hold_each_iteration",
        || {
            let src = "module m(input clk, d, output reg q, output y);
            always @(posedge clk) q <= d;
            assign y = ~d;
         endmodule";
            let flow = eda_api::run_flow(src, None)?;
            let constraints = eda_api::parse_timing_constraints(
                r#"
            create_clock -name clk -period 8.0 [get_ports clk]
            set_input_delay 0.0 -clock clk [get_ports d]
            set_clock_uncertainty 1.2 -hold [get_clocks clk]
        "#,
            )?;
            let scenarios = vec![
                eda_api::MultiCornerPhysicalTimingScenario::new(
                    "slow",
                    &flow.netlist,
                    &constraints,
                    Vec::new(),
                ),
                eda_api::MultiCornerPhysicalTimingScenario::new(
                    "fast",
                    &flow.netlist,
                    &constraints,
                    Vec::new(),
                ),
            ];
            let result = eda_api::run_multi_corner_physical_timing_flow(
                &eda_api::MultiCornerPhysicalTimingFlowOptions {
                    scenarios,
                    hold_repair: Some(eda_api::MultiCornerHoldRepairOptions {
                        hold: eda_api::HoldRepairOptions {
                            max_buffers_per_endpoint: 1,
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                    iterations: 1,
                    ..Default::default()
                },
            )?;
            let report = eda_api::multi_corner_physical_timing_flow_report(&result);
            let baseline_buffers = result
                .baseline
                .hold_repairs
                .iter()
                .map(|repair| repair.buffers_inserted)
                .sum::<usize>();
            let optimized_buffers = result.optimized[0]
                .hold_repairs
                .iter()
                .map(|repair| repair.buffers_inserted)
                .sum::<usize>();
            let slow = &result.optimized[0].corners[0];
            let fast = &result.optimized[0].corners[1];
            if baseline_buffers != 1
                || result.baseline.all_active_hold_repaired
                || optimized_buffers != 1
                || !result.optimized[0].all_active_hold_repaired
                || slow.evaluation.annotated_netlist.cell_count() != flow.netlist.cell_count() + 2
                || fast.evaluation.annotated_netlist.cell_count()
                    != slow.evaluation.annotated_netlist.cell_count()
                || !report.contains("hold status LIMITED")
                || !report.contains("hold status PASS")
            {
                Err(report)
            } else {
                Ok(())
            }
        },
    );
    c.add("api_multicorner_timing_scenarios", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let slow = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
        "#,
        )?;
        let fast = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
        "#,
        )?;
        let result = eda_api::analyze_timing_scenarios([
            eda_api::TimingScenario::new("slow", &flow.netlist, &slow),
            eda_api::TimingScenario::new("fast", &flow.netlist, &fast),
        ])?;
        let report = eda_api::multi_corner_timing_report(&result);
        if result.corners.len() == 2
            && result.worst_scenario() == Some("fast")
            && report.contains("Multi-corner STA report: 2 scenario(s)")
            && report.contains("worst scenario: fast")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_multicorner_timing_path_merge", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let slow = eda_api::parse_timing_constraints(
            "create_clock -name clk -period 4.0 [get_ports clk]",
        )?;
        let fast = eda_api::parse_timing_constraints(
            "create_clock -name clk -period 2.0 [get_ports clk]",
        )?;
        let views = [
            eda_api::TimingScenario::new("slow", &flow.netlist, &slow),
            eda_api::TimingScenario::new("fast", &flow.netlist, &fast),
        ];
        let result = eda_api::analyze_timing_scenarios(views)?;
        let paths = eda_api::multi_corner_setup_timing_paths(&result, views, 1)?;
        let report = eda_api::multi_corner_setup_timing_path_report(&paths);
        if paths.len() == 1
            && paths[0].scenario == "fast"
            && paths[0].corner_slacks.len() == 2
            && !paths[0].path.path.arcs.is_empty()
            && report.contains("Multi-corner setup timing path report: 1 endpoint(s)")
            && report.contains("output y: worst scenario 'fast'")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add(
        "api_multicorner_timing_path_merge_rejects_incomplete_views",
        || {
            let src = "module m(input a, b, output y); assign y = a & b; endmodule";
            let flow = eda_api::run_flow(src, None)?;
            let slow = eda_api::parse_timing_constraints(
                "create_clock -name clk -period 4.0 [get_ports clk]",
            )?;
            let fast = eda_api::parse_timing_constraints(
                "create_clock -name clk -period 2.0 [get_ports clk]",
            )?;
            let result = eda_api::analyze_timing_scenarios([
                eda_api::TimingScenario::new("slow", &flow.netlist, &slow),
                eda_api::TimingScenario::new("fast", &flow.netlist, &fast),
            ])?;
            match eda_api::multi_corner_setup_timing_paths(
                &result,
                [eda_api::TimingScenario::new("slow", &flow.netlist, &slow)],
                0,
            ) {
                Err(error) if error.contains("missing scenario 'fast'") => Ok(()),
                other => Err(format!("unexpected result: {:?}", other.err())),
            }
        },
    );
    c.add("api_multicorner_timing_manifest", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let scenarios = eda_api::parse_timing_scenarios(
            r#"
            scenario slow
                create_clock -name clk -period 4.0 [get_ports clk]
            endscenario
            scenario fast
                create_clock -name clk -period 2.0 [get_ports clk]
            endscenario
        "#,
        )?;
        let result = eda_api::analyze_timing_scenarios_from_definitions(&flow.netlist, &scenarios)?;
        if scenarios.len() == 2
            && result.worst_scenario() == Some("fast")
            && eda_api::multi_corner_timing_report(&result).contains("[fast]")
        {
            Ok(())
        } else {
            Err(eda_api::multi_corner_timing_report(&result))
        }
    });
    c.add("api_multicorner_incremental_timing", || {
        let src = "module m(input a, output y); assign y = ~a; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let old = eda_api::parse_timing_scenarios(
            r#"
            scenario slow
                create_clock -name clk -period 4.0 [get_ports a]
                set_input_delay 0.2 -clock clk [get_ports a]
            endscenario
            scenario fast
                create_clock -name clk -period 2.0 [get_ports a]
                set_input_delay 0.2 -clock clk [get_ports a]
            endscenario
        "#,
        )?;
        let new = eda_api::parse_timing_scenarios(
            r#"
            scenario slow
                create_clock -name clk -period 4.0 [get_ports a]
                set_input_delay 0.8 -clock clk [get_ports a]
            endscenario
            scenario fast
                create_clock -name clk -period 2.0 [get_ports a]
                set_input_delay 0.2 -clock clk [get_ports a]
            endscenario
        "#,
        )?;
        let previous = eda_api::analyze_timing_scenarios_from_definitions(&flow.netlist, &old)?;
        let diff = eda_api::diff_timing_scenarios(&old, &new)?;
        let updated =
            eda_api::update_timing_scenarios_incrementally(&flow.netlist, &previous, &old, &new)?;
        if diff.changed.contains("slow")
            && updated.reused.contains("fast")
            && updated.updates.len() == 1
            && updated.result.corners.len() == 2
            && updated.result.worst_scenario() == Some("fast")
            && eda_api::multi_corner_timing_report(&updated.result).contains("[fast]")
        {
            Ok(())
        } else {
            Err(eda_api::multi_corner_timing_report(&updated.result))
        }
    });
    c.add("api_timing_scenario_property_diff_reuses_sta", || {
        let src = "module m(input a, output y); assign y = ~a; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let old = eda_api::parse_timing_scenarios(
            r#"
            scenario slow
                create_clock -name clk -period 4.0 [get_ports a]
                set_scenario_property post_route.voltage_v 0.90
            endscenario
        "#,
        )?;
        let new = eda_api::parse_timing_scenarios(
            r#"
            scenario slow
                create_clock -name clk -period 4.0 [get_ports a]
                set_scenario_property post_route.voltage_v 0.80
                set_scenario_property post_route.frequency_mhz 200.0
            endscenario
        "#,
        )?;
        let previous = eda_api::analyze_timing_scenarios_from_definitions(&flow.netlist, &old)?;
        let diff = eda_api::diff_timing_scenarios(&old, &new)?;
        let updated =
            eda_api::update_timing_scenarios_incrementally(&flow.netlist, &previous, &old, &new)?;
        let report = eda_api::timing_scenario_diff_report(&diff);
        if diff.changed.contains("slow")
            && !diff.timing_changed.contains("slow")
            && diff.properties.contains_key("slow")
            && updated.reused.contains("slow")
            && updated.updates.is_empty()
            && report.contains("timing changed []")
            && report.contains("property 'post_route.voltage_v': '0.90' -> '0.80'")
            && report.contains("added properties [post_route.frequency_mhz]")
        {
            Ok(())
        } else {
            Err(format!(
                "{report}\nreused={:?} updates={}",
                updated.reused,
                updated.updates.len()
            ))
        }
    });
    c.add("api_multicorner_topology_incremental_timing", || {
        let src = "module m(input a, output y); assign y = ~a; endmodule";
        let old_flow = eda_api::run_flow(src, None)?;
        let sdf = eda_api::parse_delay_file(
            r#"
            (DELAYFILE
              (SDFVERSION "3.0")
              (DESIGN "m")
              (TIMESCALE 1ns)
              (CELL (CELLTYPE "INV") (INSTANCE u1_inv)
                (DELAY (ABSOLUTE (IOPATH A Y (1.0))))
              )
            )
        "#,
        )?;
        let annotated = eda_api::back_annotate(&old_flow.netlist, &sdf)?;
        let slow =
            eda_api::parse_timing_constraints("create_clock -name clk -period 4.0 [get_ports a]")?;
        let fast =
            eda_api::parse_timing_constraints("create_clock -name clk -period 2.0 [get_ports a]")?;
        let old_views = [
            eda_api::TimingScenario::new("slow", &old_flow.netlist, &slow),
            eda_api::TimingScenario::new("fast", &old_flow.netlist, &fast),
        ];
        let new_views = [
            eda_api::TimingScenario::new("slow", &old_flow.netlist, &slow),
            eda_api::TimingScenario::new("fast", &annotated, &fast),
        ];
        let previous = eda_api::analyze_timing_scenarios(old_views)?;
        let diff = eda_api::diff_timing_scenario_sets(&old_views, &new_views)?;
        let updated = eda_api::update_timing_scenarios_with_topology_incrementally(
            &previous, &old_views, &new_views,
        )?;
        let full = eda_api::analyze_timing_scenarios(new_views)?;
        if diff.changed_netlists.contains("fast")
            && updated.reused.contains("slow")
            && updated.updates["fast"].kind
                == eda_api::MultiCornerScenarioUpdateKind::TopologyIncremental
            && (updated.result.corners[1].timing.worst_slack - full.corners[1].timing.worst_slack)
                .abs()
                < 1e-12
            && eda_api::multi_corner_timing_report(&updated.result).contains("[fast]")
        {
            Ok(())
        } else {
            Err(eda_api::multi_corner_timing_report(&updated.result))
        }
    });
}
