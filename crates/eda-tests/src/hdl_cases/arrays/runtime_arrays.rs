//! Unpacked and runtime array selects, assignments, synthesis, and continuous loads.

use super::*;

pub(super) fn register(c: &mut CaseList) {
    unpacked_indexed_select_structure(c);
    runtime_array_indexed_select_structure(c);
    function_unpacked_indexed_select_structure(c);
    function_runtime_array_indexed_select_structure(c);
    function_runtime_array_indexed_assignment_structure(c);
    runtime_array_indexed_assignment_structure(c);
    runtime_array_indexed_assignment_synthesis_structure(c);
    runtime_array_continuous_indexed_structure(c);
}

pub(super) fn unpacked_indexed_select_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(
            format!("sv_unpacked_indexed_select_{index:03}"),
            move || {
                let element_width = 4 + index % 5;
                let element_mask = (1_u64 << element_width) - 1;
                let slice_width = 1 + index % element_width;
                let slice_mask = (1_u64 << slice_width) - 1;
                let start_offset = index % (element_width - slice_width + 1);
                let physical_initial = (index as u64 * 91 + 44) & element_mask;
                let logical_initial = if index % 2 == 0 {
                    physical_initial
                } else {
                    (0..element_width).fold(0_u64, |accumulated, bit| {
                        accumulated | (((physical_initial >> bit) & 1) << (element_width - 1 - bit))
                    })
                };
                let input_value = (index as u64 * 71 + 15) & slice_mask;
                let constant = 1 + index as u64 % 7;
                let operator = match index % 4 {
                    0 => "+",
                    1 => "^",
                    2 => "&",
                    _ => "|",
                };
                let replacement = match operator {
                    "+" => input_value.wrapping_add(constant) & slice_mask,
                    "^" => input_value ^ (constant & slice_mask),
                    "&" => input_value & (constant & slice_mask),
                    _ => input_value | (constant & slice_mask),
                };
                let logical_mask = slice_mask << start_offset;
                let logical_element =
                    (logical_initial & !logical_mask) | (replacement << start_offset);
                let physical_element = if index % 2 == 0 {
                    logical_element
                } else {
                    (0..element_width).fold(0_u64, |accumulated, bit| {
                        accumulated | (((logical_element >> bit) & 1) << (element_width - 1 - bit))
                    })
                };
                let expected = replacement;

                let ascending = index % 2 == 0;
                let selected_start = if ascending {
                    start_offset
                } else {
                    start_offset + slice_width - 1
                };
                let element_range = if ascending {
                    format!("{}:0", element_width - 1)
                } else {
                    format!("0:{}", element_width - 1)
                };
                let multidimensional = index % 4 == 3;
                let (array_declaration, array_index) = if multidimensional {
                    ("[0:1][0:1]".to_string(), "[0][1]".to_string())
                } else {
                    let reverse_dimension = index % 8 < 4;
                    if reverse_dimension {
                        ("[3:0]".to_string(), "[1]".to_string())
                    } else {
                        ("[0:3]".to_string(), "[2]".to_string())
                    }
                };
                let direction = if ascending { "+" } else { "-" };
                let assignment = if index % 4 < 2 { "=" } else { "<=" };
                let slice_range = format!("{}:0", slice_width - 1);
                let source = format!(
                    "module top(input logic clk, input logic [3:0] selector, \
                 input logic [{slice_range}] a, output logic [{slice_range}] y, \
                 output logic [{element_range}] whole); \
                 logic [{element_range}] memory {array_declaration}; \
                 initial memory{array_index} = {element_width}'d{physical_initial}; \
                 always @(posedge clk) \
                 memory{array_index}[selector {direction}: {slice_width}] {assignment} \
                 a {operator} {slice_width}'d{constant}; \
                 assign y = memory{array_index}[selector {direction}: {slice_width}]; \
                 assign whole = memory{array_index}; \
                 endmodule",
                    slice_range = slice_range,
                    element_range = element_range,
                    array_declaration = array_declaration,
                    array_index = array_index,
                    element_width = element_width,
                    physical_initial = physical_initial,
                    direction = direction,
                    slice_width = slice_width,
                    assignment = assignment,
                    operator = operator,
                    constant = constant
                );
                let design = compile(&source, Some("top"))
                    .map_err(|error| format!("compile error: {error}"))?;
                if design.processes.len() != 1 || design.assigns.len() != 2 {
                    return Err(format!(
                        "processes {} / assignments {}",
                        design.processes.len(),
                        design.assigns.len()
                    ));
                }
                let mut simulator = Simulator::new(&design)?;
                simulator.set_input("clk", &LogicWord::from_u64(1, 0))?;
                simulator.set_input("selector", &LogicWord::from_u64(4, selected_start as u64))?;
                simulator.set_input("a", &LogicWord::from_u64(slice_width, input_value))?;
                simulator.set_input("clk", &LogicWord::from_u64(1, 1))?;
                let actual = simulator
                    .read("y")
                    .and_then(|word| word.to_u64())
                    .ok_or("unpacked indexed select output is unknown")?;
                if actual != expected {
                    return Err(format!(
                        "initial={physical_initial} logical={logical_initial} width={slice_width} \
                     element_width={element_width} start={start_offset} operator={operator} \
                     ascending={ascending} multidim={multidimensional} blocking={} \
                     result={actual}, expected={expected}",
                        assignment == "="
                    ));
                }
                let actual_element = simulator
                    .read("whole")
                    .and_then(|word| word.to_u64())
                    .ok_or("unpacked indexed element output is unknown")?;
                if actual_element != physical_element {
                    return Err(format!(
                        "element result={actual_element}, expected={physical_element}"
                    ));
                }
                Ok(())
            },
        );
    }
}

