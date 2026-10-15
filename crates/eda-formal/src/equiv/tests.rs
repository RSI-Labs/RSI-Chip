use super::*;
use eda_hdl::compile;
use eda_synth::{synthesize, SynthOptions};

fn netlist_of(src: &str, opts: SynthOptions) -> eda_core::netlist::Netlist {
    let d = compile(src, None).unwrap();
    synthesize(&d, &opts).unwrap().netlist
}

#[test]
fn equivalence_checking_honors_cancellation() {
    let src = "module m(input a, b, output y); assign y = a & b; endmodule";
    let gold = netlist_of(src, SynthOptions::default());
    let mapped = gold.clone();
    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        check_equivalent_cancellable(&gold, &mapped, &cancellation).unwrap_err(),
        "equivalence checking cancelled"
    );
}

#[test]
fn structural_matcher_recurses_commutative_inputs_and_collects_pairs() {
    use eda_core::netlist::{Cell, Netlist as RawNetlist};

    let mut gold = RawNetlist::new("gold".to_string());
    let ga = gold.add_net("a");
    let gb = gold.add_net("b");
    let gc = gold.add_net("c");
    let gmid = gold.add_net("gmid");
    let gy = gold.add_net("y");
    gold.add_cell(Cell {
        name: "gand".into(),
        kind: CellKind::And,
        inputs: vec![ga, gb],
        output: gmid,
    });
    gold.add_cell(Cell {
        name: "gor".into(),
        kind: CellKind::Or,
        inputs: vec![gmid, gc],
        output: gy,
    });

    let mut gate = RawNetlist::new("gate".to_string());
    let ta = gate.add_net("a");
    let tb = gate.add_net("b");
    let tc = gate.add_net("c");
    let tmid = gate.add_net("tmid");
    let ty = gate.add_net("y");
    gate.add_cell(Cell {
        name: "tand".into(),
        kind: CellKind::And,
        inputs: vec![tb, ta],
        output: tmid,
    });
    gate.add_cell(Cell {
        name: "tor".into(),
        kind: CellKind::Or,
        inputs: vec![tmid, tc],
        output: ty,
    });

    let accepted = HashMap::from([(ga, ta), (gb, tb), (gc, tc)]);
    assert!(region_isomorphic(&gold, &gate, gy, ty, &accepted));
    let pairs = structural_matches(&gold, &gate, gy, ty, &accepted).unwrap();
    assert!(pairs.contains(&(gmid, tmid)), "{pairs:?}");
}

#[test]
fn failed_structural_match_preserves_independently_proved_subgraphs() {
    use eda_core::netlist::{Cell, Netlist as RawNetlist};

    let build = |prefix: &str, mismatched_input: &str| {
        let mut nl = RawNetlist::new(prefix.to_string());
        let a = nl.add_net(format!("{prefix}.a"));
        let b = nl.add_net(format!("{prefix}.b"));
        let c = nl.add_net(format!("{prefix}.c"));
        let other = nl.add_net(mismatched_input);
        let mid = nl.add_net(format!("{prefix}.mid"));
        let y = nl.add_net(format!("{prefix}.y"));
        nl.add_cell(Cell {
            name: "and".into(),
            kind: CellKind::And,
            inputs: vec![a, b],
            output: mid,
        });
        nl.add_cell(Cell {
            name: "or".into(),
            kind: CellKind::Or,
            inputs: vec![mid, other],
            output: y,
        });
        (nl, a, b, mid, y, c)
    };

    let (gold, ga, gb, gmid, gy, gc) = build("gold", "gold.c");
    let (gate, ta, tb, tmid, ty, _tc) = build("gate", "gate.d");
    let accepted = HashMap::from([(ga, ta), (gb, tb)]);
    let probe = structural_probe(&gold, &gate, gy, ty, &accepted);
    assert!(probe.root.is_none());
    let partial = probe.partial.into_iter().flatten().collect::<Vec<_>>();
    assert!(partial.contains(&(gmid, tmid)), "{partial:?}");
    assert!(!partial.contains(&(gc, _tc)));
}

