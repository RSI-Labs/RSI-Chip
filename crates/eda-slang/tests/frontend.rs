use eda_core::logic::{LogicValue, LogicWord};
use eda_sim::Simulator;
use eda_slang::{PreprocessOptions, SlangFrontend};
use std::path::PathBuf;

fn temporary_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "eda-slang-test-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).expect("create temporary test directory");
    path
}

#[test]
fn slang_preprocesses_macros_and_includes_before_rtl_lowering() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("macros");
    let include_directory = root.join("include");
    std::fs::create_dir_all(&include_directory).expect("create include directory");
    std::fs::write(include_directory.join("width.svh"), "`define BUS_WIDTH 8\n")
        .expect("write include");
    let source = r#"
`include "width.svh"
`ifndef BUS_WIDTH
`define BUS_WIDTH 4
`endif
module top(input [`BUS_WIDTH-1:0] a, output [`BUS_WIDTH-1:0] y);
  assign y = ~a;
endmodule
"#;
    let source_path = root.join("top.sv");
    std::fs::write(&source_path, source).expect("write source");

    let options = PreprocessOptions {
        include_directories: vec![include_directory],
        ..Default::default()
    };
    let design = slang
        .compile_files(&[source_path], Some("top"), &options)
        .expect("slang frontend compiles preprocessed RTL");
    assert_eq!(design.name, "top");
    assert_eq!(design.n_nets, 16);

    let mut simulator = Simulator::new(&design).expect("create simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(8, 0xa5))
        .expect("set input");
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0x5a));
}

#[test]
fn slang_ast_direct_lowering_compiles_and_simulates_simple_modules() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast");
    let source_path = root.join("simple.sv");
    std::fs::write(
        &source_path,
        r#"
module simple #(parameter SHIFT = 1)(
  input logic [3:0] a,
  input logic [3:0] b,
  output logic [3:0] sum,
  output logic [3:0] mask,
  output logic [3:0] shifted,
  output logic [3:0] inverted,
  output logic selected,
  output logic [7:0] concatenated,
  output logic [2:0] sliced,
  output logic [7:0] literal
);
  logic [3:0] initialized = 4'h3;
  assign sum = a + b;
  assign mask = a & b;
  assign shifted = a << SHIFT;
  assign inverted = ~a;
  assign selected = a[0] ? b[0] : a[1];
  assign concatenated = {a, b};
  assign sliced = a[3:1];
  assign literal = 8'hA5;
endmodule
"#,
    )
    .expect("write direct-lowering source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("simple"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower slang AST");
    let preprocessed_reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("simple"),
            &PreprocessOptions::default(),
        )
        .expect("lower the same module through the preprocessed path");
    assert_eq!(design.name, "simple");
    assert_eq!(design.n_nets, 48);
    assert_eq!(design.assigns.len(), 9);
    assert_eq!(preprocessed_reference.name, design.name);
    assert_eq!(preprocessed_reference.n_nets, design.n_nets);
    assert_eq!(preprocessed_reference.assigns.len(), design.assigns.len());
    assert_eq!(
        format!("{:?}", preprocessed_reference.ports),
        format!("{:?}", design.ports)
    );
    assert_eq!(
        format!("{:?}", preprocessed_reference.net_decls),
        format!("{:?}", design.net_decls)
    );

    let mut simulator = Simulator::new(&design).expect("create direct-AST simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(4, 0b1101))
        .expect("set a");
    simulator
        .set_input("b", &LogicWord::from_u64(4, 0b0101))
        .expect("set b");
    let output = |name: &str| {
        simulator
            .read(name)
            .unwrap_or_else(|| panic!("missing output {name}"))
            .to_u64()
            .unwrap_or_else(|| panic!("{name} was unknown"))
    };
    assert_eq!(output("sum"), 0b0010);
    assert_eq!(output("mask"), 0b0101);
    assert_eq!(output("shifted"), 0b1010);
    assert_eq!(output("inverted"), 0b0010);
    assert_eq!(output("selected"), 1);
    assert_eq!(output("concatenated"), 0b11010101);
    assert_eq!(output("sliced"), 0b110);
    assert_eq!(output("literal"), 0xa5);
    assert_eq!(output("initialized"), 3);

    let unsupported_path = root.join("unsupported.sv");
    std::fs::write(
        &unsupported_path,
        "module unsupported(output logic y); initial $display(\"unsupported\"); endmodule\n",
    )
    .expect("write unsupported source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&unsupported_path),
            Some("unsupported"),
            &PreprocessOptions::default(),
        )
        .expect_err("direct path must reject unsupported constructs");
    assert!(
        error.contains("does not support system call"),
        "unexpected error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_wildcard_package_imports() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-package-import");
    let source_path = root.join("package_import.sv");
    std::fs::write(
        &source_path,
        r#"
package math_pkg;
  parameter int WIDTH = 8;
  localparam int OFFSET = 42;

  function automatic int double(input int value);
    double = 2 * value;
  endfunction
endpackage

module package_import
  import math_pkg::*;
#(
  parameter int EXTRA = 1
)(
  input logic [WIDTH-1:0] a,
  output logic [WIDTH-1:0] y,
  output int doubled_offset
);
  assign y = a + OFFSET;
  assign doubled_offset = double(OFFSET) + EXTRA;
endmodule
"#,
    )
    .expect("write package-import source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("package_import"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower wildcard package import");
    assert_eq!(design.assigns.len(), 2);
    let mut simulator = Simulator::new(&design).expect("create package-import simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(8, 3))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(45));
    assert_eq!(simulator.read("doubled_offset").unwrap().to_u64(), Some(85));

    let explicit_path = root.join("explicit_import.sv");
    std::fs::write(
        &explicit_path,
        r#"
package scoped_pkg;
  parameter int WIDTH = 8;
  localparam int OFFSET = 21;

  function automatic int double(input int value);
    double = 2 * value;
  endfunction
endpackage

module explicit_import
  import scoped_pkg::WIDTH,
          scoped_pkg::OFFSET,
          scoped_pkg::double;
(
  input logic [WIDTH-1:0] a,
  output logic [WIDTH-1:0] imported,
  output logic [WIDTH-1:0] scoped,
  output int imported_function,
  output int scoped_function
);
  assign imported = a + OFFSET;
  assign scoped = a + scoped_pkg::OFFSET;
  assign imported_function = double(OFFSET);
  assign scoped_function = scoped_pkg::double(OFFSET);
endmodule
"#,
    )
    .expect("write explicit-import source");
    let explicit = slang
        .compile_files_via_ast(
            std::slice::from_ref(&explicit_path),
            Some("explicit_import"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower explicit package imports");
    assert_eq!(explicit.assigns.len(), 4);
    let mut explicit_simulator =
        Simulator::new(&explicit).expect("create explicit-import simulator");
    explicit_simulator
        .set_input("a", &LogicWord::from_u64(8, 3))
        .unwrap();
    assert_eq!(
        explicit_simulator.read("imported").unwrap().to_u64(),
        Some(24)
    );
    assert_eq!(
        explicit_simulator.read("scoped").unwrap().to_u64(),
        Some(24)
    );
    assert_eq!(
        explicit_simulator
            .read("imported_function")
            .unwrap()
            .to_u64(),
        Some(42)
    );
    assert_eq!(
        explicit_simulator.read("scoped_function").unwrap().to_u64(),
        Some(42)
    );

    let collision_path = root.join("package_collision.sv");
    std::fs::write(
        &collision_path,
        r#"
package left_pkg;
  parameter int SAME = 1;
endpackage

package right_pkg;
  parameter int SAME = 2;
endpackage

module package_collision
  import left_pkg::*,
          right_pkg::*;
(
  output int y
);
  assign y = 1;
endmodule
"#,
    )
    .expect("write package-collision source");
    let collision_error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&collision_path),
            Some("package_collision"),
            &PreprocessOptions::default(),
        )
        .expect_err("ambiguous imported package names must fail closed");
    assert!(
        collision_error.contains("imported package symbol collision for 'SAME'"),
        "unexpected collision error: {collision_error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_package_typedefs_and_enums() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-package-types");
    let source_path = root.join("package_types.sv");
    std::fs::write(
        &source_path,
        r#"
package types_pkg;
  typedef enum logic [1:0] {
    A = 2'd1,
    B = 2'd2
  } state_t;

  typedef struct packed {
    logic [3:0] high;
    logic [3:0] low;
  } pair_t;

  localparam state_t INIT = A;
endpackage

module package_types
  import types_pkg::*;
(
  input logic [1:0] selector,
  input logic [7:0] data,
  output state_t state,
  output pair_t pair,
  output state_t next_state,
  output state_t previous_state,
  output state_t first_state,
  output state_t last_state,
  output int enum_count,
  output string state_name,
  output logic initialized,
  output logic selected_a
);
  assign state = selector[0] ? B : A;
  assign pair = data;
  assign next_state = state.next();
  assign previous_state = state.prev();
  assign first_state = state.first();
  assign last_state = state.last();
  assign enum_count = state.num();
  assign state_name = state.name();
  assign initialized = (INIT == A);
  assign selected_a = (state == A);
endmodule
"#,
    )
    .expect("write package-typedef source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("package_types"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower package typedefs and enums");
    assert_eq!(design.assigns.len(), 10);
    let mut simulator = Simulator::new(&design).expect("create package-type simulator");
    simulator
        .set_input("selector", &LogicWord::from_u64(2, 0))
        .unwrap();
    simulator
        .set_input("data", &LogicWord::from_u64(8, 0x5A))
        .unwrap();
    assert_eq!(simulator.read("state").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("pair").unwrap().to_u64(), Some(0x5A));
    assert_eq!(simulator.read("next_state").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("previous_state").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("first_state").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("last_state").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("enum_count").unwrap().to_u64(), Some(2));
    assert_eq!(
        simulator
            .read("state_name")
            .unwrap()
            .to_ascii_string_nul_terminated(),
        "A"
    );
    assert_eq!(simulator.read("initialized").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("selected_a").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_compiles_procedural_control_flow() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-procedural");
    let source_path = root.join("procedural.sv");
    std::fs::write(
        &source_path,
        r#"
module procedural(
  input logic a,
  input logic clk,
  input logic [1:0] sel,
  output logic y,
  output logic z,
  output logic q
);
  initial q = 1'b0;
  always_comb begin
    if (a) begin
      y = 1'b1;
    end else begin
      y = 1'b0;
    end
    case (sel)
      2'b00: z = 1'b0;
      2'b01: z = 1'b1;
      default: z = a;
    endcase
  end
  always @(posedge clk) q <= a;
endmodule
"#,
    )
    .expect("write procedural direct-lowering source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("procedural"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower procedural slang AST");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("procedural"),
            &PreprocessOptions::default(),
        )
        .expect("lower procedural module through the preprocessed path");
    assert_eq!(design.n_nets, 7);
    assert_eq!(design.initials.len(), 1);
    assert_eq!(design.processes.len(), 2);
    assert_eq!(
        format!("{:?}", reference.initials),
        format!("{:?}", design.initials)
    );
    assert_eq!(
        format!("{:?}", reference.processes),
        format!("{:?}", design.processes)
    );

    let mut simulator = Simulator::new(&design).expect("create procedural simulator");
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize clk low");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .expect("set a");
    simulator
        .set_input("sel", &LogicWord::from_u64(2, 2))
        .expect("set default case selector");
    assert_eq!(simulator.read("q").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("z").unwrap().to_u64(), Some(1));
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create posedge");
    assert_eq!(simulator.read("q").unwrap().to_u64(), Some(1));

    let mixed_event_path = root.join("mixed-event.sv");
    std::fs::write(
        &mixed_event_path,
        r#"
module mixed_event(input logic a, input logic clk, output logic y);
  always @(posedge clk or a) y = a;
endmodule
"#,
    )
    .expect("write mixed-event source");
    let mixed_event = slang
        .compile_files_via_ast(
            std::slice::from_ref(&mixed_event_path),
            Some("mixed_event"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower mixed edge/level sensitivity");
    assert!(
        matches!(
            &mixed_event.processes[0].sens,
            eda_core::ir::Sens::List(list)
                if matches!(list.as_slice(), [eda_core::ir::Sens::Posedge(_), eda_core::ir::Sens::Any(_)])
        ),
        "unexpected sensitivity: {:?}",
        mixed_event.processes[0].sens
    );
}

#[test]
fn slang_ast_direct_lowering_flattens_simple_hierarchy() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-hierarchy");
    let source_path = root.join("hierarchy.sv");
    std::fs::write(
        &source_path,
        r#"
module child #(parameter W = 4)
    (input logic [W-1:0] a, output logic [W-1:0] y, output logic held);
  assign y = ~a;
  initial held = 1'b1;
endmodule

module top(input logic [3:0] a, output logic [3:0] y, output logic held);
  child #(.W(4)) u (.a(a), .y(y), .held(held));
endmodule
"#,
    )
    .expect("write hierarchy source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower simple hierarchy");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("lower hierarchy through the preprocessed path");
    assert_eq!(
        design.n_nets, 18,
        "direct declarations: {:?}; reference declarations: {:?}",
        design.net_decls, reference.net_decls
    );
    assert_eq!(design.assigns.len(), 4);
    assert_eq!(design.initials.len(), 1);
    assert_eq!(design.hierarchy.len(), 2);
    assert_eq!(
        format!("{:?}", reference.net_decls),
        format!("{:?}", design.net_decls)
    );
    assert_eq!(
        format!("{:?}", reference.assigns),
        format!("{:?}", design.assigns)
    );
    assert_eq!(
        format!("{:?}", reference.initials),
        format!("{:?}", design.initials)
    );
    assert_eq!(
        format!("{:?}", reference.hierarchy),
        format!("{:?}", design.hierarchy)
    );

    let mut simulator = Simulator::new(&design).expect("create hierarchy simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(4, 0b1010))
        .expect("set hierarchy input");
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0b0101));
    assert_eq!(simulator.read("held").unwrap().to_u64(), Some(1));

    let nested_path = root.join("nested.sv");
    std::fs::write(
        &nested_path,
        r#"
module leaf(input logic a, output logic y); assign y = a; endmodule
module middle(input logic a, output logic y); leaf u (.a(a), .y(y)); endmodule
module top(input logic a, output logic y); middle u (.a(a), .y(y)); endmodule
"#,
    )
    .expect("write nested hierarchy source");
    let nested_design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&nested_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower nested hierarchy");
    let nested_reference = slang
        .compile_files(
            std::slice::from_ref(&nested_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("lower nested hierarchy through the preprocessed path");
    assert_eq!(nested_design.n_nets, 6);
    assert_eq!(nested_design.assigns.len(), 5);
    assert_eq!(nested_design.hierarchy.len(), 3);
    assert!(
        nested_design
            .hierarchy
            .iter()
            .any(|instance| instance.path == "top.u.u"),
        "unexpected hierarchy: {:?}",
        nested_design.hierarchy
    );
    assert_eq!(
        format!("{:?}", nested_reference.net_decls),
        format!("{:?}", nested_design.net_decls)
    );
    assert_eq!(
        format!("{:?}", nested_reference.assigns),
        format!("{:?}", nested_design.assigns)
    );
    assert_eq!(
        format!("{:?}", nested_reference.hierarchy),
        format!("{:?}", nested_design.hierarchy)
    );

    let mut nested_simulator = Simulator::new(&nested_design).expect("create nested simulator");
    nested_simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .expect("set nested input");
    assert_eq!(nested_simulator.read("y").unwrap().to_u64(), Some(1));
    nested_simulator
        .set_input("a", &LogicWord::from_u64(1, 0))
        .expect("update nested input");
    assert_eq!(nested_simulator.read("y").unwrap().to_u64(), Some(0));
}

#[test]
fn slang_ast_direct_lowering_uses_sparse_memory_operations() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-memory");
    let source_path = root.join("memory.sv");
    std::fs::write(
        &source_path,
        r#"
module memory(
  input logic clk,
  input logic write,
  input logic [1:0] addr,
  input logic [7:0] wdata,
  output logic [7:0] rdata
);
  logic [7:0] mem [0:3];
  initial begin
    mem[0] = 8'h11;
    mem[1] = 8'h22;
  end
  always_comb rdata = mem[addr];
  always @(posedge clk) begin
    if (write) mem[addr] <= wdata;
  end
endmodule
"#,
    )
    .expect("write sparse-memory source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("memory"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower sparse memory");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("memory"),
            &PreprocessOptions::default(),
        )
        .expect("lower sparse memory through the preprocessed path");
    assert_eq!(design.memories.len(), 1);
    assert_eq!(design.memories[0].name, "mem");
    assert_eq!(design.memories[0].element_width, 8);
    assert_eq!(design.memories[0].flat_length, 4);
    assert_eq!(design.memories[0].kind, eda_core::ir::MemoryKind::Ram);
    assert_eq!(
        design.memories[0].template,
        eda_core::ir::MemoryTemplate::SyncRam
    );
    assert!(design.memories[0]
        .write_mask
        .as_ref()
        .expect("runtime memory write mask")
        .is_full());
    assert_eq!(
        design.memories[0].byte_enable,
        Some(eda_core::ir::MemoryByteEnable::full(8))
    );
    assert!(reference.memories.is_empty());
    assert_eq!(
        format!("{:?}", reference.ports),
        format!("{:?}", design.ports)
    );

    let mut simulator = Simulator::new(&design).expect("create sparse-memory simulator");
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize clk");
    simulator
        .set_input("write", &LogicWord::from_u64(1, 0))
        .expect("initialize write");
    simulator
        .set_input("addr", &LogicWord::from_u64(2, 0))
        .expect("select initialized element");
    assert_eq!(simulator.read("rdata").unwrap().to_u64(), Some(0x11));

    simulator
        .set_input("addr", &LogicWord::from_u64(2, 2))
        .expect("select write element");
    simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0xaa))
        .expect("set write data");
    simulator
        .set_input("write", &LogicWord::from_u64(1, 1))
        .expect("request write");
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create sparse-memory posedge");
    assert_eq!(simulator.read("rdata").unwrap().to_u64(), Some(0xaa));

    simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("reset clk");
    simulator
        .set_input("addr", &LogicWord::from_u64(2, 1))
        .expect("select another initialized element");
    assert_eq!(simulator.read("rdata").unwrap().to_u64(), Some(0x22));

    simulator
        .set_input("addr", &LogicWord::from_u64(2, 2))
        .expect("select overwritten element");
    assert_eq!(simulator.read("rdata").unwrap().to_u64(), Some(0xaa));

    simulator
        .set_input("addr", &LogicWord::from_u64(2, 3))
        .expect("select out-of-range element");
    assert_eq!(simulator.read("rdata").unwrap().to_u64(), None);

    let mut reference_simulator =
        Simulator::new(&reference).expect("create legacy-array simulator");
    reference_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize legacy clk");
    reference_simulator
        .set_input("write", &LogicWord::from_u64(1, 0))
        .expect("initialize legacy write");
    reference_simulator
        .set_input("addr", &LogicWord::from_u64(2, 0))
        .expect("select initialized legacy element");
    assert_eq!(
        reference_simulator.read("rdata").unwrap().to_u64(),
        Some(0x11)
    );
    reference_simulator
        .set_input("addr", &LogicWord::from_u64(2, 2))
        .expect("select legacy write element");
    reference_simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0xaa))
        .expect("set legacy write data");
    reference_simulator
        .set_input("write", &LogicWord::from_u64(1, 1))
        .expect("request legacy write");
    reference_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create legacy posedge");
    assert_eq!(
        reference_simulator.read("rdata").unwrap().to_u64(),
        Some(0xaa)
    );
}

#[test]
fn slang_ast_direct_lowering_uses_sparse_memory_part_selects() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-memory-parts");
    let source_path = root.join("memory-parts.sv");
    std::fs::write(
        &source_path,
        r#"
module memory_parts(
  input logic clk,
  input logic [2:0] start,
  input logic [7:0] data,
  input logic [1:0] raddr,
  output logic [3:0] simple,
  output logic [3:0] up,
  output logic [3:0] down,
  output logic [3:0] updated
);
  logic [7:0] mem [0:3];
  initial begin
    mem[0] = 8'h9c;
    mem[1] = 8'h22;
  end
  assign simple = mem[0][0 +: 4];
  assign up = mem[0][2 +: 4];
  assign down = mem[0][6 -: 4];
  assign updated = mem[raddr][start +: 4];
  always @(posedge clk) begin
    mem[1][start +: 4] <= data[3:0];
  end
endmodule
"#,
    )
    .expect("write sparse-memory part-select source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("memory_parts"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower sparse-memory part selects");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("memory_parts"),
            &PreprocessOptions::default(),
        )
        .expect("lower sparse-memory part selects through the preprocessed path");
    assert_eq!(design.memories.len(), 1);
    assert_eq!(design.assigns.len(), 4);
    assert_eq!(design.initials.len(), 1);
    assert_eq!(design.processes.len(), 1);
    assert!(design.memories[0]
        .write_mask
        .as_ref()
        .expect("dynamic part writes require a conservative mask")
        .is_full());

    let mut simulator = Simulator::new(&design).expect("create part-select simulator");
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize clk");
    simulator
        .set_input("start", &LogicWord::from_u64(3, 2))
        .expect("set part-select start");
    simulator
        .set_input("raddr", &LogicWord::from_u64(2, 1))
        .expect("set read element");
    assert_eq!(simulator.read("simple").unwrap().to_u64(), Some(0b1100));
    assert_eq!(simulator.read("up").unwrap().to_u64(), Some(0b0111));
    assert_eq!(simulator.read("down").unwrap().to_u64(), Some(0b0011));
    assert_eq!(simulator.read("updated").unwrap().to_u64(), Some(0b1000));

    simulator
        .set_input("data", &LogicWord::from_u64(8, 0b1011))
        .expect("set part-write data");
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create part-write posedge");
    assert_eq!(simulator.read("updated").unwrap().to_u64(), Some(0b1011));

    let mut reference_simulator =
        Simulator::new(&reference).expect("create legacy part-select simulator");
    reference_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize legacy clk");
    reference_simulator
        .set_input("start", &LogicWord::from_u64(3, 2))
        .expect("set legacy part-select start");
    reference_simulator
        .set_input("raddr", &LogicWord::from_u64(2, 1))
        .expect("set legacy read element");
    assert_eq!(
        reference_simulator.read("simple").unwrap().to_u64(),
        Some(0b1100)
    );
    assert_eq!(
        reference_simulator.read("up").unwrap().to_u64(),
        Some(0b0111)
    );
    assert_eq!(
        reference_simulator.read("down").unwrap().to_u64(),
        Some(0b0011)
    );
    assert_eq!(
        reference_simulator.read("updated").unwrap().to_u64(),
        Some(0b1000)
    );
    reference_simulator
        .set_input("data", &LogicWord::from_u64(8, 0b1011))
        .expect("set legacy part-write data");
    reference_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create legacy part-write posedge");
    assert_eq!(
        reference_simulator.read("updated").unwrap().to_u64(),
        Some(0b1011)
    );

    let simple_path = root.join("simple-part.sv");
    std::fs::write(
        &simple_path,
        r#"
module simple_part(output logic [3:0] y);
  logic [7:0] mem [0:0];
  initial begin
    mem[0] = 8'h9c;
  end
  assign y = mem[0][3:0];
endmodule
"#,
    )
    .expect("write simple memory part-select source");
    let simple_design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&simple_path),
            Some("simple_part"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower simple memory part select");
    let mut simple_simulator =
        Simulator::new(&simple_design).expect("create simple part-select simulator");
    simple_simulator
        .run_until(0)
        .expect("settle simple part-select outputs");
    assert_eq!(simple_simulator.read("y").unwrap().to_u64(), Some(0b1100));
}

#[test]
fn slang_and_bounded_paths_lower_array_element_simple_ranges() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("array-simple-range");
    let source_path = root.join("simple-range.sv");
    std::fs::write(
        &source_path,
        r#"
module simple_range(input logic [1:0] addr, output logic [3:0] y);
  logic [7:0] mem [0:0];
  initial begin
    mem[0] = 8'h9c;
    mem[0][2:1] = 2'b01;
  end
  assign y = mem[addr][3:0];
endmodule
"#,
    )
    .expect("write array-element simple-range source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("simple_range"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower array-element simple range");
    let bounded = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("simple_range"),
            &PreprocessOptions::default(),
        )
        .expect("lower array-element simple range through bounded path");
    assert_eq!(direct.memories.len(), 1);

    let mut direct_simulator = Simulator::new(&direct).expect("create direct simulator");
    direct_simulator
        .set_input("addr", &LogicWord::from_u64(2, 0))
        .expect("set direct address");
    assert_eq!(direct_simulator.read("y").unwrap().to_u64(), Some(0b1010));

    let mut bounded_simulator = Simulator::new(&bounded).expect("create bounded simulator");
    bounded_simulator
        .set_input("addr", &LogicWord::from_u64(2, 0))
        .expect("set bounded address");
    assert_eq!(bounded_simulator.read("y").unwrap().to_u64(), Some(0b1010));
}

