//! Module unpacked arrays, port slices, and signed synthesis.

use super::*;

pub(super) fn register(c: &mut CaseList) {
    module_unpacked_array_structure(c);
    multidim_module_unpacked_array_structure(c);
    fixed_unpacked_array_port_structure(c);
    unpacked_array_port_slice_structure(c);
    signed_synthesis_structure(c);
}

pub(super) fn module_unpacked_array_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_module_net_array_{index:03}"), move || {
            let element_width = 1 + index % 7;
            let length = 2 + index % 8;
            let total_width = element_width * length;
            let element_mask = (1_u64 << element_width) - 1;
            let total_mask = (1_u64 << total_width) - 1;
            let value = (index as u64 * 113 + 34) & element_mask;
            let selector = (index % 2) as u64;
            let constant = 1 + index as u64 % 5;
            let ascending = index % 2 == 0;
            let variant = index % 4;
            let element_range = format!("{}:0", element_width - 1);
            let total_range = format!("{}:0", total_width - 1);
            let array_range = if ascending {
                format!("0:{}", length - 1)
            } else {
                format!("{}:0", length - 1)
            };

            let assignments = if variant == 0 {
                (0..length)
                    .map(|element| {
                        format!("assign memory[{element}] = a + {element_width}'d{constant};")
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            } else if variant == 1 {
                let statements = (0..length)
                    .map(|element| format!("memory[{element}] = a ^ {element_width}'d{constant};"))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("always @(*) begin {statements} end")
            } else if variant == 2 {
                format!(
                    "generate for (genvar i=0; i<{length}; i=i+1) \
                     begin : fill assign memory[i] = a - {element_width}'d{constant}; end \
                     endgenerate"
                )
            } else {
                (0..length)
                    .map(|element| {
                        format!(
                            "assign memory[{element}] = selector ? \
                             a & {element_width}'d{constant} : a | {element_width}'d{constant};"
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            };

            let concatenation = (0..length)
                .map(|element| format!("memory[{element}]"))
                .collect::<Vec<_>>()
                .join(", ");
            let needs_selector = variant == 3;
            let port_list = if needs_selector {
                format!(
                    "input logic selector, input logic [{element_range}] a, \
                     output logic [{total_range}] y"
                )
            } else {
                format!("input logic [{element_range}] a, output logic [{total_range}] y")
            };
            let source = format!(
                "module top({port_list}); \
                 logic [{element_range}] memory [{array_range}]; \
                 {assignments} \
                 assign y = {{{concatenation}}}; \
                 endmodule",
                port_list = port_list,
                element_range = element_range,
                array_range = array_range,
                assignments = assignments,
                concatenation = concatenation
            );
            let design =
                compile(&source, None).map_err(|error| format!("compile error: {error}"))?;
            let declaration = design
                .net_decls
                .iter()
                .find(|declaration| declaration.name == "memory")
                .ok_or("missing unpacked array declaration")?;
            if declaration.width != total_width {
                return Err(format!(
                    "array width {}, expected {total_width}",
                    declaration.width
                ));
            }
            let expected = (0..length)
                .map(|element| {
                    let result = match variant {
                        0 => value.wrapping_add(constant) & element_mask,
                        1 => value ^ (constant & element_mask),
                        2 => value.wrapping_sub(constant) & element_mask,
                        _ => {
                            if selector != 0 {
                                value & (constant & element_mask)
                            } else {
                                value | (constant & element_mask)
                            }
                        }
                    };
                    result << ((length - 1 - element) * element_width)
                })
                .fold(0_u64, |accumulated, element| accumulated | element)
                & total_mask;

            let mut simulator = Simulator::new(&design)?;
            if needs_selector {
                simulator.set_input("selector", &LogicWord::from_u64(1, selector))?;
            }
            simulator.set_input("a", &LogicWord::from_u64(element_width, value))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("module unpacked-array output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "variant={variant} value={value} constant={constant} width={element_width} \
                     length={length} result={actual}, expected={expected}"
                ));
            }
            Ok(())
        });
    }
}

