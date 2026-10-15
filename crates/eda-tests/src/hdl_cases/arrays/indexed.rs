//! Static, runtime, reversed, and function-local indexed part selects.

use super::*;

pub(super) fn register(c: &mut CaseList) {
    indexed_part_select_structure(c);
    runtime_indexed_part_select_structure(c);
    runtime_indexed_part_write_structure(c);
    runtime_indexed_part_nba_structure(c);
    runtime_indexed_nba_synth_structure(c);
    function_runtime_indexed_select_structure(c);
    reversed_indexed_select_structure(c);
    reversed_function_indexed_select_structure(c);
    reversed_continuous_indexed_structure(c);
    runtime_continuous_indexed_structure(c);
    reversed_indexed_write_structure(c);
}

pub(super) fn indexed_part_select_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_indexed_part_select_{index:03}"), move || {
            const VECTOR_WIDTH: usize = 16;
            let slice_width = 1 + index % 8;
            let mask = (1_u64 << slice_width) - 1;
            let start = index % (VECTOR_WIDTH - slice_width + 1);
            let value = (index as u64 * 97 + 19) & ((1_u64 << VECTOR_WIDTH) - 1);
            let constant = 1 + index as u64 % 7;
            let operator = match index % 4 {
                0 => "+",
                1 => "^",
                2 => "&",
                _ => "|",
            };
            let element = (value >> start) & mask;
            let transformed = match operator {
                "+" => element.wrapping_add(constant) & mask,
                "^" => element ^ (constant & mask),
                "&" => element & (constant & mask),
                _ => element | (constant & mask),
            };
            let expected = if index % 4 == 3 {
                transformed << start
            } else {
                transformed
            };
            let ascending = index % 2 == 0;
            let selected = if ascending {
                format!("{start} +: {slice_width}")
            } else {
                format!("{} -: {slice_width}", start + slice_width - 1)
            };
            let vector_range = format!("{}:0", VECTOR_WIDTH - 1);
            let zero_literal = format!("{}'d0", VECTOR_WIDTH - slice_width);
            let expression = format!("a[{selected}] {operator} {slice_width}'d{constant}");
            let source = match index % 4 {
                0 | 2 => {
                    let function_definition = format!(
                        "function logic [{vector_range}] select( \
                         input logic [{vector_range}] value); \
                         select = value[{selected}] {operator} {slice_width}'d{constant}; \
                         endfunction",
                        vector_range = vector_range,
                        selected = selected,
                        operator = operator,
                        slice_width = slice_width,
                        constant = constant
                    );
                    format!(
                        "module top(input logic [{vector_range}] a, \
                         output logic [{vector_range}] y); \
                         {function_definition} \
                         assign y = select(a); \
                         endmodule",
                        vector_range = vector_range,
                        function_definition = function_definition
                    )
                }
                1 => format!(
                    "module top(input logic [{vector_range}] a, \
                     output logic [{vector_range}] y); \
                     assign y = {{{zero_literal}, {expression}}}; \
                     endmodule",
                    vector_range = vector_range,
                    expression = expression,
                    zero_literal = zero_literal
                ),
                _ => format!(
                    "module top(input logic [{vector_range}] a, \
                     output logic [{vector_range}] y); \
                     always @(*) begin \
                         y = 16'd0; \
                         y[{selected}] = a[{selected}] {operator} {slice_width}'d{constant}; \
                     end \
                     endmodule",
                    vector_range = vector_range,
                    selected = selected,
                    operator = operator,
                    slice_width = slice_width,
                    constant = constant
                ),
            };
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if index % 4 == 3 {
                if design.processes.len() != 1 || !design.assigns.is_empty() {
                    return Err(format!(
                        "processes {} / assignments {}",
                        design.processes.len(),
                        design.assigns.len()
                    ));
                }
            } else if design.assigns.len() != 1 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("a", &LogicWord::from_u64(VECTOR_WIDTH, value))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("indexed part-select output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "value={value} start={start} width={slice_width} operator={operator} \
                     ascending={ascending} result={actual}, expected={expected}"
                ));
            }
            Ok(())
        });
    }
}

