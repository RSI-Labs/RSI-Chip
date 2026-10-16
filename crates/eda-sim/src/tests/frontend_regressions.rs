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

    // The ten additional nets are generated bounds/selectors for runtime access.
    assert_eq!(sim.design.n_nets, 32_797);
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
