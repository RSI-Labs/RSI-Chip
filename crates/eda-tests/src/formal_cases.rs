//! Formal verification cases: SAT cross-checks, equivalence, BMC.

use crate::CaseList;
use eda_assert::BoolExpr;
use eda_core::ir::Design;
use eda_formal::sat::{solve, Cnf, SatResult};
use eda_formal::{
    bmc, check_equivalent, prove, prove_with_invariants, prove_with_value_invariants, BmcOptions,
    ProofInvariant, ProofStatus,
};
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

/// Small deterministic PRNG (xorshift64) for reproducible random instances.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

pub fn register(c: &mut CaseList) {
    sat_basics(c);
    sat_vs_bruteforce(c);
    equivalence(c);
    bmc_cases(c);
    induction_cases(c);
}

fn sat_basics(c: &mut CaseList) {
    c.add("sat_empty_cnf_is_sat", || {
        let cnf = Cnf::new();
        match solve(&cnf) {
            SatResult::Sat(_) => Ok(()),
            SatResult::Unsat => Err("empty CNF must be SAT".into()),
        }
    });
    c.add("sat_unit_clause", || {
        let mut cnf = Cnf::new();
        let v = cnf.add_var();
        cnf.add_clause(vec![v as i32]);
        match solve(&cnf) {
            SatResult::Sat(m) if m.get(v - 1) == Some(&true) => Ok(()),
            _ => Err("unit clause".into()),
        }
    });
    c.add("sat_contradiction", || {
        let mut cnf = Cnf::new();
        let v = cnf.add_var();
        cnf.add_clause(vec![v as i32]);
        cnf.add_clause(vec![-(v as i32)]);
        match solve(&cnf) {
            SatResult::Unsat => Ok(()),
            _ => Err("x & !x must be UNSAT".into()),
        }
    });
    // Full 3-var OR: at least one true.
    for bits in 0..8u64 {
        c.add(format!("sat_or3_{}", bits), move || {
            let mut cnf = Cnf::new();
            let vars: Vec<i32> = (0..3).map(|_| cnf.add_var() as i32).collect();
            cnf.add_clause(vars.clone());
            let want_sat = bits != 0;
            // also constrain each var's polarity to the pattern
            for (i, v) in vars.iter().enumerate() {
                let lit = if (bits >> i) & 1 == 1 { *v } else { -*v };
                cnf.add_clause(vec![lit]);
            }
            match solve(&cnf) {
                SatResult::Sat(_) if want_sat => Ok(()),
                SatResult::Unsat if !want_sat => Ok(()),
                r => Err(format!("bits {} -> {:?}", bits, r)),
            }
        });
    }
    // Exactly-one over 4 vars: satisfiable, and the model has exactly one.
    c.add("sat_exactly_one_model_shape", || {
        let mut cnf = Cnf::new();
        let vars: Vec<i32> = (0..4).map(|_| cnf.add_var() as i32).collect();
        cnf.add_clause(vars.clone());
        for i in 0..4 {
            for j in i + 1..4 {
                cnf.add_clause(vec![-vars[i], -vars[j]]);
            }
        }
        match solve(&cnf) {
            SatResult::Sat(m) => {
                let ones = vars
                    .iter()
                    .filter(|v| m.get((**v - 1) as usize) == Some(&true))
                    .count();
                if ones != 1 {
                    Err(format!("model has {} ones", ones))
                } else {
                    Ok(())
                }
            }
            _ => Err("exactly-one should be SAT".into()),
        }
    });
}

fn sat_vs_bruteforce(c: &mut CaseList) {
    // Random 3-CNF over 6-9 vars, cross-check SAT solver vs brute force.
    for seed in 1..=40u64 {
        c.add(format!("sat_random_{}", seed), move || {
            let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15));
            let nv = 6 + (rng.below(4) as usize);
            let nc = 12 + (rng.below(12) as usize);
            let mut cnf = Cnf::new();
            for _ in 0..nv {
                cnf.add_var();
            }
            let mut clauses: Vec<Vec<i32>> = Vec::new();
            for _ in 0..nc {
                let mut cl = Vec::new();
                for _ in 0..3 {
                    let v = 1 + rng.below(nv as u64) as i32;
                    let lit = if rng.below(2) == 0 { v } else { -v };
                    if !cl.contains(&lit) {
                        cl.push(lit);
                    }
                }
                if cl.len() == 3 {
                    clauses.push(cl);
                }
            }
            for cl in &clauses {
                cnf.add_clause(cl.clone());
            }
            // Brute force.
            let n = 1u64 << nv;
            let mut sat_found = false;
            'outer: for assign in 0..n {
                for cl in &clauses {
                    let ok = cl.iter().any(|l| {
                        let v = l.unsigned_abs() as usize - 1;
                        let b = (assign >> v) & 1 == 1;
                        (*l > 0) == b
                    });
                    if !ok {
                        continue 'outer;
                    }
                }
                sat_found = true;
                break;
            }
            match solve(&cnf) {
                SatResult::Sat(m) => {
                    if !sat_found {
                        return Err("solver SAT but brute force UNSAT".into());
                    }
                    // verify model satisfies all clauses
                    for cl in &clauses {
                        let ok = cl
                            .iter()
                            .any(|l| m.get(l.unsigned_abs() as usize - 1) == Some(&(*l > 0)));
                        if !ok {
                            return Err("model violates a clause".into());
                        }
                    }
                    Ok(())
                }
                SatResult::Unsat => {
                    if sat_found {
                        Err("solver UNSAT but brute force SAT".into())
                    } else {
                        Ok(())
                    }
                }
            }
        });
    }
}