#[test]
fn slang_ast_direct_lowering_uses_multidimensional_sparse_memory() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-memory2d");
    let source_path = root.join("memory2d.sv");
    std::fs::write(
        &source_path,
        r#"
module memory2d(
  input logic clk,
  input logic [1:0] x,
  input logic [2:0] y,
  input logic [7:0] wdata,
  output logic [7:0] q,
  output logic [3:0] p
);
  logic [7:0] mem [0:1][2:3];
  initial begin
    mem[0][2] = 8'h12;
    mem[1][3] = 8'h34;
  end
  always_comb q = mem[x][y];
  assign p = mem[x][y][3:0];
  always @(posedge clk) mem[x][y] <= wdata;
endmodule
"#,
    )
    .expect("write multidimensional sparse-memory source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("memory2d"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional sparse memory");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("memory2d"),
            &PreprocessOptions::default(),
        )
        .expect("lower multidimensional memory through the bounded path");
    assert_eq!(direct.memories.len(), 1);
    assert_eq!(direct.memories[0].dimensions.len(), 2);
    assert_eq!(direct.memories[0].flat_length, 4);
    assert_eq!(
        direct.memories[0].banking,
        eda_core::ir::MemoryBanking { count: 2, depth: 2 }
    );
    assert_eq!(direct.n_nets, 27);

    let mut direct_simulator = Simulator::new(&direct).expect("create direct 2D simulator");
    direct_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize direct clock");
    direct_simulator
        .set_input("x", &LogicWord::from_u64(2, 0))
        .expect("set first direct index");
    direct_simulator
        .set_input("y", &LogicWord::from_u64(3, 2))
        .expect("set second direct index");
    assert_eq!(direct_simulator.read("q").unwrap().to_u64(), Some(0x12));
    assert_eq!(direct_simulator.read("p").unwrap().to_u64(), Some(0x2));
    direct_simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0x5a))
        .expect("set direct 2D write data");
    direct_simulator
        .set_input("x", &LogicWord::from_u64(2, 1))
        .expect("update first direct index");
    direct_simulator
        .set_input("y", &LogicWord::from_u64(3, 3))
        .expect("update second direct index");
    assert_eq!(direct_simulator.read("q").unwrap().to_u64(), Some(0x34));
    assert_eq!(direct_simulator.read("p").unwrap().to_u64(), Some(0x4));
    direct_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create direct 2D posedge");
    assert_eq!(direct_simulator.read("q").unwrap().to_u64(), Some(0x5a));
    assert_eq!(direct_simulator.read("p").unwrap().to_u64(), Some(0xa));

    let mut reference_simulator = Simulator::new(&reference).expect("create bounded 2D simulator");
    reference_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize bounded clock");
    reference_simulator
        .set_input("x", &LogicWord::from_u64(2, 0))
        .expect("set first bounded index");
    reference_simulator
        .set_input("y", &LogicWord::from_u64(3, 2))
        .expect("set second bounded index");
    assert_eq!(reference_simulator.read("q").unwrap().to_u64(), Some(0x12));
    assert_eq!(reference_simulator.read("p").unwrap().to_u64(), Some(0x2));
    reference_simulator
        .set_input("x", &LogicWord::from_u64(2, 1))
        .expect("update first bounded index");
    reference_simulator
        .set_input("y", &LogicWord::from_u64(3, 3))
        .expect("update second bounded index");
    assert_eq!(reference_simulator.read("q").unwrap().to_u64(), Some(0x34));
    assert_eq!(reference_simulator.read("p").unwrap().to_u64(), Some(0x4));
    reference_simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0x5a))
        .expect("set bounded 2D write data");
    reference_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create bounded 2D posedge");
    assert_eq!(reference_simulator.read("q").unwrap().to_u64(), Some(0x5a));
    assert_eq!(reference_simulator.read("p").unwrap().to_u64(), Some(0xa));
}

#[test]
fn slang_ast_direct_lowering_uses_multidimensional_indexed_part_writes() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-memory2d-parts");
    let source_path = root.join("memory2d-parts.sv");
    std::fs::write(
        &source_path,
        r#"
module memory2d_parts(
  input logic clk,
  input logic [1:0] x,
  input logic [2:0] y,
  input logic [2:0] start,
  input logic [7:0] wdata,
  output logic [7:0] q,
  output logic [3:0] p
);
  logic [7:0] mem [0:1][2:3];
  initial begin
    mem[1][3] = 8'h34;
  end
  assign q = mem[x][y];
  assign p = mem[x][y][start +: 4];
  always @(posedge clk) mem[x][y][start +: 4] <= wdata[3:0];
endmodule
"#,
    )
    .expect("write multidimensional indexed part-write source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("memory2d_parts"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional indexed part write");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("memory2d_parts"),
            &PreprocessOptions::default(),
        )
        .expect("lower multidimensional indexed part write through bounded path");
    assert_eq!(direct.memories.len(), 1);
    assert_eq!(direct.memories[0].dimensions.len(), 2);

    let mut direct_simulator = Simulator::new(&direct).expect("create direct part-write simulator");
    direct_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize direct clock");
    direct_simulator
        .set_input("x", &LogicWord::from_u64(2, 1))
        .expect("set direct first index");
    direct_simulator
        .set_input("y", &LogicWord::from_u64(3, 3))
        .expect("set direct second index");
    direct_simulator
        .set_input("start", &LogicWord::from_u64(3, 4))
        .expect("set direct part start");
    assert_eq!(direct_simulator.read("q").unwrap().to_u64(), Some(0x34));
    assert_eq!(direct_simulator.read("p").unwrap().to_u64(), Some(0x3));
    direct_simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0x0a))
        .expect("set direct part data");
    direct_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create direct posedge");
    assert_eq!(direct_simulator.read("q").unwrap().to_u64(), Some(0xa4));
    assert_eq!(direct_simulator.read("p").unwrap().to_u64(), Some(0xa));

    let mut reference_simulator =
        Simulator::new(&reference).expect("create bounded part-write simulator");
    reference_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize bounded clock");
    reference_simulator
        .set_input("x", &LogicWord::from_u64(2, 1))
        .expect("set bounded first index");
    reference_simulator
        .set_input("y", &LogicWord::from_u64(3, 3))
        .expect("set bounded second index");
    reference_simulator
        .set_input("start", &LogicWord::from_u64(3, 4))
        .expect("set bounded part start");
    assert_eq!(reference_simulator.read("q").unwrap().to_u64(), Some(0x34));
    assert_eq!(reference_simulator.read("p").unwrap().to_u64(), Some(0x3));
    reference_simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0x0a))
        .expect("set bounded part data");
    reference_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create bounded posedge");
    assert_eq!(reference_simulator.read("q").unwrap().to_u64(), Some(0xa4));
    assert_eq!(reference_simulator.read("p").unwrap().to_u64(), Some(0xa));
}

#[test]
fn slang_ast_direct_lowering_shares_child_sparse_memory_ports() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-memory-port");
    let source_path = root.join("memory-port.sv");
    std::fs::write(
        &source_path,
        r#"
module child(
  inout wire logic [7:0] memory [0:4095],
  output logic [7:0] q
);
  assign q = memory[2];
endmodule

module top(
  input logic [7:0] din,
  output logic [7:0] child_q,
  output logic [7:0] parent_q
);
  wire logic [7:0] memory [0:4095];
  child u (
    .memory(memory),
    .q(child_q)
  );
  assign memory[2] = din;
  assign parent_q = memory[2];
endmodule
"#,
    )
    .expect("write shared sparse-memory port source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower shared sparse-memory port");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("lower shared sparse-memory port through bounded path");
    assert_eq!(direct.memories.len(), 2);
    assert_eq!(direct.memory_connections.len(), 1);
    assert_eq!(direct.memory_connections[0].child, 1);
    assert_eq!(direct.memory_connections[0].parent, 0);
    assert_eq!(reference.memories.len(), 2);
    assert_eq!(reference.memory_connections.len(), 1);
    assert_eq!(direct.memories[0].kind, eda_core::ir::MemoryKind::Ram);
    assert_eq!(direct.memories[1].kind, eda_core::ir::MemoryKind::Ram);
    assert_eq!(reference.memories[0].kind, eda_core::ir::MemoryKind::Ram);
    assert_eq!(reference.memories[1].kind, eda_core::ir::MemoryKind::Ram);
    assert_eq!(
        direct.memories[0].template,
        eda_core::ir::MemoryTemplate::AsyncRam
    );
    assert_eq!(
        direct.memories[1].template,
        eda_core::ir::MemoryTemplate::AsyncRam
    );
    assert_eq!(
        reference.memories[0].template,
        eda_core::ir::MemoryTemplate::AsyncRam
    );
    assert_eq!(
        reference.memories[1].template,
        eda_core::ir::MemoryTemplate::AsyncRam
    );
    let mut direct_simulator =
        Simulator::new(&direct).expect("create direct shared-memory simulator");
    direct_simulator
        .set_input("din", &LogicWord::from_u64(8, 0xa7))
        .expect("set direct shared data");
    assert_eq!(
        direct_simulator.read("child_q").unwrap().to_u64(),
        Some(0xa7)
    );
    assert_eq!(
        direct_simulator.read("parent_q").unwrap().to_u64(),
        Some(0xa7)
    );

    let mut reference_simulator =
        Simulator::new(&reference).expect("create bounded shared-memory simulator");
    reference_simulator
        .set_input("din", &LogicWord::from_u64(8, 0xa7))
        .expect("set bounded shared data");
    assert_eq!(
        reference_simulator.read("child_q").unwrap().to_u64(),
        Some(0xa7)
    );
    assert_eq!(
        reference_simulator.read("parent_q").unwrap().to_u64(),
        Some(0xa7)
    );
}

#[test]
fn slang_ast_direct_lowering_initializes_multidimensional_sparse_memory() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-memory-init");
    let source_path = root.join("memory-init.sv");
    std::fs::write(
        &source_path,
        r#"
module memory_init(
  output logic [7:0] first,
  output logic [7:0] middle,
  output logic [7:0] last
);
  logic [7:0] mem [0:1][0:2] = '{
    '{8'h01, 8'h02, 8'h03},
    '{8'h04, 8'h05, 8'h06}
  };
  assign first = mem[0][0];
  assign middle = mem[0][2];
  assign last = mem[1][2];
endmodule
"#,
    )
    .expect("write sparse-memory initializer source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("memory_init"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower sparse-memory initializer");
    assert_eq!(design.memories.len(), 1);
    assert_eq!(design.memories[0].dimensions.len(), 2);
    assert_eq!(design.memories[0].flat_length, 6);
    assert_eq!(design.memories[0].kind, eda_core::ir::MemoryKind::Rom);
    assert_eq!(
        design.memories[0].template,
        eda_core::ir::MemoryTemplate::AsyncRom
    );
    assert_eq!(design.memories[0].write_mask, None);
    assert_eq!(design.memories[0].byte_enable, None);
    assert_eq!(
        design.memories[0].banking,
        eda_core::ir::MemoryBanking { count: 2, depth: 3 }
    );
    assert_eq!(design.initials.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create initializer simulator");
    simulator
        .run_until(0)
        .expect("settle sparse-memory initializers");
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("middle").unwrap().to_u64(), Some(3));
    assert_eq!(simulator.read("last").unwrap().to_u64(), Some(6));
}

#[test]
fn slang_ast_direct_lowering_reads_sparse_memory_files() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-readmem");
    let source_path = root.join("readmem.sv");
    std::fs::write(
        &source_path,
        r#"
module readmem(
  output logic [7:0] first,
  output logic [7:0] second,
  output logic [7:0] skipped,
  output logic [7:0] last,
  output logic [1:0] binary
);
  logic [7:0] mem [0:4096];
  logic [1:0] bits [0:2048];
  initial begin
    $readmemh("memory.hex", mem);
    $readmemb("bits.bin", bits);
  end
  assign first = mem[0];
  assign second = mem[1];
  assign skipped = mem[2];
  assign last = mem[3];
  assign binary = bits[1];
endmodule
"#,
    )
    .expect("write memory-file source");
    std::fs::write(
        root.join("memory.hex"),
        r#"
// leading comment
01 02
/* block
   comment with ff ff */
@3 04
"#,
    )
    .expect("write hexadecimal memory file");
    std::fs::write(root.join("bits.bin"), "01\n10\n").expect("write binary memory file");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("readmem"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower sparse-memory file loads");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("readmem"),
            &PreprocessOptions::default(),
        )
        .expect("lower memory files through the bounded path");
    assert_eq!(design.memories.len(), 2);
    assert_eq!(design.memories[0].kind, eda_core::ir::MemoryKind::Rom);
    assert_eq!(design.memories[1].kind, eda_core::ir::MemoryKind::Rom);
    assert_eq!(
        design.memories[0].template,
        eda_core::ir::MemoryTemplate::AsyncRom
    );
    assert_eq!(
        design.memories[1].template,
        eda_core::ir::MemoryTemplate::AsyncRom
    );
    assert_eq!(
        design.memories[0].banking,
        eda_core::ir::MemoryBanking {
            count: 1,
            depth: 4097
        }
    );
    assert_eq!(design.initials.len(), 1);
    assert_eq!(reference.memories.len(), 2);
    assert_eq!(reference.memories[0].kind, eda_core::ir::MemoryKind::Rom);
    assert_eq!(reference.memories[1].kind, eda_core::ir::MemoryKind::Rom);
    assert_eq!(
        reference.memories[0].template,
        eda_core::ir::MemoryTemplate::AsyncRom
    );
    assert_eq!(
        reference.memories[1].template,
        eda_core::ir::MemoryTemplate::AsyncRom
    );
    assert_eq!(
        reference.memories[1].banking,
        eda_core::ir::MemoryBanking {
            count: 1,
            depth: 2049
        }
    );
    assert_eq!(reference.initials.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create memory-file simulator");
    simulator
        .run_until(0)
        .expect("settle memory-file initializers");
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("second").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("skipped").unwrap().to_u64(), None);
    assert_eq!(simulator.read("last").unwrap().to_u64(), Some(4));
    assert_eq!(simulator.read("binary").unwrap().to_u64(), Some(2));

    let mut reference_simulator =
        Simulator::new(&reference).expect("create bounded memory-file simulator");
    reference_simulator
        .run_until(0)
        .expect("settle bounded memory-file initializers");
    assert_eq!(reference_simulator.read("first").unwrap().to_u64(), Some(1));
    assert_eq!(
        reference_simulator.read("second").unwrap().to_u64(),
        Some(2)
    );
    assert_eq!(reference_simulator.read("skipped").unwrap().to_u64(), None);
    assert_eq!(reference_simulator.read("last").unwrap().to_u64(), Some(4));
    assert_eq!(
        reference_simulator.read("binary").unwrap().to_u64(),
        Some(2)
    );
}

#[test]
fn slang_and_bounded_memory_file_ranges_load_the_same_elements() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-readmem-range");
    let source_path = root.join("readmem-range.sv");
    std::fs::write(
        &source_path,
        r#"
module readmem_range(
  output logic [7:0] first,
  output logic [7:0] second,
  output logic [7:0] third,
  output logic [7:0] after
);
  logic [7:0] mem [0:4096];
  initial $readmemh("range.hex", mem, 2, 4);
  assign first = mem[2];
  assign second = mem[3];
  assign third = mem[4];
  assign after = mem[5];
endmodule
"#,
    )
    .expect("write ranged memory-file source");
    std::fs::write(root.join("range.hex"), "01 02 03 04 05\n").expect("write range data");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("readmem_range"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower ranged memory file");
    let bounded = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("readmem_range"),
            &PreprocessOptions::default(),
        )
        .expect("lower ranged memory file through bounded path");
    assert_eq!(direct.memories.len(), 1);
    assert_eq!(bounded.memories.len(), 1);
    assert_eq!(direct.initials.len(), 1);
    assert_eq!(bounded.initials.len(), 1);

    for design in [&direct, &bounded] {
        let mut simulator = Simulator::new(design).expect("create ranged memory simulator");
        simulator
            .run_until(0)
            .expect("settle ranged memory initializer");
        assert_eq!(simulator.read("first").unwrap().to_u64(), Some(1));
        assert_eq!(simulator.read("second").unwrap().to_u64(), Some(2));
        assert_eq!(simulator.read("third").unwrap().to_u64(), Some(3));
        assert_eq!(simulator.read("after").unwrap().to_u64(), None);
    }
}

#[test]
fn slang_and_bounded_paths_infer_fixed_partial_write_masks() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("fixed-write-mask");
    let source_path = root.join("write-mask.sv");
    std::fs::write(
        &source_path,
        r#"
module write_mask(
  input logic clk,
  input logic [3:0] din,
  output logic [15:0] q
);
  logic [15:0] mem [0:4095];
  always @(posedge clk) mem[0][3:0] <= din;
  assign q = mem[0];
endmodule
"#,
    )
    .expect("write fixed memory mask source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("write_mask"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower fixed memory mask");
    let bounded = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("write_mask"),
            &PreprocessOptions::default(),
        )
        .expect("lower fixed memory mask through bounded path");
    for design in [&direct, &bounded] {
        assert_eq!(design.memories.len(), 1);
        assert_eq!(design.memories[0].kind, eda_core::ir::MemoryKind::Ram);
        assert_eq!(
            design.memories[0].template,
            eda_core::ir::MemoryTemplate::SyncRam
        );
        assert_eq!(
            design.memories[0].write_mask,
            Some(eda_core::ir::MemoryWriteMask {
                width: 16,
                bits: [
                    true, true, true, true, false, false, false, false, false, false, false, false,
                    false, false, false, false
                ]
                .to_vec()
            })
        );
        assert_eq!(design.memories[0].byte_enable, None);
    }
}

#[test]
fn slang_and_bounded_paths_infer_aligned_byte_enables() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("aligned-byte-enable");
    let source_path = root.join("byte-enable.sv");
    std::fs::write(
        &source_path,
        r#"
module byte_enable(
  input logic clk,
  input logic [7:0] din,
  output logic [31:0] q
);
  logic [31:0] mem [0:4095];
  always @(posedge clk) mem[0][23:16] <= din;
  assign q = mem[0];
endmodule
"#,
    )
    .expect("write aligned byte-enable source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("byte_enable"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower aligned byte enable");
    let bounded = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("byte_enable"),
            &PreprocessOptions::default(),
        )
        .expect("lower aligned byte enable through bounded path");
    let expected = eda_core::ir::MemoryByteEnable {
        lane_width: 8,
        lanes: [false, false, true, false].to_vec(),
    };
    for design in [&direct, &bounded] {
        assert_eq!(design.memories.len(), 1);
        assert_eq!(design.memories[0].byte_enable, Some(expected.clone()));
    }
}

#[test]
fn slang_and_bounded_paths_infer_synchronous_rom_template() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("synchronous-rom-template");
    let source_path = root.join("sync-rom.sv");
    std::fs::write(
        &source_path,
        r#"
module sync_rom(
  input logic clk,
  input logic [11:0] address,
  output logic [7:0] q
);
  logic [7:0] rom [0:4095];
  initial $readmemh("rom.hex", rom);
  always @(posedge clk) q <= rom[address];
endmodule
"#,
    )
    .expect("write synchronous ROM source");
    std::fs::write(root.join("rom.hex"), "01 22 44 88\n").expect("write ROM data");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("sync_rom"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower synchronous ROM");
    let bounded = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("sync_rom"),
            &PreprocessOptions::default(),
        )
        .expect("lower synchronous ROM through bounded path");
    for design in [&direct, &bounded] {
        assert_eq!(design.memories.len(), 1);
        assert_eq!(design.memories[0].kind, eda_core::ir::MemoryKind::Rom);
        assert_eq!(
            design.memories[0].template,
            eda_core::ir::MemoryTemplate::SyncRom
        );

        let mut simulator = Simulator::new(design).expect("create synchronous ROM simulator");
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .expect("initialize ROM clock");
        simulator
            .set_input("address", &LogicWord::from_u64(12, 2))
            .expect("select initialized ROM word");
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .expect("create synchronous ROM read edge");
        assert_eq!(simulator.read("q").unwrap().to_u64(), Some(0x44));
    }
}

#[test]
fn slang_and_bounded_paths_write_sparse_memory_files() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("memory-write-files");
    let source_path = root.join("writemem.sv");
    std::fs::write(
        &source_path,
        r#"
module writemem;
  logic [7:0] mem [0:4096];
  logic [1:0] bits [0:2048];
  initial begin
    mem[2] = 8'h01;
    mem[3] = 8'h02;
    bits[1] = 2'b10;
    $writememh("writemem.hex", mem, 2, 4);
    $writememb("writemem.bin", bits, 1, 2);
  end
endmodule
"#,
    )
    .expect("write sparse-memory dump source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("writemem"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower sparse-memory dump tasks");
    let bounded = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("writemem"),
            &PreprocessOptions::default(),
        )
        .expect("lower sparse-memory dump tasks through bounded path");
    assert_eq!(direct.memories.len(), 2);
    assert_eq!(bounded.memories.len(), 2);
    assert_eq!(direct.initials.len(), 1);
    assert_eq!(bounded.initials.len(), 1);

    for design in [&direct, &bounded] {
        let mut simulator = Simulator::new(design).expect("create sparse-memory dump simulator");
        simulator
            .run_until(0)
            .expect("execute sparse-memory dump tasks");
        assert_eq!(
            std::fs::read_to_string(root.join("writemem.hex")).unwrap(),
            "1\n2\nxx\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("writemem.bin")).unwrap(),
            "10\nxx\n"
        );
    }
}

