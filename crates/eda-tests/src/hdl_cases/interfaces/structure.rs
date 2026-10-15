//! Interface and structural instance-array regression cases.

use crate::CaseList;
use eda_core::logic::LogicWord;
use eda_hdl::compile;
use eda_sim::Simulator;

pub(super) fn register(c: &mut CaseList) {
    interface_structure(c);
    interface_array_structure(c);
    multidim_interface_array_structure(c);
    module_instance_array_structure(c);
    multidim_module_instance_array_structure(c);
    virtual_interface_structure(c);
    assertion_structure(c);
    signed_arithmetic_structure(c);
}

fn interface_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_interface_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let selector = (index % 2) as u64;
            let value = (index as u64 * 73 + 10) & mask;
            let constant = 1 + index as u64 % 5;
            let expected = match index % 4 {
                0 => value,
                1 => value.wrapping_add(constant) & mask,
                2 => constant & mask,
                _ => {
                    if selector != 0 {
                        value
                    } else {
                        constant & mask
                    }
                }
            };
            let range = format!("{}:0", width - 1);
            let producer_body = match index % 4 {
                0 => "assign b.valid = 1'b1; assign b.data = value;".to_string(),
                1 => format!(
                    "assign b.valid = 1'b1; assign b.data = value + {width}'d{constant};",
                    width = width,
                    constant = constant
                ),
                2 => format!(
                    "assign b.valid = 1'b1; assign b.data = value; \
                     assign b.aux = value ^ {width}'d{constant};",
                    width = width,
                    constant = constant
                ),
                _ => format!(
                    "assign b.valid = selector; assign b.data = value; \
                     assign b.aux = {width}'d{constant};",
                    width = width,
                    constant = constant
                ),
            };
            let consumer_body = match index % 4 {
                0 => format!(
                    "assign result = b.valid ? b.data : {width}'d0;",
                    width = width
                ),
                1 => format!(
                    "assign result = b.valid ? b.data : {width}'d0;",
                    width = width
                ),
                2 => format!(
                    "assign result = b.valid ? (b.data ^ b.aux) : {width}'d0;",
                    width = width
                ),
                _ => "assign result = b.valid ? b.data : b.aux;".to_string(),
            };
            let producer_ports = "interface bus.master b, input logic selector, \
                                  input logic [range] value"
                .replace("range", &range);
            let source = format!(
                "interface bus; \
                 logic [{range}] data; \
                 logic [{range}] aux; \
                 logic valid; \
                 modport master(output valid, data, aux); \
                 modport slave(input valid, data, aux); \
                 endinterface \
                 module producer({producer_ports}); \
                 {producer_body} \
                 endmodule \
                 module consumer(interface bus.slave b, \
                 output logic [{range}] result); \
                 {consumer_body} \
                 endmodule \
                 module top(input logic selector, input logic [{range}] value, \
                 output logic [{range}] result); \
                 bus shared(); \
                 producer p(.b(shared), .selector(selector), .value(value)); \
                 consumer c(.b(shared), .result(result)); \
                 endmodule",
                range = range,
                producer_ports = producer_ports,
                producer_body = producer_body,
                consumer_body = consumer_body
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() < 2 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("selector", &LogicWord::from_u64(1, selector))?;
            simulator.set_input("value", &LogicWord::from_u64(width, value))?;
            let actual = simulator
                .read("result")
                .and_then(|word| word.to_u64())
                .ok_or("interface output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "selector={selector} value={value} constant={constant} variant={} \
                     result={actual}, expected={expected}",
                    index % 4
                ));
            }
            Ok(())
        });
    }
}

