//! Sequential circuit simulation cases: clocks, flops, counters, FSMs.

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

struct Seq {
    src: &'static str,
    steps: Vec<Step>,
}

enum Step {
    Set(&'static str, u64, usize),
    Cycle, // full clock cycle: 0 -> 1
    Expect(&'static str, u64, &'static str),
}

fn run_seq(s: Seq) -> Result<(), String> {
    let d = cache(s.src);
    let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
    for st in s.steps {
        match st {
            Step::Set(p, v, w) => sim
                .set_input(p, &LW::from_u64(w, v))
                .map_err(|e| format!("set {}: {}", p, e))?,
            Step::Cycle => {
                sim.set_input("clk", &LW::from_u64(1, 0))
                    .map_err(|e| e.to_string())?;
                sim.set_input("clk", &LW::from_u64(1, 1))
                    .map_err(|e| e.to_string())?;
            }
            Step::Expect(name, want, ctx) => match sim.read(name) {
                Some(w) => match w.to_u64() {
                    Some(got) if got == want => {}
                    other => {
                        return Err(format!(
                            "{} {} = {:?} bits={:?}, want {}",
                            ctx, name, other, w.bits, want
                        ))
                    }
                },
                None => return Err(format!("no net {}", name)),
            },
        }
    }
    Ok(())
}

pub fn register(c: &mut CaseList) {
    flops(c);
    latches_via_en(c);
    counters(c);
    shift_registers(c);
    fsms(c);
    clocking(c);
}

fn flops(c: &mut CaseList) {
    // Posedge D flop captures per cycle.
    let src: &'static str =
        "module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule";
    for (k, dvals) in [(0u64, vec![0u64, 1, 1, 0, 1]), (1, vec![1, 0, 0, 1, 1])] {
        let _ = k;
        let mut steps = vec![Step::Set("clk", 0, 1)];
        for d in dvals {
            steps.push(Step::Set("d", d, 1));
            steps.push(Step::Cycle);
            steps.push(Step::Expect("q", d, "after cycle"));
        }
        c.add(format!("seq_dff_pos_{}", k), move || {
            run_seq(Seq { src, steps })
        });
    }
    // Negedge flop.
    c.add("seq_dff_neg", || {
        let nsrc: &'static str =
            "module m(input clk, d, output reg q); always @(negedge clk) q <= d; endmodule";
        let d = cache(nsrc);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::from_u64(1, 1))
            .map_err(|e| e.to_string())?;
        sim.set_input("d", &LW::from_u64(1, 1))
            .map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::from_u64(1, 0))
            .map_err(|e| e.to_string())?;
        if sim.read("q").unwrap().to_u64() != Some(1) {
            Err("negedge capture".into())
        } else {
            Ok(())
        }
    });
    // Async reset flop (high).
    for rst in [0u64, 1] {
        let src2: &'static str = "module m(input clk, d, rst, output reg q);
            always @(posedge clk or posedge rst) if (rst) q <= 1'b0; else q <= d;
         endmodule";
        c.add(format!("seq_dffr_{}", rst), move || {
            let d = cache(src2);
            let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
            sim.set_input("clk", &LW::from_u64(1, 0))
                .map_err(|e| e.to_string())?;
            sim.set_input("d", &LW::from_u64(1, 1))
                .map_err(|e| e.to_string())?;
            sim.set_input("rst", &LW::from_u64(1, rst))
                .map_err(|e| e.to_string())?;
            sim.set_input("clk", &LW::from_u64(1, 1))
                .map_err(|e| e.to_string())?;
            let want = if rst == 1 { 0 } else { 1 };
            if sim.read("q").unwrap().to_u64() != Some(want) {
                Err(format!("rst={} capture", rst))
            } else {
                Ok(())
            }
        });
    }
    // Vector flop bank.
    let vsrc: &'static str = "module m(input clk, [3:0] d, output reg [3:0] q);
        always @(posedge clk) q <= d;
     endmodule";
    for k in 0..12u64 {
        let v = (k * 5) & 0xF;
        c.add(format!("seq_dff_vec_{}", k), move || {
            run_seq(Seq {
                src: vsrc,
                steps: vec![
                    Step::Set("clk", 0, 1),
                    Step::Set("d", v, 4),
                    Step::Cycle,
                    Step::Expect("q", v, "vec capture"),
                ],
            })
        });
    }
}