#[test]
fn slang_ast_summary_validates_elaborated_structure_after_lowering() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("ast-summary");
    let source_path = root.join("top.sv");
    std::fs::write(
        &source_path,
        r#"
module child #(parameter CHILD_WIDTH = 1)
    (input logic a, output logic y);
  assign y = ~a;
endmodule

module top #(parameter WIDTH = 4)
    (input logic [WIDTH-1:0] a, output logic [WIDTH-1:0] y,
     output logic [7:0] lit);
  logic [7:0] wide;
  logic held;
  logic [WIDTH-1:0] sum;
  child #(.CHILD_WIDTH(1)) u(.a(a[0]), .y(y[0]));
  initial held = a[0];
  assign y[3:1] = a[3:1];
  assign wide = {a, a};
  assign sum = a + a;
  assign lit = 8'hA5;
endmodule
"#,
    )
    .expect("write hierarchy source");

    let design = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("compile and structurally validate hierarchy");
    let json = slang
        .ast_json(&[source_path], Some("top"), &["top".to_string()])
        .expect("serialize slang AST");
    let summary = slang
        .summarize_ast_json(&json)
        .expect("parse slang AST summary");

    assert_eq!(summary.top_name, "top");
    for (key, expected_line) in [
        ("top", 7),
        ("parameter:WIDTH", 7),
        ("port:a", 8),
        ("declaration:wide", 10),
        ("instance:u", 13),
        ("procedural:0", 14),
        ("continuous:wide:1", 16),
        ("continuous:sum:2", 17),
        ("continuous:lit:3", 18),
    ] {
        let span = summary
            .source_spans
            .get(key)
            .unwrap_or_else(|| panic!("missing source span {key}"));
        assert!(span.file.ends_with("top.sv"), "{key} file: {}", span.file);
        assert_eq!(span.start_line, expected_line, "{key}");
        assert!(span.start_column > 0, "{key}");
    }
    assert_eq!(
        summary
            .ports
            .iter()
            .map(|port| (port.name.as_str(), port.direction.as_str()))
            .collect::<Vec<_>>(),
        vec![("a", "In"), ("y", "Out"), ("lit", "Out")]
    );
    assert!(summary.declarations.iter().any(|declaration| {
        declaration.name == "wide"
            && declaration.kind == "Variable"
            && declaration.type_name == "logic[7:0]"
            && declaration.width == Some(8)
    }));
    assert_eq!(
        summary
            .parameters
            .iter()
            .map(|parameter| {
                (
                    parameter.name.as_str(),
                    parameter.type_name.as_str(),
                    parameter.value.as_str(),
                    parameter.is_local,
                    parameter.is_port,
                )
            })
            .collect::<Vec<_>>(),
        vec![("WIDTH", "logic signed[31:0]", "4", false, true)]
    );
    assert_eq!(
        summary
            .continuous_targets
            .iter()
            .map(|target| {
                (
                    target.name.as_str(),
                    target.source.as_str(),
                    target.rhs.kind.as_str(),
                    target.rhs.operator.as_str(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            ("y", "ContinuousAssign", "RangeSelect", ""),
            ("wide", "ContinuousAssign", "Concatenation", ""),
            ("sum", "ContinuousAssign", "BinaryOp", "Add"),
            ("lit", "ContinuousAssign", "Conversion", ""),
        ]
    );
    let sum_rhs = &summary
        .continuous_targets
        .iter()
        .find(|target| target.name == "sum")
        .expect("sum assignment summary")
        .rhs;
    assert_eq!(sum_rhs.operator, "Add");
    assert_eq!(sum_rhs.type_width, Some(4));
    assert_eq!(sum_rhs.type_signed, Some(false));
    assert_eq!(
        sum_rhs
            .operands
            .iter()
            .map(|operand| (operand.kind.as_str(), operand.symbol.as_deref()))
            .collect::<Vec<_>>(),
        vec![("NamedValue", Some("a")), ("NamedValue", Some("a"))]
    );
    let child_instance = summary
        .instances
        .iter()
        .find(|instance| instance.instance_name == "u")
        .expect("child instance parameter summary");
    assert_eq!(
        child_instance
            .parameters
            .iter()
            .map(|parameter| {
                (
                    parameter.name.as_str(),
                    parameter.value.as_str(),
                    parameter.is_local,
                    parameter.is_port,
                )
            })
            .collect::<Vec<_>>(),
        vec![("CHILD_WIDTH", "1", false, true)]
    );
    let wide_rhs = &summary
        .continuous_targets
        .iter()
        .find(|target| target.name == "wide")
        .expect("wide assignment summary")
        .rhs;
    assert_eq!(
        wide_rhs
            .operands
            .iter()
            .map(|operand| operand.kind.as_str())
            .collect::<Vec<_>>(),
        vec!["NamedValue", "NamedValue"]
    );
    let literal_rhs = &summary
        .continuous_targets
        .iter()
        .find(|target| target.name == "lit")
        .expect("literal assignment summary")
        .rhs;
    assert_eq!(literal_rhs.type_width, Some(8));
    assert_eq!(literal_rhs.type_signed, Some(false));
    assert_eq!(literal_rhs.operands.len(), 1);
    assert_eq!(
        literal_rhs
            .operands
            .first()
            .and_then(|operand| operand.literal.as_deref()),
        Some("8:10100101")
    );
    assert_eq!(
        summary
            .procedural_blocks
            .iter()
            .map(|block| {
                (
                    block.procedure_kind.clone(),
                    block.assignment_targets.clone(),
                    block
                        .assignments
                        .iter()
                        .map(|assignment| {
                            (
                                assignment.target_names.clone(),
                                assignment.rhs.kind.clone(),
                                assignment.rhs.operator.clone(),
                                assignment.nonblocking,
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>(),
        vec![(
            "Initial".to_string(),
            vec!["held".to_string()],
            vec![(
                vec!["held".to_string()],
                "ElementSelect".to_string(),
                String::new(),
                false
            )]
        )]
    );
    assert_eq!(
        summary
            .instances
            .iter()
            .map(|instance| {
                (
                    instance.instance_name.as_str(),
                    instance.definition_name.as_str(),
                )
            })
            .collect::<Vec<_>>(),
        vec![("u", "child")]
    );
    assert!(summary.kinds.contains("InstanceBody"));
    assert!(summary.kinds.contains("ContinuousAssign"));
    summary
        .validate_against_design(&design)
        .expect("slang and bounded design inventories agree");

    let mut dropped_port = summary.clone();
    dropped_port.ports.pop();
    assert!(dropped_port
        .validate_against_design(&design)
        .unwrap_err()
        .contains("do not match slang"));

    let mut dropped_declaration = summary.clone();
    dropped_declaration
        .declarations
        .push(eda_slang::SlangDeclarationSummary {
            name: "missing".into(),
            kind: "Variable".into(),
            type_name: "logic".into(),
            width: Some(1),
        });
    assert!(dropped_declaration
        .validate_against_design(&design)
        .unwrap_err()
        .contains("dropped slang declaration 'missing'"));

    let mut changed_width = summary.clone();
    changed_width
        .declarations
        .iter_mut()
        .find(|declaration| declaration.name == "wide")
        .expect("wide declaration summary")
        .width = Some(7);
    assert!(changed_width
        .validate_against_design(&design)
        .unwrap_err()
        .contains("width 8 does not match slang width 7"));

    let mut dropped_continuous = summary.clone();
    dropped_continuous
        .continuous_targets
        .push(eda_slang::SlangContinuousTarget {
            name: "wide".into(),
            source: "ContinuousAssign".into(),
            rhs: eda_slang::SlangExpressionSummary {
                kind: "BinaryOp".into(),
                type_name: String::new(),
                operator: "Add".into(),
                type_width: None,
                type_signed: None,
                symbol: None,
                literal: None,
                repeat_count: None,
                operands: Vec::new(),
            },
        });
    assert!(dropped_continuous
        .validate_against_design(&design)
        .unwrap_err()
        .contains("continuous assignment shape for 'wide' does not match slang"));

    let mut changed_operand_shape = summary.clone();
    let sum_rhs = changed_operand_shape
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "sum")
        .expect("sum assignment summary")
        .rhs
        .operands
        .first_mut()
        .expect("sum binary operand");
    sum_rhs.kind = "BinaryOp".into();
    sum_rhs.operator = "Add".into();
    sum_rhs.symbol = None;
    assert!(changed_operand_shape
        .validate_against_design(&design)
        .unwrap_err()
        .contains("continuous assignment shape for 'sum' does not match slang"));

    let mut changed_operand_symbol = summary.clone();
    let operand = changed_operand_symbol
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "sum")
        .expect("sum assignment summary")
        .rhs
        .operands
        .first_mut()
        .expect("sum binary operand");
    operand.symbol = Some("wrong".into());
    assert!(changed_operand_symbol
        .validate_against_design(&design)
        .unwrap_err()
        .contains("continuous assignment shape for 'sum' does not match slang"));

    let mut changed_operand_width = changed_operand_symbol;
    let operand = changed_operand_width
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "sum")
        .expect("sum assignment summary")
        .rhs
        .operands
        .first_mut()
        .expect("sum binary operand");
    operand.type_width = Some(3);
    assert!(changed_operand_width
        .validate_against_design(&design)
        .unwrap_err()
        .contains("continuous assignment shape for 'sum' does not match slang"));

    let mut changed_operand_signedness = changed_operand_width;
    let operand = changed_operand_signedness
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "sum")
        .expect("sum assignment summary")
        .rhs
        .operands
        .first_mut()
        .expect("sum binary operand");
    operand.type_width = Some(4);
    operand.type_signed = Some(true);
    assert!(changed_operand_signedness
        .validate_against_design(&design)
        .unwrap_err()
        .contains("continuous assignment shape for 'sum' does not match slang"));

    let mut dropped_procedural = summary.clone();
    dropped_procedural
        .procedural_blocks
        .push(eda_slang::SlangProceduralBlockSummary {
            procedure_kind: "Initial".into(),
            assignment_targets: vec!["held".into()],
            assignments: Vec::new(),
        });
    assert!(dropped_procedural
        .validate_against_design(&design)
        .unwrap_err()
        .contains("dropped initial blocks"));

    let mut changed_procedural_rhs = summary.clone();
    let assignment = changed_procedural_rhs
        .procedural_blocks
        .iter_mut()
        .find(|block| block.procedure_kind == "Initial")
        .expect("initial block summary")
        .assignments
        .first_mut()
        .expect("initial assignment summary");
    assignment.rhs.kind = "BinaryOp".into();
    assignment.rhs.operator = "Add".into();
    assert!(changed_procedural_rhs
        .validate_against_design(&design)
        .unwrap_err()
        .contains("procedural assignment shape for 'held' does not match slang"));

    let mut changed_literal = summary.clone();
    let literal_rhs = changed_literal
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "lit")
        .expect("literal assignment summary")
        .rhs
        .operands
        .first_mut()
        .expect("integer literal under conversion")
        .literal
        .as_mut()
        .expect("slang literal fingerprint");
    *literal_rhs = "8:10100100".into();
    assert!(changed_literal
        .validate_against_design(&design)
        .unwrap_err()
        .contains("continuous assignment shape for 'lit' does not match slang"));

    let mut dropped_instance = summary;
    dropped_instance.instances.pop();
    assert!(dropped_instance
        .validate_against_design(&design)
        .unwrap_err()
        .contains("direct instances"));
}

#[test]
fn slang_front_end_rejects_invalid_rtl() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("invalid");
    let source_path = root.join("broken.sv");
    std::fs::write(&source_path, "module broken(output y; assign y = 1'b0;\n")
        .expect("write broken source");
    let error = slang
        .check_files(&[source_path], Some("broken"))
        .expect_err("slang must reject malformed RTL");
    let diagnostic = format!("{error}\n{}\n{}", error.stdout, error.stderr);
    assert!(
        diagnostic.contains("error"),
        "unexpected failure: {diagnostic}"
    );
}

#[test]
fn slang_structural_validation_accepts_generate_instances() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let design = slang
        .compile_source(
            r#"
module child(input logic a, output logic y);
  assign y = a;
endmodule

module top(input logic a, output logic y);
  generate
    if (1) begin : selected
      child u(.a(a), .y(y));
    end
  endgenerate
endmodule
"#,
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("compile generated instance through slang boundary");
    assert_eq!(design.name, "top");
    assert!(design.instance("top.u").is_some());
}

#[test]
fn slang_ast_direct_lowering_accepts_generated_members() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let source = r#"
module child(input logic a, output logic y);
  assign y = a;
endmodule

module top(input logic a, output logic y, output logic z);
  generate
    if (1) begin : selected
      child u(.a(a), .y(y));
      assign z = ~a;
    end
  endgenerate
endmodule
"#;
    let root = temporary_directory("direct-ast-generate");
    let source_path = root.join("generate.sv");
    std::fs::write(&source_path, source).expect("write generate source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower generated members");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("lower generated members through the preprocessed path");
    assert_eq!(design.n_nets, 5);
    assert_eq!(design.assigns.len(), 4);
    assert_eq!(reference.n_nets, design.n_nets);
    assert_eq!(reference.assigns.len(), design.assigns.len());
    assert!(design
        .hierarchy
        .iter()
        .any(|instance| instance.path == "top.selected.u"));
    // The bounded compatibility path intentionally flattens generate labels;
    // compare behavior rather than requiring identical hierarchy names.
    let mut reference_simulator = Simulator::new(&reference).expect("create reference simulator");
    reference_simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .expect("set reference generate input");

    let mut simulator = Simulator::new(&design).expect("create generate simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .expect("set generate input");
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("z").unwrap().to_u64(), Some(0));
    assert_eq!(reference_simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(reference_simulator.read("z").unwrap().to_u64(), Some(0));
}

#[test]
fn slang_ast_direct_lowering_accepts_generate_for_assignments() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-generate-for");
    let source_path = root.join("generate_for.sv");
    std::fs::write(
        &source_path,
        r#"
module generate_for(
  input logic a,
  output logic [3:0] forward,
  output logic [3:0] reverse
);
  for (genvar i = 0; i < 4; i = i + 1) begin : iteration
    assign forward[i] = a ^ i[0];
    assign reverse[3 - i] = a ^ i[1];
  end
endmodule
"#,
    )
    .expect("write generate-for source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("generate_for"),
            &PreprocessOptions::default(),
        )
        .expect("lower generated assignments");
    assert_eq!(design.assigns.len(), 8);

    let mut simulator = Simulator::new(&design).expect("create generate simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("forward").unwrap().to_u64(), Some(0b0101));
    assert_eq!(simulator.read("reverse").unwrap().to_u64(), Some(0b1100));
}

#[test]
fn slang_ast_direct_lowering_accepts_generate_for_instances() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-generate-instances");
    let source_path = root.join("generate_instances.sv");
    std::fs::write(
        &source_path,
        r#"
module child(input logic a, output logic y);
  assign y = ~a;
endmodule

module generate_instances(input logic a, output logic [3:0] y);
  for (genvar i = 0; i < 4; i = i + 1) begin : iteration
    child u(.a(a), .y(y[i]));
  end
endmodule
"#,
    )
    .expect("write generated-instance source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("generate_instances"),
            &PreprocessOptions::default(),
        )
        .expect("lower generated instances");
    assert_eq!(design.assigns.len(), 12);
    assert_eq!(design.hierarchy.len(), 5);
    let child_paths = design
        .hierarchy
        .iter()
        .filter(|instance| instance.depth == 1)
        .map(|instance| instance.path.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        child_paths,
        vec![
            "generate_instances.iteration[0].u",
            "generate_instances.iteration[1].u",
            "generate_instances.iteration[2].u",
            "generate_instances.iteration[3].u",
        ]
    );

    let mut simulator = Simulator::new(&design).expect("create generated-instance simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0));
}

#[test]
fn slang_ast_direct_lowering_preserves_continuous_assignment_delay() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-delay");
    let source_path = root.join("delay.sv");
    std::fs::write(
        &source_path,
        r#"
module delay(input logic a, output logic y);
  assign #5 y = a;
endmodule
"#,
    )
    .expect("write delayed assignment source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("delay"),
            &PreprocessOptions::default(),
        )
        .expect("lower delayed continuous assignment");
    assert_eq!(design.assigns.len(), 1);
    assert_eq!(design.assigns[0].delay, 5);

    let mut simulator = Simulator::new(&design).expect("create delayed simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .expect("set delayed input");
    simulator.run_until(4).expect("run before delay");
    assert_eq!(simulator.read("y").unwrap().to_u64(), None);
    simulator.run_until(5).expect("run to delay");
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_fork_join_all() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-fork-join");
    let source_path = root.join("fork_join.sv");
    std::fs::write(
        &source_path,
        r#"
module fork_join(output logic x, output logic y, output logic z);
  initial begin
    fork
      #5 x = 1'b1;
      #2 y = 1'b1;
    join
    z = 1'b1;
  end
endmodule
"#,
    )
    .expect("write fork/join source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("fork_join"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower fork/join");
    assert_eq!(design.initials.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create fork/join simulator");
    simulator.run_until(2).unwrap();
    assert_eq!(simulator.read("x").unwrap().to_u64(), None);
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("z").unwrap().to_u64(), None);
    simulator.run_until(5).unwrap();
    assert_eq!(simulator.read("x").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("z").unwrap().to_u64(), Some(1));

    let nested_path = root.join("nested-fork.sv");
    std::fs::write(
        &nested_path,
        r#"
module nested_fork(output logic x, output logic y, output logic w, output logic z);
  initial begin
    fork
      #1 x = 1'b1;
      fork
        #2 y = 1'b1;
        #3 w = 1'b1;
      join
    join
    #4 z = 1'b1;
  end
endmodule
"#,
    )
    .expect("write nested fork/join source");
    let nested = slang
        .compile_files_via_ast(
            std::slice::from_ref(&nested_path),
            Some("nested_fork"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower nested fork/join");
    let mut nested_simulator = Simulator::new(&nested).expect("create nested fork simulator");
    nested_simulator.run_until(1).unwrap();
    assert_eq!(nested_simulator.read("x").unwrap().to_u64(), Some(1));
    assert_eq!(nested_simulator.read("w").unwrap().to_u64(), None);
    nested_simulator.run_until(2).unwrap();
    assert_eq!(nested_simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(nested_simulator.read("z").unwrap().to_u64(), None);
    nested_simulator.run_until(3).unwrap();
    assert_eq!(nested_simulator.read("w").unwrap().to_u64(), Some(1));
    assert_eq!(nested_simulator.read("z").unwrap().to_u64(), None);
    nested_simulator.run_until(7).unwrap();
    assert_eq!(nested_simulator.read("z").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_fork_join_any_and_none() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-fork-modes");

    let any_path = root.join("join_any.sv");
    std::fs::write(
        &any_path,
        r#"
module fork_any(output logic x, output logic y, output logic z);
  initial begin
    fork
      #5 x = 1'b1;
      #2 y = 1'b1;
    join_any
    z = 1'b1;
  end
endmodule
"#,
    )
    .expect("write join_any source");
    let any = slang
        .compile_files_via_ast(
            std::slice::from_ref(&any_path),
            Some("fork_any"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower join_any");
    let mut any_simulator = Simulator::new(&any).expect("create join_any simulator");
    any_simulator.run_until(2).unwrap();
    assert_eq!(any_simulator.read("x").unwrap().to_u64(), None);
    assert_eq!(any_simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(any_simulator.read("z").unwrap().to_u64(), Some(1));
    any_simulator.run_until(5).unwrap();
    assert_eq!(any_simulator.read("x").unwrap().to_u64(), Some(1));

    let none_path = root.join("join_none.sv");
    std::fs::write(
        &none_path,
        r#"
module fork_none(
  output logic x,
  output logic y,
  output logic z,
  output logic parent_started
);
  initial begin
    parent_started = 1'b1;
    fork
      #2 x = 1'b1;
      #3 y = 1'b1;
    join_none
    z = 1'b1;
  end
endmodule
"#,
    )
    .expect("write join_none source");
    let none = slang
        .compile_files_via_ast(
            std::slice::from_ref(&none_path),
            Some("fork_none"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower join_none");
    let mut none_simulator = Simulator::new(&none).expect("create join_none simulator");
    none_simulator.run_until(0).unwrap();
    assert_eq!(
        none_simulator.read("parent_started").unwrap().to_u64(),
        Some(1)
    );
    assert_eq!(none_simulator.read("z").unwrap().to_u64(), Some(1));
    assert_eq!(none_simulator.read("x").unwrap().to_u64(), None);
    assert_eq!(none_simulator.read("y").unwrap().to_u64(), None);
    none_simulator.run_until(2).unwrap();
    assert_eq!(none_simulator.read("x").unwrap().to_u64(), Some(1));
    assert_eq!(none_simulator.read("y").unwrap().to_u64(), None);
    none_simulator.run_until(3).unwrap();
    assert_eq!(none_simulator.read("y").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_flattens_instance_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-instance-array");
    let source_path = root.join("instance_array.sv");
    std::fs::write(
        &source_path,
        r#"
module child(input logic a, output logic y);
  assign y = ~a;
endmodule

module top #(parameter N = 2) (
  input logic [N-1:0] a,
  output logic [N-1:0] y
);
  child u[N] (.a(a), .y(y));
endmodule
"#,
    )
    .expect("write instance-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower instance array");
    assert_eq!(design.hierarchy.len(), 3);
    assert_eq!(
        design
            .hierarchy
            .iter()
            .map(|instance| instance.path.as_str())
            .collect::<Vec<_>>(),
        vec!["top", "top.u[0]", "top.u[1]"]
    );
    let mut simulator = Simulator::new(&design).expect("create instance-array simulator");
    for value in 0..4_u64 {
        simulator
            .set_input("a", &LogicWord::from_u64(2, value))
            .unwrap();
        assert_eq!(simulator.read("y").unwrap().to_u64(), Some(!value & 0b11));
    }

    let multidimensional_path = root.join("instance_array_2d.sv");
    std::fs::write(
        &multidimensional_path,
        r#"
module child(input logic a, output logic y);
  assign y = ~a;
endmodule

module top (
  input logic [3:0] a,
  output logic [3:0] y
);
  child u[2][2] (.a(a), .y(y));
endmodule
"#,
    )
    .expect("write multidimensional instance-array source");
    let multidimensional = slang
        .compile_files_via_ast(
            std::slice::from_ref(&multidimensional_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional instance array");
    assert_eq!(multidimensional.hierarchy.len(), 5);
    assert_eq!(
        multidimensional
            .hierarchy
            .iter()
            .map(|instance| instance.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "top",
            "top.u[0][0]",
            "top.u[0][1]",
            "top.u[1][0]",
            "top.u[1][1]"
        ]
    );
    let mut multidimensional_simulator =
        Simulator::new(&multidimensional).expect("create multidimensional array simulator");
    multidimensional_simulator
        .set_input("a", &LogicWord::from_u64(4, 0b1010))
        .unwrap();
    assert_eq!(
        multidimensional_simulator.read("y").unwrap().to_u64(),
        Some(0b0101)
    );
}

#[test]
fn slang_ast_direct_lowering_flattens_generated_instance_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-generated-instance-array");
    let source_path = root.join("generated_instance_array.sv");
    std::fs::write(
        &source_path,
        r#"
module child(input logic a, output logic y);
  assign y = ~a;
endmodule

module generated_instance_arrays(
  input logic [3:0] a,
  output logic [3:0] y
);
  generate
    if (1) begin : generated_scope
      child cells[4] (.a(a), .y(y));
    end
  endgenerate
endmodule
"#,
    )
    .expect("write generated instance-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("generated_instance_arrays"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower generated instance array");
    assert_eq!(design.hierarchy.len(), 5);
    assert_eq!(
        design
            .hierarchy
            .iter()
            .map(|instance| instance.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "generated_instance_arrays",
            "generated_instance_arrays.generated_scope.cells[0]",
            "generated_instance_arrays.generated_scope.cells[1]",
            "generated_instance_arrays.generated_scope.cells[2]",
            "generated_instance_arrays.generated_scope.cells[3]"
        ]
    );
    let mut simulator = Simulator::new(&design).expect("create generated array simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(4, 0b1001))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0b0110));
}

#[test]
fn slang_ast_direct_lowering_supports_interface_modports() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-modport");
    let source_path = root.join("interface_modport.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic valid;
  logic ready = 1'b1;
  modport src(input clk, ready, output valid);
endinterface

module producer(
  bus_if.src bus,
  input logic go,
  output logic done
);
  assign bus.valid = go;
  assign done = bus.ready;
endmodule

module interface_top(
  input logic clk,
  input logic go,
  output logic valid,
  output logic done
);
  bus_if bus(.clk(clk));
  producer producer_u(.bus(bus), .go(go), .done(done));
endmodule
"#,
    )
    .expect("write interface-modport source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower interface modports");
    assert_eq!(design.hierarchy.len(), 3);
    assert_eq!(
        design
            .hierarchy
            .iter()
            .map(|instance| instance.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "interface_top",
            "interface_top.bus",
            "interface_top.producer_u"
        ]
    );
    assert!(design
        .net_decls
        .iter()
        .any(|declaration| declaration.name == "bus.valid"));
    assert!(design
        .net_decls
        .iter()
        .any(|declaration| declaration.name == "bus.ready"));

    let mut simulator = Simulator::new(&design).expect("create interface simulator");
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("bus.valid").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("done").unwrap().to_u64(), Some(1));

    let mut ast = serde_json::from_str::<serde_json::Value>(
        &slang
            .ast_json_with_options(
                std::slice::from_ref(&source_path),
                Some("interface_top"),
                &[],
                &PreprocessOptions::default(),
            )
            .expect("serialize authoritative interface AST"),
    )
    .expect("parse authoritative interface AST");
    fn make_valid_input(value: &mut serde_json::Value) {
        if let Some(fields) = value.as_object_mut() {
            if fields.get("kind").and_then(serde_json::Value::as_str) == Some("Modport")
                && fields.get("name").and_then(serde_json::Value::as_str) == Some("src")
            {
                if let Some(members) = fields
                    .get_mut("members")
                    .and_then(serde_json::Value::as_array_mut)
                {
                    for member in members {
                        if member.get("name").and_then(serde_json::Value::as_str) == Some("valid") {
                            member["direction"] = serde_json::Value::String("In".into());
                        }
                    }
                }
            }
            for nested in fields.values_mut() {
                make_valid_input(nested);
            }
        } else if let Some(values) = value.as_array_mut() {
            for nested in values {
                make_valid_input(nested);
            }
        }
    }
    make_valid_input(&mut ast);
    let error = slang
        .lower_ast_json(&ast.to_string())
        .expect_err("modport input writes must fail closed");
    assert!(
        error.contains("modport input variable cannot be assigned directly"),
        "unexpected modport direction error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_packed_interface_vectors() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-vector");
    let source_path = root.join("interface_vector.sv");
    std::fs::write(
        &source_path,
        r#"
interface vector_if(input logic clk);
  logic [7:0] outgoing;
  logic [7:0] incoming = 8'h3c;
  modport producer(input clk, incoming, output outgoing);
endinterface

module vector_producer(
  vector_if.producer bus,
  input logic increment,
  output logic [7:0] captured
);
  assign bus.outgoing = bus.incoming + (increment ? 8'h10 : 8'h00);
  assign captured = bus.incoming;
endmodule

module vector_interface_top(
  input logic clk,
  input logic increment,
  output logic [7:0] outgoing,
  output logic [7:0] captured
);
  vector_if bus(.clk(clk));
  vector_producer producer_u(
    .bus(bus),
    .increment(increment),
    .captured(captured)
  );
  assign outgoing = bus.outgoing;
endmodule
"#,
    )
    .expect("write packed-interface-vector source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("vector_interface_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower packed interface vectors");
    let outgoing = design
        .net_decls
        .iter()
        .find(|declaration| declaration.name == "bus.outgoing")
        .expect("interface outgoing vector");
    assert_eq!(outgoing.width, 8);

    let mut simulator = Simulator::new(&design).expect("create interface-vector simulator");
    simulator
        .set_input("increment", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("bus.outgoing").unwrap().to_u64(), Some(0x4c));
    assert_eq!(simulator.read("captured").unwrap().to_u64(), Some(0x3c));
    assert_eq!(simulator.read("outgoing").unwrap().to_u64(), Some(0x4c));
}

#[test]
fn slang_ast_direct_lowering_supports_multidimensional_interface_vectors() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-vector-multi");
    let source_path = root.join("interface_vector_multi.sv");
    std::fs::write(
        &source_path,
        r#"
interface multi_vector_if(input logic clk);
  logic [3:0][7:0] outgoing;
  logic [3:0][7:0] incoming = 32'h12345678;
  modport producer(input clk, incoming, output outgoing);
endinterface

module multi_vector_producer(
  multi_vector_if.producer bus,
  input logic [1:0] index,
  output logic [7:0] captured
);
  assign bus.outgoing = bus.incoming;
  assign captured = bus.incoming[index];
endmodule

module multidim_interface_top(
  input logic clk,
  input logic [1:0] index,
  output logic [31:0] outgoing,
  output logic [7:0] captured
);
  multi_vector_if bus(.clk(clk));
  multi_vector_producer producer_u(
    .bus(bus),
    .index(index),
    .captured(captured)
  );
  assign outgoing = bus.outgoing;
endmodule
"#,
    )
    .expect("write multidimensional interface-vector source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("multidim_interface_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional interface vectors");
    let outgoing = design
        .net_decls
        .iter()
        .find(|declaration| declaration.name == "bus.outgoing")
        .expect("multidimensional interface vector");
    assert_eq!(outgoing.width, 32);

    let mut simulator =
        Simulator::new(&design).expect("create multidimensional interface simulator");
    simulator
        .set_input("index", &LogicWord::from_u64(2, 2))
        .unwrap();
    assert_eq!(
        simulator.read("bus.outgoing").unwrap().to_u64(),
        Some(0x12345678)
    );
    assert_eq!(simulator.read("captured").unwrap().to_u64(), Some(0x34));
    assert_eq!(
        simulator.read("outgoing").unwrap().to_u64(),
        Some(0x12345678)
    );
}

#[test]
fn slang_ast_direct_lowering_supports_unpacked_interface_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-member");
    let source_path = root.join("interface_array_member.sv");
    std::fs::write(
        &source_path,
        r#"
interface memory_if(input logic clk);
  logic [7:0] data[0:3];
  modport producer(output data);
endinterface

module array_producer(
  memory_if.producer bus,
  input logic [1:0] index,
  input logic [7:0] value
);
  always_comb begin
    bus.data[index] = value;
  end
endmodule

module interface_array_member_top(
  input logic clk,
  input logic [1:0] index,
  input logic [7:0] value,
  output logic [7:0] observed
);
  memory_if bus(.clk(clk));
  array_producer producer_u(
    .bus(bus),
    .index(index),
    .value(value)
  );
  assign observed = bus.data[index];
endmodule
"#,
    )
    .expect("write unpacked-interface-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_array_member_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower unpacked interface arrays");
    assert_eq!(design.hierarchy.len(), 3);
    assert!(design
        .memories
        .iter()
        .any(|memory| memory.name == "bus.data"));
    assert_eq!(design.memory_connections.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create interface-array simulator");
    for (index, value) in [(0_u64, 0xA5), (1, 0x3C), (2, 0x5A), (3, 0x96)] {
        simulator
            .set_input("index", &LogicWord::from_u64(2, index))
            .unwrap();
        simulator
            .set_input("value", &LogicWord::from_u64(8, value))
            .unwrap();
        assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(value));
    }

    let mut ast = serde_json::from_str::<serde_json::Value>(
        &slang
            .ast_json_with_options(
                std::slice::from_ref(&source_path),
                Some("interface_array_member_top"),
                &[],
                &PreprocessOptions::default(),
            )
            .expect("serialize authoritative interface-array AST"),
    )
    .expect("parse authoritative interface-array AST");
    fn make_memory_input(value: &mut serde_json::Value) {
        if let Some(fields) = value.as_object_mut() {
            if fields.get("kind").and_then(serde_json::Value::as_str) == Some("Modport")
                && fields.get("name").and_then(serde_json::Value::as_str) == Some("producer")
            {
                if let Some(members) = fields
                    .get_mut("members")
                    .and_then(serde_json::Value::as_array_mut)
                {
                    for member in members {
                        if member.get("name").and_then(serde_json::Value::as_str) == Some("data") {
                            member["direction"] = serde_json::Value::String("In".into());
                        }
                    }
                }
            }
            for nested in fields.values_mut() {
                make_memory_input(nested);
            }
        } else if let Some(values) = value.as_array_mut() {
            for nested in values {
                make_memory_input(nested);
            }
        }
    }
    make_memory_input(&mut ast);
    let error = slang
        .lower_ast_json(&ast.to_string())
        .expect_err("modport input memory writes must fail closed");
    assert!(
        error.contains("modport input variable cannot be assigned directly"),
        "unexpected interface-memory direction error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_multidimensional_unpacked_interface_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-member-2d");
    let source_path = root.join("interface_array_member_2d.sv");
    std::fs::write(
        &source_path,
        r#"
interface multi_memory_if(input logic clk);
  logic [7:0] data[0:1][0:3];
  modport producer(output data);
endinterface

module multidim_array_producer(
  multi_memory_if.producer bus,
  input logic row,
  input logic [1:0] column,
  input logic [7:0] value
);
  always_comb begin
    bus.data[row][column] = value;
  end
endmodule

module multidim_interface_array_top(
  input logic clk,
  input logic row,
  input logic [1:0] column,
  input logic [7:0] value,
  output logic [7:0] observed
);
  multi_memory_if bus(.clk(clk));
  multidim_array_producer producer_u(
    .bus(bus),
    .row(row),
    .column(column),
    .value(value)
  );
  assign observed = bus.data[row][column];
endmodule
"#,
    )
    .expect("write multidimensional unpacked-interface-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("multidim_interface_array_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional unpacked interface arrays");
    assert_eq!(design.hierarchy.len(), 3);
    let interface_memory = design
        .memories
        .iter()
        .find(|memory| memory.name == "bus.data")
        .expect("interface multidimensional memory");
    assert_eq!(interface_memory.dimensions.len(), 2);
    assert_eq!(interface_memory.flat_length, 8);
    assert_eq!(design.memory_connections.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create 2D interface-array simulator");
    for (row, column, value) in [
        (0_u64, 0_u64, 0xA5),
        (0, 3, 0x3C),
        (1, 1, 0x5A),
        (1, 2, 0x96),
    ] {
        simulator
            .set_input("row", &LogicWord::from_u64(1, row))
            .unwrap();
        simulator
            .set_input("column", &LogicWord::from_u64(2, column))
            .unwrap();
        simulator
            .set_input("value", &LogicWord::from_u64(8, value))
            .unwrap();
        assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(value));
    }
}

#[test]
fn slang_ast_direct_lowering_supports_interface_instance_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-instance-array");
    let source_path = root.join("interface_instance_array.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic valid;
  modport producer(output valid);
endinterface

module array_producer(
  bus_if.producer bus,
  input logic value
);
  assign bus.valid = value;
endmodule

module interface_instance_array_top(
  input logic clk,
  input logic [1:0] value,
  output logic [1:0] observed
);
  bus_if buses[2] (.clk(clk));
  array_producer producers[2] (
    .bus(buses),
    .value(value)
  );
  assign observed[0] = buses[0].valid;
  assign observed[1] = buses[1].valid;
endmodule
"#,
    )
    .expect("write interface-instance-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_instance_array_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower interface instance arrays");
    assert_eq!(design.hierarchy.len(), 5);
    assert_eq!(
        design
            .hierarchy
            .iter()
            .map(|instance| instance.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "interface_instance_array_top",
            "interface_instance_array_top.buses[0]",
            "interface_instance_array_top.buses[1]",
            "interface_instance_array_top.producers[0]",
            "interface_instance_array_top.producers[1]"
        ]
    );
    assert!(design
        .net_decls
        .iter()
        .any(|declaration| declaration.name == "buses[0].valid"));
    assert!(design
        .net_decls
        .iter()
        .any(|declaration| declaration.name == "buses[1].valid"));

    let mut simulator = Simulator::new(&design).expect("create interface-instance-array simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(2, 0b10))
        .unwrap();
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(0b01));
}

#[test]
fn slang_ast_direct_lowering_supports_multidimensional_interface_instance_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-instance-array-2d");
    let source_path = root.join("interface_instance_array_2d.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic valid;
  modport producer(output valid);
endinterface

module array_producer(bus_if.producer bus, input logic value);
  assign bus.valid = value;
endmodule

module interface_instance_array_2d_top(
  input logic clk,
  input logic [3:0] value,
  output logic [3:0] observed
);
  bus_if buses[2][2] (.clk(clk));
  array_producer p00 (.bus(buses[0][0]), .value(value[0]));
  array_producer p01 (.bus(buses[0][1]), .value(value[1]));
  array_producer p10 (.bus(buses[1][0]), .value(value[2]));
  array_producer p11 (.bus(buses[1][1]), .value(value[3]));
  assign observed[0] = buses[0][0].valid;
  assign observed[1] = buses[0][1].valid;
  assign observed[2] = buses[1][0].valid;
  assign observed[3] = buses[1][1].valid;
endmodule
"#,
    )
    .expect("write multidimensional interface-instance-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_instance_array_2d_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional interface instance arrays");
    assert_eq!(design.hierarchy.len(), 9);
    assert_eq!(
        design
            .hierarchy
            .iter()
            .map(|instance| instance.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "interface_instance_array_2d_top",
            "interface_instance_array_2d_top.buses[0][0]",
            "interface_instance_array_2d_top.buses[0][1]",
            "interface_instance_array_2d_top.buses[1][0]",
            "interface_instance_array_2d_top.buses[1][1]",
            "interface_instance_array_2d_top.p00",
            "interface_instance_array_2d_top.p01",
            "interface_instance_array_2d_top.p10",
            "interface_instance_array_2d_top.p11"
        ]
    );
    for path in [
        "buses[0][0].valid",
        "buses[0][1].valid",
        "buses[1][0].valid",
        "buses[1][1].valid",
    ] {
        assert!(
            design
                .net_decls
                .iter()
                .any(|declaration| declaration.name == path),
            "missing interface signal {path}"
        );
    }

    let mut simulator = Simulator::new(&design).expect("create 2D interface-array simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(4, 0b1010))
        .unwrap();
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(0b1010));
}

#[test]
fn slang_ast_direct_lowering_supports_interface_array_ports() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-port");
    let source_path = root.join("interface_array_port.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic valid;
  modport producer(output valid);
endinterface

module producer_array(
  bus_if.producer buses[2],
  input logic [1:0] value
);
  assign buses[0].valid = value[0];
  assign buses[1].valid = value[1];
endmodule

module interface_array_port_top(
  input logic clk,
  input logic [1:0] value,
  output logic [1:0] observed
);
  bus_if buses[2] (.clk(clk));
  producer_array producers (.buses(buses), .value(value));
  assign observed[0] = buses[0].valid;
  assign observed[1] = buses[1].valid;
endmodule
"#,
    )
    .expect("write interface-array-port source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_array_port_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower interface array ports");
    assert_eq!(design.hierarchy.len(), 4);
    assert_eq!(
        design
            .hierarchy
            .iter()
            .map(|instance| instance.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "interface_array_port_top",
            "interface_array_port_top.buses[0]",
            "interface_array_port_top.buses[1]",
            "interface_array_port_top.producers"
        ]
    );
    for path in ["buses[0].valid", "buses[1].valid"] {
        assert!(
            design
                .net_decls
                .iter()
                .any(|declaration| declaration.name == path),
            "missing interface signal {path}"
        );
    }

    let mut simulator = Simulator::new(&design).expect("create interface-array-port simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(2, 0b10))
        .unwrap();
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(0b10));
}

#[test]
fn slang_ast_direct_lowering_supports_multidimensional_interface_array_ports() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-port-2d");
    let source_path = root.join("interface_array_port_2d.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic valid;
  modport producer(output valid);
endinterface

module producer_array(
  bus_if.producer buses[2][2],
  input logic [3:0] value
);
  assign buses[0][0].valid = value[0];
  assign buses[0][1].valid = value[1];
  assign buses[1][0].valid = value[2];
  assign buses[1][1].valid = value[3];
endmodule

module interface_array_port_2d_top(
  input logic clk,
  input logic [3:0] value,
  output logic [3:0] observed
);
  bus_if buses[2][2] (.clk(clk));
  producer_array producers (.buses(buses), .value(value));
  assign observed[0] = buses[0][0].valid;
  assign observed[1] = buses[0][1].valid;
  assign observed[2] = buses[1][0].valid;
  assign observed[3] = buses[1][1].valid;
endmodule
"#,
    )
    .expect("write multidimensional interface-array-port source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_array_port_2d_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional interface array ports");
    assert_eq!(design.hierarchy.len(), 6);
    let mut simulator = Simulator::new(&design).expect("create 2D interface-array-port simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(4, 0b1010))
        .unwrap();
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(0b1010));
}

#[test]
fn slang_ast_direct_lowering_supports_interface_array_port_vectors() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-port-vector");
    let source_path = root.join("interface_array_port_vector.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic [3:0] data;
  modport producer(output data);
endinterface

module producer_array(
  bus_if.producer buses[2],
  input logic [7:0] value
);
  assign buses[0].data = value[3:0];
  assign buses[1].data = value[7:4];
endmodule

module interface_array_port_vector_top(
  input logic clk,
  input logic [7:0] value,
  output logic [7:0] observed
);
  bus_if buses[2] (.clk(clk));
  producer_array producers (.buses(buses), .value(value));
  assign observed[3:0] = buses[0].data;
  assign observed[7:4] = buses[1].data;
endmodule
"#,
    )
    .expect("write interface-array-port vector source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_array_port_vector_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower interface array port vectors");
    let mut simulator =
        Simulator::new(&design).expect("create interface-array-port vector simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(8, 0b1011_0110))
        .unwrap();
    assert_eq!(
        simulator.read("observed").unwrap().to_u64(),
        Some(0b1011_0110)
    );
}

#[test]
fn slang_ast_direct_lowering_shares_interface_array_ports_between_consumers() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-port-shared");
    let source_path = root.join("interface_array_port_shared.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic valid;
  logic ready;
  modport producer(output valid, input ready);
  modport consumer(input valid, output ready);
endinterface

module producer_array(
  bus_if.producer buses[2],
  input logic [1:0] drive,
  output logic [1:0] observed_ready
);
  assign buses[0].valid = drive[0];
  assign buses[1].valid = drive[1];
  assign observed_ready[0] = buses[0].ready;
  assign observed_ready[1] = buses[1].ready;
endmodule

module consumer_array(
  bus_if.consumer buses[2],
  input logic [1:0] enable,
  output logic [1:0] observed_valid
);
  assign buses[0].ready = enable[0];
  assign buses[1].ready = enable[1];
  assign observed_valid[0] = buses[0].valid;
  assign observed_valid[1] = buses[1].valid;
endmodule

module interface_array_port_shared_top(
  input logic clk,
  input logic [1:0] drive,
  input logic [1:0] enable,
  output logic [1:0] observed_valid,
  output logic [1:0] observed_ready
);
  bus_if buses[2] (.clk(clk));
  producer_array producers (.buses(buses), .drive(drive), .observed_ready(observed_ready));
  consumer_array consumers (.buses(buses), .enable(enable), .observed_valid(observed_valid));
endmodule
"#,
    )
    .expect("write shared interface-array-port source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_array_port_shared_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower shared interface array ports");
    let mut simulator = Simulator::new(&design).expect("create shared interface-array simulator");
    simulator
        .set_input("drive", &LogicWord::from_u64(2, 0b10))
        .unwrap();
    simulator
        .set_input("enable", &LogicWord::from_u64(2, 0b01))
        .unwrap();
    assert_eq!(
        simulator.read("observed_valid").unwrap().to_u64(),
        Some(0b10)
    );
    assert_eq!(
        simulator.read("observed_ready").unwrap().to_u64(),
        Some(0b01)
    );
}

#[test]
fn slang_ast_direct_lowering_rejects_interface_array_port_input_writes() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-port-input");
    let source_path = root.join("interface_array_port_input.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic valid;
  modport consumer(input valid);
endinterface

module consumer_array(
  bus_if.consumer buses[2],
  input logic [1:0] value
);
  assign buses[0].valid = value[0];
  assign buses[1].valid = value[1];
endmodule

module interface_array_port_input_top(
  input logic clk,
  input logic [1:0] value
);
  bus_if buses[2] (.clk(clk));
  consumer_array consumers (.buses(buses), .value(value));
endmodule
"#,
    )
    .expect("write interface-array-port input source");

    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_array_port_input_top"),
            &PreprocessOptions::default(),
        )
        .expect_err("interface array port input writes must fail closed");
    assert!(
        error.contains("cannot assign to input port 'valid'"),
        "unexpected interface-array port input error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_rejects_interface_array_port_shape_mismatches() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-port-shape");
    let source_path = root.join("interface_array_port_shape.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic valid;
  modport producer(output valid);
endinterface

module producer_array(
  bus_if.producer buses[3],
  input logic [2:0] value
);
  assign buses[0].valid = value[0];
  assign buses[1].valid = value[1];
  assign buses[2].valid = value[2];
endmodule

module interface_array_port_shape_top(
  input logic clk,
  input logic [2:0] value
);
  bus_if buses[2] (.clk(clk));
  producer_array producers (.buses(buses), .value(value));
endmodule
"#,
    )
    .expect("write interface-array-port shape source");

    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_array_port_shape_top"),
            &PreprocessOptions::default(),
        )
        .expect_err("interface array port shape mismatches must fail closed");
    assert!(
        error.contains("dimensions of connection for port 'buses' do not match"),
        "unexpected interface-array port shape error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_preserves_multi_packed_width() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-multi-packed");
    let source_path = root.join("multi_packed.sv");
    std::fs::write(
        &source_path,
        r#"
module multi_packed(
  input logic [3:0][7:0] a,
  output logic [31:0] y
);
  assign y = a;
endmodule
"#,
    )
    .expect("write multi-packed source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("multi_packed"),
            &PreprocessOptions::default(),
        )
        .expect("lower multi-packed declaration");
    assert_eq!(design.n_nets, 64);
    assert_eq!(
        design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "a")
            .expect("multi-packed input")
            .width,
        32
    );

    let mut simulator = Simulator::new(&design).expect("create multi-packed simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(32, 0x1234_5678))
        .expect("set multi-packed input");
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0x1234_5678));

    let selected_path = root.join("multi_packed_select.sv");
    std::fs::write(
        &selected_path,
        r#"
module multi_packed_select(
  input logic [3:0][7:0] a,
  input logic [1:0] selector,
  output logic [7:0] y,
  output logic [7:0] dynamic_y,
  output logic nested_bit
);
  assign y = a[3];
  assign dynamic_y = a[selector];
  assign nested_bit = a[3][7];
endmodule
"#,
    )
    .expect("write multi-packed select source");
    let selected = slang
        .compile_files_via_ast(
            std::slice::from_ref(&selected_path),
            Some("multi_packed_select"),
            &PreprocessOptions::default(),
        )
        .expect("lower constant, dynamic, and nested packed selects");

    let mut simulator = Simulator::new(&selected).expect("create packed-select simulator");
    for selector in 0..4_u64 {
        simulator
            .set_input("a", &LogicWord::from_u64(32, 0xB234_5678))
            .unwrap();
        simulator
            .set_input("selector", &LogicWord::from_u64(2, selector))
            .unwrap();
        assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0xB2));
        let expected = match selector {
            0 => 0x78,
            1 => 0x56,
            2 => 0x34,
            _ => 0xB2,
        };
        assert_eq!(
            simulator.read("dynamic_y").unwrap().to_u64(),
            Some(expected)
        );
        assert_eq!(simulator.read("nested_bit").unwrap().to_u64(), Some(1));
    }
}