pub(super) fn runtime_array_indexed_select_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_runtime_array_indexed_{index:03}"), move || {
            let element_width = 4 + index % 5;
            let element_mask = (1_u64 << element_width) - 1;
            let slice_width = 1 + index % element_width;
            let slice_mask = (1_u64 << slice_width) - 1;
            let start_offset = index % (element_width - slice_width + 1);
            let multidimensional = index % 4 == 3;
            let first_ascending = index % 2 == 0;
            let second_ascending = index % 8 < 4;
            let first_offset = index % 2;
            let second_offset = (index / 2) % 2;
            let physical_element = first_offset * 2 + second_offset;
            let selected_logical_index = if multidimensional {
                0
            } else if first_ascending {
                physical_element as i64
            } else {
                3 - physical_element as i64
            };
            let mut physical_values = Vec::with_capacity(4);
            for element in 0..4_u64 {
                physical_values.push((index as u64 * 89 + element * 37 + 11) & element_mask);
            }
            let physical_initial = physical_values[physical_element];
            let logical_initial = if index % 2 == 0 {
                physical_initial
            } else {
                (0..element_width).fold(0_u64, |accumulated, bit| {
                    accumulated | (((physical_initial >> bit) & 1) << (element_width - 1 - bit))
                })
            };
            let expected = (logical_initial >> start_offset) & slice_mask;

            let element_ascending = index % 2 == 0;
            let element_range = if element_ascending {
                format!("{}:0", element_width - 1)
            } else {
                format!("0:{}", element_width - 1)
            };
            let part_ascending = index % 4 < 2;
            let selected_start = if part_ascending {
                start_offset
            } else {
                start_offset + slice_width - 1
            };
            let direction = if part_ascending { "+" } else { "-" };
            let slice_range = format!("{}:0", slice_width - 1);
            let initializers = physical_values
                .iter()
                .enumerate()
                .map(|(element, value)| {
                    let first_init = if first_ascending {
                        if multidimensional {
                            element / 2
                        } else {
                            element
                        }
                    } else {
                        if multidimensional {
                            1 - element / 2
                        } else {
                            3 - element
                        }
                    };
                    let second_init = if second_ascending || !multidimensional {
                        element % 2
                    } else {
                        1 - element % 2
                    };
                    if multidimensional {
                        format!(
                            "memory[{first_init}][{second_init}] = {element_width}'d{value};",
                            element_width = element_width,
                            value = value
                        )
                    } else {
                        format!(
                            "memory[{first_init}] = {element_width}'d{value};",
                            element_width = element_width,
                            value = value
                        )
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            let (declaration, indexes, ports) = if multidimensional {
                (
                    format!(
                        "[{}:{}][{}:{}]",
                        if first_ascending { 0 } else { 1 },
                        if first_ascending { 1 } else { 0 },
                        if second_ascending { 0 } else { 1 },
                        if second_ascending { 1 } else { 0 }
                    ),
                    "[first][second]".to_string(),
                    "input logic first, second,".to_string(),
                )
            } else {
                let declaration = if first_ascending {
                    "[0:3]".to_string()
                } else {
                    "[3:0]".to_string()
                };
                (
                    declaration,
                    "[selector]".to_string(),
                    "input logic [1:0] selector,".to_string(),
                )
            };
            let source = format!(
                "module top(input logic [3:0] packed_selector, {ports} \
                 output logic [{slice_range}] y); \
                 logic [{element_range}] memory {declaration}; \
                 initial begin {initializers} end \
                 assign y = memory{indexes}[packed_selector {direction}: {slice_width}]; \
                 endmodule",
                ports = ports,
                slice_range = slice_range,
                element_range = element_range,
                declaration = declaration,
                initializers = initializers,
                indexes = indexes,
                direction = direction,
                slice_width = slice_width
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() != 1 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input(
                "packed_selector",
                &LogicWord::from_u64(4, selected_start as u64),
            )?;
            if multidimensional {
                let first_value = if first_ascending {
                    first_offset
                } else {
                    1 - first_offset
                };
                let second_value = if second_ascending {
                    second_offset
                } else {
                    1 - second_offset
                };
                simulator.set_input("first", &LogicWord::from_u64(1, first_value as u64))?;
                simulator.set_input("second", &LogicWord::from_u64(1, second_value as u64))?;
            } else {
                simulator.set_input(
                    "selector",
                    &LogicWord::from_u64(2, selected_logical_index as u64),
                )?;
            }
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("runtime array indexed output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "physical_element={physical_element} initial={physical_initial} \
                     element_width={element_width} slice_width={slice_width} \
                     start={start_offset} multidim={multidimensional} \
                     result={actual}, expected={expected}"
                ));
            }
            Ok(())
        });
    }
}

pub(super) fn function_unpacked_indexed_select_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(
            format!("sv_function_unpacked_indexed_{index:03}"),
            move || {
                let element_width = 4 + index % 5;
                let element_mask = (1_u64 << element_width) - 1;
                let slice_width = 1 + index % element_width;
                let slice_mask = (1_u64 << slice_width) - 1;
                let start_offset = index % (element_width - slice_width + 1);
                let physical_initial = (index as u64 * 107 + 38) & element_mask;
                let logical_initial = if index % 2 == 0 {
                    physical_initial
                } else {
                    (0..element_width).fold(0_u64, |accumulated, bit| {
                        accumulated | (((physical_initial >> bit) & 1) << (element_width - 1 - bit))
                    })
                };
                let input_value = (index as u64 * 83 + 21) & slice_mask;
                let constant = 1 + index as u64 % 7;
                let operator = match index % 4 {
                    0 => "+",
                    1 => "^",
                    2 => "&",
                    _ => "|",
                };
                let replacement = match operator {
                    "+" => input_value.wrapping_add(constant) & slice_mask,
                    "^" => input_value ^ (constant & slice_mask),
                    "&" => input_value & (constant & slice_mask),
                    _ => input_value | (constant & slice_mask),
                };
                let logical_field_mask = slice_mask << start_offset;
                let logical_element =
                    (logical_initial & !logical_field_mask) | (replacement << start_offset);
                let physical_element = if index % 2 == 0 {
                    logical_element
                } else {
                    (0..element_width).fold(0_u64, |accumulated, bit| {
                        accumulated | (((logical_element >> bit) & 1) << (element_width - 1 - bit))
                    })
                };

                let element_ascending = index % 2 == 0;
                let element_range = if element_ascending {
                    format!("{}:0", element_width - 1)
                } else {
                    format!("0:{}", element_width - 1)
                };
                let multidimensional = index % 4 == 3;
                let (array_declaration, array_index) = if multidimensional {
                    ("[0:1][1:0]".to_string(), "[0][1]".to_string())
                } else if index % 8 < 4 {
                    ("[3:0]".to_string(), "[1]".to_string())
                } else {
                    ("[0:3]".to_string(), "[2]".to_string())
                };
                let part_ascending = index % 4 < 2;
                let direction = if part_ascending { "+" } else { "-" };
                let selected_start = if part_ascending {
                    start_offset
                } else {
                    start_offset + slice_width - 1
                };
                let slice_range = format!("{}:0", slice_width - 1);
                let update_definition = format!(
                    "function logic [{slice_range}] update( \
                 input logic [3:0] selector, input logic [{slice_range}] value); \
                 logic [{element_range}] lookup {array_declaration}; \
                 lookup{array_index} = {element_width}'d{physical_initial}; \
                 lookup{array_index}[selector {direction}: {slice_width}] = \
                 value {operator} {slice_width}'d{constant}; \
                 update = lookup{array_index}[selector {direction}: {slice_width}]; \
                 endfunction",
                    slice_range = slice_range,
                    element_range = element_range,
                    array_declaration = array_declaration,
                    array_index = array_index,
                    element_width = element_width,
                    physical_initial = physical_initial,
                    direction = direction,
                    slice_width = slice_width,
                    operator = operator,
                    constant = constant
                );
                let preserve_definition = format!(
                    "function logic [{element_range}] preserve( \
                 input logic [3:0] selector, input logic [{slice_range}] value); \
                 logic [{element_range}] lookup {array_declaration}; \
                 lookup{array_index} = {element_width}'d{physical_initial}; \
                 lookup{array_index}[selector {direction}: {slice_width}] = \
                 value {operator} {slice_width}'d{constant}; \
                 preserve = lookup{array_index}; \
                 endfunction",
                    element_range = element_range,
                    slice_range = slice_range,
                    array_declaration = array_declaration,
                    array_index = array_index,
                    element_width = element_width,
                    physical_initial = physical_initial,
                    direction = direction,
                    slice_width = slice_width,
                    operator = operator,
                    constant = constant
                );
                let function_definitions = format!("{update_definition} {preserve_definition}");
                let source = if index % 2 == 0 {
                    let import = "import function_array_indexed_pkg::*;";
                    format!(
                        "package function_array_indexed_pkg; {function_definitions} endpackage \
                     {import} \
                     module top(input logic [3:0] selector, \
                     input logic [{slice_range}] value, \
                     output logic [{slice_range}] selected, \
                     output logic [{element_range}] whole); \
                     assign selected = update(selector, value); \
                     assign whole = preserve(selector, value); \
                     endmodule",
                        function_definitions = function_definitions,
                        import = import,
                        slice_range = slice_range,
                        element_range = element_range
                    )
                } else {
                    format!(
                        "module top(input logic [3:0] selector, \
                     input logic [{slice_range}] value, \
                     output logic [{slice_range}] selected, \
                     output logic [{element_range}] whole); \
                     {function_definitions} \
                     assign selected = update(selector, value); \
                     assign whole = preserve(selector, value); \
                     endmodule",
                        slice_range = slice_range,
                        element_range = element_range,
                        function_definitions = function_definitions
                    )
                };
                let design = compile(&source, Some("top"))
                    .map_err(|error| format!("compile error: {error}"))?;
                if design.assigns.len() != 2 {
                    return Err(format!("assignment count {}", design.assigns.len()));
                }
                let mut simulator = Simulator::new(&design)?;
                simulator.set_input("selector", &LogicWord::from_u64(4, selected_start as u64))?;
                simulator.set_input("value", &LogicWord::from_u64(slice_width, input_value))?;
                let actual = simulator
                    .read("selected")
                    .and_then(|word| word.to_u64())
                    .ok_or("function unpacked indexed output is unknown")?;
                if actual != replacement {
                    return Err(format!(
                        "initial={physical_initial} logical={logical_initial} \
                     element_width={element_width} slice_width={slice_width} \
                     start={start_offset} operator={operator} \
                     element_ascending={element_ascending} multidim={multidimensional} \
                     selected={actual}, expected={replacement}"
                    ));
                }
                let actual_element = simulator
                    .read("whole")
                    .and_then(|word| word.to_u64())
                    .ok_or("function unpacked indexed element is unknown")?;
                if actual_element != physical_element {
                    return Err(format!(
                        "initial={physical_initial} logical={logical_initial} \
                     element_width={element_width} slice_width={slice_width} \
                     start={start_offset} operator={operator} \
                     element_ascending={element_ascending} multidim={multidimensional} \
                     element={actual_element}, expected={physical_element}"
                    ));
                }
                Ok(())
            },
        );
    }
}

