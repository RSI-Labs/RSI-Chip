//! Technology mapping: bind generic gates to standard-cell library cells,
//! including peephole merges (`AND+INV → NAND2` etc.). Mapped cells drive
//! the original net directly, so no bridging buffers are introduced.

use eda_core::library::CellLibrary;
use eda_core::netlist::{Cell, CellKind, NetId, Netlist};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

pub fn map_to_library(nl: &Netlist, lib: &CellLibrary) -> Result<Netlist, String> {
    let mut out = Netlist::new(nl.name.clone());
    for _ in 0..nl.net_count() {
        out.add_net("");
    }
    for (i, n) in nl.nets().iter().enumerate() {
        out.set_net_name(i, n.name.clone());
    }
    for p in &nl.ports {
        out.add_port(p.name.clone(), p.dir, p.bits.clone());
    }
    let mut m = Mapper {
        nl,
        out,
        lib,
        tmp: 0,
    };
    m.run()?;
    m.out.set_library(Arc::new(lib.clone()));
    Ok(m.out)
}

struct Mapper<'a> {
    nl: &'a Netlist,
    out: Netlist,
    lib: &'a CellLibrary,
    tmp: usize,
}

impl<'a> Mapper<'a> {
    fn fresh(&mut self, hint: &str) -> NetId {
        let name = format!("{}_n{}", hint, self.tmp);
        self.tmp += 1;
        self.out.add_net(name)
    }

    /// Emit one library cell driving `out` directly.
    fn emit(&mut self, cell_name: &str, inputs: Vec<NetId>, out: NetId) -> Result<(), String> {
        let id = self
            .lib
            .find(cell_name)
            .ok_or_else(|| format!("library has no cell '{}'", cell_name))?;
        let def = self.lib.get(id);
        if def.inputs.len() != inputs.len() {
            return Err(format!(
                "cell '{}' expects {} pins, got {}",
                cell_name,
                def.inputs.len(),
                inputs.len()
            ));
        }
        self.tmp += 1;
        self.out.add_cell(Cell {
            name: format!("u{}_{}", self.tmp, cell_name.to_lowercase()),
            kind: CellKind::Lib(id),
            inputs,
            output: out,
        });
        Ok(())
    }

    fn fresh_emit(&mut self, cell_name: &str, inputs: Vec<NetId>) -> Result<NetId, String> {
        let y = self.fresh("m");
        self.emit(cell_name, inputs, y)?;
        Ok(y)
    }

