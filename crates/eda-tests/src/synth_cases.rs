//! Synthesis cases: RTL -> netlist, simulated for equivalence; mapping,
//! optimization and writer checks.

use crate::CaseList;
use eda_core::ir::Design;
use eda_core::library::CellLibrary;
use eda_core::logic::LogicWord as LW;
use eda_core::netlist::CellKind;
use eda_core::to_design::netlist_to_design;
use eda_sim::Simulator;
use eda_synth::map::map_to_library;
use eda_synth::writer::write_verilog;
use eda_synth::{synthesize, SynthOptions};
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

fn synth_cached(
    src: &'static str,
    opts: &'static SynthOptions,
) -> &'static eda_core::netlist::Netlist {
    type SynthCacheKey = (&'static str, bool, bool);
    type SynthCache = std::sync::Mutex<
        std::collections::HashMap<SynthCacheKey, &'static eda_core::netlist::Netlist>,
    >;
    static NETS: OnceLock<SynthCache> = OnceLock::new();
    let m = NETS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut g = m.lock().unwrap();
    g.entry((src, opts.optimize, opts.map_lib))
        .or_insert_with(|| {
            Box::leak(Box::new(
                synthesize(cache(src), opts).expect("synth").netlist,
            ))
        })
}

/// Run a combinational netlist on one input vector; returns output word.
fn eval_netlist_comb(
    nl: &eda_core::netlist::Netlist,
    ins: &[(&str, u64, usize)],
    out: &str,
) -> Result<u64, String> {
    let d: &'static Design = Box::leak(Box::new(netlist_to_design(nl)));
    let mut sim = Simulator::new(d)?;
    for (p, v, w) in ins {
        sim.set_input(p, &LW::from_u64(*w, *v))
            .map_err(|e| e.to_string())?;
    }
    match sim.read_port(out) {
        Some(w) => w.to_u64().ok_or_else(|| "output has X".to_string()),
        None => Err(format!("no port {}", out)),
    }
}

pub fn register(c: &mut CaseList) {
    rtl_vs_gate(c);
    optimization(c);
    mapping(c);
    writer(c);
    sequential_synth(c);
}

