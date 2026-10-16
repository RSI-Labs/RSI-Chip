//! Function locals, array updates, disable-iff, virtual interfaces, and VCD tests.

use super::*;

#[test]
fn function_multidimensional_unpacked_array_local_updates_and_reads() {
    let mut sim = sim_for(
        "module top(input logic [7:0] a, b, output logic [31:0] y); \
             function logic [31:0] combine(input logic [7:0] left, right); \
                 logic [7:0] memory [1:0][2:3]; \
                 memory[1][3] = left + right; \
                 memory[1][2] = left ^ right; \
                 memory[0][3] = left & right; \
                 memory[0][2] = left | right; \
                 combine = {memory[1][3], memory[1][2], \
                            memory[0][3], memory[0][2]}; \
             endfunction \
             assign y = combine(a, b); \
         endmodule",
    );
    sim.set_input("a", &LogicWord::from_u64(8, 0x5a)).unwrap();
    sim.set_input("b", &LogicWord::from_u64(8, 0xa5)).unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0xffff_00ff));
}

#[test]
fn function_unpacked_array_elements_support_indexed_part_selects() {
    let mut sim = sim_for(
        "module top(input logic [3:0] selector, input logic [3:0] a, \
                      input logic [1:0] element_selector, \
                      output logic [3:0] selected, dynamic, \
                      output logic [7:0] updated); \
             function logic [3:0] pick(input logic [1:0] element_selector, \
                                       input logic [3:0] selector); \
                 logic [7:0] memory [0:2]; \
                 memory[0] = 8'h5a; \
                 memory[1] = 8'ha5; \
                 memory[2] = 8'h3c; \
                 pick = memory[element_selector][selector +: 4]; \
             endfunction \
             function logic [7:0] update(input logic [3:0] selector, \
                                         input logic [3:0] value); \
                 logic [7:0] memory [0:1]; \
                 memory[0] = 8'h5a; \
                 memory[0][selector -: 4] = value ^ 4'd3; \
                 update = memory[0]; \
             endfunction \
             assign selected = pick(element_selector, selector); \
             assign dynamic = pick(element_selector, selector); \
             assign updated = update(selector, a); \
         endmodule",
    );
    sim.set_input("selector", &LogicWord::from_u64(4, 4))
        .unwrap();
    sim.set_input("element_selector", &LogicWord::from_u64(2, 0))
        .unwrap();
    sim.set_input("a", &LogicWord::from_u64(4, 0xc)).unwrap();
    assert_eq!(sim.read("selected").unwrap().to_u64(), Some(0x5));
    assert_eq!(sim.read("dynamic").unwrap().to_u64(), Some(0x5));
    assert_eq!(sim.read("updated").unwrap().to_u64(), Some(0x5e));

    sim.set_input("selector", &LogicWord::from_u64(4, 3))
        .unwrap();
    sim.set_input("element_selector", &LogicWord::from_u64(2, 1))
        .unwrap();
    assert_eq!(sim.read("selected").unwrap().to_u64(), Some(0x4));
    assert_eq!(sim.read("dynamic").unwrap().to_u64(), Some(0x4));
    assert_eq!(sim.read("updated").unwrap().to_u64(), Some(0x5f));

    sim.set_input("selector", &LogicWord::from_u64(4, 13))
        .unwrap();
    assert!(sim.read("selected").unwrap().to_u64().is_none());
    sim.set_input("selector", &LogicWord::from_u64(4, 4))
        .unwrap();
    sim.set_input("element_selector", &LogicWord::from_u64(2, 3))
        .unwrap();
    assert!(sim.read("selected").unwrap().to_u64().is_none());
    assert_eq!(sim.read("updated").unwrap().to_u64(), Some(0x5e));
}

#[test]
fn function_runtime_array_elements_support_indexed_assignments() {
    let mut sim = sim_for(
        "module top(input logic [1:0] element_selector, \
                      input logic [3:0] packed_selector, \
                      input logic [3:0] value, output logic [23:0] whole); \
             function logic [23:0] update(input logic [1:0] element_selector, \
                                          input logic [3:0] packed_selector, \
                                          input logic [3:0] value); \
                 logic [7:0] memory [0:2]; \
                 memory[0] = 8'h12; \
                 memory[1] = 8'h34; \
                 memory[2] = 8'h56; \
                 memory[element_selector][packed_selector -: 4] = value ^ 4'd3; \
                 update = {memory[2], memory[1], memory[0]}; \
             endfunction \
             assign whole = update(element_selector, packed_selector, value); \
         endmodule",
    );
    sim.set_input("value", &LogicWord::from_u64(4, 0xc))
        .unwrap();
    sim.set_input("packed_selector", &LogicWord::from_u64(4, 3))
        .unwrap();
    sim.set_input("element_selector", &LogicWord::from_u64(2, 1))
        .unwrap();
    assert_eq!(sim.read("whole").unwrap().to_u64(), Some(0x56_3f_12));

    sim.set_input("element_selector", &LogicWord::from_u64(2, 3))
        .unwrap();
    sim.set_input("packed_selector", &LogicWord::from_u64(4, 4))
        .unwrap();
    sim.set_input("value", &LogicWord::from_u64(4, 0)).unwrap();
    assert_eq!(sim.read("whole").unwrap().to_u64(), Some(0x56_34_12));

    sim.set_input("element_selector", &LogicWord::from_u64(2, 2))
        .unwrap();
    sim.set_input("packed_selector", &LogicWord::from_u64(4, 13))
        .unwrap();
    sim.set_input("value", &LogicWord::from_u64(4, 0xa))
        .unwrap();
    assert_eq!(sim.read("whole").unwrap().to_u64(), Some(0x56_34_12));
}

