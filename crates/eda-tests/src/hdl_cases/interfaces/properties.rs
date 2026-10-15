//! Concurrent and temporal interface-property regression cases.

use crate::CaseList;
use eda_core::logic::LogicWord;
use eda_hdl::compile;
use eda_sim::Simulator;

pub(super) fn register(c: &mut CaseList) {
    concurrent_property_structure(c);
    nonoverlapping_property_structure(c);
    property_kind_structure(c);
    fixed_delay_property_structure(c);
    delay_range_property_structure(c);
    consecutive_repetition_property_structure(c);
    repetition_range_property_structure(c);
    goto_repetition_property_structure(c);
    goto_repetition_range_property_structure(c);
    nonconsecutive_repetition_property_structure(c);
    disable_property_structure(c);
    sequence_composition_property_structure(c);
    multilength_sequence_composition_property_structure(c);
}

fn concurrent_property_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_concurrent_property_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let selector = (index % 2) as u64;
            let should_fail = index % 4 == 3;
            let use_negedge = index % 4 == 1;
            let req = if should_fail || index % 4 != 2 {
                1
            } else {
                selector
            };
            let ready = !should_fail as u64;
            let range = format!("{}:0", width - 1);
            let clock_edge = if use_negedge { "negedge" } else { "posedge" };
            let antecedent = match index % 4 {
                2 => "selector || req".to_string(),
                _ => "req".to_string(),
            };
            let source = format!(
                "module top(input logic clk, req, ready, selector, \
                 output logic [{range}] grant); \
                 assign grant = req & ready ? {width}'d{mask} : {width}'d0; \
                 assert property (@({clock_edge} clk) {antecedent} |-> \
                 grant == {width}'d{mask}) else $error(\"property {index}\"); \
                 endmodule",
                range = range,
                clock_edge = clock_edge,
                antecedent = antecedent,
                width = width,
                mask = mask,
                index = index
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.processes.len() != 1 {
                return Err(format!("process count {}", design.processes.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            let initial_clock = u64::from(use_negedge);
            let final_clock = 1 - initial_clock;
            simulator.set_input("clk", &LogicWord::from_u64(1, initial_clock))?;
            simulator.set_input("req", &LogicWord::from_u64(1, req))?;
            simulator.set_input("ready", &LogicWord::from_u64(1, ready))?;
            simulator.set_input("selector", &LogicWord::from_u64(1, selector))?;
            let property_result = simulator.set_input("clk", &LogicWord::from_u64(1, final_clock));
            if should_fail {
                let error = property_result.err().ok_or("expected property failure")?;
                if !error.contains("assertion failed") || !error.contains(&index.to_string()) {
                    return Err(format!("unexpected property error: {error}"));
                }
            } else {
                property_result.map_err(|error| format!("unexpected property error: {error}"))?;
            }
            let grant = simulator
                .read("grant")
                .and_then(|word| word.to_u64())
                .ok_or("property-case grant is unknown")?;
            let expected_grant = if req != 0 && ready != 0 { mask } else { 0 };
            if grant != expected_grant {
                return Err(format!(
                    "req={req} ready={ready} grant={grant}, expected={expected_grant}"
                ));
            }
            Ok(())
        });
    }
}

fn nonoverlapping_property_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_property_next_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let expected = (index as u64 * 51 + 7) & mask;
            let mismatch = expected ^ (1 + index as u64 % 7);
            let should_fail = index % 4 == 2;
            let use_negedge = index % 2 == 1;
            let force_antecedent = index % 4 == 3;
            let range = format!("{}:0", width - 1);
            let clock_edge = if use_negedge { "negedge" } else { "posedge" };
            let antecedent = if force_antecedent {
                "trigger || force_sample"
            } else {
                "trigger"
            };
            let source = format!(
                "module top(input logic clk, trigger, force_sample, \
                 input logic [{range}] actual, expected); \
                 assert property (@({clock_edge} clk) {antecedent} |=> \
                 actual == expected) else $error(\"next property {index}\"); \
                 endmodule",
                range = range,
                clock_edge = clock_edge,
                antecedent = antecedent,
                index = index
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.processes.len() != 1 {
                return Err(format!("process count {}", design.processes.len()));
            }
            if !design
                .net_decls
                .iter()
                .any(|declaration| declaration.name.starts_with("__property_pending_"))
            {
                return Err("missing non-overlap pending state".to_string());
            }

            let inactive = u64::from(use_negedge);
            let active = 1 - inactive;
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
            simulator.set_input("trigger", &LogicWord::from_u64(1, 1))?;
            simulator.set_input(
                "force_sample",
                &LogicWord::from_u64(1, u64::from(force_antecedent)),
            )?;
            simulator.set_input("actual", &LogicWord::from_u64(width, mismatch))?;
            simulator.set_input("expected", &LogicWord::from_u64(width, expected))?;
            simulator.set_input("clk", &LogicWord::from_u64(1, active))?;

            simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
            simulator.set_input("trigger", &LogicWord::from_u64(1, 0))?;
            simulator.set_input("force_sample", &LogicWord::from_u64(1, 0))?;
            let second_actual = if should_fail { mismatch } else { expected };
            simulator.set_input("actual", &LogicWord::from_u64(width, second_actual))?;
            let result = simulator.set_input("clk", &LogicWord::from_u64(1, active));
            if should_fail {
                let error = result.err().ok_or("expected next-sample failure")?;
                if !error.contains("assertion failed") || !error.contains(&index.to_string()) {
                    return Err(format!("unexpected next-property error: {error}"));
                }
            } else {
                result.map_err(|error| format!("unexpected next-property error: {error}"))?;
            }
            Ok(())
        });
    }
}

