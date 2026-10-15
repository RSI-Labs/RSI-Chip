//! Signal quality and SDC quality rule cases.

use super::*;

pub(super) fn register(c: &mut CaseList) {
    signal_quality(c);
    sdc_design_quality_rules(c);
    sdc_scoped_quality_rules(c);
    sdc_transition_load_rules(c);
    sdc_clock_transition_rules(c);
    sdc_clock_transition_min_max_checks(c);
}

pub(super) fn signal_quality(c: &mut CaseList) {
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