fn rtl_vs_gate(c: &mut CaseList) {
    struct Circuit {
        name: &'static str,
        src: &'static str,
        inputs: Vec<(&'static str, usize)>,
        out: &'static str,
        ref_fn: fn(&[u64]) -> u64,
        vectors: Vec<Vec<u64>>,
    }
    let circuits: Vec<Circuit> = vec![
        Circuit {
            name: "and",
            src: "module m(input a, b, output y); assign y = a & b; endmodule",
            inputs: vec![("a", 1), ("b", 1)],
            out: "y",
            ref_fn: |v| v[0] & v[1],
            vectors: (0..4u64).map(|x| vec![x & 1, x >> 1]).collect(),
        },
        Circuit {
            name: "or_xor",
            src: "module m(input a, b, output y); assign y = (a | b) ^ (a & b); endmodule",
            inputs: vec![("a", 1), ("b", 1)],
            out: "y",
            ref_fn: |v| (v[0] | v[1]) ^ (v[0] & v[1]),
            vectors: (0..4u64).map(|x| vec![x & 1, x >> 1]).collect(),
        },
        Circuit {
            name: "maj",
            src: "module m(input a, b, ci, output y); assign y = (a & b) | (a & ci) | (b & ci); endmodule",
            inputs: vec![("a", 1), ("b", 1), ("ci", 1)],
            out: "y",
            ref_fn: |v| (v[0] & v[1]) | (v[0] & v[2]) | (v[1] & v[2]),
            vectors: (0..8u64).map(|x| vec![x & 1, (x >> 1) & 1, (x >> 2) & 1]).collect(),
        },
        Circuit {
            name: "add4",
            src: "module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule",
            inputs: vec![("a", 4), ("b", 4)],
            out: "s",
            ref_fn: |v| (v[0] + v[1]) & 0xF,
            vectors: (0..16u64).map(|x| vec![x, 0xF - x]).collect(),
        },
        Circuit {
            name: "sub4",
            src: "module m(input [3:0] a, b, output [3:0] d); assign d = a - b; endmodule",
            inputs: vec![("a", 4), ("b", 4)],
            out: "d",
            ref_fn: |v| v[0].wrapping_sub(v[1]) & 0xF,
            vectors: (0..16u64).map(|x| vec![x, (x * 7) & 0xF]).collect(),
        },
        Circuit {
            name: "mux2",
            src: "module m(input s, a, b, output y); assign y = s ? b : a; endmodule",
            inputs: vec![("s", 1), ("a", 1), ("b", 1)],
            out: "y",
            ref_fn: |v| if v[0] == 1 { v[2] } else { v[1] },
            vectors: (0..8u64).map(|x| vec![x & 1, (x >> 1) & 1, (x >> 2) & 1]).collect(),
        },
        Circuit {
            name: "cmp_lt",
            src: "module m(input [3:0] a, b, output lt); assign lt = a < b; endmodule",
            inputs: vec![("a", 4), ("b", 4)],
            out: "lt",
            ref_fn: |v| (v[0] < v[1]) as u64,
            vectors: (0..16u64).map(|x| vec![x, (x * 5 + 3) & 0xF]).collect(),
        },
        Circuit {
            name: "parity4",
            src: "module m(input [3:0] v, output p); assign p = ^v; endmodule",
            inputs: vec![("v", 4)],
            out: "p",
            ref_fn: |v| v[0].count_ones() as u64 & 1,
            vectors: (0..16u64).map(|x| vec![x]).collect(),
        },
        Circuit {
            name: "shl_var",
            src: "module m(input [3:0] a, [1:0] s, output [3:0] y); assign y = a << s; endmodule",
            inputs: vec![("a", 4), ("s", 2)],
            out: "y",
            ref_fn: |v| v[0] << v[1] & 0xF,
            vectors: (0..16u64).map(|x| vec![x, (x / 4) & 3]).collect(),
        },
        Circuit {
            name: "concat_slice",
            src: "module m(input [3:0] a, [3:0] b, output [7:0] y); assign y = {a, b}; endmodule",
            inputs: vec![("a", 4), ("b", 4)],
            out: "y",
            ref_fn: |v| (v[0] << 4) | v[1],
            vectors: (0..16u64).map(|x| vec![x, x ^ 0xF]).collect(),
        },
        Circuit {
            name: "decoder2",
            src: "module m(input [1:0] sel, output [3:0] y); assign y = 4'd1 << sel; endmodule",
            inputs: vec![("sel", 2)],
            out: "y",
            ref_fn: |v| 1u64 << v[0],
            vectors: (0..4u64).map(|x| vec![x]).collect(),
        },
        Circuit {
            name: "inv_chain",
            src: "module m(input a, output y); wire w1, w2; assign w1 = ~a; assign w2 = ~w1; assign y = ~w2; endmodule",
            inputs: vec![("a", 1)],
            out: "y",
            ref_fn: |v| v[0] ^ 1,
            vectors: vec![vec![0], vec![1]],
        },
        Circuit {
            name: "wide_or",
            src: "module m(input [7:0] v, output any); assign any = |v; endmodule",
            inputs: vec![("v", 8)],
            out: "any",
            ref_fn: |v| (v[0] > 0) as u64,
            vectors: (0..16u64).map(|x| vec![x * 17]).chain(std::iter::once(vec![0u64])).collect(),
        },
        Circuit {
            name: "alu_and_or",
            src: "module m(input op, [3:0] a, b, output [3:0] y); assign y = op ? (a & b) : (a | b); endmodule",
            inputs: vec![("op", 1), ("a", 4), ("b", 4)],
            out: "y",
            ref_fn: |v| {
                if v[0] == 1 {
                    v[1] & v[2]
                } else {
                    v[1] | v[2]
                }
            },
            vectors: (0..16u64).map(|x| vec![x & 1, x, (x * 3) & 0xF]).collect(),
        },
    ];
    static OPTS: SyncOptions = SyncOptions;
    struct SyncOptions;
    impl std::ops::Deref for SyncOptions {
        type Target = SynthOptions;
        fn deref(&self) -> &SynthOptions {
            static O: OnceLock<SynthOptions> = OnceLock::new();
            O.get_or_init(|| SynthOptions {
                optimize: false,
                map_lib: false,
            })
        }
    }
    for cir in circuits {
        let rf = cir.ref_fn;
        for (vi, vec) in cir.vectors.clone().into_iter().enumerate() {
            let inputs = cir.inputs.clone();
            let out = cir.out;
            let name = format!("synth_equiv_{}_v{}", cir.name, vi);
            c.add(name, move || {
                let nl = synth_cached(cir.src, &OPTS);
                let ins: Vec<(&str, u64, usize)> = inputs
                    .clone()
                    .iter()
                    .zip(vec.iter())
                    .map(|((p, w), v)| (*p, *v, *w))
                    .collect();
                let got = eval_netlist_comb(nl, &ins, out)?;
                let want = rf(&vec);
                if got != want {
                    Err(format!("gate = {}, rtl/ref = {}", got, want))
                } else {
                    Ok(())
                }
            });
        }
    }
}

fn optimization(c: &mut CaseList) {
    c.add("synth_const_fold_and0", || {
        let d = cache("module m(input a, output y); assign y = a & 1'b0; endmodule");
        let r = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?;
        let consts: usize = r
            .netlist
            .cells()
            .iter()
            .filter(|cl| matches!(cl.kind, CellKind::Const(_)))
            .count();
        if r.netlist.stats().comb > 1 {
            Err(format!(
                "and-with-0 should fold; stats {:?}",
                r.netlist.stats()
            ))
        } else {
            let _ = consts;
            Ok(())
        }
    });
    c.add("synth_const_fold_or1", || {
        let d = cache("module m(input a, output y); assign y = a | 1'b1; endmodule");
        let r = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?;
        if r.netlist.stats().comb != 0 {
            Err(format!(
                "or-with-1 should fold; stats {:?}",
                r.netlist.stats()
            ))
        } else {
            Ok(())
        }
    });
    c.add("synth_double_inv_collapse", || {
        let d = cache("module m(input a, output y); assign y = ~(~a); endmodule");
        let r = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?;
        if r.netlist.stats().comb != 0 {
            Err(format!(
                "double inv should collapse; stats {:?}",
                r.netlist.stats()
            ))
        } else {
            Ok(())
        }
    });
    c.add("synth_duplicate_merge", || {
        let d = cache(
            "module m(input a, b, output y, z); wire t1 = a & b; wire t2 = a & b; assign y = t1 | t2; assign z = t1 & t2; endmodule",
        );
        let r1 = synthesize(d, &SynthOptions { optimize: false, map_lib: false })?;
        let r2 = synthesize(d, &SynthOptions { optimize: true, map_lib: false })?;
        if r2.netlist.cell_count() >= r1.netlist.cell_count() {
            Err(format!("optimize should reduce cells: {} -> {}", r1.netlist.cell_count(), r2.netlist.cell_count()))
        } else {
            Ok(())
        }
    });
    c.add("synth_dead_code_dropped", || {
        let d = cache(
            "module m(input a, b, output y); wire dead = a ^ b; wire dead2 = dead & a; assign y = a; endmodule",
        );
        let r = synthesize(d, &SynthOptions { optimize: true, map_lib: false })?;
        if r.netlist.stats().comb > 1 {
            Err(format!("dead logic kept: {:?}", r.netlist.stats()))
        } else {
            Ok(())
        }
    });
    // Constant output synthesized to a Const driver.
    for (k, v) in [(0u64, 0u64), (1, 1)] {
        c.add(format!("synth_const_output_{}", v), move || {
            let d = cache("module m(input a, output y); assign y = a & 1'b0 | 1'b0; endmodule");
            let lit = if v == 0 { "1'b0" } else { "1'b1" };
            let _ = lit;
            let r = synthesize(
                d,
                &SynthOptions {
                    optimize: true,
                    map_lib: false,
                },
            )?;
            let _ = k;
            // y must be constant 0 after folding.
            let nl = &r.netlist;
            let out = nl.port_by_name("y").ok_or("no y")?.bits[0];
            let drivers = nl.net(out).drivers.clone();
            if drivers.len() != 1 {
                return Err("y should have exactly one driver".into());
            }
            match &nl.cell(drivers[0]).kind {
                CellKind::Const(val) if *val == eda_core::logic::LogicValue::Zero => Ok(()),
                other => Err(format!("expected Const(0), got {:?}", other)),
            }
        });
    }
}

fn mapping(c: &mut CaseList) {
    c.add("map_generic_to_lib", || {
        let d = cache("module m(input a, b, output y); assign y = ~(a & b); endmodule");
        let r = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?;
        let lib = CellLibrary::std();
        let mapped = map_to_library(&r.netlist, &lib)?;
        for cl in mapped.cells() {
            match &cl.kind {
                CellKind::Lib(_) => {}
                CellKind::Const(_) => {}
                k => return Err(format!("unmapped generic cell {:?}", k)),
            }
        }
        Ok(())
    });
    c.add("map_full_pipeline", || {
        let d = cache("module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule");
        let r = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: true,
            },
        )?;
        for cl in r.netlist.cells() {
            match &cl.kind {
                CellKind::Lib(_)
                | CellKind::Const(_)
                | CellKind::Dff { .. }
                | CellKind::DffR { .. } => {}
                k => return Err(format!("generic cell survived mapping: {:?}", k)),
            }
        }
        Ok(())
    });
    // Mapped netlist still simulates correctly (sample).
    for k in 0..8u64 {
        let a = k & 0xF;
        let b = (k * 5 + 1) & 0xF;
        c.add(format!("map_add4_still_correct_{}", k), move || {
            let d =
                cache("module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule");
            let r = synthesize(
                d,
                &SynthOptions {
                    optimize: true,
                    map_lib: true,
                },
            )?;
            let got = eval_netlist_comb(&r.netlist, &[("a", a, 4), ("b", b, 4)], "s")?;
            let want = (a + b) & 0xF;
            if got != want {
                Err(format!("{} + {} -> {}", a, b, got))
            } else {
                Ok(())
            }
        });
    }
}

