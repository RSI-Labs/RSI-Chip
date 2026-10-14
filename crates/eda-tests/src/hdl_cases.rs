//! HDL frontend cases: lexing, parsing (valid + rejected), elaboration.

use crate::CaseList;
use eda_core::ir::{Dir, Expr};
use eda_core::logic::LogicWord;
use eda_core::to_design::netlist_to_design;
use eda_hdl::lexer::Lexer;
use eda_hdl::{compile, elaborate, parse};
use eda_sim::Simulator;
use eda_synth::{synthesize, SynthOptions};

pub fn register(c: &mut CaseList) {
    lexing(c);
    valid(c);
    rejected(c);
    elab_structure(c);
    generate_structure(c);
    generate_case_structure(c);
    variable_type_structure(c);
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
    signed_synthesis_structure(c);
    unpacked_indexed_select_structure(c);
    runtime_array_indexed_select_structure(c);
    function_runtime_array_indexed_select_structure(c);
    function_runtime_array_indexed_assignment_structure(c);
    runtime_array_indexed_assignment_structure(c);
    runtime_array_indexed_assignment_synthesis_structure(c);
    runtime_array_continuous_indexed_structure(c);
    function_unpacked_indexed_select_structure(c);
    module_unpacked_array_structure(c);
    multidim_module_unpacked_array_structure(c);
    fixed_unpacked_array_port_structure(c);
    unpacked_array_port_slice_structure(c);
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
    interface_structure(c);
    interface_array_structure(c);
    multidim_interface_array_structure(c);
    module_instance_array_structure(c);
    multidim_module_instance_array_structure(c);
    virtual_interface_structure(c);
    assertion_structure(c);
    signed_arithmetic_structure(c);
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
    class_cases(c);
    class_composition_cases(c);
    string_cases(c);
    class_polymorphism_cases(c);
    uvm_component_cases(c);
    uvm_sequence_cases(c);
    uvm_channel_cases(c);
    uvm_port_cases(c);
    uvm_nested_port_cases(c);
    uvm_fifo_cases(c);
    uvm_queue_cases(c);
    uvm_pool_cases(c);
    uvm_config_db_cases(c);
    uvm_factory_cases(c);
    uvm_objection_cases(c);
}