#[test]
fn one_sided_flop_boundaries_do_not_fabricate_counterexamples() {
    use eda_core::netlist::{Cell, Netlist as RawNetlist};

    let build = |optimize_away_state: bool| {
        let mut nl = RawNetlist::new(if optimize_away_state {
            "gate".to_string()
        } else {
            "gold".to_string()
        });
        let clk = nl.add_net("clk");
        let a = nl.add_net("a");
        let q = nl.add_net("q");
        let d = nl.add_net("d");
        let zero = nl.add_net("zero");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "ff".into(),
            kind: CellKind::Dff { posedge: true },
            inputs: vec![clk, d],
            output: q,
        });
        if optimize_away_state {
            nl.add_cell(Cell {
                name: "tie0".into(),
                kind: CellKind::Const(eda_core::logic::LogicValue::Zero),
                inputs: vec![],
                output: y,
            });
        } else {
            nl.add_cell(Cell {
                name: "a_xor_a".into(),
                kind: CellKind::Xor,
                inputs: vec![a, a],
                output: zero,
            });
            nl.add_cell(Cell {
                name: "and".into(),
                kind: CellKind::And,
                inputs: vec![q, zero],
                output: y,
            });
        }
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("y", Dir::Out, vec![y]);
        let gold_flop = FlopRec {
            q_name: "q".into(),
            q,
            d,
            clk,
        };
        (nl, gold_flop)
    };

    let (gold, gold_flop) = build(false);
    let (gate, gate_flop) = build(true);
    let matched = [(&gold_flop, &gate_flop)];
    let gold_y = gold.net_by_name("y").unwrap();
    let gate_y = gate.net_by_name("y").unwrap();
    let verdict = prove_bit_equal(
        &gold,
        &gate,
        gold_y,
        gate_y,
        &matched,
        &[],
        None,
        &FlowCancellation::new(),
        true,
    )
    .unwrap();
    assert!(matches!(verdict, MiterVerdict::Equivalent));
}

#[test]
fn vector_region_proof_shares_wide_logic_cones() {
    use eda_core::netlist::{Cell, Netlist as RawNetlist};

    let build = |invert_one: bool| {
        let mut nl = RawNetlist::new("vector".to_string());
        let mut a = Vec::new();
        let mut b = Vec::new();
        let mut y = Vec::new();
        for bit in 0..4 {
            a.push(nl.add_net(format!("a[{bit}]")));
            b.push(nl.add_net(format!("b[{bit}]")));
            y.push(nl.add_net(format!("y[{bit}]")));
        }
        for bit in 0..4 {
            nl.add_cell(Cell {
                name: format!("xor{bit}"),
                kind: CellKind::Xor,
                inputs: if invert_one && bit == 0 {
                    vec![a[bit], a[bit]]
                } else {
                    vec![a[bit], b[bit]]
                },
                output: y[bit],
            });
        }
        for bit in 0..4 {
            nl.add_port(format!("a[{bit}]"), Dir::In, vec![a[bit]]);
            nl.add_port(format!("b[{bit}]"), Dir::In, vec![b[bit]]);
            nl.add_port(format!("y[{bit}]"), Dir::Out, vec![y[bit]]);
        }
        (nl, a, b, y)
    };

    let (gold, ga, gb, gy) = build(false);
    let (gate, ta, tb, ty) = build(false);
    let mut accepted = HashMap::new();
    for (gold_input, target_input) in ga.iter().zip(&ta) {
        accepted.insert(*gold_input, *target_input);
    }
    for (gold_input, target_input) in gb.iter().zip(&tb) {
        accepted.insert(*gold_input, *target_input);
    }
    let pairs = gy
        .iter()
        .zip(&ty)
        .map(|(g, t)| (*g, *t))
        .collect::<Vec<_>>();
    assert!(
        verify_vector_region(&gold, &gate, &pairs, &accepted, &FlowCancellation::new()).unwrap()
    );

    let (different, da, db, dy) = build(true);
    let mut different_accepted = HashMap::new();
    for (gold_input, target_input) in ga.iter().zip(&da) {
        different_accepted.insert(*gold_input, *target_input);
    }
    for (gold_input, target_input) in gb.iter().zip(&db) {
        different_accepted.insert(*gold_input, *target_input);
    }
    let different_pairs = gy
        .iter()
        .zip(&dy)
        .map(|(g, t)| (*g, *t))
        .collect::<Vec<_>>();
    assert!(!verify_vector_region(
        &gold,
        &different,
        &different_pairs,
        &different_accepted,
        &FlowCancellation::new()
    )
    .unwrap());
}

