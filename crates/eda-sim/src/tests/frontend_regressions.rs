//! Focused regression tests for frontend language constructs that previously
//! failed to elaborate or were silently mis-elaborated.

use super::*;
use eda_hdl::{compile_sources, HdlSource};

#[test]
fn runtime_whole_unpacked_array_element_read_write_and_nonblocking_update() {
    let mut sim = sim_for(
        "module m(input clk, input [1:0] selector, input [7:0] din, \
                  output reg [7:0] blocking_q, nonblocking_q); \
         logic [7:0] memory [0:3]; \
         logic [7:0] nba_memory [0:3]; \
         initial begin \
           memory[0] = 8'd10; memory[1] = 8'd20; \
           memory[2] = 8'd30; memory[3] = 8'd40; \
           nba_memory[0] = 8'd10; nba_memory[1] = 8'd20; \
           nba_memory[2] = 8'd30; nba_memory[3] = 8'd40; \
         end \
         always @(posedge clk) begin \
           blocking_q = memory[selector]; \
           memory[selector] = din; \
         end \
         always @(posedge clk) nba_memory[selector] <= din; \
         assign nonblocking_q = nba_memory[selector]; \
         endmodule",
    );

    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("selector", &LogicWord::from_u64(2, 1))
        .unwrap();
    sim.set_input("din", &LogicWord::from_u64(8, 0x5a)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

    assert_eq!(sim.read("blocking_q").unwrap().to_u64(), Some(20));
    assert_eq!(sim.read("nonblocking_q").unwrap().to_u64(), Some(0x5a));
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(sim.read("blocking_q").unwrap().to_u64(), Some(0x5a));
    assert_eq!(sim.read("nonblocking_q").unwrap().to_u64(), Some(0x5a));
}

#[test]
fn named_multi_source_designs_compile_and_simulate() {
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
    let design =
        compile_sources(&sources, Some("top")).expect("compile named multi-source hierarchy");
    let mut simulator = Simulator::new(&design).expect("simulate named multi-source hierarchy");
    simulator
        .set_input("x", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("z").unwrap().to_u64(), Some(1));
    simulator
        .set_input("x", &LogicWord::from_u64(1, 0))
        .unwrap();
    assert_eq!(simulator.read("z").unwrap().to_u64(), Some(0));
}

#[test]
fn four_kibyte_unpacked_array_supports_runtime_element_access() {
    let mut sim = sim_for(
        r#"
module large_memory(input clk, input [11:0] selector, input [7:0] din,
                     output [7:0] q);
  logic [7:0] memory [0:4095];
  initial begin
    memory[0] = 8'h3c;
    memory[4095] = 8'h5a;
  end
  always @(posedge clk) memory[selector] <= din;
  assign q = memory[selector];
endmodule
"#,
    );

    // The 4 KiB array is sparse: only its one-bit storage declaration placeholder
    // remains in the flat net space.
    assert_eq!(sim.design.n_nets, 30);
    assert_eq!(sim.design.memories.len(), 1);
    assert_eq!(sim.design.memories[0].flat_length, 4096);
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("selector", &LogicWord::from_u64(12, 0))
        .unwrap();
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0x3c));

    sim.set_input("selector", &LogicWord::from_u64(12, 4095))
        .unwrap();
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0x5a));

    sim.set_input("din", &LogicWord::from_u64(8, 0xa7)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0xa7));
}

#[test]
fn one_mebibyte_sparse_memory_uses_one_flat_placeholder_net() {
    let mut sim = sim_for(
        r#"
module large_memory(input clk, input [19:0] selector, input [7:0] din,
                     output [7:0] q);
  logic [7:0] memory [0:1048575];
  initial begin
    memory[0] = 8'h12;
    memory[1048575] = 8'h34;
  end
  always @(posedge clk) memory[selector] <= din;
  assign q = memory[selector];
endmodule
"#,
    );
    assert_eq!(sim.design.n_nets, 38);
    assert_eq!(sim.design.memories.len(), 1);
    assert_eq!(sim.design.memories[0].flat_length, 1_048_576);

    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("selector", &LogicWord::from_u64(20, 0))
        .unwrap();
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0x12));

    sim.set_input("selector", &LogicWord::from_u64(20, 1_048_575))
        .unwrap();
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0x34));

    sim.set_input("din", &LogicWord::from_u64(8, 0x56)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0x56));
}

#[test]
fn sparse_memory_partial_reads_and_writes_preserve_other_bits() {
    let mut sim = sim_for(
        r#"
module m(input clk, input [11:0] selector, input [3:0] offset,
         input [3:0] din, output [3:0] q, output [7:0] whole);
  logic [7:0] memory [0:4095];
  initial begin
    memory[5] = 8'hA5;
    memory[5][2 +: 4] = 4'hC;
    memory[6] = 8'h3c;
  end
  always @(posedge clk) memory[selector][offset +: 4] <= din;
  assign q = memory[selector][offset +: 4];
  assign whole = memory[5];
endmodule
"#,
    );
    assert_eq!(sim.design.n_nets, 34);
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("selector", &LogicWord::from_u64(12, 5))
        .unwrap();
    sim.set_input("offset", &LogicWord::from_u64(4, 2)).unwrap();
    assert_eq!(sim.read("whole").unwrap().to_u64(), Some(0xB1));
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0xC));

    sim.set_input("din", &LogicWord::from_u64(4, 0x5)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(sim.read("whole").unwrap().to_u64(), Some(0x95));
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0x5));

    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("offset", &LogicWord::from_u64(4, 6)).unwrap();
    sim.set_input("din", &LogicWord::from_u64(4, 0xF)).unwrap();
    assert_eq!(sim.read("q").unwrap().to_u64(), None);
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(sim.read("whole").unwrap().to_u64(), Some(0x95));
}