fn property_kind_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_property_kind_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let expected = (index as u64 * 57 + 9) & mask;
            let mismatch = expected ^ 1;
            let variant = index % 4;
            let is_cover = variant >= 2;
            let should_fail = variant == 1;
            let should_hit = variant == 2;
            let use_negedge = index % 2 == 1;
            let range = format!("{}:0", width - 1);
            let clock_edge = if use_negedge { "negedge" } else { "posedge" };
            let source = if is_cover {
                format!(
                    "module top(input logic clk, req, \
                     input logic [{range}] actual, expected); \
                     sampled_value: cover property (@({clock_edge} clk) req |-> \
                     actual == expected); \
                     endmodule",
                    range = range,
                    clock_edge = clock_edge
                )
            } else {
                format!(
                    "module top(input logic clk, req, \
                     input logic [{range}] actual, expected); \
                     assume property (@({clock_edge} clk) req |-> \
                     actual == expected) else $error(\"assumption {index}\"); \
                     endmodule",
                    range = range,
                    clock_edge = clock_edge,
                    index = index
                )
            };
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.processes.len() != 1 {
                return Err(format!("process count {}", design.processes.len()));
            }
            let inactive = u64::from(use_negedge);
            let active = 1 - inactive;
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
            simulator.set_input("req", &LogicWord::from_u64(1, 1))?;
            let actual = if (is_cover && !should_hit) || should_fail {
                mismatch
            } else {
                expected
            };
            simulator.set_input("actual", &LogicWord::from_u64(width, actual))?;
            simulator.set_input("expected", &LogicWord::from_u64(width, expected))?;
            let result = simulator.set_input("clk", &LogicWord::from_u64(1, active));

            if is_cover {
                result.map_err(|error| format!("cover property errored: {error}"))?;
                let hits = simulator.property_cover_hits("sampled_value");
                let expected_hits = u64::from(should_hit);
                if hits != expected_hits {
                    return Err(format!("cover hits={hits}, expected={expected_hits}"));
                }
            } else if should_fail {
                let error = result.err().ok_or("expected assumption failure")?;
                if !error.starts_with("assumption failed") || !error.contains(&index.to_string()) {
                    return Err(format!("unexpected assumption error: {error}"));
                }
            } else {
                result.map_err(|error| format!("unexpected assumption error: {error}"))?;
            }
            Ok(())
        });
    }
}

fn fixed_delay_property_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_property_delay_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let expected = (index as u64 * 61 + 12) & mask;
            let mismatch = expected ^ 1;
            let delay = index % 8;
            let variant = index % 5;
            let is_cover = variant >= 3;
            let is_assume = variant == 2;
            let should_fail = matches!(variant, 1 | 2);
            let should_hit = variant == 3;
            let use_negedge = index % 2 == 1;
            let range = format!("{}:0", width - 1);
            let clock_edge = if use_negedge { "negedge" } else { "posedge" };
            let property = if is_cover {
                format!(
                    "delayed_value: cover property (@({clock_edge} clk) req |-> \
                     ##[{delay}] actual == expected);",
                    clock_edge = clock_edge,
                    delay = delay
                )
            } else {
                format!(
                    "{kind} property (@({clock_edge} clk) req |-> \
                     ##[{delay}] actual == expected) else $error(\"delayed {index}\");",
                    kind = if is_assume { "assume" } else { "assert" },
                    clock_edge = clock_edge,
                    delay = delay,
                    index = index
                )
            };
            let source = format!(
                "module top(input logic clk, req, \
                 input logic [{range}] actual, expected); \
                 {property} \
                 endmodule",
                range = range,
                property = property
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.processes.len() != 1 {
                return Err(format!("process count {}", design.processes.len()));
            }
            let pending_count = design
                .net_decls
                .iter()
                .filter(|declaration| declaration.name.starts_with("__property_pending_"))
                .count();
            if pending_count != delay {
                return Err(format!(
                    "pending stage count {pending_count}, expected {delay}"
                ));
            }

            let inactive = u64::from(use_negedge);
            let active = 1 - inactive;
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
            simulator.set_input("req", &LogicWord::from_u64(1, 1))?;
            let initial_actual =
                if delay == 0 && ((!is_cover && !should_fail) || (is_cover && should_hit)) {
                    expected
                } else {
                    mismatch
                };
            simulator.set_input("actual", &LogicWord::from_u64(width, initial_actual))?;
            simulator.set_input("expected", &LogicWord::from_u64(width, expected))?;
            let mut result = simulator.set_input("clk", &LogicWord::from_u64(1, active));

            for edge in 1..=delay {
                simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                simulator.set_input("req", &LogicWord::from_u64(1, 0))?;
                let final_edge = edge == delay;
                let actual =
                    if final_edge && ((!is_cover && !should_fail) || (is_cover && should_hit)) {
                        expected
                    } else {
                        mismatch
                    };
                simulator.set_input("actual", &LogicWord::from_u64(width, actual))?;
                result = simulator.set_input("clk", &LogicWord::from_u64(1, active));
            }

            if is_cover {
                result.map_err(|error| format!("delayed cover property errored: {error}"))?;
                let hits = simulator.property_cover_hits("delayed_value");
                let expected_hits = u64::from(should_hit);
                if hits != expected_hits {
                    return Err(format!("cover hits={hits}, expected={expected_hits}"));
                }
            } else if should_fail {
                let error = result.err().ok_or("expected delayed property failure")?;
                let expected_prefix = if is_assume {
                    "assumption failed"
                } else {
                    "assertion failed"
                };
                if !error.starts_with(expected_prefix) || !error.contains(&index.to_string()) {
                    return Err(format!("unexpected delayed property error: {error}"));
                }
            } else {
                result.map_err(|error| format!("unexpected delayed property error: {error}"))?;
            }
            Ok(())
        });
    }
}

