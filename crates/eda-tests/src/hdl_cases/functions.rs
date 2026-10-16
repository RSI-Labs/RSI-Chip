use crate::CaseList;
use eda_core::ir::Expr;
use eda_core::logic::LogicWord;
use eda_hdl::compile;
use eda_sim::Simulator;

pub(super) fn register(c: &mut CaseList) {
    package_structure(c);
    enum_typedef_structure(c);
    packed_struct_structure(c);
    packed_union_structure(c);
    function_structure(c);
    package_function_structure(c);
    nested_function_structure(c);
    function_local_structure(c);
    function_if_structure(c);
    function_case_structure(c);
    function_for_structure(c);
    function_wildcard_case_structure(c);
    function_repeat_structure(c);
    function_while_structure(c);
    function_aggregate_local_structure(c);
    function_array_local_structure(c);
    multidim_function_array_local_structure(c);
}

fn package_structure(c: &mut CaseList) {
    for index in 1_usize..=100 {
        c.add(format!("sv_package_constant_{index:03}"), move || {
            let source = format!(
                r#"
package base;
parameter WIDTH = {index};
endpackage

package bus;
parameter WIDTH = base::WIDTH + 1;
localparam DEPTH = base::WIDTH + 3;
endpackage

module top(
input [bus::WIDTH-1:0] a,
output [bus::DEPTH-1:0] d
);
import bus::WIDTH, bus::DEPTH;
wire [WIDTH-1:0] wide;
assign d = wide;
endmodule
"#
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            let checks = [("a", index + 1), ("d", index + 3), ("wide", index + 1)];
            for (name, width) in checks {
                let declaration = design
                    .net_decls
                    .iter()
                    .find(|declaration| declaration.name == name)
                    .ok_or_else(|| format!("missing declaration {name}"))?;
                if declaration.width != width {
                    return Err(format!(
                        "{name} width {}, expected {width}",
                        declaration.width
                    ));
                }
            }
            Ok(())
        });
    }
}

fn enum_typedef_structure(c: &mut CaseList) {
    for index in 1_usize..=100 {
        c.add(format!("sv_enum_typedef_{index:03}"), move || {
            let width = 1 + index % 8;
            let base = (index as u64) & ((1_u64 << width) - 1);
            let explicit_range = index % 2 == 0;
            let package_type = index % 3 == 0;
            let range = if explicit_range {
                format!("[{}:0]", width - 1)
            } else {
                String::new()
            };
            let (file_prefix, module_prefix) = if package_type {
                let definition = format!(
                    "package states; parameter BASE = {base}; \
                         typedef enum {range} {{ A = BASE, B, C }} state_t; endpackage\n"
                );
                let import = if index % 2 == 0 {
                    "import states::*;\n".to_string()
                } else {
                    "import states::state_t;\n".to_string()
                };
                (format!("{definition}{import}"), String::new())
            } else {
                (
                    String::new(),
                    format!(
                        "parameter BASE = {base}; \
                         typedef enum {range} {{ A = BASE, B, C }} state_t;\n"
                    ),
                )
            };
            let source = format!(
                "{file_prefix}module top(output state_t q); \
                 {module_prefix}\
                 state_t state; \
                 assign q = A; \
                 endmodule"
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            let expected_width = if explicit_range { width } else { 32 };
            for name in ["q", "state"] {
                let declaration = design
                    .net_decls
                    .iter()
                    .find(|declaration| declaration.name == name)
                    .ok_or_else(|| format!("missing declaration {name}"))?;
                if declaration.width != expected_width {
                    return Err(format!(
                        "{name} width {}, expected {expected_width}",
                        declaration.width
                    ));
                }
            }
            let q = design.port("q").ok_or("missing q port")?;
            let assignment = design
                .assigns
                .iter()
                .find(|assignment| assignment.lv.net == q.base)
                .ok_or("missing q assignment")?;
            if !matches!(&assignment.e, Expr::Const(value) if value.to_u64() == Some(base)) {
                return Err(format!("A did not elaborate to {base}"));
            }
            Ok(())
        });
    }
}

fn packed_struct_structure(c: &mut CaseList) {
    for index in 1_usize..=100 {
        c.add(format!("sv_packed_struct_{index:03}"), move || {
            let high_width = 1 + index % 24;
            let total_width = high_width + 2;
            let value = ((index as u64 % 256) | ((index as u64 / 7) << 2))
                & ((1_u64 << total_width) - 1);
            let expected_low = value & 3;
            let expected_high = value >> 2;
            let package_type = index % 3 == 0;
            let named_import = index % 2 == 1;
            let definition = format!(
                "typedef struct packed {{ logic [{}:0] high; logic [1:0] low; }} packet_t;",
                high_width - 1
            );
            let file_prefix = if package_type {
                let import = if named_import {
                    "import bus::packet_t;\n"
                } else {
                    "import bus::*;\n"
                };
                format!("package bus; {definition} endpackage\n{import}")
            } else {
                String::new()
            };
            let module_definition = if package_type {
                String::new()
            } else {
                format!("{definition}\n")
            };
            let source = format!(
                "{file_prefix}module top( \
                 input logic [{}:0] value, \
                 output logic [{}:0] high, \
                 output logic [1:0] low); \
                 {module_definition}\
                 packet_t packet; \
                 assign packet = value; \
                 assign high = packet.high; \
                 assign low = packet.low; \
                 endmodule",
                total_width - 1,
                high_width - 1
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.port("packet").is_some() {
                return Err("struct object unexpectedly became a port".into());
            }
            let declaration = design
                .net_decls
                .iter()
                .find(|declaration| declaration.name == "packet")
                .ok_or("missing packet declaration")?;
            if declaration.width != total_width {
                return Err(format!(
                    "packet width {}, expected {total_width}",
                    declaration.width
                ));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("value", &LogicWord::from_u64(total_width, value))?;
            let actual_high = simulator
                .read("high")
                .and_then(|word| word.to_u64())
                .ok_or("high is unknown")?;
            let actual_low = simulator
                .read("low")
                .and_then(|word| word.to_u64())
                .ok_or("low is unknown")?;
            if actual_high != expected_high || actual_low != expected_low {
                return Err(format!(
                    "value={value} high={actual_high}/{expected_high} low={actual_low}/{expected_low}"
                ));
            }
            Ok(())
        });
    }
}

fn packed_union_structure(c: &mut CaseList) {
    for index in 1_usize..=100 {
        c.add(format!("sv_packed_union_{index:03}"), move || {
            let wide_width = 2 + index % 22;
            let value = (index as u64 * 17) & ((1_u64 << wide_width) - 1);
            let package_type = index % 4 == 0;
            let named_import = index % 2 == 0;
            let definition = format!(
                "typedef union packed {{ logic [{}:0] wide; logic [1:0] narrow; }} view_t;",
                wide_width - 1
            );
            let file_prefix = if package_type {
                let import = if named_import {
                    "import views::view_t;\n"
                } else {
                    "import views::*;\n"
                };
                format!("package views; {definition} endpackage\n{import}")
            } else {
                String::new()
            };
            let module_definition = if package_type {
                String::new()
            } else {
                format!("{definition}\n")
            };
            let source = format!(
                "{file_prefix}module top( \
                 input logic [{}:0] value, \
                 output logic [{}:0] wide, \
                 output logic [1:0] narrow); \
                 {module_definition}\
                 view_t view; \
                 assign view = value; \
                 assign wide = view.wide; \
                 assign narrow = view.narrow; \
                 endmodule",
                wide_width - 1,
                wide_width - 1
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            let declaration = design
                .net_decls
                .iter()
                .find(|declaration| declaration.name == "view")
                .ok_or("missing view declaration")?;
            if declaration.width != wide_width {
                return Err(format!(
                    "union width {}, expected {wide_width}",
                    declaration.width
                ));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("value", &LogicWord::from_u64(wide_width, value))?;
            let actual_wide = simulator
                .read("wide")
                .and_then(|word| word.to_u64())
                .ok_or("wide is unknown")?;
            let actual_narrow = simulator
                .read("narrow")
                .and_then(|word| word.to_u64())
                .ok_or("narrow is unknown")?;
            if actual_wide != value || actual_narrow != (value & 3) {
                return Err(format!(
                    "value={value} wide={actual_wide} narrow={actual_narrow}"
                ));
            }
            Ok(())
        });
    }
}

fn function_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_function_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let left = (index as u64 * 29) & mask;
            let right = (index as u64 * 11 + 7) & mask;
            let (operator, expected) = match index % 6 {
                0 => ("+", left.wrapping_add(right) & mask),
                1 => ("-", left.wrapping_sub(right) & mask),
                2 => ("&", left & right),
                3 => ("|", left | right),
                4 => ("^", left ^ right),
                _ => ("*", left.wrapping_mul(right) & mask),
            };
            let range = format!("{}:0", width - 1);
            let source = format!(
                "module top(input logic [{range}] a, b, output logic [{range}] y); \
                 function automatic logic [{range}] compute( \
                 input logic [{range}] left, right); \
                 compute = left {operator} right; \
                 endfunction \
                 assign y = compute(a, b); \
                 endmodule",
                range = range
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() != 1 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("a", &LogicWord::from_u64(width, left))?;
            simulator.set_input("b", &LogicWord::from_u64(width, right))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("function output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "left={left} right={right} op={operator} result={actual}, expected={expected}"
                ));
            }
            Ok(())
        });
    }
}

fn package_function_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_package_function_{index:03}"), move || {
            let width = 1 + index % 12;
            let mask = (1_u64 << width) - 1;
            let left = (index as u64 * 19 + 3) & mask;
            let right = (index as u64 * 7 + 11) & mask;
            let (operator, expected) = match index % 4 {
                0 => ("+", left.wrapping_add(right) & mask),
                1 => ("-", left.wrapping_sub(right) & mask),
                2 => ("&", left & right),
                _ => ("|", left | right),
            };
            let range = format!("{}:0", width - 1);
            let source = format!(
                "package math_pkg; \
                 function logic [{range}] compute( \
                 input logic [{range}] left, right); \
                 compute = left {operator} right; \
                 endfunction \
                 endpackage \
                 import math_pkg::*; \
                 module top(input logic [{range}] a, b, output logic [{range}] y); \
                 assign y = compute(a, b); \
                 endmodule",
                range = range
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() != 1 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("a", &LogicWord::from_u64(width, left))?;
            simulator.set_input("b", &LogicWord::from_u64(width, right))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("package function output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "left={left} right={right} op={operator} result={actual}, expected={expected}"
                ));
            }
            Ok(())
        });
    }
}

