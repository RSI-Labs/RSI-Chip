//! Structural function-class matching and candidate correspondence probes.

use super::*;

/// Normalized functional class used by the structural fast path. Library
/// cells collapse to the same classes as generic primitives so that 1:1
/// technology mapping (AND2 for And, MUX2 for Mux, ...) is recognized as
/// isomorphic without SAT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FuncClass {
    Const(bool),
    Buf,
    Not,
    And,
    Nand,
    Or,
    Nor,
    Xor,
    Xnor,
    Mux,
}

pub(super) fn cell_class(nl: &Netlist, kind: &CellKind) -> Option<FuncClass> {
    match kind {
        CellKind::Const(v) => v.as_bool().map(FuncClass::Const),
        CellKind::Buf => Some(FuncClass::Buf),
        CellKind::Not => Some(FuncClass::Not),
        CellKind::And => Some(FuncClass::And),
        CellKind::Nand => Some(FuncClass::Nand),
        CellKind::Or => Some(FuncClass::Or),
        CellKind::Nor => Some(FuncClass::Nor),
        CellKind::Xor => Some(FuncClass::Xor),
        CellKind::Xnor => Some(FuncClass::Xnor),
        CellKind::Mux => Some(FuncClass::Mux),
        CellKind::Dff { .. } | CellKind::DffR { .. } => None,
        CellKind::BlockRom { .. }
        | CellKind::BlockRamWrite { .. }
        | CellKind::BlockRamRead { .. } => None,
        CellKind::Lib(id) => {
            let lib = nl.library()?;
            let def = lib.get(*id);
            if def.seq.is_some() {
                return None;
            }
            library_cell_class(&def.func, def.inputs.len())
        }
    }
}

pub(super) fn direct_pin(expr: &LibFunc, pin: usize) -> bool {
    matches!(expr, LibFunc::Id(i) if *i == pin)
}

pub(super) fn direct_pins(exprs: &[LibFunc], input_count: usize) -> bool {
    if exprs.len() != input_count {
        return false;
    }
    let mut seen = vec![false; input_count];
    for expr in exprs {
        let LibFunc::Id(pin) = expr else {
            return false;
        };
        if *pin >= input_count || seen[*pin] {
            return false;
        }
        seen[*pin] = true;
    }
    true
}

/// Class only a library cell whose function is expressed directly over its
/// physical pins (for example `AND2(a,b)`). A complex Liberty expression
/// such as `AND(NOT(a), b)` has the same top-level class as AND2 but is not
/// structurally isomorphic to a primitive AND; sending it to SAT is sound.
pub(super) fn library_cell_class(func: &LibFunc, input_count: usize) -> Option<FuncClass> {
    match func {
        LibFunc::Const(v) => v.as_bool().map(FuncClass::Const),
        LibFunc::Id(pin) if *pin == 0 && input_count == 1 => Some(FuncClass::Buf),
        LibFunc::Not(expr) if input_count == 1 && direct_pin(expr, 0) => Some(FuncClass::Not),
        LibFunc::And(exprs) if direct_pins(exprs, input_count) => Some(FuncClass::And),
        LibFunc::Nand(exprs) if direct_pins(exprs, input_count) => Some(FuncClass::Nand),
        LibFunc::Or(exprs) if direct_pins(exprs, input_count) => Some(FuncClass::Or),
        LibFunc::Nor(exprs) if direct_pins(exprs, input_count) => Some(FuncClass::Nor),
        LibFunc::Xor(exprs) if direct_pins(exprs, input_count) => Some(FuncClass::Xor),
        LibFunc::Xnor(exprs) if direct_pins(exprs, input_count) => Some(FuncClass::Xnor),
        LibFunc::Mux { sel, a, b }
            if input_count == 3 && direct_pin(sel, 0) && direct_pin(a, 1) && direct_pin(b, 2) =>
        {
            Some(FuncClass::Mux)
        }
        _ => None,
    }
}

/// Structural-isomorphism fast path for a candidate correspondence pair.
/// Unlike the original one-root-only matcher, this returns every internal
/// gold/gate net pair proved in lockstep. Those pairs can all become cut
/// points without another SAT query.
struct StructuralSearch<'a> {
    gold: &'a Netlist,
    gate: &'a Netlist,
    accepted: &'a HashMap<NetId, NetId>,
    path: std::collections::HashSet<(NetId, NetId)>,
    partial_successes: HashMap<(NetId, NetId), Vec<(NetId, NetId)>>,
    budget: usize,
}

pub(super) fn commutative(class: FuncClass) -> bool {
    matches!(
        class,
        FuncClass::And
            | FuncClass::Nand
            | FuncClass::Or
            | FuncClass::Nor
            | FuncClass::Xor
            | FuncClass::Xnor
    )
}

pub(super) fn merge_pairs(
    mut base: Vec<(NetId, NetId)>,
    extra: Vec<(NetId, NetId)>,
) -> Option<Vec<(NetId, NetId)>> {
    let mut partners: HashMap<NetId, NetId> = base.iter().copied().collect();
    let mut reverse: HashMap<NetId, NetId> = base.iter().map(|(g, t)| (*t, *g)).collect();
    for pair @ (g, t) in extra {
        if partners.get(&g) == Some(&t) && reverse.get(&t) == Some(&g) {
            continue;
        }
        if partners.contains_key(&g) || reverse.contains_key(&t) {
            // A node must never receive two different structural partners.
            return None;
        }
        partners.insert(g, t);
        reverse.insert(t, g);
        if !base.contains(&pair) {
            base.push(pair);
        }
    }
    Some(base)
}