#[test]
fn runtime_array_element_targets_support_indexed_assignments() {
    let source = |assignment: &str| {
        format!(
            "module top(input logic clk, input logic [1:0] element_selector, \
                          input logic [3:0] packed_selector, \
                          input logic [3:0] value, output logic [23:0] whole); \
                 logic [7:0] memory [0:2]; \
                 initial begin \
                     memory[0] = 8'h12; \
                     memory[1] = 8'h34; \
                     memory[2] = 8'h56; \
                 end \
                 always @(posedge clk) \
                     memory[element_selector][packed_selector -: 4] \
                     {assignment} value ^ 4'd3; \
                 assign whole = {{memory[2], memory[1], memory[0]}}; \
             endmodule"
        )
    };
    for assignment in ["=", "<="] {
        let mut sim = sim_for(&source(assignment));
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("element_selector", &LogicWord::from_u64(2, 1))
            .unwrap();
        sim.set_input("packed_selector", &LogicWord::from_u64(4, 3))
            .unwrap();
        sim.set_input("value", &LogicWord::from_u64(4, 0xc))
            .unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(sim.read("whole").unwrap().to_u64(), Some(0x56_3f_12));

        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("element_selector", &LogicWord::from_u64(2, 3))
            .unwrap();
        sim.set_input("packed_selector", &LogicWord::from_u64(4, 13))
            .unwrap();
        sim.set_input("value", &LogicWord::from_u64(4, 0)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(sim.read("whole").unwrap().to_u64(), Some(0x56_3f_12));
    }
}

#[test]
fn runtime_indexed_part_selects_drive_continuous_lvalues() {
    let source = |direction: &str, reversed: bool| {
        let vector_range = if reversed { "[0:15]" } else { "[15:0]" };
        format!(
            "module top(input logic [3:0] selector, input logic [3:0] value, \
                          output logic {vector_range} y, output logic [3:0] selected); \
                 initial y = 16'h3210; \
                 assign y[selector {direction}: 4] = value ^ 4'd3; \
                 assign selected = y[selector {direction}: 4]; \
             endmodule"
        )
    };

    let mut plus = sim_for(&source("+", false));
    assert_eq!(plus.design.processes.len(), 1);
    assert_eq!(plus.design.assigns.len(), 1);
    plus.set_input("value", &LogicWord::from_u64(4, 0xc))
        .unwrap();
    plus.set_input("selector", &LogicWord::from_u64(4, 6))
        .unwrap();
    assert_eq!(plus.read("y").unwrap().to_u64(), Some(0x33d0));
    assert_eq!(plus.read("selected").unwrap().to_u64(), Some(0xf));

    plus.set_input("selector", &LogicWord::from_u64(4, 13))
        .unwrap();
    assert_eq!(plus.read("y").unwrap().to_u64(), Some(0x33d0));
    assert!(plus.read("selected").unwrap().to_u64().is_none());

    let mut minus = sim_for(&source("-", false));
    minus
        .set_input("value", &LogicWord::from_u64(4, 0xc))
        .unwrap();
    minus
        .set_input("selector", &LogicWord::from_u64(4, 7))
        .unwrap();
    assert_eq!(minus.read("y").unwrap().to_u64(), Some(0x32f0));
    assert_eq!(minus.read("selected").unwrap().to_u64(), Some(0xf));

    minus
        .set_input("value", &LogicWord::from_u64(4, 0))
        .unwrap();
    assert_eq!(minus.read("y").unwrap().to_u64(), Some(0x3230));
    assert_eq!(minus.read("selected").unwrap().to_u64(), Some(0x3));

    minus
        .set_input("selector", &LogicWord::from_u64(4, 2))
        .unwrap();
    assert_eq!(minus.read("y").unwrap().to_u64(), Some(0x3230));
    assert!(minus.read("selected").unwrap().to_u64().is_none());

    let mut reversed = sim_for(&source("+", true));
    reversed
        .set_input("value", &LogicWord::from_u64(4, 0xc))
        .unwrap();
    reversed
        .set_input("selector", &LogicWord::from_u64(4, 6))
        .unwrap();
    assert_eq!(reversed.read("y").unwrap().to_u64(), Some(0x33d0));
    assert_eq!(reversed.read("selected").unwrap().to_u64(), Some(0xf));

    reversed
        .set_input("selector", &LogicWord::from_u64(4, 13))
        .unwrap();
    assert_eq!(reversed.read("y").unwrap().to_u64(), Some(0x33d0));
    assert!(reversed.read("selected").unwrap().to_u64().is_none());
}

#[test]
fn runtime_array_element_targets_support_continuous_drives() {
    let mut sim = sim_for(
        "module top(input logic [2:0] element_selector, read_element, \
                      input logic [1:0] value, output logic [3:0] y); \
             logic [3:0] memory [0:3]; \
             initial begin \
                 memory[0] = 4'hA; \
                 memory[1] = 4'h5; \
                 memory[2] = 4'h3; \
                 memory[3] = 4'hC; \
             end \
             assign memory[element_selector][1 +: 2] = value ^ 2'h3; \
             assign y = memory[read_element][0 +: 4]; \
         endmodule",
    );
    assert_eq!(sim.design.processes.len(), 1);
    assert_eq!(sim.design.assigns.len(), 1);

    sim.set_input("element_selector", &LogicWord::from_u64(3, 2))
        .unwrap();
    sim.set_input("read_element", &LogicWord::from_u64(3, 2))
        .unwrap();
    sim.set_input("value", &LogicWord::from_u64(2, 3)).unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x1));

    sim.set_input("value", &LogicWord::from_u64(2, 0)).unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x7));

    sim.set_input("element_selector", &LogicWord::from_u64(3, 1))
        .unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x7));

    sim.set_input("element_selector", &LogicWord::from_u64(3, 4))
        .unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x7));

    sim.set_input("read_element", &LogicWord::from_u64(3, 1))
        .unwrap();
    sim.set_input("element_selector", &LogicWord::from_u64(3, 1))
        .unwrap();
    sim.set_input("value", &LogicWord::from_u64(2, 3)).unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x1));

    sim.set_input("element_selector", &LogicWord::from_u64(3, 4))
        .unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x1));
}

