use super::*;
use eda_core::ir::{BinOp, Expr, Range, Stmt};

fn compile_ok(src: &str) -> Design {
    compile(src, None).unwrap_or_else(|e| panic!("compile failed: {}", e))
}

#[test]
fn simple_assign_flattens() {
    let d = compile_ok("module m(input a, b, output y); assign y = a & b; endmodule");
    assert_eq!(d.name, "m");
    assert_eq!(d.ports.len(), 3);
    assert_eq!(d.n_nets, 3);
    assert_eq!(d.assigns.len(), 1);
    assert!(matches!(d.assigns[0].e, Expr::Bin { op: BinOp::And, .. }));
}

#[test]
fn multiple_named_sources_compile_with_source_manager_mapping() {
    let sources = [
        HdlSource::new(
            "child.sv",
            "module child(input a, output y); assign y = ~a; endmodule\n",
        ),
        HdlSource::new(
            "top.sv",
            "module top(input x, output z);\nwire w;\nchild u1(.a(x), .y(w));\nchild u2(.a(w), .y(z));\nendmodule\n",
        ),
    ];
    let manager = SourceManager::new(&sources);
    assert_eq!(manager.locate(1), Some(("child.sv", 1)));
    assert_eq!(manager.locate(3), Some(("top.sv", 1)));
    assert_eq!(manager.locate(6), Some(("top.sv", 4)));

    let design = compile_sources(&sources, Some("top")).expect("compile named sources");
    assert_eq!(design.name, "top");
    assert_eq!(design.instances_of_module("child").len(), 2);
    // Two functional child assigns plus four flattened port connections.
    assert_eq!(design.assigns.len(), 6);
}

#[test]
fn syntax_errors_in_named_sources_report_the_original_file() {
    let sources = [
        HdlSource::new(
            "first.sv",
            "module first(output y); assign y = 1'b0; endmodule\n",
        ),
        HdlSource::new(
            "broken.sv",
            "module broken(output y);\nwire missing_semicolon\nassign y = 1'b0;\nendmodule\n",
        ),
    ];
    let error =
        compile_sources(&sources, Some("first")).expect_err("malformed source must be rejected");
    assert!(error.starts_with("broken.sv:3: line 3:"), "{error}");
}

#[test]
fn vectors_and_parts() {
    let d = compile_ok("module m(input [3:0] a, output [1:0] y); assign y = a[3:2]; endmodule");
    assert_eq!(d.n_nets, 6);
    assert_eq!(d.assigns[0].lv.range, Range::Span { msb: 1, lsb: 0 });
    match &d.assigns[0].e {
        Expr::Ref { range, .. } => assert_eq!(*range, Range::Span { msb: 3, lsb: 2 }),
        other => panic!("expected ref, got {:?}", other),
    }
}

#[test]
fn reversed_range_decl() {
    let d = compile_ok("module m(input [0:3] a, output y); assign y = a[2]; endmodule");
    // [0:3]: source index 2 -> decl offset 3 - 2 = 1
    match &d.assigns[0].e {
        Expr::Ref { range, .. } => assert_eq!(*range, Range::Bit(1)),
        other => panic!("expected ref, got {:?}", other),
    }
}

#[test]
fn dynamic_packed_select_assignment_builds_conditional_concat() {
    let d = compile_ok(
        "module m(input clk, input [2:0] selector, input value, output logic [7:0] q); \
         always_ff @(posedge clk) q[selector] <= value; endmodule",
    );
    assert_eq!(d.processes.len(), 1);
    let Stmt::NBl { lv, e } = &d.processes[0].body else {
        panic!(
            "expected nonblocking assignment, got {:?}",
            d.processes[0].body
        );
    };
    assert_eq!(lv.decl_width, 8);
    assert_eq!(lv.range, Range::Span { msb: 7, lsb: 0 });
    let Expr::Concat(bits) = e else {
        panic!("expected conditional bit concatenation, got {e:?}");
    };
    assert_eq!(bits.len(), 8);
    assert!(bits.iter().all(|bit| matches!(bit, Expr::Cond { .. })));

    let blocking = compile_ok(
        "module m(input [1:0] selector, input value, output logic [3:0] q); \
         initial q[selector] = value; endmodule",
    );
    assert_eq!(blocking.initials.len(), 1);
    let Stmt::Bl { lv, e } = &blocking.initials[0] else {
        panic!(
            "expected blocking assignment, got {:?}",
            blocking.initials[0]
        );
    };
    assert_eq!(lv.decl_width, 4);
    assert_eq!(lv.range, Range::Span { msb: 3, lsb: 0 });
    let Expr::Concat(blocking_bits) = e else {
        panic!("expected blocking conditional concatenation, got {e:?}");
    };
    assert_eq!(blocking_bits.len(), 4);
    assert!(blocking_bits
        .iter()
        .all(|bit| matches!(bit, Expr::Cond { .. })));

    let reversed = compile_ok(
        "module m(input clk, input [2:0] selector, input value, output logic [0:7] q); \
         always_ff @(posedge clk) q[selector] <= value; endmodule",
    );
    assert_eq!(reversed.processes.len(), 1);
    let Stmt::NBl { lv, e } = &reversed.processes[0].body else {
        panic!(
            "expected reversed nonblocking assignment, got {:?}",
            reversed.processes[0].body
        );
    };
    assert_eq!(lv.decl_width, 8);
    assert_eq!(lv.range, Range::Span { msb: 7, lsb: 0 });
    let Expr::Concat(reversed_bits) = e else {
        panic!("expected reversed conditional concatenation, got {e:?}");
    };
    assert_eq!(reversed_bits.len(), 8);
    assert!(reversed_bits
        .iter()
        .all(|bit| matches!(bit, Expr::Cond { .. })));
}

#[test]
fn out_of_range_index_is_error() {
    assert!(compile(
        "module m(input [3:0] a, output y); assign y = a[7]; endmodule",
        None
    )
    .is_err());
}

#[test]
fn undeclared_ident_is_error() {
    assert!(compile("module m(output y); assign y = q; endmodule", None).is_err());
}

#[test]
fn parameterized_width() {
    let d = compile_ok(
        "module m #(parameter W = 4) (input [W-1:0] a, output [W-1:0] y); assign y = a + 1; endmodule",
    );
    assert_eq!(d.n_nets, 8);
}