fn delay_range_property_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_property_delay_range_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let expected = (index as u64 * 67 + 14) & mask;
            let mismatch = expected ^ 1;
            let start = index % 3;
            let end = start + 1 + index % 3;
            let variant = index % 5;
            let is_cover = variant >= 3;
            let is_assume = variant == 2;
            let should_fail = variant == 2;
            let should_hit = variant == 3;
            let hit_delay = if variant == 1 { end } else { start };
            let use_negedge = index % 2 == 1;
            let range = format!("{}:0", width - 1);
            let clock_edge = if use_negedge { "negedge" } else { "posedge" };
            let property = if is_cover {
                format!(
                    "range_value: cover property (@({clock_edge} clk) req |-> \
                     ##[{start}:{end}] actual == expected);",
                    clock_edge = clock_edge,
                    start = start,
                    end = end
                )
            } else {
                format!(
                    "{kind} property (@({clock_edge} clk) req |-> \
                     ##[{start}:{end}] actual == expected) \
                     else $error(\"range {index}\");",
                    kind = if is_assume { "assume" } else { "assert" },
                    clock_edge = clock_edge,
                    start = start,
                    end = end,
                    index = index
                )
            };
            let source = format!(
                "module top(input logic clk, req, \
                 input logic [{range}] actual, expected); \
                 {property} \
                 endmodule",
                range = range,
                property = property
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.processes.len() != 1 {
                return Err(format!("process count {}", design.processes.len()));
            }
            let pending_count = design
                .net_decls
                .iter()
                .filter(|declaration| declaration.name.starts_with("__property_pending_"))
                .count();
            if pending_count != end {
                return Err(format!(
                    "pending stage count {pending_count}, expected {end}"
                ));
            }

            let inactive = u64::from(use_negedge);
            let active = 1 - inactive;
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
            simulator.set_input("req", &LogicWord::from_u64(1, 1))?;
            let initial_hit = hit_delay == 0 && (should_hit || (!is_cover && !should_fail));
            simulator.set_input(
                "actual",
                &LogicWord::from_u64(width, if initial_hit { expected } else { mismatch }),
            )?;
            simulator.set_input("expected", &LogicWord::from_u64(width, expected))?;
            simulator.set_input("clk", &LogicWord::from_u64(1, active))?;

            let mut result = Ok(());
            for edge in 1..=end {
                simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                simulator.set_input("req", &LogicWord::from_u64(1, 0))?;
                let hit = edge == hit_delay && (should_hit || (!is_cover && !should_fail));
                simulator.set_input(
                    "actual",
                    &LogicWord::from_u64(width, if hit { expected } else { mismatch }),
                )?;
                result = simulator.set_input("clk", &LogicWord::from_u64(1, active));
                if result.is_err() {
                    break;
                }
            }

            if is_cover {
                result.map_err(|error| format!("range cover property errored: {error}"))?;
                let hits = simulator.property_cover_hits("range_value");
                let expected_hits = u64::from(should_hit);
                if hits != expected_hits {
                    return Err(format!("cover hits={hits}, expected={expected_hits}"));
                }
            } else if should_fail {
                let error = result.err().ok_or("expected range property failure")?;
                let expected_prefix = if is_assume {
                    "assumption failed"
                } else {
                    "assertion failed"
                };
                if !error.starts_with(expected_prefix) || !error.contains(&index.to_string()) {
                    return Err(format!("unexpected range property error: {error}"));
                }
            } else {
                result.map_err(|error| format!("unexpected range property error: {error}"))?;
            }
            Ok(())
        });
    }
}

fn consecutive_repetition_property_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_property_repeat_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let expected = (index as u64 * 71 + 16) & mask;
            let mismatch = expected ^ 1;
            let repetition = 1 + index % 5;
            let variant = index % 5;
            let is_cover = variant >= 3;
            let is_assume = variant == 2;
            let should_fail = matches!(variant, 1 | 2);
            let should_hit = variant == 3;
            let use_negedge = index % 2 == 1;
            let range = format!("{}:0", width - 1);
            let clock_edge = if use_negedge { "negedge" } else { "posedge" };
            let property = if is_cover {
                format!(
                    "repeated_value: cover property (@({clock_edge} clk) req |-> \
                     actual == expected[*{repetition}]);",
                    clock_edge = clock_edge,
                    repetition = repetition
                )
            } else {
                format!(
                    "{kind} property (@({clock_edge} clk) req |-> \
                     actual == expected[*{repetition}]) \
                     else $error(\"repetition {index}\");",
                    kind = if is_assume { "assume" } else { "assert" },
                    clock_edge = clock_edge,
                    repetition = repetition,
                    index = index
                )
            };
            let source = format!(
                "module top(input logic clk, req, \
                 input logic [{range}] actual, expected); \
                 {property} \
                 endmodule",
                range = range,
                property = property
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.processes.len() != 1 {
                return Err(format!("process count {}", design.processes.len()));
            }
            let repeat_stage_count = design
                .net_decls
                .iter()
                .filter(|declaration| declaration.name.starts_with("__property_repeat_"))
                .count();
            if repeat_stage_count != repetition.saturating_sub(1) {
                return Err(format!(
                    "repeat stage count {repeat_stage_count}, expected {}",
                    repetition.saturating_sub(1)
                ));
            }

            let inactive = u64::from(use_negedge);
            let active = 1 - inactive;
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
            simulator.set_input("req", &LogicWord::from_u64(1, 1))?;
            let initial_match = match variant {
                1 | 2 if repetition == 1 => false,
                4 => false,
                _ => true,
            };
            simulator.set_input(
                "actual",
                &LogicWord::from_u64(width, if initial_match { expected } else { mismatch }),
            )?;
            simulator.set_input("expected", &LogicWord::from_u64(width, expected))?;
            let mut result = simulator.set_input("clk", &LogicWord::from_u64(1, active));

            for sample in 1..repetition {
                if result.is_err() {
                    break;
                }
                simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                simulator.set_input("req", &LogicWord::from_u64(1, 0))?;
                let keep_match = !(should_fail
                    && ((variant == 1 && sample == repetition - 1)
                        || (variant == 2 && sample == 1)));
                simulator.set_input(
                    "actual",
                    &LogicWord::from_u64(width, if keep_match { expected } else { mismatch }),
                )?;
                result = simulator.set_input("clk", &LogicWord::from_u64(1, active));
            }

            if is_cover {
                result.map_err(|error| format!("repetition cover property errored: {error}"))?;
                let hits = simulator.property_cover_hits("repeated_value");
                let expected_hits = u64::from(should_hit);
                if hits != expected_hits {
                    return Err(format!("cover hits={hits}, expected={expected_hits}"));
                }
            } else if should_fail {
                let error = result.err().ok_or("expected repetition property failure")?;
                let expected_prefix = if is_assume {
                    "assumption failed"
                } else {
                    "assertion failed"
                };
                if !error.starts_with(expected_prefix) || !error.contains(&index.to_string()) {
                    return Err(format!("unexpected repetition error: {error}"));
                }
            } else {
                result.map_err(|error| format!("unexpected repetition error: {error}"))?;
            }
            Ok(())
        });
    }
}

