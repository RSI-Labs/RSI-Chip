//! SDC/MMMC child regression domain.

use super::super::*;
use super::sdc_netlist;

pub(super) fn register_checks(c: &mut CaseList) {
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
