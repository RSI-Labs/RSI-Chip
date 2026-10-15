use super::sat::{exactly_true_lits, model_value, unroll};
use super::*;

use crate::bmc::{target_lit, BmcOptions, BmcTarget};
use crate::sat::{solve, SatResult};
use eda_assert::{AssertionKind, BoolExpr};
use eda_core::cancellation::FlowCancellation;
use eda_core::netlist::Netlist;
use eda_synth::{synthesize, SynthOptions};

fn netlist(source: &str) -> Netlist {
    let design = eda_hdl::compile(source, None).unwrap();
    synthesize(
        &design,
        &SynthOptions {
            optimize: true,
            map_lib: false,
        },
    )
    .unwrap()
    .netlist
}

#[test]
fn immediate_property_counterexample_is_found() {
    let nl = netlist("module m(input a, output y); assign y = a; endmodule");
    let assertions = eda_assert::parse_assertions("assert property (!a);").unwrap();
    let result = bmc_assertion(&nl, &assertions[0], &BmcOptions::default()).unwrap();
    assert!(result.violated());
    assert!(result.trace.is_some());
}

#[test]
fn temporal_bmc_checks_cancellation_during_encoding() {
    let nl =
        netlist("module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule");
    let assertions =
        eda_assert::parse_assertions("assert property (@(posedge clk) d |-> q);").unwrap();
    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = bmc_assertion_cancellable(
        &nl,
        &assertions[0],
        &BmcOptions {
            depth: 64,
            init_state_zero: true,
        },
        &cancellation,
    )
    .unwrap_err();
    assert_eq!(error, "temporal property BMC cancelled");
}

#[test]
fn sequential_exact_counter_encodes_exactly_two() {
    let mut cnf = crate::sat::Cnf::new();
    let lits: Vec<i32> = (0..4).map(|_| cnf.add_var() as i32).collect();
    let exact = exactly_true_lits(&mut cnf, &lits, 2).unwrap();
    cnf.add_clause(vec![exact]);
    match solve(&cnf) {
        crate::sat::SatResult::Sat(model) => {
            let count = lits.iter().filter(|lit| model_value(&model, **lit)).count();
            assert_eq!(count, 2);
        }
        crate::sat::SatResult::Unsat => panic!("exact-two encoding is UNSAT"),
    }
}

#[test]
fn sequential_exact_counter_accepts_net_trace() {
    let nl = netlist(
        "module sparse(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule",
    );
    let (mut cnf, frames) = unroll(
        &nl,
        &BmcOptions {
            depth: 4,
            init_state_zero: true,
        },
        6,
        &FlowCancellation::new(),
    )
    .unwrap();
    let lits: Vec<i32> = (0..=4)
        .map(|frame| {
            target_lit(
                &nl,
                &mut cnf,
                &frames[frame],
                &BmcTarget::Expr(BoolExpr::Signal("d".into())),
            )
            .unwrap()
        })
        .collect();
    let exact = exactly_true_lits(&mut cnf, &lits, 1).unwrap();
    cnf.add_clause(vec![exact]);
    assert!(solve(&cnf) != SatResult::Unsat);
}

#[test]
fn immediate_cover_witness_is_found() {
    let nl = netlist("module m(input a, output y); assign y = a; endmodule");
    let assertions = eda_assert::parse_assertions("cover property (a);").unwrap();
    let result = bmc_assertion(&nl, &assertions[0], &BmcOptions::default()).unwrap();
    assert_eq!(result.status, PropertyBmcStatus::Satisfied);
    assert!(result.trigger_frame.is_some());
    assert!(result.trace.is_some());
    assert!(property_bmc_report(&result).contains("COVER HIT"));
}