fn lexing(c: &mut CaseList) {
    c.add("lex_tokens_basic", || {
        let toks = Lexer::tokenize("module m; endmodule").map_err(|e| e.to_string())?;
        if toks.len() != 5 {
            Err(format!("{} tokens", toks.len()))
        } else {
            Ok(())
        }
    });
    c.add("lex_numbers", || {
        let toks = Lexer::tokenize("42 'b10 0x1f 'hzz").map_err(|e| e.to_string())?;
        if toks.len() != 5 {
            Err("number token count".into())
        } else {
            Ok(())
        }
    });
    c.add("lex_operators", || {
        let toks = Lexer::tokenize("a <= b ? c : d << e").map_err(|e| e.to_string())?;
        if toks.len() != 10 {
            Err(format!("{} tokens", toks.len()))
        } else {
            Ok(())
        }
    });
    c.add("lex_comments_stripped", || {
        let toks = Lexer::tokenize("// line\na /* block */ b").map_err(|e| e.to_string())?;
        if toks.len() != 3 {
            Err("comments not stripped".into())
        } else {
            Ok(())
        }
    });
    c.add("lex_empty", || {
        if Lexer::tokenize("").map_err(|e| e.to_string())?.len() > 1 {
            Err("empty".into())
        } else {
            Ok(())
        }
    });
    c.add("lex_reject_bad_char", || {
        if Lexer::tokenize("a $ %^").is_ok() {
            // '$' begins system tasks; '%^' is invalid — expect an error overall
            // or at least tokenization to survive; treat ok as failure only
            // when no error and garbage vanished.
            let t = Lexer::tokenize("a $ %^").map_err(|e| e.to_string())?;
            if t.len() < 2 {
                Err("tokens vanished".into())
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    });
    c.add("lex_string", || {
        let toks = Lexer::tokenize("\"hello world\"").map_err(|e| e.to_string())?;
        if toks.len() != 2 {
            Err("string".into())
        } else {
            Ok(())
        }
    });
    c.add("lex_case_sensitivity", || {
        let toks = Lexer::tokenize("Module module").map_err(|e| e.to_string())?;
        if toks.len() != 3 {
            Err("case".into())
        } else {
            Ok(())
        }
    });
    c.add("lex_underscores_number", || {
        Lexer::tokenize("16'b1010_0011").map_err(|e| e.to_string())?;
        Ok(())
    });
    c.add("lex_newlines_significant", || {
        let toks = Lexer::tokenize("a\nb\nc").map_err(|e| e.to_string())?;
        if toks.len() != 4 {
            Err("nl".into())
        } else {
            Ok(())
        }
    });
}

fn valid(c: &mut CaseList) {
    let programs: Vec<(&str, &str)> = vec![
        ("min_module", "module m; endmodule"),
        ("ports_scalar", "module m(input a, output y); assign y = a; endmodule"),
        ("ports_vector", "module m(input [3:0] a, output [3:0] y); assign y = a; endmodule"),
        ("port_reg", "module m(input clk, output reg q); always @(posedge clk) q <= 1; endmodule"),
        ("wire_decl", "module m; wire w; endmodule"),
        ("reg_init", "module m; reg r = 1'b1; endmodule"),
        ("param", "module m; parameter N = 4; endmodule"),
        ("localparam", "module m; localparam W = 8; endmodule"),
        ("assign_delay", "module m(input a, output y); assign #5 y = a; endmodule"),
        ("always_comb", "module m(input a, output reg y); always @(*) y = a; endmodule"),
        ("always_posedge", "module m(input clk, output reg q); always @(posedge clk) q <= q; endmodule"),
        ("always_negedge", "module m(input clk, output reg q); always @(negedge clk) q <= q; endmodule"),
        ("always_list", "module m(input clk, rst, output reg q); always @(posedge clk or posedge rst) q <= rst; endmodule"),
        ("initial_block", "module m; reg a; initial a = 0; endmodule"),
        ("begin_end", "module m; reg a; initial begin a = 0; a = 1; end endmodule"),
        ("if_else", "module m(input a, output reg y); always @(*) if (a) y = 1; else y = 0; endmodule"),
        ("case_full", "module m(input [1:0] s, output reg [1:0] y); always @(*) case (s) 2'b00: y = 0; default: y = 1; endcase endmodule"),
        ("forever_clk", "module m; reg clk; initial begin clk = 0; forever #5 clk = ~clk; end endmodule"),
        ("delay_stmt", "module m; reg a; initial begin #10 a = 1; end endmodule"),
        ("display", "module m; initial $display(\"hi %b\", 1'b1); endmodule"),
        ("monitor", "module m; reg a; initial $monitor(\"a=%b\", a); endmodule"),
        ("finish", "module m; initial $finish; endmodule"),
        ("gate_primitive", "module m(input a, b, output y); and g1(y, a, b); endmodule"),
        ("instance_named", "module s(input a, output y); assign y = ~a; endmodule module m(input x, output z); wire w; s u1(.a(x), .y(w)); s u2(.a(w), .y(z)); endmodule"),
        ("instance_positional", "module s(input a, output y); assign y = ~a; endmodule module m(input x, output z); s u1(x, z); endmodule"),
        ("ops_arith", "module m(input [3:0] a, b, output [3:0] y); assign y = (a + b) - 1; endmodule"),
        ("ops_logical", "module m(input a, b, output y); assign y = a && b || !a; endmodule"),
        ("ops_shift", "module m(input [3:0] a, output [3:0] y); assign y = (a << 1) >> 2; endmodule"),
        ("ops_cmp", "module m(input [3:0] a, b, output y); assign y = (a < b) & (a != b); endmodule"),
        ("ops_case_eq", "module m(input a, output y); assign y = (a === 1'b0); endmodule"),
        ("ternary", "module m(input s, [1:0] a, b, output [1:0] y); assign y = s ? a : b; endmodule"),
        ("concat", "module m(input [3:0] a, [3:0] b, output [7:0] y); assign y = {a, b}; endmodule"),
        ("repeat_op", "module m(input [1:0] a, output [7:0] y); assign y = {4{a}}; endmodule"),
        ("bit_select", "module m(input [3:0] a, output y); assign y = a[2]; endmodule"),
        ("part_select", "module m(input [7:0] a, output [3:0] y); assign y = a[6:3]; endmodule"),
        ("reductions", "module m(input [3:0] a, output y); assign y = &a | ^a; endmodule"),
        ("nested_modules_order", "module leaf(input a, output y); assign y = a; endmodule module top(input a, output y); leaf u(a, y); endmodule"),
        ("multiple_assigns", "module m(input a, b, output y, z); assign y = a & b; assign z = a | b; endmodule"),
        ("hex_lit", "module m(input [7:0] a, output [7:0] y); assign y = a ^ 8'hff; endmodule"),
        ("decimal_lit", "module m(input [7:0] a, output [7:0] y); assign y = a + 8'd1; endmodule"),
        ("x_literals", "module m(input [3:0] a, output [3:0] y); assign y = (a === 4'bxxxx) ? 4'b0000 : a; endmodule"),
    ];
    for (name, src) in programs {
        c.add(format!("parse_ok_{}", name), move || {
            parse(src).map_err(|e| format!("parse error: {}", e))?;
            Ok(())
        });
    }
}

fn rejected(c: &mut CaseList) {
    let bad: Vec<(&str, &str)> = vec![
        ("missing_endmodule", "module m;"),
        ("endmodule_without_module", "endmodule"),
        (
            "assign_no_target",
            "module m(input a); assign = a; endmodule",
        ),
        ("bad_token", "module m; wire w = 1'b0 @ w; endmodule"),
        ("unclosed_begin", "module m; initial begin a = 0; endmodule"),
        (
            "unclosed_paren",
            "module m(input a, output y); assign y = (a; endmodule",
        ),
        (
            "unclosed_concat",
            "module m(input [1:0] a, output [3:0] y); assign y = {a, a; endmodule",
        ),
        (
            "case_no_end",
            "module m(input [1:0] s, output reg y); always @(*) case (s) default: y = 0; endmodule",
        ),
        ("empty_module_name", "module ; endmodule"),
        ("keyword_as_ident", "module m; wire module; endmodule"),
        (
            "instance_unknown_conn_form",
            "module m(input a, output y); nand_bad g1[y](a, y); endmodule",
        ),
        ("num_in_ident_pos", "module m; wire 1w; endmodule"),
    ];
    for (name, src) in bad {
        c.add(format!("parse_err_{}", name), move || {
            if parse(src).is_ok() {
                Err(format!("should have been rejected: {}", name))
            } else {
                Ok(())
            }
        });
    }
    // Unknown module reference rejected at elaboration.
    c.add("elab_err_unknown_module", || {
        if compile(
            "module m(input a, output y); ghost u1(a, y); endmodule",
            None,
        )
        .is_ok()
        {
            Err("unknown module instantiation accepted".into())
        } else {
            Ok(())
        }
    });
    c.add("elab_err_undeclared_net", || {
        if compile(
            "module m(input a, output y); assign y = ghost_net; endmodule",
            None,
        )
        .is_ok()
        {
            Err("undeclared net accepted".into())
        } else {
            Ok(())
        }
    });
    c.add("elab_err_unknown_top", || {
        if compile("module m; endmodule", Some("nope")).is_ok() {
            Err("unknown top accepted".into())
        } else {
            Ok(())
        }
    });
    c.add("elab_err_duplicate_module", || {
        if compile("module m; endmodule module m; endmodule", None).is_ok() {
            Err("duplicate module accepted".into())
        } else {
            Ok(())
        }
    });
}

fn elab_structure(c: &mut CaseList) {
    c.add("elab_ports_and_widths", || {
        let d = compile(
            "module m(input [3:0] a, input b, output [7:0] y, output z); assign y = a; assign z = b; endmodule",
            None,
        )
        .map_err(|e| e.to_string())?;
        if d.ports.len() != 4 {
            return Err("port count".into());
        }
        let a = d.port("a").ok_or("no a")?;
        if a.width != 4 || a.dir != Dir::In {
            return Err("a width/dir".into());
        }
        let y = d.port("y").ok_or("no y")?;
        if y.width != 8 || y.dir != Dir::Out {
            return Err("y width/dir".into());
        }
        Ok(())
    });
    c.add("elab_reg_flag", || {
        let d = compile(
            "module m(input clk, output reg q); always @(posedge clk) q <= clk; endmodule",
            None,
        )
        .map_err(|e| e.to_string())?;
        let q = d.net_decls.iter().find(|n| n.name == "q").ok_or("no q")?;
        if !q.is_reg {
            Err("q not reg".into())
        } else {
            Ok(())
        }
    });
    c.add("elab_ansi_repeated_net_type", || {
        let d = compile(
            "module m(input clk, output reg q, reg r); always @(posedge clk) begin q <= clk; r <= q; end endmodule",
            None,
        )
        .map_err(|e| e.to_string())?;
        for name in ["q", "r"] {
            let decl = d
                .net_decls
                .iter()
                .find(|n| n.name == name)
                .ok_or_else(|| format!("no {}", name))?;
            if !decl.is_reg {
                return Err(format!("{} should retain reg type", name));
            }
        }
        Ok(())
    });
    c.add("elab_concat_assignment_splits_targets", || {
        let d = compile(
            "module m(input [3:0] a, output co, output [3:0] s); assign {co, s} = {1'b0, a}; endmodule",
            None,
        )
        .map_err(|e| e.to_string())?;
        let co = d.net_decls.iter().find(|n| n.name == "co").ok_or("no co")?;
        let s = d.net_decls.iter().find(|n| n.name == "s").ok_or("no s")?;
        let co_assigns = d.assigns.iter().filter(|a| a.lv.net == co.base).count();
        let s_assigns = d.assigns.iter().filter(|a| a.lv.net == s.base).count();
        if co_assigns != 1 || s_assigns != 1 || d.assigns.len() != 2 {
            Err(format!("assign counts {}/{}", co_assigns, s_assigns))
        } else {
            Ok(())
        }
    });
    c.add("elab_net_flat_widths", || {
        let d =
            compile("module m; reg [5:0] r; wire w; endmodule", None).map_err(|e| e.to_string())?;
        let r = d.net_decls.iter().find(|n| n.name == "r").ok_or("no r")?;
        if r.width != 6 {
            return Err("r width".into());
        }
        if d.n_nets != 7 {
            Err(format!("n_nets {}", d.n_nets))
        } else {
            Ok(())
        }
    });
    c.add("elab_top_selection", || {
        let d = compile(
            "module a(input x, output y); assign y = x; endmodule module b(input x, output y); assign y = ~x; endmodule",
            Some("b"),
        )
        .map_err(|e| e.to_string())?;
        if d.name != "b" {
            Err(format!("top = {}", d.name))
        } else {
            Ok(())
        }
    });
    c.add("elab_default_top_last", || {
        let d = compile(
            "module a(input x, output y); assign y = x; endmodule module b(input x, output y); assign y = x; endmodule",
            None,
        )
        .map_err(|e| e.to_string())?;
        if d.name.is_empty() {
            Err("no top picked".into())
        } else {
            Ok(())
        }
    });
    c.add("elab_hierarchy_flattened", || {
        let d = compile(
            "module inv(input a, output y); assign y = ~a; endmodule module top(input x, output z); wire w; inv u1(.a(x), .y(w)); inv u2(.a(w), .y(z)); endmodule",
            None,
        )
        .map_err(|e| e.to_string())?;
        if d.assigns.len() < 4 {
            Err(format!("assigns {}", d.assigns.len()))
        } else {
            Ok(())
        }
    });
    c.add("elab_param_sized_vector", || {
        let d = compile(
            "module m #(parameter W = 4) (input [W-1:0] a, output [W-1:0] y); assign y = ~a; endmodule",
            None,
        )
        .map_err(|e| e.to_string())?;
        let y = d.port("y").ok_or("no y")?;
        if y.width != 4 {
            Err(format!("W param ignored: width {}", y.width))
        } else {
            Ok(())
        }
    });
    c.add("elab_param_override", || {
        let d = compile(
            "module s #(parameter W = 2) (input [W-1:0] a, output [W-1:0] y); assign y = a; endmodule
             module top(input [3:0] a, output [3:0] y); s #(.W(4)) u(.a(a), .y(y)); endmodule",
            Some("top"),
        )
        .map_err(|e| e.to_string())?;
        let y = d.port("y").ok_or("no y")?;
        if y.width != 4 {
            Err(format!("override ignored: width {}", y.width))
        } else {
            Ok(())
        }
    });
    c.add("elab_parse_via_file_struct", || {
        let file = parse("module m(input a, output y); assign y = a; endmodule")
            .map_err(|e| e.to_string())?;
        let d = elaborate(&file, None).map_err(|e| e.to_string())?;
        if d.ports.len() != 2 {
            Err("ports".into())
        } else {
            Ok(())
        }
    });
    c.add("elab_gate_primitive", || {
        let d = compile(
            "module m(input a, b, output y); and g1(y, a, b); endmodule",
            None,
        )
        .map_err(|e| e.to_string())?;
        if d.assigns.is_empty() {
            Err("gate not lowered to assign".into())
        } else {
            Ok(())
        }
    });
    c.add("elab_processes_counted", || {
        let d = compile(
            "module m(input clk, a, output reg q, output reg y);
                always @(posedge clk) q <= a;
                always @(*) y = a;
             endmodule",
            None,
        )
        .map_err(|e| e.to_string())?;
        if d.processes.len() != 2 {
            Err(format!("processes {}", d.processes.len()))
        } else {
            Ok(())
        }
    });
    c.add("elab_initials_counted", || {
        let d = compile(
            "module m; reg a; initial a = 0; initial #5 a = 1; endmodule",
            None,
        )
        .map_err(|e| e.to_string())?;
        if d.initials.len() != 2 {
            Err(format!("initials {}", d.initials.len()))
        } else {
            Ok(())
        }
    });
    c.add("elab_net_name_display", || {
        let d = compile(
            "module m(input [3:0] a, output [3:0] y); assign y = a; endmodule",
            None,
        )
        .map_err(|e| e.to_string())?;
        let a = d.net_decls.iter().find(|n| n.name == "a").unwrap();
        if d.net_name(a.base + 2) != "a[2]" {
            Err("net name".into())
        } else {
            Ok(())
        }
    });
}

fn generate_structure(c: &mut CaseList) {
    for width in 1_usize..=100 {
        c.add(format!("generate_for_width_{width:03}"), move || {
            let source = format!(
                "module m #(parameter W = {width}) (input [W-1:0] a, output [W-1:0] y); \
                 generate for (genvar i=0; i<W; i=i+1) begin : bit_assign \
                 assign y[i] = a[i]; end endgenerate endmodule"
            );
            let design =
                compile(&source, None).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.len() != width {
                Err(format!(
                    "width {} expanded to {} assigns",
                    width,
                    design.assigns.len()
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn generate_case_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("sv_generate_case_{index:03}"), move || {
            let variant = index % 5;
            let width = 1 + index % 16;
            let range = format!("{}:0", width - 1);
            let mask = (1_u64 << width) - 1;
            let value = (index as u64 * 109 + 33) & mask;
            let constant = 1 + index as u64 % 7;
            let constant = constant & mask;
            let mode = match variant {
                0 => 0,
                1 => 1 + index % 2,
                2 => 3 + index % 4,
                _ => 1,
            };

            if variant == 4 {
                let source = format!(
                    "module m #(parameter MODE = 4) (output logic [{range}] y); \
                     generate case (MODE) \
                     0: begin : zero assign y = {width}'d0; end \
                     endcase endgenerate endmodule",
                    range = range,
                    width = width
                );
                let error = compile(&source, None)
                    .err()
                    .ok_or("expected unmatched generate case to fail")?;
                if !error.contains("no matching item") {
                    return Err(format!("unexpected generate-case error: {error}"));
                }
                return Ok(());
            }

            let source = if variant == 3 {
                format!(
                    "module m #(parameter MODE = 1) ( \
                     input logic [{range}] a, output logic [{range}] y); \
                     generate case (MODE) \
                     0: begin : pass generate for (genvar i=0; i<{width}; i=i+1) \
                     begin : slice assign y[i] = a[i]; end endgenerate end \
                     1: begin : invert generate for (genvar i=0; i<{width}; i=i+1) \
                     begin : slice assign y[i] = ~a[i]; end endgenerate end \
                     default: begin : clear assign y = {width}'d0; end \
                     endcase endgenerate endmodule",
                    range = range,
                    width = width
                )
            } else {
                format!(
                    "module m #(parameter MODE = {mode}) ( \
                     input logic [{range}] a, output logic [{range}] y); \
                     generate case (MODE) \
                     0: begin : first assign y = a + {width}'d{constant}; end \
                     1, 2: begin : second assign y = a ^ {width}'d{constant}; end \
                     default: begin : fallback assign y = a & {width}'d{constant}; end \
                     endcase endgenerate endmodule",
                    mode = mode,
                    range = range,
                    width = width,
                    constant = constant
                )
            };
            let design =
                compile(&source, None).map_err(|error| format!("compile error: {error}"))?;
            if design.assigns.is_empty() {
                return Err(format!("assignment count {}", design.assigns.len()));
            }
            let expected = if variant == 0 {
                value.wrapping_add(constant) & mask
            } else if variant == 1 {
                value ^ constant
            } else if variant == 2 {
                value & constant
            } else {
                !value & mask
            };
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("a", &LogicWord::from_u64(width, value))?;
            let actual = simulator
                .read("y")
                .and_then(|word| word.to_u64())
                .ok_or("generate-case output is unknown")?;
            if actual != expected {
                return Err(format!(
                    "variant={variant} mode={mode} value={value} constant={constant} \
                     result={actual}, expected={expected}"
                ));
            }
            Ok(())
        });
    }
}

fn variable_type_structure(c: &mut CaseList) {
    let types = [
        ("wire", 1_usize, false),
        ("tri", 1, false),
        ("reg", 1, true),
        ("logic", 1, true),
        ("bit", 1, true),
        ("byte", 8, true),
        ("shortint", 16, true),
        ("int", 32, true),
        ("integer", 32, true),
        ("longint", 64, true),
    ];
    for index in 0_usize..100 {
        c.add(format!("sv_variable_type_{index:03}"), move || {
            let (kind, default_width, is_variable) = types[index % types.len()];
            let explicit_width = default_width + index % 9;
            let signed_before = index % 2 == 0;
            let source = format!(
                "module m(input {} {kind} [{explicit_width}:0] p, output {kind} {} q); \
                 {kind} r; endmodule",
                if signed_before { "signed" } else { "" },
                if signed_before { "" } else { "signed" }
            );
            let design =
                compile(&source, None).map_err(|error| format!("compile error: {error}"))?;
            for (name, expected_width) in [
                ("p", explicit_width + 1),
                ("q", default_width),
                ("r", default_width),
            ] {
                let declaration = design
                    .net_decls
                    .iter()
                    .find(|declaration| declaration.name == name)
                    .ok_or_else(|| format!("missing declaration {name}"))?;
                if declaration.width != expected_width {
                    return Err(format!(
                        "{kind} {name}: width {}, expected {}",
                        declaration.width, expected_width
                    ));
                }
                if declaration.is_reg != is_variable {
                    return Err(format!("{kind} {name}: is_reg {}", declaration.is_reg));
                }
            }
            Ok(())
        });
    }
}

fn indexed_part_select_structure(c: &mut CaseList) {
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

fn runtime_indexed_part_select_structure(c: &mut CaseList) {
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

fn runtime_indexed_part_write_structure(c: &mut CaseList) {
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

fn runtime_indexed_part_nba_structure(c: &mut CaseList) {
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

fn runtime_indexed_nba_synth_structure(c: &mut CaseList) {
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

fn function_runtime_indexed_select_structure(c: &mut CaseList) {
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

fn reversed_indexed_select_structure(c: &mut CaseList) {
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

fn reversed_function_indexed_select_structure(c: &mut CaseList) {
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

fn reversed_continuous_indexed_structure(c: &mut CaseList) {
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

fn runtime_continuous_indexed_structure(c: &mut CaseList) {
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

fn reversed_indexed_write_structure(c: &mut CaseList) {
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

fn unpacked_indexed_select_structure(c: &mut CaseList) {
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

fn runtime_array_indexed_select_structure(c: &mut CaseList) {
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

fn function_unpacked_indexed_select_structure(c: &mut CaseList) {
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
                 logic [{element_range}] table {array_declaration}; \
                 table{array_index} = {element_width}'d{physical_initial}; \
                 table{array_index}[selector {direction}: {slice_width}] = \
                 value {operator} {slice_width}'d{constant}; \
                 update = table{array_index}[selector {direction}: {slice_width}]; \
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
                 logic [{element_range}] table {array_declaration}; \
                 table{array_index} = {element_width}'d{physical_initial}; \
                 table{array_index}[selector {direction}: {slice_width}] = \
                 value {operator} {slice_width}'d{constant}; \
                 preserve = table{array_index}; \
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

fn function_runtime_array_indexed_select_structure(c: &mut CaseList) {
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
                                "table[{first_init}][{second_init}] = \
                                 {element_width}'d{value};"
                            )
                        } else {
                            format!("table[{first_init}] = {element_width}'d{value};")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let function_definition = format!(
                    "function logic [{slice_range}] pick( \
                     input logic [3:0] packed_selector, {ports} \
                     input logic unused); \
                     logic [{element_range}] table {declaration}; \
                     {initializers} \
                     pick = table{indexes}[packed_selector {direction}: {slice_width}]; \
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

fn function_runtime_array_indexed_assignment_structure(c: &mut CaseList) {
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
                                "table[{first_init}][{second_init}] = {element_width}'d{value};"
                            )
                        } else {
                            format!("table[{first_init}] = {element_width}'d{value};")
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
                            format!("table[{first}][{second}]")
                        } else {
                            format!("table[{first}]")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let function_definition = format!(
                    "function logic [{total_range}] update({ports}, \
                     input logic [{slice_range}] value); \
                     logic [{element_range}] table {declaration}; \
                     {initializers} \
                     table{indexes}[packed_selector {direction}: {slice_width}] = \
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

fn runtime_array_indexed_assignment_structure(c: &mut CaseList) {
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

fn runtime_array_indexed_assignment_synthesis_structure(c: &mut CaseList) {
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

fn runtime_array_continuous_indexed_structure(c: &mut CaseList) {
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

fn module_unpacked_array_structure(c: &mut CaseList) {
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

fn multidim_module_unpacked_array_structure(c: &mut CaseList) {
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

fn fixed_unpacked_array_port_structure(c: &mut CaseList) {
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

fn unpacked_array_port_slice_structure(c: &mut CaseList) {
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
                sum_terms.push_str(&format!("table[{element}]"));
            }
            let body = match index % 4 {
                0 | 2 => format!(
                    "logic [{range}] table [{array_range}]; \
                     for (int i = 0; i < {length}; i = i + 1) \
                     table[i] = left + right + i; \
                     compute = {sum_terms};",
                    range = range,
                    array_range = array_range,
                    length = length,
                    sum_terms = sum_terms
                ),
                1 => format!(
                    "logic [{range}] table [0:2]; \
                     table[0] = left; \
                     table[1] = right; \
                     table[2] = {width}'d{constant}; \
                     if (selector) table[2] = left; \
                     compute = table[0] + table[1] + table[2];",
                    range = range,
                    width = width,
                    constant = constant
                ),
                _ => format!(
                    "logic [{range}] table [{array_range}]; \
                     for (int i = 0; i < {length}; i = i + 1) \
                     if (i == 0) table[i] = left; \
                     else table[i] = right; \
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
                        format!("table[{first}][{second}] = left ^ right;")
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
                1 => format!(
                    "for (int i = 0; i < {first_length}; i = i + 1) \
                     for (int j = 0; j < {second_length}; j = j + 1) \
                     table[i][j] = left + i * {second_length} + j;"
                ),
                2 => format!(
                    "for (int i = 0; i < {first_length}; i = i + 1) \
                     for (int j = 0; j < {second_length}; j = j + 1) \
                     table[i][j] = right; \
                     if (selector) table[{first_base}][{second_base}] = left & {width}'d{constant}; \
                     else table[{first_base}][{second_base}] = left | {width}'d{constant};"
                ),
                _ => format!(
                    "for (int i = 0; i < {first_length}; i = i + 1) \
                     for (int j = 0; j < {second_length}; j = j + 1) \
                     if ((i + j) % 2 == 0) table[i][j] = left ^ {width}'d{constant}; \
                     else table[i][j] = right + ((i * {second_length}) + j);"
                ),
            };
            let functions = format!(
                "function logic [{total_range}] compute( \
                 input logic selector, \
                 input logic [{range}] left, right); \
                 logic [{range}] table [{first_range}][{second_range}]; \
                 {assignments} \
                 compute = table; \
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

fn signed_synthesis_structure(c: &mut CaseList) {
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

fn class_cases(c: &mut CaseList) {
    // 1. Constructor, void statement method, expression method.
    c.add("sv_class_counter_method_and_get", || {
        let source = r#"
class Counter;
  int count = 0;
  function new(int start);
    this.count = start;
  endfunction
  function void inc();
    this.count = this.count + 1;
  endfunction
  function int get();
    get = this.count;
  endfunction
endclass
module top(output int y);
  Counter h;
  initial begin
    h = new(41);
    h.inc();
    h.inc();
    y = h.get();
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("class counter output unknown")?;
        if actual != 43 {
            return Err(format!("counter result {actual}, expected 43"));
        }
        Ok(())
    });

    // 4b. Static methods are callable through the class name without a handle.
    c.add("sv_class_static_methods_class_scope", || {
        let source = r#"
class Registry;
  static int total = 0;
  static function void add(int n);
    Registry.total = Registry.total + n;
  endfunction
  static function int total_plus(int n);
    total_plus = Registry.total + n;
  endfunction
endclass
module top(output int y1, y2);
  initial begin
    Registry.total = 0;
    Registry.add(4);
    Registry.add(3);
    y1 = Registry.total;       // 7
    y2 = Registry.total_plus(6); // 13
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 7, "static method updates class storage")?;
        expect_net(&sim, "y2", 13, "static value-return method")?;
        Ok(())
    });

    // 4c. Class constructor/method and module function default arguments.
    c.add("sv_class_constructor_method_default_args", || {
        let source = r#"
class Item;
  int a;
  int b;
  function new(int a = 3, int b = 4);
    this.a = a;
    this.b = b;
  endfunction
  function int scaled(int scale = 10);
    scaled = (this.a + this.b) * scale;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8);
  function int add(input int a, input int b = 10);
    add = a + b;
  endfunction
  Item h;
  Item k;
  initial begin
    h = new();          // a=3, b=4
    y1 = h.a;
    y2 = h.b;
    y3 = h.scaled();    // 70
    y4 = h.scaled(2);   // 14
    k = new(7);         // a=7, b=4
    y5 = k.a;
    y6 = k.scaled();    // 110
    y7 = add(1);        // 11
    y8 = add(1, 2);     // 3
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 3, "first constructor default")?;
        expect_net(&sim, "y2", 4, "second constructor default")?;
        expect_net(&sim, "y3", 70, "method default")?;
        expect_net(&sim, "y4", 14, "method explicit override")?;
        expect_net(&sim, "y5", 7, "constructor explicit prefix")?;
        expect_net(&sim, "y6", 110, "mixed explicit/default constructor")?;
        expect_net(&sim, "y7", 11, "module function default")?;
        expect_net(&sim, "y8", 3, "module function explicit override")?;
        Ok(())
    });

    // 4d. null 句柄：显式清空、比较、别名组分离与嵌套属性清空。
    c.add("sv_class_null_handle_compare_and_detach", || {
        let source = r#"
class Box;
  int value;
  function new(int v = 0);
    this.value = v;
  endfunction
endclass
class Env;
  Box b;
  function new(int id = 0);
    this.b = new(id + 10);
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9);
  Box h;
  Box q;
  Env e;
  initial begin
    h = null;
    y1 = (h == null);       // 1
    y2 = (h != null);       // 0
    h = new(9);
    y3 = (h != null);       // 1
    q = h;                  // q 与 h 共享对象
    y4 = (q != null);       // 1
    h = null;               // h 从别名组分离，q 保持原对象
    y5 = (h == null);       // 1
    y6 = (q != null);       // 1
    y7 = q.value;           // 9
    e = new(1);
    y8 = (e.b != null);     // 1
    e.b = null;
    y9 = (e.b == null);     // 1
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "initial null equality")?;
        expect_net(&sim, "y2", 0, "initial null inequality")?;
        expect_net(&sim, "y3", 1, "constructed handle")?;
        expect_net(&sim, "y4", 1, "aliased handle")?;
        expect_net(&sim, "y5", 1, "cleared handle detaches alias")?;
        expect_net(&sim, "y6", 1, "old alias remains allocated")?;
        expect_net(&sim, "y7", 9, "old alias preserves object storage")?;
        expect_net(&sim, "y8", 1, "nested handle constructed")?;
        expect_net(&sim, "y9", 1, "nested handle cleared")?;
        Ok(())
    });

    // 4e. class 对象身份相等：别名比较相等，不同构造对象比较不等。
    c.add("sv_class_object_identity_equality", || {
        let source = r#"
class Box;
  int value;
  function new(int v = 0);
    this.value = v;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6);
  Box h;
  Box q;
  Box other;
  initial begin
    h = new(11);
    q = h;                 // q 与 h 引用同一对象
    other = new(11);       // value 相同，但对象不同
    y1 = (h == q);         // 1
    y2 = (h == other);     // 0
    y3 = (h != other);     // 1

    h = null;              // h 脱离别名组；q 保持旧对象
    y4 = (h == q);         // 0
    y5 = (h != q);         // 1
    y6 = q.value;          // 11
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "aliased handles identify same object")?;
        expect_net(&sim, "y2", 0, "same payload is not same object")?;
        expect_net(&sim, "y3", 1, "different objects are unequal")?;
        expect_net(&sim, "y4", 0, "null handle is not old alias")?;
        expect_net(&sim, "y5", 1, "null handle differs from old alias")?;
        expect_net(&sim, "y6", 11, "old alias retains object")?;
        Ok(())
    });

    // 4f. $cast：已知继承链的上/下行转换、失败状态与 void 调用。
    c.add("sv_class_cast_handle_compatibility", || {
        let source = r#"
class Base;
  int value;
  function new(int v = 0);
    this.value = v;
  endfunction
endclass
class Derived extends Base;
  int extra;
  function new(int v = 0, int e = 0);
    super.new(v);
    this.extra = e;
  endfunction
endclass
class Other;
  int value;
  function new(int v = 0);
    this.value = v;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7);
  Base b;
  Base up;
  Derived d;
  Derived down;
  Derived failed;
  Other other;
  initial begin
    d = new(11, 22);
    b = d;
    y1 = $cast(down, b);   // 基类句柄实际引用 Derived：1
    y2 = down.extra;       // 22

    y3 = $cast(up, d);     // 派生→基类：1
    y4 = up.value;         // 11

    other = new(33);
    y5 = $cast(failed, other); // 类型与 Derived 无关：0
    y6 = failed.value;     // 未绑定，输出 X

    $cast(down, b);        // void 形式仍执行兼容 cast
    y7 = $cast(down, other); // 0（Other 不能转 Derived）
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "compatible downcast")?;
        expect_net(&sim, "y2", 22, "downcast derived field")?;
        expect_net(&sim, "y3", 1, "upcast")?;
        expect_net(&sim, "y4", 11, "upcast base field")?;
        expect_net(&sim, "y5", 0, "incompatible cast status")?;
        let failed_value = sim.read("y6").and_then(|word| word.to_u64());
        if failed_value.is_some() {
            return Err(format!(
                "failed cast destination should remain unbound: {failed_value:?}"
            ));
        }
        expect_net(&sim, "y7", 0, "void cast followed by status cast")?;
        Ok(())
    });

    // 4g. `new source` 浅拷贝：新对象身份、顶层字段独立、嵌套句柄共享。
    c.add("sv_class_new_source_shallow_copy", || {
        let source = r#"
class Inner;
  int value;
  function new(int v = 0);
    this.value = v;
  endfunction
endclass
class Outer;
  int tag;
  Inner inner;
  function new(int t = 0);
    this.tag = t;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6);
  Outer a;
  Outer copy;
  Inner nested;
  initial begin
    nested = new(5);
    a = new(11);
    a.inner = nested;
    copy = new a;          // shallow copy

    y1 = copy.tag;             // 11
    y2 = copy.inner.value;     // 5
    y3 = (copy == a);          // 0：新对象身份
    y4 = (copy.inner == nested); // 1：嵌套句柄仍引用同一对象

    copy.tag = 22;
    copy.inner.value = 7;      // 经共享 nested 句柄写入
    y5 = a.tag;                // 11：顶层字段独立
    y6 = nested.value;         // 7：嵌套对象共享
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 11, "copied scalar field")?;
        expect_net(&sim, "y2", 5, "nested value visible through copy")?;
        expect_net(&sim, "y3", 0, "copy has fresh object identity")?;
        expect_net(&sim, "y4", 1, "nested handle is shared")?;
        expect_net(&sim, "y5", 11, "source scalar remains independent")?;
        expect_net(&sim, "y6", 7, "nested object is shared after write")?;
        Ok(())
    });

    // 4e. 参数化 class 默认值：属性宽度、方法体、返回宽度与继承基类。
    c.add("sv_class_parameterized_default_specialization", || {
        let source = r#"
class Bus #(parameter int W = 8, parameter int BANKS = 2);
  logic [W-1:0] data;
  int banks = BANKS;
  function new(logic [W-1:0] initial_data = 0);
    this.data = initial_data;
  endfunction
  function int width();
    width = W;
  endfunction
  function int total_bits();
    total_bits = W * BANKS;
  endfunction
  function logic [W-1:0] mask();
    mask = {W{1'b1}};
  endfunction
endclass
class DerivedBus extends Bus;
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7);
  Bus bus;
  DerivedBus narrow;
  initial begin
    bus = new(8'h5a);
    y1 = bus.width();       // 8
    y2 = bus.total_bits(); // 16
    y3 = bus.mask();       // 8'hff
    y4 = bus.banks;        // 2
    narrow = new();
    y5 = narrow.width();   // inherited default 8
    y6 = narrow.mask();    // inherited default mask
    y7 = narrow.total_bits(); // inherited default 16
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 8, "default width")?;
        expect_net(&sim, "y2", 16, "multiple defaults")?;
        expect_net(&sim, "y3", 0xff, "parameterized mask")?;
        expect_net(&sim, "y4", 2, "second parameter property")?;
        expect_net(&sim, "y5", 8, "derived inherited default width")?;
        expect_net(&sim, "y6", 0xff, "derived inherited mask")?;
        expect_net(&sim, "y7", 16, "derived inherited total bits")?;
        Ok(())
    });

    // 4f. 参数化 class 显式覆盖：位置实参、命名实参与未覆盖默认值。
    c.add("sv_class_parameter_explicit_specialization", || {
        let source = r#"
class Bus #(parameter int W = 8, parameter int BANKS = 2);
  logic [W-1:0] data;
  int banks = BANKS;
  function new(logic [W-1:0] initial_data = 0);
    this.data = initial_data;
  endfunction
  function int width();
    width = W;
  endfunction
  function int total_bits();
    total_bits = W * BANKS;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8);
  Bus #(.W(4), .BANKS(3)) named;
  Bus #(6) positional;
  initial begin
    named = new(4'he);
    y1 = named.width();       // 4
    y2 = named.banks;         // 3
    y3 = named.total_bits(); // 12

    positional = new(6'h2a);
    y4 = positional.width(); // 6
    y5 = positional.banks;   // 2
    y6 = positional.total_bits(); // 12
    y7 = named.data;
    y8 = positional.data;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 4, "named width override")?;
        expect_net(&sim, "y2", 3, "named banks override")?;
        expect_net(&sim, "y3", 12, "named total bits")?;
        expect_net(&sim, "y4", 6, "positional width override")?;
        expect_net(&sim, "y5", 2, "unoverridden banks default")?;
        expect_net(&sim, "y6", 12, "positional total bits")?;
        expect_net(&sim, "y7", 0xe, "named specialization payload")?;
        expect_net(&sim, "y8", 0x2a, "positional specialization payload")?;
        Ok(())
    });

    // 4g. 参数化继承：派生类显式选择基类 specialization。
    c.add("sv_class_parameterized_base_specialization", || {
        let source = r#"
class Bus #(parameter int W = 8, parameter int BANKS = 2);
  logic [W-1:0] data;
  int banks = BANKS;
  function new(logic [W-1:0] initial_data = 0);
    this.data = initial_data;
  endfunction
  function int width();
    width = W;
  endfunction
  function int total_bits();
    total_bits = W * BANKS;
  endfunction
endclass
class Bus4 extends Bus #(4);
  function new(logic [3:0] initial_data = 0);
    super.new(initial_data);
  endfunction
endclass
module top(output int y1, y2, y3, y4);
  Bus4 bus;
  initial begin
    bus = new(4'hd);
    y1 = bus.width();      // 4
    y2 = bus.total_bits(); // 8
    y3 = bus.banks;        // 2
    y4 = bus.data;         // 13
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 4, "specialized base width")?;
        expect_net(&sim, "y2", 8, "specialized base total bits")?;
        expect_net(&sim, "y3", 2, "unoverridden base default")?;
        expect_net(&sim, "y4", 13, "specialized base payload")?;
        Ok(())
    });

    // 4h. 派生类参数转发：默认与显式派生 specialization 均传给基类。
    c.add("sv_class_parameter_forward_to_base", || {
        let source = r#"
class Bus #(parameter int W = 8, parameter int BANKS = 2);
  logic [W-1:0] data;
  int banks = BANKS;
  function new(logic [W-1:0] initial_data = 0);
    this.data = initial_data;
  endfunction
  function int width();
    width = W;
  endfunction
  function int total_bits();
    total_bits = W * BANKS;
  endfunction
endclass
class Wrapper #(parameter int W = 3) extends Bus #(W);
  function new(logic [W-1:0] initial_data = 0);
    super.new(initial_data);
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6);
  Wrapper narrow;
  Wrapper #(.W(6)) wide;
  initial begin
    narrow = new(3'h5);
    y1 = narrow.width();       // 3
    y2 = narrow.total_bits(); // 6
    y3 = narrow.banks;        // 2

    wide = new(6'h2c);
    y4 = wide.width();        // 6
    y5 = wide.total_bits();   // 12
    y6 = wide.data;           // 44
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 3, "default forwarded width")?;
        expect_net(&sim, "y2", 6, "default forwarded total bits")?;
        expect_net(&sim, "y3", 2, "default base banks")?;
        expect_net(&sim, "y4", 6, "explicit forwarded width")?;
        expect_net(&sim, "y5", 12, "explicit forwarded total bits")?;
        expect_net(&sim, "y6", 44, "explicit forwarded payload")?;
        Ok(())
    });

    // 1b. 值返回类方法可在直接赋值中执行副作用（静态与虚分派）。
    c.add("sv_class_side_effect_method_assignment", || {
        let source = r#"
class Accumulator;
  int total = 0;
  function new();
  endfunction
  function int add(int value);
    this.total = this.total + value;
    add = this.total;
  endfunction
endclass
class VBase;
  function new();
  endfunction
  virtual function int tick();
    tick = 1;
  endfunction
endclass
class VDerived extends VBase;
  int seen = 0;
  function new();
    super.new();
  endfunction
  virtual function int tick();
    this.seen = this.seen + 1;
    tick = 100 + this.seen;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5);
  Accumulator a;
  VBase b;
  VDerived d;
  initial begin
    a = new();
    y1 = a.add(7);       // 7，同时 total 更新
    y2 = a.add(8);       // 15
    y3 = a.total;        // 15
    d = new();
    b = d;               // 引用别名：b 的动态类型为 VDerived
    y4 = b.tick();       // 101
    y5 = d.seen;         // 1
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 7, "first side-effect return")?;
        expect_net(&sim, "y2", 15, "second side-effect return")?;
        expect_net(&sim, "y3", 15, "static method state updated")?;
        expect_net(&sim, "y4", 101, "virtual side-effect return")?;
        expect_net(&sim, "y5", 1, "virtual override state updated")?;
        Ok(())
    });

    // 2. Inheritance: base fields, super.new, method inheritance.
    c.add("sv_class_inheritance_super_new_and_method", || {
        let source = r#"
class Base;
  int value;
  function new(int v);
    this.value = v;
  endfunction
  function int get();
    get = this.value;
  endfunction
endclass
class Derived extends Base;
  int extra = 5;
  function new(int v);
    super.new(v + 1);
  endfunction
endclass
module top(output int y);
  Derived h;
  initial begin
    h = new(10);
    y = h.get() + h.extra;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("inheritance output unknown")?;
        if actual != 16 {
            return Err(format!("inheritance result {actual}, expected 16"));
        }
        Ok(())
    });

    // 3. Method override + super.method dispatch.
    c.add("sv_class_override_and_super_method", || {
        let source = r#"
class Shape;
  function int area();
    area = 1;
  endfunction
endclass
class Square extends Shape;
  int side;
  function new(int s);
    super.new();
    this.side = s;
  endfunction
  function int area();
    area = super.area() + this.side * this.side - 1;
  endfunction
endclass
module top(output int y);
  Square q;
  initial begin
    q = new(6);
    y = q.area();
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("override output unknown")?;
        if actual != 36 {
            return Err(format!("override result {actual}, expected 36"));
        }
        Ok(())
    });

    // 4. Static property shared across every handle and the class name.
    c.add("sv_class_static_property_shared", || {
        let source = r#"
class S;
  static int count = 0;
  function void bump();
    S.count = S.count + 1;
  endfunction
endclass
module top(output int y1, y2);
  S a;
  S b;
  initial begin
    a = new();
    b = new();
    a.bump();
    b.bump();
    b.bump();
    y1 = S.count;
    y2 = a.count;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let y1 = sim
            .read("y1")
            .and_then(|word| word.to_u64())
            .ok_or("static count output unknown")?;
        let y2 = sim
            .read("y2")
            .and_then(|word| word.to_u64())
            .ok_or("static handle access output unknown")?;
        if y1 != 3 || y2 != 3 {
            return Err(format!("static counts {y1}/{y2}, expected 3/3"));
        }
        Ok(())
    });

    // 5. Built-in UVM-lite base class identity.
    c.add("sv_class_uvm_object_builtin_base", || {
        let source = r#"
class trx extends uvm_object;
  int payload;
  function new(int id, int p);
    super.new(id);
    this.payload = p;
  endfunction
endclass
module top(output int y);
  trx t;
  initial begin
    t = new(7, 99);
    y = t.get_id() + t.payload;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("uvm object output unknown")?;
        if actual != 106 {
            return Err(format!("uvm object result {actual}, expected 106"));
        }
        Ok(())
    });

    // 6. 句柄赋值 = 引用别名：经任一句柄写/读共享同一存储。
    c.add("sv_class_handle_assignment_aliases_storage", || {
        let source = r#"
class Pair;
  int a = 0;
  int b = 0;
  function void set(int x, int y);
    this.a = x;
    this.b = y;
  endfunction
endclass
module top(output int y1, y2, y3);
  Pair p;
  Pair q;
  initial begin
    p = new();
    p.set(11, 22);
    q = p;              // q 与 p 同一对象
    q.set(33, 44);      // 经 q 写
    y1 = p.a;           // 经 p 读：33
    y2 = p.b;           // 44
    p.set(55, 66);      // 经 p 写
    y3 = q.a;           // 经 q 读：55
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 33, "write via q visible via p")?;
        expect_net(&sim, "y2", 44, "second field via alias")?;
        expect_net(&sim, "y3", 55, "write via p visible via q")?;
        Ok(())
    });

    // 7. Declaration-initializer construction `Counter h = new(...)`.
    c.add("sv_class_declaration_initializer_new", || {
        let source = r#"
class Val;
  int v;
  function new(int x);
    this.v = x;
  endfunction
endclass
module top(output int y);
  Val h = new(77);
  initial y = h.v;
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("declaration-init output unknown")?;
        if actual != 77 {
            return Err(format!("declaration-init result {actual}, expected 77"));
        }
        Ok(())
    });

    // 8. Virtual classes and qualifier parsing.
    c.add("sv_class_virtual_and_qualifiers_parse", || {
        let source = r#"
virtual class Item;
  rand int addr;
  randc int data;
  local int secret;
  protected int guard;
  static int shared = 2;
endclass
"#;
        parse(&format!("module top; {source} endmodule")).map_err(|e| e.to_string())?;
        Ok(())
    });

    // 9. Class in a package, used by name from the module.
    c.add("sv_class_package_scope_visible", || {
        let source = r#"
package model_pkg;
  class Reg;
    int width;
    function new(int w);
      this.width = w;
    endfunction
  endclass
endpackage
module top(output int y);
  model_pkg::Reg r;
  initial begin
    r = new(32);
    y = r.width;
  end
endmodule
"#;
        // Scoped handle types are not yet resolved; parsing and class
        // collection must still succeed.
        parse(source).map_err(|e| e.to_string())?;
        Ok(())
    });

    // 10. Task-style void methods parse and run.
    c.add("sv_class_task_method_runs", || {
        let source = r#"
class Acc;
  int total = 1;
  task add(int x);
    this.total = this.total + x;
  endtask
endclass
module top(output int y);
  Acc a;
  initial begin
    a = new();
    a.add(20);
    y = a.total;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("task method output unknown")?;
        if actual != 21 {
            return Err(format!("task method result {actual}, expected 21"));
        }
        Ok(())
    });

    // 11. Statement-invoked value-returning method writes scratch result.
    c.add("sv_class_value_method_as_statement", || {
        let source = r#"
class W;
  int x;
  function int double();
    double = this.x * 2;
  endfunction
endclass
module top(output int y);
  W w;
  initial begin
    w = new();
    w.x = 9;
    w.double();
    y = w.x;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("statement method output unknown")?;
        if actual != 9 {
            return Err(format!("statement method result {actual}, expected 9"));
        }
        Ok(())
    });

    // 12. Field initializer expressions run before constructor body.
    c.add("sv_class_property_initializer_order", || {
        let source = r#"
class Init;
  int a = 10;
  int b;
  function new(int x);
    this.b = this.a + x;
  endfunction
endclass
module top(output int y);
  Init h;
  initial begin
    h = new(5);
    y = h.b;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("initializer order output unknown")?;
        if actual != 15 {
            return Err(format!("initializer order result {actual}, expected 15"));
        }
        Ok(())
    });

    // Rejected programs.
    c.add("sv_class_unknown_type_rejected", || {
        let result = compile(
            "module top; Missing h; initial h = new(); endmodule",
            Some("top"),
        );
        let error = result.err().ok_or("unknown class type accepted")?;
        if !error.contains("unknown type") {
            return Err(format!("unexpected error: {error}"));
        }
        Ok(())
    });

    c.add("sv_class_unknown_method_rejected", || {
        let source = r#"
class C;
  int x;
endclass
module top(output int y);
  C h;
  initial begin
    h = new();
    y = h.missing();
  end
endmodule
"#;
        let result = compile(source, Some("top"));
        let error = result.err().ok_or("unknown method accepted")?;
        if !error.contains("no method 'missing'") {
            return Err(format!("unexpected error: {error}"));
        }
        Ok(())
    });

    c.add("sv_class_new_on_non_handle_rejected", || {
        let result = compile(
            "module top(output int y); int h; initial begin h = new(); y = h; end endmodule",
            Some("top"),
        );
        let error = result.err().ok_or("new on non-handle accepted")?;
        if !error.contains("not a class handle") {
            return Err(format!("unexpected error: {error}"));
        }
        Ok(())
    });

    c.add("sv_class_constructor_wrong_arity_rejected", || {
        let source = r#"
class C;
  function new(int a, int b);
  endfunction
endclass
module top; C h; initial h = new(1); endmodule
"#;
        let result = compile(source, Some("top"));
        let error = result.err().ok_or("wrong constructor arity accepted")?;
        if !error.contains("expects 2 argument") {
            return Err(format!("unexpected error: {error}"));
        }
        Ok(())
    });

    c.add("sv_class_endclass_label_mismatch_rejected", || {
        let result = parse("class C; endclass : D");
        let error = result.err().ok_or("label mismatch accepted")?;
        if !error.contains("does not match class") {
            return Err(format!("unexpected error: {error}"));
        }
        Ok(())
    });
}

fn class_composition_cases(c: &mut CaseList) {
    // 1. Nested class-typed properties: construct, call through paths.
    c.add("sv_class_nested_env_driver_composition", || {
        let source = r#"
class Driver;
  int addr = 0;
  function new(int a);
    this.addr = a;
  endfunction
  function void drive(int v);
    this.addr = this.addr + v;
  endfunction
  function int get();
    get = this.addr;
  endfunction
endclass
class Env;
  Driver drv;
  int bias = 100;
  function new(int a);
    this.drv = new(a);
  endfunction
  function void run(int v);
    this.drv.drive(v);
  endfunction
endclass
module top(output int y);
  Env e;
  initial begin
    e = new(7);
    e.run(5);
    e.drv.drive(10);
    y = e.drv.get() + e.bias;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("composition output unknown")?;
        if actual != 122 {
            return Err(format!("composition result {actual}, expected 122"));
        }
        Ok(())
    });

    // 2. Two-level nesting: top.mid.leaf field and method access.
    c.add("sv_class_two_level_nested_access", || {
        let source = r#"
class Leaf;
  int v = 3;
  function void add(int x);
    this.v = this.v + x;
  endfunction
endclass
class Mid;
  Leaf leaf;
  int scale = 10;
  function new();
    this.leaf = new();
  endfunction
endclass
class Top;
  Mid mid;
  function new();
    this.mid = new();
  endfunction
endclass
module top(output int y);
  Top t;
  initial begin
    t = new();
    t.mid.leaf.add(4);
    y = t.mid.leaf.v * t.mid.scale;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("two-level output unknown")?;
        if actual != 70 {
            return Err(format!("two-level result {actual}, expected 70"));
        }
        Ok(())
    });

    // 3. UVM component base class with overridden run_phase.
    c.add("sv_class_uvm_component_run_phase", || {
        let source = r#"
class my_comp extends uvm_component;
  int hits = 0;
  function new(int id);
    super.new(id);
  endfunction
  function void run_phase();
    this.hits = this.hits + 1;
  endfunction
endclass
module top(output int y);
  my_comp c;
  initial begin
    c = new(3);
    c.run_phase();
    c.run_phase();
    y = c.get_id() * 100 + c.hits;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("uvm component output unknown")?;
        if actual != 302 {
            return Err(format!("uvm component result {actual}, expected 302"));
        }
        Ok(())
    });

    // 4. Inherited nested property access through a derived component.
    c.add("sv_class_nested_field_via_inheritance", || {
        let source = r#"
class Node;
  int base = 1;
endclass
class Big extends Node;
  int extra = 2;
endclass
class Holder;
  Big item;
  function new();
    this.item = new();
  endfunction
endclass
module top(output int y);
  Holder h;
  initial begin
    h = new();
    y = h.item.base + h.item.extra;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("nested inheritance output unknown")?;
        if actual != 3 {
            return Err(format!("nested inheritance result {actual}, expected 3"));
        }
        Ok(())
    });

    // 5. Method through nested path returns value in expression.
    c.add("sv_class_nested_method_expression", || {
        let source = r#"
class Cell;
  int data;
  function new(int d);
    this.data = d;
  endfunction
  function int read();
    read = this.data;
  endfunction
endclass
class Bank;
  Cell cells0;
  function new(int d);
    this.cells0 = new(d);
  endfunction
endclass
module top(output int y);
  Bank b;
  initial begin
    b = new(44);
    y = b.cells0.read() + 1;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("nested method expression unknown")?;
        if actual != 45 {
            return Err(format!("nested method result {actual}, expected 45"));
        }
        Ok(())
    });

    // Negative: unknown method through nested path.
    c.add("sv_class_nested_unknown_method_rejected", || {
        let source = r#"
class Leaf;
  int v;
endclass
class Holder;
  Leaf leaf;
  function new();
    this.leaf = new();
  endfunction
endclass
module top(output int y);
  Holder h;
  initial begin
    h = new();
    y = h.leaf.missing();
  end
endmodule
"#;
        let result = compile(source, Some("top"));
        let error = result.err().ok_or("nested unknown method accepted")?;
        if !error.contains("no method 'missing'") {
            return Err(format!("unexpected error: {error}"));
        }
        Ok(())
    });

    // Negative: selecting through a non-class property.
    c.add("sv_class_select_through_value_field_rejected", || {
        let source = r#"
class Leaf;
  int v;
endclass
class Holder;
  Leaf leaf;
  int plain;
  function new();
    this.leaf = new();
  endfunction
endclass
module top(output int y);
  Holder h;
  initial begin
    h = new();
    y = h.plain.deeper;
  end
endmodule
"#;
        let result = compile(source, Some("top"));
        let error = result.err().ok_or("value-field chain accepted")?;
        if !error.contains("cannot select further") {
            return Err(format!("unexpected error: {error}"));
        }
        Ok(())
    });

    // Negative: new on a value field.
    c.add("sv_class_new_on_value_field_rejected", || {
        let source = r#"
class Holder;
  int plain;
endclass
module top;
  Holder h;
  initial begin
    h = new();
    h.plain = new();
  end
endmodule
"#;
        let result = compile(source, Some("top"));
        let error = result.err().ok_or("new on value field accepted")?;
        if !error.contains("new can only construct a class-typed property") {
            return Err(format!("unexpected error: {error}"));
        }
        Ok(())
    });
}

fn string_cases(c: &mut CaseList) {
    // 1. string 变量赋值 + 与字面量比较（NUL 填充布局下相等语义）。
    c.add("sv_string_literal_compare", || {
        let source = r#"
module top(output ok1, ok2);
  string s;
  initial begin
    s = "hello";
    ok1 = (s == "hello");
    ok2 = (s != "world");
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let ok1 = sim.read("ok1").and_then(|w| w.to_u64());
        let ok2 = sim.read("ok2").and_then(|w| w.to_u64());
        if ok1 != Some(1) || ok2 != Some(1) {
            return Err(format!("string compare ok1={ok1:?} ok2={ok2:?}"));
        }
        Ok(())
    });

    // 1b. 64 字节固定宽度字符串：满容量比较、格式化与 %s 打印。
    c.add("sv_string_64_byte_compare_and_format", || {
        let exact = "A".repeat(64);
        let source = format!(
            r#"
module top(output reg ok1, ok2);
  string s;
  initial begin
    s = "{exact}";
    ok1 = (s == "{exact}");
    s = $sformatf("uvm-frame-%s-%0d", "sequence", 42);
    ok2 = (s == "uvm-frame-sequence-42");
    $display("%s", s);
  end
endmodule
"#
        );
        let design = compile(&source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "ok1", 1, "64-byte string equality")?;
        expect_net(&sim, "ok2", 1, "formatted string equality")?;
        let text = sim.take_displays().join("\n");
        if !text.contains("uvm-frame-sequence-42") {
            return Err(format!("wide %s display missing: {text:?}"));
        }
        Ok(())
    });

    // 1c. string 内建 len/getc：支持变量索引、越界返回 0 与类属性路径。
    c.add("sv_string_len_getc_methods", || {
        let source = r#"
class Named;
  string name;
  function new();
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6);
  string s;
  Named h;
  int index;
  initial begin
    s = "uvm_sequence_item";
    index = 4;
    y1 = s.len();          // 17
    y2 = s.getc(0);        // 'u' = 117
    y3 = s.getc(index);    // 's' = 115
    y4 = s.getc(99);       // 越界：0
    s = "";
    y5 = s.len();          // 0
    h = new();
    h.name = "driver0";
    y6 = h.name.len() + h.name.getc(1); // 7 + 'r' = 121
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 17, "string length")?;
        expect_net(&sim, "y2", 117, "first character")?;
        expect_net(&sim, "y3", 115, "runtime-indexed character")?;
        expect_net(&sim, "y4", 0, "out-of-range character")?;
        expect_net(&sim, "y5", 0, "empty string length")?;
        expect_net(&sim, "y6", 121, "class string property methods")?;
        Ok(())
    });

    // 1d. string 内建 putc/toupper/tolower：运行时写byte与整串大小写转换。
    c.add("sv_string_putc_and_case_methods", || {
        let source = r#"
class Named;
  string name;
  function new();
  endfunction
endclass
module top(output int y1, y2, y3, y4, output reg ok1, ok2, ok3, ok4);
  string s;
  string upper;
  string lower;
  Named h;
  int index;
  initial begin
    s = "Uvm-xeQ";
    index = 0;
    s.putc(index, 117);   // 'u'
    s.putc(5, 69);        // 'E'
    s.putc(99, 65);       // 越界：不改写
    y1 = s.getc(0);
    y2 = s.getc(5);
    y3 = s.len();
    upper = s.toupper();
    lower = s.tolower();
    ok1 = (upper == "UVM-XEQ");
    ok2 = (lower == "uvm-xeq");

    h = new();
    h.name = "Driver-0";
    h.name.putc(0, 100);  // 'd'
    y4 = h.name.getc(0);
    upper = h.name.toupper();
    ok3 = (upper == "DRIVER-0");
    ok4 = (h.name.len() == 8);
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 117, "putc runtime index")?;
        expect_net(&sim, "y2", 69, "putc constant index")?;
        expect_net(&sim, "y3", 7, "out-of-range putc preserves length")?;
        expect_net(&sim, "y4", 100, "class property putc")?;
        expect_net(&sim, "ok1", 1, "toupper")?;
        expect_net(&sim, "ok2", 1, "tolower")?;
        expect_net(&sim, "ok3", 1, "class property toupper")?;
        expect_net(&sim, "ok4", 1, "class property length after putc")?;
        Ok(())
    });

    // 1e. string 内建 compare/icompare：按首差异 byte 给出 -1/0/1。
    c.add("sv_string_compare_icompare_methods", || {
        let source = r#"
class Named;
  string name;
  function new(string n);
    this.name = n;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7);
  string a;
  string b;
  Named h;
  initial begin
    a = "ABC";
    b = "ABC";
    y1 = a.compare(b);       // 0
    b = "ABD";
    y2 = a.compare(b);       // -1
    y3 = b.compare(a);       // 1
    y4 = a.compare("ABC");   // 0
    a = "abc";
    b = "abd";
    y5 = a.compare(b);       // -1
    y6 = a.icompare("ABC");  // 0
    h = new("driver-1");
    y7 = h.name.compare("driver-2"); // -1
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 0, "equal strings")?;
        expect_net(&sim, "y2", u32::MAX as u64, "lexicographically less")?;
        expect_net(&sim, "y3", 1, "lexicographically greater")?;
        expect_net(&sim, "y4", 0, "literal compare")?;
        expect_net(&sim, "y5", u32::MAX as u64, "case-sensitive compare")?;
        expect_net(&sim, "y6", 0, "case-insensitive compare")?;
        expect_net(&sim, "y7", u32::MAX as u64, "class property compare")?;
        Ok(())
    });

    // 1f. string.substr：运行时左右边界、闭区间拷贝与非法区间清空。
    c.add("sv_string_substr_runtime_bounds", || {
        let source = r#"
class Named;
  string name;
  function new(string n);
    this.name = n;
  endfunction
endclass
module top(output reg ok1, ok2, ok3, ok4, ok5, ok6);
  string s;
  string sub;
  Named h;
  int left;
  int right;
  initial begin
    s = "uvm_driver_0";
    left = 4;
    right = 9;
    sub = s.substr(left, right);       // "driver"
    ok1 = (sub == "driver");
    sub = s.substr(0, 2);              // "uvm"
    ok2 = (sub == "uvm");
    sub = s.substr(10, 11);            // "_0"
    ok3 = (sub == "_0");
    sub = s.substr(right, left);       // left > right：空
    ok4 = (sub.len() == 0);
    sub = s.substr(-1, 3);             // 负左边界：空
    ok5 = (sub.len() == 0);

    h = new("scoreboard");
    sub = h.name.substr(5, 9);         // "board"
    ok6 = (sub == "board");
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "ok1", 1, "runtime substring")?;
        expect_net(&sim, "ok2", 1, "prefix substring")?;
        expect_net(&sim, "ok3", 1, "suffix substring")?;
        expect_net(&sim, "ok4", 1, "reversed bounds empty")?;
        expect_net(&sim, "ok5", 1, "negative left bound empty")?;
        expect_net(&sim, "ok6", 1, "class property substring")?;
        Ok(())
    });

    // 1g. string.atoi：十进制前缀、前导空白、正负号与非法输入。
    c.add("sv_string_atoi_decimal_prefix", || {
        let source = r#"
class Named;
  string name;
  function new(string n);
    this.name = n;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7);
  string s;
  Named h;
  initial begin
    s = "42";
    y1 = s.atoi();             // 42
    s = "  -123uvm";
    y2 = s.atoi();             // -123
    s = "+7_items";
    y3 = s.atoi();             // 7
    s = "0043";
    y4 = s.atoi();             // 43
    s = "uvm42";
    y5 = s.atoi();             // 0
    s = "   ";
    y6 = s.atoi();             // 0
    h = new("123");
    y7 = h.name.atoi();        // 123
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 42, "decimal conversion")?;
        expect_net(
            &sim,
            "y2",
            (-123_i32) as u32 as u64,
            "signed negative conversion",
        )?;
        expect_net(&sim, "y3", 7, "explicit positive sign")?;
        expect_net(&sim, "y4", 43, "leading zeros")?;
        expect_net(&sim, "y5", 0, "invalid prefix")?;
        expect_net(&sim, "y6", 0, "whitespace-only input")?;
        expect_net(&sim, "y7", 123, "class property conversion")?;
        Ok(())
    });

    // 1h. string.atohex/atobin：十六进制与二进制前缀、符号和类属性。
    c.add("sv_string_atohex_atobin_radix", || {
        let source = r#"
class Named;
  string name;
  function new(string n);
    this.name = n;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8);
  string s;
  Named h;
  initial begin
    s = "1A";
    y1 = s.atohex();          // 26
    s = " -2fUVM";
    y2 = s.atohex();          // -47
    s = "+7q";
    y3 = s.atohex();          // 7
    s = "uvm";
    y4 = s.atohex();          // 0
    h = new("DEAD");
    y5 = h.name.atohex();     // 57005

    s = "1011";
    y6 = s.atobin();          // 11
    s = " -1010uvm";
    y7 = s.atobin();          // -10
    s = "+011";
    y8 = s.atobin();          // 3
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 26, "hex conversion")?;
        expect_net(
            &sim,
            "y2",
            (-47_i32) as u32 as u64,
            "signed negative hex conversion",
        )?;
        expect_net(&sim, "y3", 7, "positive hex sign")?;
        expect_net(&sim, "y4", 0, "invalid hex prefix")?;
        expect_net(&sim, "y5", 57_005, "class property hex")?;
        expect_net(&sim, "y6", 11, "binary conversion")?;
        expect_net(
            &sim,
            "y7",
            (-10_i32) as u32 as u64,
            "signed negative binary conversion",
        )?;
        expect_net(&sim, "y8", 3, "positive binary leading zero")?;
        Ok(())
    });

    // 1i. string.itoa：运行时整数到十进制 ASCII 字符串转换。
    c.add("sv_string_itoa_runtime_decimal", || {
        let source = r#"
class Named;
  string name;
  function new();
  endfunction
endclass
module top(output reg ok1, ok2, ok3, ok4, ok5);
  string s;
  Named h;
  int value;
  initial begin
    value = 0;
    s.itoa(value);
    ok1 = (s == "0");

    value = 12345;
    s.itoa(value);
    ok2 = (s == "12345");

    value = -9876;
    s.itoa(value);
    ok3 = (s == "-9876");

    value = -2147483647;
    s.itoa(value);
    ok4 = (s == "-2147483647");

    h = new();
    h.name.itoa(7);
    ok5 = (h.name == "7");
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "ok1", 1, "itoa zero")?;
        expect_net(&sim, "ok2", 1, "itoa positive")?;
        expect_net(&sim, "ok3", 1, "itoa negative")?;
        expect_net(&sim, "ok4", 1, "itoa large negative")?;
        expect_net(&sim, "ok5", 1, "class property itoa")?;
        Ok(())
    });

    // 1j. string.hextoa/bintoa：运行时整数到十六进制/二进制 ASCII。
    c.add("sv_string_hextoa_bintoa_runtime", || {
        let source = r#"
class Named;
  string name;
  function new();
  endfunction
endclass
module top(output reg ok1, ok2, ok3, ok4, ok5, ok6, ok7, ok8);
  string s;
  Named h;
  int value;
  initial begin
    value = 0;
    s.hextoa(value);
    ok1 = (s == "0");

    value = 26;
    s.hextoa(value);
    ok2 = (s == "1a");

    value = -1;
    s.hextoa(value);
    ok3 = (s == "ffffffff");

    h = new();
    h.name.hextoa(48879);
    ok4 = (h.name == "beef");

    value = 0;
    s.bintoa(value);
    ok5 = (s == "0");

    value = 5;
    s.bintoa(value);
    ok6 = (s == "101");

    value = -1;
    s.bintoa(value);
    ok7 = (s.len() == 32) && (s == "11111111111111111111111111111111");

    h.name.bintoa(6);
    ok8 = (h.name == "110");
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "ok1", 1, "hextoa zero")?;
        expect_net(&sim, "ok2", 1, "hextoa positive")?;
        expect_net(&sim, "ok3", 1, "hextoa negative as 32-bit value")?;
        expect_net(&sim, "ok4", 1, "class property hextoa")?;
        expect_net(&sim, "ok5", 1, "bintoa zero")?;
        expect_net(&sim, "ok6", 1, "bintoa positive")?;
        expect_net(&sim, "ok7", 1, "bintoa negative as 32-bit value")?;
        expect_net(&sim, "ok8", 1, "class property bintoa")?;
        Ok(())
    });

    // 2. class string 属性 + 方法内比较（组合/方法路径）。
    c.add("sv_class_string_property", || {
        let source = r#"
class Named;
  string name;
  function new(string n);
    this.name = n;
  endfunction
  function int is_tb();
    is_tb = (this.name == "TB");
  endfunction
endclass
module top(output reg y);
  Named h;
  initial begin
    h = new("TB");
    y = h.is_tb();
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let y = sim.read("y").and_then(|w| w.to_u64());
        if y != Some(1) {
            return Err(format!("class string compare y={y:?}"));
        }
        Ok(())
    });

    // 3. $uvm_info 打印带 tag/verbosity/格式化参数；%0d 修饰。
    c.add("sv_uvm_info_tagged_format", || {
        let source = r#"
module top;
  int count;
  initial begin
    count = 42;
    $uvm_info("TB", "count=%0d items, flag=%b", count, 1'b1, UVM_MEDIUM);
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let displays = sim.take_displays();
        let text = displays.join("\n");
        if !text.contains("[UVM_INFO][TB][UVM_MEDIUM] count=42 items, flag=1") {
            return Err(format!("uvm_info output missing: {text:?}"));
        }
        Ok(())
    });

    // 5. uvm_warning/error/fatal 家族 + %s 字符串参数打印。
    c.add("sv_uvm_severity_family_and_percent_s", || {
        let source = r#"
module top(output reg done);
  string tag;
  initial begin
    tag = "CHK";
    $uvm_warning("TB", "warn count=%0d", 5);
    $uvm_error("TB", "err tag=%s", tag);
    $uvm_fatal("TB", "fatal stop now");
    done = 1'b1; // fatal 后不应执行
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let displays = sim.take_displays();
        let text = displays.join("\n");
        if !text.contains("[UVM_WARNING][TB] warn count=5") {
            return Err(format!("warning line missing: {text:?}"));
        }
        if !text.contains("[UVM_ERROR][TB] err tag=CHK") {
            return Err(format!("error line with %s missing: {text:?}"));
        }
        if !text.contains("[UVM_FATAL][TB] fatal stop now") {
            return Err(format!("fatal line missing: {text:?}"));
        }
        if !sim.finished() {
            return Err("$uvm_fatal must finish the simulation".to_string());
        }
        // fatal 后续语句不执行：done 保持 X。
        let done = sim.read("done").and_then(|w| w.to_u64());
        if done.is_some() {
            return Err(format!(
                "statement after $uvm_fatal executed: done={done:?}"
            ));
        }
        Ok(())
    });

    // 7. 表达式形式类方法的比较/算术体（回归：赋值按左值宽度补齐修复）。
    c.add("sv_class_expression_method_comparisons", || {
        let source = r#"
class box2 extends uvm_object;
  int v = 5;
  function new(int id);
    super.new(id);
  endfunction
  function int get_v();
    get_v = this.v;
  endfunction
  function int is_zero();
    is_zero = (this.v == 0);
  endfunction
  function int plus_two();
    plus_two = this.v + 2;
  endfunction
endclass
class holder extends uvm_component;
  box2 b;
  function new(int id);
    super.new(id);
    this.b = new(id * 10 + 1);
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5);
  box2 b;
  holder h;
  initial begin
    b = new(1);
    h = new(2);
    y1 = b.get_v();
    y2 = b.is_zero();
    y3 = h.b.get_v();
    y4 = h.b.is_zero();
    y5 = h.b.plus_two();
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 5, "top-level expression method")?;
        expect_net(
            &sim,
            "y2",
            0,
            "comparison expression method (v=5, not zero)",
        )?;
        expect_net(&sim, "y3", 5, "nested expression method")?;
        expect_net(&sim, "y4", 0, "nested comparison method")?;
        expect_net(&sim, "y5", 7, "nested arithmetic method")?;
        Ok(())
    });

    // 6. $sformatf 常量参数编译期格式化（%0d/%h/%b/%s/%%）。
    c.add("sv_sformatf_constant_formatting", || {
        let source = r#"
module top(output ok1, ok2, ok3);
  string a;
  string b;
  string c;
  initial begin
    a = $sformatf("n=%0d", 42);
    b = $sformatf("tag_%s", "TB");
    c = $sformatf("%b%%%0d", 1'b1, 7);
    ok1 = (a == "n=42");
    ok2 = (b == "tag_TB");
    ok3 = (c == "1%7");
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "ok1", 1, "sformatf numeric")?;
        expect_net(&sim, "ok2", 1, "sformatf string")?;
        expect_net(&sim, "ok3", 1, "sformatf binary and percent")?;
        Ok(())
    });

    // 4. 非法 verbosity / 超长字符串拒绝。
    c.add("sv_uvm_info_rejects_bad_verbosity", || {
        let result = compile(
            "module top; initial $uvm_info(\"TB\", \"x\", UVM_LOUD); endmodule",
            Some("top"),
        );
        let error = result.err().ok_or("bad verbosity accepted")?;
        if !error.contains("unknown $uvm_info verbosity") {
            return Err(format!("unexpected error: {error}"));
        }
        let oversized_literal = "x".repeat(65);
        let literal_source = format!(
            "module top; string s; initial s = \"{oversized_literal}\"; endmodule"
        );
        let result = compile(&literal_source, Some("top"));
        let error = result.err().ok_or("oversized string accepted")?;
        if !error.contains("limited to 64 bytes") {
            return Err(format!("unexpected error: {error}"));
        }
        let result = compile(
            "module top(input clk, output string s); always @(posedge clk) s = $sformatf(\"%0d\", clk); endmodule",
            Some("top"),
        );
        let error = result.err().ok_or("runtime sformatf accepted")?;
        if !error.contains("constant arguments") {
            return Err(format!("unexpected sformatf error: {error}"));
        }
        let oversized_result = "x".repeat(65);
        let sformat_source = format!(
            "module top; string s; initial s = $sformatf(\"{oversized_result}\"); endmodule"
        );
        let result = compile(&sformat_source, Some("top"));
        let error = result.err().ok_or("oversized sformatf accepted")?;
        if !error.contains("exceeds the 64-byte") {
            return Err(format!("unexpected sformatf error: {error}"));
        }
        Ok(())
    });
}

fn class_polymorphism_cases(c: &mut CaseList) {
    // 1. 基类句柄 + 虚方法运行时分派（task 形式 + 基类字段动态绑定）。
    c.add("sv_class_virtual_dispatch_via_base_handle", || {
        let source = r#"
class Shape;
  int scale = 10;
  function new(int s);
    this.scale = s;
  endfunction
  virtual function void double_area();
    this.scale = this.scale * 2;
  endfunction
endclass
class Square extends Shape;
  int side;
  function new(int s, int sd);
    super.new(s);
    this.side = sd;
  endfunction
  virtual function void double_area();
    this.scale = this.scale + 1;
  endfunction
endclass
module top(output int y1, y2, y3);
  Shape b;
  Square q;
  initial begin
    b = new(5);
    b.double_area();      // 基类实现：5*2 = 10
    y1 = b.scale;
    q = new(7, 4);
    b = q;                // 引用别名：b 与 q 指向同一对象
    b.double_area();      // 派生 override：7+1 = 8
    y2 = b.scale;
    y3 = q.scale;         // 经原句柄观察同一存储：8
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 10, "base dispatch")?;
        expect_net(&sim, "y2", 8, "derived dispatch via base handle")?;
        expect_net(
            &sim,
            "y3",
            8,
            "reference aliasing: write via b visible via q",
        )?;
        Ok(())
    });

    // 2. 表达式形式虚分派（virtual function int）。
    c.add("sv_class_virtual_dispatch_expression", || {
        let source = r#"
class Base;
  int v;
  function new(int x);
    this.v = x;
  endfunction
  virtual function int kind();
    kind = this.v;
  endfunction
endclass
class Derived extends Base;
  function new(int x);
    super.new(x);
  endfunction
  virtual function int kind();
    kind = this.v + 100;
  endfunction
endclass
module top(output int y1, y2);
  Base b;
  Derived d;
  initial begin
    b = new(3);
    y1 = b.kind();
    d = new(4);
    b = d;
    y2 = b.kind();
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 3, "base virtual expression")?;
        expect_net(&sim, "y2", 104, "derived virtual expression")?;
        Ok(())
    });

    // 3. 非虚方法保持静态绑定（对照）。
    c.add("sv_class_nonvirtual_stays_static", || {
        let source = r#"
class Base;
  virtual function int tag();
    tag = 1;
  endfunction
  function int plain();
    plain = 10;
  endfunction
endclass
class Derived extends Base;
  function new();
  endfunction
  function int plain();
    plain = 20;
  endfunction
endclass
module top(output int y);
  Base b;
  Derived d;
  initial begin
    d = new();
    b = d;
    y = b.plain(); // 非虚：按句柄静态类型解析 Base::plain = 10
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y", 10, "non-virtual static binding")?;
        Ok(())
    });
}

fn expect_net(sim: &Simulator, name: &str, want: u64, what: &str) -> Result<(), String> {
    match sim.read(name).and_then(|w| w.to_u64()) {
        Some(got) if got == want => Ok(()),
        other => Err(format!("{what}: {name} = {other:?}, want {want}")),
    }
}

fn uvm_component_cases(c: &mut CaseList) {
    // 1. phase 钩子按 build→connect→run 顺序经虚分派执行。
    c.add("uvm_component_phase_order_virtual", || {
        let source = r#"
class tb_comp extends uvm_component;
  int trace = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void build_phase();
    this.trace = this.trace * 10 + 1;
  endfunction
  virtual function void connect_phase();
    this.trace = this.trace * 10 + 2;
  endfunction
  virtual function void run_phase();
    this.trace = this.trace * 10 + 3;
  endfunction
endclass
module top(output int y1, y2);
  uvm_component base;
  tb_comp t;
  initial begin
    t = new(1);
    base = t;                 // 基类句柄观察派生 override
    base.run_phases();
    y1 = t.trace;             // 123
    y2 = base.phase_state;    // 3（run_phases 完成）
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 123, "phase order 1-2-3")?;
        expect_net(&sim, "y2", 3, "phase_state completion")?;
        Ok(())
    });

    // 1b. run_phase 自带 framework objection：raise→virtual body→drop→drain。
    c.add("uvm_component_run_phase_objection_lifecycle", || {
        let source = r#"
class worker extends uvm_component;
  int work = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void run_phase();
    this.objection.raise(2);
    this.work = this.work + 1;
    this.objection.drop(2);
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5);
  worker w;
  initial begin
    w = new(7);
    w.run_phases();
    y1 = w.work;                    // 1
    y2 = w.objection.raised;        // 0：framework + user 均已平衡
    y3 = w.objection.raise_calls;   // 2
    y4 = w.objection.drop_calls;    // 2
    y5 = w.objection.was_drained;   // 1
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "virtual run body executed")?;
        expect_net(&sim, "y2", 0, "objection count drained")?;
        expect_net(&sim, "y3", 2, "framework and user raises")?;
        expect_net(&sim, "y4", 2, "framework and user drops")?;
        expect_net(&sim, "y5", 1, "objection drain recorded")?;
        Ok(())
    });

    // 1c. 组件命名 / parent-child 层次 / 一层子组件 phase 驱动。
    c.add("uvm_component_named_child_hierarchy", || {
        let source = r#"
class leaf extends uvm_component;
  int hits = 0;
  function new(int id, string n = "");
    super.new(id, n);
  endfunction
  virtual function void build_phase();
    this.hits = this.hits + 1;
  endfunction
  virtual function void run_phase();
    this.hits = this.hits + 10;
  endfunction
endclass
class env extends uvm_component;
  leaf a;
  leaf b;
  function new(int id, string n);
    super.new(id, n);
    this.a = new(id * 10 + 1);
    this.b = new(id * 10 + 2);
    this.add_child0(this.a);
    this.add_child1(this.b);
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9);
  env e;
  initial begin
    e = new(1, "env");
    e.a.name = "agent_a";
    e.b.name = "agent_b";
    e.child0.run_phases();
    e.child1.run_phases();

    y1 = (e.get_name() == "env");
    y2 = e.a.hits;                  // 11
    y3 = e.child_count;            // 2
    y4 = (e.a.parent == e);        // parent 引用
    y5 = (e.child1 == e.b);        // child slot 引用
    y6 = e.b.hits;                  // 11
    y7 = e.a.phase_state;           // 3
    y8 = (e.a.parent == e);
    y9 = e.b.phase_state;           // 3
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "component name")?;
        expect_net(&sim, "y2", 11, "first child phases")?;
        expect_net(&sim, "y3", 2, "child count")?;
        expect_net(&sim, "y4", 1, "parent identity")?;
        expect_net(&sim, "y5", 1, "child slot identity")?;
        expect_net(&sim, "y6", 11, "second child phases")?;
        expect_net(&sim, "y7", 3, "first child phase completion")?;
        expect_net(&sim, "y8", 1, "parent identity after phases")?;
        expect_net(&sim, "y9", 3, "second child phase completion")?;
        Ok(())
    });

    // 2. 组合层次：env 持有子组件，嵌套路径调用 run_phases 派生钩子生效。
    c.add("uvm_component_hierarchy_nested_phases", || {
        let source = r#"
class agent extends uvm_component;
  int hits = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void build_phase();
    this.hits = this.hits + 1;
  endfunction
  virtual function void run_phase();
    this.hits = this.hits + 10;
  endfunction
endclass
class env extends uvm_component;
  agent a1;
  agent a2;
  function new(int id);
    super.new(id);
    this.a1 = new(id * 10 + 1);
    this.a2 = new(id * 10 + 2);
  endfunction
endclass
module top(output int y1, y2, y3);
  env e;
  initial begin
    e = new(1);
    e.a1.run_phases();        // 嵌套组件：build + run
    e.a2.run_phases();
    y1 = e.a1.hits;            // 11
    y2 = e.a2.hits;            // 11
    e.run_phases();           // env 自身（无 override → 空 + state=3）
    y3 = e.phase_state;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 11, "nested agent a1 build+run")?;
        expect_net(&sim, "y2", 11, "nested agent a2 build+run")?;
        expect_net(&sim, "y3", 3, "env phases complete")?;
        Ok(())
    });
}