pub(super) fn runtime_indexed_part_select_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_runtime_indexed_select_{index:03}"), move || {
            const VECTOR_WIDTH: usize = 16;
            let slice_width = 1 + index % 8;
            let mask = (1_u64 << slice_width) - 1;
            let start = index % (VECTOR_WIDTH - slice_width + 1);
            let value = (index as u64 * 101 + 27) & ((1_u64 << VECTOR_WIDTH) - 1);
            let constant = 1 + index as u64 % 7;
            let operator = match index % 4 {
                0 => "+",
                1 => "^",
                2 => "&",
                _ => "|",
            };
            let element = (value >> start) & mask;
            let expected = match operator {
                "+" => element.wrapping_add(constant) & mask,
                "^" => element ^ (constant & mask),
                "&" => element & (constant & mask),
                _ => element | (constant & mask),
            };
            let ascending = index % 2 == 0;
            let selected = if ascending {
                "selector + 0 +: ".to_string()
            } else {
                "selector + 0 -: ".to_string()
            };
            let vector_range = format!("{}:0", VECTOR_WIDTH - 1);
            let slice_range = format!("{}:0", slice_width - 1);
            let source = format!(
                "module top(input logic [{vector_range}] a, \
                 input logic [3:0] selector, output logic [{slice_range}] y); \
                 assign y = a[{selected}{slice_width}] {operator} {slice_width}'d{constant}; \
                 endmodule",
                vector_range = vector_range,
                slice_range = slice_range,
                selected = selected,
                slice_width = slice_width,
                operator = operator,
                constant = constant
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() != 1 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let selector_value = if ascending {
                start as u64
            } else {
                (start + slice_width - 1) as u64
            };
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("a", &LogicWord::from_u64(VECTOR_WIDTH, value))?;
            simulator.set_input("selector", &LogicWord::from_u64(4, selector_value))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("runtime indexed part-select output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "value={value} selector={selector_value} start={start} \
                     width={slice_width} operator={operator} ascending={ascending} \
                     result={actual}, expected={expected}"
                ));
            }
            Ok(())
        });
    }
}

pub(super) fn runtime_indexed_part_write_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_runtime_indexed_write_{index:03}"), move || {
            const VECTOR_WIDTH: usize = 16;
            let slice_width = 1 + index % 8;
            let mask = (1_u64 << slice_width) - 1;
            let start = index % (VECTOR_WIDTH - slice_width + 1);
            let initial = (index as u64 * 71 + 35) & ((1_u64 << VECTOR_WIDTH) - 1);
            let input_value = (index as u64 * 89 + 14) & mask;
            let constant = 1 + index as u64 % 7;
            let operator = match index % 4 {
                0 => "+",
                1 => "^",
                2 => "&",
                _ => "|",
            };
            let replacement = match operator {
                "+" => input_value.wrapping_add(constant) & mask,
                "^" => input_value ^ (constant & mask),
                "&" => input_value & (constant & mask),
                _ => input_value | (constant & mask),
            };
            let field_mask = mask << start;
            let expected = (initial & !field_mask) | (replacement << start);

            let ascending = index % 2 == 0;
            let direction = if ascending { "+" } else { "-" };
            let selector_value = if ascending {
                start as u64
            } else {
                (start + slice_width - 1) as u64
            };
            let vector_range = format!("{}:0", VECTOR_WIDTH - 1);
            let slice_range = format!("{}:0", slice_width - 1);
            let source = format!(
                "module top(input logic clk, input logic [3:0] selector, \
                 input logic [{slice_range}] a, output logic [{vector_range}] y); \
                 initial y = {vector_width}'d{initial}; \
                 always @(posedge clk) \
                 y[selector {direction}: {slice_width}] = a {operator} {slice_width}'d{constant}; \
                 endmodule",
                slice_range = slice_range,
                vector_range = vector_range,
                vector_width = VECTOR_WIDTH,
                initial = initial,
                direction = direction,
                slice_width = slice_width,
                operator = operator,
                constant = constant
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.processes.len() != 1 || !design.assigns.is_empty() {
                return Err(format!(
                    "processes {} / assignments {}",
                    design.processes.len(),
                    design.assigns.len()
                ));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("clk", &LogicWord::from_u64(1, 0))?;
            simulator.set_input("selector", &LogicWord::from_u64(4, selector_value))?;
            simulator.set_input("a", &LogicWord::from_u64(slice_width, input_value))?;
            simulator.set_input("clk", &LogicWord::from_u64(1, 1))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("runtime indexed write output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "initial={initial} input={input_value} selector={selector_value} \
                     start={start} width={slice_width} operator={operator} \
                     ascending={ascending} result={actual}, expected={expected}"
                ));
            }
            Ok(())
        });
    }
}

