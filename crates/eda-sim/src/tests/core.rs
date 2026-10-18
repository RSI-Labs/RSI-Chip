//! Combinational, sequential, scheduler, hierarchy, and arithmetic tests.

use super::*;

#[test]
fn comb_assign_propagates() {
    let mut sim = sim_for("module m(input a, b, output y); assign y = a & b; endmodule");
    sim.set_input("a", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("b", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(1));
    sim.set_input("b", &LogicWord::from_u64(1, 0)).unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0));
}

#[test]
fn adder_exhaustive() {
    let mut sim =
        sim_for("module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule");
    for x in 0..16u64 {
        for y in 0..16u64 {
            sim.set_input("a", &LogicWord::from_u64(4, x)).unwrap();
            sim.set_input("b", &LogicWord::from_u64(4, y)).unwrap();
            assert_eq!(sim.read("s").unwrap().to_u64(), Some((x + y) & 0xF));
        }
    }
}

#[test]
fn x_pessimism() {
    let mut sim = sim_for("module m(input a, b, output y); assign y = a & b; endmodule");
    assert_eq!(sim.read("y").unwrap().bit(0), LogicValue::X);
    sim.set_input("a", &LogicWord::from_u64(1, 0)).unwrap();
    assert_eq!(sim.read("y").unwrap().bit(0), LogicValue::Zero);
}

#[test]
fn mux_conditional() {
    let mut sim =
        sim_for("module m(input s, [3:0] a, b, output [3:0] y); assign y = s ? a : b; endmodule");
    sim.set_input("s", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("a", &LogicWord::from_u64(4, 0b1010)).unwrap();
    sim.set_input("b", &LogicWord::from_u64(4, 0b0101)).unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0b1010));
    sim.set_input("s", &LogicWord::from_u64(1, 0)).unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(0b0101));
}

#[test]
fn clocked_process_and_nba() {
    let mut sim =
        sim_for("module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule");
    sim.set_input("d", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(sim.read("q").unwrap().bit(0), LogicValue::X);
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(sim.read("q").unwrap().bit(0), LogicValue::One);
    sim.set_input("d", &LogicWord::from_u64(1, 0)).unwrap();
    assert_eq!(sim.read("q").unwrap().bit(0), LogicValue::One);
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(sim.read("q").unwrap().bit(0), LogicValue::Zero);
}

#[test]
fn counter_sequence() {
    let mut sim = sim_for(
        "module m(input clk, rst, output reg [3:0] q);
            always @(posedge clk or posedge rst) if (rst) q <= 4'd0; else q <= q + 4'd1;
         endmodule",
    );
    sim.set_input("rst", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("rst", &LogicWord::from_u64(1, 0)).unwrap();
    for i in 0..10u64 {
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(sim.read("q").unwrap().to_u64(), Some(i + 1));
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    }
}

#[test]
fn comb_always_case() {
    let mut sim = sim_for(
        "module m(input [1:0] sel, output reg [1:0] y);
            always @(*) case (sel)
                2'b00: y = 2'd1;
                2'b01: y = 2'd2;
                2'b10: y = 2'd3;
                default: y = 2'd0;
            endcase
         endmodule",
    );
    for s in 0..4u64 {
        sim.set_input("sel", &LogicWord::from_u64(2, s)).unwrap();
        let expect = match s {
            0 => 1,
            1 => 2,
            2 => 3,
            _ => 0,
        };
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(expect));
    }
}

#[test]
fn comb_always_case_with_continuous_loadpipe() {
    let mut sim = sim_for(
        "module m(input a, b, input [1:0] sel, output y);
            wire axb = a ^ b;
            reg r;
            always @(*) begin
                r = axb;
                case (sel)
                    2'b00: r = axb;
                    2'b01: r = a & b;
                    default: r = a | b;
                endcase
            end
            assign y = r;
         endmodule",
    );
    for (a, b) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
        sim.set_input("a", &LogicWord::from_u64(1, a)).unwrap();
        sim.set_input("b", &LogicWord::from_u64(1, b)).unwrap();
        for sel in 0..3u64 {
            sim.set_input("sel", &LogicWord::from_u64(2, sel)).unwrap();
            let expected = match sel {
                0 => a ^ b,
                1 => a & b,
                _ => a | b,
            };
            assert_eq!(
                sim.read_port("y").unwrap().to_u64(),
                Some(expected),
                "a={a} b={b} sel={sel}"
            );
        }
    }
}