fn interface_array_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_interface_array_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let length = 2 + index % 4;
            let ascending = index % 2 == 0;
            let first_element = if index % 4 < 2 { 0 } else { length - 2 };
            let second_element = length - 1;
            let first_value = (index as u64 * 79 + 20) & mask;
            let second_value = (index as u64 * 43 + 31) & mask;
            let range = format!("{}:0", width - 1);
            let array_range = if ascending {
                format!("0:{}", length - 1)
            } else {
                format!("{}:0", length - 1)
            };
            let source = format!(
                "interface bus; \
                 logic [{range}] data; \
                 logic valid; \
                 modport source(output valid, data); \
                 modport sink(input valid, data); \
                 endinterface \
                 module producer(interface bus.source b, input logic [{range}] value); \
                 assign b.valid = 1'b1; assign b.data = value; \
                 endmodule \
                 module consumer(interface bus.sink b, output logic [{range}] result); \
                 assign result = b.valid ? b.data : {width}'d0; \
                 endmodule \
                 module top(input logic [{range}] first, second, \
                 output logic [{range}] first_result, second_result); \
                 bus shared[{array_range}](); \
                 producer p0(.b(shared[{first_element}]), .value(first)); \
                 producer p1(.b(shared[{second_element}]), .value(second)); \
                 consumer c0(.b(shared[{first_element}]), .result(first_result)); \
                 consumer c1(.b(shared[{second_element}]), .result(second_result)); \
                 endmodule",
                range = range,
                width = width,
                array_range = array_range,
                first_element = first_element,
                second_element = second_element
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            let interface_elements = design
                .hierarchy
                .iter()
                .filter(|instance| {
                    instance.module == "bus"
                        && (instance.path.ends_with(&format!("shared[{first_element}]"))
                            || instance
                                .path
                                .ends_with(&format!("shared[{second_element}]")))
                })
                .count();
            if interface_elements < 2 {
                return Err(format!(
                    "expected selected interface elements, found {interface_elements}"
                ));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("first", &LogicWord::from_u64(width, first_value))?;
            simulator.set_input("second", &LogicWord::from_u64(width, second_value))?;
            let actual_first = simulator
                .read("first_result")
                .and_then(|word| word.to_u64())
                .ok_or("first interface-array output is unknown")?;
            let actual_second = simulator
                .read("second_result")
                .and_then(|word| word.to_u64())
                .ok_or("second interface-array output is unknown")?;
            if actual_first != first_value || actual_second != second_value {
                return Err(format!(
                    "first={actual_first}, expected={first_value}; \
                     second={actual_second}, expected={second_value}"
                ));
            }
            Ok(())
        });
    }
}

fn multidim_interface_array_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_interface_array2d_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let first_length = 2 + index % 2;
            let second_length = 2 + (index / 2) % 2;
            let first_ascending = index % 4 < 2;
            let second_ascending = index % 8 < 4;
            let first_index = if index % 3 == 0 { 0 } else { first_length - 1 };
            let second_index = if index % 2 == 0 { 1 } else { second_length - 1 };
            let other_first = first_length - 1 - first_index;
            let other_second = second_length - 1 - second_index;
            let first_value = (index as u64 * 91 + 26) & mask;
            let second_value = (index as u64 * 47 + 35) & mask;
            let range = format!("{}:0", width - 1);
            let first_dimension = if first_ascending {
                format!("0:{}", first_length - 1)
            } else {
                format!("{}:0", first_length - 1)
            };
            let second_dimension = if second_ascending {
                format!("0:{}", second_length - 1)
            } else {
                format!("{}:0", second_length - 1)
            };
            let source = format!(
                "interface bus; \
                 logic [{range}] data; \
                 logic valid; \
                 modport source(output valid, data); \
                 modport sink(input valid, data); \
                 endinterface \
                 module producer(interface bus.source b, input logic [{range}] value); \
                 assign b.valid = 1'b1; assign b.data = value; \
                 endmodule \
                 module consumer(interface bus.sink b, output logic [{range}] result); \
                 assign result = b.valid ? b.data : {width}'d0; \
                 endmodule \
                 module top(input logic [{range}] first, second, \
                 output logic [{range}] first_result, second_result); \
                 bus shared[{first_dimension}][{second_dimension}](); \
                 producer p0(.b(shared[{first_index}][{second_index}]), .value(first)); \
                 producer p1(.b(shared[{other_first}][{other_second}]), .value(second)); \
                 consumer c0(.b(shared[{first_index}][{second_index}]), .result(first_result)); \
                 consumer c1(.b(shared[{other_first}][{other_second}]), .result(second_result)); \
                 endmodule",
                range = range,
                width = width,
                first_dimension = first_dimension,
                second_dimension = second_dimension,
                first_index = first_index,
                second_index = second_index,
                other_first = other_first,
                other_second = other_second
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            let element_count = first_length * second_length;
            let hierarchy_count = design
                .hierarchy
                .iter()
                .filter(|instance| instance.module == "bus")
                .count();
            if hierarchy_count != element_count {
                return Err(format!(
                    "interface element hierarchy count {hierarchy_count}, expected {element_count}"
                ));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("first", &LogicWord::from_u64(width, first_value))?;
            simulator.set_input("second", &LogicWord::from_u64(width, second_value))?;
            let actual_first = simulator
                .read("first_result")
                .and_then(|word| word.to_u64())
                .ok_or("first 2D interface-array output is unknown")?;
            let actual_second = simulator
                .read("second_result")
                .and_then(|word| word.to_u64())
                .ok_or("second 2D interface-array output is unknown")?;
            if actual_first != first_value || actual_second != second_value {
                return Err(format!(
                    "first={actual_first}, expected={first_value}; \
                     second={actual_second}, expected={second_value}"
                ));
            }
            Ok(())
        });
    }
}