#[test]
fn slang_ast_direct_lowering_supports_packed_struct_types() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-packed-struct");
    let source_path = root.join("packed_struct.sv");
    std::fs::write(
        &source_path,
        r#"
module packed_struct(
  input logic [3:0] high,
  input logic [3:0] low,
  output logic [7:0] whole,
  output logic [3:0] selected_low
);
  typedef struct packed {
    logic [3:0] high;
    logic [3:0] low;
  } pair_t;

  pair_t pair;
  assign pair.high = high;
  assign pair.low = low;
  assign whole = pair;
  assign selected_low = pair.low;
endmodule
"#,
    )
    .expect("write packed-struct source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("packed_struct"),
            &PreprocessOptions::default(),
        )
        .expect("lower packed struct");
    assert_eq!(design.assigns.len(), 4);

    let mut simulator = Simulator::new(&design).expect("create packed-struct simulator");
    simulator
        .set_input("high", &LogicWord::from_u64(4, 0xa))
        .unwrap();
    simulator
        .set_input("low", &LogicWord::from_u64(4, 0x5))
        .unwrap();
    assert_eq!(simulator.read("whole").unwrap().to_u64(), Some(0xa5));
    assert_eq!(simulator.read("selected_low").unwrap().to_u64(), Some(0x5));
}

#[test]
fn slang_ast_direct_lowering_supports_nested_packed_struct_types() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-nested-packed-struct");
    let source_path = root.join("nested_packed_struct.sv");
    std::fs::write(
        &source_path,
        r#"
module nested_packed_struct(
  output logic [7:0] whole,
  output logic [1:0] selected
);
  typedef struct packed {
    logic [1:0] a;
    logic [1:0] b;
  } inner_t;

  typedef struct packed {
    inner_t left;
    logic [3:0] right;
  } outer_t;

  outer_t value;
  assign value.left.a = 2'b10;
  assign value.left.b = 2'b01;
  assign value.right = 4'h3;
  assign whole = value;
  assign selected = value.left.a;
endmodule
"#,
    )
    .expect("write nested packed-struct source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("nested_packed_struct"),
            &PreprocessOptions::default(),
        )
        .expect("lower nested packed struct");
    assert_eq!(design.assigns.len(), 5);

    let mut simulator = Simulator::new(&design).expect("create nested packed-struct simulator");
    simulator.run_until(0).expect("settle nested packed struct");
    assert_eq!(simulator.read("whole").unwrap().to_u64(), Some(0x93));
    assert_eq!(simulator.read("selected").unwrap().to_u64(), Some(0b10));
}

#[test]
fn slang_ast_direct_lowering_supports_packed_union_types() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-packed-union");
    let source_path = root.join("packed_union.sv");
    std::fs::write(
        &source_path,
        r#"
module packed_union(
  input logic [7:0] byte_value,
  input logic [15:0] word_value,
  input logic select_word,
  output logic [15:0] word_view,
  output logic [7:0] byte_view,
  output logic [15:0] whole
);
  typedef union packed {
    logic [15:0] word;
    logic [15:0] raw;
  } value_t;

  value_t value;
  always_comb begin
    if (select_word) value.word = word_value;
    else value.raw = {8'h00, byte_value};
  end
  assign word_view = value.word;
  assign byte_view = value.raw[7:0];
  assign whole = value;
endmodule
"#,
    )
    .expect("write packed-union source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("packed_union"),
            &PreprocessOptions::default(),
        )
        .expect("lower packed union");
    assert_eq!(design.processes.len(), 1);
    assert_eq!(design.assigns.len(), 3);
    assert_eq!(
        design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "value")
            .expect("packed union declaration")
            .width,
        16
    );

    let mut simulator = Simulator::new(&design).expect("create packed-union simulator");
    simulator
        .set_input("byte_value", &LogicWord::from_u64(8, 0xa5))
        .unwrap();
    simulator
        .set_input("select_word", &LogicWord::from_u64(1, 0))
        .unwrap();
    assert_eq!(simulator.read("whole").unwrap().to_u64(), Some(0x00a5));
    assert_eq!(simulator.read("word_view").unwrap().to_u64(), Some(0x00a5));
    assert_eq!(simulator.read("byte_view").unwrap().to_u64(), Some(0xa5));

    simulator
        .set_input("word_value", &LogicWord::from_u64(16, 0x1234))
        .unwrap();
    simulator
        .set_input("select_word", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("whole").unwrap().to_u64(), Some(0x1234));
    assert_eq!(simulator.read("word_view").unwrap().to_u64(), Some(0x1234));
    assert_eq!(simulator.read("byte_view").unwrap().to_u64(), Some(0x34));
}

#[test]
fn slang_ast_direct_lowering_supports_packed_struct_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-packed-struct-array");
    let source_path = root.join("packed_struct_array.sv");
    std::fs::write(
        &source_path,
        r#"
module packed_struct_array(
  input logic [1:0] index,
  input logic [3:0] high,
  input logic [3:0] low,
  output logic [3:0] selected_high,
  output logic [3:0] selected_low,
  output logic [31:0] all_values
);
  typedef struct packed {
    logic [3:0] high;
    logic [3:0] low;
  } pair_t;

  pair_t pairs [0:3];
  always_comb begin
    pairs[0].high = 4'h1;
    pairs[0].low = 4'h2;
    pairs[1].high = 4'h3;
    pairs[1].low = 4'h4;
    pairs[2].high = 4'h5;
    pairs[2].low = 4'h6;
    pairs[3].high = high;
    pairs[3].low = low;
  end

  assign selected_high = pairs[index].high;
  assign selected_low = pairs[index].low;
  assign all_values = {pairs[3], pairs[2], pairs[1], pairs[0]};
endmodule
"#,
    )
    .expect("write packed struct array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("packed_struct_array"),
            &PreprocessOptions::default(),
        )
        .expect("lower packed struct array");
    assert_eq!(design.memories.len(), 1);
    assert_eq!(design.memories[0].element_width, 8);
    assert_eq!(design.processes.len(), 1);
    assert_eq!(design.assigns.len(), 3);

    let mut simulator = Simulator::new(&design).expect("create packed struct array simulator");
    simulator
        .set_input("high", &LogicWord::from_u64(4, 0xa))
        .unwrap();
    simulator
        .set_input("low", &LogicWord::from_u64(4, 0x5))
        .unwrap();
    assert_eq!(
        simulator.read("all_values").unwrap().to_u64(),
        Some(0xa556_3412)
    );
    for (index, selected_high, selected_low) in
        [(0, 0x1, 0x2), (1, 0x3, 0x4), (2, 0x5, 0x6), (3, 0xa, 0x5)]
    {
        simulator
            .set_input("index", &LogicWord::from_u64(2, index))
            .unwrap();
        assert_eq!(
            simulator.read("selected_high").unwrap().to_u64(),
            Some(selected_high)
        );
        assert_eq!(
            simulator.read("selected_low").unwrap().to_u64(),
            Some(selected_low)
        );
    }
}