impl StructuralSearch<'_> {
    fn pair(&mut self, g: NetId, t: NetId) -> Option<Vec<(NetId, NetId)>> {
        if self.budget == 0 {
            return None;
        }
        self.budget -= 1;
        if !self.path.insert((g, t)) {
            return None;
        }
        let result = self.pair_inner(g, t);
        if let Some(pairs) = result.as_ref() {
            self.partial_successes.insert((g, t), pairs.clone());
        }
        self.path.remove(&(g, t));
        result
    }

    fn pair_inner(&mut self, g: NetId, t: NetId) -> Option<Vec<(NetId, NetId)>> {
        if let Some(&mapped) = self.accepted.get(&g) {
            return (mapped == t).then_some(Vec::new());
        }

        let gd = self.gold.net(g).drivers.clone();
        let td = self.gate.net(t).drivers.clone();
        if gd.len() != 1 || td.len() != 1 {
            return None;
        }
        let gcell = self.gold.cell(gd[0]);
        let tcell = self.gate.cell(td[0]);
        let gclass = cell_class(self.gold, &gcell.kind)?;
        let tclass = cell_class(self.gate, &tcell.kind)?;
        if gclass != tclass || gcell.inputs.len() != tcell.inputs.len() {
            return None;
        }

        let mut pairs = vec![(g, t)];
        if commutative(gclass) {
            let mut used = vec![false; tcell.inputs.len()];
            let extra = self.match_commutative(&gcell.inputs, &tcell.inputs, 0, &mut used)?;
            pairs = merge_pairs(pairs, extra)?;
        } else {
            for (&gin, &tin) in gcell.inputs.iter().zip(&tcell.inputs) {
                let extra = self.pair(gin, tin)?;
                pairs = merge_pairs(pairs, extra)?;
            }
        }
        Some(pairs)
    }

    fn match_commutative(
        &mut self,
        gold_inputs: &[NetId],
        gate_inputs: &[NetId],
        position: usize,
        used: &mut [bool],
    ) -> Option<Vec<(NetId, NetId)>> {
        let Some(&gin) = gold_inputs.get(position) else {
            return Some(Vec::new());
        };
        for (target_index, &tin) in gate_inputs.iter().enumerate() {
            if used[target_index] {
                continue;
            }
            let Some(first) = self.pair(gin, tin) else {
                continue;
            };
            used[target_index] = true;
            let Some(rest) = self.match_commutative(gold_inputs, gate_inputs, position + 1, used)
            else {
                used[target_index] = false;
                continue;
            };
            used[target_index] = false;
            return merge_pairs(first, rest);
        }
        None
    }
}

pub(super) struct StructuralProbe {
    pub(super) root: Option<Vec<(NetId, NetId)>>,
    /// Subgraphs that were independently proved even if the requested roots
    /// could not be connected structurally. They are candidates, never
    /// assumptions; induction still proves each root before acceptance.
    pub(super) partial: Vec<Vec<(NetId, NetId)>>,
}

/// Return all structurally proved pairs, including the requested roots.
#[cfg(test)]
pub(super) fn structural_matches(
    gold: &Netlist,
    gate: &Netlist,
    gn: NetId,
    tn: NetId,
    accepted: &HashMap<NetId, NetId>,
) -> Option<Vec<(NetId, NetId)>> {
    structural_probe(gold, gate, gn, tn, accepted).root
}

pub(super) fn structural_probe(
    gold: &Netlist,
    gate: &Netlist,
    gn: NetId,
    tn: NetId,
    accepted: &HashMap<NetId, NetId>,
) -> StructuralProbe {
    let mut search = StructuralSearch {
        gold,
        gate,
        accepted,
        path: Default::default(),
        partial_successes: HashMap::new(),
        budget: 50_000,
    };
    let root = search.pair(gn, tn);
    let partial = search.partial_successes.into_values().collect();
    StructuralProbe { root, partial }
}

#[cfg(test)]
pub(super) fn region_isomorphic(
    gold: &Netlist,
    gate: &Netlist,
    gn: NetId,
    tn: NetId,
    accepted: &HashMap<NetId, NetId>,
) -> bool {
    structural_matches(gold, gate, gn, tn, accepted).is_some()
}

pub(super) fn distinguished_result(
    what: String,
    model: HashMap<NetId, bool>,
    gold: &Netlist,
) -> EquivResult {
    let mut cex = HashMap::new();
    for p in &gold.ports {
        if p.dir != Dir::In {
            continue;
        }
        let bits = p
            .bits
            .iter()
            .map(|b| {
                model
                    .get(b)
                    .map(|v| eda_core::logic::LogicValue::bit(*v))
                    .unwrap_or(eda_core::logic::LogicValue::Zero)
            })
            .collect();
        cex.insert(p.name.clone(), LogicWord { bits });
    }
    EquivResult {
        equivalent: false,
        counterexample: Some(cex),
        notes: vec![format!("counterexample distinguishes {what}")],
    }
}

pub(super) fn shift(l: i32, off: i32) -> i32 {
    if l > 0 {
        l + off
    } else {
        l - off
    }
}