#[test]
fn systemverilog_variable_types_have_default_widths() {
    let d = compile_ok(
        "module m(input int a, output byte b, output tri z, output bit d, output logic [7:0] q); \
         shortint s; longint l; integer i; logic x; \
         endmodule",
    );
    for (name, width, is_reg) in [
        ("a", 32, true),
        ("b", 8, true),
        ("z", 1, false),
        ("d", 1, true),
        ("q", 8, true),
        ("s", 16, true),
        ("l", 64, true),
        ("i", 32, true),
        ("x", 1, true),
    ] {
        let declaration = d
            .net_decls
            .iter()
            .find(|declaration| declaration.name == name)
            .unwrap_or_else(|| panic!("missing declaration {name}"));
        assert_eq!(declaration.width, width, "{name} width");
        assert_eq!(declaration.is_reg, is_reg, "{name} variable flag");
    }
}

#[test]
fn package_constants_import_and_scoped_references_elaborate() {
    let d = compile_ok(
        r#"
package base;
parameter WIDTH = 8;
endpackage

package bus;
parameter WIDTH = base::WIDTH + 1;
localparam DEPTH = 4;
endpackage

import bus::*;

module top(
input  [bus::WIDTH-1:0] a,
output [bus::DEPTH-1:0] d
);
wire [WIDTH-1:0] wide;
wire [bus::DEPTH-1:0] generated_wire;
generate
if (bus::WIDTH > 4) begin : wide_bus
    assign generated_wire = d;
end else begin : narrow_bus
    assign generated_wire = d;
end
endgenerate
assign d = wide;
endmodule
"#,
    );
    assert_eq!(d.port("a").unwrap().width, 9);
    assert_eq!(d.port("d").unwrap().width, 4);
    assert_eq!(
        d.net_decls.iter().find(|n| n.name == "wide").unwrap().width,
        9
    );
    assert_eq!(
        d.net_decls
            .iter()
            .find(|n| n.name == "generated_wire")
            .unwrap()
            .width,
        4
    );
}

#[test]
fn constant_arithmetic_overflow_fails_closed() {
    let cases = [
        (
            r#"
module top;
parameter HALF = 64'd4611686018427387903;
parameter MAX = HALF * 2 + 1;
parameter BAD = MAX + 1;
wire [BAD-1:0] y;
assign y = 1'b0;
endmodule
"#,
            "constant addition overflows in module 'top'",
        ),
        (
            r#"
module top;
parameter HALF = 64'd4611686018427387903;
parameter BAD = HALF * 3;
wire [BAD-1:0] y;
assign y = 1'b0;
endmodule
"#,
            "constant multiplication overflows in module 'top'",
        ),
        (
            r#"
module top;
parameter HALF = 64'd4611686018427387903;
parameter MIN = -HALF * 2 - 2;
parameter BAD = -MIN;
wire [BAD-1:0] y;
assign y = 1'b0;
endmodule
"#,
            "constant unary negation overflows in module 'top'",
        ),
        (
            r#"
module top;
parameter HALF = 64'd4611686018427387903;
parameter MAX = HALF * 2 + 1;
parameter BAD = MAX << 64;
wire [BAD-1:0] y;
assign y = 1'b0;
endmodule
"#,
            "constant left shift count 64 is outside 0..63 in module 'top'",
        ),
    ];

    for (source, expected) in cases {
        let error =
            compile(source, None).expect_err("constant arithmetic overflow must not wrap silently");
        assert!(error.contains(expected), "unexpected error: {error}");
    }
}

#[test]
fn wildcard_import_name_collisions_fail_closed() {
    let cases = [
        (
            r#"
package left;
parameter VALUE = 1;
endpackage

package right;
parameter VALUE = 2;
endpackage

import left::*;
import right::*;

module top(output [VALUE-1:0] y);
assign y = 1'b0;
endmodule
"#,
            "imported name 'VALUE' is ambiguous between packages 'left' and 'right'",
        ),
        (
            r#"
package left;
typedef struct packed { logic a; } item_t;
endpackage

package right;
typedef struct packed { logic b; } item_t;
endpackage

import left::*;
import right::*;

module top(output logic y);
assign y = 1'b0;
endmodule
"#,
            "imported name 'item_t' is ambiguous between packages 'left' and 'right'",
        ),
        (
            r#"
package left;
function logic [31:0] value(input logic dummy);
value = 1;
endfunction
endpackage

package right;
function logic [31:0] value(input logic dummy);
value = 2;
endfunction
endpackage

import left::*;
import right::*;

module top(output logic y);
assign y = 1'b0;
endmodule
"#,
            "imported name 'value' is ambiguous between packages 'left' and 'right'",
        ),
    ];

    for (source, expected) in cases {
        let error = compile(source, None).expect_err("ambiguous wildcard imports must fail closed");
        assert!(error.contains(expected), "unexpected error: {error}");
    }

    let design = compile_ok(
        r#"
package single;
parameter FIRST = 1;
parameter SECOND = 2;
endpackage

import single::FIRST;
import single::*;

module top(output [FIRST+SECOND-1:0] y);
assign y = 1'b0;
endmodule
"#,
    );
    assert_eq!(design.port("y").unwrap().width, 3);
}

#[test]
fn enum_typedefs_define_width_and_constants() {
    let local = compile_ok(
        r#"
module top(output reg [1:0] y);
typedef enum logic [1:0] { IDLE = 2, RUN, DONE } state_t;
state_t state;
initial begin
state = IDLE;
y = state;
end
endmodule
"#,
    );
    assert_eq!(
        local
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "state")
            .unwrap()
            .width,
        2
    );

    let package = compile_ok(
        r#"
package states;
parameter BASE = 3;
typedef enum [2:0] { X = BASE, Y, Z } state_t;
endpackage

import states::*;

module top(output state_t q);
assign q = X;
endmodule
"#,
    );
    assert_eq!(package.port("q").unwrap().width, 3);
    assert!(matches!(
        package
            .assigns
            .iter()
            .find(|assignment| assignment.lv.net == package.port("q").unwrap().base)
            .map(|assignment| &assignment.e),
        Some(ExprIR::Const(value)) if value.to_u64() == Some(3)
    ));
}