#[test]
fn vector_region_cegar_rejects_impossible_correlated_cuts() {
    use eda_core::netlist::{Cell, Netlist as RawNetlist};

    let width = 6usize;
    let build = |xnor_style: bool| {
        let mut nl = RawNetlist::new(if xnor_style {
            "gold".to_string()
        } else {
            "gate".to_string()
        });
        let mut sources = Vec::new();
        let mut p_cuts = Vec::new();
        let mut q_cuts = Vec::new();
        let mut outputs = Vec::new();
        for bit in 0..width {
            let a = nl.add_net(format!("a[{bit}]"));
            let p = nl.add_net(format!("p[{bit}]"));
            let q = nl.add_net(format!("q[{bit}]"));
            let y = nl.add_net(format!("y[{bit}]"));
            nl.add_cell(Cell {
                name: format!("p{bit}"),
                kind: CellKind::Buf,
                inputs: vec![a],
                output: p,
            });
            nl.add_cell(Cell {
                name: format!("q{bit}"),
                kind: CellKind::Not,
                inputs: vec![a],
                output: q,
            });
            nl.add_cell(Cell {
                name: format!("y{bit}"),
                kind: if xnor_style {
                    CellKind::And
                } else {
                    CellKind::Xnor
                },
                inputs: vec![p, q],
                output: y,
            });
            nl.add_port(format!("a[{bit}]"), Dir::In, vec![a]);
            nl.add_port(format!("y[{bit}]"), Dir::Out, vec![y]);
            sources.push(a);
            p_cuts.push(p);
            q_cuts.push(q);
            outputs.push(y);
        }
        (nl, p_cuts, q_cuts, outputs)
    };

    let (gold, gold_p, gold_q, gold_y) = build(true);
    let (gate, gate_p, gate_q, gate_y) = build(false);
    let mut accepted = HashMap::new();
    for (gn, tn) in gold_p.iter().zip(&gate_p) {
        accepted.insert(*gn, *tn);
    }
    for (gn, tn) in gold_q.iter().zip(&gate_q) {
        accepted.insert(*gn, *tn);
    }
    let pairs = gold_y
        .iter()
        .zip(&gate_y)
        .map(|(gn, tn)| (*gn, *tn))
        .collect::<Vec<_>>();
    assert!(
        verify_vector_region(&gold, &gate, &pairs, &accepted, &FlowCancellation::new()).unwrap()
    );
}

#[test]
fn complex_library_functions_do_not_use_flat_structural_classes() {
    use eda_core::library::LibFunc;

    assert_eq!(
        library_cell_class(&LibFunc::And(vec![LibFunc::Id(0), LibFunc::Id(1)]), 2),
        Some(FuncClass::And)
    );
    assert_eq!(
        library_cell_class(
            &LibFunc::And(vec![LibFunc::Not(Box::new(LibFunc::Id(0))), LibFunc::Id(1)]),
            2
        ),
        None
    );
}

#[test]
fn local_cnf_cache_key_is_exact_and_variable_order_independent() {
    let mut first = crate::sat::Cnf::new();
    for _ in 0..2 {
        first.add_var();
    }
    first.add_clause(vec![1, -2]);
    first.add_clause(vec![-1, 2]);

    let mut second = crate::sat::Cnf::new();
    for _ in 0..9 {
        second.add_var();
    }
    second.add_clause(vec![7, -9]);
    second.add_clause(vec![-7, 9]);

    assert_eq!(canonical_cnf_key(&first), canonical_cnf_key(&second));

    let mut different = first.clone();
    different.add_clause(vec![1]);
    assert_ne!(canonical_cnf_key(&first), canonical_cnf_key(&different));
}