fn equivalence(c: &mut CaseList) {
    let circuits: Vec<(&'static str, &'static str)> = vec![
        ("and", "module m(input a, b, output y); assign y = a & b; endmodule"),
        ("or", "module m(input a, b, output y); assign y = a | b; endmodule"),
        ("xor", "module m(input a, b, output y); assign y = a ^ b; endmodule"),
        ("nand", "module m(input a, b, output y); assign y = ~(a & b); endmodule"),
        ("mux", "module m(input s, a, b, output y); assign y = s ? b : a; endmodule"),
        ("add4", "module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule"),
        ("sub4", "module m(input [3:0] a, b, output [3:0] d); assign d = a - b; endmodule"),
        ("cmp", "module m(input [3:0] a, b, output lt, eq); assign lt = a < b; assign eq = (a == b); endmodule"),
        ("parity", "module m(input [3:0] v, output p); assign p = ^v; endmodule"),
        ("decoder", "module m(input [1:0] sel, output [3:0] y); assign y = 4'd1 << sel; endmodule"),
        ("shift", "module m(input [3:0] a, [1:0] s, output [3:0] y); assign y = a << s; endmodule"),
        ("concat", "module m(input [3:0] a, [3:0] b, output [7:0] y); assign y = {a, b}; endmodule"),
        ("tree", "module m(input a, b, c, d, output y); assign y = (a & b) | (c & d); endmodule"),
        ("inv", "module m(input a, output y); assign y = ~a; endmodule"),
    ];
    for (name, src) in circuits {
        c.add(format!("equiv_synth_{}", name), move || {
            let d = cache(src);
            let gold = synthesize(
                d,
                &SynthOptions {
                    optimize: false,
                    map_lib: false,
                },
            )?
            .netlist;
            let gate = synthesize(
                d,
                &SynthOptions {
                    optimize: true,
                    map_lib: true,
                },
            )?
            .netlist;
            let r = check_equivalent(&gold, &gate)?;
            if r.equivalent {
                Ok(())
            } else {
                Err(format!(
                    "synthesized netlist differs from RTL: {:?}",
                    r.counterexample
                ))
            }
        });
    }
    // Deliberately different circuits must be reported non-equivalent.
    let pairs: Vec<(&'static str, &'static str, &'static str)> = vec![
        (
            "and_vs_or",
            "module m(input a, b, output y); assign y = a & b; endmodule",
            "module m(input a, b, output y); assign y = a | b; endmodule",
        ),
        (
            "xor_vs_xnor",
            "module m(input a, b, output y); assign y = a ^ b; endmodule",
            "module m(input a, b, output y); assign y = ~(a ^ b); endmodule",
        ),
        (
            "add1_vs_add2",
            "module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule",
            "module m(input [3:0] a, b, output [3:0] s); assign s = a + b + 4'd1; endmodule",
        ),
    ];
    for (name, a, b) in pairs {
        c.add(format!("equiv_negative_{}", name), move || {
            let ga = synthesize(
                cache(a),
                &SynthOptions {
                    optimize: false,
                    map_lib: false,
                },
            )?
            .netlist;
            let gb = synthesize(
                cache(b),
                &SynthOptions {
                    optimize: false,
                    map_lib: false,
                },
            )?
            .netlist;
            let r = check_equivalent(&ga, &gb)?;
            if r.equivalent {
                Err("different circuits claimed equivalent".into())
            } else {
                Ok(())
            }
        });
    }
}