#[test]
fn packed_struct_typedefs_elaborate_fields_and_members() {
    let local = compile_ok(
        r#"
module top(input logic [7:0] value, output logic [3:0] high, output logic low);
typedef struct packed {
logic [3:0] first;
logic second;
} pair_t;
pair_t pair;
assign pair = value;
assign high = pair.first;
assign low = pair.second;
endmodule
"#,
    );
    assert_eq!(
        local
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "pair")
            .unwrap()
            .width,
        5
    );
    assert_eq!(local.port("high").unwrap().width, 4);
    assert_eq!(local.port("low").unwrap().width, 1);

    let package = compile_ok(
        r#"
package bus;
typedef struct packed {
logic [7:0] address;
logic [1:0] byte_enable;
} request_t;
endpackage

import bus::*;

module top(input request_t request, output [7:0] address, output [1:0] enable);
assign address = request.address;
assign enable = request.byte_enable;
endmodule
"#,
    );
    assert_eq!(package.port("request").unwrap().width, 10);
    let request = package.port("request").unwrap();
    let address = package
        .assigns
        .iter()
        .find(|assignment| assignment.lv.net == package.port("address").unwrap().base)
        .unwrap();
    let enable = package
        .assigns
        .iter()
        .find(|assignment| assignment.lv.net == package.port("enable").unwrap().base)
        .unwrap();
    assert!(matches!(
        &address.e,
        ExprIR::Ref { net, range, .. }
            if *net == request.base && *range == Range::Span { msb: 9, lsb: 2 }
    ));
    assert!(matches!(
        &enable.e,
        ExprIR::Ref { net, range, .. }
            if *net == request.base && *range == Range::Span { msb: 1, lsb: 0 }
    ));
}

#[test]
fn packed_union_typedefs_overlap_members() {
    let local = compile_ok(
        r#"
module top(input logic [7:0] value, output logic [7:0] bytes, output logic [3:0] nibble);
typedef union packed {
logic [7:0] bytes;
logic [3:0] nibble;
} view_t;
view_t view;
assign view = value;
assign bytes = view.bytes;
assign nibble = view.nibble;
endmodule
"#,
    );
    assert_eq!(
        local
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "view")
            .unwrap()
            .width,
        8
    );

    let package = compile_ok(
        r#"
package views;
typedef union packed {
logic [7:0] wide;
logic [1:0] narrow;
} view_t;
endpackage

import views::*;

module top(input view_t view, output [7:0] wide, output [1:0] narrow);
assign wide = view.wide;
assign narrow = view.narrow;
endmodule
"#,
    );
    assert_eq!(package.port("view").unwrap().width, 8);
    let view = package.port("view").unwrap();
    let wide = package
        .assigns
        .iter()
        .find(|assignment| assignment.lv.net == package.port("wide").unwrap().base)
        .unwrap();
    let narrow = package
        .assigns
        .iter()
        .find(|assignment| assignment.lv.net == package.port("narrow").unwrap().base)
        .unwrap();
    assert!(matches!(
        &wide.e,
        ExprIR::Ref { net, range, .. }
            if *net == view.base && *range == Range::Span { msb: 7, lsb: 0 }
    ));
    assert!(matches!(
        &narrow.e,
        ExprIR::Ref { net, range, .. }
            if *net == view.base && *range == Range::Span { msb: 1, lsb: 0 }
    ));
}

#[test]
fn generate_for_loop_expands_assigns() {
    let d = compile_ok(
        "module m(input [3:0] a, output [3:0] y); generate for (genvar i=0; i<4; i=i+1) begin : bit_assign assign y[i] = a[i]; end endgenerate endmodule",
    );
    assert_eq!(d.assigns.len(), 4);
    for (expected, assign) in d.assigns.iter().enumerate() {
        assert_eq!(assign.lv.range, eda_core::ir::Range::Bit(expected));
    }
}

#[test]
fn generate_for_loop_accepts_declared_genvar_and_descending_iteration() {
    let d = compile_ok(
        "module m(input [2:0] a, output [2:0] y); \
         genvar i; \
         generate for (i = 2; i >= 0; i = i - 1) begin : bit_assign \
         assign y[i] = a[i]; \
         end endgenerate endmodule",
    );
    assert_eq!(d.assigns.len(), 3);
}

#[test]
fn generate_for_loop_genvar_can_be_declared_inside_generate_region() {
    let d = compile_ok(
        "module m(input [1:0] a, output [1:0] y); \
         generate genvar i; \
         for (i = 0; i < 2; i = i + 1) begin : bit_assign \
         assign y[i] = a[i]; \
         end endgenerate endmodule",
    );
    assert_eq!(d.assigns.len(), 2);
}

#[test]
fn false_generate_if_without_else_generates_no_items() {
    let d = compile_ok(
        "module m #(parameter ENABLE = 0) (input a, output y); \
         wire generated; \
         generate if (ENABLE) begin : enabled \
         assign generated = 1'b1; \
         end endgenerate \
         assign generated = 1'b0; \
         assign y = generated | a; \
         endmodule",
    );
    assert_eq!(d.assigns.len(), 2);
}

#[test]
fn generate_for_loop_reaches_the_final_iteration() {
    let d = compile_ok(
        "module m(output wire [0:0] y); \
         generate for (genvar i = 0; i < 10001; i = i + 1) begin : iterations \
         assign y = 1'b0; \
         end endgenerate endmodule",
    );
    assert_eq!(d.assigns.len(), 10001);
}

#[test]
fn generate_for_loop_reports_excessive_steps_instead_of_truncating() {
    let error = compile(
        "module m(output wire [0:0] y); \
         generate for (genvar i = 0; i < 131073; i = i + 1) begin : iterations \
         assign y = 1'b0; \
         end endgenerate endmodule",
        None,
    )
    .unwrap_err();
    assert!(error.contains("generate for-loop exceeds 131072 steps"));
}

#[test]
fn generate_if_selects_parameterized_branch() {
    let invert = compile_ok(
        "module m #(parameter INVERT = 1) (input a, output y); generate if (INVERT) begin : inv assign y = ~a; end else begin : pass assign y = a; end endgenerate endmodule",
    );
    assert_eq!(invert.assigns.len(), 1);
    assert!(matches!(
        invert.assigns[0].e,
        eda_core::ir::Expr::Un { op: UnOp::Not, .. }
    ));

    let identity = compile_ok(
        "module m #(parameter INVERT = 0) (input a, output y); generate if (INVERT) begin : inv assign y = ~a; end else begin : pass assign y = a; end endgenerate endmodule",
    );
    assert_eq!(identity.assigns.len(), 1);
    assert!(matches!(
        identity.assigns[0].e,
        eda_core::ir::Expr::Ref { .. }
    ));
}