fn nested_function_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_nested_function_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let left = (index as u64 * 31 + 5) & mask;
            let right = (index as u64 * 17 + 13) & mask;
            let expected = match index % 4 {
                0 => left.wrapping_add(right.wrapping_mul(2)).wrapping_add(2) & mask,
                1 => left.wrapping_mul(2).wrapping_add(right).wrapping_add(2) & mask,
                2 => left.wrapping_add(right.wrapping_mul(2)).wrapping_add(3) & mask,
                _ => {
                    left.wrapping_mul(2)
                        .wrapping_add(right.wrapping_mul(2))
                        .wrapping_add(2)
                        & mask
                }
            };
            let range = format!("{}:0", width - 1);
            let call = match index % 4 {
                0 => "pipeline(increment(add(a, b)), b)",
                1 => "add(increment(pipeline(a, b)), a)",
                2 => "pipeline(add(a, increment(b)), increment(b))",
                _ => "increment(pipeline(add(a, b), add(b, a)))",
            };
            let source = format!(
                "module top(input logic [{range}] a, b, output logic [{range}] y); \
                 function logic [{range}] add( \
                 input logic [{range}] left, right); \
                 add = left + right; \
                 endfunction \
                 function logic [{range}] increment( \
                 input logic [{range}] value); \
                 increment = add(value, {width}'d1); \
                 endfunction \
                 function logic [{range}] pipeline( \
                 input logic [{range}] left, right); \
                 pipeline = increment(add(left, right)); \
                 endfunction \
                 assign y = {call}; \
                 endmodule",
                range = range,
                width = width,
                call = call
            );
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() != 1 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("a", &LogicWord::from_u64(width, left))?;
            simulator.set_input("b", &LogicWord::from_u64(width, right))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("nested function output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "left={left} right={right} variant={} result={actual}, expected={expected}",
                    index % 4
                ));
            }
            Ok(())
        });
    }
}