fn module_instance_array_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_module_array_{index:03}"), move || {
            let element_width = 1 + index % 8;
            let length = 2 + index % 4;
            let total_width = element_width * length;
            let total_mask = (1_u64 << total_width) - 1;
            let element_mask = (1_u64 << element_width) - 1;
            let a = (index as u64 * 103 + 30) & total_mask;
            let b = (index as u64 * 59 + 42) & total_mask;
            let ascending = index % 2 == 0;
            let named = index % 4 < 2;
            let operator = match index % 4 {
                0 => "&",
                1 => "|",
                2 => "^",
                _ => "+",
            };
            let mut expected = 0_u64;
            for bit in 0..length {
                let logical_element = if ascending { bit } else { length - 1 - bit };
                let physical_element = length - 1 - logical_element;
                let shift = physical_element * element_width;
                let left = (a >> shift) & element_mask;
                let right = (b >> shift) & element_mask;
                let result = match operator {
                    "&" => left & right,
                    "|" => left | right,
                    "^" => left ^ right,
                    _ => left.wrapping_add(right) & element_mask,
                };
                expected |= result << shift;
            }

            let element_range = format!("{}:0", element_width - 1);
            let total_range = format!("{}:0", total_width - 1);
            let array_range = if ascending {
                format!("0:{}", length - 1)
            } else {
                format!("{}:0", length - 1)
            };
            let connections = if named {
                ".a(a), .b(b), .y(y)".to_string()
            } else {
                "a, b, y".to_string()
            };
            let source = format!(
                "module leaf(input logic [{element_range}] a, b, \
                 output logic [{element_range}] y); \
                 assign y = a {operator} b; \
                 endmodule \
                 module top(input logic [{total_range}] a, b, \
                 output logic [{total_range}] y); \
                 leaf u[{array_range}]({connections}); \
                 endmodule",
                element_range = element_range,
                operator = operator,
                total_range = total_range,
                array_range = array_range,
                connections = connections
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            let leaf_count = design
                .hierarchy
                .iter()
                .filter(|instance| instance.module == "leaf")
                .count();
            if leaf_count != length {
                return Err(format!(
                    "leaf hierarchy count {leaf_count}, expected {length}"
                ));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("a", &LogicWord::from_u64(total_width, a))?;
            simulator.set_input("b", &LogicWord::from_u64(total_width, b))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("module-array output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "a={a} b={b} operator={operator} length={length} \
                     width={element_width} result={actual}, expected={expected}"
                ));
            }
            Ok(())
        });
    }
}