#[test]
fn immediate_assumption_constrains_bounded_assertion() {
    let nl = netlist(
        "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule",
    );
    let properties = eda_assert::parse_assertions(
        r#"
        assert property (@(posedge clk) !d);
        assume property (@(posedge clk) !d);
    "#,
    )
    .unwrap();
    let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
        .into_iter()
        .partition(|property| property.kind != AssertionKind::Assume);
    let unconstrained = bmc_assertion(
        &nl,
        &assertions[0],
        &BmcOptions {
            depth: 3,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert!(unconstrained.violated());

    let constrained = bmc_assertions_with_assumptions(
        &nl,
        &assertions,
        &assumptions,
        &BmcOptions {
            depth: 3,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert_eq!(constrained[0].status, PropertyBmcStatus::BoundedPass);
}

#[test]
fn temporal_assumption_constrains_bounded_assertion() {
    let nl = netlist(
        "module m(input clk, d, en, mode, output reg state, output q);
            always @(posedge clk) state <= d;
            assign q = mode & en;
         endmodule",
    );
    let properties = eda_assert::parse_assertions(
        r#"
        assert property (@(posedge clk) mode && d |-> ##[1:1] q);
        assume property (@(posedge clk) mode && d |-> ##[1:1] mode && en);
    "#,
    )
    .unwrap();
    let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
        .into_iter()
        .partition(|property| property.kind != AssertionKind::Assume);
    let results = bmc_assertions_with_assumptions(
        &nl,
        &assertions,
        &assumptions,
        &BmcOptions {
            depth: 3,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert_eq!(results[0].status, PropertyBmcStatus::BoundedPass);
    assert_eq!(results[0].frames_checked, 6);
}

#[test]
fn inconsistent_assumptions_report_a_vacuous_result() {
    let nl = netlist(
        "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule",
    );
    let properties = eda_assert::parse_assertions(
        r#"
        assert property (@(posedge clk) d);
        assume property (@(posedge clk) d);
        assume property (@(posedge clk) !d);
    "#,
    )
    .unwrap();
    let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
        .into_iter()
        .partition(|property| property.kind != AssertionKind::Assume);
    let results =
        bmc_assertions_with_assumptions(&nl, &assertions, &assumptions, &BmcOptions::default())
            .unwrap();
    assert_eq!(results[0].status, PropertyBmcStatus::AssumptionsUnsat);
    assert!(property_bmc_report(&results[0]).contains("ASSUMPTIONS UNSAT"));
}

#[test]
fn assumptions_constrain_cover_witnesses() {
    let nl = netlist(
        "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule",
    );
    let properties = eda_assert::parse_assertions(
        r#"
        cover property (@(posedge clk) d);
        assume property (@(posedge clk) !d);
    "#,
    )
    .unwrap();
    let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
        .into_iter()
        .partition(|property| property.kind != AssertionKind::Assume);
    let results = bmc_assertions_with_assumptions(
        &nl,
        &assertions,
        &assumptions,
        &BmcOptions {
            depth: 2,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert_eq!(results[0].status, PropertyBmcStatus::NotSatisfied);
}

#[test]
fn temporal_cover_can_be_satisfied_or_missed() {
    let nl = netlist(
        "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule",
    );
    let hit =
        eda_assert::parse_assertions("cover property (@(posedge clk) d |-> ##[2:2] q2);").unwrap();
    let result = bmc_assertion(
        &nl,
        &hit[0],
        &BmcOptions {
            depth: 3,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert_eq!(result.status, PropertyBmcStatus::Satisfied);
    assert!(result.net_trace.is_some());

    let miss_nl = netlist(
        "module impossible(input clk, d, output done);
            assign done = 1'b0;
         endmodule",
    );
    let miss = eda_assert::parse_assertions("cover property (@(posedge clk) d |-> ##[1:1] done);")
        .unwrap();
    let result = bmc_assertion(
        &miss_nl,
        &miss[0],
        &BmcOptions {
            depth: 3,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert_eq!(result.status, PropertyBmcStatus::NotSatisfied);
    assert!(result.trace.is_none());
    assert!(property_bmc_report(&result).contains("COVER MISS"));
}

#[test]
fn repeated_antecedent_properties_are_checked() {
    let nl = netlist(
        "module shift(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule",
    );
    let bad =
        eda_assert::parse_assertions("assert property (@(posedge clk) d[*2] |-> !q);").unwrap();
    let result = bmc_assertion(
        &nl,
        &bad[0],
        &BmcOptions {
            depth: 3,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert!(result.violated());
    assert_eq!(result.repeat_count, Some(2));
    assert!(verify_property_trace(&nl, &result).is_ok());

    let good =
        eda_assert::parse_assertions("assert property (@(posedge clk) d[*2] |-> q);").unwrap();
    let result = bmc_assertion(
        &nl,
        &good[0],
        &BmcOptions {
            depth: 3,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert_eq!(result.status, PropertyBmcStatus::BoundedPass);

    let cover =
        eda_assert::parse_assertions("cover property (@(posedge clk) d[*2] |-> q);").unwrap();
    let result = bmc_assertion(
        &nl,
        &cover[0],
        &BmcOptions {
            depth: 3,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert_eq!(result.status, PropertyBmcStatus::Satisfied);
    assert_eq!(result.repeat_count, Some(2));
}

#[test]
fn unbounded_repetition_is_bounded_by_bmc_depth() {
    let nl = netlist(
        "module shift(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule",
    );
    let star =
        eda_assert::parse_assertions("assert property (@(posedge clk) d[*] |-> q);").unwrap();
    let result = bmc_assertion(
        &nl,
        &star[0],
        &BmcOptions {
            depth: 3,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert!(result.violated());
    assert_eq!(result.repeat_count, Some(0));

    let plus =
        eda_assert::parse_assertions("assert property (@(posedge clk) d[+] |=> q);").unwrap();
    let result = bmc_assertion(
        &nl,
        &plus[0],
        &BmcOptions {
            depth: 3,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert_eq!(result.status, PropertyBmcStatus::BoundedPass);

    let cover =
        eda_assert::parse_assertions("cover property (@(posedge clk) d[+] |-> q);").unwrap();
    let result = bmc_assertion(
        &nl,
        &cover[0],
        &BmcOptions {
            depth: 3,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert_eq!(result.status, PropertyBmcStatus::Satisfied);
    assert_eq!(result.repeat_count, Some(2));
}

#[test]
fn nonconsecutive_repetition_properties_are_checked() {
    let nl = netlist(
        "module sparse(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule",
    );
    let bad =
        eda_assert::parse_assertions("assert property (@(posedge clk) d[=2] |-> q);").unwrap();
    let result = bmc_assertion(
        &nl,
        &bad[0],
        &BmcOptions {
            depth: 4,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert!(result.violated());
    assert_eq!(result.repeat_count, Some(2));
    assert!(result.end_frame.is_some());
    assert!(verify_property_trace(&nl, &result).is_ok());

    let cover =
        eda_assert::parse_assertions("cover property (@(posedge clk) d[=2] |-> q);").unwrap();
    let result = bmc_assertion(
        &nl,
        &cover[0],
        &BmcOptions {
            depth: 4,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert_eq!(result.status, PropertyBmcStatus::Satisfied);
    assert_eq!(result.repeat_count, Some(2));
}

#[test]
fn sequence_antecedent_properties_are_checked() {
    let nl = netlist(
        "module sequence_m(input clk, a, b, output reg state, output q);
            always @(posedge clk) state <= a;
            assign q = b & state;
         endmodule",
    );
    let properties = eda_assert::parse_assertions(
        r#"
        assert property (@(posedge clk) a ##[1:2] b |-> !q);
        cover property (@(posedge clk) a ##1 b |-> q);
    "#,
    )
    .unwrap();
    let results = bmc_assertions(
        &nl,
        &properties,
        &BmcOptions {
            depth: 3,
            init_state_zero: true,
        },
    )
    .unwrap();
    let replay = verify_property_trace(&nl, &results[0]).unwrap();
    assert!(results[0].violated());
    assert!(matches!(results[0].repeat_count, Some(1 | 2)));
    assert!(results[0].end_frame.is_some());
    assert!(replay.property_violation_reproduced);
    assert_eq!(results[1].status, PropertyBmcStatus::Satisfied);
    assert_eq!(results[1].repeat_count, Some(1));
}

#[test]
fn bounded_temporal_delay_is_checked() {
    let nl = netlist(
        "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule",
    );
    let good =
        eda_assert::parse_assertions("assert property (@(posedge clk) d |-> ##[2:2] q2);").unwrap();
    let result = bmc_assertion(
        &nl,
        &good[0],
        &BmcOptions {
            depth: 4,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert_eq!(result.status, PropertyBmcStatus::BoundedPass);
    assert_eq!(result.frames_checked, 7);

    let bad =
        eda_assert::parse_assertions("assert property (@(posedge clk) d |-> ##[1:1] q2);").unwrap();
    let result = bmc_assertion(
        &nl,
        &bad[0],
        &BmcOptions {
            depth: 4,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert_eq!(result.status, PropertyBmcStatus::Violated);
    assert!(result.trigger_frame.is_some());
}

#[test]
fn negedge_sequential_properties_are_checked() {
    let nl = netlist(
        "module shift(input clk, d, output reg q1, q2);
            always @(negedge clk) q1 <= d;
            always @(negedge clk) q2 <= q1;
         endmodule",
    );
    let good =
        eda_assert::parse_assertions("assert property (@(negedge clk) d |-> ##[2:2] q2);").unwrap();
    let result = bmc_assertion(
        &nl,
        &good[0],
        &BmcOptions {
            depth: 3,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert_eq!(result.status, PropertyBmcStatus::BoundedPass);

    let bad =
        eda_assert::parse_assertions("assert property (@(negedge clk) d |-> ##[1:1] q2);").unwrap();
    let result = bmc_assertion(
        &nl,
        &bad[0],
        &BmcOptions {
            depth: 3,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert!(result.violated());
}

#[test]
fn disable_iff_suppresses_property_frames() {
    let nl = netlist(
        "module m(input clk, d, output reg q, output reset);
            always @(posedge clk) q <= d;
            assign reset = 1'b1;
         endmodule",
    );
    let assertions =
        eda_assert::parse_assertions("assert property (@(posedge clk) disable iff (reset) !q);")
            .unwrap();
    let result = bmc_assertion(
        &nl,
        &assertions[0],
        &BmcOptions {
            depth: 4,
            init_state_zero: true,
        },
    )
    .unwrap();
    assert_eq!(result.status, PropertyBmcStatus::BoundedPass);
    assert!(result.trace.is_none());
}

#[test]
fn formal_input_trace_exports_round_trip_vcd() {
    let nl = netlist(
        "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule",
    );
    let assertions =
        eda_assert::parse_assertions("assert property (@(posedge clk) d |-> ##[1:1] q2);").unwrap();
    let result = bmc_assertion(
        &nl,
        &assertions[0],
        &BmcOptions {
            depth: 2,
            init_state_zero: true,
        },
    )
    .unwrap();
    let trace = result.trace.clone().expect("counterexample");
    let text = trace_input_vcd(&nl, &trace).unwrap();
    let waveform = eda_waveform::parse_vcd(&text).unwrap();
    let d = waveform.signal("top.d").unwrap();
    let clk = waveform.signal("top.clk").unwrap();
    let trigger = result.trigger_frame.unwrap() as u64;
    assert_eq!(trace[trigger as usize]["d"], 1);
    assert_eq!(
        d.value_at(trigger).unwrap().to_u64().unwrap(),
        trace[result.trigger_frame.unwrap()]["d"]
    );
    assert!(clk.initial().is_some());
    assert!(waveform.time_points().len() <= trace.len());

    let full_trace = result.net_trace.clone().expect("full counterexample");
    let full_text = trace_net_vcd(&nl, &full_trace).unwrap();
    let full_waveform = eda_waveform::parse_vcd(&full_text).unwrap();
    assert!(full_waveform.signal("top.q1").is_some());
    assert!(full_waveform.signal("top.q2").is_some());
    assert_eq!(full_trace[trigger as usize + 1]["q2"], 0);
    assert_eq!(
        full_waveform
            .signal("top.q2")
            .unwrap()
            .value_at(trigger + 1)
            .unwrap()
            .to_u64(),
        Some(0)
    );

    let marker_text = trace_property_vcd(&nl, &result).unwrap();
    let marker_waveform = eda_waveform::parse_vcd(&marker_text).unwrap();
    let trigger_marker = marker_waveform.signal("formal_assertion_trigger").unwrap();
    let miss_marker = marker_waveform.signal("formal_assertion_miss").unwrap();
    let violation_marker = marker_waveform
        .signal("formal_assertion_violation")
        .unwrap();
    assert_eq!(trigger_marker.value_at(trigger).unwrap().to_u64(), Some(1));
    assert_eq!(miss_marker.value_at(trigger + 1).unwrap().to_u64(), Some(1));
    assert_eq!(
        violation_marker.value_at(trigger).unwrap().to_u64(),
        Some(1)
    );
    let vcd_replay = replay_property_vcd(&nl, &result, &marker_text).unwrap();
    assert_eq!(vcd_replay.frames_verified, full_trace.len());
    assert!(vcd_replay.property_violation_reproduced);

    let q2_id = marker_text
        .lines()
        .find(|line| line.contains("$var wire 1 ") && line.contains(" q2 $end"))
        .and_then(|line| line.split_whitespace().nth(3))
        .unwrap()
        .to_string();
    let tampered_vcd = format!("{}#{}\n1{}\n", marker_text, trigger + 1, q2_id);
    assert!(replay_property_vcd(&nl, &result, &tampered_vcd).is_err());

    let replay = verify_property_trace(&nl, &result).unwrap();
    assert_eq!(replay.frames_verified, full_trace.len());
    assert_eq!(replay.transitions_verified, 2 * (full_trace.len() - 1));
    assert!(replay.property_violation_reproduced);

    let mut tampered = result.clone();
    let trigger_index = result.trigger_frame.unwrap();
    tampered.net_trace.as_mut().unwrap()[trigger_index + 1].insert("q2".to_string(), 1);
    assert!(verify_property_trace(&nl, &tampered).is_err());
}
