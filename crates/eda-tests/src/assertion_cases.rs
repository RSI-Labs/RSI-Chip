//! SVA-like property parser and VCD assertion checker cases.

use crate::CaseList;

fn trace() -> eda_waveform::Vcd {
    eda_waveform::parse_vcd(
        r#"
$timescale 1ns $end
$var wire 1 ! clk $end
$var wire 1 " req $end
$var wire 1 # ack $end
$var wire 1 % reset $end
$var wire 2 $ state [1:0] $end
$enddefinitions $end
#0
0!
0"
0#
0%
b00 $
#10
1!
1"
0#
1%
b01 $
#20
0!
1"
0#
1%
b01 $
#30
1!
1"
1#
1%
b11 $
#40
0!
0"
1#
0%
b11 $
#50
1!
0"
1#
0%
b10 $
"#,
    )
    .expect("valid VCD")
}

pub fn register(c: &mut CaseList) {
    c.add("assert_parse_named_property", || {
        let assertions = eda_assert::parse_assertions(
            r#"
            property p_request;
                assert property (@(posedge clk) req |-> ##[0:1] ack);
            endproperty
        "#,
        )?;
        if assertions.len() == 1
            && assertions[0].name == "p_request"
            && assertions[0].clock.as_deref() == Some("clk")
        {
            Ok(())
        } else {
            Err(format!("{:?}", assertions))
        }
    });

    c.add("assert_boolean_operators_parse", || {
        let source = r#"
            assert property (@(posedge clk) (req || !ack) && true);
            assert property (@(posedge clk) !(req && ack) || false);
        "#;
        let assertions = eda_assert::parse_assertions(source)?;
        if assertions.len() == 2 {
            Ok(())
        } else {
            Err(format!("parsed {} assertions", assertions.len()))
        }
    });

    c.add("assert_disable_iff_parses", || {
        let assertions = eda_assert::parse_assertions(
            "assert property (@(posedge clk) disable iff (reset) !req);",
        )?;
        if assertions[0].disable.is_some() {
            Ok(())
        } else {
            Err(format!("{:?}", assertions))
        }
    });

    c.add("assert_cover_property_parses", || {
        let assertions =
            eda_assert::parse_assertions("cover property (@(posedge clk) req |-> ##[0:2] ack);")?;
        if assertions[0].kind == eda_assert::AssertionKind::Cover {
            Ok(())
        } else {
            Err(format!("{:?}", assertions[0]))
        }
    });

    c.add("assert_assume_property_parses_and_checks", || {
        let assertions =
            eda_assert::parse_assertions("assume property (@(posedge clk) req |-> ##[0:1] ack);")?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if assertions[0].kind == eda_assert::AssertionKind::Assume
            && results[0].passed()
            && eda_assert::report(&results).contains("ASSUME PASS")
        {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_assume_violation_is_detected", || {
        let assertions =
            eda_assert::parse_assertions("assume property (@(posedge clk) !(req && ack));")?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if matches!(
            results[0].status,
            eda_assert::AssertionStatus::Fail { sample: 1 }
        ) && eda_assert::report(&results).contains("ASSUME FAIL@1")
        {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_repeated_antecedent_parses", || {
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) req[*2:3] |=> ack);")?;
        match &assertions[0].property {
            eda_assert::Property::RepeatedImplication {
                min_repeat,
                max_repeat,
                overlapped,
                ..
            } if *min_repeat == 2 && *max_repeat == 3 && !*overlapped => Ok(()),
            property => Err(format!("{property:?}")),
        }
    });

    c.add("assert_repeated_antecedent_passes", || {
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) req[*2] |-> ack);")?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if results[0].passed() {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_repeated_antecedent_failure", || {
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) ack[*1] |-> !ack);")?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if matches!(
            results[0].status,
            eda_assert::AssertionStatus::Fail { sample: 1 }
        ) {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_repeated_antecedent_cover_hit_and_miss", || {
        let hit = eda_assert::parse_assertions("cover property (@(posedge clk) req[*2] |-> ack);")?;
        let hit_results = eda_assert::check_vcd(&trace(), &hit)?;
        let miss =
            eda_assert::parse_assertions("cover property (@(posedge clk) req[*1] |=> !ack);")?;
        let miss_results = eda_assert::check_vcd(&trace(), &miss)?;
        if hit_results[0].status == (eda_assert::AssertionStatus::CoverHit { sample: 0 })
            && miss_results[0].status == eda_assert::AssertionStatus::CoverMiss
        {
            Ok(())
        } else {
            Err(format!(
                "{}{}",
                eda_assert::report(&hit_results),
                eda_assert::report(&miss_results)
            ))
        }
    });

    c.add("assert_unbounded_repetition_parses", || {
        let star =
            eda_assert::parse_assertions("assert property (@(posedge clk) req[*] |-> ack);")?;
        let plus = eda_assert::parse_assertions("cover property (@(posedge clk) req[+] |=> ack);")?;
        match (&star[0].property, &plus[0].property) {
            (
                eda_assert::Property::RepeatedImplication {
                    min_repeat: star_min,
                    unbounded: star_unbounded,
                    ..
                },
                eda_assert::Property::RepeatedImplication {
                    min_repeat: plus_min,
                    unbounded: plus_unbounded,
                    ..
                },
            ) if *star_min == 0 && *star_unbounded && *plus_min == 1 && *plus_unbounded => Ok(()),
            properties => Err(format!("{:?}", properties)),
        }
    });

    c.add("assert_unbounded_repetition_dynamic_semantics", || {
        let fail =
            eda_assert::parse_assertions("assert property (@(posedge clk) req[*] |-> ack);")?;
        let fail_results = eda_assert::check_vcd(&trace(), &fail)?;
        let hit = eda_assert::parse_assertions("cover property (@(posedge clk) req[+] |=> ack);")?;
        let hit_results = eda_assert::check_vcd(&trace(), &hit)?;
        if matches!(
            fail_results[0].status,
            eda_assert::AssertionStatus::Fail { sample: 0 }
        ) && hit_results[0].status == (eda_assert::AssertionStatus::CoverHit { sample: 0 })
        {
            Ok(())
        } else {
            Err(format!(
                "{}{}",
                eda_assert::report(&fail_results),
                eda_assert::report(&hit_results)
            ))
        }
    });

    c.add("assert_nonconsecutive_repetition_parses", || {
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) req[=2:3] |=> ack);")?;
        match &assertions[0].property {
            eda_assert::Property::NonconsecutiveImplication {
                min_count,
                max_count,
                overlapped,
                ..
            } if *min_count == 2 && *max_count == 3 && !*overlapped => Ok(()),
            property => Err(format!("{property:?}")),
        }
    });

    c.add("assert_nonconsecutive_repetition_dynamic_semantics", || {
        let failure =
            eda_assert::parse_assertions("assert property (@(posedge clk) req[=1] |-> !ack);")?;
        let failure_results = eda_assert::check_vcd(&trace(), &failure)?;
        let hit = eda_assert::parse_assertions("cover property (@(posedge clk) req[=2] |-> ack);")?;
        let hit_results = eda_assert::check_vcd(&trace(), &hit)?;
        if matches!(
            failure_results[0].status,
            eda_assert::AssertionStatus::Fail { sample: 1 }
        ) && hit_results[0].status == (eda_assert::AssertionStatus::CoverHit { sample: 0 })
        {
            Ok(())
        } else {
            Err(format!(
                "{}{}",
                eda_assert::report(&failure_results),
                eda_assert::report(&hit_results)
            ))
        }
    });

    c.add("assert_sequence_antecedent_parses", || {
        let fixed =
            eda_assert::parse_assertions("assert property (@(posedge clk) req ##1 ack |-> ack);")?;
        let ranged = eda_assert::parse_assertions(
            "assert property (@(posedge clk) req ##[1:2] ack |=> !ack);",
        )?;
        match (&fixed[0].property, &ranged[0].property) {
            (
                eda_assert::Property::SequenceImplication {
                    min_delay: fixed_min,
                    max_delay: fixed_max,
                    overlapped: fixed_overlap,
                    ..
                },
                eda_assert::Property::SequenceImplication {
                    min_delay: range_min,
                    max_delay: range_max,
                    overlapped: range_overlap,
                    ..
                },
            ) if *fixed_min == 1
                && *fixed_max == 1
                && *fixed_overlap
                && *range_min == 1
                && *range_max == 2
                && !*range_overlap =>
            {
                Ok(())
            }
            properties => Err(format!("{:?}", properties)),
        }
    });

    c.add("assert_sequence_antecedent_dynamic_semantics", || {
        let failure = eda_assert::parse_assertions(
            "assert property (@(posedge clk) req ##[1:1] ack |-> !ack);",
        )?;
        let failure_results = eda_assert::check_vcd(&trace(), &failure)?;
        let hit =
            eda_assert::parse_assertions("cover property (@(posedge clk) req ##1 ack |-> ack);")?;
        let hit_results = eda_assert::check_vcd(&trace(), &hit)?;
        if matches!(
            failure_results[0].status,
            eda_assert::AssertionStatus::Fail { sample: 0 }
        ) && hit_results[0].status == (eda_assert::AssertionStatus::CoverHit { sample: 0 })
        {
            Ok(())
        } else {
            Err(format!(
                "{}{}",
                eda_assert::report(&failure_results),
                eda_assert::report(&hit_results)
            ))
        }
    });

    c.add("assert_bounded_implication_passes", || {
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) req |-> ##[0:1] ack);")?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if results[0].passed() && results[0].samples == 3 {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_immediate_cover_hit", || {
        let assertions = eda_assert::parse_assertions("cover property (@(posedge clk) ack);")?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if results[0].status == (eda_assert::AssertionStatus::CoverHit { sample: 1 }) {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_immediate_cover_miss", || {
        let assertions =
            eda_assert::parse_assertions("cover property (@(posedge clk) req && !req);")?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if results[0].status == eda_assert::AssertionStatus::CoverMiss {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_temporal_cover_hit", || {
        let assertions =
            eda_assert::parse_assertions("cover property (@(posedge clk) req |-> ##[0:1] ack);")?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if results[0].status == (eda_assert::AssertionStatus::CoverHit { sample: 0 }) {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_temporal_cover_miss", || {
        let assertions =
            eda_assert::parse_assertions("cover property (@(posedge clk) req |=> !ack);")?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if results[0].status == eda_assert::AssertionStatus::CoverMiss {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_open_cover_window_is_unknown", || {
        let assertions = eda_assert::parse_assertions(
            "cover property (@(posedge clk) reset |-> ##[2:2] !ack);",
        )?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if results[0].status == eda_assert::AssertionStatus::Unknown {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_disable_iff_masks_cover_hit", || {
        let assertions = eda_assert::parse_assertions(
            "cover property (@(posedge clk) disable iff (reset) ack);",
        )?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if results[0].status == (eda_assert::AssertionStatus::CoverHit { sample: 2 }) {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_immediate_failure_detected", || {
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) !(req && ack));")?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        match results[0].status {
            eda_assert::AssertionStatus::Fail { sample: 1 } => Ok(()),
            _ => Err(eda_assert::report(&results)),
        }
    });

    c.add("assert_disable_iff_masks_immediate_failure", || {
        let assertions = eda_assert::parse_assertions(
            "assert property (@(posedge clk) disable iff (reset) !req);",
        )?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if results[0].passed() && results[0].samples == 3 {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_disable_iff_aborts_temporal_window", || {
        let assertions = eda_assert::parse_assertions(
            "assert property (@(posedge clk) disable iff (reset) req |-> ack);",
        )?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if results[0].passed() {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_always_failure_detected", || {
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) always !ack);")?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if matches!(results[0].status, eda_assert::AssertionStatus::Fail { .. }) {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_never_is_supported", || {
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) never ack);")?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if matches!(results[0].status, eda_assert::AssertionStatus::Fail { .. }) {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_open_window_is_unknown", || {
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) req |-> ##[2:3] ack);")?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if results[0].status == eda_assert::AssertionStatus::Unknown {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_too_short_window_fails", || {
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) req |=> !ack);")?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if matches!(
            results[0].status,
            eda_assert::AssertionStatus::Fail { sample: 0 }
        ) {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_negedge_sampling", || {
        let assertions =
            eda_assert::parse_assertions("assert property (@(negedge clk) req |-> !ack);")?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if results[0].passed() && results[0].samples == 2 {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_indexed_vector_signal", || {
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) state[0]);")?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if matches!(
            results[0].status,
            eda_assert::AssertionStatus::Fail { sample: 2 }
        ) {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    c.add("assert_nonzero_vector_is_boolean_true", || {
        let assertions = eda_assert::parse_assertions(
            "assert property (@(posedge clk) state |-> ##[0:1] state);",
        )?;
        let results = eda_assert::check_vcd(&trace(), &assertions)?;
        if results[0].passed() {
            Ok(())
        } else {
            Err(eda_assert::report(&results))
        }
    });

    for (name, source) in [
        ("bad_delay", "assert property (a |-> ##[2:1] b);"),
        ("bad_expr", "assert property (a &&);"),
        ("no_assertions", "// nothing"),
        (
            "missing_signal",
            "assert property (@(posedge clk) missing);",
        ),
    ] {
        c.add(format!("assert_err_{}", name), move || {
            if let Ok(assertions) = eda_assert::parse_assertions(source) {
                if eda_assert::check_vcd(&trace(), &assertions).is_ok() {
                    return Err(format!("{} unexpectedly passed", name));
                }
            }
            Ok(())
        });
    }

    c.add("assert_api_parse_and_check", || {
        let assertions =
            eda_api::parse_assertions("assert property (@(posedge clk) req |-> ##[0:1] ack);")?;
        let results = eda_api::check_assertions(&trace(), &assertions)?;
        if results[0].passed() && eda_api::assertion_report(&results).contains("PASS") {
            Ok(())
        } else {
            Err(eda_api::assertion_report(&results))
        }
    });
}