fn bmc_cases(c: &mut CaseList) {
    // Property: q never exceeds 3 (trivially true for 2-bit) — check with a
    // `bad` wire asserted when q > 3 (constant false).
    c.add("bmc_safe_unreachable", || {
        let src: &'static str = "module m(input clk, rst, output reg [1:0] q, output bad);
            always @(posedge clk or posedge rst) if (rst) q <= 2'd0; else q <= q + 2'd1;
            assign bad = (q > 2'd3);
         endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let r = bmc(&nl, "bad", &BmcOptions::default())?;
        if r.violated {
            Err("unreachable bad reported reachable".into())
        } else {
            Ok(())
        }
    });
    // Reachable: q == 3 eventually (within 3 cycles after reset).
    c.add("bmc_reachable", || {
        let src: &'static str = "module m(input clk, rst, output reg [1:0] q, output bad);
            always @(posedge clk or posedge rst) if (rst) q <= 2'd0; else q <= q + 2'd1;
            assign bad = (q == 2'd3);
         endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let r = bmc(&nl, "bad", &BmcOptions::default())?;
        if r.violated {
            Ok(())
        } else {
            Err("reachable bad reported safe".into())
        }
    });
    // Invariant held by construction: dff output never both 0 and driven...
    // simpler: a flop initialized by reset to 0, property "q == 1 before any
    // clock" is safe at depth 0 but not reachable without d=1.
    c.add("bmc_flop_safe_property", || {
        let src: &'static str = "module m(input clk, d, rst, output reg q, output bad);
            always @(posedge clk or posedge rst) if (rst) q <= 1'b0; else q <= d;
            assign bad = q & rst;
         endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        // q && rst: after reset q=0, so bad is unreachable only if rst never
        // overlaps q=1. Since rst=1 forces q=0, bad should be unreachable.
        let r = bmc(&nl, "bad", &BmcOptions::default())?;
        if r.violated {
            Err("bad unexpectedly reachable".into())
        } else {
            Ok(())
        }
    });
}

fn induction_cases(c: &mut CaseList) {
    c.add("proof_async_reset_invariant_is_proved", || {
        let src: &'static str = "module m(input clk, d, rst, output reg q, output bad);
            always @(posedge clk or posedge rst)
                if (rst) q <= 1'b0; else q <= d;
            assign bad = q & rst;
         endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let r = prove(
            &nl,
            "bad",
            &BmcOptions {
                depth: 8,
                init_state_zero: true,
            },
        )?;
        if r.status != ProofStatus::Proved {
            Err(format!(
                "status={:?} report={}",
                r.status,
                eda_formal::proof_report(&r)
            ))
        } else if r.depth_checked != 8 || !r.inductive || r.bmc.violated {
            Err(format!(
                "depth={} inductive={} bmc={:?}",
                r.depth_checked, r.inductive, r.bmc
            ))
        } else if r.counterexample.is_some() || r.inductive_witness.is_some() {
            Err("proved result must not contain a witness".into())
        } else if !eda_formal::proof_report(&r).contains("PROVED by 8-induction") {
            Err("proof report omitted PROVED status".into())
        } else {
            Ok(())
        }
    });

    c.add("proof_reachable_property_is_refuted", || {
        let src: &'static str = "module m(input clk, d, output reg q, output bad);
            always @(posedge clk) q <= d;
            assign bad = q & d;
         endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let r = prove(
            &nl,
            "bad",
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        let trace_len = r.counterexample.as_ref().map(Vec::len);
        if r.status != ProofStatus::Refuted {
            Err(format!(
                "status={:?} report={}",
                r.status,
                eda_formal::proof_report(&r)
            ))
        } else if r.inductive || !r.bmc.violated || trace_len != Some(4) {
            Err(format!(
                "inductive={} bmc={:?} trace_len={:?}",
                r.inductive, r.bmc, trace_len
            ))
        } else if r.inductive_witness.is_some() {
            Err("refuted result must not contain an inductive witness".into())
        } else if !eda_formal::proof_report(&r).contains("REFUTED at bounded depth 3") {
            Err("proof report omitted REFUTED status".into())
        } else {
            Ok(())
        }
    });

    c.add("proof_unreachable_candidate_is_unknown", || {
        let src: &'static str = "module m(input clk, enable, output reg held, output bad);
            always @(posedge clk) held <= held;
            assign bad = held & enable;
         endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let r = prove(
            &nl,
            "bad",
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
        )?;
        let witness_len = r.inductive_witness.as_ref().map(Vec::len);
        if r.status != ProofStatus::Unknown {
            Err(format!(
                "status={:?} report={}",
                r.status,
                eda_formal::proof_report(&r)
            ))
        } else if r.inductive || r.bmc.violated {
            Err(format!("inductive={} bmc={:?}", r.inductive, r.bmc))
        } else if r.counterexample.is_some() || witness_len != Some(5) {
            Err(format!(
                "counterexample={:?} witness_len={:?}",
                r.counterexample, witness_len
            ))
        } else if r.inductive_witness.as_ref().and_then(|w| w[4].get("held")) != Some(&1)
            || r.inductive_witness.as_ref().and_then(|w| w[4].get("bad")) != Some(&1)
        {
            Err(format!(
                "final witness state={:?}",
                r.inductive_witness.as_ref().and_then(|w| w.get(4))
            ))
        } else if !eda_formal::proof_report(&r).contains("UNKNOWN") {
            Err("proof report omitted UNKNOWN status".into())
        } else {
            Ok(())
        }
    });

    c.add("proof_unknown_bad_net_is_rejected", || {
        let src: &'static str = "module m(input a, output y); assign y = a; endmodule";
        let nl = synthesize(
            cache(src),
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        match prove(
            &nl,
            "missing",
            &BmcOptions {
                depth: 2,
                init_state_zero: true,
            },
        ) {
            Err(e) if e.contains("missing") => Ok(()),
            other => Err(format!("unexpected result: {:?}", other)),
        }
    });

    c.add("proof_verified_invariant_strengthens_induction", || {
        let src: &'static str = "module m(input clk, enable, output reg held, output bad);
            always @(posedge clk) held <= held;
            assign bad = held & enable;
         endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let r = prove_with_invariants(
            &nl,
            "bad",
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
            &["held"],
        )?;
        if r.status != ProofStatus::Proved {
            Err(format!(
                "status={:?} report={}",
                r.status,
                eda_formal::proof_report(&r)
            ))
        } else if r.assumed_invariants != vec![ProofInvariant::new("held", false)]
            || r.failed_invariant.is_some()
            || r.invariant_result.is_some()
        {
            Err(format!(
                "invariant diagnostics: assumed={:?} failed={:?} nested={}",
                r.assumed_invariants,
                r.failed_invariant,
                r.invariant_result.is_some()
            ))
        } else if !eda_formal::proof_report(&r).contains("using proved invariants [held=0]") {
            Err("proof report omitted the verified invariant".into())
        } else {
            Ok(())
        }
    });

    c.add("proof_unproved_invariant_is_not_assumed", || {
        let src: &'static str = "module m(input clk, d, enable, output reg q, output bad);
            always @(posedge clk) q <= d;
            assign bad = q & enable;
         endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let r = prove_with_invariants(
            &nl,
            "bad",
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
            &["q"],
        )?;
        if r.status != ProofStatus::Unknown {
            Err(format!(
                "status={:?} report={}",
                r.status,
                eda_formal::proof_report(&r)
            ))
        } else if r.inductive || r.counterexample.is_some() {
            Err(format!(
                "inductive={} counterexample={:?}",
                r.inductive, r.counterexample
            ))
        } else if r.failed_invariant.as_deref() != Some("q") || r.invariant_result.is_none() {
            Err(format!(
                "failed={:?} nested={}",
                r.failed_invariant,
                r.invariant_result.is_some()
            ))
        } else if !eda_formal::proof_report(&r).contains("invariant 'q' could not be proved") {
            Err("proof report omitted invariant failure".into())
        } else {
            Ok(())
        }
    });

    c.add("proof_true_valued_invariant_strengthens_induction", || {
        let src: &'static str =
            "module m(input clk, enable, output reg held, output safe, output bad);
            always @(posedge clk) held <= held;
            assign safe = ~held;
            assign bad = held & enable;
         endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let r = prove_with_value_invariants(
            &nl,
            "bad",
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
            &[("safe", true)],
        )?;
        if r.status != ProofStatus::Proved {
            Err(format!(
                "status={:?} report={}",
                r.status,
                eda_formal::proof_report(&r)
            ))
        } else if r.assumed_invariants != vec![ProofInvariant::new("safe", true)] {
            Err(format!("assumed={:?}", r.assumed_invariants))
        } else if !eda_formal::proof_report(&r).contains("using proved invariants [safe=1]") {
            Err("proof report omitted the true-valued invariant".into())
        } else {
            Ok(())
        }
    });

    c.add("proof_unknown_invariant_net_is_rejected", || {
        let src: &'static str = "module m(input a, output y); assign y = a; endmodule";
        let nl = synthesize(
            cache(src),
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        match prove_with_invariants(
            &nl,
            "a",
            &BmcOptions {
                depth: 2,
                init_state_zero: true,
            },
            &["missing"],
        ) {
            Err(e) if e.contains("invariant net named 'missing'") => Ok(()),
            other => Err(format!("unexpected result: {:?}", other)),
        }
    });

    c.add("proof_boolean_expression_reset_property", || {
        let src: &'static str = "module m(input clk, d, rst, output reg q);
            always @(posedge clk or posedge rst)
                if (rst) q <= 1'b0; else q <= d;
         endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let property = BoolExpr::Not(Box::new(BoolExpr::And(
            Box::new(BoolExpr::Signal("q".into())),
            Box::new(BoolExpr::Signal("rst".into())),
        )));
        let r = eda_formal::prove_expression(
            &nl,
            &property,
            &BmcOptions {
                depth: 8,
                init_state_zero: true,
            },
        )?;
        if r.status != ProofStatus::Proved {
            Err(format!(
                "status={:?} report={}",
                r.status,
                eda_formal::proof_report(&r)
            ))
        } else {
            Ok(())
        }
    });

    c.add("proof_boolean_expression_reachable_violation", || {
        let src: &'static str = "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let property = BoolExpr::Not(Box::new(BoolExpr::And(
            Box::new(BoolExpr::Signal("q".into())),
            Box::new(BoolExpr::Signal("d".into())),
        )));
        let r = eda_formal::prove_expression(
            &nl,
            &property,
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
        )?;
        if r.status != ProofStatus::Refuted || r.counterexample.is_none() {
            Err(format!(
                "status={:?} counterexample={:?}",
                r.status, r.counterexample
            ))
        } else {
            Ok(())
        }
    });

    c.add("proof_boolean_expression_or_tautology", || {
        let src: &'static str = "module m(input a, output y); assign y = a; endmodule";
        let nl = synthesize(
            cache(src),
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let property = BoolExpr::Or(
            Box::new(BoolExpr::Signal("a".into())),
            Box::new(BoolExpr::Not(Box::new(BoolExpr::Signal("a".into())))),
        );
        let r = eda_formal::prove_expression(&nl, &property, &BmcOptions::default())?;
        if r.status != ProofStatus::Proved {
            Err(format!("status={:?}", r.status))
        } else {
            Ok(())
        }
    });

    c.add("proof_boolean_expression_implication_tautology", || {
        let src: &'static str = "module m(input a, output y); assign y = a; endmodule";
        let nl = synthesize(
            cache(src),
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let property = BoolExpr::Implication(
            Box::new(BoolExpr::Signal("a".into())),
            Box::new(BoolExpr::Signal("a".into())),
        );
        let r = eda_formal::prove_expression(&nl, &property, &BmcOptions::default())?;
        if r.status != ProofStatus::Proved {
            Err(format!("status={:?}", r.status))
        } else {
            Ok(())
        }
    });

    c.add("proof_boolean_expression_missing_signal", || {
        let src: &'static str = "module m(input a, output y); assign y = a; endmodule";
        let nl = synthesize(
            cache(src),
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        match eda_formal::prove_expression(
            &nl,
            &BoolExpr::Signal("missing".into()),
            &BmcOptions::default(),
        ) {
            Err(e) if e.contains("missing") => Ok(()),
            other => Err(format!("unexpected result: {:?}", other)),
        }
    });

    c.add("proof_expression_with_verified_net_invariant", || {
        let src: &'static str = "module m(input clk, enable, output reg held, output bad);
            always @(posedge clk) held <= held;
            assign bad = held & enable;
         endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let property = BoolExpr::Not(Box::new(BoolExpr::Signal("bad".into())));
        let r = eda_formal::prove_expression_with_value_invariants(
            &nl,
            &property,
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
            &[("held", false)],
        )?;
        if r.status != ProofStatus::Proved {
            Err(format!(
                "status={:?} report={}",
                r.status,
                eda_formal::proof_report(&r)
            ))
        } else if r.assumed_invariants != vec![ProofInvariant::new("held", false)] {
            Err(format!("assumed={:?}", r.assumed_invariants))
        } else if !eda_formal::proof_report(&r).contains("using proved invariants [held=0]") {
            Err("report omitted expression-proof invariant".into())
        } else {
            Ok(())
        }
    });

    c.add("proof_expression_rejects_unproved_net_invariant", || {
        let src: &'static str = "module m(input clk, d, enable, output reg q, output bad);
            always @(posedge clk) q <= d;
            assign bad = q & enable;
         endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let property = BoolExpr::Not(Box::new(BoolExpr::Signal("bad".into())));
        let r = eda_formal::prove_expression_with_value_invariants(
            &nl,
            &property,
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
            &[("q", false)],
        )?;
        if r.status != ProofStatus::Unknown || r.failed_invariant.as_deref() != Some("q") {
            Err(format!(
                "status={:?} failed={:?}",
                r.status, r.failed_invariant
            ))
        } else {
            Ok(())
        }
    });

    c.add(
        "proof_expression_with_verified_expression_invariant",
        || {
            let src: &'static str =
                "module m(input clk, enable, output reg held, output safe, output bad);
            always @(posedge clk) held <= held;
            assign safe = ~held;
            assign bad = held & enable;
         endmodule";
            let d = cache(src);
            let nl = synthesize(
                d,
                &SynthOptions {
                    optimize: true,
                    map_lib: false,
                },
            )?
            .netlist;
            let property = BoolExpr::Not(Box::new(BoolExpr::Signal("bad".into())));
            let invariant = BoolExpr::Signal("safe".into());
            let r = eda_formal::prove_expression_with_expression_invariants(
                &nl,
                &property,
                &BmcOptions {
                    depth: 4,
                    init_state_zero: true,
                },
                &[("safe_is_one", &invariant)],
            )?;
            if r.status != ProofStatus::Proved {
                Err(format!(
                    "status={:?} report={}",
                    r.status,
                    eda_formal::proof_report(&r)
                ))
            } else if r.assumed_invariants
                != vec![ProofInvariant::expression("safe_is_one", invariant.clone())]
            {
                Err(format!("assumed={:?}", r.assumed_invariants))
            } else if !eda_formal::proof_report(&r)
                .contains("using proved invariants [safe_is_one]")
            {
                Err("report omitted named expression invariant".into())
            } else {
                Ok(())
            }
        },
    );

    c.add(
        "proof_expression_rejects_unproved_expression_invariant",
        || {
            let src: &'static str = "module m(input clk, d, enable, output reg q, output bad);
            always @(posedge clk) q <= d;
            assign bad = q & enable;
         endmodule";
            let d = cache(src);
            let nl = synthesize(
                d,
                &SynthOptions {
                    optimize: true,
                    map_lib: false,
                },
            )?
            .netlist;
            let property = BoolExpr::Not(Box::new(BoolExpr::Signal("bad".into())));
            let invariant = BoolExpr::Not(Box::new(BoolExpr::Signal("q".into())));
            let r = eda_formal::prove_expression_with_expression_invariants(
                &nl,
                &property,
                &BmcOptions {
                    depth: 4,
                    init_state_zero: true,
                },
                &[("q_is_zero", &invariant)],
            )?;
            if r.status != ProofStatus::Unknown
                || r.failed_invariant.as_deref() != Some("q_is_zero")
            {
                Err(format!(
                    "status={:?} failed={:?}",
                    r.status, r.failed_invariant
                ))
            } else {
                Ok(())
            }
        },
    );

    c.add("formal_temporal_property_bounded_pass", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let assertions = eda_assert::parse_assertions(
            "property two_cycle;
                assert property (@(posedge clk) d |-> ##[2:2] q2);
             endproperty",
        )?;
        let results = eda_formal::bmc_assertions(
            &nl,
            &assertions,
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
        )?;
        if results[0].status != eda_formal::PropertyBmcStatus::BoundedPass
            || results[0].frames_checked != 7
        {
            Err(eda_formal::property_bmc_report(&results[0]))
        } else if !eda_formal::property_bmc_report(&results[0]).contains("BOUNDED PASS") {
            Err("temporal report omitted bounded status".into())
        } else {
            Ok(())
        }
    });

    c.add("formal_temporal_property_violation", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) d |-> ##[1:1] q2);")?;
        let results = eda_formal::bmc_assertions(
            &nl,
            &assertions,
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
        )?;
        if !results[0].violated()
            || results[0].trigger_frame.is_none()
            || results[0].trace.is_none()
        {
            Err(eda_formal::property_bmc_report(&results[0]))
        } else if !eda_formal::property_bmc_report(&results[0]).contains("VIOLATED") {
            Err("temporal report omitted violation".into())
        } else {
            Ok(())
        }
    });

    c.add("formal_temporal_property_rejects_edge_mismatch", || {
        let src: &'static str =
            "module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let assertions =
            eda_assert::parse_assertions("assert property (@(negedge clk) d |-> ##[1:1] q);")?;
        match eda_formal::bmc_assertion(&nl, &assertions[0], &BmcOptions::default()) {
            Err(e) if e.contains("negedge") => Ok(()),
            other => Err(format!("unexpected result: {:?}", other.is_ok())),
        }
    });

    c.add("formal_negedge_temporal_property", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(negedge clk) q1 <= d;
                always @(negedge clk) q2 <= q1;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let assertions = eda_assert::parse_assertions(
            "property neg_two_cycle;
                assert property (@(negedge clk) d |-> ##[2:2] q2);
             endproperty",
        )?;
        let results = eda_formal::bmc_assertions(
            &nl,
            &assertions,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        if results[0].status != eda_formal::PropertyBmcStatus::BoundedPass
            || results[0].frames_checked != 6
        {
            Err(eda_formal::property_bmc_report(&results[0]))
        } else {
            Ok(())
        }
    });

    c.add("formal_disable_iff_suppresses_frames", || {
        let src: &'static str = "module m(input clk, d, output reg q, output reset);
                always @(posedge clk) q <= d;
                assign reset = 1'b1;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let assertions = eda_assert::parse_assertions(
            "assert property (@(posedge clk) disable iff (reset) !q);",
        )?;
        let results = eda_formal::bmc_assertions(
            &nl,
            &assertions,
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
        )?;
        if results[0].status != eda_formal::PropertyBmcStatus::BoundedPass
            || results[0].trace.is_some()
        {
            Err(eda_formal::property_bmc_report(&results[0]))
        } else {
            Ok(())
        }
    });

    c.add("formal_immediate_cover_is_satisfied", || {
        let src: &'static str = "module m(input clk, d, output reg q);
                always @(posedge clk) q <= d;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let assertions = eda_assert::parse_assertions("cover property (@(posedge clk) d);")?;
        let results = eda_formal::bmc_assertions(
            &nl,
            &assertions,
            &BmcOptions {
                depth: 2,
                init_state_zero: true,
            },
        )?;
        if results[0].covered()
            && results[0].trigger_frame.is_some()
            && results[0].net_trace.is_some()
            && eda_formal::property_bmc_report(&results[0]).contains("COVER HIT")
        {
            Ok(())
        } else {
            Err(eda_formal::property_bmc_report(&results[0]))
        }
    });

    c.add("formal_temporal_cover_is_satisfied", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let assertions = eda_assert::parse_assertions(
            "property request_cover;
                cover property (@(posedge clk) d |-> ##[2:2] q2);
             endproperty",
        )?;
        let results = eda_formal::bmc_assertions(
            &nl,
            &assertions,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        if results[0].covered() && results[0].frames_checked == 6 && results[0].trace.is_some() {
            Ok(())
        } else {
            Err(eda_formal::property_bmc_report(&results[0]))
        }
    });

    c.add("formal_temporal_cover_is_not_satisfied", || {
        let src: &'static str = "module m(input clk, d, output reg q, output done);
                always @(posedge clk) q <= d;
                assign done = 1'b0;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let assertions =
            eda_assert::parse_assertions("cover property (@(posedge clk) d |-> ##[1:1] done);")?;
        let results = eda_formal::bmc_assertions(
            &nl,
            &assertions,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        if results[0].status == eda_formal::PropertyBmcStatus::NotSatisfied
            && results[0].trace.is_none()
            && eda_formal::property_bmc_report(&results[0]).contains("COVER MISS")
        {
            Ok(())
        } else {
            Err(eda_formal::property_bmc_report(&results[0]))
        }
    });

    c.add("formal_disable_iff_suppresses_cover", || {
        let src: &'static str = "module m(input clk, d, output reg q, output reset);
                always @(posedge clk) q <= d;
                assign reset = 1'b1;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let assertions =
            eda_assert::parse_assertions("cover property (@(posedge clk) disable iff (reset) d);")?;
        let results = eda_formal::bmc_assertions(
            &nl,
            &assertions,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        if results[0].status == eda_formal::PropertyBmcStatus::NotSatisfied {
            Ok(())
        } else {
            Err(eda_formal::property_bmc_report(&results[0]))
        }
    });

    c.add("formal_assumption_constrains_immediate_assertion", || {
        let src: &'static str = "module constrained(input clk, d, output reg q);
                always @(posedge clk) q <= d;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let properties = eda_assert::parse_assertions(
            r#"
            assert property (@(posedge clk) !d);
            assume property (@(posedge clk) !d);
        "#,
        )?;
        let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
            .into_iter()
            .partition(|property| property.kind != eda_assert::AssertionKind::Assume);
        let unconstrained = eda_formal::bmc_assertion(
            &nl,
            &assertions[0],
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        let constrained = eda_formal::bmc_assertions_with_assumptions(
            &nl,
            &assertions,
            &assumptions,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        if unconstrained.violated()
            && constrained[0].status == eda_formal::PropertyBmcStatus::BoundedPass
        {
            Ok(())
        } else {
            Err(eda_formal::property_bmc_report(&constrained[0]))
        }
    });

    c.add("formal_temporal_assumption_constrains_assertion", || {
        let src: &'static str = "module gated(input clk, d, en, mode, output reg state, output q);
                always @(posedge clk) state <= d;
                assign q = mode & en;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let properties = eda_assert::parse_assertions(
            r#"
            assert property (@(posedge clk) mode && d |-> ##[1:1] q);
            assume property (@(posedge clk) mode && d |-> ##[1:1] mode && en);
        "#,
        )?;
        let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
            .into_iter()
            .partition(|property| property.kind != eda_assert::AssertionKind::Assume);
        let results = eda_formal::bmc_assertions_with_assumptions(
            &nl,
            &assertions,
            &assumptions,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        if results[0].status == eda_formal::PropertyBmcStatus::BoundedPass
            && results[0].frames_checked == 6
        {
            Ok(())
        } else {
            Err(eda_formal::property_bmc_report(&results[0]))
        }
    });

    c.add("formal_inconsistent_assumptions_are_reported", || {
        let src: &'static str = "module conflict(input clk, d, output reg q);
                always @(posedge clk) q <= d;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let properties = eda_assert::parse_assertions(
            r#"
            assert property (@(posedge clk) d);
            assume property (@(posedge clk) d);
            assume property (@(posedge clk) !d);
        "#,
        )?;
        let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
            .into_iter()
            .partition(|property| property.kind != eda_assert::AssertionKind::Assume);
        let results = eda_formal::bmc_assertions_with_assumptions(
            &nl,
            &assertions,
            &assumptions,
            &BmcOptions::default(),
        )?;
        if results[0].status == eda_formal::PropertyBmcStatus::AssumptionsUnsat
            && eda_formal::property_bmc_report(&results[0]).contains("ASSUMPTIONS UNSAT")
        {
            Ok(())
        } else {
            Err(eda_formal::property_bmc_report(&results[0]))
        }
    });

    c.add("formal_assumption_constrains_cover", || {
        let src: &'static str = "module cover_gate(input clk, d, output reg q);
                always @(posedge clk) q <= d;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let properties = eda_assert::parse_assertions(
            r#"
            cover property (@(posedge clk) d);
            assume property (@(posedge clk) !d);
        "#,
        )?;
        let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
            .into_iter()
            .partition(|property| property.kind != eda_assert::AssertionKind::Assume);
        let results = eda_formal::bmc_assertions_with_assumptions(
            &nl,
            &assertions,
            &assumptions,
            &BmcOptions {
                depth: 2,
                init_state_zero: true,
            },
        )?;
        if results[0].status == eda_formal::PropertyBmcStatus::NotSatisfied {
            Ok(())
        } else {
            Err(eda_formal::property_bmc_report(&results[0]))
        }
    });

    c.add("formal_repeated_antecedent_violation_replays", || {
        let src: &'static str = "module repeat_shift(input clk, d, output reg q);
                always @(posedge clk) q <= d;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) d[*2] |-> !q);")?;
        let results = eda_formal::bmc_assertions(
            &nl,
            &assertions,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        let replay = eda_formal::verify_property_trace(&nl, &results[0])?;
        if results[0].violated()
            && results[0].repeat_count == Some(2)
            && replay.property_violation_reproduced
        {
            Ok(())
        } else {
            Err(eda_formal::property_bmc_report(&results[0]))
        }
    });

    c.add("formal_repeated_antecedent_assert_and_cover", || {
        let src: &'static str = "module repeat_shift(input clk, d, output reg q);
                always @(posedge clk) q <= d;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let properties = eda_assert::parse_assertions(
            r#"
            assert property (@(posedge clk) d[*2] |-> q);
            cover property (@(posedge clk) d[*2:3] |-> q);
        "#,
        )?;
        let results = eda_formal::bmc_assertions(
            &nl,
            &properties,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        if results[0].status == eda_formal::PropertyBmcStatus::BoundedPass
            && results[1].covered()
            && results[1].repeat_count.is_some()
        {
            Ok(())
        } else {
            Err(results
                .iter()
                .map(eda_formal::property_bmc_report)
                .collect::<String>())
        }
    });

    c.add("formal_repeated_assumption_constrains_assertion", || {
        let src: &'static str =
            "module repeated_gate(input clk, d, en, mode, output reg state, output q);
                always @(posedge clk) state <= d;
                assign q = mode & en;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let properties = eda_assert::parse_assertions(
            r#"
            assert property (@(posedge clk) (mode && d)[*2] |-> q);
            assume property (@(posedge clk) (mode && d)[*2] |-> mode && en);
        "#,
        )?;
        let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
            .into_iter()
            .partition(|property| property.kind != eda_assert::AssertionKind::Assume);
        let results = eda_formal::bmc_assertions_with_assumptions(
            &nl,
            &assertions,
            &assumptions,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        if results[0].status == eda_formal::PropertyBmcStatus::BoundedPass {
            Ok(())
        } else {
            Err(eda_formal::property_bmc_report(&results[0]))
        }
    });

    c.add("formal_unbounded_repetition_bmc", || {
        let src: &'static str = "module repeat_shift(input clk, d, output reg q);
                always @(posedge clk) q <= d;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let properties = eda_assert::parse_assertions(
            r#"
            assert property (@(posedge clk) d[*] |-> q);
            assert property (@(posedge clk) d[+] |=> q);
            cover property (@(posedge clk) d[+] |-> q);
        "#,
        )?;
        let results = eda_formal::bmc_assertions(
            &nl,
            &properties,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        if results[0].violated()
            && results[0].repeat_count == Some(0)
            && results[1].status == eda_formal::PropertyBmcStatus::BoundedPass
            && results[2].covered()
            && results[2].repeat_count.is_some()
        {
            Ok(())
        } else {
            Err(results
                .iter()
                .map(eda_formal::property_bmc_report)
                .collect::<String>())
        }
    });

    c.add("formal_unbounded_repetition_assumption", || {
        let src: &'static str =
            "module unbounded_gate(input clk, d, en, mode, output reg state, output q);
                always @(posedge clk) state <= d;
                assign q = mode & en;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let properties = eda_assert::parse_assertions(
            r#"
            assert property (@(posedge clk) (mode && d)[+] |-> q);
            assume property (@(posedge clk) (mode && d)[+] |-> mode && en);
        "#,
        )?;
        let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
            .into_iter()
            .partition(|property| property.kind != eda_assert::AssertionKind::Assume);
        let results = eda_formal::bmc_assertions_with_assumptions(
            &nl,
            &assertions,
            &assumptions,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        if results[0].status == eda_formal::PropertyBmcStatus::BoundedPass {
            Ok(())
        } else {
            Err(eda_formal::property_bmc_report(&results[0]))
        }
    });

    c.add("formal_nonconsecutive_repetition_bmc", || {
        let src: &'static str = "module sparse(input clk, d, output reg q);
                always @(posedge clk) q <= d;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let properties = eda_assert::parse_assertions(
            r#"
            assert property (@(posedge clk) d[=2] |-> q);
            cover property (@(posedge clk) d[=2] |-> q);
        "#,
        )?;
        let results = eda_formal::bmc_assertions(
            &nl,
            &properties,
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
        )?;
        let replay = eda_formal::verify_property_trace(&nl, &results[0])?;
        if results[0].violated()
            && results[0].repeat_count == Some(2)
            && results[0].end_frame.is_some()
            && replay.property_violation_reproduced
            && results[1].covered()
            && results[1].repeat_count == Some(2)
        {
            Ok(())
        } else {
            Err(results
                .iter()
                .map(eda_formal::property_bmc_report)
                .collect::<String>())
        }
    });

    c.add("formal_nonconsecutive_repetition_assumption", || {
        let src: &'static str =
            "module sparse_gate(input clk, d, en, mode, output reg state, output q);
                always @(posedge clk) state <= d;
                assign q = mode & en;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let properties = eda_assert::parse_assertions(
            r#"
            assert property (@(posedge clk) (mode && d)[=1] |-> q);
            assume property (@(posedge clk) (mode && d)[=1] |-> mode && en);
        "#,
        )?;
        let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
            .into_iter()
            .partition(|property| property.kind != eda_assert::AssertionKind::Assume);
        let results = eda_formal::bmc_assertions_with_assumptions(
            &nl,
            &assertions,
            &assumptions,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        if results[0].status == eda_formal::PropertyBmcStatus::BoundedPass {
            Ok(())
        } else {
            Err(eda_formal::property_bmc_report(&results[0]))
        }
    });

    c.add("formal_sequence_antecedent_bmc", || {
        let src: &'static str = "module sequence_m(input clk, a, b, output reg state, output q);
                always @(posedge clk) state <= a;
                assign q = b & state;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let properties = eda_assert::parse_assertions(
            r#"
            assert property (@(posedge clk) a ##[1:2] b |-> !q);
            cover property (@(posedge clk) a ##1 b |-> q);
        "#,
        )?;
        let results = eda_formal::bmc_assertions(
            &nl,
            &properties,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        let replay = eda_formal::verify_property_trace(&nl, &results[0])?;
        if results[0].violated()
            && matches!(results[0].repeat_count, Some(1 | 2))
            && results[0].end_frame.is_some()
            && replay.property_violation_reproduced
            && results[1].covered()
            && results[1].repeat_count == Some(1)
        {
            Ok(())
        } else {
            Err(results
                .iter()
                .map(eda_formal::property_bmc_report)
                .collect::<String>())
        }
    });

    c.add("formal_sequence_antecedent_assumption", || {
        let src: &'static str =
            "module sequence_gate(input clk, a, b, enable, output reg state, output q);
                always @(posedge clk) state <= a;
                assign q = b & state & enable;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let properties = eda_assert::parse_assertions(
            r#"
            assert property (@(posedge clk) a ##1 b |-> q);
            assume property (@(posedge clk) a ##1 b |-> enable);
        "#,
        )?;
        let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
            .into_iter()
            .partition(|property| property.kind != eda_assert::AssertionKind::Assume);
        let results = eda_formal::bmc_assertions_with_assumptions(
            &nl,
            &assertions,
            &assumptions,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        if results[0].status == eda_formal::PropertyBmcStatus::BoundedPass {
            Ok(())
        } else {
            Err(eda_formal::property_bmc_report(&results[0]))
        }
    });

    c.add("formal_temporal_property_rejects_wrong_clock", || {
        let src: &'static str = "module m(input real_clk, fake_clk, d, output reg q);
                always @(posedge real_clk) q <= d;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge fake_clk) d |-> ##[1:1] q);")?;
        match eda_formal::bmc_assertion(&nl, &assertions[0], &BmcOptions::default()) {
            Err(e) if e.contains("does not drive the design clock") => Ok(()),
            other => Err(format!("unexpected result: {:?}", other.is_ok())),
        }
    });

    c.add("formal_counterexample_exports_vcd", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) d |-> ##[1:1] q2);")?;
        let results = eda_formal::bmc_assertions(
            &nl,
            &assertions,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        let text = eda_formal::trace_net_vcd(&nl, results[0].net_trace.as_ref().unwrap())?;
        let waveform = eda_waveform::parse_vcd(&text)?;
        let trigger = results[0].trigger_frame.unwrap() as u64;
        if waveform.signal("d").is_none()
            || waveform.signal("clk").is_none()
            || waveform.signal("q1").is_none()
            || waveform.signal("q2").is_none()
            || waveform
                .signal("d")
                .unwrap()
                .value_at(trigger)
                .unwrap()
                .to_u64()
                != Some(1)
        {
            Err(text)
        } else {
            Ok(())
        }
    });

    c.add("formal_counterexample_netlist_replay", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) d |-> ##[1:1] q2);")?;
        let results = eda_formal::bmc_assertions(
            &nl,
            &assertions,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        let replay = eda_formal::verify_property_trace(&nl, &results[0])?;
        if replay.frames_verified != results[0].frames_checked
            || replay.transitions_verified != 2 * (replay.frames_verified - 1)
            || !replay.property_violation_reproduced
        {
            Err(format!(
                "frames={} transitions={} property={}",
                replay.frames_verified,
                replay.transitions_verified,
                replay.property_violation_reproduced
            ))
        } else {
            Ok(())
        }
    });

    c.add("formal_counterexample_vcd_replay", || {
        let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= q1;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let assertions =
            eda_assert::parse_assertions("assert property (@(posedge clk) d |-> ##[1:1] q2);")?;
        let results = eda_formal::bmc_assertions(
            &nl,
            &assertions,
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        let text = eda_formal::trace_property_vcd(&nl, &results[0])?;
        let replay = eda_formal::replay_property_vcd(&nl, &results[0], &text)?;
        if replay.frames_verified != results[0].frames_checked
            || !replay.property_violation_reproduced
        {
            Err(format!(
                "frames={} property={}",
                replay.frames_verified, replay.property_violation_reproduced
            ))
        } else {
            Ok(())
        }
    });

    c.add("formal_refuted_proof_exports_full_net_vcd", || {
        let src: &'static str = "module m(input clk, d, output reg q, output bad);
                always @(posedge clk) q <= d;
                assign bad = q & d;
             endmodule";
        let d = cache(src);
        let nl = synthesize(
            d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )?
        .netlist;
        let proof = eda_formal::prove(
            &nl,
            "bad",
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )?;
        if proof.status != ProofStatus::Refuted {
            return Err(eda_formal::proof_report(&proof));
        }
        let text = eda_api::export_proof_counterexample_vcd(&nl, &proof)?;
        let waveform = eda_waveform::parse_vcd(&text)?;
        if waveform.signal("q").is_none() || waveform.signal("bad").is_none() {
            Err(text)
        } else {
            Ok(())
        }
    });
}
