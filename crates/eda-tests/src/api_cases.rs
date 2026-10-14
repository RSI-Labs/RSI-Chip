//! End-to-end API cases (exercising the eda-api facade crate).

use crate::CaseList;

fn scaled_timing_view(
    nl: &eda_core::netlist::Netlist,
    timing_scale: f64,
) -> Result<eda_core::netlist::Netlist, String> {
    use std::sync::Arc;

    let mut view = nl.clone();
    let library = view
        .library()
        .cloned()
        .ok_or_else(|| "physical timing view requires a library".to_string())?;
    let mut scaled = (*library).clone();
    for cell in &mut scaled.cells {
        for (_, delay) in &mut cell.arcs {
            *delay *= timing_scale;
        }
    }
    view.set_library(Arc::new(scaled));
    Ok(view)
}

pub fn register(c: &mut CaseList) {
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
    c.add("api_verilog_export", || {
        let src = "module m(input a, b, output y); assign y = a ^ b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        if !flow.verilog.contains("module") {
            Err("no verilog export".into())
        } else {
            Ok(())
        }
    });
    c.add("api_place_and_route_flow", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        eda_pnr::check_legal(&flow.netlist, &flow.placement)?;
        eda_pnr::check_routing(&flow.routing)?;
        if flow.routing.segments.is_empty() {
            Err("API flow did not route any segments".into())
        } else {
            Ok(())
        }
    });
    c.add("api_hard_placement_blockage", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let blockage =
            eda_api::PlacementBlockage::new("macro_keepout", 0, 0, eda_api::SITES_PER_ROW - 1)?;
        let placement =
            eda_api::place_with_blockages(&flow.netlist, std::slice::from_ref(&blockage))?;
        let routing = eda_api::route_maze(&flow.netlist, &placement)?;
        let drc = eda_api::physical_verification(&flow.netlist, &placement, &routing)?;
        let optimized = eda_api::optimize_timing_driven_placement(
            &flow.netlist,
            placement.clone(),
            &eda_api::TimingDrivenPlacementOptions {
                max_passes: 1,
                ..Default::default()
            },
        )?;
        if placement.blockages != vec![blockage]
            || placement.cell_pos.iter().any(|position| position.row == 0)
            || drc.has_errors()
            || optimized.placement.blockages != placement.blockages
            || eda_pnr::check_legal(&flow.netlist, &optimized.placement).is_err()
            || !eda_pnr::report(&flow.netlist, &placement).contains("hard placement blockages: 1")
        {
            Err(format!(
                "placement={:?} optimized={:?} drc={:?}",
                placement, optimized.placement, drc.violations
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_route_with_capacity_options", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let options = eda_api::RouteOptions::new(4)?;
        let routing = eda_api::route_with_options(&flow.netlist, &flow.placement, &options)?;
        eda_pnr::check_routing(&routing)?;
        if routing.tracks_per_channel != 4 || routing.segments.is_empty() {
            Err(format!(
                "tracks={} segments={}",
                routing.tracks_per_channel,
                routing.segments.len()
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_maze_routing", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let routing = eda_api::route_maze(&flow.netlist, &flow.placement)?;
        eda_pnr::check_routing(&routing)?;
        if routing.segments.is_empty() {
            Err("maze router produced no segments".into())
        } else {
            Ok(())
        }
    });
    c.add("api_maze_routing_capacity_options", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let options = eda_api::RouteOptions::new(2)?;
        let routing = eda_api::route_maze_with_options(&flow.netlist, &flow.placement, &options)?;
        eda_pnr::check_routing(&routing)?;
        if routing.tracks_per_channel != 2 {
            Err(format!("tracks={}", routing.tracks_per_channel))
        } else {
            Ok(())
        }
    });
    c.add("api_maze_routing_hard_channel_blockage", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let baseline = eda_api::route_maze(&flow.netlist, &flow.placement)?;
        let Some(blocked_segment) = baseline.segments.first() else {
            return Err("baseline maze route is empty".into());
        };
        let orientation = blocked_segment.orientation;
        let channel = blocked_segment.channel;
        let options = eda_api::RouteOptions::new(4)?.with_blocked_channel(
            "api_keepout",
            orientation,
            channel,
        )?;
        let routing = eda_api::route_maze_with_options(&flow.netlist, &flow.placement, &options)?;
        let drc = eda_api::physical_verification(&flow.netlist, &flow.placement, &routing)?;
        if routing.segments.is_empty()
            || routing.blocked_channels.len() != 1
            || routing
                .segments
                .iter()
                .any(|segment| segment.orientation == orientation && segment.channel == channel)
            || drc.has_errors()
        {
            Err(format!(
                "blocked={:?} segments={:?} drc={:?}",
                routing.blocked_channels, routing.segments, drc.violations
            ))
        } else if eda_api::RouteChannelBlockage::new("", orientation, channel).is_ok() {
            Err("invalid empty routing blockage accepted".into())
        } else {
            let ripup = eda_api::route_maze_ripgup(&flow.netlist, &flow.placement, &options, 4)?;
            if ripup
                .routing
                .segments
                .iter()
                .any(|segment| segment.orientation == orientation && segment.channel == channel)
            {
                Err("ripgup router crossed hard blockage".into())
            } else if eda_pnr::check_routing(&ripup.routing).is_err() {
                Err("ripgup blockage route is illegal".into())
            } else {
                Ok(())
            }
        }
    });
    c.add("api_negotiated_ripgup_routing", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let result = eda_api::route_maze_ripgup(
            &flow.netlist,
            &flow.placement,
            &eda_api::RouteOptions::default(),
            8,
        )?;
        eda_pnr::check_routing(&result.routing)?;
        let drc = eda_pnr::drc(&flow.netlist, &flow.placement, &result.routing)?;
        if drc.has_errors() || result.passes == 0 {
            Err(format!("passes={} drc={:?}", result.passes, drc.violations))
        } else {
            Ok(())
        }
    });
    c.add("api_timing_driven_ripgup_routing", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let critical_net = (0..flow.netlist.net_count())
            .find(|net| {
                let net = flow.netlist.net(*net);
                !net.drivers.is_empty() || !net.sinks.is_empty()
            })
            .ok_or_else(|| "netlist has no internal routed net".to_string())?;
        let timing = eda_api::TimingDrivenRoutingOptions {
            net_weights: std::collections::HashMap::from([(critical_net, 10.0)]),
            default_net_weight: 1.0,
        };
        let result = eda_api::route_maze_timing_ripgup(
            &flow.netlist,
            &flow.placement,
            &eda_api::RouteOptions::default(),
            &timing,
            8,
        )?;
        eda_pnr::check_routing(&result.ripup.routing)?;
        if result.final_weighted_wirelength <= result.initial_weighted_wirelength
            && eda_api::timing_driven_ripgup_report(&result).contains("weighted wirelength")
        {
            Ok(())
        } else {
            Err(eda_api::timing_driven_ripgup_report(&result))
        }
    });
    c.add("api_timing_driven_ripgup_from_sta", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let result = eda_api::route_maze_timing_ripgup_from_timing(
            &flow.netlist,
            &flow.placement,
            &eda_api::RouteOptions::default(),
            &flow.timing,
            8,
        )?;
        eda_pnr::check_routing(&result.ripup.routing)?;
        if result.ripup.passes > 0 {
            Ok(())
        } else {
            Err(eda_api::timing_driven_ripgup_report(&result))
        }
    });
    c.add("api_physical_timing_iteration_flow", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                iterations: 2,
                ..Default::default()
            },
        )?;
        let report = eda_api::physical_timing_flow_report(&result);
        if result.baseline.sdf.interconnects.is_empty()
            || result.optimized.len() != 2
            || result.best().drc.has_errors()
            || !result
                .best()
                .parasitic_report
                .unmatched_netlist_nets
                .is_empty()
            || !report.contains("Physical timing flow")
            || !report.contains("iteration baseline")
        {
            Err(report)
        } else {
            Ok(())
        }
    });
    c.add("api_physical_timing_flow_zero_iterations", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                iterations: 0,
                ..Default::default()
            },
        )?;
        if result.optimized.is_empty()
            && result.best().index == usize::MAX
            && !result.best().sdf.interconnects.is_empty()
        {
            Ok(())
        } else {
            Err(eda_api::physical_timing_flow_report(&result))
        }
    });
    c.add("api_physical_timing_flow_rejects_invalid_options", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let bad = eda_api::PhysicalTimingFlowOptions {
            placement_passes: 0,
            ..Default::default()
        };
        if eda_api::run_physical_timing_flow(&flow.netlist, &bad).is_ok() {
            Err("invalid physical timing options accepted".into())
        } else {
            Ok(())
        }
    });
    c.add(
        "api_constrained_timing_weights_prioritize_critical_path",
        || {
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
            let constrained = eda_api::constrained_timing(&flow.netlist, &constraints)?;
            let weights =
                eda_api::timing_weights_from_constrained_timing(&flow.netlist, &constrained, 9.0)?;
            let d = flow.netlist.net_by_name("d").ok_or("missing d")?;
            let q1 = flow.netlist.net_by_name("q1").ok_or("missing q1")?;
            let q2 = flow.netlist.net_by_name("q2").ok_or("missing q2")?;
            if !weights.is_empty()
                && weights[&q1] == 10.0
                && weights[&d] > weights[&q2]
                && weights[&d] < weights[&q1]
            {
                Ok(())
            } else {
                Err(format!(
                    "d={} q1={} q2={:?}",
                    weights[&d],
                    weights[&q1],
                    weights.get(&q2)
                ))
            }
        },
    );
    c.add(
        "api_waiver_aware_timing_weights_exclude_waived_path",
        || {
            let src = "module m(input a, b, output y, z);
            assign y = ~a;
            assign z = ~b;
         endmodule";
            let flow = eda_api::run_flow(src, None)?;
            let constraints = eda_api::parse_timing_constraints(
                r#"
            create_clock -name clk -period 0.1 [get_ports a]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_input_delay 0.0 -clock clk [get_ports b]
            set_output_delay 0.1 -clock clk [get_ports y]
            set_output_delay 0.1 -clock clk [get_ports z]
        "#,
            )?;
            let constrained = eda_api::constrained_timing(&flow.netlist, &constraints)?;
            let mut store =
                eda_api::timing_violation_store(&constrained.clock, &flow.netlist, &constrained)?;
            let waiver =
                eda_api::timing_violation_waiver("accepted-y", &constrained.clock, "y", -10.0)?
                    .with_kind(eda_api::TimingViolationKind::Output);
            eda_api::add_timing_violation_waiver(&mut store, waiver)?;
            let raw =
                eda_api::timing_weights_from_constrained_timing(&flow.netlist, &constrained, 9.0)?;
            let waiver_aware = eda_api::timing_weights_from_waiver_aware_timing(
                &flow.netlist,
                &constrained,
                &store,
                9.0,
            )?;
            let y = flow.netlist.net_by_name("y").ok_or("missing y")?;
            let a = flow.netlist.net_by_name("a").ok_or("missing a")?;
            let z = flow.netlist.net_by_name("z").ok_or("missing z")?;
            let b = flow.netlist.net_by_name("b").ok_or("missing b")?;
            if raw.contains_key(&y)
                && raw.contains_key(&a)
                && !waiver_aware.contains_key(&y)
                && !waiver_aware.contains_key(&a)
                && waiver_aware.contains_key(&z)
                && waiver_aware.contains_key(&b)
                && store.violations().len() == 2
                && store.len() == 1
            {
                Ok(())
            } else {
                Err(format!(
                    "raw={:?} waiver-aware={:?} raw-count={} active-count={}",
                    raw,
                    waiver_aware,
                    store.violations().len(),
                    store.len()
                ))
            }
        },
    );

    c.add("api_waiver_aware_placement_and_routing", || {
        let src = "module m(input a, b, output y, z);
            assign y = ~a;
            assign z = ~b;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 0.1 [get_ports a]
            set_output_delay 0.1 -clock clk [get_ports y]
            set_output_delay 0.1 -clock clk [get_ports z]
        "#,
        )?;
        let constrained = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let mut store =
            eda_api::timing_violation_store(&constrained.clock, &flow.netlist, &constrained)?;
        let waiver = eda_api::timing_violation_waiver("all", &constrained.clock, "y", -10.0)?
            .with_kind(eda_api::TimingViolationKind::Output);
        eda_api::add_timing_violation_waiver(&mut store, waiver)?;
        let placement = eda_api::timing_driven_place_from_waiver_aware_timing(
            &flow.netlist,
            &constrained,
            &store,
            1,
        )?;
        let routing = eda_api::route_maze_timing_ripgup_from_waiver_aware_timing(
            &flow.netlist,
            &placement.placement,
            &eda_api::RouteOptions::default(),
            &constrained,
            &store,
            1,
        )?;
        if placement.placement.cell_pos.len() == flow.netlist.cell_count()
            && routing.ripup.routing.wirelength() > 0
        {
            Ok(())
        } else {
            Err(format!(
                "cells={} wirelength={}",
                placement.placement.cell_pos.len(),
                routing.ripup.routing.wirelength()
            ))
        }
    });

    c.add("api_slack_driven_physical_timing_flow", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 3.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.3 -clock clk [get_ports y]
        "#,
        )?;
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                constraints: Some(constraints),
                iterations: 1,
                ..Default::default()
            },
        )?;
        let report = eda_api::physical_timing_flow_report(&result);
        if result.baseline.constrained_timing.is_some()
            && result.best().constrained_timing.is_some()
            && result
                .best()
                .constrained_timing
                .as_ref()
                .map(|timing| timing.worst_slack.is_finite())
                .unwrap_or(false)
            && report.contains("constrained slack:")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_physical_timing_flow_accepts_waivers", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 0.1 [get_ports a]
            set_output_delay 0.1 -clock clk [get_ports y]
        "#,
        )?;
        let waiver = eda_api::timing_violation_waiver("physical", "clk", "y", -10.0)?
            .with_kind(eda_api::TimingViolationKind::Output);
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                constraints: Some(constraints),
                waivers: vec![waiver],
                iterations: 1,
                ..Default::default()
            },
        )?;
        let bad = eda_api::PhysicalTimingFlowOptions {
            waivers: vec![eda_api::timing_violation_waiver(
                "orphan", "clk", "y", -1.0,
            )?],
            ..Default::default()
        };
        if result.optimized.len() == 1
            && result.best().constrained_timing.is_some()
            && matches!(
                eda_api::run_physical_timing_flow(&flow.netlist, &bad),
                Err(error) if error.contains("requires SDC constraints")
            )
        {
            Ok(())
        } else {
            Err(eda_api::physical_timing_flow_report(&result))
        }
    });

    c.add("api_physical_timing_flow_repairs_antennas", || {
        use eda_core::library::CellLibrary;
        use eda_core::netlist::{Cell, CellKind, Dir};
        use std::sync::Arc;

        let library = Arc::new(CellLibrary::std());
        let mut netlist = eda_core::netlist::Netlist::new("flow-antenna");
        netlist.set_library(library.clone());
        let input = netlist.add_net("a");
        let long = netlist.add_net("long");
        let output = netlist.add_net("z");
        let buffer = library.find("BUF").unwrap();
        netlist.add_cell(Cell {
            name: "driver".into(),
            kind: CellKind::Lib(buffer),
            inputs: vec![input],
            output: long,
        });
        netlist.add_cell(Cell {
            name: "sink".into(),
            kind: CellKind::Lib(buffer),
            inputs: vec![long],
            output,
        });
        netlist.add_port("a", Dir::In, vec![input]);
        netlist.add_port("z", Dir::Out, vec![output]);
        let placement = eda_pnr::Placement {
            rows: 4,
            cell_pos: vec![
                eda_pnr::Pos { row: 0, site: 0 },
                eda_pnr::Pos { row: 3, site: 30 },
            ],
            cell_width: vec![2, 2],
            blockages: Vec::new(),
        };
        let result = eda_api::run_physical_timing_flow(
            &netlist,
            &eda_api::PhysicalTimingFlowOptions {
                initial_placement: Some(placement),
                iterations: 0,
                repair_antennas: true,
                ..Default::default()
            },
        )?;
        let report = eda_api::physical_timing_flow_report(&result);
        let diodes = result
            .baseline
            .antenna_repairs
            .iter()
            .map(|repair| repair.diodes_inserted)
            .sum::<usize>();
        if result.baseline.antenna_repairs.len() != 1
            || diodes != 1
            || result.baseline.antenna_warnings != 0
            || result.baseline.annotated_netlist.cell_count() != netlist.cell_count() + 1
            || result.baseline.placement.cell_pos.len() != netlist.cell_count() + 1
            || result.baseline.drc.has_errors()
            || eda_pnr::check_routing(&result.baseline.routing).is_err()
            || !report.contains("antenna diodes 1, antenna warnings 0")
        {
            Err(report)
        } else {
            Ok(())
        }
    });

    c.add("api_physical_timing_flow_repairs_hold", || {
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
        let placement = eda_pnr::Placement {
            rows: 4,
            cell_pos: vec![
                eda_pnr::Pos { row: 0, site: 0 },
                eda_pnr::Pos { row: 3, site: 26 },
            ],
            cell_width: flow.placement.cell_width.clone(),
            blockages: Vec::new(),
        };
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                constraints: Some(constraints),
                initial_placement: Some(placement),
                hold_repair: Some(eda_api::HoldRepairOptions::default()),
                iterations: 0,
                ..Default::default()
            },
        )?;
        let report = eda_api::physical_timing_flow_report(&result);
        let hold = result
            .hold_repair
            .as_ref()
            .ok_or("physical flow omitted hold repair")?;
        if hold.buffers_inserted == 0
            || !hold.all_active_violations_repaired
            || result
                .baseline
                .constrained_timing
                .as_ref()
                .map(|timing| timing.hold_slack < 0.0)
                .unwrap_or(true)
            || result.baseline.annotated_netlist.cell_count() <= flow.netlist.cell_count()
            || result.baseline.placement.cell_pos.len()
                != result.baseline.annotated_netlist.cell_count()
            || result.baseline.drc.has_errors()
            || eda_pnr::check_routing(&result.baseline.routing).is_err()
            || !report.contains("Physical timing flow:")
            || !report.contains("Hold repair:")
            || !report.contains("status PASS")
        {
            Err(report)
        } else {
            Ok(())
        }
    });
    c.add(
        "api_physical_timing_flow_repairs_hold_each_iteration",
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
            let result = eda_api::run_physical_timing_flow(
                &flow.netlist,
                &eda_api::PhysicalTimingFlowOptions {
                    constraints: Some(constraints),
                    hold_repair: Some(eda_api::HoldRepairOptions {
                        max_buffers_per_endpoint: 1,
                        ..Default::default()
                    }),
                    iterations: 1,
                    ..Default::default()
                },
            )?;
            let report = eda_api::physical_timing_flow_report(&result);
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
            if baseline_buffers != 1
                || result.baseline.all_active_hold_repaired
                || optimized_buffers != 1
                || !result.optimized[0].all_active_hold_repaired
                || result.baseline.annotated_netlist.cell_count() != flow.netlist.cell_count() + 1
                || result.optimized[0].annotated_netlist.cell_count()
                    != flow.netlist.cell_count() + 2
                || !report.contains("hold status LIMITED")
                || !report.contains("hold status PASS")
            {
                Err(report)
            } else {
                Ok(())
            }
        },
    );
    c.add("api_physical_timing_flow_with_timing_driven_cts", || {
        let src = "module shift(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 8.0 [get_ports clk]
            set_input_delay 0.5 -clock clk [get_ports d]
        "#,
        )?;
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                constraints: Some(constraints),
                timing_driven_cts: Some(eda_api::TimingDrivenCtsOptions {
                    target_skew_ns: 0.1,
                    ..Default::default()
                }),
                iterations: 1,
                ..Default::default()
            },
        )?;
        let report = eda_api::physical_timing_flow_report(&result);
        let clock_tree = result
            .clock_tree
            .as_ref()
            .ok_or("physical flow omitted CTS result")?;
        let inserted = clock_tree.cts.buffers_inserted;
        if inserted == 0
            || result.baseline.annotated_netlist.cell_count()
                != flow.netlist.cell_count() + inserted
            || result.baseline.placement.cell_pos.len() != flow.netlist.cell_count() + inserted
            || result.baseline.drc.has_errors()
            || result.best().drc.has_errors()
            || !clock_tree.target_met
            || !report.contains("Timing-driven CTS search:")
            || !report.contains("target MET")
        {
            Err(report)
        } else {
            Ok(())
        }
    });

    c.add("api_physical_timing_flow_exports_best_verilog", || {
        let src = "module shift(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 8.0 [get_ports clk]
            set_input_delay 0.5 -clock clk [get_ports d]
        "#,
        )?;
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                constraints: Some(constraints),
                timing_driven_cts: Some(eda_api::TimingDrivenCtsOptions {
                    target_skew_ns: 0.1,
                    ..Default::default()
                }),
                iterations: 1,
                ..Default::default()
            },
        )?;
        let best = result.best();
        let text = eda_api::export_verilog(&best.annotated_netlist)?;
        let inserted = result
            .clock_tree
            .as_ref()
            .map(|tree| tree.cts.buffers_inserted)
            .unwrap_or(0);
        let missing_instance = best
            .annotated_netlist
            .cells()
            .iter()
            .map(|cell| cell.name.as_str())
            .filter(|name| !text.contains(name))
            .collect::<Vec<_>>();
        let exported_flow = eda_api::run_flow(&text, Some("shift"))?;
        if inserted == 0
            || best.annotated_netlist.cell_count() != flow.netlist.cell_count() + inserted
            || !missing_instance.is_empty()
            || !exported_flow.equivalent
        {
            Err(format!(
                "inserted={inserted}, cells={}, missing={missing_instance:?}, equivalent={}\n{text}",
                best.annotated_netlist.cell_count(),
                exported_flow.equivalent
            ))
        } else {
            Ok(())
        }
    });

    c.add("api_physical_timing_post_route_power_ir", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
            None,
        )?;
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                iterations: 1,
                ..Default::default()
            },
        )?;
        let best = result.best();
        let vectors = eda_api::default_power_vectors(&best.annotated_netlist);
        let pre_route =
            eda_api::quick_power(&best.annotated_netlist, &eda_api::PowerOptions::default())?;
        let post_route = eda_api::analyze_physical_timing_power_ir(
            best,
            &vectors,
            &eda_api::PostRoutePowerIrOptions::default(),
        )?;
        let effective_cap = post_route.effective_capacitance_pf.values().sum::<f64>();
        if effective_cap <= 0.0
            || post_route.parasitic_dynamic_power_uw <= 0.0
            || post_route.power.dynamic_power_uw <= pre_route.dynamic_power_uw
            || post_route.ir.cells.len() != best.annotated_netlist.cell_count()
            || (post_route.ir.total_power_uw - post_route.power.total_power_uw).abs() > 1e-12
        {
            Err(format!(
                "cap={effective_cap}, parasitic={}, pre={}, post={}, ir cells={}, ir power={}\n{}",
                post_route.parasitic_dynamic_power_uw,
                pre_route.dynamic_power_uw,
                post_route.power.dynamic_power_uw,
                best.annotated_netlist.cell_count(),
                post_route.ir.total_power_uw,
                eda_api::post_route_power_ir_report(&post_route)
            ))
        } else {
            Ok(())
        }
    });

    c.add("api_physical_timing_post_route_power_from_waveform", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, output y); assign y = (a ^ b) & (a | b); endmodule",
            None,
        )?;
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                iterations: 1,
                ..Default::default()
            },
        )?;
        let waveform = eda_api::parse_vcd(
            r#"
$timescale 1ns $end
$scope module top $end
$var wire 1 ! a $end
$var wire 1 " b $end
$var wire 1 # y $end
$upscope $end
$enddefinitions $end
#0
0!
0"
0#
#10
1!
0"
1#
"#,
        )?;
        let post_route = eda_api::analyze_physical_timing_power_ir_from_waveform(
            result.best(),
            &waveform,
            &eda_api::PostRoutePowerIrOptions::default(),
        )?;
        let a = post_route
            .power
            .activity_by_net
            .get("a")
            .ok_or("post-route waveform omitted input 'a'")?;
        if a.transitions != 1
            || post_route.parasitic_dynamic_power_uw <= 0.0
            || !post_route
                .effective_capacitance_pf
                .values()
                .all(|value| value.is_finite() && *value >= 0.0)
        {
            Err(eda_api::post_route_power_ir_report(&post_route))
        } else {
            Ok(())
        }
    });

    c.add("api_mmsc_post_route_power_properties", || {
        let source = r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_scenario_property post_route.voltage_v 0.90
    set_scenario_property post_route.vdd_ohm_per_site 0.05
    set_scenario_property post_route.gnd_ohm_per_site 0.06
    set_scenario_property post_route.max_total_drop_v 0.04
    set_scenario_property post_route.domain.A.voltage_v 0.65
    set_scenario_property post_route.domain.A.vdd_ohm_per_site 0.08
endscenario
scenario fast
    create_clock -name clk -period 3.0 [get_ports a]
    set_scenario_property post_route.frequency_mhz 250.0
    set_scenario_property post_route.voltage_scaled_capacitance false
    set_scenario_property post_route.capacitive_energy_per_pf_pj 0.81
endscenario
"#;
        let definitions = eda_api::parse_timing_scenarios(source)?;
        let scenarios = eda_api::post_route_power_scenarios_from_timing_scenarios(&definitions)?;
        if scenarios.len() != 2
            || scenarios[0].name != "slow"
            || (scenarios[0].options.ir.voltage_v - 0.9).abs() > 1e-12
            || (scenarios[0].options.ir.vdd_ohm_per_site - 0.05).abs() > 1e-12
            || (scenarios[0].options.ir.gnd_ohm_per_site - 0.06).abs() > 1e-12
            || (scenarios[0].options.ir.max_total_drop_v - 0.04).abs() > 1e-12
            || (scenarios[0].options.upf.domains["A"].voltage_v - 0.65).abs() > 1e-12
            || (scenarios[0].options.upf.domains["A"].vdd_ohm_per_site - 0.08).abs() > 1e-12
            || (scenarios[0].options.upf.domains["A"].gnd_ohm_per_site - 0.06).abs() > 1e-12
            || !scenarios[1].options.upf.domains.is_empty()
            || scenarios[1].name != "fast"
            || (scenarios[1].options.power.frequency_mhz - 250.0).abs() > 1e-12
            || scenarios[1].options.voltage_scaled_capacitance
            || (scenarios[1].options.power.capacitive_energy_per_pf_pj - 0.81).abs() > 1e-12
        {
            return Err(format!("{scenarios:?}"));
        }

        let bad = eda_api::parse_timing_scenarios(
            "scenario bad\ncreate_clock -name clk -period 1 [get_ports a]\nset_scenario_property post_route.unknown 1\nendscenario",
        )?;
        if eda_api::post_route_power_scenarios_from_timing_scenarios(&bad).is_ok() {
            Err("unknown post-route property was accepted".into())
        } else {
            Ok(())
        }
    });

    c.add("api_multi_corner_post_route_power_ir", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
            None,
        )?;
        let slow_view = scaled_timing_view(&flow.netlist, 2.0)?;
        let fast_view = scaled_timing_view(&flow.netlist, 0.5)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_output_delay 0.3 -clock clk [get_ports y]
        "#,
        )?;
        let physical_scenarios = vec![
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "slow",
                &slow_view,
                &constraints,
                Vec::new(),
            ),
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "fast",
                &fast_view,
                &constraints,
                Vec::new(),
            ),
        ];
        let result = eda_api::run_multi_corner_physical_timing_flow(
            &eda_api::MultiCornerPhysicalTimingFlowOptions {
                scenarios: physical_scenarios,
                iterations: 1,
                ..Default::default()
            },
        )?;
        let power_scenarios = vec![
            eda_api::MultiCornerPostRoutePowerIrScenario::new(
                "slow",
                eda_api::PostRoutePowerIrOptions {
                    ir: eda_api::IrOptions {
                        voltage_v: 0.9,
                        vdd_ohm_per_site: 0.05,
                        gnd_ohm_per_site: 0.05,
                        ..eda_api::IrOptions::default()
                    },
                    ..eda_api::PostRoutePowerIrOptions::default()
                },
            )?,
            eda_api::MultiCornerPostRoutePowerIrScenario::new(
                "fast",
                eda_api::PostRoutePowerIrOptions::default(),
            )?,
        ];
        let Some(reference) = result.best().corners.first() else {
            return Err("MMMC physical flow omitted power reference".into());
        };
        let vectors = eda_api::default_power_vectors(&reference.evaluation.annotated_netlist);
        let power = eda_api::analyze_multi_corner_physical_timing_power_ir(
            result.best(),
            &vectors,
            &power_scenarios,
        )?;
        let report = eda_api::multi_corner_post_route_power_ir_report(&power);
        let slow = &power.corners[0];
        let fast = &power.corners[1];
        let mismatch_scenarios = vec![power_scenarios[1].clone(), power_scenarios[0].clone()];
        let mismatch = eda_api::analyze_multi_corner_physical_timing_power_ir(
            result.best(),
            &vectors,
            &mismatch_scenarios,
        );
        if power.corners.len() != 2
            || slow.scenario != "slow"
            || fast.scenario != "fast"
            || power.worst_drop_corner != 0
            || slow.result.ir.worst_drop_v <= fast.result.ir.worst_drop_v
            || !report.contains("worst IR drop 'slow'")
            || mismatch.is_ok()
        {
            Err(format!(
                "worst_drop={:?}, slow={}, fast={}, mismatch={:?}\n{report}",
                power.worst_drop_corner,
                slow.result.ir.worst_drop_v,
                fast.result.ir.worst_drop_v,
                mismatch.err()
            ))
        } else {
            Ok(())
        }
    });

    c.add("api_multi_corner_physical_timing_flow", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let slow_view = scaled_timing_view(&flow.netlist, 2.0)?;
        let fast_view = scaled_timing_view(&flow.netlist, 0.5)?;
        let slow_constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.3 -clock clk [get_ports y]
        "#,
        )?;
        let fast_constraints = slow_constraints.clone();
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
        let result = eda_api::run_multi_corner_physical_timing_flow(
            &eda_api::MultiCornerPhysicalTimingFlowOptions {
                scenarios,
                iterations: 1,
                ..Default::default()
            },
        )?;
        let report = eda_api::multi_corner_physical_timing_flow_report(&result);
        let slow = &result.baseline.corners[0];
        let fast = &result.baseline.corners[1];
        if result.baseline.corners.len() == 2
            && result.baseline.placement.rows == slow.evaluation.placement.rows
            && result.baseline.placement.cell_pos == slow.evaluation.placement.cell_pos
            && result.baseline.placement.cell_width == slow.evaluation.placement.cell_width
            && result.baseline.placement.rows == fast.evaluation.placement.rows
            && result.baseline.placement.cell_pos == fast.evaluation.placement.cell_pos
            && result.baseline.placement.cell_width == fast.evaluation.placement.cell_width
            && result.baseline.routing.wirelength() == slow.evaluation.routing.wirelength()
            && result.baseline.routing.wirelength() == fast.evaluation.routing.wirelength()
            && slow.evaluation.timing.critical_path_delay
                > fast.evaluation.timing.critical_path_delay
            && result.baseline.corners[result.baseline.worst_slack_corner].scenario == "slow"
            && result.best().index != usize::MAX
            && report.contains("Multi-corner physical timing flow")
            && report.contains("scenario 'slow' *")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("api_multi_corner_physical_timing_flow_with_cts", || {
        let src = "module shift(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let slow_view = scaled_timing_view(&flow.netlist, 2.0)?;
        let fast_view = scaled_timing_view(&flow.netlist, 0.5)?;
        let slow_constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 10.0 [get_ports clk]
            set_input_delay 0.5 -clock clk [get_ports d]
        "#,
        )?;
        let fast_constraints = slow_constraints.clone();
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
        let result = eda_api::run_multi_corner_physical_timing_flow(
            &eda_api::MultiCornerPhysicalTimingFlowOptions {
                scenarios,
                timing_driven_cts: Some(eda_api::TimingDrivenCtsOptions {
                    target_skew_ns: 0.1,
                    ..Default::default()
                }),
                iterations: 1,
                ..Default::default()
            },
        )?;
        let report = eda_api::multi_corner_physical_timing_flow_report(&result);
        let clock_tree = result
            .clock_tree
            .as_ref()
            .ok_or("MMMC physical flow omitted CTS result")?;
        let inserted = clock_tree.cts.buffers_inserted;
        let slow = &result.baseline.corners[0];
        let fast = &result.baseline.corners[1];
        if inserted == 0
            || slow.evaluation.annotated_netlist.cell_count()
                != flow.netlist.cell_count() + inserted
            || fast.evaluation.annotated_netlist.cell_count()
                != slow.evaluation.annotated_netlist.cell_count()
            || slow.evaluation.drc.has_errors()
            || fast.evaluation.drc.has_errors()
            || !clock_tree.target_met
            || !report.contains("Multi-corner physical timing flow")
            || !report.contains("Timing-driven CTS search:")
            || !report.contains("target MET")
        {
            Err(report)
        } else {
            Ok(())
        }
    });

    c.add(
        "api_multi_corner_physical_timing_flow_repairs_antennas",
        || {
            use eda_core::library::CellLibrary;
            use eda_core::netlist::{Cell, CellKind, Dir};
            use std::sync::Arc;

            let library = Arc::new(CellLibrary::std());
            let mut netlist = eda_core::netlist::Netlist::new("mmmc-antenna");
            netlist.set_library(library.clone());
            let input = netlist.add_net("a");
            let long = netlist.add_net("long");
            let output = netlist.add_net("z");
            let buffer = library.find("BUF").unwrap();
            netlist.add_cell(Cell {
                name: "driver".into(),
                kind: CellKind::Lib(buffer),
                inputs: vec![input],
                output: long,
            });
            netlist.add_cell(Cell {
                name: "sink".into(),
                kind: CellKind::Lib(buffer),
                inputs: vec![long],
                output,
            });
            netlist.add_port("a", Dir::In, vec![input]);
            netlist.add_port("z", Dir::Out, vec![output]);
            let slow_view = scaled_timing_view(&netlist, 2.0)?;
            let fast_view = scaled_timing_view(&netlist, 0.5)?;
            let slow_constraints = eda_api::parse_timing_constraints(
                "create_clock -name clk -period 8.0 [get_ports a]",
            )?;
            let fast_constraints = slow_constraints.clone();
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
                    eda_pnr::Pos { row: 3, site: 30 },
                ],
                cell_width: vec![2, 2],
                blockages: Vec::new(),
            };
            let result = eda_api::run_multi_corner_physical_timing_flow(
                &eda_api::MultiCornerPhysicalTimingFlowOptions {
                    scenarios,
                    initial_placement: Some(placement),
                    iterations: 0,
                    repair_antennas: true,
                    ..Default::default()
                },
            )?;
            let report = eda_api::multi_corner_physical_timing_flow_report(&result);
            let diodes = result
                .baseline
                .antenna_repairs
                .iter()
                .map(|repair| repair.diodes_inserted)
                .sum::<usize>();
            let slow = &result.baseline.corners[0];
            let fast = &result.baseline.corners[1];
            if diodes != 1
                || result.baseline.antenna_warnings != 0
                || slow.evaluation.annotated_netlist.cell_count() != netlist.cell_count() + 1
                || fast.evaluation.annotated_netlist.cell_count() != netlist.cell_count() + 1
                || result.baseline.placement.cell_pos.len() != netlist.cell_count() + 1
                || slow.evaluation.drc.has_errors()
                || fast.evaluation.drc.has_errors()
                || !report.contains("antenna diodes 1, antenna warnings 0")
            {
                Err(report)
            } else {
                Ok(())
            }
        },
    );

    c.add("api_multi_corner_physical_timing_waiver_objective", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let slow_view = scaled_timing_view(&flow.netlist, 2.0)?;
        let fast_view = scaled_timing_view(&flow.netlist, 0.5)?;
        let slow_constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 0.1 [get_ports a]
            set_output_delay 0.1 -clock clk [get_ports y]
        "#,
        )?;
        let fast_constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_output_delay 0.3 -clock clk [get_ports y]
        "#,
        )?;
        let slow_waiver = eda_api::timing_violation_waiver("slow-only", "slow", "y", -10.0)?
            .with_kind(eda_api::TimingViolationKind::Output);
        let scenarios = vec![
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "slow",
                &slow_view,
                &slow_constraints,
                vec![slow_waiver],
            ),
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "fast",
                &fast_view,
                &fast_constraints,
                Vec::new(),
            ),
        ];
        let result = eda_api::run_multi_corner_physical_timing_flow(
            &eda_api::MultiCornerPhysicalTimingFlowOptions {
                scenarios,
                iterations: 1,
                ..Default::default()
            },
        )?;
        let report = eda_api::multi_corner_physical_timing_flow_report(&result);
        if result.baseline.corners[result.baseline.worst_slack_corner].scenario == "slow"
            && result.baseline.corners[0]
                .violations
                .waived_violations()
                .len()
                == 1
            && result.baseline.corners[1]
                .violations
                .waived_violations()
                .is_empty()
            && result.optimized[0].optimization_corner == Some(1)
            && report.contains("scenario 'fast':")
            && report.contains("optimization objective")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("api_multicorner_physical_flow_violation_registry", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let slow_view = scaled_timing_view(&flow.netlist, 2.0)?;
        let fast_view = scaled_timing_view(&flow.netlist, 0.5)?;
        let slow_constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 0.1 [get_ports a]
            set_output_delay 0.1 -clock clk [get_ports y]
        "#,
        )?;
        let fast_constraints = slow_constraints.clone();
        let slow_waiver = eda_api::timing_violation_waiver("slow-only", "slow", "*", -10.0)?
            .with_kind(eda_api::TimingViolationKind::Output);
        let scenarios = vec![
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "slow",
                &slow_view,
                &slow_constraints,
                vec![slow_waiver],
            ),
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "fast",
                &fast_view,
                &fast_constraints,
                Vec::new(),
            ),
        ];
        let result = eda_api::run_multi_corner_physical_timing_flow(
            &eda_api::MultiCornerPhysicalTimingFlowOptions {
                scenarios,
                iterations: 1,
                ..Default::default()
            },
        )?;
        let registry = &result.baseline.registry;
        let report = eda_api::multi_corner_physical_timing_flow_report(&result);
        let slow_store = registry
            .stores()
            .get("slow")
            .ok_or("registry omitted slow store")?;
        let fast_store = registry
            .stores()
            .get("fast")
            .ok_or("registry omitted fast store")?;
        if !slow_store.is_empty()
            || slow_store.waived_violations().is_empty()
            || fast_store.is_empty()
            || !fast_store.waived_violations().is_empty()
            || registry.summaries().get("output:y").map(|summary| {
                summary.worst_scenario.as_str() != "fast" || summary.scenarios.len() != 1
            }) != Some(false)
            || !registry.waivers().is_empty()
            || !report.contains("active violations ")
            || !report.contains("cross-corner checks ")
            || result.optimized.len() != 1
            || result.optimized[0].registry_update.is_none()
            || result.optimized[0].registry.generation()
                != result.baseline.registry.generation() + 1
            || !report.contains("registry lifecycle:")
        {
            Err(report)
        } else {
            Ok(())
        }
    });

    c.add(
        "api_multi_corner_physical_timing_rejects_width_change",
        || {
            let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
            let flow = eda_api::run_flow(src, None)?;
            let mut other = flow.netlist.clone();
            let library = other
                .library()
                .cloned()
                .ok_or_else(|| "physical timing view requires a library".to_string())?;
            let mut changed = (*library).clone();
            for cell in &mut changed.cells {
                cell.width_sites += 1;
            }
            other.set_library(std::sync::Arc::new(changed));
            let constraints = eda_api::parse_timing_constraints(
                "create_clock -name clk -period 4.0 [get_ports a]",
            )?;
            let scenarios = vec![
                eda_api::MultiCornerPhysicalTimingScenario::new(
                    "reference",
                    &flow.netlist,
                    &constraints,
                    Vec::new(),
                ),
                eda_api::MultiCornerPhysicalTimingScenario::new(
                    "incompatible",
                    &other,
                    &constraints,
                    Vec::new(),
                ),
            ];
            match eda_api::run_multi_corner_physical_timing_flow(
                &eda_api::MultiCornerPhysicalTimingFlowOptions {
                    scenarios,
                    ..Default::default()
                },
            ) {
                Err(error) if error.contains("width differs") => Ok(()),
                other => Err(format!(
                    "expected physical topology rejection, got {:?}",
                    other
                )),
            }
        },
    );

    c.add("api_extract_interconnect_parasitics_from_routing", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let model = eda_api::InterconnectModel::default();
        let spef = eda_api::extract_interconnect_parasitics(&flow.netlist, &flow.routing, &model)?;
        let report = eda_api::parasitic_analysis(&flow.netlist, &spef)?;
        let expected_grounded = flow.routing.wirelength() as f64
            * model.wire_capacitance_pf_per_site
            + flow.routing.vias as f64 * model.via_capacitance_pf;
        let expected_resistance = flow.routing.wirelength() as f64
            * model.wire_resistance_ohm_per_site
            + flow.routing.vias as f64 * model.via_resistance_ohm;
        let sdf = eda_api::parasitics_to_sdf(&flow.netlist, &spef)?;
        let parsed_sdf = eda_api::parse_delay_file(&sdf)?;
        let quality = eda_api::analyze_signal_quality_with_parasitics(
            &flow.netlist,
            &report,
            &eda_api::QualityOptions {
                max_capacitance_pf: Some(0.0),
                ..Default::default()
            },
        )?;
        if report.unmatched_netlist_nets.is_empty()
            && report.net_count == flow.netlist.net_count()
            && (report.total_grounded_cap_pf - expected_grounded).abs() < 1e-12
            && (report.total_resistance_ohm - expected_resistance).abs() < 1e-12
            && report.worst_delay_ns > 0.0
            && !parsed_sdf.interconnects.is_empty()
            && quality.capacitances_checked > 0
        {
            Ok(())
        } else {
            Err(eda_api::parasitic_report(&report))
        }
    });
    c.add("api_routing_parasitics_write_round_trip", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let spef = eda_api::extract_interconnect_parasitics(
            &flow.netlist,
            &flow.routing,
            &eda_api::InterconnectModel::default(),
        )?;
        let text = eda_api::write_parasitics(&spef)?;
        let reparsed = eda_api::parse_parasitics(&text)?;
        let before = eda_api::parasitic_analysis(&flow.netlist, &spef)?;
        let after = eda_api::parasitic_analysis(&flow.netlist, &reparsed)?;
        if after.net_count == before.net_count
            && after.unmatched_netlist_nets.is_empty()
            && (after.total_resistance_ohm - before.total_resistance_ohm).abs() < 1e-12
            && (after.total_grounded_cap_pf - before.total_grounded_cap_pf).abs() < 1e-12
        {
            Ok(())
        } else {
            Err(format!("before={:?}\nafter={:?}", before, after))
        }
    });
    c.add("api_routing_coupling_extraction", || {
        let mut netlist = eda_core::netlist::Netlist::new("coupled");
        let left = netlist.add_net("left");
        let right = netlist.add_net("right");
        let routing = eda_pnr::Routing {
            segments: vec![
                eda_pnr::Segment {
                    net: left,
                    orientation: eda_pnr::Orientation::Horizontal,
                    channel: 0,
                    start: 0,
                    end: 5,
                    track: 0,
                },
                eda_pnr::Segment {
                    net: right,
                    orientation: eda_pnr::Orientation::Horizontal,
                    channel: 0,
                    start: 2,
                    end: 7,
                    track: 1,
                },
            ],
            vias: 0,
            via_locations: Vec::new(),
            tracks_per_channel: 2,
            congested_channels: Vec::new(),
            blocked_channels: Vec::new(),
        };
        let model = eda_api::InterconnectModel {
            coupling_capacitance_pf_per_site: 0.1,
            ..Default::default()
        };
        let spef = eda_api::extract_interconnect_parasitics(&netlist, &routing, &model)?;
        let report = eda_api::parasitic_analysis(&netlist, &spef)?;
        let left = report
            .nets
            .iter()
            .find(|net| net.net == "left")
            .ok_or("missing left")?;
        let right = report
            .nets
            .iter()
            .find(|net| net.net == "right")
            .ok_or("missing right")?;
        if (left.coupling_cap_pf - 0.3).abs() < 1e-12 && (right.coupling_cap_pf - 0.3).abs() < 1e-12
        {
            Ok(())
        } else {
            Err(format!("{:?}\n{:?}", left, right))
        }
    });
    c.add("api_timing_driven_placement_from_sta", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let result = eda_api::timing_driven_place_from_timing(&flow.netlist, &flow.timing, 4)?;
        eda_pnr::check_legal(&flow.netlist, &result.placement)?;
        if !eda_api::timing_driven_placement_report(&result).contains("weighted HPWL") {
            Err(eda_api::timing_driven_placement_report(&result))
        } else {
            Ok(())
        }
    });
    c.add("api_timing_driven_placement_explicit_weights", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let critical_net = (0..flow.netlist.net_count())
            .find(|net| {
                let net = flow.netlist.net(*net);
                !net.drivers.is_empty() || !net.sinks.is_empty()
            })
            .ok_or_else(|| "netlist has no connected nets".to_string())?;
        let options = eda_api::TimingDrivenPlacementOptions {
            net_weights: std::collections::HashMap::from([(critical_net, 25.0)]),
            default_net_weight: 1.0,
            max_passes: 4,
        };
        let result = eda_api::timing_driven_place(&flow.netlist, &options)?;
        if result.final_weighted_hpwl <= result.initial_weighted_hpwl
            && eda_api::timing_driven_placement_report(&result).contains("weighted HPWL")
        {
            Ok(())
        } else {
            Err(eda_api::timing_driven_placement_report(&result))
        }
    });
    c.add("api_physical_export_preserves_vias", || {
        let src = "module m(input clk, rst, output reg [3:0] q);
            always @(posedge clk or posedge rst)
                if (rst) q <= 4'd0; else q <= q + 4'd1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let (_, def_text) =
            eda_api::export_physical(&flow.netlist, &flow.placement, &flow.routing)?;
        let def = eda_api::parse_physical_def(&def_text)?;
        let via_count = def.nets.iter().map(|net| net.vias.len()).sum::<usize>();
        if via_count != flow.routing.via_locations.len() || via_count == 0 {
            Err(format!(
                "DEF vias={} explicit={}",
                via_count,
                flow.routing.via_locations.len()
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_apply_def_locations_to_spice", || {
        let def_text = r#"
DESIGN spatial_layout ;
ROW ROW_0 core 0 0 N DO 20 BY 1 STEP 100 0 ;
COMPONENTS 2 ;
- R1 RES + PLACED ( 100 0 ) N ;
- R2 RES + PLACED ( 500 0 ) N ;
END COMPONENTS
"#;
        let def = eda_api::parse_physical_def(def_text)?;
        let spice = r#"
spatial layout
Vin in 0 1.8
R1 in out 1k
R2 out 0 2k
.mc 256 op V(out) seed=91 rtol=0.05 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 spatialcorr=0.7 spatialscale=0.04
.end
"#;
        let mut circuit = eda_api::parse_spice(spice)?;
        let matched = eda_api::apply_physical_def_locations(&mut circuit, &def, 100.0)?;
        let r1 = circuit.locations.get("R1").copied();
        let r2 = circuit.locations.get("R2").copied();
        if matched != 2
            || r1 != Some(eda_api::ElementLocation {
                x_m: 1e-6,
                y_m: 0.0,
            })
            || r2 != Some(eda_api::ElementLocation {
                x_m: 5e-6,
                y_m: 0.0,
            })
        {
            Err(format!("matched={matched}, R1={r1:?}, R2={r2:?}"))
        } else {
            let mc = eda_api::monte_carlo(&circuit, &circuit.analyses[0])?;
            if mc.samples.len() != 256
                || mc.samples[0].randomized_objects != 2
                || !mc.statistics.standard_deviation.is_finite()
            {
                Err(format!("{mc:?}"))
            } else {
                Ok(())
            }
        }
    });
    c.add("api_power_analysis", || {
        let src = "module m(input a, b, output y); assign y = a ^ b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let report = eda_api::quick_power(&flow.netlist, &eda_api::PowerOptions::default())?;
        if report.total_power_uw <= 0.0 || report.vectors != 2 {
            Err(format!(
                "total={}, vectors={}",
                report.total_power_uw, report.vectors
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_physical_verification", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let result = eda_api::physical_verification(&flow.netlist, &flow.placement, &flow.routing)?;
        if result.has_errors() {
            Err(eda_pnr::drc_report(&result))
        } else {
            Ok(())
        }
    });
    c.add("api_lef_def_physical_export", || {
        let src = "module m(input a, b, output y); assign y = a ^ b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        if !flow.lef.contains("MACRO XOR2") || !flow.def.contains("COMPONENTS") {
            Err("API flow physical export incomplete".into())
        } else if eda_pnr::parse_def(&flow.def).is_err() {
            Err("API DEF does not re-parse".into())
        } else {
            Ok(())
        }
    });
    c.add("api_import_physical_def_text_round_trip", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let placement_blockage = eda_api::PlacementBlockage::new("text_keepout", 1, 2, 4)?;
        let placement = eda_api::place_with_blockages(&flow.netlist, &[placement_blockage])?;
        let baseline = eda_api::route_maze(&flow.netlist, &placement)?;
        let segment = baseline
            .segments
            .first()
            .ok_or("baseline routing has no segments")?;
        let route_options = eda_api::RouteOptions::new(5)?.with_blocked_channel(
            "text_route_keepout",
            segment.orientation,
            segment.channel,
        )?;
        let routing = eda_api::route_maze_with_options(&flow.netlist, &placement, &route_options)?;
        let (_, def_text) = eda_api::export_physical(&flow.netlist, &placement, &routing)?;
        let (imported_placement, imported_routing) =
            eda_api::import_physical_def_text(&flow.netlist, &def_text)?;
        let drc =
            eda_api::physical_verification(&flow.netlist, &imported_placement, &imported_routing)?;
        if imported_placement == placement
            && imported_routing == routing
            && !drc.has_errors()
            && def_text.contains("TRACKS 5 ;")
        {
            Ok(())
        } else {
            Err(format!(
                "placement={:?}\nrouting={:?}\n{}",
                imported_placement,
                imported_routing,
                eda_pnr::drc_report(&drc)
            ))
        }
    });
    c.add("api_def_blockage_export_round_trip", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let placement_blockage = eda_api::PlacementBlockage::new("macro", 1, 2, 5)?;
        let placement = eda_api::place_with_blockages(
            &flow.netlist,
            std::slice::from_ref(&placement_blockage),
        )?;
        let baseline = eda_api::route_maze(&flow.netlist, &placement)?;
        let segment = baseline
            .segments
            .first()
            .ok_or("baseline routing is empty")?;
        let route_options = eda_api::RouteOptions::new(4)?.with_blocked_channel(
            "route_keepout",
            segment.orientation,
            segment.channel,
        )?;
        let routing = eda_api::route_maze_with_options(&flow.netlist, &placement, &route_options)?;
        let (_, def) = eda_api::export_physical(&flow.netlist, &placement, &routing)?;
        let parsed = eda_api::parse_physical_def(&def)?;
        if parsed.placement_blockages.len() != 1
            || parsed.placement_blockages[0].name != placement_blockage.name
            || parsed.placement_blockages[0].row != placement_blockage.row
            || parsed.placement_blockages[0].start_site != placement_blockage.start_site
            || parsed.placement_blockages[0].end_site != placement_blockage.end_site
            || parsed.route_blockages.len() != 1
            || parsed.route_blockages[0].name != "route_keepout"
            || parsed.route_blockages[0].orientation != segment.orientation
            || parsed.route_blockages[0].channel != segment.channel
        {
            Err(format!(
                "placement={:?} route={:?}",
                parsed.placement_blockages, parsed.route_blockages
            ))
        } else {
            let (imported_placement, imported_routing) =
                eda_api::import_physical_def(&flow.netlist, &parsed)?;
            if imported_placement == placement
                && imported_routing == routing
                && eda_pnr::check_routing(&imported_routing).is_ok()
            {
                Ok(())
            } else {
                Err(format!(
                    "imported placement={:?} routing={:?}",
                    imported_placement, imported_routing
                ))
            }
        }
    });
    c.add("api_repair_antenna_violations", || {
        use eda_core::library::CellLibrary;
        use eda_core::netlist::{Cell, CellKind, Dir};
        use std::sync::Arc;

        let library = Arc::new(CellLibrary::std());
        let mut netlist = eda_core::netlist::Netlist::new("antenna");
        netlist.set_library(library.clone());
        let input = netlist.add_net("a");
        let routed = netlist.add_net("long");
        let output = netlist.add_net("z");
        let buffer = library.find("BUF").unwrap();
        netlist.add_cell(Cell {
            name: "driver".into(),
            kind: CellKind::Lib(buffer),
            inputs: vec![input],
            output: routed,
        });
        netlist.add_cell(Cell {
            name: "sink".into(),
            kind: CellKind::Lib(buffer),
            inputs: vec![routed],
            output,
        });
        netlist.add_port("a", Dir::In, vec![input]);
        netlist.add_port("z", Dir::Out, vec![output]);
        let placement = eda_pnr::Placement {
            rows: 4,
            cell_pos: vec![
                eda_pnr::Pos { row: 0, site: 0 },
                eda_pnr::Pos { row: 3, site: 30 },
            ],
            cell_width: vec![2, 2],
            blockages: Vec::new(),
        };
        let routing = eda_pnr::Routing {
            segments: vec![
                eda_pnr::Segment {
                    net: routed,
                    orientation: eda_pnr::Orientation::Horizontal,
                    channel: 0,
                    start: 1,
                    end: 31,
                    track: 0,
                },
                eda_pnr::Segment {
                    net: routed,
                    orientation: eda_pnr::Orientation::Vertical,
                    channel: 31,
                    start: 0,
                    end: 3,
                    track: 0,
                },
                eda_pnr::Segment {
                    net: output,
                    orientation: eda_pnr::Orientation::Vertical,
                    channel: 31,
                    start: 3,
                    end: 4,
                    track: 1,
                },
            ],
            vias: 1,
            via_locations: vec![eda_pnr::Via {
                net: routed,
                x: 31,
                y: 0,
            }],
            tracks_per_channel: 2,
            congested_channels: Vec::new(),
            blocked_channels: Vec::new(),
        };
        let before = eda_api::physical_verification(&netlist, &placement, &routing)?;
        let result = eda_api::repair_antennas(
            &netlist,
            &placement,
            &routing,
            &eda_api::AntennaRepairOptions::default(),
        )?;
        let report = eda_api::antenna_repair_report(&result);
        if before.has_errors()
            || !before.has_warnings()
            || !result.all_antennas_repaired
            || result.diodes_inserted != 1
            || result.repairs.len() != 1
            || result.repairs[0].old_ratio <= 16.0
            || result.repairs[0].new_ratio > 16.0
            || result.netlist.cell_count() != netlist.cell_count() + 1
            || result.physical_placement.cell_pos.len() != netlist.cell_count() + 1
            || eda_pnr::check_routing(&result.routing).is_err()
            || !report.contains("Antenna repair: 1 diode(s), 1 net(s), status PASS")
        {
            Err(format!(
                "before={:?}\n{}",
                before.violations,
                eda_pnr::drc_report(&before)
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_error_unknown_module", || {
        if eda_api::compile("module m; ghost u(); endmodule", None).is_ok() {
            Err("should fail".into())
        } else {
            Ok(())
        }
    });
}