pub(super) fn runtime_indexed_part_nba_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_runtime_indexed_nba_{index:03}"), move || {
            const VECTOR_WIDTH: usize = 16;
            let slice_width = 1 + index % 4;
            let mask = (1_u64 << slice_width) - 1;
            let first_start = index % (VECTOR_WIDTH - 2 * slice_width + 1);
            let second_start = first_start + slice_width;
            let initial = (index as u64 * 83 + 47) & ((1_u64 << VECTOR_WIDTH) - 1);
            let first_input = (index as u64 * 103 + 8) & mask;
            let second_input = (index as u64 * 61 + 19) & mask;
            let first_constant = 1 + index as u64 % 5;
            let second_constant = 1 + index as u64 % 7;
            let first_operator = match index % 4 {
                0 => "+",
                1 => "^",
                2 => "&",
                _ => "|",
            };
            let second_operator = match (index / 4) % 4 {
                0 => "|",
                1 => "&",
                2 => "^",
                _ => "+",
            };
            let first_value = match first_operator {
                "+" => first_input.wrapping_add(first_constant) & mask,
                "^" => first_input ^ (first_constant & mask),
                "&" => first_input & (first_constant & mask),
                _ => first_input | (first_constant & mask),
            };
            let second_value = match second_operator {
                "+" => second_input.wrapping_add(second_constant) & mask,
                "^" => second_input ^ (second_constant & mask),
                "&" => second_input & (second_constant & mask),
                _ => second_input | (second_constant & mask),
            };
            let first_mask = mask << first_start;
            let second_mask = mask << second_start;
            let expected = (initial & !first_mask & !second_mask)
                | (first_value << first_start)
                | (second_value << second_start);

            let vector_range = format!("{}:0", VECTOR_WIDTH - 1);
            let slice_range = format!("{}:0", slice_width - 1);
            let second_high = second_start + slice_width - 1;
            let source = format!(
                "module top(input logic clk, input logic [3:0] first, second, \
                 input logic [{slice_range}] a, b, output logic [{vector_range}] y); \
                 initial y = {vector_width}'d{initial}; \
                 always @(posedge clk) begin \
                     y[first +: {slice_width}] <= a {first_operator} {slice_width}'d{first_constant}; \
                     y[second -: {slice_width}] <= b {second_operator} {slice_width}'d{second_constant}; \
                 end \
                 endmodule",
                slice_range = slice_range,
                vector_range = vector_range,
                vector_width = VECTOR_WIDTH,
                initial = initial,
                slice_width = slice_width,
                first_operator = first_operator,
                first_constant = first_constant,
                second_operator = second_operator,
                second_constant = second_constant
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.processes.len() != 1 || !design.assigns.is_empty() {
                return Err(format!(
                    "processes {} / assignments {}",
                    design.processes.len(),
                    design.assigns.len()
                ));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("clk", &LogicWord::from_u64(1, 0))?;
            simulator.set_input("first", &LogicWord::from_u64(4, first_start as u64))?;
            simulator.set_input(
                "second",
                &LogicWord::from_u64(4, second_high as u64),
            )?;
            simulator.set_input("a", &LogicWord::from_u64(slice_width, first_input))?;
            simulator.set_input("b", &LogicWord::from_u64(slice_width, second_input))?;
            simulator.set_input("clk", &LogicWord::from_u64(1, 1))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("runtime indexed NBA output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "initial={initial} first={first_start} second={second_start} \
                     width={slice_width} result={actual}, expected={expected}"
                ));
            }
            Ok(())
        });
    }
}

pub(super) fn runtime_indexed_nba_synth_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_runtime_indexed_nba_synth_{index:03}"), move || {
            const VECTOR_WIDTH: usize = 8;
            let slice_width = 1 + index % 4;
            let mask = (1_u64 << slice_width) - 1;
            let first_start = index % (VECTOR_WIDTH - 2 * slice_width + 1);
            let second_start = first_start + slice_width;
            let first_input = (index as u64 * 97 + 11) & mask;
            let second_input = (index as u64 * 59 + 23) & mask;
            let first_constant = 1 + index as u64 % 5;
            let second_constant = 1 + index as u64 % 7;
            let first_operator = match index % 4 {
                0 => "+",
                1 => "^",
                2 => "&",
                _ => "|",
            };
            let second_operator = match (index / 4) % 4 {
                0 => "|",
                1 => "&",
                2 => "^",
                _ => "+",
            };
            let first_value = match first_operator {
                "+" => first_input.wrapping_add(first_constant) & mask,
                "^" => first_input ^ (first_constant & mask),
                "&" => first_input & (first_constant & mask),
                _ => first_input | (first_constant & mask),
            };
            let second_value = match second_operator {
                "+" => second_input.wrapping_add(second_constant) & mask,
                "^" => second_input ^ (second_constant & mask),
                "&" => second_input & (second_constant & mask),
                _ => second_input | (second_constant & mask),
            };
            let expected = (first_value << first_start)
                | (second_value << second_start);

            let vector_range = format!("{}:0", VECTOR_WIDTH - 1);
            let slice_range = format!("{}:0", slice_width - 1);
            let second_high = second_start + slice_width - 1;
            let source = format!(
                "module top(input logic clk, input logic [2:0] first, second, \
                 input logic [{slice_range}] a, b, output logic [{vector_range}] y); \
                 always @(posedge clk) begin \
                     y <= 8'd0; \
                     y[first +: {slice_width}] <= a {first_operator} {slice_width}'d{first_constant}; \
                     y[second -: {slice_width}] <= b {second_operator} {slice_width}'d{second_constant}; \
                 end \
                 endmodule",
                slice_range = slice_range,
                vector_range = vector_range,
                slice_width = slice_width,
                first_operator = first_operator,
                first_constant = first_constant,
                second_operator = second_operator,
                second_constant = second_constant
            );
            let rtl = compile(&source, Some("top"))
                .map_err(|error| format!("compile error: {error}"))?;
            let synthesized = synthesize(&rtl, &SynthOptions::default())
                .map_err(|error| format!("synthesis error: {error}"))?;
            if synthesized.netlist.stats().seq != VECTOR_WIDTH {
                return Err(format!(
                    "sequential cell count {}, expected {VECTOR_WIDTH}",
                    synthesized.netlist.stats().seq
                ));
            }
            let gate = netlist_to_design(&synthesized.netlist);
            let mut rtl_sim = Simulator::new(&rtl)?;
            let mut gate_sim = Simulator::new(&gate)?;
            for simulator in [&mut rtl_sim, &mut gate_sim] {
                simulator.set_input("clk", &LogicWord::from_u64(1, 0))?;
                simulator.set_input(
                    "first",
                    &LogicWord::from_u64(3, first_start as u64),
                )?;
                simulator.set_input(
                    "second",
                    &LogicWord::from_u64(3, second_high as u64),
                )?;
                simulator.set_input(
                    "a",
                    &LogicWord::from_u64(slice_width, first_input),
                )?;
                simulator.set_input(
                    "b",
                    &LogicWord::from_u64(slice_width, second_input),
                )?;
                simulator.set_input("clk", &LogicWord::from_u64(1, 1))?;
            }
            let actual_rtl = rtl_sim
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("RTL runtime indexed NBA output is unknown")?;
            let actual_gate = gate_sim
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("gate runtime indexed NBA output is unknown")?;
            if actual_rtl != expected || actual_gate != expected {
                return Err(format!(
                    "first={first_start} second={second_start} width={slice_width} \
                     rtl={actual_rtl} gate={actual_gate}, expected={expected}"
                ));
            }
            Ok(())
        });
    }
}