pub(super) fn multidim_module_unpacked_array_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_module_net_array2d_{index:03}"), move || {
            let element_width = 1 + index % 4;
            let first_length = 2 + index % 2;
            let second_length = 2 + (index / 2) % 2;
            let length = first_length * second_length;
            let total_width = element_width * length;
            let element_mask = (1_u64 << element_width) - 1;
            let total_mask = (1_u64 << total_width) - 1;
            let left = (index as u64 * 127 + 36) & element_mask;
            let right = (index as u64 * 71 + 46) & element_mask;
            let first_ascending = index % 4 < 2;
            let second_ascending = index % 8 < 4;
            let operator = match index % 4 {
                0 => "&",
                1 => "|",
                2 => "^",
                _ => "+",
            };
            let mut assignments = Vec::with_capacity(length);
            let mut expected = 0_u64;
            for logical in 0..length {
                let first = logical / second_length;
                let second = logical % second_length;
                assignments.push(format!(
                    "assign memory[{first}][{second}] = a {operator} b;"
                ));
                let result = match operator {
                    "&" => left & right,
                    "|" => left | right,
                    "^" => left ^ right,
                    _ => left.wrapping_add(right) & element_mask,
                };
                expected |= result << ((length - 1 - logical) * element_width);
            }
            expected &= total_mask;
            let element_range = format!("{}:0", element_width - 1);
            let total_range = format!("{}:0", total_width - 1);
            let first_range = if first_ascending {
                format!("0:{}", first_length - 1)
            } else {
                format!("{}:0", first_length - 1)
            };
            let second_range = if second_ascending {
                format!("0:{}", second_length - 1)
            } else {
                format!("{}:0", second_length - 1)
            };
            let mut concatenation_parts = Vec::with_capacity(length);
            for logical in 0..length {
                let first = logical / second_length;
                let second = logical % second_length;
                concatenation_parts.push(format!("memory[{first}][{second}]"));
            }
            let source = format!(
                "module top(input logic [{element_range}] a, b, \
                 output logic [{total_range}] y); \
                 logic [{element_range}] memory [{first_range}][{second_range}]; \
                 {assignments} \
                 assign y = {{{concatenation}}}; \
                 endmodule",
                element_range = element_range,
                total_range = total_range,
                first_range = first_range,
                second_range = second_range,
                assignments = assignments.join(" "),
                concatenation = concatenation_parts.join(", ")
            );
            let design =
                compile(&source, None).map_err(|error| format!("compile error: {error}"))?;
            let declaration = design
                .net_decls
                .iter()
                .find(|declaration| declaration.name == "memory")
                .ok_or("missing 2D module array declaration")?;
            if declaration.width != total_width {
                return Err(format!(
                    "array width {}, expected {total_width}",
                    declaration.width
                ));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("a", &LogicWord::from_u64(element_width, left))?;
            simulator.set_input("b", &LogicWord::from_u64(element_width, right))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("2D module array output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "left={left} right={right} operator={operator} \
                     shape={first_length}x{second_length} width={element_width} \
                     result={actual}, expected={expected}"
                ));
            }
            Ok(())
        });
    }
}

