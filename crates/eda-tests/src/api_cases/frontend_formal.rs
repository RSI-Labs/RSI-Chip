//! Compile, hierarchy, synthesis, equivalence, formal safety, and temporal assertions.

use super::*;

pub(super) fn register(c: &mut CaseList) {
    c.add("api_compile_and_sim", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let d = eda_api::compile(src, None)?;
        let y = eda_api::simulate_comb(&d, &[("a", 1), ("b", 1)], "y")?;
        if y != 1 {
            Err(format!("y = {}", y))
        } else {
            Ok(())
        }
    });
    c.add("api_compile_top_select", || {
        let src = "module a(input x, output y); assign y = x; endmodule
                   module b(input x, output y); assign y = ~x; endmodule";
        let d = eda_api::compile(src, Some("b"))?;
        let y = eda_api::simulate_comb(&d, &[("x", 1)], "y")?;
        if y != 0 {
            Err(format!("y = {}", y))
        } else {
            Ok(())
        }
    });
    c.add("api_hierarchy_and_upf_instance_scope", || {
        let src = "module leaf(input a, output y); assign y = ~a; endmodule
            module sub(input a, output y); leaf u_leaf(.a(a), .y(y)); endmodule
            module top(input a, output y); sub u1(.a(a), .y(y)); endmodule";
        let design = eda_api::compile(src, Some("top"))?;
        let hierarchy = eda_api::hierarchy_instances(&design);
        if hierarchy.len() != 3
            || hierarchy.iter().any(|instance| instance.path.contains('/'))
            || hierarchy[1].path != "top.u1"
            || hierarchy[2].path != "top.u1.u_leaf"
            || hierarchy[2].module != "leaf"
            || hierarchy[2].parent.as_deref() != Some("top.u1")
            || !eda_api::hierarchy_report(&design).contains("top.u1.u_leaf")
            || eda_api::owning_hierarchy_instance(&design, hierarchy[2].ports[0].base)
                .map(|instance| instance.path.as_str())
                != Some("top.u1.u_leaf")
        {
            return Err(eda_api::hierarchy_report(&design));
        }

        let intent = eda_api::parse_upf(
            r#"
upf_version 2.1
set_design_top top
create_power_domain SUB -elements {top.u1}
create_supply_net vdd -domain SUB
create_supply_net vss -domain SUB
set_domain_supply_net SUB -primary_power_net vdd -primary_ground_net vss
"#,
        )?;
        let result = eda_api::power_intent_check(&design, &intent);
        if result.domain_count != 1 || result.has_errors() {
            Err(eda_api::upf_report(&result))
        } else {
            let flow = eda_api::run_flow(src, Some("top"))?;
            let netlist_hierarchy = eda_api::netlist_hierarchy_instances(&flow.netlist);
            let leaf_input = netlist_hierarchy[2]
                .ports
                .iter()
                .find(|port| port.name == "a")
                .and_then(|port| port.nets.first().copied())
                .ok_or("synthesis dropped leaf hierarchy input")?;
            if netlist_hierarchy.len() != 3
                || netlist_hierarchy[2].path != "top.u1.u_leaf"
                || eda_api::owning_netlist_hierarchy_instance(&flow.netlist, leaf_input)
                    .map(|instance| instance.path.as_str())
                    != Some("top.u1.u_leaf")
                || !eda_api::netlist_hierarchy_report(&flow.netlist).contains("top.u1.u_leaf")
            {
                Err(eda_api::netlist_hierarchy_report(&flow.netlist))
            } else {
                Ok(())
            }
        }
    });
    c.add("api_hierarchical_timing_path", || {
        let src = "module leaf(input a, b, output n1, n2);
            assign n1 = a & b;
            assign n2 = ~(a | b);
        endmodule
        module sub(input a, b, output y);
            wire n1, n2;
            leaf u_leaf(.a(a), .b(b), .n1(n1), .n2(n2));
            assign y = n1 & n2;
        endmodule
        module top(input a, b, output y);
            sub u1(.a(a), .b(b), .y(y));
        endmodule";
        let flow = eda_api::run_flow(src, Some("top"))?;
        let paths = eda_api::timing_paths(&flow.netlist, &flow.timing, 10);
        let hierarchical = eda_api::hierarchical_timing_paths(&flow.netlist, &paths)?;
        let report = eda_api::hierarchical_timing_path_report(&hierarchical);
        let Some(first) = hierarchical.first() else {
            return Err("hierarchical design produced no timing path".into());
        };
        let total_segment_delay = first
            .segments
            .iter()
            .map(|segment| segment.delay_ns)
            .sum::<f64>();
        let total_arc_delay = first.path.arcs.iter().map(|arc| arc.delay_ns).sum::<f64>();
        if paths.is_empty()
            || first.segments.is_empty()
            || !first
                .segments
                .iter()
                .any(|segment| segment.instance.as_deref() == Some("top.u1.u_leaf"))
            || !first
                .segments
                .iter()
                .any(|segment| segment.instance.as_deref() == Some("top.u1"))
            || (total_segment_delay - total_arc_delay).abs() > 1e-12
            || !report.contains("Hierarchical timing path report:")
            || !report.contains("instance 'top.u1.u_leaf'")
        {
            Err(report)
        } else {
            Ok(())
        }
    });
    c.add("api_instance_scoped_false_path", || {
        let src = "module inv(input a, output y); assign y = ~a; endmodule
            module sub(input a, output y); inv u_leaf(.a(a), .y(y)); endmodule
            module top(input a, output y1, y2);
                sub u1(.a(a), .y(y1));
                sub u2(.a(a), .y(y2));
            endmodule";
        let flow = eda_api::run_flow(src, Some("top"))?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 2.0 [get_ports a]
            set_input_delay 0.3 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y1]
            set_output_delay 0.4 -clock clk [get_ports y2]
            set_false_path -from [get_ports a] -to [get_ports y*] -instance top.u1.u_leaf
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let first = timing
            .net_slacks
            .get("y1")
            .and_then(|slack| slack.output_ns)
            .ok_or("instance-scoped false path omitted y1 slack")?;
        let second = timing
            .net_slacks
            .get("y2")
            .and_then(|slack| slack.output_ns)
            .ok_or("instance-scoped false path omitted y2 slack")?;
        let paths = eda_api::constrained_timing_paths(&flow.netlist, &timing, 10);
        let report = eda_api::constrained_timing_path_report(&flow.netlist, &paths);
        let exception = timing
            .path_exceptions
            .get("Port:y1")
            .and_then(|exceptions| exceptions.first())
            .ok_or("instance-scoped false path was not recorded for y1")?;
        if first.is_finite()
            || !second.is_finite()
            || timing.exceptions_applied != 1
            || exception.instance_pattern.as_deref() != Some("top.u1.u_leaf")
        {
            Err(format!("{timing:?}\n{report}"))
        } else {
            Ok(())
        }
    });
    c.add("api_instance_scoped_delay_limit", || {
        let src = "module inv(input a, output y); assign y = ~a; endmodule
            module sub(input a, output y); inv u_leaf(.a(a), .y(y)); endmodule
            module top(input a, output y1, y2);
                sub u1(.a(a), .y(y1));
                sub u2(.a(a), .y(y2));
            endmodule";
        let flow = eda_api::run_flow(src, Some("top"))?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 2.0 [get_ports a]
            set_input_delay 0.3 -clock clk [get_ports a]
            set_max_delay 0.1 -from [get_ports a] -to [get_ports y*] -instance top.u2.u_leaf
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let first = timing.path_max_delay_limits.get("Port:y1");
        let second = timing
            .path_max_delay_limits
            .get("Port:y2")
            .and_then(|limits| limits.first())
            .ok_or("instance-scoped max-delay limit omitted y2")?;
        let paths = eda_api::constrained_timing_paths(&flow.netlist, &timing, 10);
        let report = eda_api::constrained_timing_path_report(&flow.netlist, &paths);
        if first.is_some_and(|limits| !limits.is_empty())
            || second.slack_ns >= 0.0
            || !second.slack_ns.is_finite()
            || second.limit.instance_pattern.as_deref() != Some("top.u2.u_leaf")
            || !report.contains("instance top.u2.u_leaf")
        {
            Err(format!("{timing:?}\n{report}"))
        } else {
            Ok(())
        }
    });
    c.add("api_instance_scoped_disabled_timing_arc", || {
        let src = "module inv(input a, output y); assign y = ~a; endmodule
            module sub(input a, output y); inv u_leaf(.a(a), .y(y)); endmodule
            module top(input a, output y1, y2);
                sub u1(.a(a), .y(y1));
                sub u2(.a(a), .y(y2));
            endmodule";
        let flow = eda_api::run_flow(src, Some("top"))?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 2.0 [get_ports a]
            set_input_delay 0.3 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y1]
            set_output_delay 0.4 -clock clk [get_ports y2]
            set_disable_timing [get_cells *] -instance top.u2.u_leaf
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let paths = eda_api::constrained_timing_paths(&flow.netlist, &timing, 10);
        let endpoints = paths
            .iter()
            .map(|path| match &path.path.endpoint {
                eda_api::TimingPathEndpoint::OutputPort(name) => name.clone(),
                eda_api::TimingPathEndpoint::SequentialCell(name) => name.clone(),
            })
            .collect::<Vec<_>>();
        let first = timing
            .net_slacks
            .get("y1")
            .and_then(|slack| slack.output_ns)
            .ok_or("instance-scoped disable omitted y1 slack")?;
        let second = timing
            .net_slacks
            .get("y2")
            .and_then(|slack| slack.output_ns)
            .unwrap_or(f64::INFINITY);
        if timing.base.disabled_timing_arcs != 1
            || endpoints != vec!["y1".to_string()]
            || !first.is_finite()
            || second.is_finite()
            || constraints.disabled_timing_arcs[0]
                .instance_pattern
                .as_deref()
                != Some("top.u2.u_leaf")
        {
            Err(format!("{timing:?}\n{endpoints:?}"))
        } else {
            Ok(())
        }
    });
    c.add("api_instance_scoped_case_analysis", || {
        let src = "module inv(input a, output y); assign y = ~a; endmodule
            module sub(input a, output y); inv u_leaf(.a(a), .y(y)); endmodule
            module top(input a, output y1, y2);
                sub u1(.a(a), .y(y1));
                sub u2(.a(a), .y(y2));
            endmodule";
        let flow = eda_api::run_flow(src, Some("top"))?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 2.0 [get_ports a]
            set_input_delay 0.3 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y1]
            set_output_delay 0.4 -clock clk [get_ports y2]
            set_case_analysis 0 [get_nets y*] -instance top.u2.u_leaf
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let paths = eda_api::constrained_timing_paths(&flow.netlist, &timing, 10);
        let endpoints = paths
            .iter()
            .map(|path| match &path.path.endpoint {
                eda_api::TimingPathEndpoint::OutputPort(name) => name.clone(),
                eda_api::TimingPathEndpoint::SequentialCell(name) => name.clone(),
            })
            .collect::<Vec<_>>();
        let first = timing
            .net_slacks
            .get("y1")
            .and_then(|slack| slack.output_ns)
            .ok_or("instance-scoped case analysis omitted y1 slack")?;
        if timing.base.case_analysis_nets.contains_key("y1")
            || !timing.base.case_analysis_nets.contains_key("y2")
            || endpoints != vec!["y1".to_string()]
            || !first.is_finite()
            || constraints.case_analyses[0].instance_pattern.as_deref() != Some("top.u2.u_leaf")
        {
            Err(format!("{timing:?}\n{endpoints:?}"))
        } else {
            Ok(())
        }
    });
    c.add("api_instance_scoped_timing_derate", || {
        let src = "module inv(input a, output y); assign y = ~a; endmodule
            module sub(input a, output y); inv u_leaf(.a(a), .y(y)); endmodule
            module top(input a, output y1, y2);
                sub u1(.a(a), .y(y1));
                sub u2(.a(a), .y(y2));
            endmodule";
        let flow = eda_api::run_flow(src, Some("top"))?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 2.0 [get_ports a]
            set_timing_derate -cell_delay -late 2.0 [get_cells *] -instance top.u2.u_leaf
        "#,
        )?;
        let timing = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let y1 = flow.netlist.net_by_name("y1").unwrap();
        let y2 = flow.netlist.net_by_name("y2").unwrap();
        let first = timing
            .base
            .net_arrival(y1)
            .ok_or("instance-scoped derate omitted y1 arrival")?;
        let second = timing
            .base
            .net_arrival(y2)
            .ok_or("instance-scoped derate omitted y2 arrival")?;
        let rule = constraints
            .scoped_cell_timing_derates
            .first()
            .ok_or("instance-scoped derate rule missing")?;
        if first >= second
            || (second - first - 0.4).abs() > 1e-12
            || rule.cell_pattern != "*"
            || rule.instance_pattern.as_deref() != Some("top.u2.u_leaf")
            || rule.derate.late != 2.0
        {
            Err(format!(
                "y1={first}, y2={second}, rules={:?}",
                constraints.scoped_cell_timing_derates
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_synth_flow", || {
        let src = "module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let s = eda_api::gate_eval_comb(&flow, &[("a", 7), ("b", 6)], "s")?;
        if s != 13 {
            Err(format!("s = {}", s))
        } else {
            Ok(())
        }
    });
    c.add("api_equivalence_flow", || {
        let src = "module m(input a, b, output y); assign y = ~(a & b); endmodule";
        let flow = eda_api::run_flow(src, None)?;
        if !eda_api::flow_equivalent(&flow)? {
            Err("gate != rtl".into())
        } else {
            Ok(())
        }
    });
    c.add("api_prove_safety_flow", || {
        let src = "module m(input clk, d, rst, output reg q, output bad);
            always @(posedge clk or posedge rst)
                if (rst) q <= 1'b0; else q <= d;
            assign bad = q & rst;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let proof = eda_api::prove_safety(&flow.netlist, "bad", 8)?;
        if proof.status != eda_api::ProofStatus::Proved {
            Err(format!(
                "status={:?} report={}",
                proof.status,
                eda_api::proof_report(&proof)
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_prove_safety_expression", || {
        let src = "module m(input clk, d, rst, output reg q);
            always @(posedge clk or posedge rst)
                if (rst) q <= 1'b0; else q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let property = eda_api::BoolExpr::Not(Box::new(eda_api::BoolExpr::And(
            Box::new(eda_api::BoolExpr::Signal("q".into())),
            Box::new(eda_api::BoolExpr::Signal("rst".into())),
        )));
        let proof = eda_api::prove_safety_expression(&flow.netlist, &property, 8)?;
        if proof.status != eda_api::ProofStatus::Proved {
            Err(format!(
                "status={:?} report={}",
                proof.status,
                eda_api::proof_report(&proof)
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_bounded_temporal_assertions", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let assertions = eda_api::parse_assertions(
            "property two_cycle;
                assert property (@(posedge clk) d |-> ##[2:2] q2);
             endproperty",
        )?;
        let results = eda_api::check_assertions_bounded(&flow.netlist, &assertions, 3)?;
        if results.len() == 1
            && results[0].status == eda_api::PropertyBmcStatus::BoundedPass
            && eda_api::property_bmc_report(&results[0]).contains("BOUNDED PASS")
        {
            Ok(())
        } else {
            Err(eda_api::property_bmc_report(&results[0]))
        }
    });
    c.add("api_bounded_negedge_temporal_assertion", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(negedge clk) q1 <= d;
            always @(negedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let assertions =
            eda_api::parse_assertions("assert property (@(negedge clk) d |-> ##[2:2] q2);")?;
        let results = eda_api::check_assertions_bounded(&flow.netlist, &assertions, 3)?;
        if results[0].status == eda_api::PropertyBmcStatus::BoundedPass
            && results[0].frames_checked == 6
        {
            Ok(())
        } else {
            Err(eda_api::property_bmc_report(&results[0]))
        }
    });
    c.add("api_bounded_mixed_assert_and_cover", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let assertions = eda_api::parse_assertions(
            r#"
            assert property (@(posedge clk) d |-> ##[1:1] q2);
            cover property (@(posedge clk) d |-> ##[2:2] q2);
        "#,
        )?;
        let results = eda_api::check_assertions_bounded(&flow.netlist, &assertions, 3)?;
        let report: String = results.iter().map(eda_api::property_bmc_report).collect();
        if results.len() == 2
            && results[0].violated()
            && results[1].covered()
            && results[1].kind == eda_api::AssertionKind::Cover
            && report.contains("VIOLATED")
            && report.contains("COVER HIT")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_bounded_assertions_with_sva_assumptions", || {
        let src = "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let properties = eda_api::parse_assertions(
            r#"
            assert property (@(posedge clk) !d);
            assume property (@(posedge clk) !d);
        "#,
        )?;
        let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
            .into_iter()
            .partition(|property| property.kind != eda_api::AssertionKind::Assume);
        let results = eda_api::check_assertions_bounded_with_assumptions(
            &flow.netlist,
            &assertions,
            &assumptions,
            3,
        )?;
        if results.len() == 1
            && results[0].status == eda_api::PropertyBmcStatus::BoundedPass
            && eda_api::property_bmc_report(&results[0]).contains("BOUNDED PASS")
        {
            Ok(())
        } else {
            Err(eda_api::property_bmc_report(&results[0]))
        }
    });
    c.add("api_bounded_repeated_antecedent_properties", || {
        let src = "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let assertions = eda_api::parse_assertions(
            r#"
            assert property (@(posedge clk) d[*2] |-> q);
            cover property (@(posedge clk) d[*2] |-> q);
        "#,
        )?;
        let results = eda_api::check_assertions_bounded(&flow.netlist, &assertions, 3)?;
        if results.len() == 2
            && results[0].status == eda_api::PropertyBmcStatus::BoundedPass
            && results[1].covered()
            && results[1].repeat_count == Some(2)
        {
            Ok(())
        } else {
            Err(results
                .iter()
                .map(eda_api::property_bmc_report)
                .collect::<String>())
        }
    });
    c.add("api_bounded_unbounded_repetition_properties", || {
        let src = "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let assertions = eda_api::parse_assertions(
            r#"
            assert property (@(posedge clk) d[*] |-> q);
            cover property (@(posedge clk) d[+] |-> q);
        "#,
        )?;
        let results = eda_api::check_assertions_bounded(&flow.netlist, &assertions, 3)?;
        if results[0].violated()
            && results[0].repeat_count == Some(0)
            && results[1].covered()
            && results[1].repeat_count.is_some()
        {
            Ok(())
        } else {
            Err(results
                .iter()
                .map(eda_api::property_bmc_report)
                .collect::<String>())
        }
    });
    c.add("api_bounded_nonconsecutive_repetition", || {
        let src = "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let assertions = eda_api::parse_assertions(
            r#"
            assert property (@(posedge clk) d[=2] |-> q);
            cover property (@(posedge clk) d[=2] |-> q);
        "#,
        )?;
        let results = eda_api::check_assertions_bounded(&flow.netlist, &assertions, 4)?;
        if results[0].violated()
            && results[0].repeat_count == Some(2)
            && results[0].end_frame.is_some()
            && results[1].covered()
        {
            Ok(())
        } else {
            Err(results
                .iter()
                .map(eda_api::property_bmc_report)
                .collect::<String>())
        }
    });
    c.add("api_bounded_sequence_antecedent", || {
        let src = "module m(input clk, a, b, output reg state, output q);
            always @(posedge clk) state <= a;
            assign q = b & state;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let assertions = eda_api::parse_assertions(
            r#"
            assert property (@(posedge clk) a ##[1:2] b |-> !q);
            cover property (@(posedge clk) a ##1 b |-> q);
        "#,
        )?;
        let results = eda_api::check_assertions_bounded(&flow.netlist, &assertions, 3)?;
        if results[0].violated()
            && results[0].end_frame.is_some()
            && results[1].covered()
            && results[1].repeat_count == Some(1)
        {
            Ok(())
        } else {
            Err(results
                .iter()
                .map(eda_api::property_bmc_report)
                .collect::<String>())
        }
    });
    c.add("api_disable_iff_formal_assertion", || {
        let src = "module m(input clk, d, output reg q, output reset);
            always @(posedge clk) q <= d;
            assign reset = 1'b1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let assertions =
            eda_api::parse_assertions("assert property (@(posedge clk) disable iff (reset) !q);")?;
        let results = eda_api::check_assertions_bounded(&flow.netlist, &assertions, 4)?;
        if results[0].status == eda_api::PropertyBmcStatus::BoundedPass {
            Ok(())
        } else {
            Err(eda_api::property_bmc_report(&results[0]))
        }
    });
    c.add("api_formal_counterexample_vcd", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let assertions =
            eda_api::parse_assertions("assert property (@(posedge clk) d |-> ##[1:1] q2);")?;
        let results = eda_api::check_assertions_bounded(&flow.netlist, &assertions, 3)?;
        let text = eda_api::export_property_counterexample_vcd(&flow.netlist, &results[0])?;
        let waveform = eda_api::parse_vcd(&text)?;
        let trigger = results[0].trigger_frame.unwrap() as u64;
        if waveform.signal("d").is_some()
            && waveform.signal("clk").is_some()
            && waveform.signal("q1").is_some()
            && waveform.signal("q2").is_some()
            && waveform
                .signal("formal_assertion_trigger")
                .and_then(|signal| signal.value_at(trigger))
                .and_then(|value| value.to_u64())
                == Some(1)
            && waveform
                .signal("formal_assertion_miss")
                .and_then(|signal| signal.value_at(trigger + 1))
                .and_then(|value| value.to_u64())
                == Some(1)
            && waveform
                .signal("formal_assertion_violation")
                .and_then(|signal| signal.value_at(trigger))
                .and_then(|value| value.to_u64())
                == Some(1)
        {
            Ok(())
        } else {
            Err(text)
        }
    });
    c.add("api_prove_expression_with_net_invariant", || {
        let src = "module m(input clk, enable, output reg held, output bad);
            always @(posedge clk) held <= held;
            assign bad = held & enable;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let property = eda_api::BoolExpr::Not(Box::new(eda_api::BoolExpr::Signal("bad".into())));
        let proof = eda_api::prove_safety_expression_with_value_invariants(
            &flow.netlist,
            &property,
            4,
            &[("held", false)],
        )?;
        if proof.status != eda_api::ProofStatus::Proved {
            Err(format!(
                "status={:?} report={}",
                proof.status,
                eda_api::proof_report(&proof)
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_formal_counterexample_replay", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let assertions =
            eda_api::parse_assertions("assert property (@(posedge clk) d |-> ##[1:1] q2);")?;
        let results = eda_api::check_assertions_bounded(&flow.netlist, &assertions, 3)?;
        let replay = eda_api::verify_property_counterexample(&flow.netlist, &results[0])?;
        if replay.property_violation_reproduced && replay.transitions_verified > 0 {
            Ok(())
        } else {
            Err(format!("{:?}", replay))
        }
    });
    c.add("api_formal_counterexample_vcd_replay", || {
        let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let assertions =
            eda_api::parse_assertions("assert property (@(posedge clk) d |-> ##[1:1] q2);")?;
        let results = eda_api::check_assertions_bounded(&flow.netlist, &assertions, 3)?;
        let text = eda_api::export_property_counterexample_vcd(&flow.netlist, &results[0])?;
        let replay =
            eda_api::replay_property_counterexample_vcd(&flow.netlist, &results[0], &text)?;
        if replay.frames_verified == results[0].frames_checked
            && replay.property_violation_reproduced
        {
            Ok(())
        } else {
            Err(format!("{:?}", replay))
        }
    });
    c.add("api_prove_expression_with_expression_invariant", || {
        let src = "module m(input clk, enable, output reg held, output safe, output bad);
            always @(posedge clk) held <= held;
            assign safe = ~held;
            assign bad = held & enable;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let property = eda_api::BoolExpr::Not(Box::new(eda_api::BoolExpr::Signal("bad".into())));
        let invariant = eda_api::BoolExpr::Signal("safe".into());
        let proof = eda_api::prove_safety_expression_with_expression_invariants(
            &flow.netlist,
            &property,
            4,
            &[("safe_is_one", &invariant)],
        )?;
        if proof.status != eda_api::ProofStatus::Proved {
            Err(format!(
                "status={:?} report={}",
                proof.status,
                eda_api::proof_report(&proof)
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_prove_safety_with_invariant", || {
        let src = "module m(input clk, enable, output reg held, output bad);
            always @(posedge clk) held <= held;
            assign bad = held & enable;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let without = eda_api::prove_safety(&flow.netlist, "bad", 4)?;
        let with = eda_api::prove_safety_with_invariants(&flow.netlist, "bad", 4, &["held"])?;
        if without.status != eda_api::ProofStatus::Unknown {
            Err(format!(
                "plain proof unexpectedly converged: {:?}",
                without.status
            ))
        } else if with.status != eda_api::ProofStatus::Proved {
            Err(format!(
                "status={:?} report={}",
                with.status,
                eda_api::proof_report(&with)
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_prove_safety_with_value_invariant", || {
        let src = "module m(input clk, enable, output reg held, output safe, output bad);
            always @(posedge clk) held <= held;
            assign safe = ~held;
            assign bad = held & enable;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let proof = eda_api::prove_safety_with_value_invariants(
            &flow.netlist,
            "bad",
            4,
            &[("safe", true)],
        )?;
        if proof.status != eda_api::ProofStatus::Proved {
            Err(format!(
                "status={:?} report={}",
                proof.status,
                eda_api::proof_report(&proof)
            ))
        } else if proof.assumed_invariants != vec![eda_api::ProofInvariant::new("safe", true)] {
            Err(format!("assumed={:?}", proof.assumed_invariants))
        } else {
            Ok(())
        }
    });
}