fn repetition_range_property_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_property_repeat_range_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let expected = (index as u64 * 73 + 18) & mask;
            let mismatch = expected ^ 1;
            let minimum = 1 + index % 4;
            let maximum = minimum + 1 + index % 3;
            let variant = index % 5;
            let is_cover = variant >= 3;
            let is_assume = variant == 2;
            let should_fail = matches!(variant, 1 | 2);
            let should_hit = variant == 3;
            let use_negedge = index % 2 == 1;
            let range = format!("{}:0", width - 1);
            let clock_edge = if use_negedge { "negedge" } else { "posedge" };
            let property = if is_cover {
                format!(
                    "range_repeat_value: cover property (@({clock_edge} clk) req |-> \
                     actual == expected[*{minimum}:{maximum}]);",
                    clock_edge = clock_edge,
                    minimum = minimum,
                    maximum = maximum
                )
            } else {
                format!(
                    "{kind} property (@({clock_edge} clk) req |-> \
                     actual == expected[*{minimum}:{maximum}]) \
                     else $error(\"repeat range {index}\");",
                    kind = if is_assume { "assume" } else { "assert" },
                    clock_edge = clock_edge,
                    minimum = minimum,
                    maximum = maximum,
                    index = index
                )
            };
            let source = format!(
                "module top(input logic clk, req, \
                 input logic [{range}] actual, expected); \
                 {property} \
                 endmodule",
                range = range,
                property = property
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.processes.len() != 1 {
                return Err(format!("process count {}", design.processes.len()));
            }
            let repeat_stage_count = design
                .net_decls
                .iter()
                .filter(|declaration| declaration.name.starts_with("__property_repeat_"))
                .count();
            if repeat_stage_count != minimum - 1 {
                return Err(format!(
                    "repeat stage count {repeat_stage_count}, expected {}",
                    minimum - 1
                ));
            }

            let inactive = u64::from(use_negedge);
            let active = 1 - inactive;
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
            simulator.set_input("req", &LogicWord::from_u64(1, 1))?;
            let initial_match = if is_cover {
                should_hit
            } else if should_fail {
                minimum > 1
            } else {
                true
            };
            simulator.set_input(
                "actual",
                &LogicWord::from_u64(width, if initial_match { expected } else { mismatch }),
            )?;
            simulator.set_input("expected", &LogicWord::from_u64(width, expected))?;
            let mut result = simulator.set_input("clk", &LogicWord::from_u64(1, active));

            let last_sample = if is_cover && !should_hit {
                maximum
            } else if should_fail {
                if is_assume {
                    minimum
                        .saturating_sub(2)
                        .max(if minimum == 1 { 0 } else { 1 })
                } else {
                    minimum - 1
                }
            } else {
                maximum
            };
            for sample in 1..=last_sample {
                if result.is_err() {
                    break;
                }
                simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                simulator.set_input("req", &LogicWord::from_u64(1, 0))?;
                let keep_match = if is_cover {
                    should_hit && sample < minimum
                } else if should_fail {
                    if is_assume {
                        sample < if minimum == 1 { 0 } else { 1 }
                    } else {
                        sample < minimum - 1
                    }
                } else {
                    sample < minimum
                };
                simulator.set_input(
                    "actual",
                    &LogicWord::from_u64(width, if keep_match { expected } else { mismatch }),
                )?;
                result = simulator.set_input("clk", &LogicWord::from_u64(1, active));
            }

            if is_cover {
                result.map_err(|error| format!("range repetition cover errored: {error}"))?;
                let hits = simulator.property_cover_hits("range_repeat_value");
                let expected_hits = u64::from(should_hit);
                if hits != expected_hits {
                    return Err(format!("cover hits={hits}, expected={expected_hits}"));
                }
            } else if should_fail {
                let error = result.err().ok_or("expected range repetition failure")?;
                let expected_prefix = if is_assume {
                    "assumption failed"
                } else {
                    "assertion failed"
                };
                if !error.starts_with(expected_prefix) || !error.contains(&index.to_string()) {
                    return Err(format!("unexpected range repetition error: {error}"));
                }
            } else {
                result.map_err(|error| format!("unexpected range repetition error: {error}"))?;
            }
            Ok(())
        });
    }
}

fn goto_repetition_property_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_property_goto_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let expected = (index as u64 * 89 + 27) & mask;
            let mismatch = expected ^ 1;
            let variant = index % 5;
            let repetition = if variant == 4 { 1 } else { 1 + index % 5 };
            let is_cover = matches!(variant, 2 | 3);
            let is_assume = variant == 1;
            let should_hit = variant != 3;
            let should_fail = variant == 4;
            let use_negedge = index % 2 == 1;
            let range = format!("{}:0", width - 1);
            let clock_edge = if use_negedge { "negedge" } else { "posedge" };
            let property = if is_cover {
                format!(
                    "goto_value: cover property (@({clock_edge} clk) req |-> \
                     actual == expected[->{repetition}]);",
                    clock_edge = clock_edge,
                    repetition = repetition
                )
            } else {
                format!(
                    "{kind} property (@({clock_edge} clk) req |-> \
                     actual == expected[->{repetition}]) \
                     else $error(\"goto {index}\");",
                    kind = if is_assume { "assume" } else { "assert" },
                    clock_edge = clock_edge,
                    repetition = repetition,
                    index = index
                )
            };
            let source = format!(
                "module top(input logic clk, req, \
                 input logic [{range}] actual, expected); \
                 {property} \
                 endmodule",
                range = range,
                property = property
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.processes.len() != 1 {
                return Err(format!("process count {}", design.processes.len()));
            }
            let goto_stage_count = design
                .net_decls
                .iter()
                .filter(|declaration| declaration.name.starts_with("__property_goto_"))
                .count();
            if goto_stage_count != repetition.saturating_sub(1) {
                return Err(format!(
                    "goto stage count {goto_stage_count}, expected {}",
                    repetition.saturating_sub(1)
                ));
            }

            let inactive = u64::from(use_negedge);
            let active = 1 - inactive;
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
            simulator.set_input("req", &LogicWord::from_u64(1, 1))?;
            simulator.set_input(
                "actual",
                &LogicWord::from_u64(
                    width,
                    if should_fail {
                        mismatch
                    } else if should_hit {
                        expected
                    } else {
                        mismatch
                    },
                ),
            )?;
            simulator.set_input("expected", &LogicWord::from_u64(width, expected))?;
            let mut result = simulator.set_input("clk", &LogicWord::from_u64(1, active));

            // A hit pattern places repetition true samples at even sample
            // offsets and false samples between them.  A miss pattern stops
            // one nonconsecutive occurrence early.
            let samples = if should_hit {
                2 * (repetition - 1)
            } else {
                2 * repetition
            };
            for sample in 1..=samples {
                if result.is_err() {
                    break;
                }
                simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                simulator.set_input("req", &LogicWord::from_u64(1, 0))?;
                let hit = sample % 2 == 0 && should_hit && sample / 2 < repetition;
                simulator.set_input(
                    "actual",
                    &LogicWord::from_u64(width, if hit { expected } else { mismatch }),
                )?;
                result = simulator.set_input("clk", &LogicWord::from_u64(1, active));
            }

            if is_cover {
                result.map_err(|error| format!("goto cover property errored: {error}"))?;
                let hits = simulator.property_cover_hits("goto_value");
                let expected_hits = u64::from(should_hit);
                if hits != expected_hits {
                    return Err(format!("cover hits={hits}, expected={expected_hits}"));
                }
            } else if should_fail {
                let error = result.err().ok_or("expected goto property failure")?;
                if !error.contains(&index.to_string()) {
                    return Err(format!("unexpected goto error: {error}"));
                }
            } else {
                result.map_err(|error| format!("unexpected goto error: {error}"))?;
            }
            Ok(())
        });
    }
}