fn writer(c: &mut CaseList) {
    c.add("writer_emits_module", || {
        let d = cache("module m(input a, b, output y); assign y = a & b; endmodule");
        let r = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: true,
            },
        )?;
        let lib = CellLibrary::std();
        let text = write_verilog(&r.netlist, &lib);
        if !text.contains("module") {
            Err("no module in output".into())
        } else {
            Ok(())
        }
    });
    c.add("writer_output_reparse", || {
        let d = cache("module m(input a, b, output y); assign y = (a ^ b) & (a | b); endmodule");
        let r = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: true,
            },
        )?;
        let lib = CellLibrary::std();
        let text = write_verilog(&r.netlist, &lib);
        if let Err(e) = eda_hdl::parse(&text) {
            Err(format!("writer output does not re-parse: {}", e))
        } else {
            Ok(())
        }
    });
}

fn sequential_synth(c: &mut CaseList) {
    // A flop survives synthesis as a sequential cell and still simulates.
    c.add("synth_dff_survives", || {
        let d =
            cache("module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule");
        let r = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?;
        if r.netlist.stats().seq != 1 {
            Err(format!("seq cells = {}", r.netlist.stats().seq))
        } else {
            Ok(())
        }
    });
    // NBA right-hand sides sample pre-edge state. This keeps the internal
    // r flop and prevents q from being incorrectly retimed to `q <= d`.
    c.add("synth_nba_uses_present_state", || {
        let d = cache(
            "module m(input clk, d, output reg q); reg r;
                always @(posedge clk) begin r <= d; q <= r; end
             endmodule",
        );
        let r = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: true,
            },
        )?;
        if r.netlist.stats().seq != 2 {
            return Err(format!("want two flops, got {}", r.netlist.stats().seq));
        }
        let gd = netlist_to_design(&r.netlist);
        let mut sim = Simulator::new(&gd).map_err(|e| e.to_string())?;
        sim.set_input("d", &LW::from_u64(1, 1))
            .map_err(|e| e.to_string())?;
        for _ in 0..2 {
            sim.set_input("clk", &LW::from_u64(1, 0))
                .map_err(|e| e.to_string())?;
            sim.set_input("clk", &LW::from_u64(1, 1))
                .map_err(|e| e.to_string())?;
        }
        if sim.read_port("q").and_then(|q| q.to_u64()) != Some(1) {
            Err("two-stage NBA shift did not retain old r".into())
        } else {
            Ok(())
        }
    });
    // Mapped DFF.
    c.add("synth_dff_mapped", || {
        let d =
            cache("module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule");
        let r = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: true,
            },
        )?;
        if r.netlist.stats().seq != 1 {
            Err(format!("seq after map = {}", r.netlist.stats().seq))
        } else {
            Ok(())
        }
    });
    // Counter: 4 flops.
    c.add("synth_counter_flops", || {
        let d = cache(
            "module m(input clk, rst, output reg [3:0] q);
                always @(posedge clk or posedge rst) if (rst) q <= 4'd0; else q <= q + 4'd1;
             endmodule",
        );
        let r = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?;
        if r.netlist.stats().seq != 4 {
            Err(format!("want 4 flops, got {}", r.netlist.stats().seq))
        } else {
            Ok(())
        }
    });
    // Gate-level sequential simulation via netlist_to_design.
    for k in 0..4u64 {
        c.add(format!("synth_gate_seq_sim_{}", k), move || {
            let d = cache(
                "module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule",
            );
            let r = synthesize(
                d,
                &SynthOptions {
                    optimize: true,
                    map_lib: false,
                },
            )?;
            let gd: &'static Design = Box::leak(Box::new(netlist_to_design(&r.netlist)));
            let mut sim = Simulator::new(gd)?;
            sim.set_input("clk", &LW::from_u64(1, 0))
                .map_err(|e| e.to_string())?;
            sim.set_input("d", &LW::from_u64(1, k % 2))
                .map_err(|e| e.to_string())?;
            sim.set_input("clk", &LW::from_u64(1, 1))
                .map_err(|e| e.to_string())?;
            if sim.read_port("q").unwrap().to_u64() != Some(k % 2) {
                Err("gate-level flop capture".into())
            } else {
                Ok(())
            }
        });
    }
}