pub(super) fn function_runtime_indexed_select_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(
            format!("sv_function_runtime_indexed_{index:03}"),
            move || {
                const VECTOR_WIDTH: usize = 16;
                let slice_width = 1 + index % 8;
                let mask = (1_u64 << slice_width) - 1;
                let start = index % (VECTOR_WIDTH - slice_width + 1);
                let value = (index as u64 * 109 + 31) & ((1_u64 << VECTOR_WIDTH) - 1);
                let constant = 1 + index as u64 % 7;
                let operator = match index % 4 {
                    0 => "+",
                    1 => "^",
                    2 => "&",
                    _ => "|",
                };
                let element = (value >> start) & mask;
                let expected = match operator {
                    "+" => element.wrapping_add(constant) & mask,
                    "^" => element ^ (constant & mask),
                    "&" => element & (constant & mask),
                    _ => element | (constant & mask),
                };
                let ascending = index % 4 < 2;
                let selected = if ascending {
                    format!("selector +: {slice_width}")
                } else {
                    format!("selector -: {slice_width}")
                };
                let selector_value = if ascending {
                    start as u64
                } else {
                    (start + slice_width - 1) as u64
                };
                let vector_range = format!("{}:0", VECTOR_WIDTH - 1);
                let slice_range = format!("{}:0", slice_width - 1);
                let use_local = index % 4 == 1 || index % 4 == 3;
                let body = if use_local {
                    format!(
                        "logic [{vector_range}] copy; \
                     copy = value; \
                     pick = copy[{selected}] {operator} {slice_width}'d{constant};",
                        vector_range = vector_range,
                        selected = selected,
                        operator = operator,
                        slice_width = slice_width,
                        constant = constant
                    )
                } else {
                    format!(
                        "pick = value[{selected}] {operator} {slice_width}'d{constant};",
                        selected = selected,
                        operator = operator,
                        slice_width = slice_width,
                        constant = constant
                    )
                };
                let function_definition = format!(
                    "function logic [{slice_range}] pick( \
                 input logic [{vector_range}] value, input logic [3:0] selector); \
                 {body} \
                 endfunction",
                    slice_range = slice_range,
                    vector_range = vector_range,
                    body = body
                );
                let source = if index % 2 == 0 {
                    let import = if index % 4 == 0 {
                        "import runtime_indexed_pkg::*;"
                    } else {
                        "import runtime_indexed_pkg::pick;"
                    };
                    format!(
                        "package runtime_indexed_pkg; {function_definition} endpackage \
                     {import} \
                     module top(input logic [{vector_range}] a, \
                     input logic [3:0] selector, output logic [{slice_range}] y); \
                     assign y = pick(a, selector); \
                     endmodule",
                        function_definition = function_definition,
                        import = import,
                        vector_range = vector_range,
                        slice_range = slice_range
                    )
                } else {
                    format!(
                        "module top(input logic [{vector_range}] a, \
                     input logic [3:0] selector, output logic [{slice_range}] y); \
                     {function_definition} \
                     assign y = pick(a, selector); \
                     endmodule",
                        vector_range = vector_range,
                        slice_range = slice_range,
                        function_definition = function_definition
                    )
                };
                let design = compile(&source, Some("top"))
                    .map_err(|error| format!("compile error: {error}"))?;
                if design.assigns.len() != 1 {
                    return Err(format!("assignment count {}", design.assigns.len()));
                }
                let mut simulator = Simulator::new(&design)?;
                simulator.set_input("a", &LogicWord::from_u64(VECTOR_WIDTH, value))?;
                simulator.set_input("selector", &LogicWord::from_u64(4, selector_value))?;
                let actual = simulator
                    .read("y")
                    .and_then(|word| word.to_u64())
                    .ok_or("function runtime indexed output is unknown")?;
                if actual != expected {
                    return Err(format!(
                        "value={value} selector={selector_value} start={start} \
                     width={slice_width} operator={operator} ascending={ascending} \
                     local={use_local} result={actual}, expected={expected}"
                    ));
                }
                Ok(())
            },
        );
    }
}