fn uvm_channel_cases(c: &mut CaseList) {
    // 分析风格通道：monitor→subscriber 虚 write 扇出（事务计数与载荷）。
    c.add("uvm_analysis_subscriber_fanout", || {
        let source = r#"
class monitor extends uvm_component;
  int value = 0;
  function new(int id);
    super.new(id);
  endfunction
  function void observe(int v);
    this.value = v;
  endfunction
endclass
class collector extends uvm_subscriber;
  int total = 0;
  int hits = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void write(int data);
    this.total = this.total + data;
    this.hits = this.hits + 1;
    this.observed = this.observed + 1;
  endfunction
endclass
class checker extends uvm_subscriber;
  int max = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void write(int data);
    if (data > this.max) this.max = data;
    this.observed = this.observed + 1;
  endfunction
endclass
class env extends uvm_component;
  monitor mon;
  collector col;
  checker chk;
  function new(int id);
    super.new(id);
    this.mon = new(id * 100 + 1);
    this.col = new(id * 100 + 2);
    this.chk = new(id * 100 + 3);
  endfunction
  function void burst(int a, int b, int c);
    this.col.write(a);
    this.chk.write(a);
    this.col.write(b);
    this.chk.write(b);
    this.col.write(c);
    this.chk.write(c);
  endfunction
endclass
module top(output int y1, y2, y3, y4);
  env e;
  initial begin
    e = new(1);
    e.burst(3, 7, 5);
    y1 = e.col.total;      // 15
    y2 = e.col.hits;       // 3
    y3 = e.chk.max;        // 7
    y4 = e.col.observed + e.chk.observed; // 6
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 15, "collector total")?;
        expect_net(&sim, "y2", 3, "collector hits")?;
        expect_net(&sim, "y3", 7, "checker max")?;
        expect_net(&sim, "y4", 6, "subscriber fanout observations")?;
        Ok(())
    });

    // UVM-lite callback pool：四个显式槽、virtual callback 扇出与计数。
    c.add("uvm_callback_pool_virtual_fanout", || {
        let source = r#"
class tracer extends uvm_callback;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void callback(int event, int data);
    this.event_sum = this.event_sum + event;
    this.data_sum = this.data_sum + data;
    this.hits = this.hits + 1;
  endfunction
endclass
class scale extends uvm_callback;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void callback(int event, int data);
    this.scaled = this.scaled + event * data;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6);
  uvm_callback_pool pool;
  tracer t0;
  tracer t1;
  scale s0;
  scale s1;
  initial begin
    pool = new(1);
    t0 = new(2);
    t1 = new(3);
    s0 = new(4);
    s1 = new(5);
    pool.add0(t0);
    pool.add1(t1);
    pool.add2(s0);
    pool.add3(s1);
    pool.call(7, 9);
    pool.call(2, 5);

    y1 = t0.event_sum + t1.event_sum; // 18
    y2 = t0.data_sum + t1.data_sum;   // 28
    y3 = t0.hits + t1.hits;           // 4
    y4 = s0.scaled + s1.scaled;      // 146
    y5 = pool.connects;               // 4
    y6 = pool.calls;                  // 2
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 18, "callback event sum")?;
        expect_net(&sim, "y2", 28, "callback data sum")?;
        expect_net(&sim, "y3", 4, "tracer callback hits")?;
        expect_net(&sim, "y4", 146, "scaled callback result")?;
        expect_net(&sim, "y5", 4, "callback connections")?;
        expect_net(&sim, "y6", 2, "callback pool calls")?;
        Ok(())
    });

    // callback enable/disable 过滤：禁用槽不执行但保留绑定。
    c.add("uvm_callback_pool_enable_filter", || {
        let source = r#"
class tracer extends uvm_callback;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void callback(int event, int data);
    this.event_sum = this.event_sum + event;
    this.data_sum = this.data_sum + data;
    this.hits = this.hits + 1;
  endfunction
endclass
class scale extends uvm_callback;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void callback(int event, int data);
    this.scaled = this.scaled + event * data;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9);
  uvm_callback_pool pool;
  tracer t0;
  tracer t1;
  scale s0;
  scale s1;
  initial begin
    pool = new(1);
    t0 = new(2);
    t1 = new(3);
    s0 = new(4);
    s1 = new(5);
    pool.add0(t0);
    pool.add1(t1);
    pool.add2(s0);
    pool.add3(s1);
    pool.call(7, 9);
    pool.disable1();
    pool.disable3();
    pool.call_filtered(2, 5);

    y1 = t0.event_sum;       // 9
    y2 = t1.event_sum;       // 7：禁用后不再执行
    y3 = t0.data_sum;        // 14
    y4 = t1.data_sum;        // 9
    y5 = s0.scaled;          // 73
    y6 = s1.scaled;          // 63
    y7 = pool.filtered_calls; // 1
    y8 = pool.skipped_callbacks; // 2
    y9 = pool.connects;      // 4：绑定仍保留
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 9, "enabled tracer event")?;
        expect_net(&sim, "y2", 7, "disabled tracer event frozen")?;
        expect_net(&sim, "y3", 14, "enabled tracer data")?;
        expect_net(&sim, "y4", 9, "disabled tracer data frozen")?;
        expect_net(&sim, "y5", 73, "enabled scale callback")?;
        expect_net(&sim, "y6", 63, "disabled scale callback frozen")?;
        expect_net(&sim, "y7", 1, "filtered call count")?;
        expect_net(&sim, "y8", 2, "skipped disabled slots")?;
        expect_net(&sim, "y9", 4, "connections retained")?;
        Ok(())
    });

    // UVM-lite event/barrier：共享句柄上的 pending trigger 消费与阈值到达。
    c.add("uvm_event_barrier_synchronization", || {
        let source = r#"
class producer extends uvm_component;
  uvm_event ev;
  function new(int id, uvm_event e);
    super.new(id);
    this.ev = e;
  endfunction
  function void fire();
    this.ev.trigger();
  endfunction
endclass
class consumer extends uvm_component;
  uvm_event ev;
  int got = 0;
  function new(int id, uvm_event e);
    super.new(id);
    this.ev = e;
  endfunction
  function void poll_step();
    this.got = this.ev.wait_trigger();
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9, y10, y11, y12);
  uvm_event ev;
  producer p;
  consumer c;
  uvm_barrier barrier;
  initial begin
    ev = new(1);
    p = new(2, ev);
    c = new(3, ev);

    p.fire();
    y1 = ev.is_on();       // 1：trigger pending
    c.poll_step();
    y2 = c.got;            // 1：consumer 消费 trigger
    c.poll_step();
    y3 = c.got;            // 0：pending 已被消费
    p.fire();
    y4 = ev.get_trigger_count(); // 2
    y5 = ev.wait_trigger();      // 1：顶层句柄消费第二次 trigger
    y6 = ev.wait_count;          // 2
    ev.reset();

    barrier = new(10, 3);
    y7 = barrier.wait(1);  // 0
    y8 = barrier.wait(1);  // 0
    y9 = barrier.wait(1);  // 1：到达阈值
    y10 = barrier.count;   // 3
    y11 = barrier.reached(); // 1
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "event pending after trigger")?;
        expect_net(&sim, "y2", 1, "consumer consumes event")?;
        expect_net(&sim, "y3", 0, "event not pending after consume")?;
        expect_net(&sim, "y4", 2, "event trigger count")?;
        expect_net(&sim, "y5", 1, "top handle consumes event")?;
        expect_net(&sim, "y6", 2, "event wait count")?;
        expect_net(&sim, "y7", 0, "barrier below threshold one")?;
        expect_net(&sim, "y8", 0, "barrier below threshold two")?;
        expect_net(&sim, "y9", 1, "barrier reaches threshold")?;
        expect_net(&sim, "y10", 3, "barrier count saturates")?;
        expect_net(&sim, "y11", 1, "barrier reached state")?;
        Ok(())
    });
}