fn function_local_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_function_locals_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let left = (index as u64 * 37 + 2) & mask;
            let right = (index as u64 * 23 + 19) & mask;
            let constant = 1_u64 + index as u64 % 3;
            let expected = match index % 4 {
                0 => {
                    left.wrapping_mul(2)
                        .wrapping_add(right)
                        .wrapping_add(constant)
                        & mask
                }
                1 => {
                    left.wrapping_add(right.wrapping_mul(2))
                        .wrapping_add(constant)
                        & mask
                }
                2 => {
                    left.wrapping_mul(3)
                        .wrapping_add(right)
                        .wrapping_add(constant)
                        & mask
                }
                _ => {
                    left.wrapping_mul(2)
                        .wrapping_add(right.wrapping_mul(2))
                        .wrapping_add(constant)
                        & mask
                }
            };
            let range = format!("{}:0", width - 1);
            let biased_expression = match index % 4 {
                0 => "biased = add(biased, left);",
                1 => "biased = add(biased, right);",
                _ => "biased = add(biased, left);",
            };
            let return_call = match index % 4 {
                0 | 1 => "compute = add(first, biased);",
                2 => "compute = add(add(first, biased), left);",
                _ => "compute = add(add(biased, left), add(right, right));",
            };
            let functions = format!(
                "function logic [{range}] add( \
                 input logic [{range}] left, right); \
                 add = left + right; \
                 endfunction \
                 function logic [{range}] compute( \
                 input logic [{range}] left, right); \
                 logic [{range}] first; \
                 logic [{range}] biased = {width}'d{constant}; \
                 first = add(left, right); \
                 {biased_expression} \
                 {return_call} \
                 endfunction",
                range = range,
                width = width,
                constant = constant,
                biased_expression = biased_expression,
                return_call = return_call
            );
            let source = if index % 2 == 0 {
                let import = if index % 4 == 0 {
                    "import math_pkg::*;"
                } else {
                    "import math_pkg::add, math_pkg::compute;"
                };
                format!(
                    "package math_pkg; {functions} endpackage \
                     {import} \
                     module top(input logic [{range}] a, b, output logic [{range}] y); \
                     assign y = compute(a, b); \
                     endmodule",
                    functions = functions,
                    import = import,
                    range = range
                )
            } else {
                format!(
                    "module top(input logic [{range}] a, b, output logic [{range}] y); \
                     {functions} \
                     assign y = compute(a, b); \
                     endmodule",
                    functions = functions,
                    range = range
                )
            };
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() != 1 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("a", &LogicWord::from_u64(width, left))?;
            simulator.set_input("b", &LogicWord::from_u64(width, right))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("function-local output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "left={left} right={right} constant={constant} variant={} \
                     result={actual}, expected={expected}",
                    index % 4
                ));
            }
            Ok(())
        });
    }
}

fn function_if_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_function_if_else_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let selector = (index % 2) as u64;
            let alternate = ((index / 2) % 2) as u64;
            let left = (index as u64 * 41 + 7) & mask;
            let right = (index as u64 * 13 + 29) & mask;
            let constant = 1 + index as u64 % 3;
            let operator = match index % 4 {
                0 => "&",
                1 => "|",
                2 => "^",
                _ => "+",
            };
            let combine = |left: u64, right: u64| match operator {
                "&" => left & right,
                "|" => left | right,
                "^" => left ^ right,
                _ => left.wrapping_add(right) & mask,
            };
            let expected = match index % 4 {
                0 => {
                    if selector != 0 {
                        combine(left, right)
                    } else {
                        combine(right, constant)
                    }
                }
                1 => {
                    if selector != 0 {
                        combine(left, constant)
                    } else {
                        combine(right, constant)
                    }
                }
                2 => {
                    if selector != 0 {
                        combine(left, right)
                    } else if alternate != 0 {
                        combine(left, constant)
                    } else {
                        combine(right, constant)
                    }
                }
                _ => {
                    if selector != 0 {
                        combine(left, right)
                    } else {
                        combine(right, constant)
                    }
                }
            } & mask;
            let range = format!("{}:0", width - 1);
            let body = match index % 4 {
                0 => format!(
                    "logic [{range}] result; \
                     if (selector) result = left {operator} right; \
                     else result = right {operator} {width}'d{constant}; \
                     compute = result;",
                    range = range,
                    operator = operator,
                    width = width,
                    constant = constant
                ),
                1 => format!(
                    "if (selector) compute = left {operator} {width}'d{constant}; \
                     else compute = right {operator} {width}'d{constant};",
                    operator = operator,
                    width = width,
                    constant = constant
                ),
                2 => format!(
                    "logic [{range}] result; \
                     if (selector) result = left {operator} right; \
                     else if (alternate) result = left {operator} {width}'d{constant}; \
                     else result = right {operator} {width}'d{constant}; \
                     compute = result;",
                    range = range,
                    operator = operator,
                    width = width,
                    constant = constant
                ),
                _ => format!(
                    "logic [{range}] result = right {operator} {width}'d{constant}; \
                     if (selector) result = left {operator} right; \
                     compute = result;",
                    range = range,
                    operator = operator,
                    width = width,
                    constant = constant
                ),
            };
            let functions = format!(
                "function logic [{range}] compute( \
                 input logic selector, alternate, \
                 input logic [{range}] left, right); \
                 {body} \
                 endfunction",
                range = range,
                body = body
            );
            let source = if index % 2 == 0 {
                let import = if index % 4 == 0 {
                    "import control_pkg::*;"
                } else {
                    "import control_pkg::compute;"
                };
                format!(
                    "package control_pkg; {functions} endpackage \
                     {import} \
                     module top(input logic selector, alternate, \
                     input logic [{range}] a, b, output logic [{range}] y); \
                     assign y = compute(selector, alternate, a, b); \
                     endmodule",
                    functions = functions,
                    import = import,
                    range = range
                )
            } else {
                format!(
                    "module top(input logic selector, alternate, \
                     input logic [{range}] a, b, output logic [{range}] y); \
                     {functions} \
                     assign y = compute(selector, alternate, a, b); \
                     endmodule",
                    functions = functions,
                    range = range
                )
            };
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() != 1 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("selector", &LogicWord::from_u64(1, selector))?;
            simulator.set_input("alternate", &LogicWord::from_u64(1, alternate))?;
            simulator.set_input("a", &LogicWord::from_u64(width, left))?;
            simulator.set_input("b", &LogicWord::from_u64(width, right))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("function if/else output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "selector={selector} alternate={alternate} left={left} right={right} \
                     operator={operator} variant={} result={actual}, expected={expected}",
                    index % 4
                ));
            }
            Ok(())
        });
    }
}