pub(super) fn reversed_indexed_select_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(
            format!("sv_reversed_indexed_select_{index:03}"),
            move || {
                const VECTOR_WIDTH: usize = 16;
                let slice_width = 1 + index % 8;
                let mask = (1_u64 << slice_width) - 1;
                let reversed_base = if index % 2 == 0 { 0 } else { 2 };
                let start_offset = index % (VECTOR_WIDTH - slice_width + 1);
                let logical_start = reversed_base + start_offset;
                let physical_value = (index as u64 * 137 + 52) & ((1_u64 << VECTOR_WIDTH) - 1);
                let mut logical_value = 0_u64;
                for logical_bit in 0..VECTOR_WIDTH {
                    let physical_bit = VECTOR_WIDTH - 1 - logical_bit;
                    if (physical_value >> physical_bit) & 1 != 0 {
                        logical_value |= 1_u64 << logical_bit;
                    }
                }
                let element = (logical_value >> start_offset) & mask;
                let constant = 1 + index as u64 % 7;
                let operator = match index % 4 {
                    0 => "+",
                    1 => "^",
                    2 => "&",
                    _ => "|",
                };
                let expected = match operator {
                    "+" => element.wrapping_add(constant) & mask,
                    "^" => element ^ (constant & mask),
                    "&" => element & (constant & mask),
                    _ => element | (constant & mask),
                };

                let ascending = index % 2 == 0;
                let selected_start = if ascending {
                    logical_start
                } else {
                    logical_start + slice_width - 1
                };
                let vector_range = format!("{reversed_base}:{}", reversed_base + VECTOR_WIDTH - 1);
                let slice_range = format!("{}:0", slice_width - 1);
                let runtime = index % 4 < 2;
                let selected = if ascending {
                    format!("{selected_start} +: {slice_width}")
                } else {
                    format!("{selected_start} -: {slice_width}")
                };
                let source = if runtime {
                    format!(
                        "module top(input logic [{vector_range}] a, \
                     input logic [4:0] selector, output logic [{slice_range}] y); \
                     assign y = a[selector {direction}: {slice_width}] {operator} \
                     {slice_width}'d{constant}; \
                     endmodule",
                        vector_range = vector_range,
                        slice_range = slice_range,
                        direction = if ascending { "+" } else { "-" },
                        slice_width = slice_width,
                        operator = operator,
                        constant = constant
                    )
                } else {
                    format!(
                        "module top(input logic [{vector_range}] a, \
                     output logic [{slice_range}] y); \
                     assign y = a[{selected}] {operator} {slice_width}'d{constant}; \
                     endmodule",
                        vector_range = vector_range,
                        slice_range = slice_range,
                        selected = selected,
                        operator = operator,
                        slice_width = slice_width,
                        constant = constant
                    )
                };
                let design = compile(&source, Some("top"))
                    .map_err(|error| format!("compile error: {error}"))?;
                if design.assigns.len() != 1 {
                    return Err(format!("assignment count {}", design.assigns.len()));
                }
                let mut simulator = Simulator::new(&design)?;
                simulator.set_input("a", &LogicWord::from_u64(VECTOR_WIDTH, physical_value))?;
                if runtime {
                    simulator
                        .set_input("selector", &LogicWord::from_u64(5, selected_start as u64))?;
                }
                let actual = simulator
                    .read("y")
                    .and_then(|word| word.to_u64())
                    .ok_or("reversed indexed select output is unknown")?;
                if actual != expected {
                    return Err(format!(
                        "physical={physical_value} logical={logical_value} base={reversed_base} \
                     start={logical_start} width={slice_width} operator={operator} \
                     ascending={ascending} runtime={runtime} result={actual}, expected={expected}"
                    ));
                }
                Ok(())
            },
        );
    }
}