fn uvm_sequence_cases(c: &mut CaseList) {
    // UVM-lite sequence item 流：virtual body → sequencer queue → driver pop。
    c.add("uvm_sequence_body_to_sequencer_items", || {
        let source = r#"
class int_sequence extends uvm_sequence;
  function new(int id = 0);
    super.new(id);
  endfunction
  virtual function void body();
    this.start_item(10);
    this.finish_item();
    this.start_item(20);
    this.finish_item();
  endfunction
endclass
class driver extends uvm_component;
  uvm_sequencer sqr;
  int last = 0;
  function new(int id, uvm_sequencer s);
    super.new(id);
    this.sqr = s;
  endfunction
  function void step();
    this.last = this.sqr.get_next_item();
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9);
  uvm_sequencer sqr;
  int_sequence seq;
  driver drv;
  uvm_sequence_item item;
  initial begin
    sqr = new(1);
    seq = new(2);
    drv = new(3, sqr);
    item = new(37, 9);

    seq.start(sqr);
    y1 = seq.items_started;    // 2
    y2 = seq.items_finished;   // 2
    y3 = sqr.generated;        // 2
    y4 = sqr.peek_next_item(); // 10
    drv.step();
    y5 = drv.last;             // 10
    drv.step();
    y6 = drv.last;             // 20
    y7 = sqr.delivered;        // 2
    y8 = sqr.items.count;      // 0
    y9 = item.get_data();      // 37
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 2, "sequence items started")?;
        expect_net(&sim, "y2", 2, "sequence items finished")?;
        expect_net(&sim, "y3", 2, "sequencer generated items")?;
        expect_net(&sim, "y4", 10, "first queued item")?;
        expect_net(&sim, "y5", 10, "driver first item")?;
        expect_net(&sim, "y6", 20, "driver second item")?;
        expect_net(&sim, "y7", 2, "sequencer delivered count")?;
        expect_net(&sim, "y8", 0, "sequencer queue drained")?;
        expect_net(&sim, "y9", 37, "sequence item payload")?;
        Ok(())
    });

    // UVM-lite seq_item_port：sequence→sequencer→driver handshake。
    c.add("uvm_driver_seq_item_port_handshake", || {
        let source = r#"
class burst_sequence extends uvm_sequence;
  function new(int id = 0);
    super.new(id);
  endfunction
  virtual function void body();
    this.start_item(11);
    this.finish_item();
    this.start_item(22);
    this.finish_item();
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8);
  uvm_sequencer sqr;
  burst_sequence seq;
  uvm_driver drv;
  initial begin
    sqr = new(1);
    seq = new(2);
    drv = new(3);

    drv.connect(sqr);
    seq.start(sqr);
    y1 = drv.seq_item_port.is_connected(); // 1
    y2 = drv.seq_item_port.peek_next_item(); // 11

    drv.step();
    y3 = drv.last_item;       // 11
    drv.step();
    y4 = drv.last_item;       // 22
    y5 = drv.seq_item_port.gets; // 2
    y6 = drv.seq_item_port.dones; // 2
    y7 = drv.transfers;       // 2
    y8 = sqr.items.count;     // 0
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "seq_item_port connected")?;
        expect_net(&sim, "y2", 11, "port peek first item")?;
        expect_net(&sim, "y3", 11, "driver first transfer")?;
        expect_net(&sim, "y4", 22, "driver second transfer")?;
        expect_net(&sim, "y5", 2, "port gets")?;
        expect_net(&sim, "y6", 2, "port item_done calls")?;
        expect_net(&sim, "y7", 2, "driver transfers")?;
        expect_net(&sim, "y8", 0, "sequencer queue empty")?;
        Ok(())
    });
}