fn function_case_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_function_case_{index:03}"), move || {
            let data_width = 1 + index % 16;
            let selector_width = 1 + index % 4;
            let data_mask = (1_u64 << data_width) - 1;
            let selector = (index as u64 * 3 + 1) & ((1_u64 << selector_width) - 1);
            let left = (index as u64 * 43 + 11) & data_mask;
            let right = (index as u64 * 19 + 31) & data_mask;
            let constant = 1 + index as u64 % 4;
            let operator = match index % 4 {
                0 => "&",
                1 => "|",
                2 => "^",
                _ => "+",
            };
            let combine = |left: u64, right: u64| match operator {
                "&" => left & right,
                "|" => left | right,
                "^" => left ^ right,
                _ => left.wrapping_add(right) & data_mask,
            };
            let expected = match index % 4 {
                0 | 1 => {
                    if selector == 0 {
                        combine(left, constant)
                    } else if selector == 1 || selector == 2 {
                        combine(right, constant)
                    } else {
                        combine(left, right)
                    }
                }
                2 => {
                    if selector == 0 {
                        combine(left, right)
                    } else if selector == 1 {
                        combine(left, constant)
                    } else {
                        combine(right, constant)
                    }
                }
                _ => {
                    if selector == 0 {
                        combine(left, right)
                    } else if selector == 1 {
                        combine(left, constant)
                    } else {
                        combine(right, left)
                    }
                }
            } & data_mask;
            let data_range = format!("{}:0", data_width - 1);
            let selector_range = format!("{}:0", selector_width - 1);
            let body = match index % 4 {
                0 => format!(
                    "logic [{data_range}] result; \
                     case (selector) \
                     {selector_width}'d0: result = left {operator} {data_width}'d{constant}; \
                     {selector_width}'d1, {selector_width}'d2: \
                     result = right {operator} {data_width}'d{constant}; \
                     default: result = left {operator} right; \
                     endcase \
                     compute = result;",
                    data_range = data_range,
                    selector_width = selector_width,
                    operator = operator,
                    data_width = data_width,
                    constant = constant
                ),
                1 => format!(
                    "case (selector) \
                     {selector_width}'d0: compute = left {operator} {data_width}'d{constant}; \
                     {selector_width}'d1, {selector_width}'d2: \
                     compute = right {operator} {data_width}'d{constant}; \
                     default: compute = left {operator} right; \
                     endcase",
                    selector_width = selector_width,
                    operator = operator,
                    data_width = data_width,
                    constant = constant
                ),
                2 => format!(
                    "logic [{data_range}] result = right {operator} {data_width}'d{constant}; \
                     case (selector) \
                     {selector_width}'d0: result = left {operator} right; \
                     {selector_width}'d1: result = left {operator} {data_width}'d{constant}; \
                     endcase \
                     compute = result;",
                    data_range = data_range,
                    selector_width = selector_width,
                    operator = operator,
                    data_width = data_width,
                    constant = constant
                ),
                _ => format!(
                    "logic [{data_range}] result; \
                     case (selector) \
                     {selector_width}'d0: result = left {operator} right; \
                     {selector_width}'d1: if (selector == {selector_width}'d1) \
                     result = left {operator} {data_width}'d{constant}; \
                     else result = right {operator} {data_width}'d{constant}; \
                     default: result = right {operator} left; \
                     endcase \
                     compute = result;",
                    data_range = data_range,
                    selector_width = selector_width,
                    operator = operator,
                    data_width = data_width,
                    constant = constant
                ),
            };
            let functions = format!(
                "function logic [{data_range}] compute( \
                 input logic [{selector_range}] selector, \
                 input logic [{data_range}] left, right); \
                 {body} \
                 endfunction",
                data_range = data_range,
                selector_range = selector_range,
                body = body
            );
            let source = if index % 2 == 0 {
                let import = if index % 4 == 0 {
                    "import case_pkg::*;"
                } else {
                    "import case_pkg::compute;"
                };
                format!(
                    "package case_pkg; {functions} endpackage \
                     {import} \
                     module top(input logic [{selector_range}] selector, \
                     input logic [{data_range}] a, b, output logic [{data_range}] y); \
                     assign y = compute(selector, a, b); \
                     endmodule",
                    functions = functions,
                    import = import,
                    selector_range = selector_range,
                    data_range = data_range
                )
            } else {
                format!(
                    "module top(input logic [{selector_range}] selector, \
                     input logic [{data_range}] a, b, output logic [{data_range}] y); \
                     {functions} \
                     assign y = compute(selector, a, b); \
                     endmodule",
                    selector_range = selector_range,
                    data_range = data_range,
                    functions = functions
                )
            };
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() != 1 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("selector", &LogicWord::from_u64(selector_width, selector))?;
            simulator.set_input("a", &LogicWord::from_u64(data_width, left))?;
            simulator.set_input("b", &LogicWord::from_u64(data_width, right))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("function case output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "selector={selector} left={left} right={right} operator={operator} \
                     variant={} result={actual}, expected={expected}",
                    index % 4
                ));
            }
            Ok(())
        });
    }
}

fn function_for_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_function_for_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let left = (index as u64 * 47 + 5) & mask;
            let right = (index as u64 * 29 + 17) & mask;
            let constant = 1 + index as u64 % 4;
            let count = 1 + index % 8;
            let outer = 1 + index % 4;
            let inner = 1 + (index / 4) % 4;
            let expected = match index % 4 {
                0 => {
                    right.wrapping_add(
                        (left.wrapping_add(constant) & mask).wrapping_mul(count as u64),
                    ) & mask
                }
                1 => right.wrapping_mul(1_u64 + (outer as u64 * inner as u64)) & mask,
                2 => right.wrapping_add(constant) & mask,
                _ => {
                    left.wrapping_mul(2)
                        .wrapping_add(constant.wrapping_mul(count.saturating_sub(1) as u64))
                        & mask
                }
            };
            let range = format!("{}:0", width - 1);
            let body = match index % 4 {
                0 => format!(
                    "logic [{range}] total = right; \
                     for (int i = 0; i < {count}; i = i + 1) \
                     total = total + left + {width}'d{constant}; \
                     compute = total;",
                    range = range,
                    count = count,
                    width = width,
                    constant = constant
                ),
                1 => format!(
                    "logic [{range}] total = right; \
                     for (int i = 0; i < {outer}; i = i + 1) \
                     for (int j = 0; j < {inner}; j = j + 1) \
                     total = total + right; \
                     compute = total;",
                    range = range,
                    outer = outer,
                    inner = inner
                ),
                2 => format!(
                    "logic [{range}] total = right; \
                     for (int i = 0; i < 0; i = i + 1) \
                     total = total + left; \
                     compute = total + {width}'d{constant};",
                    range = range,
                    width = width,
                    constant = constant
                ),
                _ => format!(
                    "logic [{range}] total = left; \
                     for (int i = 0; i < {count}; i = i + 1) \
                     if (i == 0) total = total + left; \
                     else total = total + {width}'d{constant}; \
                     compute = total;",
                    range = range,
                    count = count,
                    width = width,
                    constant = constant
                ),
            };
            let functions = format!(
                "function logic [{range}] compute( \
                 input logic [{range}] left, right); \
                 {body} \
                 endfunction",
                range = range,
                body = body
            );
            let source = if index % 2 == 0 {
                let import = if index % 4 == 0 {
                    "import loop_pkg::*;"
                } else {
                    "import loop_pkg::compute;"
                };
                format!(
                    "package loop_pkg; {functions} endpackage \
                     {import} \
                     module top(input logic [{range}] a, b, output logic [{range}] y); \
                     assign y = compute(a, b); \
                     endmodule",
                    functions = functions,
                    import = import,
                    range = range
                )
            } else {
                format!(
                    "module top(input logic [{range}] a, b, output logic [{range}] y); \
                     {functions} \
                     assign y = compute(a, b); \
                     endmodule",
                    functions = functions,
                    range = range
                )
            };
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() != 1 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("a", &LogicWord::from_u64(width, left))?;
            simulator.set_input("b", &LogicWord::from_u64(width, right))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("function for-loop output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "left={left} right={right} constant={constant} count={count} \
                     variant={} result={actual}, expected={expected}",
                    index % 4
                ));
            }
            Ok(())
        });
    }
}