pub(super) fn reversed_function_indexed_select_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(
            format!("sv_reversed_function_indexed_{index:03}"),
            move || {
                const VECTOR_WIDTH: usize = 16;
                let slice_width = 1 + index % 8;
                let mask = (1_u64 << slice_width) - 1;
                let reversed_base = if index % 2 == 0 { 0 } else { 2 };
                let start_offset = index % (VECTOR_WIDTH - slice_width + 1);
                let logical_start = reversed_base + start_offset;
                let physical_value = (index as u64 * 139 + 58) & ((1_u64 << VECTOR_WIDTH) - 1);
                let mut logical_value = 0_u64;
                for logical_bit in 0..VECTOR_WIDTH {
                    let physical_bit = VECTOR_WIDTH - 1 - logical_bit;
                    if (physical_value >> physical_bit) & 1 != 0 {
                        logical_value |= 1_u64 << logical_bit;
                    }
                }
                let element = (logical_value >> start_offset) & mask;
                let constant = 1 + index as u64 % 7;
                let operator = match index % 4 {
                    0 => "+",
                    1 => "^",
                    2 => "&",
                    _ => "|",
                };
                let expected = match operator {
                    "+" => element.wrapping_add(constant) & mask,
                    "^" => element ^ (constant & mask),
                    "&" => element & (constant & mask),
                    _ => element | (constant & mask),
                };

                let ascending = index % 2 == 0;
                let selected_start = if ascending {
                    logical_start
                } else {
                    logical_start + slice_width - 1
                };
                let vector_range = format!("{reversed_base}:{}", reversed_base + VECTOR_WIDTH - 1);
                let slice_range = format!("{}:0", slice_width - 1);
                let use_local = index % 4 == 1 || index % 4 == 3;
                let body = if use_local {
                    format!(
                        "logic [{vector_range}] copy; \
                     copy = value; \
                     pick = copy[selector {direction}: {slice_width}] {operator} \
                     {slice_width}'d{constant};",
                        vector_range = vector_range,
                        direction = if ascending { "+" } else { "-" },
                        slice_width = slice_width,
                        operator = operator,
                        constant = constant
                    )
                } else {
                    format!(
                        "pick = value[selector {direction}: {slice_width}] {operator} \
                     {slice_width}'d{constant};",
                        direction = if ascending { "+" } else { "-" },
                        slice_width = slice_width,
                        operator = operator,
                        constant = constant
                    )
                };
                let function_definition = format!(
                    "function logic [{slice_range}] pick( \
                 input logic [{vector_range}] value, input logic [4:0] selector); \
                 {body} \
                 endfunction",
                    slice_range = slice_range,
                    vector_range = vector_range,
                    body = body
                );
                let source = if index % 2 == 0 {
                    let import = if index % 4 == 0 {
                        "import reversed_indexed_pkg::*;"
                    } else {
                        "import reversed_indexed_pkg::pick;"
                    };
                    format!(
                        "package reversed_indexed_pkg; {function_definition} endpackage \
                     {import} \
                     module top(input logic [{vector_range}] a, \
                     input logic [4:0] selector, output logic [{slice_range}] y); \
                     assign y = pick(a, selector); \
                     endmodule",
                        function_definition = function_definition,
                        import = import,
                        vector_range = vector_range,
                        slice_range = slice_range
                    )
                } else {
                    format!(
                        "module top(input logic [{vector_range}] a, \
                     input logic [4:0] selector, output logic [{slice_range}] y); \
                     {function_definition} \
                     assign y = pick(a, selector); \
                     endmodule",
                        vector_range = vector_range,
                        slice_range = slice_range,
                        function_definition = function_definition
                    )
                };
                let design = compile(&source, Some("top"))
                    .map_err(|error| format!("compile error: {error}"))?;
                if design.assigns.len() != 1 {
                    return Err(format!("assignment count {}", design.assigns.len()));
                }
                let mut simulator = Simulator::new(&design)?;
                simulator.set_input("a", &LogicWord::from_u64(VECTOR_WIDTH, physical_value))?;
                simulator.set_input("selector", &LogicWord::from_u64(5, selected_start as u64))?;
                let actual = simulator
                    .read("y")
                    .and_then(|word| word.to_u64())
                    .ok_or("reversed function indexed output is unknown")?;
                if actual != expected {
                    return Err(format!(
                        "physical={physical_value} base={reversed_base} start={logical_start} \
                     width={slice_width} operator={operator} ascending={ascending} \
                     local={use_local} result={actual}, expected={expected}"
                    ));
                }
                Ok(())
            },
        );
    }
}