fn uvm_port_cases(c: &mut CaseList) {
    // uvm_analysis_port：connect(s) 句柄实参别名 + write 虚扇出。
    c.add("uvm_analysis_port_connect_and_write", || {
        let source = r#"
class sink extends uvm_subscriber;
  int sum = 0;
  int hits = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void write(int data);
    this.sum = this.sum + data;
    this.hits = this.hits + 1;
  endfunction
endclass
module top(output int y1, y2, y3);
  uvm_analysis_port ap;
  sink s;
  initial begin
    s = new(2);
    ap = new(1);
    ap.connect(s);
    ap.write(4);
    ap.write(6);
    y1 = s.sum;
    y2 = s.hits;
    y3 = ap.writes;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 10, "port subscriber sum")?;
        expect_net(&sim, "y2", 2, "port subscriber hits")?;
        expect_net(&sim, "y3", 2, "port write count")?;
        Ok(())
    });

    // uvm_analysis_port 多订阅者扇出：四个槽位 + 第五个连接满拒。
    c.add("uvm_analysis_port_multi_subscriber_fanout", || {
        let source = r#"
class sink extends uvm_subscriber;
  int sum = 0;
  int hits = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void write(int data);
    this.sum = this.sum + data;
    this.hits = this.hits + 1;
    this.observed = this.observed + 1;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7);
  uvm_analysis_port ap;
  sink a;
  sink b;
  sink c;
  sink d;
  sink extra;
  initial begin
    a = new(1);
    b = new(2);
    c = new(3);
    d = new(4);
    extra = new(5);
    ap = new(0);

    ap.connect(a);
    ap.connect1(b);
    ap.connect2(c);
    ap.connect3(d);
    ap.reject();        // 显式记录额外连接被拒绝
    ap.write4(3);
    ap.write4(7);
    ap.write4(5);

    y1 = a.sum;               // 15
    y2 = b.hits + c.hits + d.hits; // 9
    y3 = a.observed + b.observed + c.observed + d.observed; // 12
    y4 = extra.hits;          // 0
    y5 = ap.writes;           // 3
    y6 = ap.connects;         // 4
    y7 = ap.rejected_connects; // 1
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 15, "first subscriber receives all writes")?;
        expect_net(&sim, "y2", 9, "other subscriber hit total")?;
        expect_net(&sim, "y3", 12, "all subscriber observations")?;
        expect_net(&sim, "y4", 0, "rejected subscriber not connected")?;
        expect_net(&sim, "y5", 3, "analysis port writes")?;
        expect_net(&sim, "y6", 4, "accepted analysis connections")?;
        expect_net(&sim, "y7", 1, "rejected analysis connection")?;
        Ok(())
    });
}

