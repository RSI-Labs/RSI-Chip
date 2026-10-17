//! Tseitin encoding of gate-level netlists into CNF.

use crate::sat::Cnf;
use eda_core::library::LibFunc;
use eda_core::netlist::{CellKind, NetId, Netlist};
use std::collections::HashMap;

/// Incremental encoder over one netlist.
pub struct NetlistEncoder<'a> {
    nl: &'a Netlist,
    pub cnf: Cnf,
    pub var: HashMap<NetId, i32>,
    // memoized per-cell output literals keyed by cell id (for shared encoding)
    cell_lit: HashMap<usize, i32>,
}

impl<'a> NetlistEncoder<'a> {
    pub fn new(nl: &'a Netlist) -> Self {
        NetlistEncoder {
            nl,
            cnf: Cnf::new(),
            var: HashMap::new(),
            cell_lit: HashMap::new(),
        }
    }

    pub fn fresh(&mut self) -> i32 {
        self.cnf.add_var() as i32
    }

    pub fn lit_of(&mut self, net: NetId) -> i32 {
        if let Some(&l) = self.var.get(&net) {
            return l;
        }
        let l = self.fresh();
        self.var.insert(net, l);
        l
    }

    pub fn set_lit(&mut self, net: NetId, lit: i32) {
        self.var.insert(net, lit);
    }

    pub fn has_lit(&self, net: NetId) -> bool {
        self.var.contains_key(&net)
    }

    // ---- gate encodings ----

    pub fn enc_and(&mut self, a: i32, b: i32, o: i32) {
        self.cnf.add_clause(vec![-a, -b, o]);
        self.cnf.add_clause(vec![a, -o]);
        self.cnf.add_clause(vec![b, -o]);
    }

    pub fn enc_or(&mut self, a: i32, b: i32, o: i32) {
        self.cnf.add_clause(vec![a, b, -o]);
        self.cnf.add_clause(vec![-a, o]);
        self.cnf.add_clause(vec![-b, o]);
    }

    pub fn enc_xor(&mut self, a: i32, b: i32, o: i32) {
        self.cnf.add_clause(vec![-a, -b, -o]);
        self.cnf.add_clause(vec![-a, b, o]);
        self.cnf.add_clause(vec![a, -b, o]);
        self.cnf.add_clause(vec![a, b, -o]);
    }

    pub fn enc_not(&mut self, a: i32, o: i32) {
        self.cnf.add_clause(vec![-a, -o]);
        self.cnf.add_clause(vec![a, o]);
    }

    pub fn enc_equal(&mut self, a: i32, b: i32) {
        self.cnf.add_clause(vec![-a, b]);
        self.cnf.add_clause(vec![a, -b]);
    }

    /// Full truth-table encoding for k-input functions (k <= 4).
    fn enc_truthtable(&mut self, ins: &[i32], eval: &dyn Fn(&[bool]) -> bool, o: i32) {
        let k = ins.len();
        for m in 0u32..(1 << k) {
            let input: Vec<bool> = (0..k).map(|i| (m >> i) & 1 == 1).collect();
            let f = eval(&input);
            let mut cl: Vec<i32> = Vec::new();
            for (i, b) in input.iter().enumerate() {
                cl.push(if *b { -ins[i] } else { ins[i] });
            }
            cl.push(if f { o } else { -o });
            self.cnf.add_clause(cl);
        }
    }

    /// Encode one combinational cell; returns the output literal.
    #[allow(clippy::needless_range_loop)]
    pub fn enc_cell(&mut self, ci: usize) -> Result<i32, String> {
        if let Some(&l) = self.cell_lit.get(&ci) {
            return Ok(l);
        }
        let c = self.nl.cell(ci);
        let ins: Vec<i32> = c.inputs.iter().map(|n| self.lit_of(*n)).collect();
        let out = self.lit_of(c.output);
        match &c.kind {
            CellKind::Const(v) => {
                let b = v.as_bool().ok_or("const with x/z in formal check")?;
                self.cnf.add_clause(vec![if b { out } else { -out }]);
            }
            CellKind::Buf => self.enc_equal(ins[0], out),
            CellKind::Not => self.enc_not(ins[0], out),
            CellKind::And
            | CellKind::Nand
            | CellKind::Or
            | CellKind::Nor
            | CellKind::Xor
            | CellKind::Xnor => {
                let negated = matches!(c.kind, CellKind::Nand | CellKind::Nor | CellKind::Xnor);
                let mut acc = ins[0];
                for k in 1..ins.len() {
                    let t = self.fresh();
                    match c.kind {
                        CellKind::And | CellKind::Nand => self.enc_and(acc, ins[k], t),
                        CellKind::Or | CellKind::Nor => self.enc_or(acc, ins[k], t),
                        _ => self.enc_xor(acc, ins[k], t),
                    }
                    acc = t;
                }
                if negated {
                    let t = self.fresh();
                    self.enc_not(acc, t);
                    self.enc_equal(t, out);
                } else {
                    self.enc_equal(acc, out);
                }
            }
            CellKind::Mux => {
                let (s, a, b) = (ins[0], ins[1], ins[2]);
                self.enc_truthtable(&[s, a, b], &|v| if v[0] { v[2] } else { v[1] }, out);
            }
            CellKind::Dff { .. } | CellKind::DffR { .. } => {
                // sequential boundary: treated as free state vars by callers
                return Err("sequential cell in combinational cone".into());
            }
            CellKind::BlockRom {
                address_width,
                bit_index,
                contents,
            } => {
                if *address_width > 20 {
                    return Err(
                        "BlockRom is too large for the current truth-table SAT encoder".to_string(),
                    );
                }
                if contents.len() != (1_usize << *address_width) {
                    return Err(
                        "BlockRom with a partial address space cannot be encoded in two-valued SAT"
                            .to_string(),
                    );
                }
                let contents = contents.clone();
                self.enc_truthtable(
                    &ins,
                    &|address| {
                        let mut index = 0_usize;
                        for (bit, set) in address.iter().enumerate() {
                            if *set {
                                index |= 1 << bit;
                            }
                        }
                        contents[index].bit(*bit_index).as_bool().unwrap_or(false)
                    },
                    out,
                );
            }
            CellKind::Lib(lid) => {
                let lib = self
                    .nl
                    .library()
                    .ok_or("netlist has no library attached; attach with set_library")?;
                let def = lib.get(*lid);
                if def.seq.is_some() {
                    return Err("sequential library cell in combinational cone".into());
                }
                let f = def.func.clone();
                self.enc_truthtable(&ins, &|v| libfunc_bool(&f, v), out);
            }
        }
        self.cell_lit.insert(ci, out);
        Ok(out)
    }

