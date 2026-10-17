//! RTL-to-netlist behavioral regression tests.

use super::super::*;
use eda_core::logic::LogicWord;
use eda_hdl::compile;
use eda_sim::Simulator;

fn synth(src: &str) -> SynthResult {
    let d = compile(src, None).unwrap();
    synthesize(&d, &SynthOptions::default()).unwrap_or_else(|e| panic!("synth failed: {}", e))
}

#[test]
fn simple_and_maps_to_and2() {
    let r = synth("module m(input a, b, output y); assign y = a & b; endmodule");
    assert_eq!(r.netlist.stats().lib_cells, 1);
    let c = &r.netlist.cells()[0];
    assert!(matches!(&c.kind, CellKind::Lib(id) if r.library.get(*id).name == "AND2"));
}

#[test]
fn constant_folding_removes_logic() {
    let r = synth("module m(input a, output y); assign y = a & 1'b1; endmodule");
    assert_eq!(r.netlist.stats().lib_cells, 0);
    let y = r.netlist.port_by_name("y").unwrap();
    let a = r.netlist.port_by_name("a").unwrap();
    assert_eq!(y.bits, a.bits, "feedthrough should alias the source net");

    let r0 = synth("module m(input a, output y); assign y = a & 1'b0; endmodule");
    assert_eq!(r0.netlist.stats().lib_cells, 1);
    let c0 = &r0.netlist.cells()[0];
    assert!(matches!(&c0.kind, CellKind::Lib(id) if r0.library.get(*id).name == "TIE0"));
}

#[test]
fn duplicate_logic_merged() {
    // port drivers stay separate, but the shared inner cone must merge
    let r = synth(
        "module m(input a, b, c, d, output y, z);
                wire w; assign w = a & b;
                assign y = w | c; assign z = w | d;
             endmodule",
    );
    let ands = r
        .netlist
        .cells()
        .iter()
        .filter(|c| matches!(&c.kind, CellKind::Lib(id) if r.library.get(*id).name == "AND2"))
        .count();
    assert_eq!(ands, 1);
}

#[test]
fn dead_code_eliminated() {
    let r = synth(
        "module m(input a, b, output y); wire dead; assign dead = a ^ b; assign y = a; endmodule",
    );
    assert!(r.netlist.stats().cells <= 1); // y is a pass-through (0 or 1 BUF)
}

#[test]
fn adder_synthesizes_and_matches_rtl() {
    let src = "module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule";
    let r = synth(src);
    let d = compile(src, None).unwrap();
    let gd = eda_core::to_design::netlist_to_design(&r.netlist);
    let mut rtl = Simulator::new(&d).unwrap();
    let mut gate = Simulator::new(&gd).unwrap();
    for x in 0..16u64 {
        for y in 0..16u64 {
            for sim in [&mut rtl, &mut gate] {
                sim.set_input("a", &LogicWord::from_u64(4, x)).unwrap();
                sim.set_input("b", &LogicWord::from_u64(4, y)).unwrap();
            }
            let rv = rtl.read_port("s").unwrap();
            let gv = gate.read_port("s").unwrap();
            assert_eq!(rv.to_u64(), gv.to_u64(), "{}+{}", x, y);
            assert_eq!(rv.to_u64(), Some((x + y) & 0xF));
        }
    }
    assert_eq!(r.netlist.stats().seq, 0);
}

#[test]
fn flop_infers_dff() {
    let r = synth("module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule");
    assert_eq!(r.netlist.stats().seq, 1);
    let c = r
        .netlist
        .cells()
        .iter()
        .find(|c| matches!(c.kind, CellKind::Lib(_)))
        .unwrap();
    let name = match &c.kind {
        CellKind::Lib(id) => r.library.get(*id).name.clone(),
        _ => unreachable!(),
    };
    assert_eq!(name, "DFF");
}