fn goto_repetition_range_property_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_property_goto_range_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let expected = (index as u64 * 97 + 28) & mask;
            let mismatch = expected ^ 1;
            let minimum = if index % 5 == 4 { 1 } else { 1 + index % 5 };
            let maximum = minimum + 1 + index % 3;
            let variant = index % 5;
            let is_cover = matches!(variant, 2 | 3);
            let is_assume = variant == 1;
            let should_hit = variant != 3;
            let should_fail = variant == 4;
            let use_negedge = index % 2 == 1;
            let range = format!("{}:0", width - 1);
            let clock_edge = if use_negedge { "negedge" } else { "posedge" };
            let property = if is_cover {
                format!(
                    "goto_range_value: cover property (@({clock_edge} clk) req |-> \
                     actual == expected[->{minimum}:{maximum}]);",
                    clock_edge = clock_edge,
                    minimum = minimum,
                    maximum = maximum
                )
            } else {
                format!(
                    "{kind} property (@({clock_edge} clk) req |-> \
                     actual == expected[->{minimum}:{maximum}]) \
                     else $error(\"goto range {index}\");",
                    kind = if is_assume { "assume" } else { "assert" },
                    clock_edge = clock_edge,
                    minimum = minimum,
                    maximum = maximum,
                    index = index
                )
            };
            let source = format!(
                "module top(input logic clk, req, \
                 input logic [{range}] actual, expected); \
                 {property} \
                 endmodule",
                range = range,
                property = property
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.processes.len() != 1 {
                return Err(format!("process count {}", design.processes.len()));
            }
            let stage_count = design
                .net_decls
                .iter()
                .filter(|declaration| declaration.name.starts_with("__property_goto_"))
                .count();
            if stage_count != minimum.saturating_sub(1) {
                return Err(format!(
                    "goto range stage count {stage_count}, expected {}",
                    minimum.saturating_sub(1)
                ));
            }

            let inactive = u64::from(use_negedge);
            let active = 1 - inactive;
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
            simulator.set_input("req", &LogicWord::from_u64(1, 1))?;
            simulator.set_input(
                "actual",
                &LogicWord::from_u64(
                    width,
                    if should_fail {
                        mismatch
                    } else if should_hit {
                        expected
                    } else {
                        mismatch
                    },
                ),
            )?;
            simulator.set_input("expected", &LogicWord::from_u64(width, expected))?;
            let mut result = simulator.set_input("clk", &LogicWord::from_u64(1, active));

            let samples = if should_hit {
                2 * (minimum - 1) + 1
            } else {
                2 * minimum
            };
            for sample in 1..=samples {
                if result.is_err() {
                    break;
                }
                simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                simulator.set_input("req", &LogicWord::from_u64(1, 0))?;
                let hit = sample % 2 == 0 && should_hit && sample / 2 < minimum;
                simulator.set_input(
                    "actual",
                    &LogicWord::from_u64(width, if hit { expected } else { mismatch }),
                )?;
                result = simulator.set_input("clk", &LogicWord::from_u64(1, active));
            }

            if is_cover {
                result.map_err(|error| format!("goto range cover errored: {error}"))?;
                let hits = simulator.property_cover_hits("goto_range_value");
                let expected_hits = u64::from(should_hit);
                if hits != expected_hits {
                    return Err(format!("cover hits={hits}, expected={expected_hits}"));
                }
            } else if should_fail {
                let error = result.err().ok_or("expected goto range failure")?;
                if !error.contains(&index.to_string()) {
                    return Err(format!("unexpected goto range error: {error}"));
                }
            } else {
                result.map_err(|error| format!("unexpected goto range error: {error}"))?;
            }
            Ok(())
        });
    }
}