#[test]
fn accepted_fanout_proposes_the_next_structural_pair() {
    use eda_core::netlist::{Cell, Netlist as RawNetlist};

    let build = |prefix: &str| {
        let mut nl = RawNetlist::new(prefix.to_string());
        let a = nl.add_net(format!("{prefix}.a"));
        let b = nl.add_net(format!("{prefix}.b"));
        let c = nl.add_net(format!("{prefix}.c"));
        let mid = nl.add_net(format!("{prefix}.mid"));
        let y = nl.add_net(format!("{prefix}.y"));
        nl.add_cell(Cell {
            name: "and".into(),
            kind: CellKind::And,
            inputs: vec![a, b],
            output: mid,
        });
        nl.add_cell(Cell {
            name: "or".into(),
            kind: CellKind::Or,
            inputs: vec![mid, c],
            output: y,
        });
        (nl, a, b, c, mid, y)
    };

    let (gold, _ga, _gb, _gc, gmid, gy) = build("gold");
    let (gate, _ta, _tb, _tc, tmid, ty) = build("gate");
    let accepted = HashMap::from([(gmid, tmid)]);
    let proposals = direct_fanout_candidates(&gold, &gate, gmid, tmid, &accepted);
    assert_eq!(proposals, vec![(gy, ty)]);
}

#[test]
fn flop_correspondence_uses_names_not_cell_order() {
    use eda_core::netlist::{Cell, Netlist as RawNetlist};

    let build = |reverse: bool| {
        let mut nl = RawNetlist::new("flops".to_string());
        let clk = nl.add_net("clk");
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let q0 = nl.add_net("q0");
        let q1 = nl.add_net("q1");
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("q0", Dir::Out, vec![q0]);
        nl.add_port("q1", Dir::Out, vec![q1]);
        let cells = [
            Cell {
                name: "q0".into(),
                kind: CellKind::Dff { posedge: true },
                inputs: vec![clk, a],
                output: q0,
            },
            Cell {
                name: "q1".into(),
                kind: CellKind::Dff { posedge: true },
                inputs: vec![clk, b],
                output: q1,
            },
        ];
        if reverse {
            nl.add_cell(cells[1].clone());
            nl.add_cell(cells[0].clone());
        } else {
            nl.add_cell(cells[0].clone());
            nl.add_cell(cells[1].clone());
        }
        nl
    };

    let result = check_equivalent(&build(false), &build(true)).unwrap();
    assert!(result.equivalent, "{:?}", result.notes);
}

#[test]
fn local_regions_tie_same_named_free_nets() {
    let src = "module m(input a, output y); wire z; assign y = a & z; endmodule";
    let gold = netlist_of(
        src,
        SynthOptions {
            optimize: false,
            map_lib: false,
        },
    );
    let mapped = netlist_of(src, SynthOptions::default());
    let result = check_equivalent(&gold, &mapped).unwrap();
    assert!(
        result.equivalent,
        "{:?} {:?}",
        result.notes, result.counterexample
    );
}

#[test]
fn equivalent_duplicate_logic_supports_many_to_one_correspondence() {
    let src = "module m(input a, b, output y);
        wire p = a & b;
        wire q = a & b;
        assign y = p | q;
     endmodule";
    let gold = netlist_of(
        src,
        SynthOptions {
            optimize: false,
            map_lib: true,
        },
    );
    let mapped = netlist_of(src, SynthOptions::default());
    let result = check_equivalent(&gold, &mapped).unwrap();
    assert!(
        result.equivalent,
        "{:?} {:?}",
        result.notes, result.counterexample
    );
}

#[test]
fn bounded_region_helper_respects_cuts_and_cap() {
    let mut nl = eda_core::netlist::Netlist::new("region".to_string());
    let a = nl.add_net("a");
    let b = nl.add_net("b");
    let y = nl.add_net("y");
    nl.add_cell(eda_core::netlist::Cell {
        name: "and".into(),
        kind: CellKind::And,
        inputs: vec![a, b],
        output: y,
    });
    let cuts = [a].into_iter().collect::<NetIdSet>();
    assert_eq!(bounded_region_len(&nl, y, |net| cuts.contains(net), 128), 3);
    assert_eq!(
        bounded_region_len(&nl, y, |net| cuts.contains(net), 1),
        usize::MAX
    );

    let c = nl.add_net("one");
    nl.add_cell(eda_core::netlist::Cell {
        name: "tie1".into(),
        kind: CellKind::Const(eda_core::logic::LogicValue::One),
        inputs: vec![],
        output: c,
    });
    assert_eq!(explicit_constant(&nl, c), Some(true));
}

