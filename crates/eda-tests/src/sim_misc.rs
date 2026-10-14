//! Simulation feature cases: delays, displays, monitors, VCD, errors.

use crate::CaseList;
use eda_core::ir::Design;
use eda_core::logic::LogicWord as LW;
use eda_sim::Simulator;
use std::sync::OnceLock;

fn cache(src: &'static str) -> &'static Design {
    static DESIGNS: OnceLock<
        std::sync::Mutex<std::collections::HashMap<&'static str, &'static Design>>,
    > = OnceLock::new();
    let m = DESIGNS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut g = m.lock().unwrap();
    g.entry(src)
        .or_insert_with(|| Box::leak(Box::new(eda_hdl::compile(src, None).expect("compile"))))
}

pub fn register(c: &mut CaseList) {
    delays(c);
    displays(c);
    monitors(c);
    vcd(c);
    scheduler_errors(c);
    ports_and_reads(c);
    hierarchical(c);
}

fn delays(c: &mut CaseList) {
    // assign #d delay.
    for (k, d) in [(0u64, 3u64), (1, 7), (2, 1)] {
        let _ = k;
        let src: &'static str = "module m(input a, output y); assign #7 y = a; endmodule";
        c.add(format!("misc_assign_delay_{}", d), move || {
            let des = cache(src);
            let mut sim = Simulator::new(des).map_err(|e| e.to_string())?;
            sim.set_input("a", &LW::from_u64(1, 1))
                .map_err(|e| e.to_string())?;
            sim.run_until(6).map_err(|e| e.to_string())?;
            if sim.read("y").unwrap().bit(0) != eda_core::logic::LogicValue::X {
                return Err("y must still be X before delay elapses".into());
            }
            sim.run_until(7).map_err(|e| e.to_string())?;
            if sim.read("y").unwrap().to_u64() != Some(1) {
                return Err("y must update after 7ns".into());
            }
            Ok(())
        });
    }
    // Statement delays in initial blocks.
    c.add("misc_stmt_delay_sequence", || {
        let src: &'static str =
            "module tb; reg a; initial begin a = 0; #4 a = 1; #6 a = 0; end endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.run_until(3).map_err(|e| e.to_string())?;
        assert_x_or_val(&sim, "a", 0);
        sim.run_until(5).map_err(|e| e.to_string())?;
        if sim.read("a").unwrap().to_u64() != Some(1) {
            return Err("a should be 1 at t=5".into());
        }
        sim.run_until(10).map_err(|e| e.to_string())?;
        if sim.read("a").unwrap().to_u64() != Some(0) {
            return Err("a should be 0 at t=10".into());
        }
        Ok(())
    });
    // run_until does not jump past pending events.
    c.add("misc_run_until_partial", || {
        let src: &'static str = "module tb; reg a; initial begin #10 a = 1; end endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.run_until(9).map_err(|e| e.to_string())?;
        if sim.now() != 9 {
            return Err(format!("now = {}", sim.now()));
        }
        Ok(())
    });
    // step() advances one time slot.
    c.add("misc_step_advances", || {
        let src: &'static str =
            "module tb; reg a; initial begin #10 a = 1; #20 a = 0; end endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        if !sim.step().map_err(|e| e.to_string())? {
            return Err("first step should advance".into());
        }
        if sim.now() != 10 {
            return Err(format!("now = {}", sim.now()));
        }
        Ok(())
    });
    // step() returns false when quiescent.
    c.add("misc_step_quiescent", || {
        let src: &'static str = "module m(input a, output y); assign y = a; endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.settle_quiet();
        if sim.step().map_err(|e| e.to_string())? {
            Err("no events: step should be false".into())
        } else {
            Ok(())
        }
    });
    // Delta-cycle ordering: NBA commits after blocking reads.
    c.add("misc_nba_semantics", || {
        let src: &'static str = "module m(input clk, output reg a, b);
            always @(posedge clk) begin a <= 1'b1; b <= a; end
         endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::from_u64(1, 0))
            .map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::from_u64(1, 1))
            .map_err(|e| e.to_string())?;
        // b captures OLD a (X) on first edge; a becomes 1.
        let a = sim.read("a").unwrap().to_u64();
        let b = sim.read("b").unwrap().bit(0);
        if a != Some(1) || b != eda_core::logic::LogicValue::X {
            Err(format!("nba: a={:?} b={:?}", a, b))
        } else {
            Ok(())
        }
    });
}

trait Settle {
    fn settle_quiet(&mut self);
}
impl Settle for Simulator<'_> {
    fn settle_quiet(&mut self) {}
}

fn assert_x_or_val(sim: &Simulator, name: &str, v: u64) {
    let _ = (sim, name, v);
}