#[test]
fn slang_ast_direct_lowering_supports_packed_union_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-packed-union-array");
    let source_path = root.join("packed_union_array.sv");
    std::fs::write(
        &source_path,
        r#"
module packed_union_array(
  input logic clk,
  input logic [1:0] index,
  input logic [15:0] word_value,
  input logic [15:0] raw_value,
  input logic use_word,
  output logic [15:0] word_view,
  output logic [15:0] raw_view,
  output logic [63:0] all_values
);
  typedef union packed {
    logic [15:0] word;
    logic [15:0] raw;
  } value_t;

  value_t values [0:3];
  initial begin
    values[0].raw = 16'h1111;
    values[1].raw = 16'h2222;
    values[2].raw = 16'h3333;
    values[3].raw = 16'h4444;
  end

  always @(posedge clk) begin
    if (use_word) values[index].word = word_value;
    else values[index].raw = raw_value;
  end

  assign word_view = values[index].word;
  assign raw_view = values[index].raw;
  assign all_values = {values[3], values[2], values[1], values[0]};
endmodule
"#,
    )
    .expect("write packed union array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("packed_union_array"),
            &PreprocessOptions::default(),
        )
        .expect("lower packed union array");
    assert_eq!(design.memories.len(), 1);
    assert_eq!(design.memories[0].element_width, 16);
    assert_eq!(design.processes.len(), 1);
    assert_eq!(design.assigns.len(), 3);

    let mut simulator = Simulator::new(&design).expect("create packed union array simulator");
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    assert_eq!(
        simulator.read("all_values").unwrap().to_u64(),
        Some(0x4444_3333_2222_1111)
    );

    for (index, word, raw) in [(0, 0x5555, 0x6666), (1, 0x7777, 0x8888)] {
        simulator
            .set_input("index", &LogicWord::from_u64(2, index))
            .unwrap();
        simulator
            .set_input("word_value", &LogicWord::from_u64(16, word))
            .unwrap();
        simulator
            .set_input("raw_value", &LogicWord::from_u64(16, raw))
            .unwrap();
        simulator
            .set_input("use_word", &LogicWord::from_u64(1, 1))
            .unwrap();
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(simulator.read("word_view").unwrap().to_u64(), Some(word));
        assert_eq!(simulator.read("raw_view").unwrap().to_u64(), Some(word));

        simulator
            .set_input("use_word", &LogicWord::from_u64(1, 0))
            .unwrap();
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(simulator.read("word_view").unwrap().to_u64(), Some(raw));
        assert_eq!(simulator.read("raw_view").unwrap().to_u64(), Some(raw));
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
    }
    assert_eq!(
        simulator.read("all_values").unwrap().to_u64(),
        Some(0x4444_3333_8888_6666)
    );
}

#[test]
fn slang_ast_direct_lowering_supports_packed_struct_assignment_patterns() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-struct-patterns");
    let source_path = root.join("struct_patterns.sv");
    std::fs::write(
        &source_path,
        r#"
module struct_patterns(
  input logic [3:0] a,
  input logic [3:0] b,
  output logic [7:0] positional,
  output logic [7:0] named
);
  typedef struct packed {
    logic [3:0] high;
    logic [3:0] low;
  } pair_t;

  pair_t positional_value;
  pair_t named_value;
  always_comb begin
    positional_value = '{a, b};
    named_value = '{high: b, low: a};
  end
  assign positional = positional_value;
  assign named = named_value;
endmodule
"#,
    )
    .expect("write packed struct assignment-pattern source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("struct_patterns"),
            &PreprocessOptions::default(),
        )
        .expect("lower packed struct assignment patterns");
    assert_eq!(design.processes.len(), 1);
    assert_eq!(design.assigns.len(), 2);

    let mut simulator = Simulator::new(&design).expect("create assignment-pattern simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(4, 0xa))
        .unwrap();
    simulator
        .set_input("b", &LogicWord::from_u64(4, 0x5))
        .unwrap();
    assert_eq!(simulator.read("positional").unwrap().to_u64(), Some(0xa5));
    assert_eq!(simulator.read("named").unwrap().to_u64(), Some(0x5a));
}

#[test]
fn slang_ast_direct_lowering_supports_enum_types() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-enum");
    let source_path = root.join("enum.sv");
    std::fs::write(
        &source_path,
        r#"
module enum_module(
  input logic [1:0] selector,
  output logic [1:0] raw,
  output logic is_b,
  output logic [1:0] cast_raw
);
  typedef enum logic [1:0] {
    A = 2'b00,
    B = 2'b01,
    C = 2'b10
  } state_t;

  state_t state;
  always_comb begin
    case (selector)
      A: state = A;
      B: state = B;
      default: state = C;
    endcase
  end
  assign raw = state;
  assign is_b = state == B;
  assign cast_raw = state_t'(2'b11);
endmodule
"#,
    )
    .expect("write enum source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("enum_module"),
            &PreprocessOptions::default(),
        )
        .expect("lower enum type");
    assert_eq!(design.processes.len(), 1);
    assert_eq!(design.assigns.len(), 3);
    assert_eq!(
        design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "state")
            .expect("enum state declaration")
            .width,
        2
    );

    let mut simulator = Simulator::new(&design).expect("create enum simulator");
    for (selector, expected_raw, expected_is_b) in
        [(0b00, 0b00, false), (0b01, 0b01, true), (0b10, 0b10, false)]
    {
        simulator
            .set_input("selector", &LogicWord::from_u64(2, selector))
            .unwrap();
        assert_eq!(simulator.read("raw").unwrap().to_u64(), Some(expected_raw));
        assert_eq!(
            simulator.read("is_b").unwrap().to_u64(),
            Some(expected_is_b as u64)
        );
    }
    assert_eq!(simulator.read("cast_raw").unwrap().to_u64(), Some(0b11));
}

#[test]
fn slang_ast_direct_lowering_supports_enum_next_and_prev() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-enum-next-prev");
    let source_path = root.join("enum_next_prev.sv");
    std::fs::write(
        &source_path,
        r#"
module enum_next_prev(
  input logic [1:0] selector,
  output logic [1:0] next_state,
  output logic [1:0] prev_state,
  output logic [1:0] next_by_two,
  output logic [1:0] constant_next,
  output logic [1:0] constant_prev,
  output logic [1:0] invalid_next,
  output logic [1:0] first_value,
  output logic [1:0] last_value,
  output int count
);
  typedef enum logic [1:0] {
    A = 2'b00,
    B = 2'b01,
    C = 2'b10
  } state_t;

  state_t state;
  always_comb state = state_t'(selector);
  assign next_state = state.next();
  assign prev_state = state.prev();
  assign next_by_two = state.next(2);
  assign constant_next = C.next();
  assign constant_prev = A.prev();
  assign invalid_next = state_t'(2'b11).next();
  assign first_value = state.first();
  assign last_value = state.last();
  assign count = state.num();
endmodule
"#,
    )
    .expect("write enum next/prev source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("enum_next_prev"),
            &PreprocessOptions::default(),
        )
        .expect("lower enum next/prev");
    assert_eq!(design.processes.len(), 1);
    assert_eq!(design.assigns.len(), 9);

    let mut simulator = Simulator::new(&design).expect("create enum next/prev simulator");
    for (selector, expected_next, expected_prev, expected_by_two) in [
        (0b00, 0b01, 0b10, 0b10),
        (0b01, 0b10, 0b00, 0b00),
        (0b10, 0b00, 0b01, 0b01),
    ] {
        simulator
            .set_input("selector", &LogicWord::from_u64(2, selector))
            .unwrap();
        assert_eq!(
            simulator.read("next_state").unwrap().to_u64(),
            Some(expected_next)
        );
        assert_eq!(
            simulator.read("prev_state").unwrap().to_u64(),
            Some(expected_prev)
        );
        assert_eq!(
            simulator.read("next_by_two").unwrap().to_u64(),
            Some(expected_by_two)
        );
    }

    simulator
        .set_input("selector", &LogicWord::from_u64(2, 0))
        .unwrap();
    assert_eq!(
        simulator.read("constant_next").unwrap().to_u64(),
        Some(0b00)
    );
    assert_eq!(
        simulator.read("constant_prev").unwrap().to_u64(),
        Some(0b10)
    );
    assert_eq!(simulator.read("first_value").unwrap().to_u64(), Some(0b00));
    assert_eq!(simulator.read("last_value").unwrap().to_u64(), Some(0b10));
    assert_eq!(simulator.read("count").unwrap().to_u64(), Some(3));

    simulator
        .set_input("selector", &LogicWord::from_u64(2, 0b11))
        .unwrap();
    assert_eq!(simulator.read("invalid_next").unwrap().to_hex_str(), "x");
}

#[test]
fn slang_ast_direct_lowering_supports_string_literals_and_enum_names() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-strings");
    let source_path = root.join("strings.sv");
    std::fs::write(
        &source_path,
        r#"
module strings(
  input logic [1:0] selector,
  input string left,
  input string right,
  input string mixed,
  input string same,
  input string case_variant,
  input string decimal_text,
  input string hex_text,
  input string binary_text,
  input int byte_index,
  input int substr_left,
  input int substr_right,
  output string literal_value,
  output string joined,
  output string literal_joined,
  output string state_name,
  output string constant_name,
  output string invalid_name,
  output int left_length,
  output int right_length,
  output byte constant_byte,
  output byte dynamic_byte,
  output byte method_byte,
  output byte out_of_range,
  output string upper_value,
  output string lower_value,
  output string put_value,
  output string substr_value,
  output string empty_substr,
  output string out_of_range_substr,
  output string decimal_string,
  output string hex_string,
  output string binary_string,
  output int decimal_value,
  output int hex_value,
  output int binary_value,
  output logic strings_equal,
  output logic strings_less,
  output int compare_result,
  output int case_compare_result,
  output int case_insensitive_compare_result
);
  typedef enum logic [1:0] {
    A = 2'b00,
    B = 2'b01,
    C = 2'b10
  } state_t;

  string literal_source = "aBcD";
  int conversion_source = -42;
  state_t state;
  always_comb begin
    state = state_t'(selector);
    constant_byte = right[1];
    dynamic_byte = right[byte_index];
    method_byte = right.getc(byte_index);
    out_of_range = right.getc(100);
  end
  initial begin
    put_value = literal_source;
    put_value.putc(1, 8'h5A);
    put_value.putc(64, 8'h21);
    put_value.putc(0, 8'h00);
    decimal_string.itoa(conversion_source);
    hex_string.hextoa(conversion_source);
    binary_string.bintoa(conversion_source);
  end
  assign literal_value = "READY";
  assign joined = {left, right};
  assign literal_joined = {"AB", "CD"};
  assign state_name = state.name();
  assign constant_name = B.name();
  assign invalid_name = state_t'(2'b11).name();
  assign left_length = left.len();
  assign right_length = right.len();
  assign upper_value = mixed.toupper();
  assign lower_value = mixed.tolower();
  assign strings_equal = mixed == same;
  assign strings_less = mixed < right;
  assign compare_result = mixed.compare(case_variant);
  assign case_compare_result = mixed.compare(case_variant);
  assign case_insensitive_compare_result = mixed.icompare(case_variant);
  assign substr_value = mixed.substr(substr_left, substr_right);
  assign empty_substr = mixed.substr(2, 1);
  assign out_of_range_substr = mixed.substr(0, 64);
  assign decimal_value = decimal_text.atoi();
  assign hex_value = hex_text.atohex();
  assign binary_value = binary_text.atobin();
endmodule
"#,
    )
    .expect("write string and enum-name source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("strings"),
            &PreprocessOptions::default(),
        )
        .expect("lower strings and enum names");
    assert_eq!(design.processes.len(), 1);
    assert_eq!(design.assigns.len(), 23);
    assert_eq!(
        design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "literal_value")
            .expect("string output declaration")
            .width,
        512
    );

    let mut simulator = Simulator::new(&design).expect("create string simulator");
    for (selector, name) in [(0, "A"), (1, "B"), (2, "C")] {
        simulator
            .set_input("selector", &LogicWord::from_u64(2, selector))
            .unwrap();
        simulator
            .set_input(
                "left",
                &LogicWord::from_bytes_le(512, selector.to_string().as_bytes()),
            )
            .unwrap();
        simulator
            .set_input("right", &LogicWord::from_bytes_le(512, b"-STATE"))
            .unwrap();
        simulator
            .set_input("mixed", &LogicWord::from_bytes_le(512, b"aBcD"))
            .unwrap();
        simulator
            .set_input("same", &LogicWord::from_bytes_le(512, b"aBcD"))
            .unwrap();
        simulator
            .set_input("case_variant", &LogicWord::from_bytes_le(512, b"AbCd"))
            .unwrap();
        simulator
            .set_input(
                "decimal_text",
                &LogicWord::from_bytes_le(512, b"  -042junk"),
            )
            .unwrap();
        simulator
            .set_input("hex_text", &LogicWord::from_bytes_le(512, b" -2fUVM"))
            .unwrap();
        simulator
            .set_input("binary_text", &LogicWord::from_bytes_le(512, b" +0101uvm"))
            .unwrap();
        simulator
            .set_input("byte_index", &LogicWord::from_u64(32, selector))
            .unwrap();
        simulator
            .set_input("substr_left", &LogicWord::from_u64(32, 1))
            .unwrap();
        simulator
            .set_input("substr_right", &LogicWord::from_u64(32, 2))
            .unwrap();
        assert_eq!(
            simulator
                .read("literal_value")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "READY"
        );
        assert_eq!(
            simulator
                .read("state_name")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            name
        );
        assert_eq!(
            simulator
                .read("constant_name")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "B"
        );
        assert_eq!(
            simulator
                .read("invalid_name")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            ""
        );
        let joined = format!("{selector}-STATE");
        assert_eq!(
            simulator
                .read("joined")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            joined
        );
        assert_eq!(
            simulator
                .read("literal_joined")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "ABCD"
        );
        assert_eq!(
            simulator.read("left_length").unwrap().to_u64(),
            Some(selector.to_string().len() as u64)
        );
        assert_eq!(simulator.read("right_length").unwrap().to_u64(), Some(6));
        assert_eq!(
            simulator.read("constant_byte").unwrap().to_u64(),
            Some(b'S' as u64)
        );
        let expected_byte = b"-STATE"[selector as usize] as u64;
        assert_eq!(
            simulator.read("dynamic_byte").unwrap().to_u64(),
            Some(expected_byte)
        );
        assert_eq!(
            simulator.read("method_byte").unwrap().to_u64(),
            Some(expected_byte)
        );
        assert_eq!(simulator.read("out_of_range").unwrap().to_u64(), Some(0));
        assert_eq!(
            simulator
                .read("upper_value")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "ABCD"
        );
        assert_eq!(
            simulator
                .read("lower_value")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "abcd"
        );
        assert_eq!(simulator.read("strings_equal").unwrap().to_u64(), Some(1));
        assert_eq!(simulator.read("strings_less").unwrap().to_u64(), Some(0));
        assert_eq!(simulator.read("compare_result").unwrap().to_u64(), Some(1));
        assert_eq!(
            simulator.read("case_compare_result").unwrap().to_u64(),
            Some(1)
        );
        assert_eq!(
            simulator
                .read("case_insensitive_compare_result")
                .unwrap()
                .to_u64(),
            Some(0)
        );
        assert_eq!(
            simulator
                .read("put_value")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "aZcD"
        );
        assert_eq!(
            simulator
                .read("substr_value")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "Bc"
        );
        assert_eq!(
            simulator
                .read("empty_substr")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            ""
        );
        assert_eq!(
            simulator
                .read("out_of_range_substr")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            ""
        );
        assert_eq!(
            simulator
                .read("decimal_string")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "-42"
        );
        assert_eq!(
            simulator
                .read("hex_string")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "ffffffd6"
        );
        assert_eq!(
            simulator
                .read("binary_string")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "11111111111111111111111111010110"
        );
        assert_eq!(
            simulator.read("decimal_value").unwrap().to_u64(),
            Some((-42_i32) as u32 as u64)
        );
        assert_eq!(
            simulator.read("hex_value").unwrap().to_u64(),
            Some((-47_i32) as u32 as u64)
        );
        assert_eq!(simulator.read("binary_value").unwrap().to_u64(), Some(5));
    }

    let bounded_source_path = root.join("strings-bounded.sv");
    std::fs::write(
        &bounded_source_path,
        r#"
module strings_bounded(
  output string put_value,
  output string substr_value
);
  string literal_source = "aBcD";
  initial begin
    substr_value = literal_source.substr(1, 2);
    literal_source.putc(1, 8'h5A);
    literal_source.putc(64, 8'h21);
    literal_source.putc(0, 8'h00);
    put_value = literal_source;
  end
endmodule
"#,
    )
    .expect("write bounded string source");
    let bounded = slang
        .compile_files(
            std::slice::from_ref(&bounded_source_path),
            Some("strings_bounded"),
            &PreprocessOptions::default(),
        )
        .expect("lower strings through the bounded compatibility path");
    let mut bounded_simulator = Simulator::new(&bounded).expect("create bounded string simulator");
    bounded_simulator.run_until(0).expect("run bounded strings");
    assert_eq!(
        bounded_simulator
            .read("put_value")
            .unwrap()
            .to_ascii_string_nul_terminated(),
        "aZcD"
    );
    assert_eq!(
        bounded_simulator
            .read("substr_value")
            .unwrap()
            .to_ascii_string_nul_terminated(),
        "Bc"
    );
}

#[test]
fn slang_ast_direct_lowering_preserves_event_sensitivity_lists() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-events");
    let source_path = root.join("events.sv");
    std::fs::write(
        &source_path,
        r#"
module events(
  input logic a,
  input logic b,
  input logic c,
  input logic clk,
  input logic rst,
  input logic [3:0] vector,
  output logic level_y,
  output logic edge_q,
  output logic bit_q
);
  always @(a or b) level_y = a & c;
  always @(posedge clk or posedge rst) begin
    if (rst) begin
      edge_q <= 1'b0;
    end else begin
      edge_q <= ~edge_q;
    end
  end
  always @(posedge vector[0]) bit_q <= vector[1];
endmodule
"#,
    )
    .expect("write event-list source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("events"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower event sensitivity lists");
    assert_eq!(design.processes.len(), 3);
    assert!(matches!(
        &design.processes[0].sens,
        eda_core::ir::Sens::List(list)
            if matches!(list.as_slice(), [eda_core::ir::Sens::Any(_), eda_core::ir::Sens::Any(_)])
    ));
    assert!(matches!(
        &design.processes[1].sens,
        eda_core::ir::Sens::List(list)
            if matches!(list.as_slice(), [eda_core::ir::Sens::Posedge(_), eda_core::ir::Sens::Posedge(_)])
    ));

    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("events"),
            &PreprocessOptions::default(),
        )
        .expect("lower event sensitivity lists through the bounded path");
    assert_eq!(reference.processes.len(), design.processes.len());

    let run_events = |design: &eda_core::ir::Design| {
        let mut simulator = Simulator::new(design).expect("create event-list simulator");
        simulator
            .set_input("a", &LogicWord::from_u64(1, 1))
            .unwrap();
        simulator.set_input("c", &LogicWord::zero(1)).unwrap();
        simulator.set_input("b", &LogicWord::zero(1)).unwrap();
        assert_eq!(simulator.read("level_y").unwrap().to_u64(), Some(0));

        simulator
            .set_input("c", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(simulator.read("level_y").unwrap().to_u64(), Some(0));

        simulator
            .set_input("b", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(simulator.read("level_y").unwrap().to_u64(), Some(1));

        simulator
            .set_input("rst", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(simulator.read("edge_q").unwrap().to_u64(), Some(0));
        simulator.set_input("rst", &LogicWord::zero(1)).unwrap();
        for expected in [1, 0, 1] {
            simulator
                .set_input("clk", &LogicWord::from_u64(1, 1))
                .unwrap();
            assert_eq!(simulator.read("edge_q").unwrap().to_u64(), Some(expected));
            simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
        }

        simulator
            .set_input("vector", &LogicWord::from_u64(4, 0b0010))
            .unwrap();
        simulator
            .set_input("vector", &LogicWord::from_u64(4, 0b0011))
            .unwrap();
        assert_eq!(simulator.read("bit_q").unwrap().to_u64(), Some(1));
        simulator
            .set_input("vector", &LogicWord::from_u64(4, 0b0010))
            .unwrap();
        simulator
            .set_input("vector", &LogicWord::from_u64(4, 0b0011))
            .unwrap();
        assert_eq!(simulator.read("bit_q").unwrap().to_u64(), Some(1));
    };
    run_events(&design);
    run_events(&reference);
}

#[test]
fn slang_ast_direct_lowering_supports_named_event_triggers() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-events");
    let source_path = root.join("events.sv");
    std::fs::write(
        &source_path,
        r#"
module events(input logic go, output logic [3:0] count, output logic done);
  event e;
  initial begin
    count = 4'd0;
    @(e);
    count = count + 1;
    @(e);
    count = count + 1;
    done = 1'b1;
  end

  initial begin
    wait (go) ->e;
    #1 ->e;
  end
endmodule
"#,
    )
    .expect("write named-event source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("events"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower named events");
    let mut simulator = Simulator::new(&design).expect("create named-event simulator");
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("count").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("done").unwrap().to_u64(), None);
    simulator.run_until(1).unwrap();
    assert_eq!(simulator.read("count").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("done").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_event_list_controls() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-event-list");
    let source_path = root.join("event_list.sv");
    std::fs::write(
        &source_path,
        r#"
module event_list(input logic go, output logic y);
  event first;
  event second;
  initial begin
    y = 1'b0;
    @(first or second);
    y = 1'b1;
  end

  initial begin
    wait (go);
    ->second;
  end
endmodule
"#,
    )
    .expect("write event-list source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("event_list"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower event-list control");
    let mut simulator = Simulator::new(&design).expect("create event-list simulator");
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_edge_qualified_event_lists() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-edge-event-list");
    let source_path = root.join("edge_event_list.sv");
    std::fs::write(
        &source_path,
        r#"
module edge_event_list(
  input logic clk,
  input logic go,
  input logic reset,
  output logic rose
);
  initial begin
    rose = 1'b0;
    wait (go);
    @(posedge clk or negedge reset);
    rose = clk;
    @(posedge clk);
    rose = 1'b1;
  end
endmodule
"#,
    )
    .expect("write edge-qualified event-list source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("edge_event_list"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower edge-qualified event list");
    let mut simulator = Simulator::new(&design).expect("create edge-event simulator");
    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    simulator
        .set_input("reset", &LogicWord::from_u64(1, 1))
        .unwrap();
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("rose").unwrap().to_u64(), Some(0));
    simulator.set_input("reset", &LogicWord::zero(1)).unwrap();
    assert_eq!(simulator.read("rose").unwrap().to_u64(), Some(0));
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("rose").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_event_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-event-array");
    let source_path = root.join("event_array.sv");
    std::fs::write(
        &source_path,
        r#"
module event_array(
  input logic go,
  input int index,
  input int wrong,
  output logic first,
  output logic second,
  output logic triggered_now,
  output logic triggered_later
);
  event events[2];
  initial begin
    first = 1'b0;
    second = 1'b0;
    wait (go);
    fork
      begin
        @(events[index]);
        #2 first = 1'b1;
        @(events[index]);
        second = 1'b1;
      end
      begin
        #1 ->events[wrong];
        #1 ->events[index];
        #3 ->events[index];
      end
    join
  end
  initial begin
    triggered_now = 1'b0;
    triggered_later = 1'b0;
    wait (go);
    ->events[index];
    triggered_now = events[index].triggered;
    #1 triggered_later = events[index].triggered;
  end
endmodule
"#,
    )
    .expect("write event-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("event_array"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower event array");
    assert_eq!(
        design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "events")
            .expect("event-array declaration")
            .width,
        2
    );
    let mut simulator = Simulator::new(&design).expect("create event-array simulator");
    simulator
        .set_input("index", &LogicWord::from_u64(32, 0))
        .unwrap();
    simulator
        .set_input("wrong", &LogicWord::from_u64(32, 1))
        .unwrap();
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    simulator.run_until(10).unwrap();
    assert_eq!(simulator.read("triggered_now").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("triggered_later").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("second").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_rejects_runtime_out_of_range_event_waits() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-event-array-range");
    let source_path = root.join("event_array_range.sv");
    std::fs::write(
        &source_path,
        r#"
module event_array_range(input logic go, input int index);
  event events[2];
  initial begin
    wait (go);
    @(events[index]);
  end
endmodule
"#,
    )
    .expect("write out-of-range event-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("event_array_range"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower event-array range source");
    let mut simulator = Simulator::new(&design).expect("create event-array range simulator");
    simulator
        .set_input("index", &LogicWord::from_u64(32, 2))
        .unwrap();
    let error = simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .expect_err("runtime out-of-range event waits must fail closed");
    assert!(
        error.contains("event array index is out of range"),
        "unexpected error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_constant_nonblocking_event_array_triggers() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-event-array-constant");
    let source_path = root.join("event_array_constant.sv");
    std::fs::write(
        &source_path,
        r#"
module event_array_constant(input logic go, output logic done);
  event events[2];
  initial begin
    done = 1'b0;
    wait (go);
    fork
      @(events[1]) done = 1'b1;
      #1 ->>events[1];
    join
  end
endmodule
"#,
    )
    .expect("write constant event-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("event_array_constant"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower constant event-array trigger");
    let mut simulator = Simulator::new(&design).expect("create constant event simulator");
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    simulator.run_until(2).unwrap();
    assert_eq!(simulator.read("done").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_multidimensional_event_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-event-array-2d");
    let source_path = root.join("event_array_2d.sv");
    std::fs::write(
        &source_path,
        r#"
module event_array_2d(
  input logic go,
  input int row,
  input int column,
  output logic done
);
  event events[1:0][1:0];
  initial begin
    done = 1'b0;
    wait (go);
    fork
      @(events[row][column]) done = 1'b1;
      begin
        #1 ->events[1 - row][column];
        #1 ->events[row][column];
      end
    join
  end
endmodule
"#,
    )
    .expect("write multidimensional event-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("event_array_2d"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional event array");
    assert_eq!(
        design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "events")
            .expect("multidimensional event-array declaration")
            .width,
        4
    );
    let mut simulator = Simulator::new(&design).expect("create multidimensional event simulator");
    simulator
        .set_input("row", &LogicWord::from_u64(32, 1))
        .unwrap();
    simulator
        .set_input("column", &LogicWord::from_u64(32, 0))
        .unwrap();
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    simulator.run_until(3).unwrap();
    assert_eq!(simulator.read("done").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_clocking_block_events() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-clocking-event");
    let source_path = root.join("clocking_event.sv");
    std::fs::write(
        &source_path,
        r#"
module clocking_event(
  input logic clk,
  input logic go,
  input logic sampled_input,
  output logic first_sample,
  output logic second_sample
);
  clocking cb @(posedge clk);
    input sampled_input;
  endclocking

  initial begin
    first_sample = 1'b0;
    second_sample = 1'b0;
    wait (go);
    @(cb);
    first_sample = cb.sampled_input;
    @(cb);
    second_sample = cb.sampled_input;
  end
endmodule
"#,
    )
    .expect("write clocking-event source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("clocking_event"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower clocking-block event");
    let mut simulator = Simulator::new(&design).expect("create clocking simulator");
    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    simulator
        .set_input("sampled_input", &LogicWord::zero(1))
        .unwrap();
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("first_sample").unwrap().to_u64(), Some(0));
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("first_sample").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("second_sample").unwrap().to_u64(), Some(0));
    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    simulator
        .set_input("sampled_input", &LogicWord::from_u64(1, 1))
        .unwrap();
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("second_sample").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_rejects_unsupported_clocking_semantics() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-clocking-unsupported");

    let skew_path = root.join("clocking_skew.sv");
    std::fs::write(
        &skew_path,
        r#"
module clocking_skew(input logic clk, input logic a);
  clocking cb @(posedge clk);
    input #1step a;
  endclocking
endmodule
"#,
    )
    .expect("write clocking-skew source");
    let skew_error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&skew_path),
            Some("clocking_skew"),
            &PreprocessOptions::default(),
        )
        .expect_err("clocking skew must fail closed");
    assert!(
        skew_error.contains("skew is not supported"),
        "unexpected skew error: {skew_error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_clocking_output_drives() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-clocking-drive");
    let source_path = root.join("clocking_drive.sv");
    std::fs::write(
        &source_path,
        r#"
module clocking_drive(
  input logic clk,
  input logic a,
  output logic b
);
  clocking cb @(posedge clk);
    input a;
    output b;
  endclocking

  initial begin
    b = 1'b0;
    @(cb);
    cb.b <= cb.a;
  end
endmodule
"#,
    )
    .expect("write clocking-drive source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("clocking_drive"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower clocking output drive");
    let mut simulator = Simulator::new(&design).expect("create clocking-drive simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .unwrap();
    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    assert_eq!(simulator.read("b").unwrap().to_u64(), Some(0));
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("b").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_clocking_cycle_delays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-clocking-cycle-delay");
    let source_path = root.join("cycle_delay.sv");
    std::fs::write(
        &source_path,
        r#"
module cycle_delay(
  input logic clk,
  input logic go,
  output logic zero_cycle,
  output logic first,
  output logic second
);
  default clocking cb @(posedge clk);
  endclocking

  initial begin
    zero_cycle = 1'b0;
    first = 1'b0;
    second = 1'b0;
    wait (go);
    ##0 zero_cycle = 1'b1;
    ##1 first = 1'b1;
    ##2 second = 1'b1;
  end
endmodule
"#,
    )
    .expect("write clocking cycle-delay source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("cycle_delay"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower clocking cycle delays");
    let mut simulator = Simulator::new(&design).expect("create cycle-delay simulator");
    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("zero_cycle").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("second").unwrap().to_u64(), Some(0));

    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("second").unwrap().to_u64(), Some(0));

    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("second").unwrap().to_u64(), Some(0));

    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("second").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_preserves_clocking_blocks_across_hierarchy() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-clocking-hierarchy");
    let source_path = root.join("clocking_hierarchy.sv");
    std::fs::write(
        &source_path,
        r#"
module clocking_child(
  input logic clk,
  input logic sampled_input,
  output logic driven_output
);
  default clocking cb @(posedge clk);
    input sampled_input;
    output driven_output;
  endclocking

  initial begin
    driven_output = 1'b0;
    @(cb);
    cb.driven_output <= cb.sampled_input;
    ##1 cb.driven_output <= 1'b0;
  end
endmodule

module clocking_hierarchy(
  input logic clk,
  input logic sampled_input,
  output logic driven_output
);
  clocking_child child(
    .clk(clk),
    .sampled_input(sampled_input),
    .driven_output(driven_output)
  );
endmodule
"#,
    )
    .expect("write hierarchical clocking source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("clocking_hierarchy"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower hierarchical clocking block");
    assert_eq!(design.clocking_blocks.len(), 1);
    let mut simulator = Simulator::new(&design).expect("create hierarchical clocking simulator");
    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    simulator
        .set_input("sampled_input", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("driven_output").unwrap().to_u64(), Some(0));

    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("driven_output").unwrap().to_u64(), Some(1));

    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("driven_output").unwrap().to_u64(), Some(0));
}

