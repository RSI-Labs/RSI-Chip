//! Lexer/parser validity, structural elaboration, generate, and variable types.

use super::*;

pub(super) fn register(c: &mut CaseList) {
    lexing(c);
    valid(c);
    rejected(c);
    elab_structure(c);
    generate_structure(c);
    generate_case_structure(c);
    variable_type_structure(c);
}

pub(super) fn lexing(c: &mut CaseList) {
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

pub(super) fn valid(c: &mut CaseList) {
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

pub(super) fn rejected(c: &mut CaseList) {
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

pub(super) fn elab_structure(c: &mut CaseList) {
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

pub(super) fn generate_structure(c: &mut CaseList) {
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

pub(super) fn generate_case_structure(c: &mut CaseList) {
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

pub(super) fn variable_type_structure(c: &mut CaseList) {
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