fn multidim_module_instance_array_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_module_array2d_{index:03}"), move || {
            let element_width = 1 + index % 4;
            let first_length = 2 + index % 2;
            let second_length = 2 + (index / 2) % 2;
            let length = first_length * second_length;
            let total_width = element_width * length;
            let total_mask = (1_u64 << total_width) - 1;
            let element_mask = (1_u64 << element_width) - 1;
            let a = (index as u64 * 107 + 32) & total_mask;
            let b = (index as u64 * 61 + 44) & total_mask;
            let first_ascending = index % 4 < 2;
            let second_ascending = index % 8 < 4;
            let named = index % 2 == 0;
            let operator = match index % 4 {
                0 => "&",
                1 => "|",
                2 => "^",
                _ => "+",
            };
            let mut expected = 0_u64;
            for element in 0..length {
                let shift = (length - 1 - element) * element_width;
                let left = (a >> shift) & element_mask;
                let right = (b >> shift) & element_mask;
                let result = match operator {
                    "&" => left & right,
                    "|" => left | right,
                    "^" => left ^ right,
                    _ => left.wrapping_add(right) & element_mask,
                };
                expected |= result << shift;
            }

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
            let connections = if named {
                ".a(a), .b(b), .y(y)".to_string()
            } else {
                "a, b, y".to_string()
            };
            let source = format!(
                "module leaf(input logic [{element_range}] a, b, \
                 output logic [{element_range}] y); \
                 assign y = a {operator} b; \
                 endmodule \
                 module top(input logic [{total_range}] a, b, \
                 output logic [{total_range}] y); \
                 leaf u[{first_range}][{second_range}]({connections}); \
                 endmodule",
                element_range = element_range,
                operator = operator,
                total_range = total_range,
                first_range = first_range,
                second_range = second_range,
                connections = connections
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            let leaf_count = design
                .hierarchy
                .iter()
                .filter(|instance| instance.module == "leaf")
                .count();
            if leaf_count != length {
                return Err(format!(
                    "leaf hierarchy count {leaf_count}, expected {length}"
                ));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("a", &LogicWord::from_u64(total_width, a))?;
            simulator.set_input("b", &LogicWord::from_u64(total_width, b))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("2D module-array output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "a={a} b={b} operator={operator} shape={first_length}x{second_length} \
                     width={element_width} result={actual}, expected={expected}"
                ));
            }
            Ok(())
        });
    }
}

fn virtual_interface_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_virtual_interface_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let selector = (index % 2) as u64;
            let value = (index as u64 * 87 + 24) & mask;
            let constant = 1 + index as u64 % 5;
            let expected = match index % 4 {
                0 => value,
                1 => value.wrapping_add(constant) & mask,
                2 => constant & mask,
                _ if selector != 0 => value,
                _ => constant & mask,
            };
            let range = format!("{}:0", width - 1);
            let producer_body = match index % 4 {
                0 => "assign b.valid = 1'b1; assign b.data = value;".to_string(),
                1 => format!(
                    "assign b.valid = 1'b1; assign b.data = value + {width}'d{constant};",
                    width = width,
                    constant = constant
                ),
                2 => format!(
                    "assign b.valid = 1'b1; assign b.data = value; \
                     assign b.aux = value ^ {width}'d{constant};",
                    width = width,
                    constant = constant
                ),
                _ => format!(
                    "assign b.valid = selector; assign b.data = value; \
                     assign b.aux = {width}'d{constant};",
                    width = width,
                    constant = constant
                ),
            };
            let consumer_body = match index % 4 {
                0 | 1 => "assign result = b.valid ? b.data : 'd0;".to_string(),
                2 => "assign result = b.valid ? (b.data ^ b.aux) : 'd0;".to_string(),
                _ => "assign result = b.valid ? b.data : b.aux;".to_string(),
            };
            let source = format!(
                "interface bus; \
                 logic [{range}] data; \
                 logic [{range}] aux; \
                 logic valid; \
                 modport source(output valid, data, aux); \
                 modport sink(input valid, data, aux); \
                 endinterface \
                 module producer(virtual interface bus.source b, \
                 input logic selector, input logic [{range}] value); \
                 {producer_body} \
                 endmodule \
                 module consumer(virtual interface bus.sink b, \
                 output logic [{range}] result); \
                 {consumer_body} \
                 endmodule \
                 module top(input logic selector, input logic [{range}] value, \
                 output logic [{range}] result); \
                 bus shared(); \
                 producer p(.b(shared), .selector(selector), .value(value)); \
                 consumer c(.b(shared), .result(result)); \
                 endmodule",
                range = range,
                producer_body = producer_body,
                consumer_body = consumer_body
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if !design.processes.is_empty() || design.assigns.len() < 2 {
                return Err("virtual interface design was not fully lowered".to_string());
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("selector", &LogicWord::from_u64(1, selector))?;
            simulator.set_input("value", &LogicWord::from_u64(width, value))?;
            let actual = simulator
                .read("result")
                .and_then(|word| word.to_u64())
                .ok_or("virtual-interface output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "selector={selector} value={value} variant={} result={actual}, expected={expected}",
                    index % 4
                ));
            }
            Ok(())
        });
    }
}