    /// Encode the combinational cone feeding `sink_net`; primary inputs and
    /// flop outputs act as free variables. Returns the sink literal.
    pub fn enc_cone(&mut self, sink: NetId) -> Result<i32, String> {
        self.enc_cone_with_cuts(sink, |_| false)
    }

    /// Encode the combinational cone of `sink`, stopping descent at any net
    /// in `cuts` (those nets become free variables to be tied by the caller).
    /// This is the LEC correspondence-point mechanism: each query stays local.
    pub fn enc_cone_with_cuts<F: Fn(&NetId) -> bool>(
        &mut self,
        sink: NetId,
        cuts: F,
    ) -> Result<i32, String> {
        // Iterative depth-first topological encoding. Pushing a parent above
        // its not-yet-visited children made the former work stack regrow on
        // every visit for every multi-input cone. Explicit Enter/Leave frames
        // bound work to circuit size and turn cycles into an error instead of
        // unbounded allocation.
        enum Visit {
            Enter(NetId),
            Leave(NetId),
        }

        // 0 = unseen, 1 = visiting, 2 = encoded
        let mut state: HashMap<NetId, u8> = HashMap::new();
        let mut stack = vec![Visit::Enter(sink)];
        while let Some(visit) = stack.pop() {
            match visit {
                Visit::Enter(n) => match state.get(&n).copied().unwrap_or(0) {
                    2 => {}
                    1 => {
                        return Err(format!(
                            "combinational loop reaches net '{}' during formal encoding",
                            self.nl.net(n).name
                        ));
                    }
                    _ => {
                        state.insert(n, 1);
                        let drivers = self.nl.net(n).drivers.clone();
                        let sequential_boundary = drivers.iter().any(|&d| {
                            matches!(
                                self.nl.cell(d).kind,
                                CellKind::Dff { .. } | CellKind::DffR { .. }
                            ) || self
                                .nl
                                .library()
                                .and_then(|lib| {
                                    if let CellKind::Lib(id) = self.nl.cell(d).kind {
                                        Some(lib.get(id).seq.is_some())
                                    } else {
                                        None
                                    }
                                })
                                .unwrap_or(false)
                        });
                        if drivers.is_empty() || sequential_boundary || cuts(&n) {
                            // Primary inputs and flip-flop Q pins are free
                            // variables. Do not traverse a sequential cell's D
                            // cone: that edge belongs to the next time frame.
                            self.lit_of(n);
                            state.insert(n, 2);
                            continue;
                        }
                        stack.push(Visit::Leave(n));
                        for &d in &drivers {
                            let cell = self.nl.cell(d);
                            for &input in &cell.inputs {
                                match state.get(&input).copied().unwrap_or(0) {
                                    1 => {
                                        return Err(format!(
                                            "combinational loop through cell '{}' ('{}' -> '{}')",
                                            cell.name,
                                            self.nl.net(input).name,
                                            self.nl.net(n).name
                                        ));
                                    }
                                    0 => stack.push(Visit::Enter(input)),
                                    _ => {}
                                }
                            }
                        }
                    }
                },
                Visit::Leave(n) => {
                    if state.get(&n).copied().unwrap_or(0) == 2 {
                        continue;
                    }
                    let drivers = self.nl.net(n).drivers.clone();
                    for &d in &drivers {
                        // `enc_cell` memoizes shared subgraphs.
                        self.enc_cell(d)?;
                    }
                    if drivers.len() > 1 {
                        // Formal validation rejects multiple drivers. Preserve
                        // the legacy first-driver behavior for old callers.
                    }
                    state.insert(n, 2);
                }
            }
        }
        Ok(self.lit_of(sink))
    }
}

