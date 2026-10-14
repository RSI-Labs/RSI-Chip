//! Netlist construction, evaluation, topology and maintenance cases.

use crate::CaseList;
use eda_core::library::CellLibrary;
use eda_core::logic::{LogicValue as LV, LogicWord};
use eda_core::netlist::{Cell, CellKind, Dir, Netlist};
use std::sync::Arc;

pub fn register(c: &mut CaseList) {
    gate_eval(c);
    build_query(c);
    topo(c);
    compact(c);
    lib_cells(c);
    stats(c);
}

fn gate_eval(c: &mut CaseList) {
    // Every 2-input gate over all 4 input value pairs.
    let kinds = [
        ("And", CellKind::And),
        ("Nand", CellKind::Nand),
        ("Or", CellKind::Or),
        ("Nor", CellKind::Nor),
        ("Xor", CellKind::Xor),
        ("Xnor", CellKind::Xnor),
    ];
    let vals = [LV::Zero, LV::One, LV::X, LV::Z];
    for (name, kind) in kinds {
        for a in vals {
            for b in vals {
                let kind = kind.clone();
                let expected = ref_gate(name, a, b);
                c.add(
                    format!("cell_{}_{}{}", name, a.to_char(), b.to_char()),
                    move || {
                        if kind.eval(&[a, b]) != expected {
                            Err(format!("{}({:?},{:?})", name, a, b))
                        } else {
                            Ok(())
                        }
                    },
                );
            }
        }
    }
    // Buf/Not/Const.
    for v in vals {
        let kind = CellKind::Not;
        let want = match v {
            LV::Zero => LV::One,
            LV::One => LV::Zero,
            _ => LV::X,
        };
        c.add(format!("cell_not_{}", v.to_char()), move || {
            if kind.eval(&[v]) != want {
                Err("not".into())
            } else {
                Ok(())
            }
        });
        c.add(format!("cell_buf_{}", v.to_char()), move || {
            if CellKind::Buf.eval(&[v]) != v {
                Err("buf".into())
            } else {
                Ok(())
            }
        });
    }
    for v in vals {
        let want = v;
        c.add(format!("cell_const_{}", v.to_char()), move || {
            if CellKind::Const(v).eval(&[]) != want {
                Err("const".into())
            } else {
                Ok(())
            }
        });
    }
    // Mux: [sel, in0, in1].
    let mvals = [LV::Zero, LV::One, LV::X];
    for s in mvals {
        for a in mvals {
            for b in mvals {
                let (s, a, b) = (s, a, b);
                let expected = match s {
                    LV::Zero => a,
                    LV::One => b,
                    _ => {
                        let m = a.resolve(b);
                        if m.is_unknown() {
                            LV::X
                        } else {
                            m
                        }
                    }
                };
                c.add(
                    format!("cell_mux_{}{}{}", s.to_char(), a.to_char(), b.to_char()),
                    move || {
                        if CellKind::Mux.eval(&[s, a, b]) != expected {
                            Err(format!("mux {:?}", (s, a, b)))
                        } else {
                            Ok(())
                        }
                    },
                );
            }
        }
    }
    // 3-input associative gates.
    for bits in 0..8u64 {
        let ins = vec![
            LV::bit(bits & 1 == 1),
            LV::bit((bits >> 1) & 1 == 1),
            LV::bit((bits >> 2) & 1 == 1),
        ];
        let ones = bits.count_ones();
        let checks: Vec<(&str, LV)> = vec![
            ("And", if ones == 3 { LV::One } else { LV::Zero }),
            ("Or", if ones > 0 { LV::One } else { LV::Zero }),
            ("Xor", LV::bit(ones % 2 == 1)),
        ];
        for (name, want) in checks {
            let kind = match name {
                "And" => CellKind::And,
                "Or" => CellKind::Or,
                _ => CellKind::Xor,
            };
            let ins = ins.clone();
            c.add(format!("cell3_{}_{}", name, bits), move || {
                let got = kind.eval(&ins);
                if got != want {
                    Err(format!(
                        "{}3 {} ins={:?} got={:?} want={:?}",
                        name, bits, ins, got, want
                    ))
                } else {
                    Ok(())
                }
            });
        }
    }
}