pub(super) fn reversed_continuous_indexed_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(
            format!("sv_reversed_continuous_indexed_{index:03}"),
            move || {
                const VECTOR_WIDTH: usize = 16;
                let slice_width = 1 + index % 8;
                let mask = (1_u64 << slice_width) - 1;
                let reversed_base = if index % 2 == 0 { 0 } else { 2 };
                let start_offset = index % (VECTOR_WIDTH - slice_width + 1);
                let logical_start = reversed_base + start_offset;
                let input_value = (index as u64 * 83 + 37) & mask;
                let constant = 1 + index as u64 % 7;
                let operator = match index % 4 {
                    0 => "+",
                    1 => "^",
                    2 => "&",
                    _ => "|",
                };
                let expected = match operator {
                    "+" => input_value.wrapping_add(constant) & mask,
                    "^" => input_value ^ (constant & mask),
                    "&" => input_value & (constant & mask),
                    _ => input_value | (constant & mask),
                };

                let ascending = index % 2 == 0;
                let selected_start = if ascending {
                    logical_start
                } else {
                    logical_start + slice_width - 1
                };
                let vector_range = format!("{reversed_base}:{}", reversed_base + VECTOR_WIDTH - 1);
                let slice_range = format!("{}:0", slice_width - 1);
                let direction = if ascending { "+" } else { "-" };
                let source = format!(
                    "module top(input logic [{slice_range}] a, \
                 output logic [{vector_range}] y, output logic [{slice_range}] selected); \
                 assign y[{selected_start} {direction}: {slice_width}] = \
                 a {operator} {slice_width}'d{constant}; \
                 assign selected = y[{selected_start} {direction}: {slice_width}]; \
                 endmodule",
                    slice_range = slice_range,
                    vector_range = vector_range,
                    selected_start = selected_start,
                    direction = direction,
                    slice_width = slice_width,
                    operator = operator,
                    constant = constant
                );
                let design = compile(&source, Some("top"))
                    .map_err(|error| format!("compile error: {error}"))?;
                if design.assigns.len() != 2 {
                    return Err(format!("assignment count {}", design.assigns.len()));
                }
                let mut simulator = Simulator::new(&design)?;
                simulator.set_input("a", &LogicWord::from_u64(slice_width, input_value))?;
                let actual = simulator
                    .read("selected")
                    .and_then(|word| word.to_u64())
                    .ok_or("reversed continuous indexed output is unknown")?;
                if actual != expected {
                    return Err(format!(
                        "input={input_value} base={reversed_base} start={logical_start} \
                     width={slice_width} operator={operator} ascending={ascending} \
                     result={actual}, expected={expected}"
                    ));
                }
                Ok(())
            },
        );
    }
}

pub(super) fn runtime_continuous_indexed_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(
            format!("sv_runtime_continuous_indexed_{index:03}"),
            move || {
                const VECTOR_WIDTH: usize = 16;
                let slice_width = 1 + index % 8;
                let slice_mask = (1_u64 << slice_width) - 1;
                let start_offset = index % (VECTOR_WIDTH - slice_width + 1);
                let reversed_declaration = index % 2 == 1;
                let declaration_base = if index % 4 < 2 { 0 } else { 2 };
                let initial = (index as u64 * 109 + 47) & ((1_u64 << VECTOR_WIDTH) - 1);
                let first_input = (index as u64 * 83 + 17) & slice_mask;
                let second_input = (index as u64 * 47 + 29) & slice_mask;
                let constant = 1 + index as u64 % 7;
                let operator = match index % 4 {
                    0 => "+",
                    1 => "^",
                    2 => "&",
                    _ => "|",
                };
                let transformed = |input: u64| match operator {
                    "+" => input.wrapping_add(constant) & slice_mask,
                    "^" => input ^ (constant & slice_mask),
                    "&" => input & (constant & slice_mask),
                    _ => input | (constant & slice_mask),
                };
                let replacement = transformed(second_input);
                let expected = if reversed_declaration {
                    let mut field_mask = 0_u64;
                    let mut replacement_bits = 0_u64;
                    for bit in 0..slice_width {
                        let physical_bit = VECTOR_WIDTH - 1 - (start_offset + bit);
                        field_mask |= 1_u64 << physical_bit;
                        replacement_bits |= ((replacement >> bit) & 1) << physical_bit;
                    }
                    (initial & !field_mask) | replacement_bits
                } else {
                    let field_mask = slice_mask << start_offset;
                    (initial & !field_mask) | (replacement << start_offset)
                };

                let ascending = index % 2 == 0;
                let logical_start = declaration_base + start_offset;
                let selected_start = if ascending {
                    logical_start
                } else {
                    logical_start + slice_width - 1
                };
                let vector_range = if reversed_declaration {
                    format!("{declaration_base}:{}", declaration_base + VECTOR_WIDTH - 1)
                } else {
                    format!("{}:{declaration_base}", declaration_base + VECTOR_WIDTH - 1)
                };
                let slice_range = format!("{}:0", slice_width - 1);
                let direction = if ascending { "+" } else { "-" };
                let source = format!(
                    "module top(input logic [4:0] selector, \
                     input logic [{slice_range}] a, \
                     output logic [{vector_range}] y, output logic [{slice_range}] selected); \
                     initial y = 16'd{initial}; \
                     assign y[selector {direction}: {slice_width}] = \
                     a {operator} {slice_width}'d{constant}; \
                     assign selected = y[selector {direction}: {slice_width}]; \
                     endmodule",
                    slice_range = slice_range,
                    vector_range = vector_range,
                    initial = initial,
                    direction = direction,
                    slice_width = slice_width,
                    operator = operator,
                    constant = constant
                );
                let design = compile(&source, Some("top"))
                    .map_err(|error| format!("compile error: {error}"))?;
                if design.processes.len() != 1 || design.assigns.len() != 1 {
                    return Err(format!(
                        "processes {} / assignments {}",
                        design.processes.len(),
                        design.assigns.len()
                    ));
                }
                let mut simulator = Simulator::new(&design)?;
                simulator.set_input("a", &LogicWord::from_u64(slice_width, first_input))?;
                simulator.set_input("selector", &LogicWord::from_u64(5, selected_start as u64))?;
                let first_selected = simulator
                    .read("selected")
                    .and_then(|word| word.to_u64())
                    .ok_or("runtime continuous indexed first output is unknown")?;
                if first_selected != transformed(first_input) {
                    return Err(format!(
                        "first selected={first_selected}, expected={}",
                        transformed(first_input)
                    ));
                }

                simulator.set_input("a", &LogicWord::from_u64(slice_width, second_input))?;
                let actual = simulator
                    .read("y")
                    .and_then(|word| word.to_u64())
                    .ok_or("runtime continuous indexed whole output is unknown")?;
                let selected = simulator
                    .read("selected")
                    .and_then(|word| word.to_u64())
                    .ok_or("runtime continuous indexed selected output is unknown")?;
                if actual != expected || selected != replacement {
                    return Err(format!(
                        "whole={actual:#x}, expected={expected:#x}; \
                         selected={selected}, expected={replacement}"
                    ));
                }

                let invalid_start = declaration_base + VECTOR_WIDTH;
                simulator.set_input("selector", &LogicWord::from_u64(5, invalid_start as u64))?;
                let unchanged = simulator
                    .read("y")
                    .and_then(|word| word.to_u64())
                    .ok_or("runtime continuous indexed no-op output is unknown")?;
                if unchanged != expected {
                    return Err(format!(
                        "invalid whole={unchanged:#x}, expected={expected:#x}"
                    ));
                }
                if simulator.read("selected").unwrap().to_u64().is_some() {
                    return Err("invalid indexed read should be X".to_string());
                }
                Ok(())
            },
        );
    }
}