#[test]
fn slang_ast_direct_lowering_supports_nonblocking_event_triggers() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-nonblocking-events");
    let source_path = root.join("nonblocking_events.sv");
    std::fs::write(
        &source_path,
        r#"
module nonblocking_events(
  input logic go,
  output logic ready,
  output logic observed_ready,
  output logic waiter_done
);
  event e;
  initial begin
    ready = 1'b0;
    observed_ready = 1'b0;
    waiter_done = 1'b0;
    @(e);
    observed_ready = ready;
    waiter_done = 1'b1;
  end

  initial begin
    wait (go);
    ready = 1'b1;
    ->>e;
  end
endmodule
"#,
    )
    .expect("write nonblocking event source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("nonblocking_events"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower nonblocking event trigger");
    let mut simulator = Simulator::new(&design).expect("create nonblocking event simulator");
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("ready").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("observed_ready").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("waiter_done").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_force_and_release() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-force");
    let source_path = root.join("force.sv");
    std::fs::write(
        &source_path,
        r#"
module force_release(
  input logic a,
  input logic en,
  output logic y,
  output logic released
);
  assign y = a;
  initial begin
    released = 1'b0;
    wait (en);
    force y = 1'b0;
    #2;
    release y;
    released = 1'b1;
  end
endmodule
"#,
    )
    .expect("write force/release source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("force_release"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower force/release");
    let mut simulator = Simulator::new(&design).expect("create force/release simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));

    simulator
        .set_input("en", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0));

    simulator.run_until(1).unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0));
    simulator.run_until(2).unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("released").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_wait_order() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-wait-order");
    let ordered_path = root.join("ordered.sv");
    std::fs::write(
        &ordered_path,
        r#"
module ordered(input logic go, output logic done);
  event first;
  event second;
  initial begin
    done = 1'b0;
    wait_order(first, second);
    done = 1'b1;
  end

  initial begin
    wait (go);
    ->first;
    #1;
    ->second;
  end
endmodule
"#,
    )
    .expect("write ordered wait_order source");
    let ordered_design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&ordered_path),
            Some("ordered"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower ordered wait_order");
    let mut ordered_simulator =
        Simulator::new(&ordered_design).expect("create ordered wait_order simulator");
    ordered_simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(ordered_simulator.read("done").unwrap().to_u64(), Some(0));
    ordered_simulator.run_until(1).unwrap();
    assert_eq!(ordered_simulator.read("done").unwrap().to_u64(), Some(1));

    let out_of_order_path = root.join("out-of-order.sv");
    std::fs::write(
        &out_of_order_path,
        r#"
module out_of_order(input logic go, output int status);
  event first;
  event second;
  initial begin
    status = 0;
    wait_order(first, second) status = 1; else status = 2;
  end

  initial begin
    wait (go);
    ->second;
  end
endmodule
"#,
    )
    .expect("write out-of-order wait_order source");
    let out_of_order_design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&out_of_order_path),
            Some("out_of_order"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower out-of-order wait_order");
    let mut out_of_order_simulator =
        Simulator::new(&out_of_order_design).expect("create out-of-order simulator");
    out_of_order_simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(
        out_of_order_simulator.read("status").unwrap().to_u64(),
        Some(2)
    );
}

#[test]
fn slang_ast_direct_lowering_supports_event_triggered() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-event-triggered");
    let source_path = root.join("event_triggered.sv");
    std::fs::write(
        &source_path,
        r#"
module event_triggered(
  input logic go,
  output logic was_triggered,
  output logic later_not_triggered
);
  event e;
  initial begin
    was_triggered = 1'b0;
    later_not_triggered = 1'b0;
    @(e);
    was_triggered = e.triggered;
    #1;
    later_not_triggered = !e.triggered;
  end

  initial begin
    wait (go);
    ->e;
  end
endmodule
"#,
    )
    .expect("write event-triggered source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("event_triggered"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower event-triggered check");
    let mut simulator = Simulator::new(&design).expect("create event-triggered simulator");
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("was_triggered").unwrap().to_u64(), Some(1));
    assert_eq!(
        simulator.read("later_not_triggered").unwrap().to_u64(),
        Some(0)
    );
    simulator.run_until(1).unwrap();
    assert_eq!(
        simulator.read("later_not_triggered").unwrap().to_u64(),
        Some(1)
    );
}

