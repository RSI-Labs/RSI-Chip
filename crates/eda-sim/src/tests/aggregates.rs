//! Interface/module arrays, unpacked ports, and indexed part-select tests.

use super::*;

#[test]
fn interface_array_elements_remain_distinct() {
    let mut sim = sim_for(
        "interface bus;
            logic [7:0] data;
            logic valid;
            modport source(output valid, data);
            modport sink(input valid, data);
         endinterface

         module producer(interface bus.source b, input logic [7:0] value);
            assign b.valid = 1'b1;
            assign b.data = value;
         endmodule

         module consumer(interface bus.sink b, output logic [7:0] result);
            assign result = b.valid ? b.data : 8'd0;
         endmodule

         module top(input logic [7:0] first, second,
             output logic [7:0] first_result, second_result);
            bus shared[0:1]();
            producer p0(.b(shared[0]), .value(first));
            producer p1(.b(shared[1]), .value(second));
            consumer c0(.b(shared[0]), .result(first_result));
            consumer c1(.b(shared[1]), .result(second_result));
         endmodule",
    );
    assert_eq!(
        sim.design
            .hierarchy
            .iter()
            .filter(|instance| instance.path.ends_with("shared[0]")
                || instance.path.ends_with("shared[1]"))
            .count(),
        2
    );
    sim.set_input("first", &LogicWord::from_u64(8, 0x3c))
        .unwrap();
    sim.set_input("second", &LogicWord::from_u64(8, 0xa5))
        .unwrap();
    assert_eq!(sim.read("first_result").unwrap().to_u64(), Some(0x3c));
    assert_eq!(sim.read("second_result").unwrap().to_u64(), Some(0xa5));
}

#[test]
fn multidimensional_interface_array_elements_remain_distinct() {
    let mut sim = sim_for(
        "interface bus;
            logic [7:0] data;
            logic valid;
            modport source(output valid, data);
            modport sink(input valid, data);
         endinterface

         module producer(interface bus.source b, input logic [7:0] value);
            assign b.valid = 1'b1;
            assign b.data = value;
         endmodule

         module consumer(interface bus.sink b, output logic [7:0] result);
            assign result = b.valid ? b.data : 8'd0;
         endmodule

         module top(input logic [7:0] first, second,
             output logic [7:0] first_result, second_result);
            bus shared[0:1][0:2]();
            producer p0(.b(shared[0][1]), .value(first));
            producer p1(.b(shared[1][2]), .value(second));
            consumer c0(.b(shared[0][1]), .result(first_result));
            consumer c1(.b(shared[1][2]), .result(second_result));
         endmodule",
    );
    assert_eq!(
        sim.design
            .hierarchy
            .iter()
            .filter(|instance| instance.module == "bus")
            .count(),
        6
    );
    sim.set_input("first", &LogicWord::from_u64(8, 0x47))
        .unwrap();
    sim.set_input("second", &LogicWord::from_u64(8, 0xb9))
        .unwrap();
    assert_eq!(sim.read("first_result").unwrap().to_u64(), Some(0x47));
    assert_eq!(sim.read("second_result").unwrap().to_u64(), Some(0xb9));
}

#[test]
fn module_instance_array_slices_vector_connections() {
    let mut sim = sim_for(
        "module leaf(input a, b, output y);
            assign y = a & b;
         endmodule

         module top(input [3:0] a, b, output [3:0] y);
            leaf u[0:3](.a(a), .b(b), .y(y));
         endmodule",
    );
    assert_eq!(
        sim.design
            .hierarchy
            .iter()
            .filter(|instance| instance.module == "leaf")
            .count(),
        4
    );
    sim.set_input("a", &LogicWord::from_u64(4, 0b1101)).unwrap();
    sim.set_input("b", &LogicWord::from_u64(4, 0b1011)).unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0b1001));

    let mut vector_sim = sim_for(
        "module leaf(input [1:0] a, output [1:0] y);
            assign y = ~a;
         endmodule

         module top(input [3:0] a, output [3:0] y);
            leaf u[0:1](.a(a), .y(y));
         endmodule",
    );
    vector_sim
        .set_input("a", &LogicWord::from_u64(4, 0b0110))
        .unwrap();
    assert_eq!(vector_sim.read("y").unwrap().to_u64(), Some(0b1001));
}