pub(super) fn function_runtime_array_indexed_select_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(
            format!("sv_function_runtime_array_indexed_{index:03}"),
            move || {
                let element_width = 4 + index % 5;
                let element_mask = (1_u64 << element_width) - 1;
                let slice_width = 1 + index % element_width;
                let slice_mask = (1_u64 << slice_width) - 1;
                let start_offset = index % (element_width - slice_width + 1);
                let multidimensional = index % 4 == 3;
                let first_ascending = index % 2 == 0;
                let second_ascending = index % 8 < 4;
                let first_offset = if multidimensional {
                    index % 2
                } else {
                    index % 4
                };
                let second_offset = (index / 2) % 2;
                let physical_element = if multidimensional {
                    first_offset * 2 + second_offset
                } else {
                    first_offset
                };
                let mut physical_values = Vec::with_capacity(4);
                for element in 0..4_u64 {
                    physical_values.push((index as u64 * 97 + element * 41 + 13) & element_mask);
                }
                let physical_initial = physical_values[physical_element];
                let logical_initial = if index % 3 == 0 {
                    physical_initial
                } else {
                    (0..element_width).fold(0_u64, |accumulated, bit| {
                        accumulated | (((physical_initial >> bit) & 1) << (element_width - 1 - bit))
                    })
                };
                let expected = (logical_initial >> start_offset) & slice_mask;

                let packed_ascending = index % 4 < 2;
                let element_range = if index % 3 == 0 {
                    format!("{}:0", element_width - 1)
                } else {
                    format!("0:{}", element_width - 1)
                };
                let direction = if packed_ascending { "+" } else { "-" };
                let selected_start = if packed_ascending {
                    start_offset
                } else {
                    start_offset + slice_width - 1
                };
                let slice_range = format!("{}:0", slice_width - 1);
                let ports = "input logic [1:0] first, input logic second,";
                let inputs = if multidimensional {
                    (
                        if first_ascending {
                            first_offset as u64
                        } else {
                            1 - first_offset as u64
                        },
                        if second_ascending {
                            second_offset as u64
                        } else {
                            1 - second_offset as u64
                        },
                    )
                } else if first_ascending {
                    (physical_element as u64, 0)
                } else {
                    (3 - physical_element as u64, 0)
                };
                let (declaration, indexes) = if multidimensional {
                    (
                        format!(
                            "[{}:{}][{}:{}]",
                            if first_ascending { 0 } else { 1 },
                            if first_ascending { 1 } else { 0 },
                            if second_ascending { 0 } else { 1 },
                            if second_ascending { 1 } else { 0 }
                        ),
                        "[first][second]".to_string(),
                    )
                } else {
                    (
                        if first_ascending {
                            "[0:3]".to_string()
                        } else {
                            "[3:0]".to_string()
                        },
                        "[first]".to_string(),
                    )
                };
                let initializers = physical_values
                    .iter()
                    .enumerate()
                    .map(|(element, value)| {
                        let first_init = if multidimensional {
                            if first_ascending {
                                element / 2
                            } else {
                                1 - element / 2
                            }
                        } else if first_ascending {
                            element
                        } else {
                            3 - element
                        };
                        let second_init = if multidimensional && !second_ascending {
                            1 - element % 2
                        } else {
                            element % 2
                        };
                        if multidimensional {
                            format!(
                                "lookup[{first_init}][{second_init}] = \
                                 {element_width}'d{value};"
                            )
                        } else {
                            format!("lookup[{first_init}] = {element_width}'d{value};")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let function_definition = format!(
                    "function logic [{slice_range}] pick( \
                     input logic [3:0] packed_selector, {ports} \
                     input logic unused); \
                     logic [{element_range}] lookup {declaration}; \
                     {initializers} \
                     pick = lookup{indexes}[packed_selector {direction}: {slice_width}]; \
                     endfunction",
                    slice_range = slice_range,
                    ports = ports,
                    element_range = element_range,
                    declaration = declaration,
                    initializers = initializers,
                    indexes = indexes,
                    direction = direction,
                    slice_width = slice_width
                );
                let source = if index % 2 == 0 {
                    format!(
                        "package function_runtime_array_pkg; \
                         {function_definition} endpackage \
                         import function_runtime_array_pkg::*; \
                         module top(input logic [3:0] packed_selector, {ports} \
                         input logic unused, output logic [{slice_range}] y); \
                         assign y = pick(packed_selector, first, second, unused); \
                         endmodule",
                        function_definition = function_definition,
                        ports = ports,
                        slice_range = slice_range
                    )
                } else {
                    format!(
                        "module top(input logic [3:0] packed_selector, {ports} \
                         input logic unused, output logic [{slice_range}] y); \
                         {function_definition} \
                         assign y = pick(packed_selector, first, second, unused); \
                         endmodule",
                        ports = ports,
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
                simulator.set_input(
                    "packed_selector",
                    &LogicWord::from_u64(4, selected_start as u64),
                )?;
                simulator.set_input("first", &LogicWord::from_u64(2, inputs.0))?;
                simulator.set_input("second", &LogicWord::from_u64(1, inputs.1))?;
                simulator.set_input("unused", &LogicWord::from_u64(1, 0))?;
                let actual = simulator
                    .read("y")
                    .and_then(|word| word.to_u64())
                    .ok_or("function runtime array indexed output is unknown")?;
                if actual != expected {
                    return Err(format!(
                        "physical_element={physical_element} initial={physical_initial} \
                         element_width={element_width} slice_width={slice_width} \
                         start={start_offset} multidim={multidimensional} \
                         result={actual}, expected={expected}"
                    ));
                }
                Ok(())
            },
        );
    }
}

pub(super) fn function_runtime_array_indexed_assignment_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(
            format!("sv_function_runtime_array_assign_{index:03}"),
            move || {
                let element_width = 4 + index % 5;
                let total_width = element_width * 4;
                let element_mask = (1_u64 << element_width) - 1;
                let slice_width = 1 + index % element_width;
                let slice_mask = (1_u64 << slice_width) - 1;
                let start_offset = index % (element_width - slice_width + 1);
                let multidimensional = index % 4 == 3;
                let first_ascending = index % 2 == 0;
                let second_ascending = index % 8 < 4;
                let first_offset = if multidimensional {
                    index % 2
                } else {
                    index % 4
                };
                let second_offset = (index / 2) % 2;
                let physical_element = if multidimensional {
                    first_offset * 2 + second_offset
                } else {
                    first_offset
                };

                let mut physical_values = Vec::with_capacity(4);
                for element in 0..4_u64 {
                    physical_values.push((index as u64 * 103 + element * 51 + 31) & element_mask);
                }
                let packed_ascending = index % 3 == 0;
                let logical_initial = if packed_ascending {
                    physical_values[physical_element]
                } else {
                    (0..element_width).fold(0_u64, |accumulated, bit| {
                        accumulated
                            | (((physical_values[physical_element] >> bit) & 1)
                                << (element_width - 1 - bit))
                    })
                };
                let input_value = (index as u64 * 73 + 19) & slice_mask;
                let constant = 1 + index as u64 % 7;
                let operator = match index % 4 {
                    0 => "+",
                    1 => "^",
                    2 => "&",
                    _ => "|",
                };
                let replacement = match operator {
                    "+" => input_value.wrapping_add(constant) & slice_mask,
                    "^" => input_value ^ (constant & slice_mask),
                    "&" => input_value & (constant & slice_mask),
                    _ => input_value | (constant & slice_mask),
                };
                let logical_mask = slice_mask << start_offset;
                let logical_element =
                    (logical_initial & !logical_mask) | (replacement << start_offset);
                let mut expected_values = physical_values.clone();
                expected_values[physical_element] = if packed_ascending {
                    logical_element
                } else {
                    (0..element_width).fold(0_u64, |accumulated, bit| {
                        accumulated | (((logical_element >> bit) & 1) << (element_width - 1 - bit))
                    })
                };
                let whole = expected_values.iter().enumerate().fold(
                    0_u64,
                    |accumulated, (position, value)| {
                        accumulated | (value << (position * element_width))
                    },
                );
                let baseline_whole = physical_values.iter().enumerate().fold(
                    0_u64,
                    |accumulated, (position, value)| {
                        accumulated | (value << (position * element_width))
                    },
                );

                let direction = if index % 4 < 2 { "+" } else { "-" };
                let selected_start = if index % 4 < 2 {
                    start_offset
                } else {
                    start_offset + slice_width - 1
                };
                let element_range = if packed_ascending {
                    format!("{}:0", element_width - 1)
                } else {
                    format!("0:{}", element_width - 1)
                };
                let total_range = format!("{}:0", total_width - 1);
                let slice_range = format!("{}:0", slice_width - 1);
                let ports = "input logic [2:0] first, \
                             input logic [2:0] second, \
                             input logic [3:0] packed_selector";
                let inputs = if multidimensional {
                    (
                        if first_ascending {
                            first_offset as u64
                        } else {
                            1 - first_offset as u64
                        },
                        if second_ascending {
                            second_offset as u64
                        } else {
                            1 - second_offset as u64
                        },
                    )
                } else if first_ascending {
                    (physical_element as u64, second_offset as u64)
                } else {
                    (3 - physical_element as u64, second_offset as u64)
                };
                let (declaration, indexes) = if multidimensional {
                    (
                        format!(
                            "[{}:{}][{}:{}]",
                            if first_ascending { 0 } else { 1 },
                            if first_ascending { 1 } else { 0 },
                            if second_ascending { 0 } else { 1 },
                            if second_ascending { 1 } else { 0 }
                        ),
                        "[first][second]".to_string(),
                    )
                } else {
                    (
                        if first_ascending {
                            "[0:3]".to_string()
                        } else {
                            "[3:0]".to_string()
                        },
                        "[first]".to_string(),
                    )
                };
                let initializers = physical_values
                    .iter()
                    .enumerate()
                    .map(|(element, value)| {
                        let first_init = if multidimensional {
                            if first_ascending {
                                element / 2
                            } else {
                                1 - element / 2
                            }
                        } else if first_ascending {
                            element
                        } else {
                            3 - element
                        };
                        let second_init = if multidimensional && !second_ascending {
                            1 - element % 2
                        } else {
                            element % 2
                        };
                        if multidimensional {
                            format!(
                                "lookup[{first_init}][{second_init}] = {element_width}'d{value};"
                            )
                        } else {
                            format!("lookup[{first_init}] = {element_width}'d{value};")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let return_elements = (0..4)
                    .rev()
                    .map(|element| {
                        let first = if multidimensional {
                            if first_ascending {
                                element / 2
                            } else {
                                1 - element / 2
                            }
                        } else if first_ascending {
                            element
                        } else {
                            3 - element
                        };
                        let second = if multidimensional && !second_ascending {
                            1 - element % 2
                        } else {
                            element % 2
                        };
                        if multidimensional {
                            format!("lookup[{first}][{second}]")
                        } else {
                            format!("lookup[{first}]")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let function_definition = format!(
                    "function logic [{total_range}] update({ports}, \
                     input logic [{slice_range}] value); \
                     logic [{element_range}] lookup {declaration}; \
                     {initializers} \
                     lookup{indexes}[packed_selector {direction}: {slice_width}] = \
                     value {operator} {slice_width}'d{constant}; \
                     update = {{{return_elements}}}; \
                     endfunction",
                    total_range = total_range,
                    ports = ports,
                    slice_range = slice_range,
                    element_range = element_range,
                    declaration = declaration,
                    initializers = initializers,
                    indexes = indexes,
                    direction = direction,
                    slice_width = slice_width,
                    operator = operator,
                    constant = constant
                );
                let source = if index % 2 == 0 {
                    format!(
                        "package function_runtime_array_assign_pkg; \
                         {function_definition} endpackage \
                         import function_runtime_array_assign_pkg::*; \
                         module top({ports}, input logic [{slice_range}] value, \
                         output logic [{total_range}] y); \
                         assign y = update(first, second, packed_selector, value); \
                         endmodule",
                        function_definition = function_definition,
                        ports = ports,
                        slice_range = slice_range,
                        total_range = total_range
                    )
                } else {
                    format!(
                        "module top({ports}, input logic [{slice_range}] value, \
                         output logic [{total_range}] y); \
                         {function_definition} \
                         assign y = update(first, second, packed_selector, value); \
                         endmodule",
                        ports = ports,
                        slice_range = slice_range,
                        total_range = total_range,
                        function_definition = function_definition
                    )
                };
                let design = compile(&source, Some("top"))
                    .map_err(|error| format!("compile error: {error}"))?;
                if design.assigns.len() != 1 {
                    return Err(format!("assignment count {}", design.assigns.len()));
                }
                let mut simulator = Simulator::new(&design)?;
                simulator.set_input("first", &LogicWord::from_u64(3, inputs.0))?;
                simulator.set_input("second", &LogicWord::from_u64(3, inputs.1))?;
                simulator.set_input(
                    "packed_selector",
                    &LogicWord::from_u64(4, selected_start as u64),
                )?;
                simulator.set_input("value", &LogicWord::from_u64(slice_width, input_value))?;
                let actual = simulator
                    .read("y")
                    .and_then(|word| word.to_u64())
                    .ok_or("function runtime array assignment output is unknown")?;
                if actual != whole {
                    return Err(format!(
                        "physical_element={physical_element} result={actual:#x}, \
                         expected={whole:#x}"
                    ));
                }

                let invalid_element = index % 2 == 0;
                let invalid_first = if invalid_element {
                    if multidimensional {
                        2
                    } else {
                        4
                    }
                } else {
                    inputs.0
                };
                let invalid_packed = if invalid_element {
                    selected_start as u64
                } else {
                    element_width as u64
                };
                simulator.set_input("first", &LogicWord::from_u64(3, invalid_first))?;
                simulator.set_input("packed_selector", &LogicWord::from_u64(4, invalid_packed))?;
                simulator.set_input(
                    "value",
                    &LogicWord::from_u64(slice_width, !input_value & slice_mask),
                )?;
                let no_op = simulator
                    .read("y")
                    .and_then(|word| word.to_u64())
                    .ok_or("function runtime array no-op output is unknown")?;
                if no_op != baseline_whole {
                    return Err(format!(
                        "invalid_element={invalid_element} result={no_op:#x}, \
                         expected={baseline_whole:#x}"
                    ));
                }
                Ok(())
            },
        );
    }
}

pub(super) fn runtime_array_indexed_assignment_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(
            format!("sv_runtime_array_indexed_assign_{index:03}"),
            move || {
                let element_width = 4 + index % 5;
                let element_mask = (1_u64 << element_width) - 1;
                let slice_width = 1 + index % element_width;
                let slice_mask = (1_u64 << slice_width) - 1;
                let start_offset = index % (element_width - slice_width + 1);
                let multidimensional = index % 4 == 0;
                let first_ascending = index % 2 == 0;
                let second_ascending = index % 8 < 4;
                let first_offset = if multidimensional {
                    index % 2
                } else {
                    index % 4
                };
                let second_offset = (index / 2) % 2;
                let physical_element = if multidimensional {
                    first_offset * 2 + second_offset
                } else {
                    first_offset
                };
                let mut physical_values = Vec::with_capacity(4);
                for element in 0..4_u64 {
                    physical_values.push((index as u64 * 83 + element * 47 + 17) & element_mask);
                }
                let physical_initial = physical_values[physical_element];
                let logical_initial = if index % 3 == 0 {
                    physical_initial
                } else {
                    (0..element_width).fold(0_u64, |accumulated, bit| {
                        accumulated | (((physical_initial >> bit) & 1) << (element_width - 1 - bit))
                    })
                };
                let input_value = (index as u64 * 71 + 23) & slice_mask;
                let constant = 1 + index as u64 % 7;
                let operator = match index % 4 {
                    0 => "+",
                    1 => "^",
                    2 => "&",
                    _ => "|",
                };
                let replacement = match operator {
                    "+" => input_value.wrapping_add(constant) & slice_mask,
                    "^" => input_value ^ (constant & slice_mask),
                    "&" => input_value & (constant & slice_mask),
                    _ => input_value | (constant & slice_mask),
                };
                let logical_mask = slice_mask << start_offset;
                let logical_element =
                    (logical_initial & !logical_mask) | (replacement << start_offset);
                let mut expected_values = physical_values.clone();
                expected_values[physical_element] = if index % 3 == 0 {
                    logical_element
                } else {
                    (0..element_width).fold(0_u64, |accumulated, bit| {
                        accumulated | (((logical_element >> bit) & 1) << (element_width - 1 - bit))
                    })
                };
                let expected_whole = expected_values.iter().enumerate().fold(
                    0_u64,
                    |accumulated, (position, value)| {
                        accumulated | (value << (position * element_width))
                    },
                );

                let packed_ascending = index % 4 < 2;
                let direction = if packed_ascending { "+" } else { "-" };
                let selected_start = if packed_ascending {
                    start_offset
                } else {
                    start_offset + slice_width - 1
                };
                let element_range = if index % 3 == 0 {
                    format!("{}:0", element_width - 1)
                } else {
                    format!("0:{}", element_width - 1)
                };
                let (declaration, indexes, first_input, second_input) = if multidimensional {
                    (
                        format!(
                            "[{}:{}][{}:{}]",
                            if first_ascending { 0 } else { 1 },
                            if first_ascending { 1 } else { 0 },
                            if second_ascending { 0 } else { 1 },
                            if second_ascending { 1 } else { 0 }
                        ),
                        "[first][second]".to_string(),
                        if first_ascending {
                            first_offset as u64
                        } else {
                            1 - first_offset as u64
                        },
                        if second_ascending {
                            second_offset as u64
                        } else {
                            1 - second_offset as u64
                        },
                    )
                } else {
                    (
                        if first_ascending {
                            "[0:3]".to_string()
                        } else {
                            "[3:0]".to_string()
                        },
                        "[first]".to_string(),
                        if first_ascending {
                            physical_element as u64
                        } else {
                            3 - physical_element as u64
                        },
                        second_offset as u64,
                    )
                };
                let initializers = physical_values
                    .iter()
                    .enumerate()
                    .map(|(element, value)| {
                        let first_init = if multidimensional {
                            if first_ascending {
                                element / 2
                            } else {
                                1 - element / 2
                            }
                        } else if first_ascending {
                            element
                        } else {
                            3 - element
                        };
                        let second_init = if multidimensional && !second_ascending {
                            1 - element % 2
                        } else {
                            element % 2
                        };
                        if multidimensional {
                            format!(
                                "memory[{first_init}][{second_init}] = \
                                 {element_width}'d{value};"
                            )
                        } else {
                            format!("memory[{first_init}] = {element_width}'d{value};")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let assignment = if index % 2 == 0 { "=" } else { "<=" };
                let slice_range = format!("{}:0", slice_width - 1);
                let source = format!(
                    "module top(input logic clk, input logic [3:0] packed_selector, \
                     input logic [1:0] first, input logic second, \
                     input logic [{slice_range}] value, \
                     output logic [{slice_range}] selected); \
                     logic [{element_range}] memory {declaration}; \
                     initial begin {initializers} end \
                     always @(posedge clk) \
                     memory{indexes}[packed_selector {direction}: {slice_width}] \
                     {assignment} value {operator} {slice_width}'d{constant}; \
                     assign selected = memory{indexes}[packed_selector {direction}: \
                     {slice_width}]; \
                     endmodule",
                    slice_range = slice_range,
                    element_range = element_range,
                    declaration = declaration,
                    initializers = initializers,
                    indexes = indexes,
                    direction = direction,
                    slice_width = slice_width,
                    assignment = assignment,
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
                simulator.set_input("clk", &LogicWord::from_u64(1, 0))?;
                simulator.set_input(
                    "packed_selector",
                    &LogicWord::from_u64(4, selected_start as u64),
                )?;
                simulator.set_input("first", &LogicWord::from_u64(2, first_input))?;
                simulator.set_input("second", &LogicWord::from_u64(1, second_input))?;
                simulator.set_input("value", &LogicWord::from_u64(slice_width, input_value))?;
                simulator.set_input("clk", &LogicWord::from_u64(1, 1))?;
                let selected = simulator
                    .read("selected")
                    .and_then(|word| word.to_u64())
                    .ok_or("runtime array indexed assignment selected output is unknown")?;
                let whole = simulator
                    .read("memory")
                    .and_then(|word| word.to_u64())
                    .ok_or("runtime array indexed assignment memory state is unknown")?;
                if selected != replacement || whole != expected_whole {
                    return Err(format!(
                        "physical_element={physical_element} selected={selected}, \
                         expected={replacement}; whole={whole}, expected={expected_whole}"
                    ));
                }
                Ok(())
            },
        );
    }
}

pub(super) fn runtime_array_indexed_assignment_synthesis_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(
            format!("sv_runtime_array_indexed_assign_synth_{index:03}"),
            move || {
                let element_width = 4 + index % 5;
                let total_width = element_width * 4;
                let element_mask = (1_u64 << element_width) - 1;
                let slice_width = 1 + index % element_width;
                let slice_mask = (1_u64 << slice_width) - 1;
                let start_offset = index % (element_width - slice_width + 1);
                let multidimensional = index % 4 == 0;
                let first_ascending = index % 2 == 0;
                let second_ascending = index % 8 < 4;
                let physical_element = index % 4;
                let (physical_first, physical_second) =
                    (physical_element / 2, physical_element % 2);

                let mut physical_values = Vec::with_capacity(4);
                for element in 0..4_u64 {
                    physical_values.push((index as u64 * 97 + element * 43 + 19) & element_mask);
                }
                let physical_initial = physical_values[physical_element];
                let packed_ascending = index % 3 == 0;
                let logical_initial = if packed_ascending {
                    physical_initial
                } else {
                    (0..element_width).fold(0_u64, |accumulated, bit| {
                        accumulated | (((physical_initial >> bit) & 1) << (element_width - 1 - bit))
                    })
                };
                let input_value = (index as u64 * 71 + 23) & slice_mask;
                let constant = 1 + index as u64 % 7;
                let operator = match index % 4 {
                    0 => "+",
                    1 => "^",
                    2 => "&",
                    _ => "|",
                };
                let replacement = match operator {
                    "+" => input_value.wrapping_add(constant) & slice_mask,
                    "^" => input_value ^ (constant & slice_mask),
                    "&" => input_value & (constant & slice_mask),
                    _ => input_value | (constant & slice_mask),
                };
                let logical_mask = slice_mask << start_offset;
                let logical_element =
                    (logical_initial & !logical_mask) | (replacement << start_offset);
                let mut expected_values = physical_values.clone();
                expected_values[physical_element] = if packed_ascending {
                    logical_element
                } else {
                    (0..element_width).fold(0_u64, |accumulated, bit| {
                        accumulated | (((logical_element >> bit) & 1) << (element_width - 1 - bit))
                    })
                };
                let baseline_whole = physical_values.iter().enumerate().fold(
                    0_u64,
                    |accumulated, (position, value)| {
                        accumulated | (value << (position * element_width))
                    },
                );
                let expected_whole = expected_values.iter().enumerate().fold(
                    0_u64,
                    |accumulated, (position, value)| {
                        accumulated | (value << (position * element_width))
                    },
                );

                let direction = if index % 4 < 2 { "+" } else { "-" };
                let selected_start = if index % 4 < 2 {
                    start_offset
                } else {
                    start_offset + slice_width - 1
                };
                let element_range = if packed_ascending {
                    format!("{}:0", element_width - 1)
                } else {
                    format!("0:{}", element_width - 1)
                };
                let slice_range = format!("{}:0", slice_width - 1);
                let assignment = if index % 2 == 0 { "<=" } else { "=" };
                let (array_declaration, indexes, valid_first, valid_second) = if multidimensional {
                    (
                        format!(
                            "[{}:{}][{}:{}]",
                            if first_ascending { 0 } else { 1 },
                            if first_ascending { 1 } else { 0 },
                            if second_ascending { 0 } else { 1 },
                            if second_ascending { 1 } else { 0 }
                        ),
                        "[first][second]".to_string(),
                        if first_ascending {
                            physical_first as u64
                        } else {
                            1 - physical_first as u64
                        },
                        if second_ascending {
                            physical_second as u64
                        } else {
                            1 - physical_second as u64
                        },
                    )
                } else {
                    (
                        if first_ascending {
                            "[0:3]".to_string()
                        } else {
                            "[3:0]".to_string()
                        },
                        "[first]".to_string(),
                        if first_ascending {
                            physical_element as u64
                        } else {
                            3 - physical_element as u64
                        },
                        physical_second as u64,
                    )
                };
                let resets = physical_values
                    .iter()
                    .enumerate()
                    .map(|(physical_index, value)| {
                        let (first, second) = (physical_index / 2, physical_index % 2);
                        let first = if multidimensional {
                            if first_ascending {
                                first
                            } else {
                                1 - first
                            }
                        } else if first_ascending {
                            physical_index
                        } else {
                            3 - physical_index
                        };
                        let second = if multidimensional && !second_ascending {
                            1 - second
                        } else {
                            second
                        };
                        if multidimensional {
                            format!(
                                "memory[{first}][{second}] {assignment} {element_width}'d{value};"
                            )
                        } else {
                            format!("memory[{first}] {assignment} {element_width}'d{value};")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let source = format!(
                    "module top(input logic clk, input logic [2:0] first, second, \
                     input logic [3:0] packed_selector, \
                     input logic [{slice_range}] value, \
                     output logic [{element_range}] memory {array_declaration}); \
                     always @(posedge clk) begin {resets} \
                     memory{indexes}[packed_selector {direction}: {slice_width}] \
                     {assignment} value {operator} {slice_width}'d{constant}; \
                     end endmodule",
                    slice_range = slice_range,
                    element_range = element_range,
                    array_declaration = array_declaration,
                    resets = resets,
                    indexes = indexes,
                    direction = direction,
                    slice_width = slice_width,
                    assignment = assignment,
                    operator = operator,
                    constant = constant
                );

                let rtl = compile(&source, Some("top"))
                    .map_err(|error| format!("compile error: {error}"))?;
                let synthesized = synthesize(&rtl, &SynthOptions::default())
                    .map_err(|error| format!("synthesis error: {error}"))?;
                let sequential_cells = synthesized.netlist.stats().seq;
                if sequential_cells != total_width {
                    return Err(format!(
                        "sequential cell count {sequential_cells}, expected {total_width}"
                    ));
                }
                let gate = netlist_to_design(&synthesized.netlist);
                let mut rtl_sim = Simulator::new(&rtl)?;
                let mut gate_sim = Simulator::new(&gate)?;
                for simulator in [&mut rtl_sim, &mut gate_sim] {
                    simulator.set_input("clk", &LogicWord::from_u64(1, 0))?;
                    simulator.set_input("first", &LogicWord::from_u64(3, valid_first))?;
                    simulator.set_input("second", &LogicWord::from_u64(3, valid_second))?;
                    simulator.set_input(
                        "packed_selector",
                        &LogicWord::from_u64(4, selected_start as u64),
                    )?;
                    simulator.set_input("value", &LogicWord::from_u64(slice_width, input_value))?;
                    simulator.set_input("clk", &LogicWord::from_u64(1, 1))?;
                }
                let actual_rtl = rtl_sim
                    .read_port("memory")
                    .and_then(|word| word.to_u64())
                    .ok_or("RTL runtime array indexed synthesis output is unknown")?;
                let actual_gate = gate_sim
                    .read_port("memory")
                    .and_then(|word| word.to_u64())
                    .ok_or("gate runtime array indexed synthesis output is unknown")?;
                if actual_rtl != expected_whole || actual_gate != expected_whole {
                    return Err(format!(
                        "physical_element={physical_element} rtl={actual_rtl:#x} \
                         gate={actual_gate:#x}, expected={expected_whole:#x}"
                    ));
                }

                let invalid_element = index % 2 == 0;
                let (invalid_first, invalid_second, invalid_packed) = if invalid_element {
                    let invalid_first = if multidimensional { 2 } else { 4 };
                    (invalid_first, valid_second, selected_start as u64)
                } else {
                    (valid_first, valid_second, element_width as u64)
                };
                for simulator in [&mut rtl_sim, &mut gate_sim] {
                    simulator.set_input("clk", &LogicWord::from_u64(1, 0))?;
                    simulator.set_input("first", &LogicWord::from_u64(3, invalid_first))?;
                    simulator.set_input("second", &LogicWord::from_u64(3, invalid_second))?;
                    simulator
                        .set_input("packed_selector", &LogicWord::from_u64(4, invalid_packed))?;
                    simulator.set_input(
                        "value",
                        &LogicWord::from_u64(slice_width, !input_value & slice_mask),
                    )?;
                    simulator.set_input("clk", &LogicWord::from_u64(1, 1))?;
                }
                let noop_rtl = rtl_sim
                    .read_port("memory")
                    .and_then(|word| word.to_u64())
                    .ok_or("RTL runtime array indexed no-op output is unknown")?;
                let noop_gate = gate_sim
                    .read_port("memory")
                    .and_then(|word| word.to_u64())
                    .ok_or("gate runtime array indexed no-op output is unknown")?;
                if noop_rtl != baseline_whole || noop_gate != baseline_whole {
                    return Err(format!(
                        "invalid_element={invalid_element} rtl={noop_rtl:#x} \
                         gate={noop_gate:#x}, expected={baseline_whole:#x}"
                    ));
                }
                Ok(())
            },
        );
    }
}

pub(super) fn runtime_array_continuous_indexed_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(
            format!("sv_runtime_array_continuous_indexed_{index:03}"),
            move || {
                let element_width = 4 + index % 5;
                let element_mask = (1_u64 << element_width) - 1;
                let slice_width = 1 + index % element_width;
                let slice_mask = (1_u64 << slice_width) - 1;
                let start_offset = index % (element_width - slice_width + 1);
                let multidimensional = index % 4 == 0;
                let first_ascending = index % 2 == 0;
                let second_ascending = index % 8 < 4;
                let physical_first_element = index % 4;
                let physical_second_element = (physical_first_element + 1) % 4;

                let mut physical_values = Vec::with_capacity(4);
                for element in 0..4_u64 {
                    physical_values.push((index as u64 * 101 + element * 53 + 29) & element_mask);
                }
                let initial_values = physical_values.clone();
                let packed_ascending = index % 3 == 0;
                let logical_value = |value: u64| {
                    if packed_ascending {
                        value
                    } else {
                        (0..element_width).fold(0_u64, |accumulated, bit| {
                            accumulated | (((value >> bit) & 1) << (element_width - 1 - bit))
                        })
                    }
                };
                let input_first = (index as u64 * 67 + 5) & slice_mask;
                let input_second = (index as u64 * 41 + 17) & slice_mask;
                let input_third = (index as u64 * 29 + 11) & slice_mask;
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
                let update_element = |physical: u64, replacement: u64| {
                    let logical = logical_value(physical);
                    let mask = slice_mask << start_offset;
                    let logical = (logical & !mask) | (replacement << start_offset);
                    if packed_ascending {
                        logical
                    } else {
                        logical_value(logical)
                    }
                };
                physical_values[physical_first_element] = update_element(
                    initial_values[physical_first_element],
                    transformed(input_second),
                );
                physical_values[physical_second_element] = update_element(
                    initial_values[physical_second_element],
                    transformed(input_third),
                );
                let expected_first_first_update = logical_value(update_element(
                    initial_values[physical_first_element],
                    transformed(input_first),
                ));
                let expected_first = logical_value(physical_values[physical_first_element]);
                let expected_second = logical_value(physical_values[physical_second_element]);
                let input_fourth = !input_third & slice_mask;

                let direction = if index % 4 < 2 { "+" } else { "-" };
                let selected_start = if index % 4 < 2 {
                    start_offset
                } else {
                    start_offset + slice_width - 1
                };
                let element_range = if packed_ascending {
                    format!("{}:0", element_width - 1)
                } else {
                    format!("0:{}", element_width - 1)
                };
                let (array_declaration, indexes, read_indexes) = if multidimensional {
                    (
                        format!(
                            "[{}:{}][{}:{}]",
                            if first_ascending { 0 } else { 1 },
                            if first_ascending { 1 } else { 0 },
                            if second_ascending { 0 } else { 1 },
                            if second_ascending { 1 } else { 0 }
                        ),
                        "[first][second]".to_string(),
                        "[read_first][read_second]".to_string(),
                    )
                } else {
                    (
                        if first_ascending {
                            "[0:3]".to_string()
                        } else {
                            "[3:0]".to_string()
                        },
                        "[first]".to_string(),
                        "[read_first]".to_string(),
                    )
                };
                let logical_indexes = |physical: usize| {
                    if multidimensional {
                        let first = physical / 2;
                        let second = physical % 2;
                        (
                            if first_ascending {
                                first as u64
                            } else {
                                1 - first as u64
                            },
                            if second_ascending {
                                second as u64
                            } else {
                                1 - second as u64
                            },
                        )
                    } else {
                        (
                            if first_ascending {
                                physical as u64
                            } else {
                                3 - physical as u64
                            },
                            (physical % 2) as u64,
                        )
                    }
                };
                let (target_first, target_second) = logical_indexes(physical_first_element);
                let (second_target_first, second_target_second) =
                    logical_indexes(physical_second_element);
                let initializers = initial_values
                    .iter()
                    .enumerate()
                    .map(|(physical, value)| {
                        let (first, second) = logical_indexes(physical);
                        if multidimensional {
                            format!("memory[{first}][{second}] = {element_width}'d{value};")
                        } else {
                            format!("memory[{first}] = {element_width}'d{value};")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let slice_range = format!("{}:0", slice_width - 1);
                let source = format!(
                    "module top(input logic [2:0] first, second, read_first, read_second, \
                     input logic [{slice_range}] value, \
                     output logic [{element_range}] selected); \
                     logic [{element_range}] memory {array_declaration}; \
                     initial begin {initializers} end \
                     assign memory{indexes}[{selected_start} {direction}: {slice_width}] = \
                     value {operator} {slice_width}'d{constant}; \
                     assign selected = memory{read_indexes}[0 +: {element_width}]; \
                     endmodule",
                    slice_range = slice_range,
                    element_range = element_range,
                    array_declaration = array_declaration,
                    initializers = initializers,
                    indexes = indexes,
                    selected_start = selected_start,
                    direction = direction,
                    slice_width = slice_width,
                    operator = operator,
                    constant = constant,
                    read_indexes = read_indexes
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
                let set_target = |simulator: &mut Simulator,
                                  first: u64,
                                  second: u64,
                                  read_first: u64,
                                  read_second: u64,
                                  value: u64|
                 -> Result<(), String> {
                    simulator
                        .set_input("first", &LogicWord::from_u64(3, first))
                        .map_err(|error| error.to_string())?;
                    simulator
                        .set_input("second", &LogicWord::from_u64(3, second))
                        .map_err(|error| error.to_string())?;
                    simulator
                        .set_input("read_first", &LogicWord::from_u64(3, read_first))
                        .map_err(|error| error.to_string())?;
                    simulator
                        .set_input("read_second", &LogicWord::from_u64(3, read_second))
                        .map_err(|error| error.to_string())?;
                    simulator
                        .set_input("value", &LogicWord::from_u64(slice_width, value))
                        .map_err(|error| error.to_string())?;
                    Ok(())
                };

                set_target(
                    &mut simulator,
                    target_first,
                    target_second,
                    target_first,
                    target_second,
                    input_first,
                )?;
                let selected = simulator
                    .read("selected")
                    .and_then(|word| word.to_u64())
                    .ok_or("runtime array continuous first output is unknown")?;
                if selected
                    != logical_value(update_element(
                        initial_values[physical_first_element],
                        transformed(input_first),
                    ))
                    && selected != expected_first_first_update
                {
                    return Err(format!(
                        "first stimulus selected={selected}, expected reactivity update"
                    ));
                }

                set_target(
                    &mut simulator,
                    target_first,
                    target_second,
                    target_first,
                    target_second,
                    input_second,
                )?;
                let selected = simulator
                    .read("selected")
                    .and_then(|word| word.to_u64())
                    .ok_or("runtime array continuous updated output is unknown")?;
                if selected != expected_first {
                    return Err(format!(
                        "updated selected={selected}, expected={expected_first}"
                    ));
                }

                set_target(
                    &mut simulator,
                    second_target_first,
                    second_target_second,
                    second_target_first,
                    second_target_second,
                    input_third,
                )?;
                let selected = simulator
                    .read("selected")
                    .and_then(|word| word.to_u64())
                    .ok_or("runtime array continuous second output is unknown")?;
                if selected != expected_second {
                    return Err(format!(
                        "second selected={selected}, expected={expected_second}"
                    ));
                }

                set_target(
                    &mut simulator,
                    if multidimensional { 2 } else { 4 },
                    second_target_second,
                    second_target_first,
                    second_target_second,
                    input_fourth,
                )?;
                let selected = simulator
                    .read("selected")
                    .and_then(|word| word.to_u64())
                    .ok_or("runtime array continuous no-op output is unknown")?;
                if selected != expected_second {
                    return Err(format!(
                        "invalid-index selected={selected}, expected={expected_second}"
                    ));
                }
                Ok(())
            },
        );
    }
}