#[test]
fn generate_case_selects_constant_item() {
    let source = |mode: usize| {
        format!(
            "module m #(parameter MODE = {mode}) (output [7:0] y); \
             generate case (MODE) \
             0: begin : zero assign y = 8'd10; end \
             1, 2: begin : one_or_two assign y = 8'd20; end \
             default: begin : other assign y = 8'd30; end \
             endcase endgenerate endmodule"
        )
    };
    for (mode, expected) in [(0, 10), (1, 20), (2, 20), (3, 30)] {
        let design = compile_ok(&source(mode));
        assert_eq!(design.assigns.len(), 1);
        match &design.assigns[0].e {
            ExprIR::Const(word) => assert_eq!(word.to_u64(), Some(expected)),
            other => panic!("expected constant, got {:?}", other),
        }
    }

    assert!(compile(
        "module m #(parameter MODE = 4) (output y); \
         generate case (MODE) 0: begin : zero assign y = 1'b0; end endcase endgenerate endmodule",
        None
    )
    .is_err());
}

#[test]
fn instance_flattening() {
    let d = compile_ok(
        "module inv(input a, output y); assign y = ~a; endmodule
         module top(input x, output z); wire w; inv u1(.a(x), .y(w)); inv u2(.a(w), .y(z)); endmodule",
    );
    assert_eq!(d.name, "top");
    assert_eq!(d.n_nets, 7); // x, z, w + 2 per instance
    assert!(d.net_decls.iter().any(|n| n.name == "u1.a"));
    assert!(d.net_decls.iter().any(|n| n.name == "u2.y"));
}

#[test]
fn instance_positional_and_params() {
    let d = compile_ok(
        "module add #(parameter W = 2) (input [W-1:0] a, b, output [W-1:0] s);
            assign s = a + b;
         endmodule
         module top(input [3:0] x, y, output [3:0] z);
            add #(4) u1(x, y, z);
         endmodule",
    );
    assert_eq!(d.name, "top");
    assert_eq!(d.n_nets, 24); // top 12 + child 12
}

#[test]
fn hierarchy_metadata_is_preserved() {
    let d = compile_ok(
        "module leaf(input a, output y); assign y = ~a; endmodule
         module sub(input a, output y); wire n; leaf u_leaf(.a(a), .y(n)); assign y = n; endmodule
         module top(input a, output y); sub u1(.a(a), .y(y)); endmodule",
    );
    assert_eq!(d.hierarchy.len(), 3);
    assert_eq!(d.hierarchy[0].path, "top");
    assert_eq!(d.hierarchy[0].parent, None);
    assert_eq!(d.hierarchy[0].depth, 0);
    assert_eq!(d.hierarchy[1].path, "top.u1");
    assert_eq!(d.hierarchy[1].module, "sub");
    assert_eq!(d.hierarchy[1].parent.as_deref(), Some("top"));
    assert_eq!(d.hierarchy[2].path, "top.u1.u_leaf");
    assert_eq!(d.hierarchy[2].module, "leaf");
    assert_eq!(d.hierarchy[2].parent.as_deref(), Some("top.u1"));
    assert_eq!(d.hierarchy[2].depth, 2);
    assert!(d.hierarchy[2].net_start >= d.hierarchy[1].net_start);
    assert!(d.hierarchy[2].net_end <= d.hierarchy[1].net_end);
    assert_eq!(
        d.hierarchy[2]
            .ports
            .iter()
            .map(|port| (port.name.as_str(), port.direction, port.width))
            .collect::<Vec<_>>(),
        vec![("a", Dir::In, 1), ("y", Dir::Out, 1)]
    );
    assert_eq!(d.instance("top.u1").unwrap().module, "sub");
    assert_eq!(d.instances_of_module("leaf").len(), 1);
    let leaf_a = d.hierarchy[2].ports[0].base;
    assert_eq!(
        d.owning_instance(leaf_a)
            .map(|instance| instance.path.as_str()),
        Some("top.u1.u_leaf")
    );
    assert!(d.hierarchy_report().contains("top.u1.u_leaf"));
}

#[test]
fn gate_primitives_become_assigns() {
    let d =
        compile_ok("module m(input a, b, output y, n); and g1(y, a, b); not g2(n, y); endmodule");
    assert_eq!(d.assigns.len(), 2);
}

#[test]
fn always_blocks_and_sensitivity() {
    let d = compile_ok(
        "module m(input clk, rst, d, output reg q);
            always @(posedge clk or posedge rst) if (rst) q <= 1'b0; else q <= d;
         endmodule",
    );
    assert_eq!(d.processes.len(), 1);
    assert!(matches!(&d.processes[0].body, Stmt::If { .. }));
}

#[test]
fn case_becomes_case() {
    let d = compile_ok(
        "module m(input [1:0] sel, output reg y);
            always @(*) case (sel) 2'b00: y = 1'b1; 2'b01: y = 1'b0; default: y = 1'b1; endcase
         endmodule",
    );
    match &d.processes[0].body {
        Stmt::Case { arms, dflt, .. } => {
            assert_eq!(arms.len(), 2);
            assert!(dflt.is_some());
        }
        other => panic!("expected case, got {:?}", other),
    }
}

#[test]
fn wildcard_case_lowers_to_ordered_conditionals() {
    let d = compile_ok(
        "module m(input [3:0] sel, output reg [1:0] y); always @(*) casez (sel) 4'b1???: y = 2'd3; default: y = 2'd0; endcase endmodule",
    );
    match &d.processes[0].body {
        Stmt::If { f: Some(_), .. } => {}
        other => panic!("expected ordered wildcard conditional, got {:?}", other),
    }
}

#[test]
fn simple_functions_inline_into_expressions() {
    let d = compile_ok(
        r#"
module top(input [7:0] a, b, output [7:0] sum, product);
function logic [7:0] add(input logic [7:0] left, right);
add = left + right;
endfunction
function logic [7:0] multiply(input logic [7:0] left, right);
multiply = left * right;
endfunction
assign sum = add(a, b);
assign product = multiply(a, b);
endmodule
"#,
    );
    assert_eq!(d.assigns.len(), 2);
    assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));
    assert!(matches!(&d.assigns[1].e, Expr::Bin { op: BinOp::Mul, .. }));

    assert!(compile(
        r#"
module top(input [7:0] a, output [7:0] y);
function logic [7:0] add(input logic [7:0] left, right);
add = left + right;
endfunction
assign y = add(a);
endmodule
"#,
        None
    )
    .is_err());
}

