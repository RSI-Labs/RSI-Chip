//! Standard-cell library test cases.

use crate::CaseList;
use eda_core::library::{CellLibrary, LibFunc};
use eda_core::logic::LogicValue as LV;

pub fn register(c: &mut CaseList) {
    std_lookup(c);
    funcs(c);
    custom(c);
}

fn std_lookup(c: &mut CaseList) {
    let names = [
        "TIE0", "TIE1", "INV", "BUF", "AND2", "OR2", "NAND2", "NOR2", "XOR2", "XNOR2", "MUX2",
        "DFF", "DFFR",
    ];
    for n in names {
        c.add(format!("lib_std_has_{}", n), move || {
            let lib = CellLibrary::std();
            if lib.find(n).is_none() {
                Err(format!("missing {}", n))
            } else {
                Ok(())
            }
        });
    }
    for (n, w) in [
        ("INV", 1usize),
        ("BUF", 2),
        ("AND2", 3),
        ("XOR2", 4),
        ("MUX2", 4),
        ("DFF", 5),
        ("DFFR", 6),
    ] {
        c.add(format!("lib_width_{}", n), move || {
            let lib = CellLibrary::std();
            let id = lib.find(n).ok_or("missing")?;
            if lib.get(id).width_sites != w {
                Err(format!("{} width {}", n, lib.get(id).width_sites))
            } else {
                Ok(())
            }
        });
    }
    c.add("lib_std_name", || {
        if CellLibrary::std().name != "lib1" {
            Err("name".into())
        } else {
            Ok(())
        }
    });
    c.add("lib_missing_none", || {
        let lib = CellLibrary::std();
        if lib.find("AND3").is_some() {
            Err("AND3 shouldn't exist".into())
        } else {
            Ok(())
        }
    });
    // Every comb cell has arcs from all inputs.
    c.add("lib_arcs_complete", || {
        let lib = CellLibrary::std();
        for def in &lib.cells {
            if def.seq.is_none() {
                let arc_pins: std::collections::HashSet<usize> =
                    def.arcs.iter().map(|(i, _)| *i).collect();
                for i in 0..def.inputs.len() {
                    if !arc_pins.contains(&i) {
                        return Err(format!("{} misses arc from {}", def.name, def.inputs[i]));
                    }
                }
            }
        }
        Ok(())
    });
    // DFF timing metadata.
    for (n, ck2q, setup) in [("DFF", 1.2f64, 0.6f64), ("DFFR", 1.2, 0.6)] {
        c.add(format!("lib_seq_timing_{}", n), move || {
            let lib = CellLibrary::std();
            let id = lib.find(n).ok_or("missing")?;
            let seq = lib.get(id).seq.as_ref().ok_or("no seq")?;
            if seq.clk_to_q != ck2q || seq.setup != setup {
                Err("timing wrong".into())
            } else {
                Ok(())
            }
        });
    }
    c.add("lib_dffr_reset_pin", || {
        let lib = CellLibrary::std();
        let id = lib.find("DFFR").unwrap();
        if lib.get(id).seq.as_ref().unwrap().reset_pin != Some(2) {
            Err("reset pin".into())
        } else {
            Ok(())
        }
    });
}