fn ref_gate(name: &str, a: LV, b: LV) -> LV {
    let n = |v: LV| if v.is_unknown() { LV::X } else { v };
    let (a, b) = (n(a), n(b));
    match name {
        "And" => {
            if a == LV::Zero || b == LV::Zero {
                LV::Zero
            } else if a == LV::One && b == LV::One {
                LV::One
            } else {
                LV::X
            }
        }
        "Nand" => ref_gate("And", a, b).not(),
        "Or" => {
            if a == LV::One || b == LV::One {
                LV::One
            } else if a == LV::Zero && b == LV::Zero {
                LV::Zero
            } else {
                LV::X
            }
        }
        "Nor" => ref_gate("Or", a, b).not(),
        "Xor" => match (a.as_bool(), b.as_bool()) {
            (Some(x), Some(y)) => LV::bit(x ^ y),
            _ => LV::X,
        },
        "Xnor" => ref_gate("Xor", a, b).not(),
        _ => LV::X,
    }
}

fn build_query(c: &mut CaseList) {
    c.add("nl_build_basic", || {
        let mut nl = Netlist::new("top");
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "g".into(),
            kind: CellKind::And,
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        if nl.net_count() != 3 || nl.cell_count() != 1 || nl.ports.len() != 3 {
            Err("counts".into())
        } else if nl.net_by_name("b") != Some(b) || nl.net_by_name("zz").is_some() {
            Err("lookup".into())
        } else {
            Ok(())
        }
    });
    c.add("nl_port_by_name", || {
        let mut nl = Netlist::new("t");
        let x = nl.add_net("x");
        nl.add_port("x", Dir::In, vec![x]);
        let p = nl.port_by_name("x").ok_or("missing port")?;
        if p.bits != vec![x] {
            Err("bits".into())
        } else {
            Ok(())
        }
    });
    // Multi-bit port, MSB-first bit order.
    c.add("nl_vector_port_order", || {
        let mut nl = Netlist::new("t");
        let b3 = nl.add_net("b3");
        let b2 = nl.add_net("b2");
        let b1 = nl.add_net("b1");
        let b0 = nl.add_net("b0");
        nl.add_port("b", Dir::In, vec![b3, b2, b1, b0]);
        let p = nl.port_by_name("b").unwrap();
        if p.bits != vec![b3, b2, b1, b0] {
            Err("port bit order".into())
        } else {
            Ok(())
        }
    });
    // Driver/sink adjacency.
    c.add("nl_adjacency", || {
        let mut nl = Netlist::new("t");
        let a = nl.add_net("a");
        let y = nl.add_net("y");
        let z = nl.add_net("z");
        let g1 = nl.add_cell(Cell {
            name: "n1".into(),
            kind: CellKind::Not,
            inputs: vec![a],
            output: y,
        });
        let g2 = nl.add_cell(Cell {
            name: "n2".into(),
            kind: CellKind::Not,
            inputs: vec![y],
            output: z,
        });
        if nl.net(a).sinks != vec![g1]
            || nl.net(y).drivers != vec![g1]
            || nl.net(y).sinks != vec![g2]
            || nl.net(z).drivers != vec![g2]
        {
            Err("adjacency wrong".into())
        } else {
            Ok(())
        }
    });
    // retarget_cell_output moves the driver.
    c.add("nl_retarget", || {
        let mut nl = Netlist::new("t");
        let a = nl.add_net("a");
        let y = nl.add_net("y");
        let w = nl.add_net("w");
        let g = nl.add_cell(Cell {
            name: "n".into(),
            kind: CellKind::Not,
            inputs: vec![a],
            output: y,
        });
        nl.retarget_cell_output(g, w);
        if !nl.net(y).drivers.is_empty() || nl.net(w).drivers != vec![g] {
            Err("retarget".into())
        } else {
            Ok(())
        }
    });
    // replace_cell re-registers adjacency.
    c.add("nl_replace_cell", || {
        let mut nl = Netlist::new("t");
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        let g = nl.add_cell(Cell {
            name: "n".into(),
            kind: CellKind::Not,
            inputs: vec![a],
            output: y,
        });
        nl.replace_cell(
            g,
            Cell {
                name: "n2".into(),
                kind: CellKind::And,
                inputs: vec![a, b],
                output: y,
            },
        );
        if nl.net(y).drivers != vec![g] || nl.net(a).sinks != vec![g] || nl.net(b).sinks != vec![g]
        {
            Err("replace".into())
        } else {
            Ok(())
        }
    });
}