#[test]
fn multidimensional_module_array_slices_row_major_connections() {
    let mut sim = sim_for(
        "module leaf(input [1:0] a, output [1:0] y);
            assign y = ~a;
         endmodule

         module top(input [11:0] a, output [11:0] y);
            leaf u[0:1][0:2](.a(a), .y(y));
         endmodule",
    );
    assert_eq!(
        sim.design
            .hierarchy
            .iter()
            .filter(|instance| instance.module == "leaf")
            .count(),
        6
    );
    sim.set_input("a", &LogicWord::from_u64(12, 0b011010100110))
        .unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0b100101011001));

    let mut descending = sim_for(
        "module leaf(input a, output y);
            assign y = ~a;
         endmodule

         module top(input [3:0] a, output [3:0] y);
            leaf u[1:0][1:0](.a(a), .y(y));
         endmodule",
    );
    descending
        .set_input("a", &LogicWord::from_u64(4, 0b0110))
        .unwrap();
    assert_eq!(descending.read("y").unwrap().to_u64(), Some(0b1001));
}

#[test]
fn module_unpacked_net_array_elements_update_independently() {
    let mut sim = sim_for(
        "module top(input [7:0] a, b, output [31:0] y);
            logic [7:0] memory [0:3];
            assign memory[0] = a & b;
            assign memory[1] = a | b;
            assign memory[2] = a ^ b;
            assign memory[3] = a + b;
            assign y = {memory[0], memory[1], memory[2], memory[3]};
         endmodule",
    );
    let declaration = sim
        .design
        .net_decls
        .iter()
        .find(|declaration| declaration.name == "memory")
        .expect("memory declaration");
    assert_eq!(declaration.width, 32);
    sim.set_input("a", &LogicWord::from_u64(8, 0x5a)).unwrap();
    sim.set_input("b", &LogicWord::from_u64(8, 0xa5)).unwrap();
    let expected = ((0x5a_u64 + 0xa5) & 0xff) | ((0x5a ^ 0xa5) << 8) | ((0x5a | 0xa5) << 16);
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(expected));

    let mut descending = sim_for(
        "module top(input [7:0] a, output [15:0] y);
            logic [7:0] memory [3:0];
            assign memory[0] = a;
            assign memory[3] = ~a;
            assign y = {memory[3], memory[0]};
         endmodule",
    );
    descending
        .set_input("a", &LogicWord::from_u64(8, 0x3c))
        .unwrap();
    assert_eq!(
        descending.read("y").unwrap().to_u64(),
        Some((!0x3c_u64 & 0xff) << 8 | 0x3c)
    );
}

#[test]
fn module_multidimensional_unpacked_net_array_selects_elements() {
    let mut sim = sim_for(
        "module top(input [7:0] a, b, output [47:0] y);
            logic [7:0] memory [0:1][0:2];
            assign memory[0][0] = a & b;
            assign memory[0][1] = a | b;
            assign memory[0][2] = a ^ b;
            assign memory[1][0] = a + b;
            assign memory[1][1] = a - b;
            assign memory[1][2] = ~a;
            assign y = {
                memory[0][0], memory[0][1], memory[0][2],
                memory[1][0], memory[1][1], memory[1][2]
            };
         endmodule",
    );
    let declaration = sim
        .design
        .net_decls
        .iter()
        .find(|declaration| declaration.name == "memory")
        .expect("memory declaration");
    assert_eq!(declaration.width, 48);
    sim.set_input("a", &LogicWord::from_u64(8, 0x5a)).unwrap();
    sim.set_input("b", &LogicWord::from_u64(8, 0xa5)).unwrap();
    let expected = ((0x5a_u64 | 0xa5) << 32)
        | ((0x5a ^ 0xa5) << 24)
        | (((0x5a + 0xa5) & 0xff) << 16)
        | ((((0x5a + 0x100) - 0xa5) & 0xff) << 8)
        | (!0x5a_u64 & 0xff);
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(expected));
}