fn latches_via_en(c: &mut CaseList) {
    // Gated hold via mux feedback.
    let src: &'static str = "module m(input clk, d, en, output reg q);
        always @(posedge clk) if (en) q <= d;
     endmodule";
    for (k, seqs) in [
        (0u64, vec![(1u64, 1u64), (0u64, 1u64), (1u64, 0u64)]),
        (1, vec![(0, 0), (1, 1), (0, 1), (1, 0)]),
    ] {
        let _ = k;
        let mut steps = vec![Step::Set("clk", 0, 1)];
        // initial q is X; first cycle sets it via en or holds X — prime it:
        steps.push(Step::Set("en", 1, 1));
        steps.push(Step::Set("d", 0, 1));
        steps.push(Step::Cycle);
        let mut q: u64 = 0;
        for (d, en) in seqs {
            steps.push(Step::Set("d", d, 1));
            steps.push(Step::Set("en", en, 1));
            steps.push(Step::Cycle);
            if en == 1 {
                q = d;
            }
            steps.push(Step::Expect("q", q, "gated"));
        }
        c.add(format!("seq_gated_{}", k), move || {
            run_seq(Seq { src, steps })
        });
    }
}

fn counters(c: &mut CaseList) {
    // 4-bit up counter with reset.
    let src: &'static str = "module m(input clk, rst, output reg [3:0] q);
        always @(posedge clk or posedge rst) if (rst) q <= 4'd0; else q <= q + 4'd1;
     endmodule";
    for n in 1..=16u64 {
        let mut steps = vec![
            Step::Set("clk", 0, 1),
            Step::Set("rst", 1, 1),
            Step::Cycle,
            Step::Set("rst", 0, 1),
        ];
        for i in 0..n {
            steps.push(Step::Cycle);
            steps.push(Step::Expect("q", (i + 1) & 0xF, "count"));
        }
        c.add(format!("seq_counter_up_{}", n), move || {
            run_seq(Seq { src, steps })
        });
    }
    // Down counter.
    let dsrc: &'static str = "module m(input clk, rst, ld, [3:0] d, output reg [3:0] q);
        always @(posedge clk or posedge rst) begin
            if (rst) q <= 4'd0;
            else if (ld) q <= d;
            else q <= q - 4'd1;
        end
     endmodule";
    for (k, start) in [(0u64, 5u64), (1, 9), (2, 1), (3, 0)] {
        let _ = k;
        let mut steps = vec![
            Step::Set("clk", 0, 1),
            Step::Set("rst", 1, 1),
            Step::Cycle,
            Step::Set("rst", 0, 1),
            Step::Set("ld", 1, 1),
            Step::Set("d", start, 4),
            Step::Cycle,
            Step::Set("ld", 0, 1),
        ];
        let mut q = start;
        for _ in 0..3 {
            steps.push(Step::Cycle);
            q = q.wrapping_sub(1) & 0xF;
            steps.push(Step::Expect("q", q, "down"));
        }
        c.add(format!("seq_counter_down_{}_{}", k, start), move || {
            run_seq(Seq { src: dsrc, steps })
        });
    }
    // Ring counter (4 bits, one hot).
    let rsrc: &'static str = "module m(input clk, rst, output reg [3:0] q);
        always @(posedge clk or posedge rst) begin
            if (rst) q <= 4'b0001;
            else q <= {q[0], q[3], q[2], q[1]};
        end
     endmodule";
    let mut steps = vec![
        Step::Set("clk", 0, 1),
        Step::Set("rst", 1, 1),
        Step::Cycle,
        Step::Set("rst", 0, 1),
    ];
    let states = [8u64, 4, 2, 1, 8, 4];
    for want in states {
        steps.push(Step::Cycle);
        steps.push(Step::Expect("q", want, "ring"));
    }
    c.add("seq_ring_counter", move || {
        run_seq(Seq { src: rsrc, steps })
    });
    // Johnson counter.
    let jsrc: &'static str = "module m(input clk, rst, output reg [3:0] q);
        always @(posedge clk or posedge rst) begin
            if (rst) q <= 4'd0;
            else q <= {~q[0], q[3], q[2], q[1]};
        end
     endmodule";
    let mut steps = vec![
        Step::Set("clk", 0, 1),
        Step::Set("rst", 1, 1),
        Step::Cycle,
        Step::Set("rst", 0, 1),
    ];
    let jstates = [
        0b1000u64, 0b1100, 0b1110, 0b1111, 0b0111, 0b0011, 0b0001, 0b0000,
    ];
    for want in jstates {
        steps.push(Step::Cycle);
        steps.push(Step::Expect("q", want, "johnson"));
    }
    c.add("seq_johnson_counter", move || {
        run_seq(Seq { src: jsrc, steps })
    });
    // LFSR (x^4 + x^3 + 1): state advance.
    let lsrc: &'static str = "module m(input clk, rst, output reg [3:0] q);
        always @(posedge clk or posedge rst) begin
            if (rst) q <= 4'd1;
            else q <= {q[2], q[1], q[0], q[3] ^ q[2]};
        end
     endmodule";
    let mut steps = vec![
        Step::Set("clk", 0, 1),
        Step::Set("rst", 1, 1),
        Step::Cycle,
        Step::Set("rst", 0, 1),
    ];
    let mut q: u64 = 0b0001;
    for i in 0..15 {
        steps.push(Step::Cycle);
        let fb = ((q >> 3) & 1) ^ ((q >> 2) & 1);
        q = ((q << 1) | fb) & 0xF;
        steps.push(Step::Expect("q", q, "lfsr"));
        let _ = i;
    }
    c.add("seq_lfsr4", move || run_seq(Seq { src: lsrc, steps }));
}