fn function_wildcard_case_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_function_wildcard_case_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let selector = index as u64 % 4;
            let left = (index as u64 * 53 + 3) & mask;
            let right = (index as u64 * 27 + 23) & mask;
            let constant = 1 + index as u64 % 4;
            let operator = match index % 4 {
                0 => "&",
                1 => "|",
                2 => "^",
                _ => "+",
            };
            let combine = |left: u64, right: u64| match operator {
                "&" => left & right,
                "|" => left | right,
                "^" => left ^ right,
                _ => left.wrapping_add(right) & mask,
            };
            let expected = match index % 4 {
                0 => {
                    if selector == 0 || selector == 2 {
                        combine(left, constant)
                    } else if selector == 1 {
                        combine(right, constant)
                    } else {
                        combine(left, right)
                    }
                }
                1 => {
                    if selector < 2 {
                        combine(left, right)
                    } else {
                        combine(right, constant)
                    }
                }
                2 => {
                    if selector & 1 != 0 {
                        combine(right, constant)
                    } else {
                        combine(left, constant)
                    }
                }
                _ => {
                    if selector & 1 != 0 {
                        combine(left, right)
                    } else {
                        combine(right, constant)
                    }
                }
            } & mask;
            let range = format!("{}:0", width - 1);
            let body = match index % 4 {
                0 => format!(
                    "logic [{range}] result; \
                         casez (selector) \
                         2'b?0: result = left {operator} {width}'d{constant}; \
                         2'b01: result = right {operator} {width}'d{constant}; \
                         default: result = left {operator} right; \
                         endcase \
                         compute = result;",
                    range = range,
                    operator = operator,
                    width = width,
                    constant = constant
                ),
                1 => format!(
                    "logic [{range}] result; \
                         casex (selector) \
                         2'b0?: result = left {operator} right; \
                         2'b1?: result = right {operator} {width}'d{constant}; \
                         default: result = left {operator} right; \
                         endcase \
                         compute = result;",
                    range = range,
                    operator = operator,
                    width = width,
                    constant = constant
                ),
                2 => format!(
                    "logic [{range}] result = left {operator} {width}'d{constant}; \
                         casez (selector) \
                         2'b?1: result = right {operator} {width}'d{constant}; \
                         endcase \
                         compute = result;",
                    range = range,
                    operator = operator,
                    width = width,
                    constant = constant
                ),
                _ => format!(
                    "logic [{range}] result; \
                         casez (selector) \
                         2'b?1: result = left {operator} right; \
                         2'b01: result = left {operator} {width}'d{constant}; \
                         default: result = right {operator} {width}'d{constant}; \
                         endcase \
                         compute = result;",
                    range = range,
                    operator = operator,
                    width = width,
                    constant = constant
                ),
            };
            let functions = format!(
                "function logic [{range}] compute( \
                     input logic [1:0] selector, \
                     input logic [{range}] left, right); \
                     {body} \
                     endfunction",
                range = range,
                body = body
            );
            let source = if index % 2 == 0 {
                let import = if index % 4 == 0 {
                    "import wildcard_pkg::*;"
                } else {
                    "import wildcard_pkg::compute;"
                };
                format!(
                    "package wildcard_pkg; {functions} endpackage \
                         {import} \
                         module top(input logic [1:0] selector, \
                         input logic [{range}] a, b, output logic [{range}] y); \
                         assign y = compute(selector, a, b); \
                         endmodule",
                    functions = functions,
                    import = import,
                    range = range
                )
            } else {
                format!(
                    "module top(input logic [1:0] selector, \
                         input logic [{range}] a, b, output logic [{range}] y); \
                         {functions} \
                         assign y = compute(selector, a, b); \
                         endmodule",
                    functions = functions,
                    range = range
                )
            };
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() != 1 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("selector", &LogicWord::from_u64(2, selector))?;
            simulator.set_input("a", &LogicWord::from_u64(width, left))?;
            simulator.set_input("b", &LogicWord::from_u64(width, right))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("function wildcard-case output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "selector={selector} left={left} right={right} operator={operator} \
                         variant={} result={actual}, expected={expected}",
                    index % 4
                ));
            }
            Ok(())
        });
    }
}