    fn run(&mut self) -> Result<(), String> {
        // Peephole: 2-input And/Or/Xor whose only consumer is a Not.
        let port_bits: HashSet<NetId> = self
            .nl
            .ports
            .iter()
            .flat_map(|port| port.bits.iter().copied())
            .collect();
        let mut consumed: HashSet<usize> = HashSet::new();
        let mut merges: BTreeMap<usize, &'static str> = BTreeMap::new(); // not cell -> cell name
        for ci in 0..self.nl.cell_count() {
            if self.nl.cell(ci).kind != CellKind::Not {
                continue;
            }
            let src = self.nl.cell(ci).inputs[0];
            if port_bits.contains(&src) {
                continue;
            }
            if self.nl.net(src).sinks.len() != 1 {
                continue;
            }
            let ds = self.nl.net(src).drivers.clone();
            if ds.len() != 1 || consumed.contains(&ds[0]) {
                continue;
            }
            let d = ds[0];
            let (dkind, dins) = (self.nl.cell(d).kind.clone(), self.nl.cell(d).inputs.clone());
            let name = match (&dkind, dins.len()) {
                (CellKind::And, 2) => "NAND2",
                (CellKind::Or, 2) => "NOR2",
                (CellKind::Xor, 2) => "XNOR2",
                _ => continue,
            };
            consumed.insert(d);
            merges.insert(ci, name);
        }

        for ci in 0..self.nl.cell_count() {
            if consumed.contains(&ci) {
                continue;
            }
            if merges.contains_key(&ci) {
                // handled in the second sweep below (needs driver inputs)
                continue;
            }
            let c = self.nl.cell(ci);
            let ins: Vec<NetId> = c.inputs.clone();
            let out = c.output;
            match &c.kind {
                CellKind::Const(v) => {
                    let name = if *v == eda_core::logic::LogicValue::One {
                        "TIE1"
                    } else {
                        "TIE0"
                    };
                    self.emit(name, vec![], out)?;
                }
                CellKind::Not => {
                    self.emit("INV", vec![ins[0]], out)?;
                }
                CellKind::Buf => {
                    self.emit("BUF", vec![ins[0]], out)?;
                }
                CellKind::Mux => {
                    self.emit("MUX2", vec![ins[0], ins[1], ins[2]], out)?;
                }
                CellKind::And | CellKind::Or | CellKind::Xor => {
                    let base = match c.kind {
                        CellKind::And => "AND2",
                        CellKind::Or => "OR2",
                        _ => "XOR2",
                    };
                    self.tree_to(base, &ins, out)?;
                }
                CellKind::Nand | CellKind::Nor | CellKind::Xnor => {
                    let (base, direct) = match c.kind {
                        CellKind::Nand => ("AND2", "NAND2"),
                        CellKind::Nor => ("OR2", "NOR2"),
                        _ => ("XOR2", "XNOR2"),
                    };
                    if ins.len() == 2 {
                        self.emit(direct, ins, out)?;
                    } else {
                        let y = self.tree(base, &ins)?;
                        self.emit("INV", vec![y], out)?;
                    }
                }
                CellKind::Dff { posedge } => {
                    let clk = if *posedge {
                        ins[0]
                    } else {
                        self.fresh_emit("INV", vec![ins[0]])?
                    };
                    self.emit("DFF", vec![clk, ins[1]], out)?;
                }
                CellKind::DffR {
                    posedge,
                    reset_high,
                } => {
                    let clk = if *posedge {
                        ins[0]
                    } else {
                        self.fresh_emit("INV", vec![ins[0]])?
                    };
                    let rst = if *reset_high {
                        ins[2]
                    } else {
                        self.fresh_emit("INV", vec![ins[2]])?
                    };
                    self.emit("DFFR", vec![clk, ins[1], rst], out)?;
                }
                CellKind::Lib(_) => {
                    self.out.add_cell(Cell {
                        name: c.name.clone(),
                        kind: c.kind.clone(),
                        inputs: ins,
                        output: out,
                    });
                }
                CellKind::BlockRom { .. } => {
                    // Keep the macro boundary intact. A block ROM is deliberately
                    // not exploded into standard-cell logic during mapping.
                    self.out.add_cell(Cell {
                        name: c.name.clone(),
                        kind: c.kind.clone(),
                        inputs: ins,
                        output: out,
                    });
                }
            }
        }
        for (not_ci, name) in &merges {
            let not_cell = self.nl.cell(*not_ci);
            let src = not_cell.inputs[0];
            let d = self.nl.net(src).drivers[0];
            let dins = self.nl.cell(d).inputs.clone();
            self.emit(name, dins, not_cell.output)?;
        }
        Ok(())
    }

    fn tree(&mut self, base: &str, ins: &[NetId]) -> Result<NetId, String> {
        let mut acc = ins[0];
        for &input in ins.iter().skip(1) {
            let y = self.fresh("t");
            self.emit(base, vec![acc, input], y)?;
            acc = y;
        }
        Ok(acc)
    }

    fn tree_to(&mut self, base: &str, ins: &[NetId], out: NetId) -> Result<(), String> {
        if ins.len() == 1 {
            self.emit("BUF", vec![ins[0]], out)?;
            return Ok(());
        }
        let mut acc = ins[0];
        for k in 1..ins.len() {
            let last = k == ins.len() - 1;
            let target = if last { out } else { self.fresh("t") };
            self.emit(base, vec![acc, ins[k]], target)?;
            acc = target;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::netlist::Dir;

    #[test]
    fn maps_onto_original_nets() {
        let lib = CellLibrary::std();
        let mut nl = Netlist::new("m");
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::And,
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let m = map_to_library(&nl, &lib).unwrap();
        assert_eq!(m.cell_count(), 1);
        assert!(matches!(&m.cell(0).kind, CellKind::Lib(_)));
        assert_eq!(m.cell(0).output, y);
        assert!(m.library().is_some());
    }

    #[test]
    fn negedge_flop_gets_clock_inverter() {
        let lib = CellLibrary::std();
        let mut nl = Netlist::new("m");
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let q = nl.add_net("q");
        nl.add_cell(Cell {
            name: "ff".into(),
            kind: CellKind::Dff { posedge: false },
            inputs: vec![clk, d],
            output: q,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("q", Dir::Out, vec![q]);
        let m = map_to_library(&nl, &lib).unwrap();
        assert_eq!(m.cell_count(), 2); // INV + DFF
        let names: Vec<String> = m
            .cells()
            .iter()
            .map(|c| match &c.kind {
                CellKind::Lib(id) => lib.get(*id).name.clone(),
                k => format!("{:?}", k),
            })
            .collect();
        assert!(names.contains(&"INV".to_string()));
        assert!(names.contains(&"DFF".to_string()));
    }
}
