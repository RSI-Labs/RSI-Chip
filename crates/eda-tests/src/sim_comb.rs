//! Combinational circuit simulation cases: one case per (circuit, vector).

use crate::CaseList;
use eda_core::ir::Design;
use eda_core::logic::LogicWord as LW;
use eda_sim::Simulator;
use std::sync::OnceLock;

/// Shared leaky cache of compiled designs keyed by source text.
fn cache(src: &'static str) -> &'static Design {
    static DESIGNS: OnceLock<
        std::sync::Mutex<std::collections::HashMap<&'static str, &'static Design>>,
    > = OnceLock::new();
    let m = DESIGNS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut g = m.lock().unwrap();
    g.entry(src)
        .or_insert_with(|| Box::leak(Box::new(eda_hdl::compile(src, None).expect("compile"))))
}

struct CombCase {
    src: &'static str,
    inputs: Vec<(&'static str, u64, usize)>, // (port, value, width)
    outputs: Vec<(&'static str, u64)>,       // (decl, expected)
}

fn check_comb(cc: CombCase) -> Result<(), String> {
    let d = cache(cc.src);
    let mut sim = Simulator::new(d)?;
    for (p, v, w) in &cc.inputs {
        sim.set_input(p, &LW::from_u64(*w, *v))
            .map_err(|e| format!("set {}: {}", p, e))?;
    }
    for (name, want) in &cc.outputs {
        match sim.read(name) {
            Some(w) => match w.to_u64() {
                Some(got) if got == *want => {}
                Some(got) => return Err(format!("{} = {}, want {}", name, got, want)),
                None => {
                    return Err(format!(
                        "{} = X bits={:?}, want {}, decls={:?}, assigns={:?}",
                        name, w.bits, want, d.net_decls, d.assigns
                    ))
                }
            },
            None => return Err(format!("no net {}", name)),
        }
    }
    Ok(())
}

pub fn register(c: &mut CaseList) {
    gates(c);
    adders(c);
    muxes(c);
    decoders(c);
    comparators(c);
    alu_ops(c);
    shifts(c);
    parity_trees(c);
    priority_logic(c);
    concat_slice(c);
    concat_assignment(c);
    wildcard_cases(c);
    x_propagation(c);
}

fn wildcard_cases(c: &mut CaseList) {
    for index in 0_u32..100 {
        c.add(format!("comb_wildcard_case_{index:03}"), move || {
            let keyword = if index % 2 == 0 { "casez" } else { "casex" };
            let source = format!(
                "module m(input [3:0] sel, output reg [2:0] y); \
                 always @(*) {keyword} (sel) \
                 4'b1???: y = 3'd7; \
                 4'b01??: y = 3'd6; \
                 4'b001?: y = 3'd5; \
                 4'b0001: y = 3'd4; \
                 default: y = 3'd0; endcase endmodule"
            );
            let design = eda_hdl::compile(&source, None).map_err(|error| error.to_string())?;
            let value = u64::from(index % 16);
            let expected = if value >= 8 {
                7
            } else if value >= 4 {
                6
            } else if value >= 2 {
                5
            } else if value == 1 {
                4
            } else {
                0
            };
            let mut simulator = Simulator::new(&design)?;
            simulator.set_input("sel", &LW::from_u64(4, value))?;
            match simulator.read("y").and_then(|value| value.to_u64()) {
                Some(actual) if actual == expected => Ok(()),
                Some(actual) => Err(format!(
                    "{} sel={} y={} expected={}",
                    keyword, value, actual, expected
                )),
                None => Err(format!("{} sel={} y is unknown", keyword, value)),
            }
        });
    }
}

fn concat_assignment(c: &mut CaseList) {
    for a in 0..16u64 {
        c.add(format!("comb_concat_assign_{}", a), move || {
            check_comb(CombCase {
                src: "module m(input [3:0] a, output co, output [3:0] s); assign {co, s} = {1'b0, a}; endmodule",
                inputs: vec![("a", a, 4)],
                outputs: vec![("co", 0), ("s", a)],
            })
        });
    }
}

fn gates(c: &mut CaseList) {
    let srcs: Vec<(&'static str, &'static str)> = vec![
        (
            "and",
            "module m(input a, b, output y); assign y = a & b; endmodule",
        ),
        (
            "or",
            "module m(input a, b, output y); assign y = a | b; endmodule",
        ),
        (
            "xor",
            "module m(input a, b, output y); assign y = a ^ b; endmodule",
        ),
        (
            "nand",
            "module m(input a, b, output y); assign y = ~(a & b); endmodule",
        ),
        (
            "nor",
            "module m(input a, b, output y); assign y = ~(a | b); endmodule",
        ),
        (
            "xnor",
            "module m(input a, b, output y); assign y = ~(a ^ b); endmodule",
        ),
        (
            "not",
            "module m(input a, output y); assign y = ~a; endmodule",
        ),
        (
            "buf",
            "module m(input a, output y); assign y = a & 1'b1; endmodule",
        ),
    ];
    let f = |n: &str, x: u64, y: u64| -> u64 {
        match n {
            "and" => x & y,
            "or" => x | y,
            "xor" => x ^ y,
            "nand" => (x & y) ^ 1,
            "nor" => (x | y) ^ 1,
            "xnor" => (x ^ y) ^ 1,
            "not" => x ^ 1,
            _ => x,
        }
    };
    for (name, src) in srcs {
        for x in 0..2u64 {
            for y in 0..2u64 {
                let want = f(name, x, y);
                c.add(format!("comb_gate_{}_{}{}", name, x, y), move || {
                    check_comb(CombCase {
                        src,
                        inputs: if name == "not" || name == "buf" {
                            vec![("a", x, 1)]
                        } else {
                            vec![("a", x, 1), ("b", y, 1)]
                        },
                        outputs: vec![("y", want)],
                    })
                });
            }
        }
    }
    // Multi-level trees.
    for bits in 0..8u64 {
        let (a, b, ci) = (bits & 1, (bits >> 1) & 1, (bits >> 2) & 1);
        let want = (a & b) | (a & ci) | (b & ci);
        c.add(format!("comb_maj_{}", bits), move || {
            check_comb(CombCase {
                src: "module m(input a, b, ci, output y); assign y = (a & b) | (a & ci) | (b & ci); endmodule",
                inputs: vec![("a", a, 1), ("b", b, 1), ("ci", ci, 1)],
                outputs: vec![("y", want)],
            })
        });
    }
}

fn adders(c: &mut CaseList) {
    // Half adder.
    for bits in 0..4u64 {
        let (a, b) = (bits & 1, (bits >> 1) & 1);
        c.add(format!("comb_half_adder_{}", bits), move || {
            check_comb(CombCase {
                src: "module m(input a, b, output s, co); assign s = a ^ b; assign co = a & b; endmodule",
                inputs: vec![("a", a, 1), ("b", b, 1)],
                outputs: vec![("s", a ^ b), ("co", a & b)],
            })
        });
    }
    // Full adder.
    for bits in 0..8u64 {
        let (a, b, ci) = (bits & 1, (bits >> 1) & 1, (bits >> 2) & 1);
        let sum = a + b + ci;
        c.add(format!("comb_full_adder_{}", bits), move || {
            check_comb(CombCase {
                src: "module m(input a, b, ci, output s, co); wire p = a ^ b; assign s = p ^ ci; assign co = (p & ci) | (a & b); endmodule",
                inputs: vec![("a", a, 1), ("b", b, 1), ("ci", ci, 1)],
                outputs: vec![("s", sum & 1), ("co", sum >> 1)],
            })
        });
    }
    // 4-bit ripple exhaustive (256).
    for ab in 0..256u64 {
        let (a, b) = (ab & 0xF, ab >> 4);
        let sum = (a + b) & 0xF;
        c.add(format!("comb_add4_{}", ab), move || {
            check_comb(CombCase {
                src: "module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule",
                inputs: vec![("a", a, 4), ("b", b, 4)],
                outputs: vec![("s", sum)],
            })
        });
    }
    // 8-bit ripple sampled.
    for k in 0..32u64 {
        let a = (k * 37) & 0xFF;
        let b = (k * 91 + 13) & 0xFF;
        let sum = (a + b) & 0xFF;
        c.add(format!("comb_add8_{}", k), move || {
            check_comb(CombCase {
                src: "module m(input [7:0] a, b, output [7:0] s); assign s = a + b; endmodule",
                inputs: vec![("a", a, 8), ("b", b, 8)],
                outputs: vec![("s", sum)],
            })
        });
    }
    // Subtractors.
    for k in 0..32u64 {
        let a = (k * 53) & 0xF;
        let b = (k * 29 + 7) & 0xF;
        let d = (a.wrapping_sub(b)) & 0xF;
        c.add(format!("comb_sub4_{}", k), move || {
            check_comb(CombCase {
                src: "module m(input [3:0] a, b, output [3:0] d); assign d = a - b; endmodule",
                inputs: vec![("a", a, 4), ("b", b, 4)],
                outputs: vec![("d", d)],
            })
        });
    }
    // Multiply low bits.
    for k in 0..16u64 {
        let a = k & 0xF;
        let b = (k * 3 + 1) & 0xF;
        let m = (a.wrapping_mul(b)) & 0xF;
        c.add(format!("comb_mul4_{}", k), move || {
            check_comb(CombCase {
                src: "module m(input [3:0] a, b, output [3:0] p); assign p = a * b; endmodule",
                inputs: vec![("a", a, 4), ("b", b, 4)],
                outputs: vec![("p", m)],
            })
        });
    }
}

fn muxes(c: &mut CaseList) {
    // 2:1 exhaustive.
    for bits in 0..8u64 {
        let (s, a, b) = (bits & 1, (bits >> 1) & 1, (bits >> 2) & 1);
        let y = if s == 1 { b } else { a };
        c.add(format!("comb_mux2_{}", bits), move || {
            check_comb(CombCase {
                src: "module m(input s, a, b, output y); assign y = s ? b : a; endmodule",
                inputs: vec![("s", s, 1), ("a", a, 1), ("b", b, 1)],
                outputs: vec![("y", y)],
            })
        });
    }
    // 4:1 with case.
    for sel in 0..4u64 {
        let input = 0b1001_0110u64;
        let d = [
            input & 1,
            (input >> 1) & 1,
            (input >> 2) & 1,
            (input >> 3) & 1,
        ];
        let y = d[sel as usize];
        c.add(format!("comb_mux4_{}", sel), move || {
            check_comb(CombCase {
                src: "module m(input [1:0] sel, input [3:0] d, output y); wire d0 = d[0], d1 = d[1], d2 = d[2], d3 = d[3]; assign y = sel == 2'd0 ? d0 : sel == 2'd1 ? d1 : sel == 2'd2 ? d2 : d3; endmodule",
                inputs: vec![("sel", sel, 2), ("d", 0b1001_0110, 4)],
                outputs: vec![("y", y)],
            })
        });
    }
    // Vector mux.
    for s in 0..2u64 {
        let (a, b) = (0xA5u64, 0x3C);
        let y = if s == 1 { b } else { a };
        c.add(format!("comb_vmux_{}", s), move || {
            check_comb(CombCase {
                src: "module m(input s, [7:0] a, b, output [7:0] y); assign y = s ? b : a; endmodule",
                inputs: vec![("s", s, 1), ("a", a, 8), ("b", b, 8)],
                outputs: vec![("y", y)],
            })
        });
    }
    // Nested ternaries (mux chain).
    for v in 0..8u64 {
        let y = match (v & 1, (v >> 1) & 1, (v >> 2) & 1) {
            (0, 0, _) => 1,
            (0, 1, _) => 2,
            (1, 0, _) => 3,
            _ => 4,
        };
        c.add(format!("comb_mux_chain_{}", v), move || {
            check_comb(CombCase {
                src: "module m(input a, b, c, output [2:0] y); assign y = a ? (b ? 3'd4 : 3'd3) : (b ? 3'd2 : 3'd1); endmodule",
                inputs: vec![("a", v & 1, 1), ("b", (v >> 1) & 1, 1), ("c", (v >> 2) & 1, 1)],
                outputs: vec![("y", y)],
            })
        });
    }
}

fn decoders(c: &mut CaseList) {
    // 2->4 decoder.
    for sel in 0..4u64 {
        let y = 1u64 << sel;
        c.add(format!("comb_dec2to4_{}", sel), move || {
            check_comb(CombCase {
                src: "module m(input [1:0] sel, input en, output [3:0] y); assign y = en ? (1 << sel) : 4'd0; endmodule",
                inputs: vec![("sel", sel, 2), ("en", 1, 1)],
                outputs: vec![("y", y)],
            })
        });
    }
    c.add("comb_dec_disabled", || {
        check_comb(CombCase {
            src: "module m(input [1:0] sel, input en, output [3:0] y); assign y = en ? (1 << sel) : 4'd0; endmodule",
            inputs: vec![("sel", 2, 2), ("en", 0, 1)],
            outputs: vec![("y", 0)],
        })
    });
    // 3->8 decoder sampled.
    for sel in [0u64, 3, 5, 7] {
        let y = 1u64 << sel;
        c.add(format!("comb_dec3to8_{}", sel), move || {
            check_comb(CombCase {
                src: "module m(input [2:0] sel, output [7:0] y); assign y = 8'd1 << sel; endmodule",
                inputs: vec![("sel", sel, 3)],
                outputs: vec![("y", y)],
            })
        });
    }
    // 4->2 encoder (or-reduce).
    for v in 0..16u64 {
        let any = (v > 0) as u64;
        let odd = (v.count_ones() % 2) as u64;
        c.add(format!("comb_enc_or_{}", v), move || {
            check_comb(CombCase {
                src: "module m(input [3:0] v, output any, odd); assign any = |v; assign odd = ^v; endmodule",
                inputs: vec![("v", v, 4)],
                outputs: vec![("any", any), ("odd", odd)],
            })
        });
    }
}

fn comparators(c: &mut CaseList) {
    for k in 0..32u64 {
        let a = (k * 5) & 0xF;
        let b = (k * 11 + 2) & 0xF;
        let (lt, eq, gt) = ((a < b) as u64, (a == b) as u64, (a > b) as u64);
        c.add(format!("comb_cmp_{}", k), move || {
            check_comb(CombCase {
                src: "module m(input [3:0] a, b, output lt, eq, gt); assign lt = a < b; assign eq = (a == b); assign gt = a > b; endmodule",
                inputs: vec![("a", a, 4), ("b", b, 4)],
                outputs: vec![("lt", lt), ("eq", eq), ("gt", gt)],
            })
        });
    }
    // Logical connectives of comparisons.
    for k in 0..16u64 {
        let a = (k * 3) & 0xF;
        let b = (k * 7) & 0xF;
        let inrange = (3..=9).contains(&a) as u64;
        c.add(format!("comb_inrange_{}", k), move || {
            check_comb(CombCase {
                src: "module m(input [3:0] a, b, output y); assign y = (a >= 4'd3) && (a <= 4'd9); endmodule",
                inputs: vec![("a", a, 4), ("b", b, 4)],
                outputs: vec![("y", inrange)],
            })
        });
    }
}

fn alu_ops(c: &mut CaseList) {
    // A small ALU: op selects add/sub/and/or/xor/pass.
    for k in 0..64u64 {
        let op = k % 5;
        let a = (k * 13) & 0xF;
        let b = (k * 7 + 3) & 0xF;
        let y = match op {
            0 => (a + b) & 0xF,
            1 => (a.wrapping_sub(b)) & 0xF,
            2 => a & b,
            3 => a | b,
            _ => a ^ b,
        };
        c.add(format!("comb_alu_{}", k), move || {
            check_comb(CombCase {
                src: "module m(input [2:0] op, [3:0] a, b, output [3:0] y);
                    assign y = op == 3'd0 ? a + b : op == 3'd1 ? a - b : op == 3'd2 ? (a & b) : op == 3'd3 ? (a | b) : (a ^ b);
                 endmodule",
                inputs: vec![("op", op, 3), ("a", a, 4), ("b", b, 4)],
                outputs: vec![("y", y)],
            })
        });
    }
}

fn shifts(c: &mut CaseList) {
    for k in 0..32u64 {
        let a = (k * 17) & 0xF;
        let s = k % 4;
        let l = (a << s) & 0xF;
        let r = a >> s;
        c.add(format!("comb_shift_{}", k), move || {
            check_comb(CombCase {
                src: "module m(input [3:0] a, [1:0] s, output [3:0] l, r); assign l = a << s; assign r = a >> s; endmodule",
                inputs: vec![("a", a, 4), ("s", s, 2)],
                outputs: vec![("l", l), ("r", r)],
            })
        });
    }
    // Constant shifts.
    for a in 0..16u64 {
        let l1 = (a << 1) & 0xF;
        let r2 = a >> 2;
        c.add(format!("comb_cshift_{}", a), move || {
            check_comb(CombCase {
                src: "module m(input [3:0] a, output [3:0] l1, r2); assign l1 = a << 1; assign r2 = a >> 2; endmodule",
                inputs: vec![("a", a, 4)],
                outputs: vec![("l1", l1), ("r2", r2)],
            })
        });
    }
}

fn parity_trees(c: &mut CaseList) {
    for v in 0..32u64 {
        let ones = v.count_ones();
        let even = (ones % 2 == 0) as u64;
        let all1 = (ones == 5) as u64;
        let any1 = (ones > 0) as u64;
        c.add(format!("comb_parity_{}", v), move || {
            check_comb(CombCase {
                src: "module m(input [4:0] v, output even, all, any); assign even = ~^v; assign all = &v; assign any = |v; endmodule",
                inputs: vec![("v", v, 5)],
                outputs: vec![("even", even), ("all", all1), ("any", any1)],
            })
        });
    }
    // XOR of wires (parity via tree).
    for v in 0..16u64 {
        let p = (v.count_ones() % 2) as u64;
        c.add(format!("comb_xor_tree_{}", v), move || {
            check_comb(CombCase {
                src: "module m(input [3:0] v, output p); wire a = v[0] ^ v[1]; wire b = v[2] ^ v[3]; assign p = a ^ b; endmodule",
                inputs: vec![("v", v, 4)],
                outputs: vec![("p", p)],
            })
        });
    }
}

fn priority_logic(c: &mut CaseList) {
    // Priority encoder.
    for v in 0..8u64 {
        let (idx, valid) = if v == 0 {
            (0u64, 0u64)
        } else {
            ((63 - v.leading_zeros()) as u64, 1)
        };
        c.add(format!("comb_priority_{}", v), move || {
            check_comb(CombCase {
                src: "module m(input [2:0] r, output [1:0] idx, output valid);
                    assign idx = r[2] ? 2'd2 : r[1] ? 2'd1 : 2'd0;
                    assign valid = |r;
                 endmodule",
                inputs: vec![("r", v, 3)],
                outputs: vec![("idx", idx), ("valid", valid)],
            })
        });
    }
    // if/else chain in always @(*).
    for v in 0..8u64 {
        let y = match v {
            0 => 1,
            1 | 2 => 2,
            3 | 4 => 3,
            _ => 4,
        };
        c.add(format!("comb_ifchain_{}", v), move || {
            check_comb(CombCase {
                src: "module m(input [2:0] v, output reg [2:0] y);
                    always @(*) begin
                        if (v == 3'd0) y = 3'd1;
                        else if (v == 3'd1) y = 3'd2;
                        else if (v == 3'd2) y = 3'd2;
                        else if (v == 3'd3) y = 3'd3;
                        else if (v == 3'd4) y = 3'd3;
                        else y = 3'd4;
                    end
                 endmodule",
                inputs: vec![("v", v, 3)],
                outputs: vec![("y", y)],
            })
        });
    }
    // case decoder via always.
    for sel in 0..4u64 {
        let y = [5u64, 6, 7, 8][sel as usize];
        c.add(format!("comb_case_{}", sel), move || {
            check_comb(CombCase {
                src: "module m(input [1:0] sel, output reg [3:0] y);
                    always @(*) case (sel)
                        2'd0: y = 4'd5;
                        2'd1: y = 4'd6;
                        2'd2: y = 4'd7;
                        default: y = 4'd8;
                    endcase
                 endmodule",
                inputs: vec![("sel", sel, 2)],
                outputs: vec![("y", y)],
            })
        });
    }
}

fn concat_slice(c: &mut CaseList) {
    for v in 0..16u64 {
        let cat = (v << 4) | (v ^ 0xF);
        c.add(format!("comb_concat_{}", v), move || {
            check_comb(CombCase {
                src: "module m(input [3:0] a, output [7:0] y); assign y = {a, ~a}; endmodule",
                inputs: vec![("a", v, 4)],
                outputs: vec![("y", cat)],
            })
        });
    }
    for v in (0..256u64).step_by(17) {
        let hi = (v >> 4) & 0xF;
        c.add(format!("comb_slice_{}", v), move || {
            check_comb(CombCase {
                src: "module m(input [7:0] a, output [3:0] hi, output [3:0] lo, output b7, output b0); assign hi = a[7:4]; assign lo = a[3:0]; assign b7 = a[7]; assign b0 = a[0]; endmodule",
                inputs: vec![("a", v, 8)],
                outputs: vec![("hi", hi), ("lo", v & 0xF), ("b7", v >> 7), ("b0", v & 1)],
            })
        });
    }
    for v in 0..8u64 {
        let rep = v | (v << 3) | (v << 6);
        c.add(format!("comb_repeat_{}", v), move || {
            check_comb(CombCase {
                src: "module m(input [2:0] a, output [8:0] y); assign y = {3{a}}; endmodule",
                inputs: vec![("a", v, 3)],
                outputs: vec![("y", rep)],
            })
        });
    }
}

fn x_propagation(c: &mut CaseList) {
    // Uninitialized inputs are X; gates with a controlling 0/1 resolve.
    let src: &'static str =
        "module m(input a, b, output y1, y2, y3); assign y1 = a & b; assign y2 = a | b; assign y3 = a ^ b; endmodule";
    c.add("comb_x_and_stays_x", move || {
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.set_input("a", &LW::xes(1)).map_err(|e| e.to_string())?;
        if sim.read("y1").unwrap().bit(0) != eda_core::logic::LogicValue::X {
            Err("X&X should stay X".into())
        } else {
            Ok(())
        }
    });
    c.add("comb_x_and_zero_is_zero", move || {
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.set_input("a", &LW::from_u64(1, 0))
            .map_err(|e| e.to_string())?;
        if sim.read("y1").unwrap().bit(0) != eda_core::logic::LogicValue::Zero {
            Err("0&X should be 0".into())
        } else {
            Ok(())
        }
    });
    c.add("comb_x_or_one_is_one", move || {
        let d = cache(src);
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.set_input("a", &LW::from_u64(1, 1))
            .map_err(|e| e.to_string())?;
        if sim.read("y2").unwrap().bit(0) != eda_core::logic::LogicValue::One {
            Err("1|X should be 1".into())
        } else {
            Ok(())
        }
    });
    c.add("comb_x_xor_stays_x", move || {
        let d = cache(src);
        let sim = Simulator::new(d).map_err(|e| e.to_string())?;
        if sim.read("y3").unwrap().bit(0) != eda_core::logic::LogicValue::X {
            Err("X^X should stay X".into())
        } else {
            Ok(())
        }
    });
    // Unset input port read as X.
    c.add("comb_unset_output_is_x", || {
        let d = cache("module m(input a, output y); assign y = ~a; endmodule");
        let sim = Simulator::new(d).map_err(|e| e.to_string())?;
        if sim.read_port("y").unwrap().bit(0) != eda_core::logic::LogicValue::X {
            Err("unset input -> X output".into())
        } else {
            Ok(())
        }
    });
}