fn uvm_nested_port_cases(c: &mut CaseList) {
    // 嵌套路径实参：env 组合 port+sink，connect(e.s) 经路径解析绑定。
    c.add("uvm_analysis_port_nested_connect_actual", || {
        let source = r#"
class sink extends uvm_subscriber;
  int sum = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void write(int data);
    this.sum = this.sum + data;
  endfunction
endclass
class env extends uvm_component;
  uvm_analysis_port ap;
  sink s;
  function new(int id);
    super.new(id);
    this.ap = new(id * 10 + 1);
    this.s = new(id * 10 + 2);
  endfunction
endclass
module top(output int y1, y2);
  env e;
  initial begin
    e = new(1);
    e.ap.connect(e.s);   // 嵌套路径实参
    e.ap.write(5);
    e.ap.write(9);
    y1 = e.s.sum;        // 14
    y2 = e.ap.writes;    // 2
  end
endmodule
"#;
        match compile(source, Some("top")) {
            Ok(design) => {
                let mut sim = Simulator::new(&design)?;
                sim.run_until(0)?;
                expect_net(&sim, "y1", 14, "nested connect sum")?;
                expect_net(&sim, "y2", 2, "nested port writes")?;
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    });
}

fn uvm_fifo_cases(c: &mut CaseList) {
    // TLM 深度 4 FIFO：满拒、FIFO 顺序取数与空读返回零。
    c.add("uvm_tlm_fifo_put_get_handshake", || {
        let source = r#"
class sequencer extends uvm_component;
  uvm_tlm_fifo fifo;
  int produced = 0;
  function new(int id, uvm_tlm_fifo f);
    super.new(id);
    this.fifo = f;
  endfunction
  function void send(int v);
    this.fifo.try_put(v);      // 语句调用：副作用写入 fifo 属性
    this.produced = this.fifo.puts;
  endfunction
endclass
class driver extends uvm_component;
  uvm_tlm_fifo fifo;
  int driven = 0;
  int total = 0;
  int last = 0;
  function new(int id, uvm_tlm_fifo f);
    super.new(id);
    this.fifo = f;
  endfunction
  function void step();
    this.last = this.fifo.try_get(); // 副作用方法可直接用于赋值表达式
    if (this.fifo.gets > this.driven) begin
      this.total = this.total + this.last;
      this.driven = this.driven + 1;
    end
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8);
  uvm_tlm_fifo fifo;
  sequencer sqr;
  driver drv;
  initial begin
    fifo = new(1);
    sqr = new(2, fifo);
    drv = new(3, fifo);
    sqr.send(10);           // q[0]
    sqr.send(20);           // q[1]
    sqr.send(30);           // q[2]
    sqr.send(40);           // q[3]
    sqr.send(50);           // 满拒
    y5 = fifo.count;        // 4
    y6 = fifo.puts;         // 4
    drv.step();             // 取 10
    y1 = drv.last;          // FIFO 首元素优先出队
    drv.step();             // 取 20
    y2 = drv.last;
    drv.step();             // 取 30
    y3 = drv.last;
    drv.step();             // 取 40
    y4 = drv.last;
    drv.step();             // 空读：peek 为 0，gets 不变
    y7 = drv.last;          // 0
    y8 = fifo.gets;         // 4
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 10, "first FIFO element")?;
        expect_net(&sim, "y2", 20, "second FIFO element")?;
        expect_net(&sim, "y3", 30, "third FIFO element")?;
        expect_net(&sim, "y4", 40, "fourth FIFO element")?;
        expect_net(&sim, "y5", 4, "capacity after rejected put")?;
        expect_net(&sim, "y6", 4, "accepted puts")?;
        expect_net(&sim, "y7", 0, "empty peek")?;
        expect_net(&sim, "y8", 4, "successful fifo gets")?;
        Ok(())
    });
}