#[test]
fn opt_vs_no_opt_equivalent() {
    let src = "module m(input [3:0] a, b, output [3:0] s, co);
        assign {co, s} = {1'b0, a} + {1'b0, b};
     endmodule";
    let g = netlist_of(
        src,
        SynthOptions {
            optimize: false,
            map_lib: true,
        },
    );
    let t = netlist_of(src, SynthOptions::default());
    let r = check_equivalent(&g, &t).unwrap();
    assert!(r.equivalent, "{:?} {:?}", r.notes, r.counterexample);
    let repeat = check_equivalent(&g, &t).unwrap();
    assert_eq!(
        r.notes, repeat.notes,
        "correspondence induction is not deterministic"
    );
}

#[test]
fn cut_refinement_rejects_impossible_correlated_cut_models() {
    use eda_core::netlist::{Cell, Netlist as RawNetlist};

    let build = |xor_style: bool| {
        let mut nl = RawNetlist::new(if xor_style {
            "gold".to_string()
        } else {
            "gate".to_string()
        });
        let a = nl.add_net("a");
        let p = nl.add_net("p");
        let q = nl.add_net("q");
        let correlated = nl.add_net("correlated");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "p_is_a".into(),
            kind: CellKind::Buf,
            inputs: vec![a],
            output: p,
        });
        nl.add_cell(Cell {
            name: "q_is_not_a".into(),
            kind: CellKind::Not,
            inputs: vec![a],
            output: q,
        });
        nl.add_cell(Cell {
            name: "correlated".into(),
            kind: if xor_style {
                CellKind::And
            } else {
                CellKind::Xnor
            },
            inputs: vec![p, q],
            output: correlated,
        });
        nl.add_cell(Cell {
            name: "y".into(),
            kind: CellKind::Or,
            inputs: vec![a, correlated],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y", Dir::Out, vec![y]);
        (nl, p, q, y)
    };

    let (gold, gp, gq, gy) = build(true);
    let (gate, tp, tq, ty) = build(false);
    let points = [(gp, tp, false), (gq, tq, false)];
    let verdict = prove_bit_equal(
        &gold,
        &gate,
        gy,
        ty,
        &[],
        &points,
        None,
        &FlowCancellation::new(),
        true,
    )
    .unwrap();
    assert!(matches!(verdict, MiterVerdict::Equivalent));
}

#[test]
fn cut_refinement_generalizes_across_correlated_source_families() {
    use eda_core::netlist::{Cell, Netlist as RawNetlist};

    let width = 6usize;
    let build = |xnor_style: bool| {
        let mut nl = RawNetlist::new(if xnor_style {
            "gold".to_string()
        } else {
            "gate".to_string()
        });
        let mut points = Vec::new();
        let mut terms = Vec::new();
        for i in 0..width {
            let a = nl.add_net(format!("a{i}"));
            let p = nl.add_net(format!("p{i}"));
            let q = nl.add_net(format!("q{i}"));
            let term = nl.add_net(format!("term{i}"));
            nl.add_cell(Cell {
                name: format!("p{i}_is_a"),
                kind: CellKind::Buf,
                inputs: vec![a],
                output: p,
            });
            nl.add_cell(Cell {
                name: format!("q{i}_is_not_a"),
                kind: CellKind::Not,
                inputs: vec![a],
                output: q,
            });
            nl.add_cell(Cell {
                name: format!("term{i}"),
                kind: if xnor_style {
                    CellKind::And
                } else {
                    CellKind::Xnor
                },
                inputs: vec![p, q],
                output: term,
            });
            nl.add_port(format!("a{i}"), Dir::In, vec![a]);
            points.push((p, q));
            terms.push(term);
        }
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "y".into(),
            kind: CellKind::Or,
            inputs: terms,
            output: y,
        });
        nl.add_port("y", Dir::Out, vec![y]);
        (nl, points, y)
    };

    let (gold, gold_points, gy) = build(true);
    let (gate, gate_points, ty) = build(false);
    let points = gold_points
        .iter()
        .zip(&gate_points)
        .flat_map(|((gp, gq), (tp, tq))| [(*gp, *tp, false), (*gq, *tq, false)])
        .collect::<Vec<_>>();
    let verdict = prove_bit_equal(
        &gold,
        &gate,
        gy,
        ty,
        &[],
        &points,
        None,
        &FlowCancellation::new(),
        true,
    )
    .unwrap();
    assert!(matches!(verdict, MiterVerdict::Equivalent));
}