#[test]
fn reversed_sparse_memory_element_part_selects_use_logical_bit_order() {
    let mut sim = sim_for(
        r#"
module m(input clk, input [8:0] selector, input [2:0] offset,
         input [3:0] din, output [3:0] q, output [7:0] whole);
  logic [0:7] memory [0:512];
  initial begin
    memory[5] = 8'h12;
    memory[5][0 +: 4] = 4'hC;
    memory[5][4 +: 4] = 4'h3;
  end
  always @(posedge clk) memory[selector][offset +: 4] <= din;
  assign q = memory[selector][offset +: 4];
  assign whole = memory[5];
endmodule
"#,
    );
    assert_eq!(sim.design.n_nets, 30);
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("selector", &LogicWord::from_u64(9, 5))
        .unwrap();
    sim.set_input("offset", &LogicWord::from_u64(3, 0)).unwrap();
    assert_eq!(sim.read("whole").unwrap().to_u64(), Some(0x3C));
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0xC));

    sim.set_input("offset", &LogicWord::from_u64(3, 4)).unwrap();
    sim.set_input("din", &LogicWord::from_u64(4, 0xA)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(sim.read("whole").unwrap().to_u64(), Some(0xAC));
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0xA));

    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("offset", &LogicWord::from_u64(3, 5)).unwrap();
    assert_eq!(sim.read("q").unwrap().to_u64(), None);
}

#[test]
fn sparse_memory_nonblocking_partial_writes_merge_disjoint_fields() {
    let mut sim = sim_for(
        r#"
module m(output [7:0] q);
  logic [7:0] memory [0:4095];
  initial begin
    memory[5] = 8'hA5;
    memory[5][0 +: 4] <= 4'hC;
    memory[5][4 +: 4] <= 4'h3;
  end
  assign q = memory[5];
endmodule
"#,
    );
    assert_eq!(sim.design.n_nets, 9);
    sim.run_until(0).unwrap();
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0x3C));
}

#[test]
fn sparse_memory_partial_continuous_lvalue_preserves_other_bits() {
    let mut sim = sim_for(
        r#"
module m(input [3:0] offset, input [3:0] din, output [7:0] q);
  logic [7:0] memory [0:4095];
  initial memory[5] = 8'hA5;
  assign memory[5][offset +: 4] = din;
  assign q = memory[5];
endmodule
"#,
    );
    assert_eq!(sim.design.n_nets, 17);
    sim.set_input("offset", &LogicWord::from_u64(4, 2)).unwrap();
    sim.set_input("din", &LogicWord::from_u64(4, 0xC)).unwrap();
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0xB1));

    sim.set_input("offset", &LogicWord::from_u64(4, 6)).unwrap();
    sim.set_input("din", &LogicWord::from_u64(4, 0xF)).unwrap();
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0xB1));
}

#[test]
fn runtime_whole_unpacked_array_element_supports_continuous_assignment() {
    let mut sim = sim_for(
        "module m(input [1:0] selector, input [7:0] din, output [7:0] q); \
         logic [7:0] memory [0:3]; \
         assign memory[selector] = din; \
         assign q = memory[selector]; \
         endmodule",
    );

    sim.set_input("selector", &LogicWord::from_u64(2, 2))
        .unwrap();
    sim.set_input("din", &LogicWord::from_u64(8, 0xa7)).unwrap();
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0xa7));

    sim.set_input("selector", &LogicWord::from_u64(2, 0))
        .unwrap();
    sim.set_input("din", &LogicWord::from_u64(8, 0x3c)).unwrap();
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0x3c));
}

#[test]
fn runtime_whole_unpacked_array_element_read_returns_x_out_of_range() {
    let mut sim = sim_for(
        "module m(input [2:0] selector, output [7:0] q); \
         logic [7:0] memory [0:3]; \
         initial begin \
           memory[0] = 8'd10; memory[1] = 8'd20; \
           memory[2] = 8'd30; memory[3] = 8'd40; \
         end \
         assign q = memory[selector]; \
         endmodule",
    );

    sim.set_input("selector", &LogicWord::from_u64(3, 4))
        .unwrap();
    assert!(sim.read("q").unwrap().to_u64().is_none());
    sim.set_input("selector", &LogicWord::from_u64(3, 7))
        .unwrap();
    assert!(sim.read("q").unwrap().to_u64().is_none());
}

#[test]
fn descending_generate_loop_and_false_if_without_else_simulate() {
    let mut descending = sim_for(
        "module m(input [2:0] a, output [2:0] y); \
         genvar i; \
         generate for (i = 2; i >= 0; i = i - 1) begin : bits \
           assign y[i] = ~a[i]; \
         end endgenerate endmodule",
    );
    descending
        .set_input("a", &LogicWord::from_u64(3, 0b010))
        .unwrap();
    assert_eq!(descending.read("y").unwrap().to_u64(), Some(0b101));

    let mut disabled = sim_for(
        "module m #(parameter ENABLE = 0) (input a, output y); \
         wire generated = 1'b0; \
         generate if (ENABLE) begin : enabled \
           assign generated = 1'b1; \
         end endgenerate \
         assign y = generated | a; \
         endmodule",
    );
    disabled.set_input("a", &LogicWord::from_u64(1, 0)).unwrap();
    assert_eq!(disabled.read("y").unwrap().to_u64(), Some(0));
}