#[test]
fn modern_function_argument_lists_elaborate() {
    let design = compile_ok(
        r#"
module top(input int a, b, output int sum);
function automatic int add(int left, int right);
add = left + right;
endfunction
assign sum = add(a, b);
endmodule
"#,
    );
    assert_eq!(design.ports.len(), 3);
    assert!(matches!(
        &design.assigns[0].e,
        Expr::Bin { op: BinOp::Add, .. }
    ));
}

#[test]
fn package_functions_import_and_inline() {
    let d = compile_ok(
        r#"
package math;
function logic [7:0] add(input logic [7:0] left, right);
add = left + right;
endfunction

function logic [7:0] bitwise_or(input logic [7:0] left, right);
bitwise_or = left | right;
endfunction
endpackage

import math::add, math::bitwise_or;

module top(input [7:0] a, b, output [7:0] sum, union_result);
assign sum = add(a, b);
assign union_result = bitwise_or(a, b);
endmodule
"#,
    );
    assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));
    assert!(matches!(&d.assigns[1].e, Expr::Bin { op: BinOp::Or, .. }));

    assert!(compile(
        r#"
package math;
function logic [7:0] add(input logic [7:0] left, right);
add = left + right;
endfunction
endpackage

module top(input [7:0] a, output [7:0] y);
assign y = add(a, a);
endmodule
"#,
        None
    )
    .is_err());
}

#[test]
fn nested_function_calls_inline_and_recursion_is_rejected() {
    let d = compile_ok(
        r#"
module top(input [7:0] a, b, output [7:0] y);
function logic [7:0] add(input logic [7:0] left, right);
add = left + right;
endfunction
function logic [7:0] increment(input logic [7:0] value);
increment = add(value, 8'd1);
endfunction
function logic [7:0] pipeline(input logic [7:0] left, right);
pipeline = increment(add(left, right));
endfunction
assign y = pipeline(a, increment(b));
endmodule
"#,
    );
    assert_eq!(d.assigns.len(), 1);
    assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));

    assert!(compile(
        r#"
module recursive(input [7:0] a, output [7:0] y);
function logic [7:0] left(input logic [7:0] value);
left = right(value);
endfunction
function logic [7:0] right(input logic [7:0] value);
right = left(value);
endfunction
assign y = left(a);
endmodule
"#,
        None
    )
    .is_err());
}

#[test]
fn function_local_variables_inline_in_assignment_order() {
    let d = compile_ok(
        r#"
module top(input [7:0] a, b, c, output [7:0] y);
function logic [7:0] sum3(
input logic [7:0] left, middle, right);
logic [7:0] first;
logic [7:0] second = 8'd2;
first = left + middle;
second = first + right;
sum3 = second + 8'd1;
endfunction
assign y = sum3(a, b, c);
endmodule
"#,
    );
    assert_eq!(d.assigns.len(), 1);
    assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));

    assert!(compile(
        r#"
module bad(input [7:0] a, output [7:0] y);
function logic [7:0] f(input logic [7:0] value);
unknown = value;
f = unknown;
endfunction
assign y = f(a);
endmodule
"#,
        None
    )
    .is_err());
}

#[test]
fn function_if_else_assignments_merge_to_conditionals() {
    let d = compile_ok(
        r#"
module top(input sel, [7:0] a, b, output [7:0] y);
function logic [7:0] select(input logic selector,
input logic [7:0] left, right);
logic [7:0] result;
if (selector)
    result = left;
else
    result = right;
select = result;
endfunction
assign y = select(sel, a, b);
endmodule
"#,
    );
    assert_eq!(d.assigns.len(), 1);
    assert!(matches!(&d.assigns[0].e, Expr::Cond { .. }));

    let d = compile_ok(
        r#"
module top(input s0, s1, [7:0] a, b, output [7:0] y);
function logic [7:0] priority_select(input logic high, low,
input logic [7:0] left, right);
logic [7:0] result;
if (high)
    result = left;
else if (low)
    result = right;
else
    result = 8'd0;
priority_select = result;
endfunction
assign y = priority_select(s0, s1, a, b);
endmodule
"#,
    );
    assert!(matches!(&d.assigns[0].e, Expr::Cond { .. }));

    assert!(compile(
        r#"
module bad(input sel, [7:0] a, output [7:0] y);
function logic [7:0] f(input logic selector,
input logic [7:0] value);
if (selector)
    f = value;
f = 8'd0;
endfunction
assign y = f(sel, a);
endmodule
"#,
        None
    )
    .is_err());
}

#[test]
fn function_exact_case_assignments_merge_in_priority_order() {
    let d = compile_ok(
        r#"
module top(input [1:0] sel, [7:0] a, b, output [7:0] y);
function logic [7:0] decode(input logic [1:0] selector,
input logic [7:0] left, right);
logic [7:0] result;
case (selector)
    2'd0: result = left + 8'd1;
    2'd1, 2'd2: result = right ^ 8'd2;
    default: result = left & right;
endcase
decode = result;
endfunction
assign y = decode(sel, a, b);
endmodule
"#,
    );
    assert_eq!(d.assigns.len(), 1);
    assert!(matches!(&d.assigns[0].e, Expr::Cond { .. }));

    let d = compile_ok(
        r#"
module top(input [1:0] sel, [7:0] a, b, output [7:0] y);
function logic [7:0] decode(input logic [1:0] selector,
input logic [7:0] left, right);
case (selector)
    2'd0: decode = left;
    2'd1: decode = right;
    default: decode = left | right;
endcase
endfunction
assign y = decode(sel, a, b);
endmodule
"#,
    );
    assert!(matches!(&d.assigns[0].e, Expr::Cond { .. }));

    assert!(compile(
        r#"
module bad(input [1:0] sel, [7:0] a, output [7:0] y);
function logic [7:0] f(input logic [1:0] selector,
input logic [7:0] value);
casez (selector)
    2'b?0: f = value;
    default: f = 8'd0;
endcase
endfunction
assign y = f(sel, a);
endmodule
"#,
        None
    )
    .is_ok());
}