#[test]
fn unpacked_array_elements_support_indexed_part_selects() {
    let mut sim = sim_for(
        "module top(input logic clk, input logic [3:0] selector, \
                      input logic [1:0] element_selector, \
                      input logic [3:0] a, output logic [3:0] y, z, q, dynamic); \
             logic [7:0] memory [0:3]; \
             initial begin \
                 memory[1] = 8'ha5; \
                 memory[2] = 8'h3c; \
             end \
             assign y = memory[1][selector +: 4]; \
             assign memory[0][3 -: 2] = a & 2'd3; \
             assign q = memory[0][3 -: 2]; \
             always @(posedge clk) \
                 memory[2][selector -: 4] <= a ^ 4'd3; \
             assign z = memory[2][selector -: 4]; \
             assign dynamic = memory[element_selector][selector +: 4]; \
         endmodule",
    );
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("selector", &LogicWord::from_u64(4, 4))
        .unwrap();
    sim.set_input("element_selector", &LogicWord::from_u64(2, 1))
        .unwrap();
    sim.set_input("a", &LogicWord::from_u64(4, 0xc)).unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0xa));
    assert_eq!(sim.read("dynamic").unwrap().to_u64(), Some(0xa));
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0x0));
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(sim.read("z").unwrap().to_u64(), Some(0xf));
    sim.set_input("element_selector", &LogicWord::from_u64(2, 2))
        .unwrap();
    assert_eq!(sim.read("dynamic").unwrap().to_u64(), Some(0x3));
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("selector", &LogicWord::from_u64(4, 13))
        .unwrap();
    assert!(sim.read("y").unwrap().to_u64().is_none());
    sim.set_input("element_selector", &LogicWord::from_u64(2, 3))
        .unwrap();
    assert!(sim.read("dynamic").unwrap().to_u64().is_none());
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert!(sim.read("z").unwrap().to_u64().is_none());
}

#[test]
fn fixed_unpacked_array_ports_connect_as_flat_vectors() {
    let mut sim = sim_for(
        "module child(
            input  logic [7:0] in_mem  [0:3],
            output logic [7:0] out_mem [0:3]
        );
            assign out_mem[0] = in_mem[0] + 8'd1;
            assign out_mem[1] = in_mem[1] & 8'hff;
            assign out_mem[2] = in_mem[2] | 8'h0f;
            assign out_mem[3] = in_mem[3] ^ 8'h5a;
         endmodule

         module top(input logic [31:0] a, output logic [31:0] y);
            child c(.in_mem(a), .out_mem(y));
         endmodule",
    );
    let child_input = sim
        .design
        .net_decls
        .iter()
        .find(|declaration| declaration.name == "c.in_mem")
        .expect("child array input declaration");
    assert_eq!(child_input.width, 32);
    let child_output = sim
        .design
        .net_decls
        .iter()
        .find(|declaration| declaration.name == "c.out_mem")
        .expect("child array output declaration");
    assert_eq!(child_output.width, 32);
    let child_ports = sim
        .design
        .hierarchy
        .iter()
        .find(|instance| instance.module == "child")
        .expect("child hierarchy record");
    assert_eq!(child_ports.ports.len(), 2);
    assert!(child_ports.ports.iter().all(|port| port.width == 32));

    sim.set_input("a", &LogicWord::from_u64(32, 0x4030_2010))
        .unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x1a3f_2011));
}

#[test]
fn non_ansi_unpacked_array_port_merges_body_declaration() {
    let mut sim = sim_for(
        "module top(mem, y);
            input logic [7:0] mem [3:0];
            output logic [31:0] y;
            assign y = {mem[3], mem[2], mem[1], mem[0]};
         endmodule",
    );
    let mem = sim
        .design
        .net_decls
        .iter()
        .find(|declaration| declaration.name == "mem")
        .expect("non-ANSI array port declaration");
    assert_eq!(mem.width, 32);
    assert_eq!(
        sim.design
            .ports
            .iter()
            .find(|port| port.name == "mem")
            .expect("non-ANSI array top port")
            .width,
        32
    );
    sim.set_input("mem", &LogicWord::from_u64(32, 0x4030_2010))
        .unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x1020_3040));
}