fn function_repeat_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_function_repeat_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let selector = (index % 2) as u64;
            let left = (index as u64 * 59 + 13) & mask;
            let right = (index as u64 * 31 + 7) & mask;
            let constant = 1 + index as u64 % 4;
            let count = 1 + index % 8;
            let outer = 1 + index % 4;
            let inner = 1 + (index / 4) % 4;
            let expected = match index % 4 {
                0 => {
                    right.wrapping_add(
                        (left.wrapping_add(constant) & mask).wrapping_mul(count as u64),
                    ) & mask
                }
                1 => right.wrapping_mul(1_u64 + (outer as u64 * inner as u64)) & mask,
                2 => left.wrapping_mul(2).wrapping_add(constant) & mask,
                _ => {
                    if selector != 0 {
                        right.wrapping_add(left.wrapping_mul(count as u64)) & mask
                    } else {
                        right.wrapping_add(constant.wrapping_mul(count as u64)) & mask
                    }
                }
            };
            let range = format!("{}:0", width - 1);
            let body = match index % 4 {
                0 => format!(
                    "logic [{range}] total = right; \
                     repeat ({count}) total = total + left + {width}'d{constant}; \
                     compute = total;",
                    range = range,
                    count = count,
                    width = width,
                    constant = constant
                ),
                1 => format!(
                    "logic [{range}] total = right; \
                     repeat ({outer}) repeat ({inner}) total = total + right; \
                     compute = total;",
                    range = range,
                    outer = outer,
                    inner = inner
                ),
                2 => format!(
                    "logic [{range}] total = left; \
                     repeat (0) total = total + right; \
                     total = total + left; \
                     compute = total + {width}'d{constant};",
                    range = range,
                    width = width,
                    constant = constant
                ),
                _ => format!(
                    "logic [{range}] total = right; \
                     repeat ({count}) \
                     if (selector) total = total + left; \
                     else total = total + {width}'d{constant}; \
                     compute = total;",
                    range = range,
                    count = count,
                    width = width,
                    constant = constant
                ),
            };
            let functions = format!(
                "function logic [{range}] compute( \
                 input logic selector, \
                 input logic [{range}] left, right); \
                 {body} \
                 endfunction",
                range = range,
                body = body
            );
            let source = if index % 2 == 0 {
                let import = if index % 4 == 0 {
                    "import repeat_pkg::*;"
                } else {
                    "import repeat_pkg::compute;"
                };
                format!(
                    "package repeat_pkg; {functions} endpackage \
                     {import} \
                     module top(input logic selector, \
                     input logic [{range}] a, b, output logic [{range}] y); \
                     assign y = compute(selector, a, b); \
                     endmodule",
                    functions = functions,
                    import = import,
                    range = range
                )
            } else {
                format!(
                    "module top(input logic selector, \
                     input logic [{range}] a, b, output logic [{range}] y); \
                     {functions} \
                     assign y = compute(selector, a, b); \
                     endmodule",
                    functions = functions,
                    range = range
                )
            };
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() != 1 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("selector", &LogicWord::from_u64(1, selector))?;
            simulator.set_input("a", &LogicWord::from_u64(width, left))?;
            simulator.set_input("b", &LogicWord::from_u64(width, right))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("function repeat output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "selector={selector} left={left} right={right} constant={constant} \
                     count={count} variant={} result={actual}, expected={expected}",
                    index % 4
                ));
            }
            Ok(())
        });
    }
}

fn function_while_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_function_while_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let selector = (index % 2) as u64;
            let left = (index as u64 * 61 + 9) & mask;
            let right = (index as u64 * 37 + 11) & mask;
            let constant = 1 + index as u64 % 4;
            let count = 1 + index % 8;
            let outer = 1 + index % 4;
            let inner = 1 + (index / 4) % 4;
            let expected = match index % 4 {
                0 => {
                    right.wrapping_add(
                        (left.wrapping_add(constant) & mask).wrapping_mul(count as u64),
                    ) & mask
                }
                1 => right.wrapping_mul(1_u64 + (outer as u64 * inner as u64)) & mask,
                2 => left.wrapping_add(right).wrapping_add(constant) & mask,
                _ => {
                    if selector != 0 {
                        right.wrapping_add(left.wrapping_mul(count as u64)) & mask
                    } else {
                        right.wrapping_add(constant.wrapping_mul(count as u64)) & mask
                    }
                }
            };
            let range = format!("{}:0", width - 1);
            let body = match index % 4 {
                0 => format!(
                    "logic [{range}] total = right; \
                     int counter = 0; \
                     while (counter < {count}) begin \
                     total = total + left + {width}'d{constant}; \
                     counter = counter + 1; \
                     end \
                     compute = total;",
                    range = range,
                    count = count,
                    width = width,
                    constant = constant
                ),
                1 => format!(
                    "logic [{range}] total = right; \
                     int outer = 0, inner = 0; \
                     while (outer < {outer}) begin \
                     inner = 0; \
                     while (inner < {inner_count}) begin \
                     total = total + right; \
                     inner = inner + 1; \
                     end \
                     outer = outer + 1; \
                     end \
                     compute = total;",
                    range = range,
                    outer = outer,
                    inner_count = inner
                ),
                2 => format!(
                    "logic [{range}] total = left + right; \
                     int counter = 0; \
                     while (counter < 0) begin \
                     total = total + left; \
                     counter = counter + 1; \
                     end \
                     compute = total + {width}'d{constant};",
                    range = range,
                    width = width,
                    constant = constant
                ),
                _ => format!(
                    "logic [{range}] total = right; \
                     int counter = 0; \
                     while (counter < {count}) begin \
                     if (selector) total = total + left; \
                     else total = total + {width}'d{constant}; \
                     counter = counter + 1; \
                     end \
                     compute = total;",
                    range = range,
                    count = count,
                    width = width,
                    constant = constant
                ),
            };
            let functions = format!(
                "function logic [{range}] compute( \
                 input logic selector, \
                 input logic [{range}] left, right); \
                 {body} \
                 endfunction",
                range = range,
                body = body
            );
            let source = if index % 2 == 0 {
                let import = if index % 4 == 0 {
                    "import while_pkg::*;"
                } else {
                    "import while_pkg::compute;"
                };
                format!(
                    "package while_pkg; {functions} endpackage \
                     {import} \
                     module top(input logic selector, \
                     input logic [{range}] a, b, output logic [{range}] y); \
                     assign y = compute(selector, a, b); \
                     endmodule",
                    functions = functions,
                    import = import,
                    range = range
                )
            } else {
                format!(
                    "module top(input logic selector, \
                     input logic [{range}] a, b, output logic [{range}] y); \
                     {functions} \
                     assign y = compute(selector, a, b); \
                     endmodule",
                    functions = functions,
                    range = range
                )
            };
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() != 1 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("selector", &LogicWord::from_u64(1, selector))?;
            simulator.set_input("a", &LogicWord::from_u64(width, left))?;
            simulator.set_input("b", &LogicWord::from_u64(width, right))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("function while output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "selector={selector} left={left} right={right} constant={constant} \
                     count={count} variant={} result={actual}, expected={expected}",
                    index % 4
                ));
            }
            Ok(())
        });
    }
}