pub(super) fn fixed_unpacked_array_port_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_module_array_port_{index:03}"), move || {
            let multidimensional = index % 4 == 0;
            let element_width = 1 + index % 8;
            let element_mask = (1_u64 << element_width) - 1;
            let first_ascending = index % 4 < 2;
            let second_ascending = index % 8 < 4;
            let named = index % 2 == 0;
            let constant = 1 + index as u64 % 7;
            let constant_masked = constant & element_mask;
            let (first_indexes, first_range) = if first_ascending {
                ((0..2_i64).collect::<Vec<_>>(), "0:1".to_string())
            } else {
                (vec![1_i64, 0], "1:0".to_string())
            };
            let (second_indexes, second_range) = if second_ascending {
                (vec![0_i64, 1], "0:1".to_string())
            } else {
                (vec![1_i64, 0], "1:0".to_string())
            };
            let indexes = if multidimensional {
                let mut pairs = Vec::with_capacity(first_indexes.len() * second_indexes.len());
                for first in &first_indexes {
                    for second in &second_indexes {
                        pairs.push(vec![*first, *second]);
                    }
                }
                pairs
            } else {
                let length = 2 + index % 7;
                let indexes = if index % 2 == 0 {
                    (0..length as i64).collect::<Vec<_>>()
                } else {
                    (0..length as i64).rev().collect::<Vec<_>>()
                };
                indexes.into_iter().map(|value| vec![value]).collect()
            };
            let length = indexes.len();
            let total_width = element_width * length;
            let total_mask = if total_width == 64 {
                u64::MAX
            } else {
                (1_u64 << total_width) - 1
            };
            let input_value = (index as u64 * 109 + 21) & total_mask;
            let variant = if multidimensional {
                (index / 4) % 4
            } else {
                index % 4
            };
            let operator = match variant {
                0 => "+",
                1 => "^",
                2 => "&",
                _ => "|",
            };
            let mut assignments = Vec::with_capacity(length);
            let mut expected = 0_u64;
            for (storage_offset, index_set) in indexes.iter().enumerate() {
                let shift = storage_offset * element_width;
                let element = (input_value >> shift) & element_mask;
                let result = match operator {
                    "+" => element.wrapping_add(constant_masked) & element_mask,
                    "^" => element ^ constant_masked,
                    "&" => element & constant_masked,
                    _ => element | constant_masked,
                };
                expected |= result << shift;
                let selected = index_set
                    .iter()
                    .map(|value| format!("[{value}]"))
                    .collect::<String>();
                assignments.push(format!(
                    "assign out_mem{selected} = in_mem{selected} {operator} \
                     {element_width}'d{constant};"
                ));
            }
            expected &= total_mask;
            let element_range = format!("{}:0", element_width - 1);
            let total_range = format!("{}:0", total_width - 1);
            let array_dimensions = if multidimensional {
                format!("[{first_range}][{second_range}]")
            } else {
                let length_range = if index % 2 == 0 {
                    format!("0:{}", length - 1)
                } else {
                    format!("{}:0", length - 1)
                };
                format!("[{length_range}]")
            };
            let connections = if named {
                ".in_mem(a), .out_mem(y)".to_string()
            } else {
                "a, y".to_string()
            };
            let source = format!(
                "module child( \
                 input logic [{element_range}] in_mem {array_dimensions}, \
                 output logic [{element_range}] out_mem {array_dimensions}); \
                 {assignments} \
                 endmodule \
                 module top(input logic [{total_range}] a, output logic [{total_range}] y); \
                 child c({connections}); \
                 endmodule",
                element_range = element_range,
                array_dimensions = array_dimensions,
                assignments = assignments.join(" "),
                total_range = total_range,
                connections = connections
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            for name in ["in_mem", "out_mem"] {
                let declaration = design
                    .net_decls
                    .iter()
                    .find(|declaration| declaration.name == format!("c.{name}"))
                    .ok_or_else(|| format!("missing child array port {name}"))?;
                if declaration.width != total_width {
                    return Err(format!(
                        "child port {name} width {}, expected {total_width}",
                        declaration.width
                    ));
                }
            }
            let child = design
                .hierarchy
                .iter()
                .find(|instance| instance.module == "child")
                .ok_or("missing child instance hierarchy record")?;
            if child.ports.len() != 2 || child.ports.iter().any(|port| port.width != total_width) {
                return Err(format!(
                    "child port widths {:?}, expected two ports of width {total_width}",
                    child
                        .ports
                        .iter()
                        .map(|port| port.width)
                        .collect::<Vec<_>>()
                ));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("a", &LogicWord::from_u64(total_width, input_value))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("array-port output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "shape={} width={element_width} operator={operator} named={named} \
                     input={input_value} result={actual}, expected={expected}",
                    if multidimensional { "2x2" } else { "1D" }
                ));
            }
            Ok(())
        });
    }
}