#[test]
fn module_instance_arrays_slice_child_array_ports() {
    let mut sim = sim_for(
        "module child(input logic [7:0] in_mem [0:1], \
                      output logic [7:0] out_mem [0:1]); \
             assign out_mem[0] = in_mem[0] + 8'd1; \
             assign out_mem[1] = in_mem[1] ^ 8'h0f; \
         endmodule \
         module top(input logic [31:0] a, output logic [31:0] y); \
             child c[0:1](.in_mem(a), .out_mem(y)); \
         endmodule",
    );
    let children = sim
        .design
        .hierarchy
        .iter()
        .filter(|instance| instance.module == "child")
        .count();
    assert_eq!(children, 2);
    assert!(sim
        .design
        .hierarchy
        .iter()
        .filter(|instance| instance.module == "child")
        .all(|instance| instance.ports.len() == 2
            && instance.ports.iter().all(|port| port.width == 16)));
    sim.set_input("a", &LogicWord::from_u64(32, 0x3020_1000))
        .unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x3f21_1f01));
}

#[test]
fn unpacked_array_slices_connect_array_ports() {
    let mut sim = sim_for(
        "module child(input logic [7:0] in_mem [0:1], \
                      output logic [7:0] out_mem [0:1]); \
             assign out_mem[0] = in_mem[0] + 8'd1; \
             assign out_mem[1] = in_mem[1] ^ 8'h0f; \
         endmodule \
         module top(input logic [31:0] a, output logic [31:0] y); \
             logic [7:0] source [0:3]; \
             logic [7:0] result [0:3]; \
             assign source = a; \
             initial result = 32'd0; \
             child c(.in_mem(source[1:2]), .out_mem(result[2:3])); \
             assign y = result; \
         endmodule",
    );
    sim.set_input("a", &LogicWord::from_u64(32, 0x4030_2010))
        .unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x3f21_0000));

    let error = compile(
        "module top(input logic [7:0] a, output logic [15:0] y); \
             logic [7:0] source [3:0]; \
             assign source = {a, a, a, a}; \
             assign y = source[1:2]; \
         endmodule",
        None,
    )
    .unwrap_err();
    assert!(
        error.contains("does not follow declaration direction"),
        "{error}"
    );
}

#[test]
fn indexed_part_selects_read_and_write_contiguous_ranges() {
    let mut sim = sim_for(
        "module top(input logic [15:0] a, output logic [15:0] y); \
             always @(*) begin \
                 y = 16'd0; \
                 y[6 +: 4] = a[11 -: 4] ^ 4'd3; \
                 y[5 -: 4] = a[8 +: 4] & 4'd5; \
             end \
         endmodule",
    );
    sim.set_input("a", &LogicWord::from_u64(16, 0x9c3f))
        .unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x03d0));

    let error = compile(
        "module top(input logic [7:0] a, output logic [7:0] y); \
             assign y = a[6 +: 0]; \
         endmodule",
        None,
    )
    .unwrap_err();
    assert!(error.contains("width must be positive"), "{error}");
}

#[test]
fn runtime_indexed_part_select_reads_use_variable_start() {
    let mut sim = sim_for(
        "module top(input logic [15:0] a, input logic [3:0] selector, \
                      output logic [3:0] plus, minus); \
             assign plus = a[selector +: 4]; \
             assign minus = a[selector -: 4]; \
         endmodule",
    );
    sim.set_input("a", &LogicWord::from_u64(16, 0xa4c7))
        .unwrap();
    sim.set_input("selector", &LogicWord::from_u64(4, 8))
        .unwrap();
    assert_eq!(sim.read("plus").unwrap().to_u64(), Some(0x4));
    assert_eq!(sim.read("minus").unwrap().to_u64(), Some(0x6));

    sim.set_input("selector", &LogicWord::from_u64(4, 13))
        .unwrap();
    assert!(sim.read("plus").unwrap().to_u64().is_none());
    sim.set_input("selector", &LogicWord::from_u64(4, 2))
        .unwrap();
    assert!(sim.read("minus").unwrap().to_u64().is_none());
}