#[test]
fn mutated_netlist_inequivalent_with_real_cex() {
    use eda_core::library::CellLibrary;

    use std::sync::Arc;
    let src = "module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule";
    let g = netlist_of(src, SynthOptions::default());
    let mut t = g.clone();
    // flip one XOR2 into XNOR2
    let lib = Arc::new(CellLibrary::std());
    for c in t.cells_mut() {
        if let CellKind::Lib(id) = c.kind {
            if lib.get(id).name == "XOR2" {
                c.kind = CellKind::Lib(lib.find("XNOR2").unwrap());
                break;
            }
        }
    }
    let r = check_equivalent(&g, &t).unwrap();
    assert!(!r.equivalent);
    let cex = r.counterexample.unwrap();
    // verify the counterexample actually distinguishes via simulation
    let gd = eda_core::to_design::netlist_to_design(&g);
    let td = eda_core::to_design::netlist_to_design(&t);
    let mut gs = eda_sim::Simulator::new(&gd).unwrap();
    let mut ts = eda_sim::Simulator::new(&td).unwrap();
    for (name, w) in &cex {
        gs.set_input(name, w).unwrap();
        ts.set_input(name, w).unwrap();
    }
    let y1 = gs.read_port("s").unwrap();
    let y2 = ts.read_port("s").unwrap();
    assert_ne!(y1.bits, y2.bits);
}

#[test]
fn sequential_counter_equivalent() {
    let src = "module m(input clk, rst, output reg [3:0] q);
        always @(posedge clk or posedge rst) if (rst) q <= 4'd0; else q <= q + 4'd1;
     endmodule";
    let g = netlist_of(
        src,
        SynthOptions {
            optimize: false,
            map_lib: true,
        },
    );
    let t = netlist_of(src, SynthOptions::default());
    let r = check_equivalent(&g, &t).unwrap();
    assert!(r.equivalent, "{:?}", r.notes);
}

#[test]
fn broken_counter_inequivalent() {
    let good = "module m(input clk, rst, output reg [3:0] q);
        always @(posedge clk or posedge rst) if (rst) q <= 4'd0; else q <= q + 4'd1; endmodule";
    let bad = "module m(input clk, rst, output reg [3:0] q);
        always @(posedge clk or posedge rst) if (rst) q <= 4'd0; else q <= q + 4'd2; endmodule";
    let g = netlist_of(good, SynthOptions::default());
    let t = netlist_of(bad, SynthOptions::default());
    let r = check_equivalent(&g, &t).unwrap();
    assert!(!r.equivalent, "counters +1 and +2 must differ");
}

#[test]
fn two_descriptions_of_same_function() {
    // RTL conditional vs arithmetic
    let a = "module m(input [1:0] s, output y); assign y = (s == 2'd2); endmodule";
    let b = "module m(input [1:0] s, output y); assign y = s[1] & ~s[0]; endmodule";
    let g = netlist_of(a, SynthOptions::default());
    let t = netlist_of(b, SynthOptions::default());
    let r = check_equivalent(&g, &t).unwrap();
    assert!(r.equivalent, "{:?} {:?}", r.notes, r.counterexample);
}

#[test]
fn port_mismatch_rejected() {
    let a = "module m(input x, output y); assign y = x; endmodule";
    let b = "module m(input x, z, output y); assign y = x & z; endmodule";
    let g = netlist_of(a, SynthOptions::default());
    let t = netlist_of(b, SynthOptions::default());
    let r = check_equivalent(&g, &t).unwrap();
    assert!(!r.equivalent);
}

#[test]
fn flop_count_mismatch_detected() {
    let a = "module m(input clk, d, output reg q); always @(posedge clk) q <= 1'b1; endmodule";
    let b = "module m(input clk, d, output reg q); reg r; always @(posedge clk) begin r <= d; q <= r ^ d; end endmodule";
    let g = netlist_of(a, SynthOptions::default());
    let t = netlist_of(b, SynthOptions::default());
    assert_ne!(
        g.stats().seq,
        t.stats().seq,
        "test netlists g={:?} t={:?}",
        g.stats(),
        t.stats()
    );
    let r = check_equivalent(&g, &t).unwrap();
    assert!(!r.equivalent);
    assert!(
        r.notes.iter().any(|n| n.contains("flop count")),
        "notes={:?}",
        r.notes
    );
}