#[test]
fn function_multidimensional_array_whole_local_layout() {
    let mut sim = sim_for(
        "module top(input logic dummy, output logic [7:0] y); \
             function logic [7:0] whole(input logic unused); \
                 logic [1:0] memory [0:1][0:1]; \
                 memory[0][0] = 0; memory[0][1] = 1; \
                 memory[1][0] = 2; memory[1][1] = 3; \
                 whole = memory; \
             endfunction \
             assign y = whole(dummy); \
         endmodule",
    );
    sim.set_input("dummy", &LogicWord::from_u64(1, 0)).unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0b0001_1011));
}

#[test]
fn disable_iff_clears_pending_property_attempts() {
    let mut sim = sim_for(
        "module m(input clk, reset, req, grant);
            assert property (@(posedge clk) disable iff (reset)
                req |-> ##[2] grant)
                else $error(\"disabled attempt must not fire\");
         endmodule",
    );
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("reset", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("grant", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("reset", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("req", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("reset", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
}

#[test]
fn disable_iff_aborts_goto_repetition_attempts() {
    let mut sim = sim_for(
        "module m(input clk, reset, req, grant);
            assert property (@(posedge clk) disable iff (reset)
                req |-> grant[->3])
                else $error(\"disabled goto attempt must not fire\");
         endmodule",
    );
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("reset", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("req", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("reset", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("reset", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
}

#[test]
fn static_virtual_interface_ports_share_typed_bindings() {
    let mut sim = sim_for(
        "interface bus;
            logic [7:0] data;
            logic valid;
            modport source(output valid, data);
            modport sink(input valid, data);
         endinterface

         module producer(virtual interface bus.source b,
             input logic [7:0] value);
            assign b.valid = 1'b1;
            assign b.data = value;
         endmodule

         module consumer(virtual interface bus.sink b,
             output logic [7:0] result);
            assign result = b.valid ? b.data : 8'd0;
         endmodule

         module top(input logic [7:0] value, output logic [7:0] result);
            bus shared();
            producer p(.b(shared), .value(value));
            consumer c(.b(shared), .result(result));
         endmodule",
    );
    sim.set_input("value", &LogicWord::from_u64(8, 0x5a))
        .unwrap();
    assert_eq!(sim.read("result").unwrap().to_u64(), Some(0x5a));
}

#[test]
fn snapshot_and_vcd() {
    let mut sim = sim_for("module m(input a, output y); assign y = ~a; endmodule");
    sim.enable_vcd();
    sim.set_input("a", &LogicWord::from_u64(1, 1)).unwrap();
    sim.run_until(5).unwrap();
    let v = sim.vcd().unwrap();
    assert!(v.contains("$enddefinitions"));
    assert!(v.contains("$var"));
    let snap = sim.snapshot();
    assert!(snap.iter().any(|(n, _)| n == "y"));
}

#[test]
fn instance_with_delay_feedback() {
    // D flip-flop built structurally from processes across module boundary
    let mut sim = sim_for(
        "module dff(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule
         module top(input clk, d, output q);
            wire w; dff u1(.clk(clk), .d(d), .q(w));
            assign q = w;
         endmodule",
    );
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("d", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(sim.read_port("q").unwrap().to_u64(), Some(1));
}