pub(super) fn unpacked_array_port_slice_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_module_array_port_slice_{index:03}"), move || {
            let width = 1 + index % 8;
            let element_mask = (1_u64 << width) - 1;
            let source_length = 4 + index % 5;
            let slice_length = 2 + index % 3;
            let input_start = index % (source_length + 1 - slice_length);
            let output_start = (index / 3) % (source_length + 1 - slice_length);
            let source_ascending = index % 2 == 0;
            let result_ascending = index % 4 < 2;
            let named = index % 2 == 0;
            let operator = match index % 4 {
                0 => "+",
                1 => "^",
                2 => "&",
                _ => "|",
            };
            let constant = 1 + index as u64 % 7;
            let total_width = width * source_length;
            let total_mask = if total_width == 64 {
                u64::MAX
            } else {
                (1_u64 << total_width) - 1
            };
            let input_value = (index as u64 * 131 + 45) & total_mask;

            let source_range = if source_ascending {
                format!("0:{}", source_length - 1)
            } else {
                format!("{}:0", source_length - 1)
            };
            let result_range = if result_ascending {
                format!("0:{}", source_length - 1)
            } else {
                format!("{}:0", source_length - 1)
            };
            let input_slice = if source_ascending {
                format!(
                    "{}:{}",
                    input_start,
                    input_start + slice_length - 1
                )
            } else {
                format!(
                    "{}:{}",
                    source_length - 1 - input_start,
                    source_length - input_start - slice_length
                )
            };
            let output_slice = if result_ascending {
                format!(
                    "{}:{}",
                    output_start,
                    output_start + slice_length - 1
                )
            } else {
                format!(
                    "{}:{}",
                    source_length - 1 - output_start,
                    source_length - output_start - slice_length
                )
            };

            let mut expected_storage = vec![0_u64; source_length];
            for relative in 0..slice_length {
                let source_element =
                    (input_value >> ((input_start + relative) * width)) & element_mask;
                let result_element = match operator {
                    "+" => source_element.wrapping_add(constant) & element_mask,
                    "^" => source_element ^ (constant & element_mask),
                    "&" => source_element & (constant & element_mask),
                    _ => source_element | (constant & element_mask),
                };
                expected_storage[output_start + relative] = result_element;
            }
            let expected = expected_storage
                .iter()
                .enumerate()
                .fold(0_u64, |accumulated, (element, value)| {
                    accumulated | (value << (element * width))
                })
                & total_mask;

            let element_range = format!("{}:0", width - 1);
            let total_range = format!("{}:0", total_width - 1);
            let child_array_range = format!("0:{}", slice_length - 1);
            let connections = if named {
                format!(
                    ".in_mem(source[{input_slice}]), \
                     .out_mem(result[{output_slice}])"
                )
            } else {
                format!("source[{input_slice}], result[{output_slice}]")
            };
            let source = format!(
                "module child( \
                 input logic [{element_range}] in_mem [{child_array_range}], \
                 output logic [{element_range}] out_mem [{child_array_range}]); \
                 generate for (genvar i = 0; i < {slice_length}; i = i + 1) \
                 begin : slice_transform \
                 assign out_mem[i] = in_mem[i] {operator} {width}'d{constant}; \
                 end endgenerate \
                 endmodule \
                 module top(input logic [{total_range}] a, \
                 output logic [{total_range}] y); \
                 logic [{element_range}] source [{source_range}]; \
                 logic [{element_range}] result [{result_range}]; \
                 assign source = a; \
                 initial result = {total_width}'d0; \
                 child c({connections}); \
                 assign y = result; \
                 endmodule",
                element_range = element_range,
                child_array_range = child_array_range,
                slice_length = slice_length,
                operator = operator,
                width = width,
                constant = constant,
                total_range = total_range,
                source_range = source_range,
                result_range = result_range,
                total_width = total_width,
                connections = connections
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            let child = design
                .hierarchy
                .iter()
                .find(|instance| instance.module == "child")
                .ok_or("missing slice-port child hierarchy record")?;
            let expected_child_width = width * slice_length;
            if child.ports.len() != 2
                || child
                    .ports
                    .iter()
                    .any(|port| port.width != expected_child_width)
            {
                return Err(format!(
                    "child slice-port widths {:?}, expected two ports of width {expected_child_width}",
                    child.ports.iter().map(|port| port.width).collect::<Vec<_>>()
                ));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("a", &LogicWord::from_u64(total_width, input_value))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("array-slice port output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "source={input_value} width={width} source_length={source_length} \
                     slice_length={slice_length} input_start={input_start} \
                     output_start={output_start} operator={operator} named={named} \
                     result={actual}, expected={expected}"
                ));
            }
            Ok(())
        });
    }
}
pub(super) fn signed_synthesis_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_signed_synth_{index:03}"), move || {
            let width = 2 + index % 7;
            let narrow_width = width - 1;
            let mask = (1_u64 << width) - 1;
            let min = -(1_i64 << (width - 1));
            let max = (1_i64 << (width - 1)) - 1;
            let a = min + (index as i64 * 19 + 7) % (max - min + 1);
            let b = min + (index as i64 * 31 + 13) % (max - min + 1);
            let narrow_min = -(1_i64 << (narrow_width - 1));
            let narrow_max = (1_i64 << (narrow_width - 1)) - 1;
            let c = narrow_min + (index as i64 * 23 + 3) % (narrow_max - narrow_min + 1);
            let shift = index % 8;
            let variant = index % 12;
            let comparison = (3..=6).contains(&variant) || variant >= 10;
            let output_width = if comparison { 1 } else { width };
            let output_mask = (1_u64 << output_width) - 1;
            let expected = match variant {
                0 => a.wrapping_add(b),
                1 => a.wrapping_sub(b),
                2 => a.wrapping_mul(b),
                3 => (a < b) as i64,
                4 => (a <= b) as i64,
                5 => (a > b) as i64,
                6 => (a >= b) as i64,
                7 => a >> shift,
                8 => a.wrapping_add(c),
                9 => {
                    if a < b {
                        a
                    } else {
                        b
                    }
                }
                10 => (a == b) as i64,
                _ => (a != b) as i64,
            };
            let expected_bits = (expected as u64) & output_mask;
            let raw_a = (a as u64) & mask;
            let raw_b = (b as u64) & mask;
            let raw_c = (c as u64) & ((1_u64 << narrow_width) - 1);
            let range = format!("{}:0", width - 1);
            let narrow_range = format!("{}:0", narrow_width - 1);
            let output_range = format!("{}:0", output_width - 1);
            let expression = match variant {
                0 => "a + b".to_string(),
                1 => "a - b".to_string(),
                2 => "a * b".to_string(),
                3 => "a < b".to_string(),
                4 => "a <= b".to_string(),
                5 => "a > b".to_string(),
                6 => "a >= b".to_string(),
                7 => format!("a >> {shift}"),
                8 => "$signed(a) + $signed(c)".to_string(),
                9 => "(a < b) ? a : b".to_string(),
                10 => "a == b".to_string(),
                _ => "a != b".to_string(),
            };
            let source = format!(
                "module top(input logic signed [{range}] a, b, \
                 input logic signed [{narrow_range}] c, \
                 output logic signed [{output_range}] y); \
                 assign y = {expression}; \
                 endmodule",
                output_range = output_range,
                narrow_range = narrow_range,
                expression = expression
            );
            let rtl =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            let synthesized = synthesize(&rtl, &SynthOptions::default())
                .map_err(|error| format!("synthesis error: {error}"))?;
            if synthesized.netlist.stats().seq != 0 {
                return Err(format!(
                    "sequential cell count {}, expected 0",
                    synthesized.netlist.stats().seq
                ));
            }
            let gate = netlist_to_design(&synthesized.netlist);
            let mut rtl_sim = Simulator::new(&rtl)?;
            let mut gate_sim = Simulator::new(&gate)?;
            for simulator in [&mut rtl_sim, &mut gate_sim] {
                simulator.set_input("a", &LogicWord::from_u64(width, raw_a))?;
                simulator.set_input("b", &LogicWord::from_u64(width, raw_b))?;
                simulator.set_input("c", &LogicWord::from_u64(narrow_width, raw_c))?;
            }
            let actual_rtl = rtl_sim
                .read_port("y")
                .and_then(|word| word.to_u64())
                .ok_or("RTL signed synthesis output is unknown")?;
            let actual_gate = gate_sim
                .read_port("y")
                .and_then(|word| word.to_u64())
                .ok_or("gate signed synthesis output is unknown")?;
            if actual_rtl != expected_bits || actual_gate != expected_bits {
                return Err(format!(
                    "variant={variant} a={a} b={b} c={c} shift={shift} \
                     rtl={actual_rtl:#x} gate={actual_gate:#x}, \
                     expected={expected_bits:#x}"
                ));
            }
            Ok(())
        });
    }
}