/// Boolean evaluation of a library function.
pub fn libfunc_bool(f: &LibFunc, ins: &[bool]) -> bool {
    let lv: Vec<eda_core::logic::LogicValue> = ins
        .iter()
        .map(|b| eda_core::logic::LogicValue::bit(*b))
        .collect();
    f.eval(&lv).as_bool().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sat::{solve, SatResult};
    use eda_core::netlist::{Cell, Dir, Netlist};

    #[test]
    fn encodes_and_gate() {
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
        let mut e = NetlistEncoder::new(&nl);
        let yl = e.enc_cone(y).unwrap();
        // force y=1: satisfiable only if a=b=1
        let mut c2 = e.cnf.clone();
        c2.add_clause(vec![yl]);
        match solve(&c2) {
            SatResult::Sat(m) => {
                let av = *e.var.get(&a).unwrap() as usize - 1;
                let bv = *e.var.get(&b).unwrap() as usize - 1;
                assert!(m[av] && m[bv]);
            }
            _ => panic!("expected sat"),
        }
    }

    #[test]
    fn encodes_mux_truth_table() {
        let mut nl = Netlist::new("m");
        let s = nl.add_net("s");
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Mux,
            inputs: vec![s, a, b],
            output: y,
        });
        let mut e = NetlistEncoder::new(&nl);
        let yl = e.enc_cone(y).unwrap();
        // for all models: y == (s ? b : a) — enumerate s,a,b
        for m in 0..8u32 {
            let sv = *e.var.get(&s).unwrap();
            let av = *e.var.get(&a).unwrap();
            let bv = *e.var.get(&b).unwrap();
            let mut c2 = e.cnf.clone();
            c2.add_clause(vec![if m & 1 != 0 { sv } else { -sv }]);
            c2.add_clause(vec![if m & 2 != 0 { av } else { -av }]);
            c2.add_clause(vec![if m & 4 != 0 { bv } else { -bv }]);
            let expect = if m & 1 != 0 { m & 4 != 0 } else { m & 2 != 0 };
            match solve(&c2) {
                SatResult::Sat(assign) => {
                    let yv = assign[(yl.unsigned_abs() - 1) as usize];
                    assert_eq!(yv, expect, "m={}", m);
                }
                _ => panic!("expected sat for m={}", m),
            }
        }
    }

    #[test]
    fn lib_cell_encoding() {
        use eda_core::library::CellLibrary;
        use std::sync::Arc;
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("m");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("XNOR2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let mut e = NetlistEncoder::new(&nl);
        let yl = e.enc_cone(y).unwrap();
        // y == XNOR(a,b): check a=1,b=0 forces y=0
        let av = e.lit_of(a);
        let bv = e.lit_of(b);
        let mut c2 = e.cnf.clone();
        c2.add_clause(vec![av]);
        c2.add_clause(vec![-bv]);
        c2.add_clause(vec![yl]); // force y=1 → should be UNSAT
        assert_eq!(solve(&c2), SatResult::Unsat);
    }

    #[test]
    fn wide_cone_encoding_is_bounded_and_linear() {
        // This shape repeatedly triggered the old parent-on-top-of-children
        // work-stack bug and allocated until the process was killed.
        const CHAIN_LEN: usize = 12_000;
        let mut nl = Netlist::new("wide");
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let mut prev = nl.add_net("n0");
        nl.add_cell(Cell {
            name: "u0".into(),
            kind: CellKind::And,
            inputs: vec![a, b],
            output: prev,
        });
        for i in 1..CHAIN_LEN {
            let n = nl.add_net(format!("n{i}"));
            nl.add_cell(Cell {
                name: format!("u{i}"),
                kind: CellKind::And,
                inputs: vec![prev, b],
                output: n,
            });
            prev = n;
        }
        assert_eq!(nl.net_count(), CHAIN_LEN + 2);
        assert_eq!(nl.cell_count(), CHAIN_LEN);
        let mut e = NetlistEncoder::new(&nl);
        let out = e.enc_cone(prev).expect("bounded cone must encode");
        assert!(out > 0);
        // Every visited cell is memoized and contributes O(1) clauses. The
        // exact count depends on structural simplification, but must remain
        // linear rather than growing with encoder iterations.
        assert!(e.cnf.clauses.len() <= 5 * CHAIN_LEN);
        assert!(e.cnf.n_vars <= 2 * (CHAIN_LEN + 2));
        assert_eq!(e.var.len(), CHAIN_LEN + 2);
    }

    #[test]
    fn combinational_cone_cycle_is_rejected_without_growth() {
        let mut nl = Netlist::new("cycle");
        let a = nl.add_net("a");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u_loop".into(),
            kind: CellKind::And,
            inputs: vec![a, y],
            output: y,
        });
        let mut e = NetlistEncoder::new(&nl);
        let err = e.enc_cone(y).unwrap_err();
        assert!(err.contains("combinational loop"), "{}", err);
    }
}