#[test]
fn function_wildcard_cases_preserve_dont_care_bits() {
    let d = compile_ok(
        r#"
module top(input [1:0] sel, [7:0] a, b, output [7:0] y);
function logic [7:0] decode(input logic [1:0] selector,
input logic [7:0] left, right);
logic [7:0] result;
casez (selector)
    2'b?0: result = left;
    2'b11: result = right;
    default: result = left & right;
endcase
decode = result;
endfunction
assign y = decode(sel, a, b);
endmodule
"#,
    );
    assert!(matches!(&d.assigns[0].e, Expr::Cond { .. }));

    let d = compile_ok(
        r#"
module top(input [1:0] sel, [7:0] a, b, output [7:0] y);
function logic [7:0] decode(input logic [1:0] selector,
input logic [7:0] left, right);
casex (selector)
    2'b0?: decode = left;
    2'b1?: decode = right;
    default: decode = left | right;
endcase
endfunction
assign y = decode(sel, a, b);
endmodule
"#,
    );
    assert!(matches!(&d.assigns[0].e, Expr::Cond { .. }));
}

#[test]
fn function_bounded_for_loops_unroll_during_inlining() {
    let d = compile_ok(
        r#"
module top(input [7:0] a, output [7:0] y);
function logic [7:0] bump(input logic [7:0] value);
logic [7:0] total = value;
for (int i = 0; i < 4; i = i + 1)
    total = total + 8'd1;
bump = total;
endfunction
assign y = bump(a);
endmodule
"#,
    );
    assert_eq!(d.assigns.len(), 1);
    assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));

    let d = compile_ok(
        r#"
module top(input [7:0] a, output [7:0] y);
function logic [7:0] grid(input logic [7:0] value);
logic [7:0] total = value;
for (int i = 0; i < 2; i = i + 1)
    for (int j = 0; j < 2; j = j + 1)
        total = total + 8'd1;
grid = total;
endfunction
assign y = grid(a);
endmodule
"#,
    );
    assert_eq!(d.assigns.len(), 1);

    assert!(compile(
        r#"
module bad(input [7:0] a, output [7:0] y);
function logic [7:0] stuck(input logic [7:0] value);
for (int i = 0; i < 4; i = i)
    value = value + 8'd1;
stuck = value;
endfunction
assign y = stuck(a);
endmodule
"#,
        None
    )
    .is_err());
}

#[test]
fn function_constant_repeat_loops_unroll_during_inlining() {
    let d = compile_ok(
        r#"
module top(input [7:0] a, output [7:0] y);
function logic [7:0] bump(input logic [7:0] value);
logic [7:0] total = value;
repeat (4)
    total = total + 8'd1;
bump = total;
endfunction
assign y = bump(a);
endmodule
"#,
    );
    assert_eq!(d.assigns.len(), 1);
    assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));

    let d = compile_ok(
        r#"
module top(input [7:0] a, output [7:0] y);
function logic [7:0] grid(input logic [7:0] value);
logic [7:0] total = value;
repeat (2)
    repeat (2)
        total = total + 8'd1;
grid = total;
endfunction
assign y = grid(a);
endmodule
"#,
    );
    assert_eq!(d.assigns.len(), 1);

    assert!(compile(
        r#"
module bad(input [7:0] a, output [7:0] y);
function logic [7:0] too_many(input logic [7:0] value);
repeat (257)
    value = value + 8'd1;
too_many = value;
endfunction
assign y = too_many(a);
endmodule
"#,
        None
    )
    .is_err());
}

#[test]
fn function_bounded_while_loops_unroll_during_inlining() {
    let d = compile_ok(
        r#"
module top(input [7:0] a, output [7:0] y);
function logic [7:0] bump(input logic [7:0] value);
logic [7:0] total = value;
int counter = 0;
while (counter < 4) begin
    total = total + 8'd1;
    counter = counter + 1;
end
bump = total;
endfunction
assign y = bump(a);
endmodule
"#,
    );
    assert_eq!(d.assigns.len(), 1);
    assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));

    let d = compile_ok(
        r#"
module top(input [7:0] a, output [7:0] y);
function logic [7:0] grid(input logic [7:0] value);
logic [7:0] total = value;
int outer = 0;
int inner = 0;
while (outer < 2) begin
    inner = 0;
    while (inner < 2) begin
        total = total + 8'd1;
        inner = inner + 1;
    end
    outer = outer + 1;
end
grid = total;
endfunction
assign y = grid(a);
endmodule
"#,
    );
    assert_eq!(d.assigns.len(), 1);

    assert!(compile(
        r#"
module bad(input [7:0] a, output [7:0] y);
function logic [7:0] runtime(input logic [7:0] value);
while (value != 8'd0)
    value = value - 8'd1;
runtime = value;
endfunction
assign y = runtime(a);
endmodule
"#,
        None
    )
    .is_err());
}

#[test]
fn function_packed_struct_locals_update_and_read_fields() {
    let d = compile_ok(
        r#"
module top(input [7:0] a, b, output [11:0] y);
typedef struct packed {
logic [7:0] high;
logic [3:0] low;
} packet_t;
function logic [11:0] combine(input logic [7:0] left, right);
packet_t packet;
packet.high = left + 8'd1;
packet.low = right;
combine = packet;
endfunction
assign y = combine(a, b);
endmodule
"#,
    );
    assert_eq!(d.assigns.len(), 1);
    assert!(matches!(&d.assigns[0].e, Expr::Concat { .. }));

    let d = compile_ok(
        r#"
module top(input [7:0] a, b, output [7:0] y);
typedef struct packed {
logic [7:0] high;
logic [3:0] low;
} packet_t;
function logic [7:0] sum(input logic [7:0] left, right);
packet_t packet;
packet.high = left;
packet.low = right;
sum = packet.high + packet.low;
endfunction
assign y = sum(a, b);
endmodule
"#,
    );
    assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));

    let d = compile_ok(
        r#"
module top(input [3:0] a, output [7:0] y);
typedef union packed {
logic [7:0] wide;
logic [3:0] narrow;
} view_t;
function logic [7:0] view_value(input logic [3:0] value);
view_t view;
view.narrow = value;
view_value = view.narrow;
endfunction
assign y = view_value(a);
endmodule
"#,
    );
    assert_eq!(d.assigns.len(), 1);

    assert!(compile(
        r#"
module bad(input [7:0] a, output [7:0] y);
function logic [7:0] f(input logic [7:0] value);
packet_t packet;
packet.high = value;
f = packet.high;
endfunction
assign y = f(a);
endmodule
"#,
        None
    )
    .is_err());
}