pub(super) fn reversed_indexed_write_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_reversed_indexed_write_{index:03}"), move || {
            const VECTOR_WIDTH: usize = 16;
            let slice_width = 1 + index % 8;
            let mask = (1_u64 << slice_width) - 1;
            let reversed_base = if index % 2 == 0 { 0 } else { 2 };
            let start_offset = index % (VECTOR_WIDTH - slice_width + 1);
            let logical_start = reversed_base + start_offset;
            let initial = (index as u64 * 79 + 41) & ((1_u64 << VECTOR_WIDTH) - 1);
            let input_value = (index as u64 * 113 + 26) & mask;
            let constant = 1 + index as u64 % 7;
            let operator = match index % 4 {
                0 => "+",
                1 => "^",
                2 => "&",
                _ => "|",
            };
            let replacement = match operator {
                "+" => input_value.wrapping_add(constant) & mask,
                "^" => input_value ^ (constant & mask),
                "&" => input_value & (constant & mask),
                _ => input_value | (constant & mask),
            };
            let mut field_mask = 0_u64;
            for bit in 0..slice_width {
                field_mask |= 1_u64 << (VECTOR_WIDTH - 1 - (start_offset + bit));
            }
            let replacement_bits = (0..slice_width).fold(0_u64, |accumulated, bit| {
                accumulated
                    | (((replacement >> bit) & 1) << (VECTOR_WIDTH - 1 - (start_offset + bit)))
            });
            let expected = (initial & !field_mask) | replacement_bits;

            let ascending = index % 2 == 0;
            let selected_start = if ascending {
                logical_start
            } else {
                logical_start + slice_width - 1
            };
            let vector_range = format!("{reversed_base}:{}", reversed_base + VECTOR_WIDTH - 1);
            let slice_range = format!("{}:0", slice_width - 1);
            let direction = if ascending { "+" } else { "-" };
            let assignment = if index % 4 < 2 { "=" } else { "<=" };
            let source = format!(
                "module top(input logic clk, input logic [4:0] selector, \
                 input logic [{slice_range}] a, output logic [{vector_range}] y); \
                 initial y = {vector_width}'d{initial}; \
                 always @(posedge clk) \
                 y[selector {direction}: {slice_width}] {assignment} \
                 a {operator} {slice_width}'d{constant}; \
                 endmodule",
                slice_range = slice_range,
                vector_range = vector_range,
                vector_width = VECTOR_WIDTH,
                initial = initial,
                direction = direction,
                slice_width = slice_width,
                assignment = assignment,
                operator = operator,
                constant = constant
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.processes.len() != 1 || !design.assigns.is_empty() {
                return Err(format!(
                    "processes {} / assignments {}",
                    design.processes.len(),
                    design.assigns.len()
                ));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("clk", &LogicWord::from_u64(1, 0))?;
            simulator.set_input("selector", &LogicWord::from_u64(5, selected_start as u64))?;
            simulator.set_input("a", &LogicWord::from_u64(slice_width, input_value))?;
            simulator.set_input("clk", &LogicWord::from_u64(1, 1))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("reversed indexed write output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "initial={initial} input={input_value} base={reversed_base} \
                     start={logical_start} width={slice_width} operator={operator} \
                     ascending={ascending} blocking={} result={actual}, expected={expected}",
                    assignment == "="
                ));
            }
            Ok(())
        });
    }
}