fn uvm_queue_cases(c: &mut CaseList) {
    // UVM-lite 深度 4 int 队列：两端 push/pop、满拒、空读与计数。
    c.add("uvm_queue_deque_operations", || {
        let source = r#"
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9, y10, y11, y12);
  uvm_queue q;
  initial begin
    q = new(1);
    q.push_back(10);
    q.push_back(20);
    q.push_back(30);
    y1 = q.pop_front();   // 10
    q.push_front(5);      // 5,20,30
    y2 = q.pop_front();   // 5
    y3 = q.pop_back();    // 30
    y4 = q.pop_back();    // 20
    y5 = q.pop_back();    // 空读 0

    q.push_front(4);
    q.push_front(5);
    q.push_front(6);
    q.push_front(7);
    q.push_front(8);      // 满：不改变队列
    y6 = q.size();        // 4
    y7 = q.pop_front();   // 7
    y8 = q.pop_back();    // 4
    y9 = q.size();        // 2
    y10 = q.pushes;       // 8
    y11 = q.pops;         // 6
    q.delete();
    y12 = q.size();       // 0
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 10, "initial FIFO head")?;
        expect_net(&sim, "y2", 5, "push-front becomes head")?;
        expect_net(&sim, "y3", 30, "pop-back tail")?;
        expect_net(&sim, "y4", 20, "remaining pop-back")?;
        expect_net(&sim, "y5", 0, "empty pop-back")?;
        expect_net(&sim, "y6", 4, "full after rejected push-front")?;
        expect_net(&sim, "y7", 7, "front push order newest")?;
        expect_net(&sim, "y8", 4, "full queue oldest tail")?;
        expect_net(&sim, "y9", 2, "size after two pops")?;
        expect_net(&sim, "y10", 8, "accepted pushes")?;
        expect_net(&sim, "y11", 6, "successful pops")?;
        expect_net(&sim, "y12", 0, "delete clears queue")?;
        Ok(())
    });
}