#[test]
fn runtime_indexed_nba_synthesizes_and_simulates() {
    let src = "module m(input clk, input logic [3:0] first, second, \
                          input logic [7:0] a, b, output logic [15:0] y); \
                    always @(posedge clk) begin \
                        y[first +: 8] <= a; \
                        y[second -: 8] <= b; \
                    end \
                  endmodule";
    let r = synth(src);
    assert_eq!(
        r.netlist.stats().seq,
        16,
        "stats={:?}, warnings={:?}",
        r.netlist.stats(),
        r.warnings
    );
    let d = compile(src, None).unwrap();
    let gd = eda_core::to_design::netlist_to_design(&r.netlist);
    let mut rtl = Simulator::new(&d).unwrap();
    let mut gate = Simulator::new(&gd).unwrap();
    for sim in [&mut rtl, &mut gate] {
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("first", &LogicWord::from_u64(4, 0)).unwrap();
        sim.set_input("second", &LogicWord::from_u64(4, 15))
            .unwrap();
        sim.set_input("a", &LogicWord::from_u64(8, 0xa5)).unwrap();
        sim.set_input("b", &LogicWord::from_u64(8, 0x3c)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    }
    let expected = Some(0x3ca5_u64);
    assert_eq!(rtl.read_port("y").unwrap().to_u64(), expected);
    assert_eq!(gate.read_port("y").unwrap().to_u64(), expected);
}

#[test]
fn runtime_array_indexed_nba_synthesizes_and_simulates() {
    let src = "module m(input clk, input logic [2:0] element, \
                          input logic [1:0] packed_selector, input logic [1:0] value, \
                          output logic [3:0] memory [0:3]); \
                    always @(posedge clk) begin \
                        memory[0] <= 4'hA; \
                        memory[1] <= 4'h5; \
                        memory[2] <= 4'h3; \
                        memory[3] <= 4'hC; \
                        memory[element][packed_selector +: 2] <= value ^ 2'h3; \
                    end \
                  endmodule";
    let r = synth(src);
    assert_eq!(r.netlist.stats().seq, 16);
    let d = compile(src, None).unwrap();
    let gd = eda_core::to_design::netlist_to_design(&r.netlist);
    let mut rtl = Simulator::new(&d).unwrap();
    let mut gate = Simulator::new(&gd).unwrap();

    for sim in [&mut rtl, &mut gate] {
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("element", &LogicWord::from_u64(3, 2))
            .unwrap();
        sim.set_input("packed_selector", &LogicWord::from_u64(2, 1))
            .unwrap();
        sim.set_input("value", &LogicWord::from_u64(2, 0b11))
            .unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    }
    let expected = Some(0xC15A_u64);
    assert_eq!(rtl.read_port("memory").unwrap().to_u64(), expected);
    assert_eq!(gate.read_port("memory").unwrap().to_u64(), expected);

    for sim in [&mut rtl, &mut gate] {
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("element", &LogicWord::from_u64(3, 4))
            .unwrap();
        sim.set_input("packed_selector", &LogicWord::from_u64(2, 0))
            .unwrap();
        sim.set_input("value", &LogicWord::from_u64(2, 0b01))
            .unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    }
    let unchanged = Some(0xC35A_u64);
    assert_eq!(rtl.read_port("memory").unwrap().to_u64(), unchanged);
    assert_eq!(gate.read_port("memory").unwrap().to_u64(), unchanged);
}

#[test]
fn reversed_runtime_indexed_nba_synthesizes_and_simulates() {
    let src = "module m(input clk, input logic [3:0] first, second, \
                          input logic [7:0] a, b, output logic [0:15] y); \
                    always @(posedge clk) begin \
                        y[first +: 8] <= a; \
                        y[second -: 8] <= b; \
                    end \
                  endmodule";
    let r = synth(src);
    assert_eq!(r.netlist.stats().seq, 16);
    let d = compile(src, None).unwrap();
    let gd = eda_core::to_design::netlist_to_design(&r.netlist);
    let mut rtl = Simulator::new(&d).unwrap();
    let mut gate = Simulator::new(&gd).unwrap();
    for sim in [&mut rtl, &mut gate] {
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("first", &LogicWord::from_u64(4, 0)).unwrap();
        sim.set_input("second", &LogicWord::from_u64(4, 15))
            .unwrap();
        sim.set_input("a", &LogicWord::from_u64(8, 0xa5)).unwrap();
        sim.set_input("b", &LogicWord::from_u64(8, 0x3c)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    }
    let expected = Some(0xa53c_u64);
    assert_eq!(rtl.read_port("y").unwrap().to_u64(), expected);
    assert_eq!(gate.read_port("y").unwrap().to_u64(), expected);
}

#[test]
fn async_reset_infers_dffr() {
    let r = synth(
        "module m(input clk, rst_n, d, output reg q);
                always @(posedge clk or negedge rst_n) if (!rst_n) q <= 1'b0; else q <= d;
             endmodule",
    );
    assert_eq!(r.netlist.stats().seq, 1);
    let names: Vec<String> = r
        .netlist
        .cells()
        .iter()
        .filter_map(|c| match &c.kind {
            CellKind::Lib(id) => Some(r.library.get(*id).name.clone()),
            _ => None,
        })
        .collect();
    assert!(names.contains(&"DFFR".to_string()), "{:?}", names);
}

#[test]
fn counter_synthesizes_and_simulates() {
    let src = "module m(input clk, rst, output reg [3:0] q);
            always @(posedge clk or posedge rst) if (rst) q <= 4'd0; else q <= q + 4'd1;
         endmodule";
    let r = synth(src);
    assert_eq!(r.netlist.stats().seq, 4);
    let d = compile(src, None).unwrap();
    let gd = eda_core::to_design::netlist_to_design(&r.netlist);
    let mut rtl = Simulator::new(&d).unwrap();
    let mut gate = Simulator::new(&gd).unwrap();
    for sim in [&mut rtl, &mut gate] {
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("rst", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("rst", &LogicWord::from_u64(1, 0)).unwrap();
    }
    for i in 0..16u64 {
        for sim in [&mut rtl, &mut gate] {
            sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        }
        let rv = rtl.read_port("q").unwrap();
        let gv = gate.read_port("q").unwrap();
        assert_eq!(rv.to_u64(), gv.to_u64(), "cycle {}", i);
        assert_eq!(rv.to_u64(), Some((i + 1) & 0xF)); // 4-bit wrap
    }
}

#[test]
fn mux_case_synthesizes() {
    let src = "module m(input [1:0] sel, [3:0] a, b, c, d, output [3:0] y);
            always @(*) case (sel)
                2'd0: y = a;
                2'd1: y = b;
                2'd2: y = c;
                default: y = d;
            endcase
         endmodule";
    let r = synth(src);
    let d = compile(src, None).unwrap();
    let gd = eda_core::to_design::netlist_to_design(&r.netlist);
    let mut rtl = Simulator::new(&d).unwrap();
    let mut gate = Simulator::new(&gd).unwrap();
    let vals = [0u64, 1, 6, 9];
    for s in 0..4u64 {
        for k in 0..4 {
            let (a, b, c, dd) = (
                vals[k],
                vals[(k + 1) % 4],
                vals[(k + 2) % 4],
                vals[(k + 3) % 4],
            );
            for sim in [&mut rtl, &mut gate] {
                sim.set_input("sel", &LogicWord::from_u64(2, s)).unwrap();
                sim.set_input("a", &LogicWord::from_u64(4, a)).unwrap();
                sim.set_input("b", &LogicWord::from_u64(4, b)).unwrap();
                sim.set_input("c", &LogicWord::from_u64(4, c)).unwrap();
                sim.set_input("d", &LogicWord::from_u64(4, dd)).unwrap();
            }
            let expect = [a, b, c, dd][s as usize];
            assert_eq!(rtl.read_port("y").unwrap().to_u64(), Some(expect));
            assert_eq!(gate.read_port("y").unwrap().to_u64(), Some(expect));
        }
    }
}

#[test]
fn barrel_shifter_synthesizes() {
    let src = "module m(input [3:0] a, [1:0] sh, output [3:0] y); assign y = a << sh; endmodule";
    let r = synth(src);
    let d = compile(src, None).unwrap();
    let gd = eda_core::to_design::netlist_to_design(&r.netlist);
    let mut rtl = Simulator::new(&d).unwrap();
    let mut gate = Simulator::new(&gd).unwrap();
    for a in 0..16u64 {
        for sh in 0..4u64 {
            for sim in [&mut rtl, &mut gate] {
                sim.set_input("a", &LogicWord::from_u64(4, a)).unwrap();
                sim.set_input("sh", &LogicWord::from_u64(2, sh)).unwrap();
            }
            assert_eq!(
                rtl.read_port("y").unwrap().to_u64(),
                gate.read_port("y").unwrap().to_u64()
            );
            assert_eq!(rtl.read_port("y").unwrap().to_u64(), Some(a << sh & 0xF));
        }
    }
}

#[test]
fn multiplier_synthesizes() {
    let src = "module m(input [3:0] a, b, output [3:0] y); assign y = a * b; endmodule";
    let r = synth(src);
    let d = compile(src, None).unwrap();
    let gd = eda_core::to_design::netlist_to_design(&r.netlist);
    let mut rtl = Simulator::new(&d).unwrap();
    let mut gate = Simulator::new(&gd).unwrap();
    for a in 0..16u64 {
        for b in 0..16u64 {
            for sim in [&mut rtl, &mut gate] {
                sim.set_input("a", &LogicWord::from_u64(4, a)).unwrap();
                sim.set_input("b", &LogicWord::from_u64(4, b)).unwrap();
            }
            assert_eq!(
                gate.read_port("y").unwrap().to_u64(),
                Some((a * b) & 0xF),
                "{}*{}",
                a,
                b
            );
            assert_eq!(
                rtl.read_port("y").unwrap().to_u64(),
                gate.read_port("y").unwrap().to_u64()
            );
        }
    }
}

#[test]
fn signed_arithmetic_synthesizes_and_simulates() {
    let src = "module m(input logic signed [7:0] a, b, \
                          input logic [2:0] shift, \
                          input logic signed [5:0] c, \
                          output logic signed [7:0] sum, diff, product, shifted, selected, \
                          output logic signed [7:0] mixed, \
                          output logic lt, le, gt, ge, mixed_lt, eq_debug); \
                    assign sum = a + b; \
                    assign diff = a - b; \
                    assign product = a * b; \
                    assign shifted = a >> shift; \
                    assign selected = (a < b) ? a : b; \
                    assign mixed = $signed(a) + $signed(c); \
                    assign lt = a < b; \
                    assign le = a <= b; \
                    assign gt = a > b; \
                    assign ge = a >= b; \
                    assign mixed_lt = $signed(a) < $signed(c); \
                    assign eq_debug = a == b; \
                  endmodule";
    let r = synth(src);
    assert_eq!(r.netlist.stats().seq, 0);
    let d = compile(src, None).unwrap();
    let gd = eda_core::to_design::netlist_to_design(&r.netlist);
    let mut rtl = Simulator::new(&d).unwrap();
    let mut gate = Simulator::new(&gd).unwrap();
    for sim in [&mut rtl, &mut gate] {
        sim.set_input("a", &LogicWord::from_u64(8, (-5_i8) as u8 as u64))
            .unwrap();
        sim.set_input("b", &LogicWord::from_u64(8, 7)).unwrap();
        sim.set_input("shift", &LogicWord::from_u64(3, 2)).unwrap();
        sim.set_input("c", &LogicWord::from_u64(6, (-7_i8) as u8 as u64))
            .unwrap();
    }
    for (name, expected) in [
        ("sum", 0x02_u64),
        ("diff", 0xf4),
        ("product", 0xdd),
        ("shifted", 0xfe),
        ("selected", 0xfb),
        ("mixed", 0xf4),
        ("lt", 1),
        ("le", 1),
        ("gt", 0),
        ("ge", 0),
        ("mixed_lt", 0),
        ("eq_debug", 0),
    ] {
        assert_eq!(
            rtl.read_port(name).unwrap().to_u64(),
            Some(expected),
            "RTL {name}"
        );
        assert_eq!(
            gate.read_port(name).unwrap().to_u64(),
            Some(expected),
            "gate {name}"
        );
    }
}

#[test]
fn nand_peephole() {
    let r = synth("module m(input a, b, output y); assign y = ~(a & b); endmodule");
    let names: Vec<String> = r
        .netlist
        .cells()
        .iter()
        .filter_map(|c| match &c.kind {
            CellKind::Lib(id) => Some(r.library.get(*id).name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["NAND2".to_string()]);
}

#[test]
fn fsm_synthesizes_and_matches() {
    let src = "module fsm(input clk, rst, go, output reg [1:0] state, output reg done);
            always @(posedge clk or posedge rst) begin
                if (rst) state <= 2'b00;
                else case (state)
                    2'b00: state <= go ? 2'b01 : 2'b00;
                    2'b01: state <= go ? 2'b10 : 2'b00;
                    2'b10: state <= go ? 2'b11 : 2'b00;
                    2'b11: state <= 2'b00;
                endcase
            end
            always @(*) done = (state == 2'b10);
        endmodule";
    let r = synth(src);
    assert_eq!(r.netlist.stats().seq, 2);
    let d = compile(src, None).unwrap();
    let gd = eda_core::to_design::netlist_to_design(&r.netlist);
    let mut rtl = Simulator::new(&d).unwrap();
    let mut gate = Simulator::new(&gd).unwrap();
    for sim in [&mut rtl, &mut gate] {
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("rst", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("go", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("rst", &LogicWord::from_u64(1, 0)).unwrap();
    }
    for i in 0..6u64 {
        for sim in [&mut rtl, &mut gate] {
            sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        }
        assert_eq!(
            rtl.read_port("state").unwrap().to_u64(),
            gate.read_port("state").unwrap().to_u64(),
            "cycle {}",
            i
        );
        assert_eq!(
            rtl.read_port("done").unwrap().to_u64(),
            gate.read_port("done").unwrap().to_u64()
        );
    }
}

#[test]
fn netlist_writer_round_trips() {
    let r = synth(
        "module m(input clk, rst, d, output reg q, nq);
            always @(posedge clk or posedge rst) if (rst) q <= 1'b0; else q <= d;
            assign nq = ~q;
         endmodule",
    );
    let text = write_verilog(&r.netlist, &r.library);
    let d = compile(&text, None).unwrap();
    let r2 = synthesize(&d, &SynthOptions::default()).unwrap();
    assert_eq!(r2.netlist.stats().seq, 1);
}

#[test]
fn unmapped_option_keeps_generic_cells() {
    let d = compile(
        "module m(input a, b, output y); assign y = a & b; endmodule",
        None,
    )
    .unwrap();
    let r = synthesize(
        &d,
        &SynthOptions {
            optimize: true,
            map_lib: false,
        },
    )
    .unwrap();
    let s = r.netlist.stats();
    assert_eq!(s.lib_cells, 0);
    assert!(s.comb >= 1);
}

#[test]
fn xor_x_self_folds_to_zero() {
    let r = synth("module m(input a, output y); assign y = a ^ a; endmodule");
    assert_eq!(r.netlist.stats().lib_cells, 1);
    let c = &r.netlist.cells()[0];
    assert!(matches!(&c.kind, CellKind::Lib(id) if r.library.get(*id).name == "TIE0"));
}

#[test]
fn undriven_output_is_error() {
    let d = compile(
        "module m(input a, output y, z); assign y = a; endmodule",
        None,
    )
    .unwrap();
    assert!(synthesize(&d, &SynthOptions::default()).is_err());
}

#[test]
fn multi_driver_is_error() {
    let d = compile(
        "module m(input a, b, output y); assign y = a; assign y = b; endmodule",
        None,
    )
    .unwrap();
    assert!(synthesize(&d, &SynthOptions::default()).is_err());
}

#[test]
fn conditional_and_logical_ops() {
    let src = "module m(input [1:0] a, b, output y);
            assign y = (a == 2'd3) && (b != 2'd0);
         endmodule";
    let r = synth(src);
    let d = compile(src, None).unwrap();
    let gd = eda_core::to_design::netlist_to_design(&r.netlist);
    let mut rtl = Simulator::new(&d).unwrap();
    let mut gate = Simulator::new(&gd).unwrap();
    for a in 0..4u64 {
        for b in 0..4u64 {
            for sim in [&mut rtl, &mut gate] {
                sim.set_input("a", &LogicWord::from_u64(2, a)).unwrap();
                sim.set_input("b", &LogicWord::from_u64(2, b)).unwrap();
            }
            let expect = (a == 3 && b != 0) as u64;
            assert_eq!(rtl.read_port("y").unwrap().to_u64(), Some(expect));
            assert_eq!(gate.read_port("y").unwrap().to_u64(), Some(expect));
        }
    }
}