#[test]
fn runtime_indexed_part_select_writes_blocking_targets() {
    let source = |direction: &str| {
        format!(
            "module top(input logic trigger, input logic [15:0] a, input logic [3:0] selector, \
                          output logic [15:0] y); \
                 initial y = 16'h3210; \
                 always @(posedge trigger) begin \
                     y[selector {direction}: 4] = a[3:0] ^ 4'd3; \
                 end \
             endmodule"
        )
    };
    let mut plus = sim_for(&source("+"));
    plus.set_input("trigger", &LogicWord::from_u64(1, 0))
        .unwrap();
    plus.set_input("a", &LogicWord::from_u64(16, 0x00a5))
        .unwrap();
    plus.set_input("selector", &LogicWord::from_u64(4, 6))
        .unwrap();
    plus.set_input("trigger", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(plus.read("y").unwrap().to_u64(), Some(0x3190));

    let mut minus = sim_for(&source("-"));
    minus
        .set_input("trigger", &LogicWord::from_u64(1, 0))
        .unwrap();
    minus
        .set_input("a", &LogicWord::from_u64(16, 0x00a5))
        .unwrap();
    minus
        .set_input("selector", &LogicWord::from_u64(4, 7))
        .unwrap();
    minus
        .set_input("trigger", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(minus.read("y").unwrap().to_u64(), Some(0x3260));

    plus.set_input("trigger", &LogicWord::from_u64(1, 0))
        .unwrap();
    plus.set_input("selector", &LogicWord::from_u64(4, 13))
        .unwrap();
    plus.set_input("trigger", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(plus.read("y").unwrap().to_u64(), Some(0x3190));
    minus
        .set_input("trigger", &LogicWord::from_u64(1, 0))
        .unwrap();
    minus
        .set_input("selector", &LogicWord::from_u64(4, 2))
        .unwrap();
    minus
        .set_input("trigger", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(minus.read("y").unwrap().to_u64(), Some(0x3260));

    let mut nonblocking = sim_for(
        "module top(input logic clk, input logic [3:0] first, second, a, b, \
                      output logic [15:0] y); \
             initial y = 16'h3210; \
             always @(posedge clk) begin \
                 y[first +: 4] <= a ^ 4'd3; \
                 y[second -: 4] <= b & 4'd5; \
             end \
         endmodule",
    );
    nonblocking
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    nonblocking
        .set_input("first", &LogicWord::from_u64(4, 6))
        .unwrap();
    nonblocking
        .set_input("second", &LogicWord::from_u64(4, 14))
        .unwrap();
    nonblocking
        .set_input("a", &LogicWord::from_u64(4, 0xc))
        .unwrap();
    nonblocking
        .set_input("b", &LogicWord::from_u64(4, 0xa))
        .unwrap();
    nonblocking
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(nonblocking.read("y").unwrap().to_u64(), Some(0x03d0));

    nonblocking
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    nonblocking
        .set_input("first", &LogicWord::from_u64(4, 13))
        .unwrap();
    nonblocking
        .set_input("second", &LogicWord::from_u64(4, 2))
        .unwrap();
    nonblocking
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(nonblocking.read("y").unwrap().to_u64(), Some(0x03d0));
}

#[test]
fn runtime_indexed_part_selects_work_inside_inline_functions() {
    let mut sim = sim_for(
        "module top(input logic [15:0] a, input logic [3:0] selector, \
                      output logic [3:0] plus, minus, local_slice); \
             function logic [3:0] pick(input logic [15:0] value, \
                                       input logic [3:0] selector); \
                 pick = value[selector +: 4]; \
             endfunction \
             function logic [3:0] pick_reverse(input logic [15:0] value, \
                                               input logic [3:0] selector); \
                 pick_reverse = value[selector -: 4]; \
             endfunction \
             function logic [3:0] use_local(input logic [15:0] value, \
                                            input logic [3:0] selector); \
                 logic [15:0] copy; \
                 copy = value; \
                 use_local = copy[selector +: 4]; \
             endfunction \
             assign plus = pick(a, selector); \
             assign minus = pick_reverse(a, selector); \
             assign local_slice = use_local(a, selector); \
         endmodule",
    );
    sim.set_input("a", &LogicWord::from_u64(16, 0xa4c7))
        .unwrap();
    sim.set_input("selector", &LogicWord::from_u64(4, 8))
        .unwrap();
    assert_eq!(sim.read("plus").unwrap().to_u64(), Some(0x4));
    assert_eq!(sim.read("minus").unwrap().to_u64(), Some(0x6));
    assert_eq!(sim.read("local_slice").unwrap().to_u64(), Some(0x4));

    sim.set_input("selector", &LogicWord::from_u64(4, 13))
        .unwrap();
    assert!(sim.read("plus").unwrap().to_u64().is_none());
    assert!(sim.read("local_slice").unwrap().to_u64().is_none());
    sim.set_input("selector", &LogicWord::from_u64(4, 2))
        .unwrap();
    assert!(sim.read("minus").unwrap().to_u64().is_none());
}

#[test]
fn runtime_indexed_part_selects_read_reversed_packed_vectors() {
    let mut sim = sim_for(
        "module top(input logic [0:15] a, input logic [3:0] selector, \
                      output logic [3:0] plus, minus); \
             assign plus = a[selector +: 4]; \
             assign minus = a[selector -: 4]; \
         endmodule",
    );
    sim.set_input("a", &LogicWord::from_u64(16, 0xa4c7))
        .unwrap();
    sim.set_input("selector", &LogicWord::from_u64(4, 8))
        .unwrap();
    assert_eq!(sim.read("plus").unwrap().to_u64(), Some(3));
    assert_eq!(sim.read("minus").unwrap().to_u64(), Some(9));

    sim.set_input("selector", &LogicWord::from_u64(4, 13))
        .unwrap();
    assert!(sim.read("plus").unwrap().to_u64().is_none());
    sim.set_input("selector", &LogicWord::from_u64(4, 2))
        .unwrap();
    assert!(sim.read("minus").unwrap().to_u64().is_none());
}

#[test]
fn runtime_indexed_part_selects_work_in_reversed_function_declarations() {
    let mut sim = sim_for(
        "module top(input logic [0:15] a, input logic [3:0] selector, \
                      output logic [3:0] plus, minus, local_slice); \
             function logic [3:0] pick(input logic [0:15] value, \
                                       input logic [3:0] selector); \
                 pick = value[selector +: 4]; \
             endfunction \
             function logic [3:0] pick_reverse(input logic [0:15] value, \
                                               input logic [3:0] selector); \
                 pick_reverse = value[selector -: 4]; \
             endfunction \
             function logic [3:0] use_local(input logic [0:15] value, \
                                            input logic [3:0] selector); \
                 logic [0:15] copy; \
                 copy = value; \
                 use_local = copy[selector +: 4]; \
             endfunction \
             assign plus = pick(a, selector); \
             assign minus = pick_reverse(a, selector); \
             assign local_slice = use_local(a, selector); \
         endmodule",
    );
    sim.set_input("a", &LogicWord::from_u64(16, 0xa4c7))
        .unwrap();
    sim.set_input("selector", &LogicWord::from_u64(4, 8))
        .unwrap();
    assert_eq!(sim.read("plus").unwrap().to_u64(), Some(3));
    assert_eq!(sim.read("minus").unwrap().to_u64(), Some(9));
    assert_eq!(sim.read("local_slice").unwrap().to_u64(), Some(3));

    sim.set_input("selector", &LogicWord::from_u64(4, 13))
        .unwrap();
    assert!(sim.read("plus").unwrap().to_u64().is_none());
    assert!(sim.read("local_slice").unwrap().to_u64().is_none());
    sim.set_input("selector", &LogicWord::from_u64(4, 2))
        .unwrap();
    assert!(sim.read("minus").unwrap().to_u64().is_none());
}

#[test]
fn reversed_indexed_part_selects_drive_continuous_lvalues() {
    let mut sim = sim_for(
        "module top(input logic [3:0] a, output logic [0:15] y, \
                      output logic [3:0] selected); \
             assign y[0 +: 4] = 4'd2; \
             assign y[4 +: 4] = a ^ 4'd3; \
             assign y[8 +: 4] = 4'd5; \
             assign y[15 -: 4] = 4'd9; \
             assign selected = y[4 +: 4]; \
         endmodule",
    );
    sim.set_input("a", &LogicWord::from_u64(4, 0xa)).unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x49a9));
    assert_eq!(sim.read("selected").unwrap().to_u64(), Some(0x9));
}

#[test]
fn runtime_indexed_part_select_writes_reversed_packed_vectors() {
    let blocking = |direction: &str| {
        format!(
            "module top(input logic trigger, input logic [3:0] selector, \
                          input logic [3:0] a, output logic [0:15] y); \
                 initial y = 16'h3210; \
                 always @(posedge trigger) \
                 y[selector {direction}: 4] = a; \
             endmodule"
        )
    };
    let mut plus = sim_for(&blocking("+"));
    plus.set_input("trigger", &LogicWord::from_u64(1, 0))
        .unwrap();
    plus.set_input("selector", &LogicWord::from_u64(4, 6))
        .unwrap();
    plus.set_input("a", &LogicWord::from_u64(4, 0xc)).unwrap();
    plus.set_input("trigger", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(plus.read("y").unwrap().to_u64(), Some(0x30d0));
    plus.set_input("trigger", &LogicWord::from_u64(1, 0))
        .unwrap();
    plus.set_input("selector", &LogicWord::from_u64(4, 13))
        .unwrap();
    plus.set_input("trigger", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(plus.read("y").unwrap().to_u64(), Some(0x30d0));

    let mut minus = sim_for(&blocking("-"));
    minus
        .set_input("trigger", &LogicWord::from_u64(1, 0))
        .unwrap();
    minus
        .set_input("selector", &LogicWord::from_u64(4, 9))
        .unwrap();
    minus.set_input("a", &LogicWord::from_u64(4, 0xc)).unwrap();
    minus
        .set_input("trigger", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(minus.read("y").unwrap().to_u64(), Some(0x30d0));
    minus
        .set_input("trigger", &LogicWord::from_u64(1, 0))
        .unwrap();
    minus
        .set_input("selector", &LogicWord::from_u64(4, 2))
        .unwrap();
    minus
        .set_input("trigger", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(minus.read("y").unwrap().to_u64(), Some(0x30d0));

    let mut nonblocking = sim_for(
        "module top(input logic trigger, input logic [3:0] first, second, \
                      input logic [3:0] a, b, output logic [0:15] y); \
             initial y = 16'h3210; \
             always @(posedge trigger) begin \
                 y[first +: 4] <= a; \
                 y[second -: 4] <= b; \
             end \
         endmodule",
    );
    nonblocking
        .set_input("trigger", &LogicWord::from_u64(1, 0))
        .unwrap();
    nonblocking
        .set_input("first", &LogicWord::from_u64(4, 2))
        .unwrap();
    nonblocking
        .set_input("second", &LogicWord::from_u64(4, 14))
        .unwrap();
    nonblocking
        .set_input("a", &LogicWord::from_u64(4, 0xa))
        .unwrap();
    nonblocking
        .set_input("b", &LogicWord::from_u64(4, 0x5))
        .unwrap();
    nonblocking
        .set_input("trigger", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(nonblocking.read("y").unwrap().to_u64(), Some(0x1614));
}