#[test]
fn slang_ast_direct_lowering_supports_final_blocks() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-final-blocks");
    let source_path = root.join("final_blocks.sv");
    std::fs::write(
        &source_path,
        r#"
module final_blocks(
  input logic go,
  output logic y,
  output logic finalized
);
  initial begin
    y = 1'b0;
    finalized = 1'b0;
    wait (go);
    #1 y = 1'b1;
    $finish;
  end

  final begin
    finalized = 1'b1;
  end
endmodule
"#,
    )
    .expect("write final-block source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("final_blocks"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower final blocks");
    assert_eq!(design.final_blocks.len(), 1);
    let mut simulator = Simulator::new(&design).expect("create final-block simulator");
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("finalized").unwrap().to_u64(), Some(0));
    simulator.run_until(1).unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert!(simulator.finished());
    assert_eq!(simulator.read("finalized").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_constant_delay_controls() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-delays");
    let source_path = root.join("delays.sv");
    std::fs::write(
        &source_path,
        r#"
module delays(
  input logic a,
  input logic clk,
  output logic y,
  output logic q,
  output logic clock,
  output logic delayed_q
);
  initial begin
    q = 1'b0;
    clock = 1'b0;
    delayed_q = 1'b0;
  end
  initial begin
    #5 y = a;
    #3 y = ~a;
  end
  always #5 q = ~q;
  initial forever #5 clock = ~clock;
  always @(posedge clk) #2 delayed_q = ~delayed_q;
endmodule
"#,
    )
    .expect("write delay-control source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("delays"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower constant delay controls");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("delays"),
            &PreprocessOptions::default(),
        )
        .expect("lower constant delay controls through the bounded path");
    assert_eq!(design.initials.len(), 3);
    assert_eq!(design.processes.len(), 2);
    assert_eq!(reference.initials.len(), design.initials.len());
    assert_eq!(reference.processes.len(), design.processes.len());

    let run_delays = |design: &eda_core::ir::Design| {
        let mut simulator = Simulator::new(design).expect("create delay simulator");
        simulator
            .set_input("a", &LogicWord::from_u64(1, 1))
            .unwrap();
        simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
        assert_eq!(simulator.read("y").unwrap().to_u64(), None);
        assert_eq!(simulator.read("q").unwrap().to_u64(), Some(0));
        assert_eq!(simulator.read("clock").unwrap().to_u64(), Some(0));
        assert_eq!(simulator.read("delayed_q").unwrap().to_u64(), Some(0));
        simulator.run_until(4).unwrap();
        assert_eq!(simulator.read("y").unwrap().to_u64(), None);
        simulator.run_until(5).unwrap();
        assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
        assert_eq!(simulator.read("q").unwrap().to_u64(), Some(1));
        assert_eq!(simulator.read("clock").unwrap().to_u64(), Some(1));
        simulator.run_until(8).unwrap();
        assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0));
        assert_eq!(simulator.read("q").unwrap().to_u64(), Some(1));
        assert_eq!(simulator.read("clock").unwrap().to_u64(), Some(1));
        simulator.run_until(10).unwrap();
        assert_eq!(simulator.read("q").unwrap().to_u64(), Some(0));
        assert_eq!(simulator.read("clock").unwrap().to_u64(), Some(0));
        simulator.run_until(15).unwrap();
        assert_eq!(simulator.read("q").unwrap().to_u64(), Some(1));
        assert_eq!(simulator.read("clock").unwrap().to_u64(), Some(1));
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        simulator.run_until(17).unwrap();
        assert_eq!(simulator.read("delayed_q").unwrap().to_u64(), Some(1));
        simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        simulator.run_until(19).unwrap();
        assert_eq!(simulator.read("delayed_q").unwrap().to_u64(), Some(0));
    };
    run_delays(&design);
    run_delays(&reference);

    let min_typ_max_path = root.join("min-typ-max.sv");
    std::fs::write(
        &min_typ_max_path,
        r#"
module min_typ_max(input logic a, output logic y);
  initial #(1:2:3) y = a;
endmodule
"#,
    )
    .expect("write min:typ:max delay source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&min_typ_max_path),
            Some("min_typ_max"),
            &PreprocessOptions::default(),
        )
        .expect_err("min:typ:max delays must fail closed");
    assert!(
        error.contains("min:typ:max delay controls are not supported"),
        "unexpected error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_wait_statements() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-wait");
    let source_path = root.join("wait.sv");
    std::fs::write(
        &source_path,
        r#"
module waits(
  input logic a,
  input logic b,
  output logic y,
  output logic z,
  output logic invert
);
  initial begin
    wait (a && b) y = 1'b1;
    wait (a) z = b;
  end
  initial wait (!a) invert = 1'b1;
endmodule
"#,
    )
    .expect("write wait source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("waits"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower wait statements");
    assert_eq!(design.initials.len(), 2);

    let mut simulator = Simulator::new(&design).expect("create wait simulator");
    simulator
        .set_input("b", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), None);
    assert_eq!(simulator.read("z").unwrap().to_u64(), None);
    assert_eq!(simulator.read("invert").unwrap().to_u64(), None);

    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("z").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("invert").unwrap().to_u64(), None);

    simulator.run_until(10).unwrap();
    assert_eq!(simulator.read("invert").unwrap().to_u64(), None);
    simulator.set_input("a", &LogicWord::zero(1)).unwrap();
    assert_eq!(simulator.read("invert").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("z").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_runtime_repeat_loops() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-repeat");
    let source_path = root.join("repeat.sv");
    std::fs::write(
        &source_path,
        r#"
module repeats(
  input logic a,
  input int dynamic_count,
  output logic [3:0] y,
  output logic [3:0] immediate,
  output logic zero,
  output logic [3:0] dynamic
);
  initial begin
    y = 4'd0;
    repeat (4) begin
      #1 y = y + 1;
    end
  end

  initial begin
    immediate = 4'd0;
    repeat (2) immediate = immediate + 1;

    zero = 1'b0;
    repeat (0) zero = 1'b1;
  end

  initial begin
    #1 dynamic = 4'd0;
    repeat (dynamic_count) dynamic = dynamic + 1;
  end
endmodule
"#,
    )
    .expect("write repeat-loop source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("repeats"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower repeat loops");

    let run_repeat = |design: &eda_core::ir::Design| {
        let mut simulator = Simulator::new(design).expect("create repeat simulator");
        simulator
            .set_input("a", &LogicWord::from_u64(1, 1))
            .unwrap();
        simulator
            .set_input("dynamic_count", &LogicWord::from_u64(32, 3))
            .unwrap();
        assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0));
        assert_eq!(simulator.read("immediate").unwrap().to_u64(), Some(2));
        assert_eq!(simulator.read("zero").unwrap().to_u64(), Some(0));
        for expected in 1..=4 {
            simulator.run_until(expected).unwrap();
            assert_eq!(simulator.read("y").unwrap().to_u64(), Some(expected));
        }
        assert_eq!(simulator.read("dynamic").unwrap().to_u64(), Some(3));
    };
    // The bounded compatibility frontend still rejects module-level repeat
    // loops; this regression intentionally covers the authoritative direct AST
    // path until the legacy elaborator catches up.
    run_repeat(&design);

    let excessive_path = root.join("excessive.sv");
    std::fs::write(
        &excessive_path,
        r#"
module excessive(output logic y);
  initial repeat (70000) y = 1'b1;
endmodule
"#,
    )
    .expect("write excessive repeat source");
    let excessive = slang
        .compile_files_via_ast(
            std::slice::from_ref(&excessive_path),
            Some("excessive"),
            &PreprocessOptions::default(),
        )
        .expect("lower excessive repeat loop");
    let mut excessive_simulator = Simulator::new(&excessive).expect("create excessive simulator");
    let error = excessive_simulator
        .run_until(0)
        .expect_err("excessive repeat loops must fail closed");
    assert!(
        error.contains("exceeds the 65536-iteration execution limit"),
        "unexpected error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_runtime_while_and_for_loops() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-runtime-loops");
    let source_path = root.join("loops.sv");
    std::fs::write(
        &source_path,
        r#"
module loops(
  input logic a,
  output logic [3:0] for_value,
  output logic [3:0] increment_value,
  output logic [3:0] while_value
);
  int i;
  int j;
  int k;
  initial begin
    for_value = 4'd0;
    for (i = 0; i < 4; i = i + 1) begin
      for_value[i] = 1'b1;
    end

    increment_value = 4'd0;
    for (j = 0; j < 3; j++) begin
      increment_value = increment_value + 1;
    end

    while_value = 4'd0;
    k = 0;
    while (k < 3) begin
      #1 while_value = while_value + 1;
      k++;
    end
  end
endmodule
"#,
    )
    .expect("write runtime loop source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("loops"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower runtime while and for loops");
    let mut simulator = Simulator::new(&design).expect("create runtime loop simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("for_value").unwrap().to_u64(), Some(0b1111));
    assert_eq!(simulator.read("increment_value").unwrap().to_u64(), Some(3));
    assert_eq!(simulator.read("while_value").unwrap().to_u64(), Some(0));
    for expected in 1..=3 {
        simulator.run_until(expected).unwrap();
        assert_eq!(
            simulator.read("while_value").unwrap().to_u64(),
            Some(expected)
        );
    }

    let inline_local_path = root.join("inline-local.sv");
    std::fs::write(
        &inline_local_path,
        r#"
module inline_local(output logic [3:0] y, output logic [3:0] z);
  initial begin
    y = 4'd0;
    z = 4'd0;
    for (int i = 0; i < 4; i++) y[i] = 1'b1;
    for (int i = 0; i < 2; i++) z[i] = 1'b1;
  end
endmodule
"#,
    )
    .expect("write inline local loop source");
    let inline_local = slang
        .compile_files_via_ast(
            std::slice::from_ref(&inline_local_path),
            Some("inline_local"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower procedure-local loop variables");
    let mut inline_simulator =
        Simulator::new(&inline_local).expect("create procedure-local loop simulator");
    inline_simulator.run_until(0).unwrap();
    assert_eq!(inline_simulator.read("y").unwrap().to_u64(), Some(0b1111));
    assert_eq!(inline_simulator.read("z").unwrap().to_u64(), Some(0b0011));

    let excessive_path = root.join("excessive.sv");
    std::fs::write(
        &excessive_path,
        r#"
module excessive(output logic y);
  int i;
  initial begin
    i = 0;
    while (i < 70000) i = i + 1;
    y = 1'b1;
  end
endmodule
"#,
    )
    .expect("write excessive while source");
    let excessive = slang
        .compile_files_via_ast(
            std::slice::from_ref(&excessive_path),
            Some("excessive"),
            &PreprocessOptions::default(),
        )
        .expect("lower excessive while loop");
    let mut excessive_simulator = Simulator::new(&excessive).expect("create excessive simulator");
    let error = excessive_simulator
        .run_until(0)
        .expect_err("excessive while loop must fail closed");
    assert!(
        error.contains("exceeds the 65536-iteration execution limit"),
        "unexpected error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_lexical_named_block_disable() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-named-disable");
    let source_path = root.join("named_disable.sv");
    std::fs::write(
        &source_path,
        r#"
module named_disable(
  input logic go,
  output logic reached_disable,
  output logic after_inner,
  output logic after_outer
);
  initial begin : outer
    reached_disable = 1'b0;
    after_inner = 1'b0;
    after_outer = 1'b0;
    wait (go);
    begin : inner
      reached_disable = 1'b1;
      disable outer;
      after_inner = 1'b1;
    end
    after_outer = 1'b1;
  end
endmodule
"#,
    )
    .expect("write named-disable source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("named_disable"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower named-block disable");
    let mut simulator = Simulator::new(&design).expect("create named-disable simulator");
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("reached_disable").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("after_inner").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("after_outer").unwrap().to_u64(), Some(0));

    let inner_path = root.join("disable_inner.sv");
    std::fs::write(
        &inner_path,
        r#"
module disable_inner(output logic skipped, output logic continued);
  initial begin : outer
    skipped = 1'b0;
    continued = 1'b0;
    begin : inner
      disable inner;
      skipped = 1'b1;
    end
    continued = 1'b1;
  end
endmodule
"#,
    )
    .expect("write inner-disable source");
    let inner = slang
        .compile_files_via_ast(
            std::slice::from_ref(&inner_path),
            Some("disable_inner"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower inner named-block disable");
    let mut inner_simulator = Simulator::new(&inner).expect("create inner-disable simulator");
    inner_simulator.run_until(0).unwrap();
    assert_eq!(inner_simulator.read("skipped").unwrap().to_u64(), Some(0));
    assert_eq!(inner_simulator.read("continued").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_named_block_disable_from_loops() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-named-disable-loop");
    let source_path = root.join("named_disable_loop.sv");
    std::fs::write(
        &source_path,
        r#"
module named_disable_loop(
  output logic [3:0] count,
  output logic unreachable
);
  int index;
  initial begin : outer
    count = 4'd0;
    unreachable = 1'b0;
    index = 0;
    while (index < 5) begin
      count = count + 1;
      index = index + 1;
      if (index == 2) begin
        disable outer;
      end
    end
    unreachable = 1'b1;
  end
endmodule
"#,
    )
    .expect("write named-disable loop source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("named_disable_loop"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower named-block loop disable");
    let mut simulator = Simulator::new(&design).expect("create named-loop simulator");
    simulator.run_until(0).unwrap();
    assert_eq!(simulator.read("count").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("unreachable").unwrap().to_u64(), Some(0));
}

#[test]
fn slang_ast_direct_lowering_rejects_forks_inside_named_blocks() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-named-disable-fork");
    let source_path = root.join("named_disable_fork.sv");
    std::fs::write(
        &source_path,
        r#"
module named_disable_fork(output logic x, output logic y);
  initial begin : outer
    fork
      x = 1'b1;
      begin : cancellable
        disable outer;
        y = 1'b1;
      end
    join
  end
endmodule
"#,
    )
    .expect("write named-block fork source");

    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("named_disable_fork"),
            &PreprocessOptions::default(),
        )
        .expect_err("named-block process cancellation must fail closed");
    assert!(
        error.contains("named blocks containing fork are not supported"),
        "unexpected error: {error}"
    );

    let safe_path = root.join("named_fork_no_disable.sv");
    std::fs::write(
        &safe_path,
        r#"
module named_fork_no_disable(output logic x, output logic y);
  initial begin : outer
    fork
      x = 1'b1;
      y = 1'b1;
    join
  end
endmodule
"#,
    )
    .expect("write named-block fork without disable source");
    let safe = slang
        .compile_files_via_ast(
            std::slice::from_ref(&safe_path),
            Some("named_fork_no_disable"),
            &PreprocessOptions::default(),
        )
        .expect("named blocks without disable still support fork");
    let mut safe_simulator = Simulator::new(&safe).expect("create named-block fork simulator");
    safe_simulator.run_until(0).unwrap();
    assert_eq!(safe_simulator.read("x").unwrap().to_u64(), Some(1));
    assert_eq!(safe_simulator.read("y").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_runtime_do_while_loops() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-do-while");
    let source_path = root.join("do_while.sv");
    std::fs::write(
        &source_path,
        r#"
module do_while(
  output logic [3:0] count,
  output logic execute_once
);
  int i;
  initial begin
    i = 0;
    count = 4'd0;
    do begin
      #1 count = count + 1;
      i = i + 1;
    end while (i < 3);

    i = 100;
    do begin
      execute_once = 1'b1;
    end while (i < 0);
  end
endmodule
"#,
    )
    .expect("write do-while source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("do_while"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower do-while loops");
    let mut simulator = Simulator::new(&design).expect("create do-while simulator");
    simulator.run_until(0).unwrap();
    assert_eq!(simulator.read("count").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("execute_once").unwrap().to_u64(), None);
    for expected in 1..=3 {
        simulator.run_until(expected).unwrap();
        assert_eq!(simulator.read("count").unwrap().to_u64(), Some(expected));
    }
    assert_eq!(simulator.read("execute_once").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_runtime_break_and_continue() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-loop-jumps");
    let source_path = root.join("jumps.sv");
    std::fs::write(
        &source_path,
        r#"
module jumps(
  output logic [3:0] break_index,
  output logic [3:0] continue_count,
  output logic [3:0] repeat_break_count,
  output logic [3:0] repeat_continue_count,
  output logic [3:0] repeat_late_count,
  output logic [3:0] after_loops
);
  int i;
  initial begin
    break_index = 4'd0;
    for (i = 0; i < 10; i++) begin
      if (i == 3) begin
        break;
      end
      break_index = i[3:0] + 1;
    end

    continue_count = 4'd0;
    for (i = 0; i < 8; i++) begin
      if (i < 4) begin
        continue;
      end
      continue_count = continue_count + 1;
    end

    repeat_break_count = 4'd0;
    repeat (10) begin
      repeat_break_count = repeat_break_count + 1;
      if (repeat_break_count == 3) begin
        break;
      end
    end

    repeat_continue_count = 4'd0;
    repeat_late_count = 4'd0;
    repeat (6) begin
      repeat_continue_count = repeat_continue_count + 1;
      if (repeat_continue_count < 3) begin
        continue;
      end
      repeat_late_count = repeat_late_count + 1;
    end

    after_loops = 4'd1;
  end
endmodule
"#,
    )
    .expect("write loop-jump source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("jumps"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower break and continue");
    let mut simulator = Simulator::new(&design).expect("create loop-jump simulator");
    simulator.run_until(0).unwrap();
    assert_eq!(simulator.read("break_index").unwrap().to_u64(), Some(3));
    assert_eq!(simulator.read("continue_count").unwrap().to_u64(), Some(4));
    assert_eq!(
        simulator.read("repeat_break_count").unwrap().to_u64(),
        Some(3)
    );
    assert_eq!(
        simulator.read("repeat_continue_count").unwrap().to_u64(),
        Some(6)
    );
    assert_eq!(
        simulator.read("repeat_late_count").unwrap().to_u64(),
        Some(4)
    );
    assert_eq!(simulator.read("after_loops").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_replication() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-replication");
    let source_path = root.join("replication.sv");
    std::fs::write(
        &source_path,
        r#"
module replication(input logic [3:0] a, output logic [15:0] y);
  assign y = {4{a}};
endmodule
"#,
    )
    .expect("write replication source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("replication"),
            &PreprocessOptions::default(),
        )
        .expect("lower replication");
    assert_eq!(design.assigns.len(), 1);
    assert_eq!(design.assigns[0].e.width(), 16);

    let mut simulator = Simulator::new(&design).expect("create replication simulator");
    for value in [0x3_u64, 0x5, 0xa, 0xf] {
        simulator
            .set_input("a", &LogicWord::from_u64(4, value))
            .unwrap();
        let expected = 0x1111_u64 * value;
        assert_eq!(simulator.read("y").unwrap().to_u64(), Some(expected));
    }
}

#[test]
fn slang_ast_direct_lowering_supports_dynamic_packed_select() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-dynamic-packed");
    let source_path = root.join("dynamic_packed.sv");
    std::fs::write(
        &source_path,
        r#"
module dynamic_packed(
  input logic [7:0] a,
  input logic [2:0] selector,
  output logic y
);
  assign y = a[selector];
endmodule
"#,
    )
    .expect("write dynamic packed-select source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("dynamic_packed"),
            &PreprocessOptions::default(),
        )
        .expect("lower dynamic packed select");
    assert_eq!(design.assigns.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create dynamic packed simulator");
    for (selector, expected) in [(0_u64, 1_u64), (1, 0), (6, 0), (7, 1)] {
        simulator
            .set_input("a", &LogicWord::from_u64(8, 0x81))
            .unwrap();
        simulator
            .set_input("selector", &LogicWord::from_u64(3, selector))
            .unwrap();
        assert_eq!(simulator.read("y").unwrap().to_u64(), Some(expected));
    }
}

#[test]
fn slang_ast_direct_lowering_supports_dynamic_packed_lvalues() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-dynamic-packed-lvalue");
    let source_path = root.join("dynamic_lvalue.sv");
    std::fs::write(
        &source_path,
        r#"
module dynamic_lvalue(
  input logic clk,
  input logic [2:0] selector,
  input logic value,
  output logic [7:0] q
);
  always_ff @(posedge clk) q[selector] <= value;
endmodule
"#,
    )
    .expect("write dynamic packed-lvalue source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("dynamic_lvalue"),
            &PreprocessOptions::default(),
        )
        .expect("lower dynamic packed lvalue");
    assert_eq!(design.initials.len(), 0);
    assert_eq!(design.processes.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create dynamic lvalue simulator");
    let mut expected = 0_u64;
    for selector in 0..8_u64 {
        let value = selector % 2;
        expected |= value << selector;
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        simulator
            .set_input("selector", &LogicWord::from_u64(3, selector))
            .unwrap();
        simulator
            .set_input("value", &LogicWord::from_u64(1, value))
            .unwrap();
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
    }
    assert_eq!(simulator.read("q").unwrap().to_u64(), Some(expected));

    let packed_path = root.join("dynamic_packed_lvalue.sv");
    std::fs::write(
        &packed_path,
        r#"
module dynamic_packed_lvalue(
  input logic clk,
  input logic [1:0] selector,
  input logic [7:0] value,
  output logic [3:0][7:0] q
);
  always_ff @(posedge clk) q[selector] <= value;
endmodule
"#,
    )
    .expect("write dynamic multi-packed lvalue source");
    let packed = slang
        .compile_files_via_ast(
            std::slice::from_ref(&packed_path),
            Some("dynamic_packed_lvalue"),
            &PreprocessOptions::default(),
        )
        .expect("lower dynamic multi-packed lvalue");
    let mut packed_simulator = Simulator::new(&packed).expect("create packed lvalue simulator");
    let mut bytes = [0_u64; 4];
    for selector in 0..4_u64 {
        let value = 0x10 + selector;
        bytes[selector as usize] = value;
        packed_simulator
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        packed_simulator
            .set_input("selector", &LogicWord::from_u64(2, selector))
            .unwrap();
        packed_simulator
            .set_input("value", &LogicWord::from_u64(8, value))
            .unwrap();
        packed_simulator
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
    }
    let expected = bytes
        .iter()
        .rev()
        .fold(0_u64, |acc, byte| (acc << 8) | byte);
    assert_eq!(packed_simulator.read("q").unwrap().to_u64(), Some(expected));
}

#[test]
fn slang_ast_direct_lowering_supports_arithmetic_shifts() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-arithmetic-shift");
    let source_path = root.join("arithmetic_shift.sv");
    std::fs::write(
        &source_path,
        r#"
module arithmetic_shift(
  input logic signed [7:0] a,
  input logic [2:0] n,
  output logic signed [7:0] left,
  output logic signed [7:0] right
);
  assign left = a <<< n;
  assign right = a >>> n;
endmodule
"#,
    )
    .expect("write arithmetic-shift source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("arithmetic_shift"),
            &PreprocessOptions::default(),
        )
        .expect("lower arithmetic shifts");
    assert_eq!(design.assigns.len(), 2);
    assert!(design
        .net_decls
        .iter()
        .any(|declaration| declaration.name == "a" && declaration.is_signed));

    let mut simulator = Simulator::new(&design).expect("create shift simulator");
    for (shift, expected_left, expected_right) in [
        (0_u64, 0xf8_u64, 0xf8_u64),
        (1, 0xf0, 0xfc),
        (2, 0xe0, 0xfe),
        (7, 0x00, 0xff),
    ] {
        simulator
            .set_input("a", &LogicWord::from_u64(8, 0xf8))
            .unwrap();
        simulator
            .set_input("n", &LogicWord::from_u64(3, shift))
            .unwrap();
        assert_eq!(
            simulator.read("left").unwrap().to_u64(),
            Some(expected_left),
            "shift left {shift}"
        );
        assert_eq!(
            simulator.read("right").unwrap().to_u64(),
            Some(expected_right),
            "shift right {shift}"
        );
    }
}

#[test]
fn slang_ast_direct_lowering_supports_power_operator() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-power");
    let source_path = root.join("power.sv");
    std::fs::write(
        &source_path,
        r#"
module power(input logic [7:0] base, input logic [7:0] exponent, output logic [7:0] y);
  assign y = base ** exponent;
endmodule
"#,
    )
    .expect("write power source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("power"),
            &PreprocessOptions::default(),
        )
        .expect("lower power operator");
    assert_eq!(design.assigns.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create power simulator");
    for (base, exponent, expected) in [
        (2_u64, 3_u64, 8_u64),
        (3, 4, 81),
        (17, 2, 33),
        (5, 0, 1),
        (0, 0, 1),
    ] {
        simulator
            .set_input("base", &LogicWord::from_u64(8, base))
            .unwrap();
        simulator
            .set_input("exponent", &LogicWord::from_u64(8, exponent))
            .unwrap();
        assert_eq!(
            simulator.read("y").unwrap().to_u64(),
            Some(expected),
            "{base} ** {exponent}"
        );
    }
}

#[test]
fn slang_ast_direct_lowering_supports_streaming_concatenation() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-streaming");
    let source_path = root.join("streaming.sv");
    std::fs::write(
        &source_path,
        r#"
module streaming(
  input logic [7:0] a,
  input logic [3:0] b,
  output logic [7:0] reverse_bits,
  output logic [7:0] forward_bits,
  output logic [7:0] reverse_nibbles,
  output logic [7:0] reverse_pairs,
  output logic [11:0] multi_bits,
  output logic [11:0] multi_nibbles
);
  assign reverse_bits = {<<{a}};
  assign forward_bits = {>>{a}};
  assign reverse_nibbles = {<<4{a}};
  assign reverse_pairs = {<<2{a}};
  assign multi_bits = {<<{a, b}};
  assign multi_nibbles = {<<4{a, b}};
endmodule
"#,
    )
    .expect("write streaming-concatenation source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("streaming"),
            &PreprocessOptions::default(),
        )
        .expect("lower streaming concatenation");
    assert_eq!(design.assigns.len(), 6);

    let mut simulator = Simulator::new(&design).expect("create streaming simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(8, 0xb2))
        .unwrap();
    simulator
        .set_input("b", &LogicWord::from_u64(4, 0x2))
        .unwrap();
    assert_eq!(simulator.read("reverse_bits").unwrap().to_u64(), Some(0x4d));
    assert_eq!(simulator.read("forward_bits").unwrap().to_u64(), Some(0xb2));
    assert_eq!(
        simulator.read("reverse_nibbles").unwrap().to_u64(),
        Some(0x2b)
    );
    assert_eq!(
        simulator.read("reverse_pairs").unwrap().to_u64(),
        Some(0x8e)
    );
    // The stream operands form one bitstream in source order before the
    // streaming operator reorders it. Thus `{a, b}` is 12'b1011_0010_0010.
    assert_eq!(simulator.read("multi_bits").unwrap().to_u64(), Some(0x44d));
    assert_eq!(
        simulator.read("multi_nibbles").unwrap().to_u64(),
        Some(0x22b)
    );
}

#[test]
fn slang_ast_direct_lowering_preserves_ascending_indexed_packed_selects() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-ascending-indexed");
    let source_path = root.join("ascending_indexed.sv");
    std::fs::write(
        &source_path,
        r#"
module ascending_indexed(
  input logic [0:7] a,
  input logic [2:0] start,
  output logic [3:0] up,
  output logic [3:0] down
);
  assign up = a[start +: 4];
  assign down = a[start -: 4];
endmodule
"#,
    )
    .expect("write ascending indexed-select source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("ascending_indexed"),
            &PreprocessOptions::default(),
        )
        .expect("lower ascending indexed selects");
    assert_eq!(design.assigns.len(), 2);

    let mut simulator = Simulator::new(&design).expect("create ascending simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(8, 0xa5))
        .unwrap();
    simulator
        .set_input("start", &LogicWord::from_u64(3, 0))
        .unwrap();
    assert_eq!(simulator.read("up").unwrap().to_u64(), Some(0xa));
    assert_eq!(simulator.read("down").unwrap().to_hex_str(), "x");

    simulator
        .set_input("start", &LogicWord::from_u64(3, 4))
        .unwrap();
    assert_eq!(simulator.read("up").unwrap().to_u64(), Some(0x5));
    assert_eq!(simulator.read("down").unwrap().to_u64(), Some(0x4));
}

#[test]
fn slang_ast_direct_lowering_supports_integral_conversions() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-conversions");
    let source_path = root.join("conversions.sv");
    std::fs::write(
        &source_path,
        r#"
module conversions(
  input byte signed source,
  input logic [15:0] wide,
  output logic [15:0] widen,
  output logic [3:0] truncate,
  output logic [15:0] unsigned_same,
  output logic signed [15:0] signed_widen,
  output logic signed_lt_zero,
  output logic unsigned_lt_zero
);
  assign widen = 16'(source);
  assign truncate = 4'(wide);
  assign unsigned_same = unsigned'(source);
  assign signed_widen = signed'(16'(source));
  assign signed_lt_zero = signed_widen < 16'sd0;
  assign unsigned_lt_zero = widen < 16'd0;
endmodule
"#,
    )
    .expect("write integral-conversion source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("conversions"),
            &PreprocessOptions::default(),
        )
        .expect("lower integral conversions");
    let signed_output = design
        .net_decls
        .iter()
        .find(|declaration| declaration.name == "signed_widen")
        .expect("signed conversion output");
    assert!(signed_output.is_signed);

    let mut simulator = Simulator::new(&design).expect("create conversion simulator");
    simulator
        .set_input("source", &LogicWord::from_u64(8, 0xfb))
        .unwrap();
    simulator
        .set_input("wide", &LogicWord::from_u64(16, 0xabcd))
        .unwrap();

    assert_eq!(simulator.read("widen").unwrap().to_u64(), Some(0xfffb));
    assert_eq!(simulator.read("truncate").unwrap().to_u64(), Some(0xd));
    assert_eq!(
        simulator.read("unsigned_same").unwrap().to_u64(),
        Some(0x00fb)
    );
    assert_eq!(simulator.read("signed_widen").unwrap().to_hex_str(), "fffb");
    assert_eq!(simulator.read("signed_lt_zero").unwrap().to_u64(), Some(1));
    assert_eq!(
        simulator.read("unsigned_lt_zero").unwrap().to_u64(),
        Some(0)
    );
}

#[test]
fn slang_ast_direct_lowering_supports_straight_line_function_calls() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-functions");
    let source_path = root.join("functions.sv");
    std::fs::write(
        &source_path,
        r#"
module functions(
  input byte signed a,
  input int b,
  output int sum,
  output int constant,
  output int nested,
  output int initial_sum,
  output int selected,
  output int assigned,
  output int loop_sum,
  output int while_sum,
  output int repeat_sum
);
  function automatic int add(int x, int y);
    int result;
    begin
      result = x + y;
      return result;
    end
  endfunction

  function automatic int twice(int x);
    return add(x, x);
  endfunction

  function automatic int pick(int x, int y);
    if (x < y) return x;
    else return y;
  endfunction

  function automatic int cond_assign(int x, int y);
    int result = x;
    if (x < y) result = y;
    return result;
  endfunction

  function automatic int sum_four();
    int result = 0;
    for (int i = 0; i < 4; i = i + 1) result = result + i;
    return result;
  endfunction

  function automatic int while_four();
    int result = 0;
    int i = 0;
    while (i < 4) begin
      result = result + i;
      i = i + 1;
    end
    return result;
  endfunction

  function automatic int repeat_four();
    int result = 0;
    repeat (4) result = result + 1;
    return result;
  endfunction

  assign sum = add(a, b);
  assign constant = add(2, 3);
  assign nested = twice(b);
  initial initial_sum = add(7, 8);
  assign selected = pick(a, b);
  assign assigned = cond_assign(a, b);
  assign loop_sum = sum_four();
  assign while_sum = while_four();
  assign repeat_sum = repeat_four();
endmodule
"#,
    )
    .expect("write function-call source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("functions"),
            &PreprocessOptions::default(),
        )
        .expect("lower function calls");
    assert_eq!(design.assigns.len(), 8);
    assert_eq!(design.initials.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create function simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(8, 0xff))
        .unwrap();
    simulator
        .set_input("b", &LogicWord::from_u64(32, 4))
        .unwrap();
    assert_eq!(simulator.read("sum").unwrap().to_u64(), Some(3));
    assert_eq!(simulator.read("constant").unwrap().to_u64(), Some(5));
    assert_eq!(simulator.read("nested").unwrap().to_u64(), Some(8));
    assert_eq!(simulator.read("initial_sum").unwrap().to_u64(), Some(15));
    assert_eq!(
        simulator.read("selected").unwrap().to_u64(),
        Some(0xffff_ffff)
    );
    assert_eq!(simulator.read("assigned").unwrap().to_u64(), Some(4));
    assert_eq!(simulator.read("loop_sum").unwrap().to_u64(), Some(6));
    assert_eq!(simulator.read("while_sum").unwrap().to_u64(), Some(6));
    assert_eq!(simulator.read("repeat_sum").unwrap().to_u64(), Some(4));

    simulator
        .set_input("a", &LogicWord::from_u64(8, 5))
        .unwrap();
    assert_eq!(simulator.read("selected").unwrap().to_u64(), Some(4));
    assert_eq!(simulator.read("assigned").unwrap().to_u64(), Some(5));
}

#[test]
fn slang_ast_direct_lowering_rejects_recursive_function_calls() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-recursive-function");
    let source_path = root.join("recursive_function.sv");
    std::fs::write(
        &source_path,
        r#"
module recursive_function(output int y);
  function automatic int loop(int x);
    return loop(x);
  endfunction

  assign y = loop(0);
endmodule
"#,
    )
    .expect("write recursive-function source");

    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("recursive_function"),
            &PreprocessOptions::default(),
        )
        .expect_err("unbounded recursive function inlining must be rejected");
    assert!(error.contains("recursive function"), "{error}");
}

#[test]
fn slang_ast_direct_lowering_supports_tasks_with_output_arguments() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-task-output");
    let source_path = root.join("task_output.sv");
    std::fs::write(
        &source_path,
        r#"
module task_output(
  input int a,
  output int y,
  output int z
);
  task automatic add_one(input int x, output int result);
    result = x + 1;
  endtask

  always_comb begin
    add_one(a, y);
    add_one(41, z);
  end
endmodule
"#,
    )
    .expect("write task-output source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("task_output"),
            &PreprocessOptions::default(),
        )
        .expect("lower task with output argument");
    assert_eq!(design.processes.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create task simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(32, 7))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(8));
    assert_eq!(simulator.read("z").unwrap().to_u64(), Some(42));
}

#[test]
fn slang_ast_direct_lowering_supports_task_inout_and_ref_arguments() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-task-inout-ref");
    let source_path = root.join("task_inout_ref.sv");
    std::fs::write(
        &source_path,
        r#"
module task_inout_ref(
  output int inout_result,
  output int ref_result
);
  task automatic add_inout(inout int value, input int delta);
    value = value + delta;
  endtask

  task automatic add_ref(ref int value, input int delta);
    value = value + delta;
  endtask

  int inout_state = 10;
  int ref_state = 20;

  initial begin
    add_inout(inout_state, 5);
    add_ref(ref_state, 7);
  end

  assign inout_result = inout_state;
  assign ref_result = ref_state;
endmodule
"#,
    )
    .expect("write task inout/ref source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("task_inout_ref"),
            &PreprocessOptions::default(),
        )
        .expect("lower task inout/ref arguments");
    assert_eq!(design.initials.len(), 1);
    assert_eq!(design.assigns.len(), 4);

    let mut simulator = Simulator::new(&design).expect("create task inout/ref simulator");
    simulator.run_until(0).expect("settle task inout/ref calls");
    assert_eq!(simulator.read("inout_result").unwrap().to_u64(), Some(15));
    assert_eq!(simulator.read("ref_result").unwrap().to_u64(), Some(27));
}

#[test]
fn slang_ast_direct_lowering_supports_task_local_variables() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-task-locals");
    let source_path = root.join("task_locals.sv");
    std::fs::write(
        &source_path,
        r#"
module task_locals(
  input int a,
  output int y
);
  task automatic compute(input int x, output int result);
    int initialized = 10;
    int accumulated;
    accumulated = initialized + x;
    result = accumulated * 2;
  endtask

  always_comb compute(a, y);
endmodule
"#,
    )
    .expect("write task-local source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("task_locals"),
            &PreprocessOptions::default(),
        )
        .expect("lower task with local variables");
    assert_eq!(design.processes.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create task-local simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(32, 5))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(30));
}

#[test]
fn slang_ast_direct_lowering_supports_task_local_conditionals() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-task-local-conditionals");
    let source_path = root.join("task_local_conditionals.sv");
    std::fs::write(
        &source_path,
        r#"
module task_local_conditionals(
  input logic select,
  input int a,
  input int b,
  output int selected,
  output int maybe_incremented
);
  task automatic pick(
    input logic use_a,
    input int a_value,
    input int b_value,
    output int result
  );
    int value = 0;
    if (use_a) value = a_value;
    else value = b_value;
    result = value + 1;
  endtask

  task automatic increment_if(
    input logic enable,
    input int value,
    output int result
  );
    int accumulated = 7;
    if (enable) accumulated = value;
    result = accumulated;
  endtask

  always_comb begin
    pick(select, a, b, selected);
    increment_if(select, a, maybe_incremented);
  end
endmodule
"#,
    )
    .expect("write task-local conditional source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("task_local_conditionals"),
            &PreprocessOptions::default(),
        )
        .expect("lower task-local conditionals");
    assert_eq!(design.processes.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create task-local conditional simulator");
    for (select, a, b, selected, maybe_incremented) in
        [(false, 11, 22, 23, 7), (true, 11, 22, 12, 11)]
    {
        simulator
            .set_input("select", &LogicWord::from_u64(1, select as u64))
            .unwrap();
        simulator
            .set_input("a", &LogicWord::from_u64(32, a))
            .unwrap();
        simulator
            .set_input("b", &LogicWord::from_u64(32, b))
            .unwrap();
        assert_eq!(simulator.read("selected").unwrap().to_u64(), Some(selected));
        assert_eq!(
            simulator.read("maybe_incremented").unwrap().to_u64(),
            Some(maybe_incremented)
        );
    }
}

#[test]
fn slang_ast_direct_lowering_supports_task_local_loops() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-task-local-loops");
    let source_path = root.join("task_local_loops.sv");
    std::fs::write(
        &source_path,
        r#"
module task_local_loops(
  output int for_sum,
  output int while_product,
  output int repeat_sum
);
  task automatic sum_for(output int result);
    int total = 0;
    for (int i = 0; i < 4; i = i + 1) total = total + i;
    result = total;
  endtask

  task automatic product_while(output int result);
    int total = 1;
    int i = 0;
    while (i < 4) begin
      total = total * 2;
      i = i + 1;
    end
    result = total;
  endtask

  task automatic sum_repeat(output int result);
    int total = 2;
    repeat (4) total = total + 1;
    result = total;
  endtask

  always_comb begin
    sum_for(for_sum);
    product_while(while_product);
    sum_repeat(repeat_sum);
  end
endmodule
"#,
    )
    .expect("write task-local loop source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("task_local_loops"),
            &PreprocessOptions::default(),
        )
        .expect("lower task-local loops");
    assert_eq!(design.processes.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create task-local loop simulator");
    simulator.run_until(0).expect("settle task-local loops");
    assert_eq!(simulator.read("for_sum").unwrap().to_u64(), Some(6));
    assert_eq!(simulator.read("while_product").unwrap().to_u64(), Some(16));
    assert_eq!(simulator.read("repeat_sum").unwrap().to_u64(), Some(6));
}

#[test]
fn slang_ast_direct_lowering_rejects_recursive_tasks() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-recursive-task");
    let source_path = root.join("recursive_task.sv");
    std::fs::write(
        &source_path,
        r#"
module recursive_task(output int y);
  task automatic loop(input int x, output int result);
    if (x == 0) result = 1;
    else loop(x - 1, result);
  endtask

  always_comb loop(2, y);
endmodule
"#,
    )
    .expect("write recursive-task source");

    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("recursive_task"),
            &PreprocessOptions::default(),
        )
        .expect_err("unbounded recursive task inlining must be rejected");
    assert!(error.contains("recursive task"), "{error}");
}