fn shift_registers(c: &mut CaseList) {
    // SIPO.
    let src: &'static str = "module m(input clk, si, output reg [3:0] q); initial q = 4'd0;
        always @(posedge clk) q <= {q[2], q[1], q[0], si};
     endmodule";
    let pattern = [1u64, 0, 1, 1, 0, 1];
    let mut steps = vec![Step::Set("clk", 0, 1)];
    let mut q: u64 = 0;
    for bit in pattern {
        steps.push(Step::Set("si", bit, 1));
        steps.push(Step::Cycle);
        q = ((q << 1) | bit) & 0xF;
        steps.push(Step::Expect("q", q, "sipo"));
    }
    c.add("seq_sipo4", move || run_seq(Seq { src, steps }));
    // PISO with load then shift.
    let psrc: &'static str = "module m(input clk, ld, [3:0] d, output reg [3:0] q);
        always @(posedge clk) begin
            if (ld) q <= d;
            else q <= {1'b0, q[3], q[2], q[1]};
        end
     endmodule";
    for dval in [0b1011u64, 0b0110, 0xF, 0] {
        let mut steps = vec![
            Step::Set("clk", 0, 1),
            Step::Set("ld", 1, 1),
            Step::Set("d", dval, 4),
            Step::Cycle,
            Step::Expect("q", dval, "piso load"),
            Step::Set("ld", 0, 1),
        ];
        let mut q = dval;
        for _ in 0..3 {
            steps.push(Step::Cycle);
            q >>= 1;
            steps.push(Step::Expect("q", q, "piso shift"));
        }
        c.add(format!("seq_piso_{}", dval), move || {
            run_seq(Seq { src: psrc, steps })
        });
    }
}

fn fsms(c: &mut CaseList) {
    // Moore FSM: sequence detector for 1011.
    let src: &'static str =
        "module m(input clk, rst, in, output reg [1:0] state, output reg found);
        always @(posedge clk or posedge rst) begin
            if (rst) state <= 2'd0;
            else case (state)
                2'd0: state <= in ? 2'd1 : 2'd0;
                2'd1: state <= in ? 2'd1 : 2'd2;
                2'd2: state <= in ? 2'd3 : 2'd0;
                2'd3: state <= in ? 2'd1 : 2'd2;
                default: state <= 2'd0;
            endcase
        end
        always @(*) found = (state == 2'd3) & in;
     endmodule";
    let streams: Vec<Vec<u64>> = vec![
        vec![1, 0, 1, 1],
        vec![0, 1, 0, 1, 1, 0],
        vec![1, 1, 1, 0, 1, 1],
        vec![0, 0, 0, 0],
        vec![1, 0, 1, 0, 1, 1],
    ];
    for (k, stream) in streams.into_iter().enumerate() {
        // Reference model of the FSM.
        let mut state = 0u64;
        let mut founds = Vec::new();
        for &i in &stream {
            state = match state {
                0 => {
                    if i == 1 {
                        1
                    } else {
                        0
                    }
                }
                1 => {
                    if i == 1 {
                        1
                    } else {
                        2
                    }
                }
                2 => {
                    if i == 1 {
                        3
                    } else {
                        0
                    }
                }
                _ => {
                    if i == 1 {
                        1
                    } else {
                        2
                    }
                }
            };
            founds.push(((state == 3) && i == 1) as u64);
        }
        let mut steps = vec![
            Step::Set("clk", 0, 1),
            Step::Set("rst", 1, 1),
            Step::Cycle,
            Step::Set("rst", 0, 1),
        ];
        for (i, f) in stream.iter().zip(founds) {
            steps.push(Step::Set("in", *i, 1));
            steps.push(Step::Cycle);
            steps.push(Step::Expect("found", f, "found"));
        }
        c.add(format!("seq_fsm_1011_{}", k), move || {
            run_seq(Seq { src, steps })
        });
    }
    // Gray code counter FSM.
    let gsrc: &'static str = "module m(input clk, rst, output reg [2:0] g);
        always @(posedge clk or posedge rst) begin
            if (rst) g <= 3'd0;
            else case (g)
                3'd0: g <= 3'd1;
                3'd1: g <= 3'd3;
                3'd3: g <= 3'd2;
                3'd2: g <= 3'd6;
                3'd6: g <= 3'd7;
                3'd7: g <= 3'd5;
                3'd5: g <= 3'd4;
                default: g <= 3'd0;
            endcase
        end
     endmodule";
    let gseq = [1u64, 3, 2, 6, 7, 5, 4, 0, 1];
    let mut steps = vec![
        Step::Set("clk", 0, 1),
        Step::Set("rst", 1, 1),
        Step::Cycle,
        Step::Set("rst", 0, 1),
    ];
    for want in gseq {
        steps.push(Step::Cycle);
        steps.push(Step::Expect("g", want, "gray"));
    }
    c.add("seq_gray_fsm", move || run_seq(Seq { src: gsrc, steps }));
    // Toggle flop / clock divider by 2.
    let tsrc: &'static str = "module m(input clk, rst, output reg div2);
        always @(posedge clk or posedge rst) if (rst) div2 <= 1'b0; else div2 <= ~div2;
     endmodule";
    for n in 1..=8u64 {
        let mut steps = vec![
            Step::Set("clk", 0, 1),
            Step::Set("rst", 1, 1),
            Step::Cycle,
            Step::Set("rst", 0, 1),
        ];
        for i in 0..n {
            steps.push(Step::Cycle);
            steps.push(Step::Expect("div2", (i + 1) % 2, "div2"));
        }
        c.add(format!("seq_div2_{}", n), move || {
            run_seq(Seq { src: tsrc, steps })
        });
    }
    // BCD counter digit (wraps at 9).
    let bsrc: &'static str = "module m(input clk, rst, output reg [3:0] d);
        always @(posedge clk or posedge rst) begin
            if (rst) d <= 4'd0;
            else if (d == 4'd9) d <= 4'd0;
            else d <= d + 4'd1;
        end
     endmodule";
    let mut steps = vec![
        Step::Set("clk", 0, 1),
        Step::Set("rst", 1, 1),
        Step::Cycle,
        Step::Set("rst", 0, 1),
    ];
    for i in 0..12u64 {
        steps.push(Step::Cycle);
        steps.push(Step::Expect("d", (i + 1) % 10, "bcd"));
    }
    c.add("seq_bcd_counter", move || run_seq(Seq { src: bsrc, steps }));
}

fn clocking(c: &mut CaseList) {
    // forever-based clock generator: frequency check.
    c.add("seq_clkgen_period10", || {
        let src: &'static str =
            "module tb; reg clk; initial begin clk = 0; forever #5 clk = ~clk; end endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.run_until(50).map_err(|e| e.to_string())?;
        // toggles at 5,10,...,50 → after 50 (inclusive), 10 toggles → clk = 0.
        if sim.read("clk").unwrap().to_u64() != Some(0) {
            Err("clk after 50ns".into())
        } else {
            sim.run_until(55).map_err(|e| e.to_string())?;
            if sim.read("clk").unwrap().to_u64() != Some(1) {
                Err("clk after 55ns".into())
            } else {
                Ok(())
            }
        }
    });
    // Edge sensitivity only on transitions (no re-fire on same level).
    c.add("seq_no_refire_same_level", || {
        let src: &'static str =
            "module m(input clk, d, output reg [3:0] n); initial n = 4'd0; always @(posedge clk) n <= n + 4'd1; endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::from_u64(1, 0)).map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::from_u64(1, 1)).map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::from_u64(1, 1)).map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::from_u64(1, 1)).map_err(|e| e.to_string())?;
        if sim.read("n").unwrap().to_u64() != Some(1) {
            Err("repeated 1 must not re-trigger".into())
        } else {
            Ok(())
        }
    });
    // X->1 counts as posedge (old != One).
    c.add("seq_x_to_1_is_posedge", || {
        let src: &'static str =
            "module m(input clk, output reg q); always @(posedge clk) q <= 1'b1; endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::from_u64(1, 1))
            .map_err(|e| e.to_string())?;
        if sim.read("q").unwrap().to_u64() != Some(1) {
            Err("X->1 should be a posedge".into())
        } else {
            Ok(())
        }
    });
    // 1->X is not a posedge.
    c.add("seq_1_to_x_not_posedge", || {
        let src: &'static str =
            "module m(input clk, output reg [3:0] n); initial n = 4'd0; always @(posedge clk) n <= n + 4'd1; endmodule";
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::from_u64(1, 1)).map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::xes(1)).map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::from_u64(1, 1)).map_err(|e| e.to_string())?;
        if sim.read("n").unwrap().to_u64() != Some(2) {
            Err(format!("1->X->1 should be exactly one new posedge, got {:?}", sim.read("n")))
        } else {
            Ok(())
        }
    });
}