fn assertion_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_immediate_assert_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let expected;
            let (left, right, operator, should_fail) = match index % 4 {
                0 => {
                    let left = (index as u64 * 3) & mask;
                    let right = (left + 1) & mask;
                    expected = left & right;
                    (left, right, "<", false)
                }
                1 => {
                    let right = (index as u64 * 5) & mask;
                    let left = (right + 2) & mask;
                    expected = left & right;
                    (left, right, ">", false)
                }
                2 => {
                    let left = (index as u64 * 7 + 4) & mask;
                    expected = left;
                    (left, left, "==", false)
                }
                _ => {
                    let left = (index as u64 * 11 + 3) & mask;
                    expected = left;
                    (left, left, "!=", true)
                }
            };
            let source = format!(
                "module top(input logic [{range}] a, b, \
                 output logic [{range}] y); \
                 assign y = a & b; \
                 initial begin \
                 #1; \
                 assert (a {operator} b) else $error(\"assertion {index}\"); \
                 end \
                 endmodule",
                range = format!("{}:0", width - 1),
                operator = operator,
                index = index
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.initials.len() != 1 {
                return Err(format!("initial count {}", design.initials.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("a", &LogicWord::from_u64(width, left))?;
            simulator.set_input("b", &LogicWord::from_u64(width, right))?;
            let assertion_result = simulator.run_until(2);
            if should_fail {
                let error = assertion_result.err().ok_or("expected assertion failure")?;
                if !error.contains("assertion failed") || !error.contains(&index.to_string()) {
                    return Err(format!("unexpected assertion error: {error}"));
                }
            } else {
                assertion_result.map_err(|error| format!("unexpected assertion error: {error}"))?;
            }
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("assertion-case output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "left={left} right={right} result={actual}, expected={expected}"
                ));
            }
            Ok(())
        });
    }
}

fn signed_arithmetic_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_signed_arith_{index:03}"), move || {
            let width = 2 + index % 15;
            let mask = (1_u64 << width) - 1;
            let min = -(1_i64 << (width - 1));
            let max = (1_i64 << (width - 1)) - 1;
            let a = min + (index as i64 * 17 + 5) % (max - min + 1);
            let b = min + (index as i64 * 29 + 11) % (max - min + 1);
            let b = if index % 6 >= 4 && b == 0 { 1 } else { b };
            let expected = match index % 6 {
                0 => a.wrapping_add(b),
                1 => a.wrapping_sub(b),
                2 => a.min(b),
                3 => a.wrapping_add(b),
                4 => a / b,
                _ => a % b,
            };
            let expected_bits = (expected as u64) & mask;
            let raw_a = (a as u64) & mask;
            let raw_b = (b as u64) & mask;
            let range = format!("{}:0", width - 1);
            let expression = match index % 6 {
                0 => "a + b".to_string(),
                1 => "a - b".to_string(),
                2 => "(a < b) ? a : b".to_string(),
                3 => format!(
                    "$signed({width}'d{raw_a}) + $signed({width}'d{raw_b})",
                    width = width,
                    raw_a = raw_a,
                    raw_b = raw_b
                ),
                4 => "a / b".to_string(),
                _ => "a % b".to_string(),
            };
            let source = format!(
                "module top(input logic signed [{range}] a, b, \
                 output logic signed [{range}] y); \
                 assign y = {expression}; \
                 endmodule",
                range = range,
                expression = expression
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if !design
                .net_decls
                .iter()
                .all(|declaration| declaration.name == "y" || declaration.is_signed)
            {
                return Err("signed declaration metadata was lost".to_string());
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("a", &LogicWord::from_u64(width, raw_a))?;
            simulator.set_input("b", &LogicWord::from_u64(width, raw_b))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("signed arithmetic output is unknown")?;
            if actual != expected_bits {
                return Err(format!(
                    "a={a} b={b} variant={} result={actual}, expected={expected_bits}",
                    index % 6
                ));
            }
            Ok(())
        });
    }
}