fn nonconsecutive_repetition_property_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(
            format!("sv_property_nonconsecutive_{index:03}"),
            move || {
                let width = 1 + index % 16;
                let mask = (1_u64 << width) - 1;
                let expected = (index as u64 * 101 + 29) & mask;
                let mismatch = expected ^ 1;
                let minimum = if index % 5 == 4 { 1 } else { 1 + index % 5 };
                let maximum = minimum + 1 + index % 4;
                let variant = index % 5;
                let is_cover = matches!(variant, 2 | 3);
                let is_assume = variant == 1;
                let should_hit = variant != 3;
                let should_fail = variant == 4;
                let use_negedge = index % 2 == 1;
                let range = format!("{}:0", width - 1);
                let clock_edge = if use_negedge { "negedge" } else { "posedge" };
                let repetition = if index % 2 == 0 {
                    format!("[={minimum}]")
                } else {
                    format!("[={minimum}:{maximum}]")
                };
                let property = if is_cover {
                    format!(
                        "nonconsecutive_value: cover property (@({clock_edge} clk) req |-> \
                     actual == expected{repetition});",
                        clock_edge = clock_edge,
                        repetition = repetition
                    )
                } else {
                    format!(
                        "{kind} property (@({clock_edge} clk) req |-> \
                     actual == expected{repetition}) \
                     else $error(\"nonconsecutive {index}\");",
                        kind = if is_assume { "assume" } else { "assert" },
                        clock_edge = clock_edge,
                        repetition = repetition,
                        index = index
                    )
                };
                let source = format!(
                    "module top(input logic clk, req, \
                 input logic [{range}] actual, expected); \
                 {property} \
                 endmodule",
                    range = range,
                    property = property
                );
                let design = compile(&source, Some("top"))
                    .map_err(|error| format!("compile error: {error}"))?;
                if design.processes.len() != 1 {
                    return Err(format!("process count {}", design.processes.len()));
                }
                let stage_count = design
                    .net_decls
                    .iter()
                    .filter(|declaration| declaration.name.starts_with("__property_goto_"))
                    .count();
                if stage_count != minimum.saturating_sub(1) {
                    return Err(format!(
                        "nonconsecutive stage count {stage_count}, expected {}",
                        minimum.saturating_sub(1)
                    ));
                }

                let inactive = u64::from(use_negedge);
                let active = 1 - inactive;
                let mut simulator = Simulator::new(&design)?;
                simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                simulator.set_input("req", &LogicWord::from_u64(1, 1))?;
                simulator.set_input(
                    "actual",
                    &LogicWord::from_u64(
                        width,
                        if should_fail || !should_hit {
                            mismatch
                        } else {
                            expected
                        },
                    ),
                )?;
                simulator.set_input("expected", &LogicWord::from_u64(width, expected))?;
                let mut result = simulator.set_input("clk", &LogicWord::from_u64(1, active));
                let samples = if should_hit {
                    2 * (minimum - 1) + 3
                } else {
                    2 * minimum
                };
                for sample in 1..=samples {
                    if result.is_err() {
                        break;
                    }
                    simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                    simulator.set_input("req", &LogicWord::from_u64(1, 0))?;
                    let hit = sample % 2 == 0 && should_hit && sample / 2 < minimum;
                    simulator.set_input(
                        "actual",
                        &LogicWord::from_u64(width, if hit { expected } else { mismatch }),
                    )?;
                    result = simulator.set_input("clk", &LogicWord::from_u64(1, active));
                }
                if is_cover {
                    result.map_err(|error| format!("nonconsecutive cover errored: {error}"))?;
                    let hits = simulator.property_cover_hits("nonconsecutive_value");
                    let expected_hits = u64::from(should_hit);
                    if hits != expected_hits {
                        return Err(format!("cover hits={hits}, expected={expected_hits}"));
                    }
                } else if should_fail {
                    let error = result.err().ok_or("expected nonconsecutive failure")?;
                    if !error.contains(&index.to_string()) {
                        return Err(format!("unexpected nonconsecutive error: {error}"));
                    }
                } else {
                    result.map_err(|error| format!("unexpected nonconsecutive error: {error}"))?;
                }
                Ok(())
            },
        );
    }
}

