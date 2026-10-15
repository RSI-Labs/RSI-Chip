//! SDC/MMMC child regression domain.

use super::super::*;
use super::sdc_netlist;

pub(super) fn register_constraints(c: &mut CaseList) {
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
}