fn topo(c: &mut CaseList) {
    // Chain respects data order.
    c.add("nl_topo_chain", || {
        let mut nl = Netlist::new("t");
        let a = nl.add_net("a");
        let mut prev = a;
        let mut order = Vec::new();
        for i in 0..6 {
            let out = nl.add_net(format!("n{}", i));
            order.push(nl.add_cell(Cell {
                name: format!("g{}", i),
                kind: CellKind::Not,
                inputs: vec![prev],
                output: out,
            }));
            prev = out;
        }
        let topo = nl.topo_cells().ok_or("loop?")?;
        for (k, &cell) in order.iter().enumerate() {
            let pos = topo.iter().position(|&x| x == cell).ok_or("missing")?;
            if pos != k {
                return Err(format!("position {} != {}", pos, k));
            }
        }
        Ok(())
    });
    // Sequential cells break combinational cycles.
    c.add("nl_topo_seq_breaks_loop", || {
        let mut nl = Netlist::new("t");
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let q = nl.add_net("q");
        let fb = nl.add_net("fb");
        nl.add_cell(Cell {
            name: "ff".into(),
            kind: CellKind::Dff { posedge: true },
            inputs: vec![clk, d],
            output: q,
        });
        nl.add_cell(Cell {
            name: "inv".into(),
            kind: CellKind::Not,
            inputs: vec![q],
            output: fb,
        });
        nl.topo_cells()
            .map(|_| ())
            .ok_or_else(|| "should be acyclic through flop".to_string())
    });
    // Pure combinational loop detected.
    c.add("nl_topo_comb_loop", || {
        let mut nl = Netlist::new("t");
        let a = nl.add_net("a");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "b1".into(),
            kind: CellKind::Buf,
            inputs: vec![y],
            output: a,
        });
        nl.add_cell(Cell {
            name: "b2".into(),
            kind: CellKind::Buf,
            inputs: vec![a],
            output: y,
        });
        if nl.topo_cells().is_some() {
            Err("loop not detected".into())
        } else {
            Ok(())
        }
    });
    // Fan-out diamond orders source before sinks.
    c.add("nl_topo_diamond", || {
        let mut nl = Netlist::new("t");
        let a = nl.add_net("a");
        let m = nl.add_net("m");
        let l = nl.add_net("l");
        let r = nl.add_net("r");
        let y = nl.add_net("y");
        let inv = nl.add_cell(Cell {
            name: "i".into(),
            kind: CellKind::Not,
            inputs: vec![a],
            output: m,
        });
        let g1 = nl.add_cell(Cell {
            name: "l".into(),
            kind: CellKind::And,
            inputs: vec![m, a],
            output: l,
        });
        let g2 = nl.add_cell(Cell {
            name: "r".into(),
            kind: CellKind::Or,
            inputs: vec![m, a],
            output: r,
        });
        let g3 = nl.add_cell(Cell {
            name: "y".into(),
            kind: CellKind::Xor,
            inputs: vec![l, r],
            output: y,
        });
        let topo = nl.topo_cells().ok_or("loop?")?;
        let pos = |id: usize| topo.iter().position(|&x| x == id).unwrap();
        if !(pos(inv) < pos(g1) && pos(inv) < pos(g2) && pos(g1) < pos(g3) && pos(g2) < pos(g3)) {
            Err("diamond order".into())
        } else {
            Ok(())
        }
    });
}