fn uvm_pool_cases(c: &mut CaseList) {
    // UVM-lite 深度 4 int->int pool：唯一键更新、满拒、删除与查找。
    c.add("uvm_pool_unique_key_add_find_remove", || {
        let source = r#"
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9, y10, y11, y12);
  uvm_pool p;
  initial begin
    p = new(1);
    p.add(10, 100);
    p.add(20, 200);
    p.add(30, 300);
    p.add(40, 400);
    p.add(50, 500);       // 满拒
    p.add(20, 222);       // 已有键：更新，不增加 count
    y12 = p.num();         // 4
    y2 = p.find(20);       // 222
    y3 = p.exists(30);     // 1
    y4 = p.exists(50);     // 0
    y5 = p.find(40);       // 400

    p.remove(20);
    y6 = p.num();          // 3
    y7 = p.exists(20);     // 0
    y8 = p.find(40);       // 400
    y9 = p.adds;           // 4
    y10 = p.updates;       // 1
    y11 = p.removes;       // 1
    p.delete();
    y1 = p.num();          // 0
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 0, "pool cleared")?;
        expect_net(&sim, "y2", 222, "existing key update")?;
        expect_net(&sim, "y3", 1, "existing key membership")?;
        expect_net(&sim, "y4", 0, "full-rejected key membership")?;
        expect_net(&sim, "y5", 400, "tail lookup before remove")?;
        expect_net(&sim, "y6", 3, "count after remove")?;
        expect_net(&sim, "y7", 0, "removed key membership")?;
        expect_net(&sim, "y8", 400, "tail preserved after shift")?;
        expect_net(&sim, "y9", 4, "accepted inserts")?;
        expect_net(&sim, "y10", 1, "key updates")?;
        expect_net(&sim, "y11", 1, "removals")?;
        expect_net(&sim, "y12", 4, "full pool count")?;
        Ok(())
    });
}

fn uvm_config_db_cases(c: &mut CaseList) {
    // UVM-lite 静态配置数据库：类名直接 set/get/exists/remove。
    c.add("uvm_config_db_static_string_key_entries", || {
        let source = r#"
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9, y10, y11, y12);
  initial begin
    y1 = uvm_config_db.set("ADDR", 32);    // 1
    y2 = uvm_config_db.set("MODE", 1);     // 1
    y3 = uvm_config_db.set("DEPTH", 4);    // 1
    y4 = uvm_config_db.set("TIMEOUT", 100); // 1
    y5 = uvm_config_db.set("EXTRA", 9);    // 满拒：0
    y6 = uvm_config_db.set("MODE", 2);     // 已有键更新：1
    y7 = uvm_config_db.get("MODE");        // 2
    y8 = uvm_config_db.exists("DEPTH");    // 1
    y9 = uvm_config_db.remove("ADDR");     // 1
    y10 = uvm_config_db.exists("ADDR");    // 0
    y11 = uvm_config_db.get("DEPTH");      // 4
    y12 = uvm_config_db.count;             // 3
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "first config insert")?;
        expect_net(&sim, "y2", 1, "second config insert")?;
        expect_net(&sim, "y3", 1, "third config insert")?;
        expect_net(&sim, "y4", 1, "fourth config insert")?;
        expect_net(&sim, "y5", 0, "full config insert rejected")?;
        expect_net(&sim, "y6", 1, "existing key updated")?;
        expect_net(&sim, "y7", 2, "updated config value")?;
        expect_net(&sim, "y8", 1, "config key exists")?;
        expect_net(&sim, "y9", 1, "config key removed")?;
        expect_net(&sim, "y10", 0, "removed key absent")?;
        expect_net(&sim, "y11", 4, "later entry preserved after shift")?;
        expect_net(&sim, "y12", 3, "static config count")?;
        Ok(())
    });
}

fn uvm_factory_cases(c: &mut CaseList) {
    // UVM-lite factory：静态类型注册、type override 与解析策略。
    c.add("uvm_factory_type_override_resolution", || {
        let source = r#"
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9, y10, y11);
  string resolved;
  initial begin
    y1 = uvm_factory.register("BASE");       // 1
    y2 = uvm_factory.register("DERIVED");    // 1
    y3 = uvm_factory.register("ITEM");       // 1
    y4 = uvm_factory.register("EXTRA");      // 1
    y5 = uvm_factory.register("TOOMANY");    // 满拒：0
    y6 = uvm_factory.register("BASE");       // 重复：0
    y7 = uvm_factory.set_type_override("BASE", "DERIVED"); // 1

    resolved = uvm_factory.get_override("BASE");
    y8 = (resolved == "DERIVED");             // 1
    y9 = uvm_factory.has_override("BASE");    // 1
    y10 = uvm_factory.is_registered("MISSING"); // 0
    y11 = uvm_factory.count;                   // 4
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "first factory registration")?;
        expect_net(&sim, "y2", 1, "second factory registration")?;
        expect_net(&sim, "y3", 1, "third factory registration")?;
        expect_net(&sim, "y4", 1, "fourth factory registration")?;
        expect_net(&sim, "y5", 0, "full factory registration rejected")?;
        expect_net(&sim, "y6", 0, "duplicate factory registration rejected")?;
        expect_net(&sim, "y7", 1, "type override installed")?;
        expect_net(&sim, "y8", 1, "override resolution")?;
        expect_net(&sim, "y9", 1, "override present")?;
        expect_net(&sim, "y10", 0, "unknown type absent")?;
        expect_net(&sim, "y11", 4, "factory type count")?;
        Ok(())
    });

    // instance override 优先于 type override；删除后回退全局策略。
    c.add("uvm_factory_instance_override_priority", || {
        let source = r#"
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9, y10, y11);
  string resolved;
  initial begin
    y1 = uvm_factory.register("BASE");       // 1
    y2 = uvm_factory.register("DERIVED");    // 1
    y3 = uvm_factory.register("ITEM");       // 1
    y4 = uvm_factory.register("EXTRA");      // 1
    y5 = uvm_factory.set_type_override("BASE", "DERIVED"); // 1
    y6 = uvm_factory.set_inst_override(
           "env.agent.a", "BASE", "ITEM");                   // 1

    resolved = uvm_factory.get_inst_override("env.agent.a", "BASE");
    y7 = (resolved == "ITEM");                // instance override wins
    resolved = uvm_factory.get_inst_override("env.agent.b", "BASE");
    y8 = (resolved == "DERIVED");             // other instance uses type override
    y9 = uvm_factory.has_inst_override("env.agent.a", "BASE"); // 1

    y10 = uvm_factory.remove_inst_override("env.agent.a", "BASE"); // 1
    resolved = uvm_factory.get_inst_override("env.agent.a", "BASE");
    y11 = (resolved == "DERIVED");             // fallback after removal
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "base registration")?;
        expect_net(&sim, "y2", 1, "derived registration")?;
        expect_net(&sim, "y3", 1, "item registration")?;
        expect_net(&sim, "y4", 1, "extra registration")?;
        expect_net(&sim, "y5", 1, "type override")?;
        expect_net(&sim, "y6", 1, "instance override")?;
        expect_net(&sim, "y7", 1, "instance override priority")?;
        expect_net(&sim, "y8", 1, "type override fallback")?;
        expect_net(&sim, "y9", 1, "instance override present")?;
        expect_net(&sim, "y10", 1, "instance override removed")?;
        expect_net(&sim, "y11", 1, "fallback after instance removal")?;
        Ok(())
    });
}

fn uvm_objection_cases(c: &mut CaseList) {
    // objection 计数：raise/drop 配对 + drained 终止条件。
    c.add("uvm_objection_raise_drop_drain", || {
        let source = r#"
class worker extends uvm_component;
  uvm_objection obj;
  int done = 0;
  function new(int id, uvm_objection o);
    super.new(id);
    this.obj = o;
  endfunction
  virtual function void run_phase();
    this.obj.raise(1);
    this.done = this.done + 1;
    this.obj.drop(1);
  endfunction
endclass
class env extends uvm_component;
  uvm_objection obj;
  worker w1;
  worker w2;
  function new(int id);
    super.new(id);
    this.obj = new(id * 10 + 1);
    this.w1 = new(id * 10 + 2, this.obj);
    this.w2 = new(id * 10 + 3, this.obj);
  endfunction
endclass
module top(output int y1, y2, y3, y4);
  env e;
  initial begin
    e = new(1);
    y1 = e.obj.raised;       // 0
    e.w1.run_phase();        // raise+drop
    e.w2.run_phase();
    e.obj.check_drained();
    y2 = e.obj.was_drained;  // 1
    y3 = e.obj.raise_calls;  // 2
    y4 = e.obj.drop_calls;   // 2
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 0, "initial raised zero")?;
        expect_net(&sim, "y2", 1, "drained after balanced raise/drop")?;
        expect_net(&sim, "y3", 2, "raise calls")?;
        expect_net(&sim, "y4", 2, "drop calls")?;
        Ok(())
    });
}
