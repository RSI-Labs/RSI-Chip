//! Formal property implementation module.

use eda_assert::AssertionKind;

use super::types::*;

pub fn property_bmc_report(result: &PropertyBmcResult) -> String {
    if result.kind == AssertionKind::Assume {
        return match result.status {
            PropertyBmcStatus::BoundedPass => format!(
                "Temporal BMC report '{}': ASSUMPTION HOLDS through {} cycle(s), {} frame(s)\n",
                result.assertion, result.depth_checked, result.frames_checked
            ),
            PropertyBmcStatus::Violated => format!(
                "Temporal BMC report '{}': ASSUMPTION VIOLATION at cycle {}, {} frame(s) checked\n",
                result.assertion,
                result
                    .trigger_frame
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".into()),
                result.frames_checked
            ),
            PropertyBmcStatus::AssumptionsUnsat => format!(
                "Temporal BMC report '{}': ASSUMPTIONS UNSAT (vacuous result)\n",
                result.assertion
            ),
            PropertyBmcStatus::Satisfied => format!(
                "Temporal BMC report '{}': ASSUMPTION WITNESS at cycle {}\n",
                result.assertion,
                result
                    .trigger_frame
                    .map(|v| v.to_string())
                    .unwrap_or_default()
            ),
            PropertyBmcStatus::NotSatisfied => format!(
                "Temporal BMC report '{}': NO ASSUMPTION WITNESS through {} cycle(s)\n",
                result.assertion, result.depth_checked
            ),
        };
    }
    let repeat = result
        .repeat_count
        .map(|count| format!(", repeat={}", count))
        .unwrap_or_default();
    match result.status {
        PropertyBmcStatus::BoundedPass => format!(
            "Temporal BMC report '{}': BOUNDED PASS through {} cycle(s), {} frame(s)\n",
            result.assertion, result.depth_checked, result.frames_checked
        ),
        PropertyBmcStatus::Violated => format!(
            "Temporal BMC report '{}': VIOLATED at trigger cycle {}{}, {} frame(s) checked\n",
            result.assertion,
            result
                .trigger_frame
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".into()),
            repeat,
            result.frames_checked
        ),
        PropertyBmcStatus::Satisfied => format!(
            "Temporal BMC report '{}': COVER HIT at cycle {}{}, {} frame(s) checked\n",
            result.assertion,
            result
                .trigger_frame
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".into()),
            repeat,
            result.frames_checked
        ),
        PropertyBmcStatus::NotSatisfied => format!(
            "Temporal BMC report '{}': COVER MISS through {} cycle(s), {} frame(s)\n",
            result.assertion, result.depth_checked, result.frames_checked
        ),
        PropertyBmcStatus::AssumptionsUnsat => format!(
            "Temporal BMC report '{}': ASSUMPTIONS UNSAT (vacuous result), {} frame(s)\n",
            result.assertion, result.frames_checked
        ),
    }
}