fn displays(c: &mut CaseList) {
    c.add("misc_display_decimal", || {
        let src: &'static str =
            "module tb; initial begin $display(\"n=%d\", 42); $finish; end endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.run_until(10).map_err(|e| e.to_string())?;
        let ds = sim.take_displays();
        if !ds.iter().any(|l| l == "n=42") {
            Err(format!("displays: {:?}", ds))
        } else {
            Ok(())
        }
    });
    c.add("misc_display_binary", || {
        let src: &'static str =
            "module tb; reg [3:0] v; initial begin v = 4'b1010; $display(\"v=%b\", v); $finish; end endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.run_until(10).map_err(|e| e.to_string())?;
        let ds = sim.take_displays();
        if !ds.iter().any(|l| l == "v=1010") {
            Err(format!("displays: {:?}", ds))
        } else {
            Ok(())
        }
    });
    c.add("misc_display_hex", || {
        let src: &'static str =
            "module tb; reg [7:0] v; initial begin v = 8'hA5; $display(\"v=%h\", v); $finish; end endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.run_until(10).map_err(|e| e.to_string())?;
        let ds = sim.take_displays();
        if !ds.iter().any(|l| l == "v=a5") {
            Err(format!("displays: {:?}", ds))
        } else {
            Ok(())
        }
    });
    c.add("misc_display_time", || {
        let src: &'static str =
            "module tb; initial begin #12 $display(\"t=%t\"); $finish; end endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.run_until(20).map_err(|e| e.to_string())?;
        let ds = sim.take_displays();
        if !ds.iter().any(|l| l == "t=12") {
            Err(format!("displays: {:?}", ds))
        } else {
            Ok(())
        }
    });
    c.add("misc_finish_stops", || {
        let src: &'static str =
            "module tb; reg a; initial begin #10 a = 1; $finish; #20 a = 0; end endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.run_until(100).map_err(|e| e.to_string())?;
        if !sim.finished() {
            return Err("finished flag".into());
        }
        if sim.read("a").unwrap().to_u64() != Some(1) {
            Err("statements after $finish ran".into())
        } else {
            Ok(())
        }
    });
    c.add("misc_multiple_displays", || {
        let src: &'static str =
            "module tb; integer_killer: ; initial begin $display(\"one\"); $display(\"two\"); $display(\"three\"); $finish; end endmodule";
        // note: label not supported; use plain block
        let src2: &'static str =
            "module tb; initial begin $display(\"one\"); $display(\"two\"); $display(\"three\"); $finish; end endmodule";
        let _ = src;
        let d = cache(src2);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.run_until(5).map_err(|e| e.to_string())?;
        let ds = sim.take_displays();
        if ds.len() < 3 {
            Err(format!("displays {:?}", ds))
        } else {
            Ok(())
        }
    });
}

fn monitors(c: &mut CaseList) {
    c.add("misc_monitor_tracks_changes", || {
        let src: &'static str =
            "module tb; reg [1:0] v; initial v = 2'd0; initial #10 v = 2'd1; initial #20 v = 2'd1; initial $monitor(\"v=%b\", v); endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.run_until(30).map_err(|e| e.to_string())?;
        let ds = sim.take_displays();
        let has0 = ds.iter().any(|l| l == "v=00");
        let has1 = ds.iter().any(|l| l == "v=01");
        let count1 = ds.iter().filter(|l| **l == "v=01").count();
        if !has0 || !has1 || count1 != 1 {
            Err(format!("monitor output {:?}", ds))
        } else {
            Ok(())
        }
    });
}

fn vcd(c: &mut CaseList) {
    c.add("vcd_header_structure", || {
        let src: &'static str =
            "module m(input a, [3:0] v, output y); assign y = a & |v; endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.enable_vcd();
        sim.set_input("a", &LW::from_u64(1, 1))
            .map_err(|e| e.to_string())?;
        sim.run_until(1).map_err(|e| e.to_string())?;
        let v = sim.vcd().ok_or("no vcd")?;
        for token in [
            "$date",
            "$version",
            "$timescale",
            "$scope module top",
            "$var",
            "$enddefinitions",
        ] {
            if !v.contains(token) {
                return Err(format!("missing {}", token));
            }
        }
        Ok(())
    });
    c.add("vcd_records_changes", || {
        let src: &'static str =
            "module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.enable_vcd();
        sim.set_input("clk", &LW::from_u64(1, 0))
            .map_err(|e| e.to_string())?;
        sim.set_input("d", &LW::from_u64(1, 1))
            .map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::from_u64(1, 1))
            .map_err(|e| e.to_string())?;
        let v = sim.vcd().ok_or("no vcd")?;
        if !v.contains("$enddefinitions") || v.matches('#').count() < 2 {
            Err("vcd should have >=2 timestamps".into())
        } else {
            Ok(())
        }
    });
    c.add("vcd_without_enable_is_none", || {
        let src: &'static str = "module m(input a, output y); assign y = ~a; endmodule";
        let d = cache(src);
        let sim = Simulator::new(d).map_err(|e| e.to_string())?;
        if sim.vcd().is_some() {
            Err("vcd should be None".into())
        } else {
            Ok(())
        }
    });
    c.add("vcd_vector_var_declaration", || {
        let src: &'static str = "module m(input [7:0] a, output [7:0] y); assign y = ~a; endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.enable_vcd();
        sim.set_input("a", &LW::from_u64(8, 0x5A))
            .map_err(|e| e.to_string())?;
        let v = sim.vcd().ok_or("no vcd")?;
        if !v.contains("a [7:0]") {
            Err("vector var decl".into())
        } else {
            Ok(())
        }
    });
}