#[test]
fn slang_ast_direct_lowering_rejects_excessive_function_loops() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-excessive-function-loop");
    let source_path = root.join("excessive_function_loop.sv");
    std::fs::write(
        &source_path,
        r#"
module excessive_function_loop(output int y);
  function automatic int sum();
    int result = 0;
    for (int i = 0; i < 257; i = i + 1) result = result + 1;
    return result;
  endfunction

  assign y = sum();
endmodule
"#,
    )
    .expect("write excessive-function-loop source");

    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("excessive_function_loop"),
            &PreprocessOptions::default(),
        )
        .expect_err("excessive function-loop inlining must be bounded");
    assert!(error.contains("256-iteration"), "{error}");
}

#[test]
fn slang_ast_direct_lowering_rejects_excessive_while_and_repeat_loops() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let cases = [
        (
            "while",
            r#"
module m(output int y);
  function automatic int sum();
    int result = 0;
    int i = 0;
    while (i < 257) begin
      result = result + 1;
      i = i + 1;
    end
    return result;
  endfunction
  assign y = sum();
endmodule
"#,
        ),
        (
            "repeat",
            r#"
module m(output int y);
  function automatic int sum();
    int result = 0;
    repeat (257) result = result + 1;
    return result;
  endfunction
  assign y = sum();
endmodule
"#,
        ),
    ];
    for (name, source) in cases {
        let root = temporary_directory("direct-ast-excessive-function-loop");
        let source_path = root.join(format!("{name}.sv"));
        std::fs::write(&source_path, source).expect("write excessive-loop source");
        let error = slang
            .compile_files_via_ast(
                std::slice::from_ref(&source_path),
                Some("m"),
                &PreprocessOptions::default(),
            )
            .expect_err("function-loop inlining must stay bounded");
        assert!(error.contains("256-iteration"), "{name}: {error}");
    }
}

#[test]
fn slang_ast_direct_lowering_supports_case_equality() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-case-equality");
    let source_path = root.join("case_equality.sv");
    std::fs::write(
        &source_path,
        r#"
module case_equality(
  input logic [3:0] a,
  input logic [3:0] b,
  output logic equal,
  output logic unequal
);
  assign equal = a === b;
  assign unequal = a !== b;
endmodule
"#,
    )
    .expect("write case-equality source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("case_equality"),
            &PreprocessOptions::default(),
        )
        .expect("lower case equality");
    assert_eq!(design.assigns.len(), 2);

    let mut simulator = Simulator::new(&design).expect("create case-equality simulator");
    simulator
        .set_input("a", &LogicWord::parse(4, "'b1x0z").expect("a with X/Z"))
        .unwrap();
    simulator
        .set_input("b", &LogicWord::parse(4, "'b1x0z").expect("matching X/Z"))
        .unwrap();
    assert_eq!(simulator.read("equal").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("unequal").unwrap().to_u64(), Some(0));

    simulator
        .set_input(
            "b",
            &LogicWord::parse(4, "'b1x00").expect("different Z bit"),
        )
        .unwrap();
    assert_eq!(simulator.read("equal").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("unequal").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_unary_operators() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-unary-operators");
    let source_path = root.join("unary_operators.sv");
    std::fs::write(
        &source_path,
        r#"
module unary_operators(
  input int signed value,
  input logic [3:0] bits,
  output int signed plus,
  output int signed minus,
  output logic reduction_and,
  output logic reduction_nand,
  output logic reduction_or,
  output logic reduction_nor,
  output logic reduction_xor,
  output logic reduction_xnor
);
  assign plus = +value;
  assign minus = -value;
  assign reduction_and = &bits;
  assign reduction_nand = ~&bits;
  assign reduction_or = |bits;
  assign reduction_nor = ~|bits;
  assign reduction_xor = ^bits;
  assign reduction_xnor = ~^bits;
endmodule
"#,
    )
    .expect("write unary-operator source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("unary_operators"),
            &PreprocessOptions::default(),
        )
        .expect("lower unary operators");
    assert_eq!(design.assigns.len(), 8);

    let mut simulator = Simulator::new(&design).expect("create unary simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(32, 5))
        .unwrap();
    simulator
        .set_input("bits", &LogicWord::from_u64(4, 0b0011))
        .unwrap();
    assert_eq!(simulator.read("plus").unwrap().to_u64(), Some(5));
    assert_eq!(simulator.read("minus").unwrap().to_hex_str(), "fffffffb");
    assert_eq!(simulator.read("reduction_and").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("reduction_nand").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("reduction_or").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("reduction_nor").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("reduction_xor").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("reduction_xnor").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_logical_implication_and_equivalence() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-logical-composition");
    let source_path = root.join("logical_composition.sv");
    std::fs::write(
        &source_path,
        r#"
module logical_composition(
  input logic a,
  input logic b,
  output logic imply_00,
  output logic imply_01,
  output logic imply_10,
  output logic imply_11,
  output logic equiv_00,
  output logic equiv_01,
  output logic equiv_10,
  output logic equiv_11
);
  assign imply_00 = a -> b;
  assign imply_01 = a -> b;
  assign imply_10 = a -> b;
  assign imply_11 = a -> b;
  assign equiv_00 = a <-> b;
  assign equiv_01 = a <-> b;
  assign equiv_10 = a <-> b;
  assign equiv_11 = a <-> b;
endmodule
"#,
    )
    .expect("write logical-composition source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("logical_composition"),
            &PreprocessOptions::default(),
        )
        .expect("lower logical composition");
    assert_eq!(design.assigns.len(), 8);

    let mut simulator = Simulator::new(&design).expect("create logical simulator");
    for (a, b, label) in [
        (false, false, "00"),
        (false, true, "01"),
        (true, false, "10"),
        (true, true, "11"),
    ] {
        simulator
            .set_input("a", &LogicWord::from_u64(1, a as u64))
            .unwrap();
        simulator
            .set_input("b", &LogicWord::from_u64(1, b as u64))
            .unwrap();
        assert_eq!(
            simulator.read(&format!("imply_{label}")).unwrap().to_u64(),
            Some((!a || b) as u64)
        );
        assert_eq!(
            simulator.read(&format!("equiv_{label}")).unwrap().to_u64(),
            Some((a == b) as u64)
        );
    }
}

#[test]
fn slang_ast_direct_lowering_supports_wildcard_and_inside_cases() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-wildcard-cases");
    let source_path = root.join("wildcard_cases.sv");
    std::fs::write(
        &source_path,
        r#"
module wildcard_cases(
  input logic [3:0] selector,
  input logic [1:0] pair,
  output logic [1:0] casez_y,
  output logic [1:0] casex_y,
  output logic [1:0] inside_y
);
  always_comb begin
    casez_y = 2'd0;
    casez (selector)
      4'b1???: casez_y = 2'd1;
      4'b01??: casez_y = 2'd2;
      default casez_y = 2'd3;
    endcase
  end

  always_comb begin
    casex_y = 2'd0;
    casex (selector)
      4'b1xxx: casex_y = 2'd1;
      4'b01xx: casex_y = 2'd2;
      default casex_y = 2'd3;
    endcase
  end

  always_comb begin
    inside_y = 2'd0;
    case (pair) inside
      [0:1]: inside_y = 2'd1;
      2: inside_y = 2'd2;
      default inside_y = 2'd3;
    endcase
  end
endmodule
"#,
    )
    .expect("write wildcard-case source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("wildcard_cases"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower wildcard and inside cases");
    let mut simulator = Simulator::new(&design).expect("create wildcard-case simulator");
    for (selector, expected) in [
        (0b1000_u64, 1_u64),
        (0b1101, 1),
        (0b0100, 2),
        (0b0110, 2),
        (0b0010, 3),
    ] {
        simulator
            .set_input("selector", &LogicWord::from_u64(4, selector))
            .unwrap();
        assert_eq!(simulator.read("casez_y").unwrap().to_u64(), Some(expected));
        assert_eq!(simulator.read("casex_y").unwrap().to_u64(), Some(expected));
    }
    for (pair, expected) in [(0_u64, 1_u64), (1, 1), (2, 2), (3, 3)] {
        simulator
            .set_input("pair", &LogicWord::from_u64(2, pair))
            .unwrap();
        assert_eq!(simulator.read("inside_y").unwrap().to_u64(), Some(expected));
    }

    simulator
        .set_input(
            "selector",
            &LogicWord {
                bits: vec![
                    LogicValue::Zero,
                    LogicValue::Zero,
                    LogicValue::Zero,
                    LogicValue::Z,
                ],
            },
        )
        .unwrap();
    assert_eq!(simulator.read("casez_y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("casex_y").unwrap().to_u64(), Some(1));

    simulator
        .set_input(
            "selector",
            &LogicWord {
                bits: vec![
                    LogicValue::Zero,
                    LogicValue::Zero,
                    LogicValue::X,
                    LogicValue::Zero,
                ],
            },
        )
        .unwrap();
    assert_eq!(simulator.read("casez_y").unwrap().to_u64(), Some(3));
    assert_eq!(simulator.read("casex_y").unwrap().to_u64(), Some(2));
}

#[test]
fn slang_ast_direct_lowering_supports_qualified_if_and_case_checks() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-qualified-checks");
    let source_path = root.join("qualified.sv");
    std::fs::write(
        &source_path,
        r#"
module qualified(
  input logic [1:0] selector,
  output logic [1:0] priority_if_y,
  output logic [1:0] unique_if_y,
  output logic [1:0] priority_case_y,
  output logic [1:0] unique_case_y
);
  always_comb begin
    priority_if_y = 2'd0;
    priority if (selector == 2'd0) priority_if_y = 2'd1;
    else if (selector == 2'd1) priority_if_y = 2'd2;
    else priority_if_y = 2'd3;
  end

  always_comb begin
    unique_if_y = 2'd0;
    unique if (selector == 2'd0) unique_if_y = 2'd1;
    else if (selector == 2'd1) unique_if_y = 2'd2;
    else unique_if_y = 2'd3;
  end

  always_comb begin
    priority_case_y = 2'd0;
    priority case (selector)
      2'd0: priority_case_y = 2'd1;
      2'd1: priority_case_y = 2'd2;
      default priority_case_y = 2'd3;
    endcase
  end

  always_comb begin
    unique_case_y = 2'd0;
    unique case (selector)
      2'd0: unique_case_y = 2'd1;
      2'd1: unique_case_y = 2'd2;
      default unique_case_y = 2'd3;
    endcase
  end
endmodule
"#,
    )
    .expect("write qualified-check source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("qualified"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower qualified if/case checks");
    let mut simulator = Simulator::new(&design).expect("create qualified-check simulator");
    for selector in 0..4 {
        let expected = if selector < 2 { selector + 1 } else { 3 };
        simulator
            .set_input("selector", &LogicWord::from_u64(2, selector))
            .unwrap();
        assert_eq!(
            simulator.read("priority_if_y").unwrap().to_u64(),
            Some(expected)
        );
        assert_eq!(
            simulator.read("unique_if_y").unwrap().to_u64(),
            Some(expected)
        );
        assert_eq!(
            simulator.read("priority_case_y").unwrap().to_u64(),
            Some(expected)
        );
        assert_eq!(
            simulator.read("unique_case_y").unwrap().to_u64(),
            Some(expected)
        );
    }

    let qualified_wildcard_path = root.join("qualified_wildcard.sv");
    std::fs::write(
        &qualified_wildcard_path,
        r#"
module qualified_wildcard(input logic [1:0] selector, output logic y);
  always_comb unique casez (selector)
    2'b?0: y = 1'b1;
    default y = 1'b0;
  endcase
endmodule
"#,
    )
    .expect("write qualified-wildcard source");
    let qualified_wildcard_error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&qualified_wildcard_path),
            Some("qualified_wildcard"),
            &PreprocessOptions::default(),
        )
        .expect_err("qualified wildcard checks must fail closed");
    assert!(
        qualified_wildcard_error.contains("Unique wildcard case semantics"),
        "unexpected qualified wildcard error: {qualified_wildcard_error}"
    );

    let overlap_path = root.join("unique_overlap.sv");
    std::fs::write(
        &overlap_path,
        r#"
module unique_overlap(input logic [1:0] selector, output logic [1:0] y);
  always_comb begin
    y = 2'd0;
    unique if (selector == 2'd0) y = 2'd1;
    else if (selector < 2'd2) y = 2'd2;
    else y = 2'd3;
  end
endmodule
"#,
    )
    .expect("write unique-if overlap source");
    let overlap = slang
        .compile_files_via_ast(
            std::slice::from_ref(&overlap_path),
            Some("unique_overlap"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower unique-if overlap check");
    let mut overlap_simulator = Simulator::new(&overlap).expect("create overlap simulator");
    let error = overlap_simulator
        .set_input("selector", &LogicWord::from_u64(2, 0))
        .expect_err("multiple unique-if matches must fail at runtime");
    assert!(
        error.contains("unique if has multiple matching conditions"),
        "unexpected overlap error: {error}"
    );

    let no_match_path = root.join("priority_no_match.sv");
    std::fs::write(
        &no_match_path,
        r#"
module priority_no_match(input logic [1:0] selector, output logic [1:0] y);
  always_comb begin
    y = 2'd0;
    priority case (selector)
      2'd0: y = 2'd1;
      2'd1: y = 2'd2;
    endcase
  end
endmodule
"#,
    )
    .expect("write priority no-match source");
    let no_match = slang
        .compile_files_via_ast(
            std::slice::from_ref(&no_match_path),
            Some("priority_no_match"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower priority no-match check");
    let mut no_match_simulator = Simulator::new(&no_match).expect("create no-match simulator");
    let error = no_match_simulator
        .set_input("selector", &LogicWord::from_u64(2, 2))
        .expect_err("priority case no-match must fail at runtime");
    assert!(
        error.contains("priority case has no matching item"),
        "unexpected no-match error: {error}"
    );

    let duplicate_path = root.join("unique_duplicate.sv");
    std::fs::write(
        &duplicate_path,
        r#"
module unique_duplicate(input logic [1:0] selector, output logic [1:0] y);
  always_comb begin
    y = 2'd0;
    unique case (selector)
      2'd0: y = 2'd1;
      2'd0: y = 2'd2;
    endcase
  end
endmodule
"#,
    )
    .expect("write unique-case duplicate source");
    let duplicate = slang
        .compile_files_via_ast(
            std::slice::from_ref(&duplicate_path),
            Some("unique_duplicate"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower unique-case overlap check");
    let mut duplicate_simulator = Simulator::new(&duplicate).expect("create duplicate simulator");
    let error = duplicate_simulator
        .set_input("selector", &LogicWord::from_u64(2, 0))
        .expect_err("multiple unique-case items must fail at runtime");
    assert!(
        error.contains("unique case has multiple matching items"),
        "unexpected duplicate error: {error}"
    );

    let unique_zero_path = root.join("unique_zero.sv");
    std::fs::write(
        &unique_zero_path,
        r#"
module unique_zero(input logic [1:0] selector, output logic [1:0] y);
  always_comb begin
    y = 2'd0;
    unique0 case (selector)
      2'd0: y = 2'd1;
    endcase
  end
endmodule
"#,
    )
    .expect("write unique0 source");
    let unique_zero = slang
        .compile_files_via_ast(
            std::slice::from_ref(&unique_zero_path),
            Some("unique_zero"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower unique0 case check");
    let mut unique_zero_simulator = Simulator::new(&unique_zero).expect("create unique0 simulator");
    unique_zero_simulator
        .set_input("selector", &LogicWord::from_u64(2, 1))
        .unwrap();
    assert_eq!(unique_zero_simulator.read("y").unwrap().to_u64(), Some(0));
}

#[test]
fn slang_ast_direct_lowering_supports_indexed_packed_part_selects() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-indexed-packed");
    let source_path = root.join("indexed_packed.sv");
    std::fs::write(
        &source_path,
        r#"
module indexed_packed(
  input logic [15:0] a,
  input logic [3:0] start,
  output logic [7:0] up,
  output logic [7:0] down
);
  assign up = a[start +: 8];
  assign down = a[start -: 8];
endmodule
"#,
    )
    .expect("write indexed packed select source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("indexed_packed"),
            &PreprocessOptions::default(),
        )
        .expect("lower indexed packed selects");
    assert_eq!(design.assigns.len(), 2);

    let mut simulator = Simulator::new(&design).expect("create indexed select simulator");
    for (start, expected_up, expected_down) in [
        (0_u64, 0x34_u64, 0),
        (4, 0x23, 0),
        (7, 0x24, 0x34),
        (8, 0x12, 0x1a),
    ] {
        simulator
            .set_input("a", &LogicWord::from_u64(16, 0x1234))
            .unwrap();
        simulator
            .set_input("start", &LogicWord::from_u64(4, start))
            .unwrap();
        assert_eq!(simulator.read("up").unwrap().to_u64(), Some(expected_up));
        if start >= 7 {
            assert_eq!(
                simulator.read("down").unwrap().to_u64(),
                Some(expected_down)
            );
        } else {
            assert!(simulator.read("down").unwrap().to_u64().is_none());
        }
    }
}

#[test]
fn slang_ast_direct_lowering_resolves_elaborated_parameters() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-parameters");
    let source_path = root.join("parameters.sv");
    std::fs::write(
        &source_path,
        r#"
module parameters(output logic [7:0] byte_value, output logic [15:0] word_value);
  parameter logic [7:0] BYTE = 8'hA5;
  localparam logic [15:0] WORD = 16'h1234;
  assign byte_value = BYTE;
  assign word_value = WORD;
endmodule
"#,
    )
    .expect("write parameter source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("parameters"),
            &PreprocessOptions::default(),
        )
        .expect("resolve elaborated parameters");
    assert_eq!(design.assigns.len(), 2);

    let mut simulator = Simulator::new(&design).expect("create parameter simulator");
    simulator
        .run_until(1)
        .expect("settle parameter assignments");
    assert_eq!(simulator.read("byte_value").unwrap().to_u64(), Some(0xa5));
    assert_eq!(simulator.read("word_value").unwrap().to_u64(), Some(0x1234));
}

#[test]
fn slang_ast_direct_lowering_reduces_logical_not_to_one_bit() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-logical-not");
    let source_path = root.join("logical_not.sv");
    std::fs::write(
        &source_path,
        r#"
module logical_not(input logic [7:0] a, output logic y);
  assign y = !a;
endmodule
"#,
    )
    .expect("write logical-not source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("logical_not"),
            &PreprocessOptions::default(),
        )
        .expect("lower logical reduction");

    let mut simulator = Simulator::new(&design).expect("create logical-not simulator");
    for (input, expected) in [(0_u64, 1_u64), (1, 0), (0x80, 0), (0xff, 0)] {
        simulator
            .set_input("a", &LogicWord::from_u64(8, input))
            .expect("set logical-not input");
        assert_eq!(simulator.read("y").unwrap().to_u64(), Some(expected));
    }
}

#[test]
fn slang_ast_direct_lowering_preserves_ascending_packed_selects() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-ascending");
    let source_path = root.join("ascending.sv");
    std::fs::write(
        &source_path,
        r#"
module ascending(
  input logic [0:7] a,
  output logic bit0,
  output logic [3:0] high
);
  assign bit0 = a[0];
  assign high = a[0:3];
endmodule
"#,
    )
    .expect("write ascending packed source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("ascending"),
            &PreprocessOptions::default(),
        )
        .expect("lower ascending packed declaration");
    assert_eq!(design.n_nets, 13);

    let mut simulator = Simulator::new(&design).expect("create ascending simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(8, 0xf0))
        .expect("set ascending input");
    assert_eq!(simulator.read("bit0").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("high").unwrap().to_u64(), Some(0xf));
}

#[test]
fn slang_ast_direct_lowering_supports_wildcard_equality() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-wildcard-equality");
    let source_path = root.join("wildcard_equality.sv");
    std::fs::write(
        &source_path,
        r#"
module wildcard_equality(
  input logic [7:0] a,
  output logic equal,
  output logic unequal
);
  assign equal = (a ==? 8'b1010??01);
  assign unequal = (a !=? 8'b1010??01);
endmodule
"#,
    )
    .expect("write wildcard-equality source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("wildcard_equality"),
            &PreprocessOptions::default(),
        )
        .expect("lower wildcard equality");
    assert_eq!(design.assigns.len(), 2);

    let mut simulator = Simulator::new(&design).expect("create wildcard simulator");
    for (value, expected) in [(0xa5_u64, true), (0xa1, true), (0x85, false), (0xa4, false)] {
        simulator
            .set_input("a", &LogicWord::from_u64(8, value))
            .unwrap();
        assert_eq!(
            simulator.read("equal").unwrap().to_u64(),
            Some(expected as u64)
        );
        assert_eq!(
            simulator.read("unequal").unwrap().to_u64(),
            Some((!expected) as u64)
        );
    }
}

#[test]
fn slang_ast_direct_lowering_supports_inside() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-inside");
    let source_path = root.join("inside.sv");
    std::fs::write(
        &source_path,
        r#"
module inside_expr(input logic [7:0] a, output logic y);
  assign y = a inside {8'h00, [8'h0a:8'h0f], 8'hf0};
endmodule
"#,
    )
    .expect("write inside source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("inside_expr"),
            &PreprocessOptions::default(),
        )
        .expect("lower inside expression");
    assert_eq!(design.assigns.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create inside simulator");
    for (value, expected) in [
        (0x00_u64, true),
        (0x01, false),
        (0x09, false),
        (0x0a, true),
        (0x0d, true),
        (0x0f, true),
        (0x10, false),
        (0xef, false),
        (0xf0, true),
        (0xf1, false),
    ] {
        simulator
            .set_input("a", &LogicWord::from_u64(8, value))
            .unwrap();
        assert_eq!(
            simulator.read("y").unwrap().to_u64(),
            Some(expected as u64),
            "value {value:#04x}"
        );
    }
}

#[test]
fn slang_ast_direct_lowering_reports_expression_source_spans() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-source-span");
    let source_path = root.join("spanned.sv");
    std::fs::write(
        &source_path,
        r#"
module spanned(
  input logic [3:0] a,
  output logic [3:0] y
);
  assign y = $countones(a);
endmodule
"#,
    )
    .expect("write source-span source");

    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("spanned"),
            &PreprocessOptions::default(),
        )
        .expect_err("unsupported direct expression must include its source span");
    assert!(
        error.contains("unknown direct-lowering function"),
        "{error}"
    );
    assert!(error.contains("[slang at "), "{error}");
    assert!(error.contains("spanned.sv:6:14"), "{error}");
}

#[test]
fn slang_ast_json_contains_elaborated_module_symbols() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let design = slang
        .compile_source(
            "module json_probe(input [3:0] a, output [3:0] y); assign y = a + 4'd1; endmodule",
            Some("json_probe"),
            &PreprocessOptions::default(),
        )
        .expect("compile AST probe");
    assert_eq!(design.n_nets, 8);

    let path = std::env::temp_dir().join(format!(
        "eda-slang-json-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &path,
        "module json_probe(input [3:0] a, output [3:0] y); assign y = a + 4'd1; endmodule",
    )
    .expect("write AST source");
    let json = slang
        .ast_json(&[path], Some("json_probe"), &["json_probe".to_string()])
        .expect("serialize slang AST");
    assert!(json.starts_with('{'));
    assert!(json.contains("\"kind\": \"Instance\""));
    assert!(json.contains("\"name\": \"json_probe\""));
}

#[test]
fn slang_accepts_the_official_accellera_uvm_reference_source() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some(uvm_home) = std::env::var_os("UVM_HOME") else {
        return;
    };
    let root = PathBuf::from(uvm_home);
    let source = root.join("src").join("uvm.sv");
    if !source.is_file() {
        return;
    }
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    slang
        .check_files_with_options(&[source], None, &options)
        .expect("slang elaborates the official UVM reference library");
}

#[test]
fn slang_validates_the_repository_cpu_rtl_compilation_units() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf();
    let rtl = root.join("platform").join("rtl");
    let i8086 = [
        rtl.join("i8086_prefix_decoder.sv"),
        rtl.join("i8086_opcode_decoder.sv"),
        rtl.join("i8086_modrm_decoder.sv"),
        rtl.join("i8086_full_core.sv"),
        rtl.join("i8086_full_runtime_tb.sv"),
    ];
    slang
        .check_files(&i8086, Some("i8086_full_runtime_tb"))
        .expect("slang validates the i8086 RTL compilation unit");

    let rv32i = [
        rtl.join("rv32i_full_core.sv"),
        rtl.join("rv32i_full_runtime_tb.sv"),
    ];
    slang
        .check_files(&rv32i, Some("rv32i_full_runtime_tb"))
        .expect("slang validates the RV32I RTL compilation unit");

    let rv32i_directed = [
        rtl.join("rv32i_full_core.sv"),
        rtl.join("rv32i_full_soc_16.sv"),
        rtl.join("rv32i_full_tb_16.sv"),
    ];
    slang
        .check_files(&rv32i_directed, Some("rv32i_full_tb_16"))
        .expect("slang validates the directed RV32I RTL compilation unit");
}