fn function_aggregate_local_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_function_struct_local_{index:03}"), move || {
            let high_width = 1 + index % 16;
            let low_width = 1 + index % 4;
            let total_width = high_width + low_width;
            let high_mask = (1_u64 << high_width) - 1;
            let low_mask = (1_u64 << low_width) - 1;
            let total_mask = (1_u64 << total_width) - 1;
            let selector = (index % 2) as u64;
            let left = (index as u64 * 71 + 6) & high_mask;
            let right = (index as u64 * 43 + 18) & high_mask;
            let low_right = right & low_mask;
            let constant = 1 + index as u64 % 3;
            let operator = match index % 4 {
                0 => "&",
                1 => "|",
                2 => "^",
                _ => "+",
            };
            let combine = |left: u64, right: u64| match operator {
                "&" => left & right,
                "|" => left | right,
                "^" => left ^ right,
                _ => left.wrapping_add(right) & total_mask,
            };
            let expected = match index % 4 {
                0 => ((left & high_mask) << low_width) | low_right,
                1 => left.wrapping_add(low_right) & total_mask,
                2 => {
                    let low = if selector != 0 {
                        constant & low_mask
                    } else {
                        low_right
                    };
                    combine(left, low)
                }
                _ => low_right.wrapping_add(left) & high_mask,
            };
            let high_range = format!("{}:0", high_width - 1);
            let low_range = format!("{}:0", low_width - 1);
            let total_range = format!("{}:0", total_width - 1);
            let helper = format!(
                "function logic [{high_range}] add( \
                 input logic [{high_range}] left, right); \
                 add = left + right; \
                 endfunction",
                high_range = high_range
            );
            let type_definition = format!(
                "typedef struct packed {{ \
                 logic [{high_range}] high; \
                 logic [{low_range}] low; \
                 }} packet_t;",
                high_range = high_range,
                low_range = low_range
            );
            let body = match index % 4 {
                0 => "packet_t packet; packet.high = left; packet.low = right; compute = packet;"
                    .to_string(),
                1 => "packet_t packet; packet.high = left; packet.low = right; \
                      compute = packet.high + packet.low;"
                    .to_string(),
                2 => format!(
                    "packet_t packet; \
                     packet.high = left; \
                     packet.low = right; \
                     if (selector) packet.low = {low_width}'d{constant}; \
                     compute = packet.high {operator} packet.low;",
                    low_width = low_width,
                    constant = constant,
                    operator = operator
                ),
                _ => "packet_t packet; packet.low = right; \
                      packet.high = add(packet.low, left); compute = packet.high;"
                    .to_string(),
            };
            let functions = format!(
                "{helper} \
                 function logic [{total_range}] compute( \
                 input logic selector, \
                 input logic [{high_range}] left, right); \
                 {body} \
                 endfunction",
                helper = helper,
                total_range = total_range,
                high_range = high_range,
                body = body
            );
            let source = if index % 2 == 0 {
                let import = if index % 4 == 0 {
                    "import aggregate_pkg::*;"
                } else {
                    "import aggregate_pkg::packet_t; import aggregate_pkg::compute;"
                };
                format!(
                    "package aggregate_pkg; {type_definition} {functions} endpackage \
                     {import} \
                     module top(input logic selector, \
                     input logic [{high_range}] a, b, output logic [{total_range}] y); \
                     assign y = compute(selector, a, b); \
                     endmodule",
                    type_definition = type_definition,
                    functions = functions,
                    import = import,
                    high_range = high_range,
                    total_range = total_range
                )
            } else {
                format!(
                    "module top(input logic selector, \
                     input logic [{high_range}] a, b, output logic [{total_range}] y); \
                     {type_definition} \
                     {functions} \
                     assign y = compute(selector, a, b); \
                     endmodule",
                    high_range = high_range,
                    total_range = total_range,
                    type_definition = type_definition,
                    functions = functions
                )
            };
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() != 1 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("selector", &LogicWord::from_u64(1, selector))?;
            simulator.set_input("a", &LogicWord::from_u64(high_width, left))?;
            simulator.set_input("b", &LogicWord::from_u64(high_width, right))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("function struct-local output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "selector={selector} left={left} right={right} high_width={high_width} \
                     low_width={low_width} variant={} result={actual}, expected={expected}",
                    index % 4
                ));
            }
            Ok(())
        });
    }
}

fn function_array_local_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_function_array_local_{index:03}"), move || {
            let width = 1 + index % 16;
            let mask = (1_u64 << width) - 1;
            let selector = (index % 2) as u64;
            let length = 2 + index % 8;
            let left = (index as u64 * 67 + 8) & mask;
            let right = (index as u64 * 41 + 21) & mask;
            let constant = 1 + index as u64 % 5;
            let expected = match index % 4 {
                0 | 2 => {
                    let base = left.wrapping_add(right) & mask;
                    (0..length as u64).fold(0_u64, |sum, i| {
                        sum.wrapping_add(base.wrapping_add(i) & mask)
                    }) & mask
                }
                1 => {
                    let third = if selector != 0 { left } else { constant };
                    left.wrapping_add(right).wrapping_add(third) & mask
                }
                _ => {
                    right
                        .wrapping_mul(length.saturating_sub(1) as u64)
                        .wrapping_add(left)
                        & mask
                }
            };
            let range = format!("{}:0", width - 1);
            let array_range = match index % 4 {
                2 => format!("{}:0", length - 1),
                _ => format!("0:{}", length - 1),
            };
            let mut sum_terms = String::new();
            for element in 0..length {
                if element > 0 {
                    sum_terms.push_str(" + ");
                }
                sum_terms.push_str(&format!("lookup[{element}]"));
            }
            let body = match index % 4 {
                0 | 2 => format!(
                    "logic [{range}] lookup [{array_range}]; \
                     for (int i = 0; i < {length}; i = i + 1) \
                     lookup[i] = left + right + i; \
                     compute = {sum_terms};",
                    range = range,
                    array_range = array_range,
                    length = length,
                    sum_terms = sum_terms
                ),
                1 => format!(
                    "logic [{range}] lookup [0:2]; \
                     lookup[0] = left; \
                     lookup[1] = right; \
                     lookup[2] = {width}'d{constant}; \
                     if (selector) lookup[2] = left; \
                     compute = lookup[0] + lookup[1] + lookup[2];",
                    range = range,
                    width = width,
                    constant = constant
                ),
                _ => format!(
                    "logic [{range}] lookup [{array_range}]; \
                     for (int i = 0; i < {length}; i = i + 1) \
                     if (i == 0) lookup[i] = left; \
                     else lookup[i] = right; \
                     compute = {sum_terms};",
                    range = range,
                    array_range = array_range,
                    length = length,
                    sum_terms = sum_terms
                ),
            };
            let functions = format!(
                "function logic [{range}] compute( \
                 input logic selector, \
                 input logic [{range}] left, right); \
                 {body} \
                 endfunction",
                range = range,
                body = body
            );
            let source = if index % 2 == 0 {
                let import = if index % 4 == 0 {
                    "import array_pkg::*;"
                } else {
                    "import array_pkg::compute;"
                };
                format!(
                    "package array_pkg; {functions} endpackage \
                     {import} \
                     module top(input logic selector, \
                     input logic [{range}] a, b, output logic [{range}] y); \
                     assign y = compute(selector, a, b); \
                     endmodule",
                    functions = functions,
                    import = import,
                    range = range
                )
            } else {
                format!(
                    "module top(input logic selector, \
                     input logic [{range}] a, b, output logic [{range}] y); \
                     {functions} \
                     assign y = compute(selector, a, b); \
                     endmodule",
                    functions = functions,
                    range = range
                )
            };
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() != 1 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("selector", &LogicWord::from_u64(1, selector))?;
            simulator.set_input("a", &LogicWord::from_u64(width, left))?;
            simulator.set_input("b", &LogicWord::from_u64(width, right))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("function array-local output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "selector={selector} left={left} right={right} length={length} \
                     variant={} result={actual}, expected={expected}",
                    index % 4
                ));
            }
            Ok(())
        });
    }
}