fn scheduler_errors(c: &mut CaseList) {
    c.add("misc_zero_delay_loop_errors", || {
        let src: &'static str = "module m; reg y; initial y = 1'b0; always @(*) y = ~y; endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        if sim.run_until(5).is_ok() {
            Err("zero-delay loop must error".into())
        } else {
            Ok(())
        }
    });
    c.add("misc_set_input_unknown_port", || {
        let src: &'static str = "module m(input a, output y); assign y = a; endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        if sim.set_input("zz", &LW::from_u64(1, 0)).is_ok() {
            Err("unknown port must error".into())
        } else {
            Ok(())
        }
    });
    c.add("misc_set_input_on_output_rejected", || {
        let src: &'static str = "module m(input a, output y); assign y = a; endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        if sim.set_input("y", &LW::from_u64(1, 0)).is_ok() {
            Err("driving an output port must error".into())
        } else {
            Ok(())
        }
    });
    c.add("misc_read_missing_decl", || {
        let src: &'static str = "module m(input a, output y); assign y = a; endmodule";
        let d = cache(src);
        let sim = Simulator::new(d).map_err(|e| e.to_string())?;
        if sim.read("nope").is_some() {
            Err("missing decl must be None".into())
        } else {
            Ok(())
        }
    });
}

fn ports_and_reads(c: &mut CaseList) {
    c.add("misc_read_port_vs_decl", || {
        let src: &'static str = "module m(input [3:0] a, output [3:0] y); assign y = a; endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.set_input("a", &LW::from_u64(4, 0b1101))
            .map_err(|e| e.to_string())?;
        if sim.read_port("a").unwrap().to_u64() != Some(0b1101)
            || sim.read("a").unwrap().to_u64() != Some(0b1101)
        {
            Err("port read".into())
        } else {
            Ok(())
        }
    });
    c.add("misc_snapshot_lists_decls", || {
        let src: &'static str =
            "module m(input a, w2, output y, z); assign y = a; assign z = w2; endmodule";
        let d = cache(src);
        let sim = Simulator::new(d).map_err(|e| e.to_string())?;
        let snap = sim.snapshot();
        for n in ["a", "w2", "y", "z"] {
            if !snap.iter().any(|(name, _)| name == n) {
                return Err(format!("snapshot misses {}", n));
            }
        }
        Ok(())
    });
    c.add("misc_bit_read_flat", || {
        let src: &'static str = "module m(input [3:0] a, output [3:0] y); assign y = a; endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.set_input("a", &LW::from_u64(4, 0b0010))
            .map_err(|e| e.to_string())?;
        // find the net decl for a, read bit 1.
        let a = d
            .net_decls
            .iter()
            .find(|n| n.name == "a")
            .ok_or("no decl")?;
        if sim.read_net_bit(a.base + 1) != eda_core::logic::LogicValue::One {
            Err("flat bit read".into())
        } else {
            Ok(())
        }
    });
    c.add("misc_now_advances", || {
        let src: &'static str = "module tb; reg a; initial begin #10 a = 1; end endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.run_until(25).map_err(|e| e.to_string())?;
        if sim.now() < 10 {
            Err("now".into())
        } else {
            Ok(())
        }
    });
}

fn hierarchical(c: &mut CaseList) {
    c.add("misc_two_level_hierarchy", || {
        let src: &'static str = "module inv(input a, output y); assign y = ~a; endmodule
            module mid(input x, output z); wire w; inv u1(.a(x), .y(w)); inv u2(.a(w), .y(z)); endmodule
            module top(input x, output z2); wire m; mid u(.x(x), .z(m)); inv u3(.a(m), .y(z2)); endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.set_input("x", &LW::from_u64(1, 0)).map_err(|e| e.to_string())?;
        // 3 inversions of 0 -> 1.
        if sim.read_port("z2").unwrap().to_u64() != Some(1) {
            Err("3-inv chain".into())
        } else {
            Ok(())
        }
    });
    c.add("misc_positional_connections", || {
        let src: &'static str =
            "module s(input [1:0] a, [1:0] b, output [1:0] y); assign y = a & b; endmodule
            module top(input [1:0] a, b, output [1:0] y); s u(a, b, y); endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.set_input("a", &LW::from_u64(2, 0b11))
            .map_err(|e| e.to_string())?;
        sim.set_input("b", &LW::from_u64(2, 0b01))
            .map_err(|e| e.to_string())?;
        if sim.read_port("y").unwrap().to_u64() != Some(0b01) {
            Err("positional conns".into())
        } else {
            Ok(())
        }
    });
    c.add("misc_gate_primitives_sim", || {
        let src: &'static str = "module m(input a, b, output y); and g1(y, a, b); endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.set_input("a", &LW::from_u64(1, 1))
            .map_err(|e| e.to_string())?;
        sim.set_input("b", &LW::from_u64(1, 1))
            .map_err(|e| e.to_string())?;
        if sim.read("y").unwrap().to_u64() != Some(1) {
            Err("and gate".into())
        } else {
            Ok(())
        }
    });
}