fn disable_property_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_property_disable_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let expected = (index as u64 * 83 + 22) & mask;
            let mismatch = expected ^ 1;
            let form = index % 5;
            let is_cover = form == 4;
            let is_assume = form == 1;
            let use_negedge = index % 2 == 1;
            let range = format!("{}:0", width - 1);
            let clock_edge = if use_negedge { "negedge" } else { "posedge" };
            let sequence = match form {
                0 => "actual == expected".to_string(),
                1 => "actual == expected".to_string(),
                2 => "##[2] actual == expected".to_string(),
                3 => "actual == expected[*2]".to_string(),
                _ => "##[2] actual == expected".to_string(),
            };
            let implication = if form == 1 { "|=>" } else { "|->" };
            let property = if is_cover {
                format!(
                    "disabled_value: cover property (@({clock_edge} clk) disable iff (gate) \
                     req {implication} {sequence});",
                    clock_edge = clock_edge,
                    implication = implication,
                    sequence = sequence
                )
            } else {
                format!(
                    "{kind} property (@({clock_edge} clk) disable iff (gate) \
                     req {implication} {sequence}) else $error(\"disabled {index}\");",
                    kind = if is_assume { "assume" } else { "assert" },
                    clock_edge = clock_edge,
                    implication = implication,
                    sequence = sequence,
                    index = index
                )
            };
            let source = format!(
                "module top(input logic clk, gate, req, \
                 input logic [{range}] actual, expected); \
                 {property} \
                 endmodule",
                range = range,
                property = property
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.processes.len() != 1 {
                return Err(format!("process count {}", design.processes.len()));
            }

            let inactive = u64::from(use_negedge);
            let active = 1 - inactive;
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
            simulator.set_input("gate", &LogicWord::from_u64(1, u64::from(form == 0)))?;
            simulator.set_input("req", &LogicWord::from_u64(1, 1))?;
            if form == 3 {
                simulator.set_input("actual", &LogicWord::from_u64(width, expected))?;
            } else {
                simulator.set_input("actual", &LogicWord::from_u64(width, mismatch))?;
            }
            simulator.set_input("expected", &LogicWord::from_u64(width, expected))?;
            simulator.set_input("clk", &LogicWord::from_u64(1, active))?;

            // Reach the edge at which the first attempt would fail or hit,
            // but keep `disable iff` asserted so the attempt is discarded.
            match form {
                0 => {
                    simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                    simulator.set_input("req", &LogicWord::from_u64(1, 0))?;
                    simulator.set_input("gate", &LogicWord::from_u64(1, 1))?;
                    simulator.set_input("clk", &LogicWord::from_u64(1, active))?;
                }
                1 | 2 | 4 => {
                    let disable_edge = if form == 1 { 1 } else { 2 };
                    for edge in 1..=2 {
                        simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                        simulator.set_input("req", &LogicWord::from_u64(1, 0))?;
                        simulator.set_input(
                            "gate",
                            &LogicWord::from_u64(1, u64::from(edge == disable_edge)),
                        )?;
                        simulator.set_input("actual", &LogicWord::from_u64(width, mismatch))?;
                        simulator.set_input("clk", &LogicWord::from_u64(1, active))?;
                    }
                }
                _ => {
                    simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                    simulator.set_input("req", &LogicWord::from_u64(1, 0))?;
                    simulator.set_input("gate", &LogicWord::from_u64(1, 1))?;
                    simulator.set_input("actual", &LogicWord::from_u64(width, mismatch))?;
                    simulator.set_input("clk", &LogicWord::from_u64(1, active))?;
                }
            }

            // Start a fresh enabled attempt.  Without the prior state clear,
            // the disabled attempt would still be pending and could fire late.
            simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
            simulator.set_input("gate", &LogicWord::from_u64(1, 0))?;
            simulator.set_input("req", &LogicWord::from_u64(1, 1))?;
            simulator.set_input("actual", &LogicWord::from_u64(width, expected))?;
            simulator.set_input("clk", &LogicWord::from_u64(1, active))?;

            let mut result = Ok(());
            match form {
                0 => {}
                1 => {
                    simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                    simulator.set_input("req", &LogicWord::from_u64(1, 0))?;
                    simulator.set_input("clk", &LogicWord::from_u64(1, active))?;
                }
                2 | 4 => {
                    for _ in 0..2 {
                        simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                        simulator.set_input("req", &LogicWord::from_u64(1, 0))?;
                        simulator.set_input("clk", &LogicWord::from_u64(1, active))?;
                    }
                    result = simulator.set_input("clk", &LogicWord::from_u64(1, active));
                }
                _ => {
                    simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                    simulator.set_input("req", &LogicWord::from_u64(1, 0))?;
                    simulator.set_input("clk", &LogicWord::from_u64(1, active))?;
                }
            }

            if is_cover {
                result.map_err(|error| format!("disabled cover property errored: {error}"))?;
                let hits = simulator.property_cover_hits("disabled_value");
                if hits != 1 {
                    return Err(format!("cover hits={hits}, expected=1"));
                }
            } else {
                result.map_err(|error| {
                    format!("disabled property fired after re-enabling: {error}")
                })?;
            }
            Ok(())
        });
    }
}
fn sequence_composition_property_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_property_composition_{index:03}"), move || {
            let width = 1 + index % 8;
            let mask = (1_u64 << width) - 1;
            let expected_first = (index as u64 * 67 + 11) & mask;
            let expected_second = (index as u64 * 41 + 23) & mask;
            let mismatch_first = expected_first ^ 1;
            let mismatch_second = expected_second ^ 1;
            let is_and = index % 2 == 0;
            let operator = if is_and { "and" } else { "or" };
            let use_next = index % 3 == 2;
            let delay = if use_next { 1 } else { index % 4 };
            let variant = index % 5;
            let is_cover = variant >= 3;
            let is_assume = variant == 2;
            let should_succeed = matches!(variant, 0 | 3);
            let use_negedge = index % 4 == 1;
            let clock_edge = if use_negedge { "negedge" } else { "posedge" };
            let range = format!("{}:0", width - 1);
            let (final_first, final_second) = if is_and {
                (
                    expected_first,
                    if should_succeed {
                        expected_second
                    } else {
                        mismatch_second
                    },
                )
            } else if should_succeed {
                (expected_first, mismatch_second)
            } else {
                (mismatch_first, mismatch_second)
            };
            let property = if is_cover {
                format!(
                    "composed: cover property (@({clock_edge} clk) req {implication} \
             {delay_syntax}first == expected_first {operator} \
             second == expected_second);",
                    clock_edge = clock_edge,
                    implication = if use_next { "|=>" } else { "|->" },
                    delay_syntax = if use_next {
                        String::new()
                    } else {
                        format!("##[{delay}] ")
                    },
                    operator = operator
                )
            } else {
                format!(
                    "{kind} property (@({clock_edge} clk) req {implication} \
             {delay_syntax}first == expected_first {operator} \
             second == expected_second) else $error(\"composition {index}\");",
                    kind = if is_assume { "assume" } else { "assert" },
                    clock_edge = clock_edge,
                    implication = if use_next { "|=>" } else { "|->" },
                    delay_syntax = if use_next {
                        String::new()
                    } else {
                        format!("##[{delay}] ")
                    },
                    operator = operator,
                    index = index
                )
            };
            let source = format!(
                "module top(input logic clk, req, \
         input logic [{range}] first, second, expected_first, expected_second); \
         {property} \
         endmodule",
                range = range,
                property = property
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.processes.len() != 1 {
                return Err(format!("process count {}", design.processes.len()));
            }
            let pending_count = design
                .net_decls
                .iter()
                .filter(|declaration| declaration.name.starts_with("__property_pending_"))
                .count();
            if pending_count != delay {
                return Err(format!(
                    "pending stage count {pending_count}, expected {delay}"
                ));
            }

            let inactive = u64::from(use_negedge);
            let active = 1 - inactive;
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
            simulator.set_input("req", &LogicWord::from_u64(1, 1))?;
            let initial_first = if delay == 0 && should_succeed {
                final_first
            } else {
                mismatch_first
            };
            let initial_second = if delay == 0 && should_succeed {
                final_second
            } else {
                mismatch_second
            };
            simulator.set_input("first", &LogicWord::from_u64(width, initial_first))?;
            simulator.set_input("second", &LogicWord::from_u64(width, initial_second))?;
            simulator.set_input(
                "expected_first",
                &LogicWord::from_u64(width, expected_first),
            )?;
            simulator.set_input(
                "expected_second",
                &LogicWord::from_u64(width, expected_second),
            )?;
            let mut result = simulator.set_input("clk", &LogicWord::from_u64(1, active));
            for edge in 1..=delay {
                simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                simulator.set_input("req", &LogicWord::from_u64(1, 0))?;
                let final_edge = edge == delay;
                let first = if final_edge {
                    final_first
                } else {
                    mismatch_first
                };
                let second = if final_edge {
                    final_second
                } else {
                    mismatch_second
                };
                simulator.set_input("first", &LogicWord::from_u64(width, first))?;
                simulator.set_input("second", &LogicWord::from_u64(width, second))?;
                result = simulator.set_input("clk", &LogicWord::from_u64(1, active));
            }

            if is_cover {
                result.map_err(|error| format!("composition cover errored: {error}"))?;
                let hits = simulator.property_cover_hits("composed");
                let expected_hits = u64::from(should_succeed);
                if hits != expected_hits {
                    return Err(format!("cover hits={hits}, expected={expected_hits}"));
                }
            } else if should_succeed {
                result.map_err(|error| format!("unexpected composition error: {error}"))?;
            } else {
                let error = result
                    .err()
                    .ok_or("expected composition property failure")?;
                let expected_prefix = if is_assume {
                    "assumption failed"
                } else {
                    "assertion failed"
                };
                if !error.starts_with(expected_prefix)
                    || !error.contains(&format!("composition {index}"))
                {
                    return Err(format!("unexpected composition error: {error}"));
                }
            }
            Ok(())
        });
    }
}
fn multilength_sequence_composition_property_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(
            format!("sv_property_multilength_composition_{index:03}"),
            move || {
                let width = 1 + index % 8;
                let mask = (1_u64 << width) - 1;
                let expected_left = (index as u64 * 71 + 17) & mask;
                let expected_right = (index as u64 * 43 + 29) & mask;
                let mismatch_left = expected_left ^ 1;
                let mismatch_right = expected_right ^ 1;
                let composition_variant = index % 3;
                let is_or = composition_variant == 2;
                let right_delay = 1 + index % 4;
                let use_next = index % 4 == 3;
                let base_delay = if use_next { 1 } else { index % 3 };
                let kind_variant = index % 5;
                let is_cover = kind_variant >= 3;
                let is_assume = kind_variant == 2;
                let should_succeed = matches!(kind_variant, 0 | 3);
                let use_negedge = index % 2 == 1;
                let clock_edge = if use_negedge { "negedge" } else { "posedge" };
                let range = format!("{}:0", width - 1);
                let base_left = if is_or { mismatch_left } else { expected_left };
                let final_right = if should_succeed {
                    expected_right
                } else {
                    mismatch_right
                };
                let sequence = match composition_variant {
                    0 => format!(
                        "left == expected_left ##[{right_delay}] \
             right == expected_right"
                    ),
                    1 => format!(
                        "left == expected_left and ##[{right_delay}] \
             right == expected_right"
                    ),
                    _ => format!(
                        "left == expected_left or ##[{right_delay}] \
             right == expected_right"
                    ),
                };
                let property = if is_cover {
                    format!(
                        "multilength: cover property (@({clock_edge} clk) \
             req {implication} {base_delay}{sequence});",
                        clock_edge = clock_edge,
                        implication = if use_next { "|=>" } else { "|->" },
                        base_delay = if use_next {
                            String::new()
                        } else {
                            format!("##[{base_delay}] ")
                        }
                    )
                } else {
                    format!(
                        "{kind} property (@({clock_edge} clk) req {implication} \
             {base_delay}{sequence}) else $error(\"multilength {index}\");",
                        kind = if is_assume { "assume" } else { "assert" },
                        clock_edge = clock_edge,
                        implication = if use_next { "|=>" } else { "|->" },
                        base_delay = if use_next {
                            String::new()
                        } else {
                            format!("##[{base_delay}] ")
                        },
                        index = index
                    )
                };
                let source = format!(
                    "module top(input logic clk, req, \
         input logic [{range}] left, right, expected_left, expected_right); \
         {property} \
         endmodule",
                    range = range,
                    property = property
                );
                let design = compile(&source, Some("top"))
                    .map_err(|error| format!("compile error: {error}"))?;
                if design.processes.len() != 1 {
                    return Err(format!("process count {}", design.processes.len()));
                }
                let pending_count = design
                    .net_decls
                    .iter()
                    .filter(|declaration| declaration.name.starts_with("__property_pending_"))
                    .count();
                let composed_count = design
                    .net_decls
                    .iter()
                    .filter(|declaration| declaration.name.starts_with("__property_composed_"))
                    .count();
                if pending_count != base_delay || composed_count != right_delay {
                    return Err(format!(
                        "pending={pending_count}/{base_delay}, \
             composed={composed_count}/{right_delay}"
                    ));
                }
                let inactive = u64::from(use_negedge);
                let active = 1 - inactive;
                let mut simulator = Simulator::new(&design)?;
                simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                simulator.set_input("req", &LogicWord::from_u64(1, 1))?;
                let initial_left = if base_delay == 0 {
                    base_left
                } else {
                    mismatch_left
                };
                simulator.set_input("left", &LogicWord::from_u64(width, initial_left))?;
                simulator.set_input("right", &LogicWord::from_u64(width, mismatch_right))?;
                simulator.set_input("expected_left", &LogicWord::from_u64(width, expected_left))?;
                simulator.set_input(
                    "expected_right",
                    &LogicWord::from_u64(width, expected_right),
                )?;
                let mut result = simulator.set_input("clk", &LogicWord::from_u64(1, active));
                for edge in 1..=base_delay {
                    simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                    simulator.set_input("req", &LogicWord::from_u64(1, 0))?;
                    let left = if edge == base_delay {
                        base_left
                    } else {
                        mismatch_left
                    };
                    simulator.set_input("left", &LogicWord::from_u64(width, left))?;
                    simulator.set_input("right", &LogicWord::from_u64(width, mismatch_right))?;
                    result = simulator.set_input("clk", &LogicWord::from_u64(1, active));
                }
                for edge in 1..=right_delay {
                    simulator.set_input("clk", &LogicWord::from_u64(1, inactive))?;
                    simulator.set_input("req", &LogicWord::from_u64(1, 0))?;
                    simulator.set_input("left", &LogicWord::from_u64(width, mismatch_left))?;
                    let right = if edge == right_delay {
                        final_right
                    } else {
                        mismatch_right
                    };
                    simulator.set_input("right", &LogicWord::from_u64(width, right))?;
                    result = simulator.set_input("clk", &LogicWord::from_u64(1, active));
                }

                if is_cover {
                    result.map_err(|error| format!("multilength cover errored: {error}"))?;
                    let hits = simulator.property_cover_hits("multilength");
                    let expected_hits = u64::from(should_succeed);
                    if hits != expected_hits {
                        return Err(format!("cover hits={hits}, expected={expected_hits}"));
                    }
                } else if should_succeed {
                    result.map_err(|error| format!("unexpected multilength error: {error}"))?;
                } else {
                    let error = result
                        .err()
                        .ok_or("expected multilength property failure")?;
                    let expected_prefix = if is_assume {
                        "assumption failed"
                    } else {
                        "assertion failed"
                    };
                    if !error.starts_with(expected_prefix)
                        || !error.contains(&format!("multilength {index}"))
                    {
                        return Err(format!("unexpected multilength error: {error}"));
                    }
                }
                Ok(())
            },
        );
    }
}