fn multidim_function_array_local_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_function_array2d_local_{index:03}"), move || {
            let width = 1 + index % 6;
            let mask = (1_u64 << width) - 1;
            let selector = (index % 2) as u64;
            let first_length = 2 + index % 2;
            let second_length = 2 + (index / 2) % 2;
            let length = first_length * second_length;
            let total_width = width * length;
            let left = (index as u64 * 89 + 17) & mask;
            let right = (index as u64 * 53 + 29) & mask;
            let constant = 1 + index as u64 % 5;
            let first_ascending = index % 4 < 2;
            let second_ascending = index % 8 < 4;
            let first_indexes = if first_ascending {
                (0..first_length as i64).collect::<Vec<_>>()
            } else {
                (0..first_length as i64).rev().collect::<Vec<_>>()
            };
            let second_indexes = if second_ascending {
                (0..second_length as i64).collect::<Vec<_>>()
            } else {
                (0..second_length as i64).rev().collect::<Vec<_>>()
            };
            let mut storage_indexes = Vec::with_capacity(length);
            for first in &first_indexes {
                for second in &second_indexes {
                    storage_indexes.push((*first, *second));
                }
            }

            let mut expected = 0_u64;
            for (storage_offset, (first, second)) in storage_indexes.iter().enumerate() {
                let result = match index % 4 {
                    0 => left ^ right,
                    1 => left
                        .wrapping_add(
                            (*first as u64).wrapping_mul(second_length as u64) + *second as u64,
                        )
                        & mask,
                    2 => {
                        if storage_offset == 0 {
                            if selector != 0 {
                                left & constant
                            } else {
                                left | constant
                            }
                        } else {
                            right
                        }
                    }
                    _ => {
                        if (first + second) % 2 == 0 {
                            left ^ constant
                        } else {
                            right
                                .wrapping_add(
                                    (*first as u64).wrapping_mul(second_length as u64)
                                        + *second as u64,
                                )
                                & mask
                        }
                    }
                } & mask;
                let shift = (length - 1 - storage_offset) * width;
                expected |= result << shift;
            }

            let range = format!("{}:0", width - 1);
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
            let first_base = first_indexes[0];
            let second_base = second_indexes[0];
            let assignments = match index % 4 {
                0 => storage_indexes
                    .iter()
                    .map(|(first, second)| {
                        format!("lookup[{first}][{second}] = left ^ right;")
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
                1 => format!(
                    "for (int i = 0; i < {first_length}; i = i + 1) \
                     for (int j = 0; j < {second_length}; j = j + 1) \
                     lookup[i][j] = left + i * {second_length} + j;"
                ),
                2 => format!(
                    "for (int i = 0; i < {first_length}; i = i + 1) \
                     for (int j = 0; j < {second_length}; j = j + 1) \
                     lookup[i][j] = right; \
                     if (selector) lookup[{first_base}][{second_base}] = left & {width}'d{constant}; \
                     else lookup[{first_base}][{second_base}] = left | {width}'d{constant};"
                ),
                _ => format!(
                    "for (int i = 0; i < {first_length}; i = i + 1) \
                     for (int j = 0; j < {second_length}; j = j + 1) \
                     if ((i + j) % 2 == 0) lookup[i][j] = left ^ {width}'d{constant}; \
                     else lookup[i][j] = right + ((i * {second_length}) + j);"
                ),
            };
            let functions = format!(
                "function logic [{total_range}] compute( \
                 input logic selector, \
                 input logic [{range}] left, right); \
                 logic [{range}] lookup [{first_range}][{second_range}]; \
                 {assignments} \
                 compute = lookup; \
                 endfunction",
                total_range = total_range,
                range = range,
                first_range = first_range,
                second_range = second_range,
                assignments = assignments
            );
            let source = if index % 2 == 0 {
                let import = if index % 4 == 0 {
                    "import array2d_pkg::*;"
                } else {
                    "import array2d_pkg::compute;"
                };
                format!(
                    "package array2d_pkg; {functions} endpackage \
                     {import} \
                     module top(input logic selector, \
                     input logic [{range}] a, b, output logic [{total_range}] y); \
                     assign y = compute(selector, a, b); \
                     endmodule",
                    functions = functions,
                    import = import,
                    range = range,
                    total_range = total_range
                )
            } else {
                format!(
                    "module top(input logic selector, \
                     input logic [{range}] a, b, output logic [{total_range}] y); \
                     {functions} \
                     assign y = compute(selector, a, b); \
                     endmodule",
                    range = range,
                    total_range = total_range,
                    functions = functions
                )
            };
            let design =
                compile(&source, Some("top")).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() != 1 {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("selector", &LogicWord::from_u64(1, selector))?;
            simulator.set_input("a", &LogicWord::from_u64(width, left))?;
            simulator.set_input("b", &LogicWord::from_u64(width, right))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("2D function array-local output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "selector={selector} left={left} right={right} \
                     shape={first_length}x{second_length} width={width} variant={} \
                     result={actual}, expected={expected}",
                    index % 4
                ));
            }
            Ok(())
        });
    }
}