#[test]
fn testbench_display_and_finish() {
    let mut sim = sim_for(
        "module tb;
            reg a;
            initial begin
                a = 0;
                #10 a = 1;
                #5 $display(\"a=%b t=%t\", a, a);
                $display(\"done\");
                $finish;
            end
         endmodule",
    );
    sim.run_until(100).unwrap();
    assert!(sim.finished());
    let out = sim.take_displays();
    assert!(out.iter().any(|l| l.starts_with("a=1 t=1")));
    assert!(out.iter().any(|l| l == "done"));
}

#[test]
fn clock_generator_forever() {
    let mut sim =
        sim_for("module tb; reg clk; initial begin clk = 0; forever #5 clk = ~clk; end endmodule");
    sim.run_until(22).unwrap();
    // toggles at 5,10,15,20 -> after 22 clk is 0 (4 toggles)
    assert_eq!(sim.read("clk").unwrap().to_u64(), Some(0));
    assert_eq!(sim.now(), 22);
}

#[test]
fn scheduler_step_budget_is_per_settle_not_cumulative() {
    let mut sim =
        sim_for("module tb; reg clk; initial begin clk = 0; forever #1 clk = ~clk; end endmodule");
    // More timestamp slots than MAX_STEPS_PER_TIME. Each individual settle
    // is tiny; treating the guard as a cumulative counter would incorrectly
    // report a zero-delay loop after long but legitimate activity.
    sim.run_until(MAX_STEPS_PER_TIME + 1).unwrap();
    assert_eq!(sim.now(), MAX_STEPS_PER_TIME + 1);
}

#[test]
fn delays_and_time() {
    let mut sim = sim_for(
        "module tb; reg a; wire y; assign #7 y = a; initial begin #20 a = 1; end endmodule",
    );
    sim.run_until(19).unwrap();
    assert_eq!(sim.read("a").unwrap().bit(0), LogicValue::X);
    sim.run_until(26).unwrap();
    assert_eq!(sim.read("y").unwrap().bit(0), LogicValue::X);
    sim.run_until(27).unwrap();
    assert_eq!(sim.read("y").unwrap().bit(0), LogicValue::One);
}

#[test]
fn monitor_fires_on_change() {
    let mut sim = sim_for(
        "module tb; reg a; initial a = 0; initial #10 a = 1; initial $monitor(\"a=%b\", a); endmodule",
    );
    sim.run_until(20).unwrap();
    assert!(sim.displays.iter().any(|l| l == "a=0"));
    assert!(sim.displays.iter().any(|l| l == "a=1"));
}

#[test]
fn structural_gate_netlist_sim() {
    use eda_core::netlist::{Cell, CellKind, Dir, Netlist};
    use eda_core::to_design::netlist_to_design;
    let mut nl = Netlist::new("g");
    let a = nl.add_net("a");
    let b = nl.add_net("b");
    let w1 = nl.add_net("w1");
    let y = nl.add_net("y");
    nl.add_cell(Cell {
        name: "u1".into(),
        kind: CellKind::Xor,
        inputs: vec![a, b],
        output: w1,
    });
    nl.add_cell(Cell {
        name: "u2".into(),
        kind: CellKind::Not,
        inputs: vec![w1],
        output: y,
    });
    nl.add_port("a", Dir::In, vec![a]);
    nl.add_port("b", Dir::In, vec![b]);
    nl.add_port("y", Dir::Out, vec![y]);
    let d: &'static Design = Box::leak(Box::new(netlist_to_design(&nl)));
    let mut sim = Simulator::new(d).unwrap();
    for (x, y_) in [(0u64, 0u64), (0, 1), (1, 0), (1, 1)] {
        sim.set_input("a", &LogicWord::from_u64(1, x)).unwrap();
        sim.set_input("b", &LogicWord::from_u64(1, y_)).unwrap();
        let expect = ((x ^ y_) == 0) as u64;
        assert_eq!(sim.read_port("y").unwrap().to_u64(), Some(expect));
    }
}

#[test]
fn fsm_state_sequence() {
    let src = "module fsm(input clk, rst, in, output reg [1:0] state, output reg out);
        always @(posedge clk or posedge rst) begin
            if (rst) state <= 2'b00;
            else case (state)
                2'b00: state <= in ? 2'b01 : 2'b00;
                2'b01: state <= in ? 2'b10 : 2'b00;
                2'b10: state <= in ? 2'b11 : 2'b00;
                2'b11: state <= 2'b00;
            endcase
        end
        always @(*) out = (state == 2'b10);
    endmodule";
    let mut sim = sim_for(src);
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("rst", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("in", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("rst", &LogicWord::from_u64(1, 0)).unwrap();
    let seq = [0u64, 1, 2, 3, 0];
    for (i, expect) in seq.iter().enumerate() {
        if i > 0 {
            sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        }
        assert_eq!(
            sim.read("state").unwrap().to_u64(),
            Some(*expect),
            "step {}",
            i
        );
    }
    assert_eq!(sim.read("out").unwrap().to_u64(), Some(0));
}

#[test]
fn zero_delay_loop_detected() {
    let mut sim = sim_for("module m; reg y; initial y = 1'b0; always @(*) y = ~y; endmodule");
    assert!(sim.run_until(5).is_err());
}

#[test]
fn hierarchical_flatten_sim() {
    let mut sim = sim_for(
        "module inv(input a, output y); assign y = ~a; endmodule
         module top(input x, output z); wire w; inv u1(.a(x), .y(w)); inv u2(.a(w), .y(z)); endmodule",
    );
    sim.set_input("x", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(sim.read_port("z").unwrap().to_u64(), Some(1));
    sim.set_input("x", &LogicWord::from_u64(1, 0)).unwrap();
    assert_eq!(sim.read_port("z").unwrap().to_u64(), Some(0));
}

#[test]
fn hierarchical_clocked_nba() {
    let mut sim = sim_for(
        "module d(input clk, rst, output reg [1:0] q);
            always @(posedge clk) if (rst) q <= 2'd0; else q <= q + 2'd1;
         endmodule
         module top(input clk, rst, output [1:0] q); d u(.clk(clk), .rst(rst), .q(q)); endmodule",
    );
    sim.set_input("rst", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("rst", &LogicWord::from_u64(1, 0)).unwrap();
    for i in 0..4u64 {
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(sim.read_port("q").unwrap().to_u64(), Some((i + 1) & 3));
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    }
}

#[test]
fn wide_hierarchical_vector_load_stays_within_step_budget() {
    let mut sim = sim_for(
        "module leaf(input [1023:0] p, output [1023:0] q); assign q = p; endmodule
         module top(input [1023:0] p, output [1023:0] q); leaf u(.p(p), .q(q)); endmodule",
    );
    let value = LogicWord {
        bits: (0..1024)
            .map(|i| {
                if i % 3 == 0 {
                    LogicValue::One
                } else {
                    LogicValue::Zero
                }
            })
            .collect(),
    };
    sim.set_input("p", &value).unwrap();
    assert_eq!(sim.read_port("q"), Some(value));
}

#[test]
fn clocked_case_with_localparam_label() {
    let mut sim = sim_for(
        "module m(input clk, rst, output reg [2:0] s);
            localparam A = 3'd0;
            always @(posedge clk)
                if (rst) s <= 3'd0;
                else case (s)
                    A: s <= 3'd1;
                    default: s <= 3'd0;
                endcase
         endmodule",
    );
    sim.set_input("rst", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("rst", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(sim.read_port("s").unwrap().to_u64(), Some(1));
}

#[test]
fn time_zero_initial_settles_before_input_edge() {
    let mut sim = sim_for(
        "module m(input clk, output reg [3:0] n);
            initial n = 4'd0;
            always @(posedge clk) n <= n + 4'd1;
         endmodule",
    );
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(sim.read_port("n").unwrap().to_u64(), Some(1));
    sim.set_input("clk", &LogicWord::xes(1)).unwrap();
    assert_eq!(sim.read_port("n").unwrap().to_u64(), Some(1));
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(sim.read_port("n").unwrap().to_u64(), Some(2));
}

#[test]
fn shifts_and_compares() {
    let mut sim = sim_for(
        "module m(input [3:0] a, [1:0] sh, output [3:0] l, r, output lt, eq);
            assign l = a << sh;
            assign r = a >> sh;
            assign lt = a < 4'd9;
            assign eq = (a == 4'd9);
         endmodule",
    );
    sim.set_input("a", &LogicWord::from_u64(4, 0b1001)).unwrap();
    sim.set_input("sh", &LogicWord::from_u64(2, 1)).unwrap();
    assert_eq!(sim.read("l").unwrap().to_u64(), Some(0b0010));
    assert_eq!(sim.read("r").unwrap().to_u64(), Some(0b0100));
    assert_eq!(sim.read("lt").unwrap().to_u64(), Some(0));
    assert_eq!(sim.read("eq").unwrap().to_u64(), Some(1));
}

#[test]
fn signed_declarations_literals_arithmetic_and_shift() {
    let mut sim = sim_for(
        "module m(input logic signed [7:0] a, b, \
         output logic signed [7:0] sum, shifted, quotient, remainder, \
         output logic less);
            assign sum = a + b;
            assign shifted = a >> 1;
            assign quotient = a / b;
            assign remainder = a % b;
            assign less = a < b;
         endmodule",
    );
    sim.set_input("a", &LogicWord::from_u64(8, 0xf9)).unwrap();
    sim.set_input("b", &LogicWord::from_u64(8, 2)).unwrap();
    assert_eq!(sim.read("sum").unwrap().to_u64(), Some(0xfb));
    assert_eq!(sim.read("shifted").unwrap().to_u64(), Some(0xfc));
    assert_eq!(sim.read("quotient").unwrap().to_u64(), Some(0xfd));
    assert_eq!(sim.read("remainder").unwrap().to_u64(), Some(0xff));
    assert_eq!(sim.read("less").unwrap().to_u64(), Some(1));

    let mut sim = sim_for(
        "module m(output logic [7:0] y, output logic less);
            assign y = $signed(8'sd250) + $signed(8'sd7);
            assign less = $signed(8'sh80) < $signed(8'sd0);
         endmodule",
    );
    sim.run_until(1).unwrap();
    assert_eq!(sim.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(sim.read("less").unwrap().to_u64(), Some(1));
}