#[test]
fn function_unpacked_array_locals_update_and_read_elements() {
    let d = compile_ok(
        r#"
module top(input [7:0] a, b, output [7:0] y);
function logic [7:0] sum4(input logic [7:0] left, right);
logic [7:0] memory [0:3];
for (int i = 0; i < 4; i = i + 1)
    memory[i] = left + right + i;
sum4 = memory[0] + memory[1] + memory[2] + memory[3];
endfunction
assign y = sum4(a, b);
endmodule
"#,
    );
    assert_eq!(d.assigns.len(), 1);
    assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));

    assert!(compile(
        r#"
module bad(input [7:0] a, output [7:0] y);
function logic [7:0] f(input logic [7:0] value);
logic [7:0] memory [0:3];
memory[4] = value;
f = memory[4];
endfunction
assign y = f(a);
endmodule
"#,
        None
    )
    .is_err());
}

#[test]
fn function_multidimensional_unpacked_array_locals_select_elements() {
    let d = compile_ok(
        r#"
module top(input [7:0] a, b, output [7:0] y);
function logic [7:0] sum4(input logic [7:0] left, right);
logic [7:0] memory [0:1][1:0];
for (int i = 0; i < 2; i = i + 1)
    for (int j = 0; j < 2; j = j + 1)
        memory[i][j] = left + right + i * 2 + j;
sum4 = memory[0][0] + memory[0][1] + memory[1][0] + memory[1][1];
endfunction
assign y = sum4(a, b);
endmodule
"#,
    );
    assert_eq!(d.assigns.len(), 1);
    assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));

    assert!(compile(
        r#"
module bad(input [7:0] a, output [7:0] y);
function logic [7:0] f(input logic [7:0] value);
logic [7:0] memory [0:1][1:0];
memory[0] = value;
f = memory[0][1];
endfunction
assign y = f(a);
endmodule
"#,
        None
    )
    .is_err());
}

#[test]
fn interface_signals_connect_producer_and_consumer() {
    let d = compile_ok(
        r#"
interface bus;
logic [7:0] data;
logic valid;
modport master(output valid, data);
modport slave(input valid, data);
endinterface

module producer(interface bus.master b, input logic [7:0] value);
assign b.valid = 1'b1;
assign b.data = value;
endmodule

module consumer(interface bus.slave b, output logic [7:0] result);
assign result = b.valid ? b.data : 8'd0;
endmodule

module top(input logic [7:0] value, output logic [7:0] result);
bus shared();
producer p(.b(shared), .value(value));
consumer c(.b(shared), .result(result));
endmodule
"#,
    );
    assert_eq!(d.ports.len(), 2);
    assert!(d.assigns.len() >= 3);
    assert!(d.hierarchy.iter().any(|instance| instance.module == "bus"));

    assert!(compile(
        r#"
interface bus;
logic [7:0] data;
modport master(output data);
endinterface
module bad(interface bus.master b, output logic [7:0] y);
assign y = b.data;
endmodule
module top(output logic [7:0] y);
bus shared();
bad b(.b(shared), .y(y));
endmodule
"#,
        None
    )
    .is_err());

    assert!(compile(
        r#"
interface bus;
logic [7:0] data;
modport slave(input data);
endinterface
module bad(interface bus.slave b, input logic [7:0] value);
assign b.data = value;
endmodule
module top(input logic [7:0] value);
bus shared();
bad b(.b(shared), .value(value));
endmodule
"#,
        None
    )
    .is_err());
}

#[test]
fn immediate_assertions_lower_to_simulation_statements() {
    let d = compile_ok(
        r#"
module checks(input logic [7:0] a, b);
initial begin
#1;
assert (a < b) else $error("a must be less than b");
end
endmodule
"#,
    );
    assert_eq!(d.initials.len(), 1);
    assert!(matches!(
        &d.initials[0],
        StmtIR::Block(statements) if matches!(statements[1], StmtIR::Assert { .. })
    ));

    assert!(compile("module m(input a); assert property (a); endmodule", None).is_err());
}

#[test]
fn top_selection_resolves_unique_uninstantiated_tb() {
    let d = compile_ok(
        "module dut(input a, output y); assign y = a; endmodule
         module tb; reg a; wire y; dut u1(a, y); initial begin #1 a = 0; end endmodule",
    );
    assert_eq!(d.name, "tb");
}

#[test]
fn ambiguous_top_is_an_error() {
    let error = compile(
        "module first(output wire y); assign y = 1'b0; endmodule \
         module second(output wire y); assign y = 1'b1; endmodule",
        None,
    )
    .unwrap_err();
    assert!(error.contains("ambiguous top module"));
    assert!(error.contains("first"));
    assert!(error.contains("second"));
}

#[test]
fn explicit_top_overrides() {
    let d = compile(
        "module dut(input a, output y); assign y = a; endmodule
         module tb; reg a; wire y; dut u1(a, y); endmodule",
        Some("dut"),
    )
    .unwrap();
    assert_eq!(d.name, "dut");
    assert_eq!(d.ports.len(), 2);
}

#[test]
fn concat_and_replicate() {
    let d = compile_ok(
        "module m(input [3:0] a, b, output [7:0] y, [7:0] z);
            assign y = {a, b};
            assign z = {2{a}};
         endmodule",
    );
    assert_eq!(d.n_nets, 24); // 4+4+8+8
    assert!(matches!(d.assigns[0].e, Expr::Concat(_)));
    assert!(matches!(d.assigns[1].e, Expr::Repeat { times: 2, .. }));
}

#[test]
fn reg_ports_supported() {
    let d = compile_ok(
        "module cnt(input clk, output reg [3:0] q);
            always @(posedge clk) q <= q + 1;
         endmodule",
    );
    assert_eq!(d.ports.len(), 2);
    assert_eq!(d.processes.len(), 1);
}

#[test]
fn recursive_instance_detected() {
    let e = compile(
        "module a(input x, output y); a u1(x, y); assign y = x; endmodule",
        None,
    );
    assert!(e.is_err());
}

#[test]
fn unknown_instance_module_is_error() {
    assert!(compile(
        "module top(input x, output y); nosuch u1(x, y); endmodule",
        None
    )
    .is_err());
}