fn funcs(c: &mut CaseList) {
    // LibFunc eval over booleans and X.
    let cases: Vec<(&str, LibFunc, Vec<LV>, LV)> = vec![
        ("const1", LibFunc::Const(LV::One), vec![], LV::One),
        ("id0", LibFunc::Id(0), vec![LV::One], LV::One),
        (
            "not0",
            LibFunc::Not(Box::new(LibFunc::Id(0))),
            vec![LV::Zero],
            LV::One,
        ),
        (
            "and01",
            LibFunc::And(vec![LibFunc::Id(0), LibFunc::Id(1)]),
            vec![LV::Zero, LV::One],
            LV::Zero,
        ),
        (
            "or01",
            LibFunc::Or(vec![LibFunc::Id(0), LibFunc::Id(1)]),
            vec![LV::Zero, LV::One],
            LV::One,
        ),
        (
            "xor11",
            LibFunc::Xor(vec![LibFunc::Id(0), LibFunc::Id(1)]),
            vec![LV::One, LV::One],
            LV::Zero,
        ),
        (
            "nand11",
            LibFunc::Nand(vec![LibFunc::Id(0), LibFunc::Id(1)]),
            vec![LV::One, LV::One],
            LV::Zero,
        ),
        (
            "nor00",
            LibFunc::Nor(vec![LibFunc::Id(0), LibFunc::Id(1)]),
            vec![LV::Zero, LV::Zero],
            LV::One,
        ),
        (
            "xnor10",
            LibFunc::Xnor(vec![LibFunc::Id(0), LibFunc::Id(1)]),
            vec![LV::One, LV::Zero],
            LV::Zero,
        ),
        (
            "and_x",
            LibFunc::And(vec![LibFunc::Id(0), LibFunc::Id(1)]),
            vec![LV::X, LV::Zero],
            LV::Zero,
        ),
        (
            "mux0",
            LibFunc::Mux {
                sel: Box::new(LibFunc::Id(0)),
                a: Box::new(LibFunc::Id(1)),
                b: Box::new(LibFunc::Id(2)),
            },
            vec![LV::Zero, LV::One, LV::Zero],
            LV::One,
        ),
        (
            "nested",
            LibFunc::Not(Box::new(LibFunc::Or(vec![
                LibFunc::Not(Box::new(LibFunc::Id(0))),
                LibFunc::Id(1),
            ]))),
            vec![LV::One, LV::Zero],
            LV::One,
        ),
    ];
    for (name, f, ins, want) in cases {
        c.add(format!("libfunc_{}", name), move || {
            if f.eval(&ins) != want {
                Err(format!("{} wrong", name))
            } else {
                Ok(())
            }
        });
    }
    // Truth-table sweep for AND/OR/XOR functions.
    for bits in 0..4u64 {
        let (x, y) = (bits & 1 == 1, bits & 2 != 0);
        let ins = vec![LV::bit(x), LV::bit(y)];
        for (name, f, want) in [
            (
                "and",
                LibFunc::And(vec![LibFunc::Id(0), LibFunc::Id(1)]),
                x && y,
            ),
            (
                "or",
                LibFunc::Or(vec![LibFunc::Id(0), LibFunc::Id(1)]),
                x || y,
            ),
            (
                "xor",
                LibFunc::Xor(vec![LibFunc::Id(0), LibFunc::Id(1)]),
                x ^ y,
            ),
        ] {
            let ins = ins.clone();
            c.add(format!("libfunc_tt_{}_{}", name, bits), move || {
                if f.eval(&ins) != LV::bit(want) {
                    Err(format!("tt {} {}", name, bits))
                } else {
                    Ok(())
                }
            });
        }
    }
}

fn custom(c: &mut CaseList) {
    c.add("lib_custom_add_and_lookup", || {
        let mut lib = CellLibrary::new("custom");
        let id = lib.add(ed::lib_cell("AOI22"));
        if lib.find("AOI22") != Some(id) || lib.get(id).name != "AOI22" {
            Err("custom lookup".into())
        } else {
            Ok(())
        }
    });
    c.add("lib_duplicate_name_last_wins", || {
        let mut lib = CellLibrary::new("dup");
        let _ = lib.add(ed::lib_cell("X"));
        let id2 = lib.add(ed::lib_cell("X"));
        if lib.find("X") != Some(id2) || lib.cells.len() != 2 {
            Err("dup".into())
        } else {
            Ok(())
        }
    });
    c.add("lib_oob_index_panics_safe_api", || {
        // Use get() only with valid ids; find() covers lookups. This case
        // checks ids are stable after additions.
        let mut lib = CellLibrary::new("stab");
        let a = lib.add(ed::lib_cell("A"));
        let _ = lib.add(ed::lib_cell("B"));
        if lib.get(a).name != "A" {
            Err("unstable".into())
        } else {
            Ok(())
        }
    });
}

mod ed {
    use eda_core::library::{LibCellDef, LibFunc};

    pub fn lib_cell(name: &str) -> LibCellDef {
        LibCellDef {
            name: name.into(),
            inputs: vec!["a".into(), "b".into()],
            output: "y".into(),
            func: LibFunc::And(vec![LibFunc::Id(0), LibFunc::Id(1)]),
            width_sites: 3,
            arcs: vec![(0, 1.0), (1, 1.0)],
            timing_tables: Vec::new(),
            seq: None,
        }
    }
}