fn compact(c: &mut CaseList) {
    c.add("nl_compact_drops_dead_cell", || {
        let mut nl = Netlist::new("t");
        let a = nl.add_net("a");
        let dead = nl.add_net("dead");
        let y = nl.add_net("y");
        let g1 = nl.add_cell(Cell {
            name: "d".into(),
            kind: CellKind::Not,
            inputs: vec![a],
            output: dead,
        });
        let g2 = nl.add_cell(Cell {
            name: "y".into(),
            kind: CellKind::Not,
            inputs: vec![a],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y", Dir::Out, vec![y]);
        nl.disconnect_cell(g1);
        nl.compact(&[g1]);
        if nl.cell_count() != 1 || nl.net_by_name("dead").is_some() {
            Err("dead not removed".into())
        } else {
            let _ = g2;
            Ok(())
        }
    });
    c.add("nl_compact_keeps_port_nets", || {
        let mut nl = Netlist::new("t");
        let a = nl.add_net("a");
        let _unused = nl.add_net("unused");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "g".into(),
            kind: CellKind::Not,
            inputs: vec![a],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y", Dir::Out, vec![y]);
        nl.compact(&[]);
        if nl.net_by_name("a").is_none()
            || nl.net_by_name("y").is_none()
            || nl.net_by_name("unused").is_some()
        {
            Err("port nets must survive, unused must go".into())
        } else {
            Ok(())
        }
    });
    c.add("nl_compact_renumbers_ports", || {
        let mut nl = Netlist::new("t");
        let a = nl.add_net("a");
        let d1 = nl.add_net("d1");
        let y = nl.add_net("y");
        let g1 = nl.add_cell(Cell {
            name: "d".into(),
            kind: CellKind::Not,
            inputs: vec![a],
            output: d1,
        });
        nl.add_cell(Cell {
            name: "g".into(),
            kind: CellKind::Not,
            inputs: vec![d1],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y", Dir::Out, vec![y]);
        nl.disconnect_cell(g1);
        nl.compact(&[g1]);
        let p = nl.port_by_name("y").ok_or("port lost")?;
        let ynet = p.bits[0];
        if nl.net(ynet).drivers.is_empty() {
            Err("y should be undriven after removing its driver".into())
        } else {
            Ok(())
        }
    });
}

fn lib_cells(c: &mut CaseList) {
    // Library cell evaluation through Netlist::eval_cell.
    let cases: Vec<(&str, Vec<LV>, LV)> = vec![
        ("INV", vec![LV::Zero], LV::One),
        ("INV", vec![LV::One], LV::Zero),
        ("BUF", vec![LV::X], LV::X),
        ("AND2", vec![LV::One, LV::One], LV::One),
        ("AND2", vec![LV::One, LV::Zero], LV::Zero),
        ("OR2", vec![LV::Zero, LV::One], LV::One),
        ("NAND2", vec![LV::One, LV::One], LV::Zero),
        ("NOR2", vec![LV::Zero, LV::Zero], LV::One),
        ("XOR2", vec![LV::One, LV::One], LV::Zero),
        ("XNOR2", vec![LV::One, LV::Zero], LV::Zero),
        ("MUX2", vec![LV::Zero, LV::One, LV::Zero], LV::One),
        ("MUX2", vec![LV::One, LV::One, LV::Zero], LV::Zero),
        ("TIE0", vec![], LV::Zero),
        ("TIE1", vec![], LV::One),
    ];
    for (i, (cell, ins, want)) in cases.into_iter().enumerate() {
        c.add(format!("nl_libcell_{}_{}", i, cell), move || {
            let lib = Arc::new(CellLibrary::std());
            let mut nl = Netlist::new("t");
            nl.set_library(lib.clone());
            let id = lib.find(cell).ok_or("cell missing")?;
            let mut nets = Vec::new();
            for (k, _) in ins.iter().enumerate() {
                nets.push(nl.add_net(format!("i{}", k)));
            }
            let y = nl.add_net("y");
            nl.add_cell(Cell {
                name: "u".into(),
                kind: CellKind::Lib(id),
                inputs: nets.clone(),
                output: y,
            });
            let mut vals = std::collections::HashMap::new();
            for (k, v) in ins.iter().enumerate() {
                vals.insert(nets[k], *v);
            }
            vals.insert(y, LV::X);
            if nl.eval_cell(0, &vals) != want {
                Err(format!("{} -> wrong", cell))
            } else {
                Ok(())
            }
        });
    }
    // Sequential library cell classification.
    c.add("nl_lib_seq_classification", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("t");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let y = nl.add_net("y");
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        nl.add_cell(Cell {
            name: "comb".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![a],
            output: y,
        });
        let q2 = nl.add_net("q");
        nl.add_cell(Cell {
            name: "ff".into(),
            kind: CellKind::Lib(lib.find("DFF").unwrap()),
            inputs: vec![clk, d],
            output: q2,
        });
        if nl.cell_is_sequential(&nl.cell(0).kind) || !nl.cell_is_sequential(&nl.cell(1).kind) {
            Err("seq classification".into())
        } else {
            Ok(())
        }
    });
    // eval_cell on Dff returns X (not combinational).
    c.add("nl_dff_eval_is_x", || {
        let mut nl = Netlist::new("t");
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let q = nl.add_net("q");
        nl.add_cell(Cell {
            name: "ff".into(),
            kind: CellKind::Dff { posedge: true },
            inputs: vec![clk, d],
            output: q,
        });
        let mut vals = std::collections::HashMap::new();
        vals.insert(clk, LV::One);
        vals.insert(d, LV::One);
        if nl.eval_cell(0, &vals) != LV::X {
            Err("Dff must not evaluate combinationally".into())
        } else {
            Ok(())
        }
    });
    // Vector port wiring evaluated through eval_cell map.
    c.add("nl_eval_const_driver", || {
        let mut nl = Netlist::new("t");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "c".into(),
            kind: CellKind::Const(LV::One),
            inputs: vec![],
            output: y,
        });
        if nl.eval_cell(0, &std::collections::HashMap::new()) != LV::One {
            Err("const driver".into())
        } else {
            Ok(())
        }
    });
}

fn stats(c: &mut CaseList) {
    c.add("nl_stats_mixed", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("t");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let q = nl.add_net("q");
        nl.add_cell(Cell {
            name: "g".into(),
            kind: CellKind::And,
            inputs: vec![a, b],
            output: y,
        });
        let cn = nl.add_net("c");
        nl.add_cell(Cell {
            name: "c".into(),
            kind: CellKind::Const(LV::Zero),
            inputs: vec![],
            output: cn,
        });
        nl.add_cell(Cell {
            name: "ff".into(),
            kind: CellKind::Lib(lib.find("DFF").unwrap()),
            inputs: vec![clk, d],
            output: q,
        });
        let s = nl.stats();
        if s.comb != 1 || s.consts != 1 || s.lib_cells != 1 || s.seq != 1 || s.cells != 3 {
            Err(format!("stats {:?}", s))
        } else {
            Ok(())
        }
    });
    // Word helper used to keep the LogicWord import meaningful for vector ports.
    c.add("nl_word_width_helper", || {
        let w = LogicWord::ones(4);
        if w.width() != 4 || !w.is_fully_known() {
            Err("helper".into())
        } else {
            Ok(())
        }
    });
}