#[test]
fn malformed_instance_bindings_are_rejected() {
    let cases = [
        (
            "module child(input a, output y); assign y=a; endmodule \
             module top(input a, output y); child u(.a(a), .missing(a), .y(y)); endmodule",
            "port 'missing' does not exist in 'child'",
        ),
        (
            "module child(input a, output y); assign y=a; endmodule \
             module top(input a, output y); child u(.a(a), .a(a), .y(y)); endmodule",
            "duplicate connection for port 'a'",
        ),
        (
            "module child(input a, output y); assign y=a; endmodule \
             module top(input a, output y); child u(a, y, a); endmodule",
            "too many port connections",
        ),
        (
            "module child(input a, b, output y); assign y=a&b; endmodule \
             module top(input a, b, output y); child u(a, .b(b), .y(y)); endmodule",
            "mixing ordered and named port connections",
        ),
        (
            "module child #(parameter W=1, BAD=1) (input [W-1:0] a, output [W-1:0] y); assign y=a; endmodule \
             module top(input a, output y); child #(.W(1), .MISSING(2)) u(a, y); endmodule",
            "module 'child' has no parameter 'MISSING'",
        ),
        (
            "module child #(parameter W=1, BAD=1) (input [W-1:0] a, output [W-1:0] y); assign y=a; endmodule \
             module top(input a, output y); child #(.W(1), .W(2)) u(a, y); endmodule",
            "duplicate parameter connection 'W'",
        ),
        (
            "module child #(parameter W=1, BAD=1) (input [W-1:0] a, output [W-1:0] y); assign y=a; endmodule \
             module top(input a, output y); child #(1, 2, 3) u(a, y); endmodule",
            "too many parameter connections",
        ),
        (
            "module child #(parameter W=1, BAD=1) (input [W-1:0] a, output [W-1:0] y); assign y=a; endmodule \
             module top(input a, output y); child #(1, .BAD(2)) u(a, y); endmodule",
            "unexpected token",
        ),
    ];

    for (source, expected) in cases {
        let error = compile(source, None).unwrap_err();
        assert!(
            error.contains(expected),
            "expected {error:?} to contain {expected:?}"
        );
    }
}

#[test]
fn automatic_top_respects_modules_nested_in_generate() {
    let design = compile(
        "module child_a(output y); assign y=1'b1; endmodule \
         module child_b(output y); assign y=1'b0; endmodule \
         module top #(parameter ENABLE=1) (output y); \
           generate if (ENABLE) begin : selected child_a u(.y(y)); end \
           else begin : unselected child_b u(.y(y)); end endgenerate \
         endmodule",
        None,
    )
    .unwrap();
    assert_eq!(design.name, "top");
    assert_eq!(design.hierarchy.len(), 2);
}

#[test]
fn top_level_inout_is_rejected_explicitly() {
    let error = compile("module top(inout wire pad); endmodule", None).unwrap_err();
    assert_eq!(error, "top module 'top': inout port 'pad' is not supported");
}

#[test]
fn duplicate_module_declarations_are_rejected() {
    let cases = [
        (
            "module top(input a, input a); endmodule",
            "duplicate port 'a'",
        ),
        (
            "module top #(parameter W=1) (); parameter W=2; endmodule",
            "duplicate parameter 'W'",
        ),
        (
            "package pkg; parameter W=1; parameter W=2; endpackage \
             module top(); endmodule",
            "duplicate parameter 'W'",
        ),
    ];

    for (source, expected) in cases {
        let error = compile(source, None).unwrap_err();
        assert!(
            error.contains(expected),
            "expected {error:?} to contain {expected:?}"
        );
    }
}

#[test]
fn huge_sparse_memory_port_is_rejected_until_memory_ports_exist() {
    let error = compile(
        "module top(input [7:0] memory [0:1048575]); endmodule",
        None,
    )
    .unwrap_err();
    assert!(
        error.contains("sparse memory ports are not supported yet"),
        "unexpected error: {error}"
    );
}

#[test]
fn sparse_memory_port_shape_mismatch_is_rejected() {
    let error = compile(
        "module child(inout [7:0] memory [0:4095]); endmodule \
         module top; logic [7:0] memory [0:4094]; \
         child u(.memory(memory)); endmodule",
        None,
    )
    .unwrap_err();
    assert!(
        error.contains("memory port 'memory' shape does not match parent memory"),
        "unexpected error: {error}"
    );
}

#[test]
fn monitor_flag_propagates() {
    let d = compile_ok("module tb; reg a; initial $monitor(\"a=%b\", a); endmodule");
    match &d.initials[0] {
        Stmt::Display { monitor, .. } => assert!(*monitor),
        other => panic!("expected display, got {:?}", other),
    }
}

#[test]
fn net_init_becomes_assign() {
    let d = compile_ok("module m(output y); wire w = 1'b1; assign y = w; endmodule");
    assert_eq!(d.assigns.len(), 2);
}

#[test]
fn logical_ops_lowered_via_reduction() {
    let d = compile_ok(
        "module m(input [1:0] a, b, output y); assign y = (a == 2'd3) && (b != 0); endmodule",
    );
    // `&&` lowers to And of OrR reductions
    match &d.assigns[0].e {
        Expr::Bin { op: BinOp::And, .. } => {}
        other => panic!("expected and, got {:?}", other),
    }
}

#[test]
fn delayed_runtime_continuous_lvalue_is_rejected() {
    let error = compile(
        "module m(input [1:0] selector, input [3:0] a, output reg [15:0] y); \
         assign #1 y[selector +: 4] = a; \
         endmodule",
        None,
    )
    .unwrap_err();
    assert!(error.contains("delayed runtime indexed continuous-assignment lvalue is not supported"));
}

#[test]
fn runtime_whole_unpacked_array_elements_compile_as_reads_and_writes() {
    let d = compile_ok(
        "module m(input clk, input [1:0] selector, input [7:0] din, \
                  output reg [7:0] q); \
         logic [7:0] memory [0:3]; \
         initial begin \
           memory[0] = 8'd10; memory[1] = 8'd20; \
           memory[2] = 8'd30; memory[3] = 8'd40; \
         end \
         always @(posedge clk) begin \
           q = memory[selector]; \
           memory[selector] = din; \
         end \
         endmodule",
    );
    assert_eq!(d.initials.len(), 1);
    assert_eq!(d.processes.len(), 1);

    let continuous = compile_ok(
        "module m(input [1:0] selector, input [7:0] din, output [7:0] q); \
         logic [7:0] memory [0:3]; \
         assign memory[selector] = din; \
         assign q = memory[selector]; \
         endmodule",
    );
    assert_eq!(continuous.processes.len(), 1);
}

#[test]
fn sequence_composition_with_repetition_is_rejected() {
    let error = compile(
        "module m(input clk, req, first, second); \
         assert property (@(posedge clk) req |-> first and second[*2]); \
         endmodule",
        None,
    )
    .unwrap_err();
    assert!(error.contains("sequence composition does not yet compose with repetition"));
}
