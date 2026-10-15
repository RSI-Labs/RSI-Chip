//! Combinational & sequential equivalence checking via miter construction
//! over a shared SAT instance.

use crate::cnf::NetlistEncoder;
use crate::sat::{solve_local_cancellable, IncrementalSolver, SatResult};
use eda_core::cancellation::FlowCancellation;
use eda_core::library::LibFunc;
use eda_core::logic::LogicWord;
use eda_core::netlist::{CellKind, Dir, NetId, Netlist};
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};

mod structural;
mod verification;

use structural::*;
use verification::*;

/// Net IDs are dense `usize`s in LEC-sized sets. Identity hashing preserves
/// set semantics while avoiding SipHash's per-candidate cost in queues that
/// rebuild cut frontiers millions of times on RV32I.
#[derive(Default)]
struct NetIdHasher(u64);

impl Hasher for NetIdHasher {
    fn write(&mut self, bytes: &[u8]) {
        for (index, byte) in bytes.iter().enumerate() {
            self.0 ^= (*byte as u64) << ((index % 8) * 8);
        }
    }

    fn write_usize(&mut self, value: usize) {
        self.0 = value as u64;
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[derive(Clone, Default)]
struct NetIdHashBuilder;

impl BuildHasher for NetIdHashBuilder {
    type Hasher = NetIdHasher;

    fn build_hasher(&self) -> Self::Hasher {
        NetIdHasher::default()
    }
}

type NetIdSet = std::collections::HashSet<NetId, NetIdHashBuilder>;

#[derive(Clone, Debug)]
pub struct EquivResult {
    pub equivalent: bool,
    /// When not equivalent: an input assignment (per port name) that
    /// distinguishes the two netlists.
    pub counterexample: Option<HashMap<String, LogicWord>>,
    pub notes: Vec<String>,
}

fn flop_records(nl: &Netlist) -> Result<Vec<FlopRec>, String> {
    let mut out = Vec::new();
    for c in nl.cells() {
        let (q, d, clk) = match &c.kind {
            CellKind::Dff { .. } | CellKind::DffR { .. } => (c.output, c.inputs[1], c.inputs[0]),
            CellKind::Lib(lid) => {
                let lib = nl.library().ok_or("library cell without library")?;
                let def = lib.get(*lid);
                match &def.seq {
                    Some(seq) => (c.output, c.inputs[1], c.inputs[seq.clk_pin]),
                    None => continue,
                }
            }
            _ => continue,
        };
        out.push(FlopRec {
            q_name: nl.net(q).name.clone(),
            q,
            d,
            clk,
        });
    }
    Ok(out)
}

struct FlopRec {
    q_name: String,
    q: NetId,
    d: NetId,
    #[allow(dead_code)]
    clk: NetId,
}

/// DFS (pre, post) intervals for every net in `sink`'s combinational cone.
/// Interval nesting encodes fan-in dependency without repeated cone walks.
fn cone_intervals(nl: &Netlist, sink: NetId) -> HashMap<NetId, (u32, u32)> {
    let mut out: HashMap<NetId, (u32, u32)> = HashMap::new();
    let mut clock = 0u32;
    enum Frame {
        Enter(NetId),
        Leave(NetId),
    }
    let mut stack = vec![Frame::Enter(sink)];
    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Enter(n) => {
                if out.contains_key(&n) {
                    continue;
                }
                let pre = clock;
                clock += 1;
                out.insert(n, (pre, u32::MAX));
                let drivers = nl.net(n).drivers.clone();
                let sequential = drivers.iter().any(|&d| {
                    let cell = nl.cell(d);
                    matches!(cell.kind, CellKind::Dff { .. } | CellKind::DffR { .. })
                        || nl.library().is_some_and(|lib| {
                            matches!(cell.kind, CellKind::Lib(id) if lib.get(id).seq.is_some())
                        })
                });
                stack.push(Frame::Leave(n));
                if !sequential {
                    for &d in &drivers {
                        for &input in &nl.cell(d).inputs {
                            stack.push(Frame::Enter(input));
                        }
                    }
                }
            }
            Frame::Leave(n) => {
                if let Some(entry) = out.get_mut(&n) {
                    entry.1 = clock;
                    clock += 1;
                }
            }
        }
    }
    out
}

/// Check functional equivalence of two netlists (ports matched by name).
/// Flip-flops are matched by their output net name; their next-state
/// functions and all outputs are compared symbolically over all inputs.
pub fn check_equivalent(gold: &Netlist, gate: &Netlist) -> Result<EquivResult, String> {
    check_equivalent_cancellable(gold, gate, &FlowCancellation::new())
}

/// Cooperatively cancel netlist equivalence checking.
pub fn check_equivalent_cancellable(
    gold: &Netlist,
    gate: &Netlist,
    cancellation: &FlowCancellation,
) -> Result<EquivResult, String> {
    if cancellation.is_cancelled() {
        return Err("equivalence checking cancelled".into());
    }
    let mut notes = Vec::new();
    // port sanity
    let same_ports = |a: &Netlist, b: &Netlist| -> bool {
        a.ports.len() == b.ports.len()
            && a.ports.iter().all(|p| {
                b.ports
                    .iter()
                    .any(|q| q.name == p.name && q.dir == p.dir && q.bits.len() == p.bits.len())
            })
    };
    if !same_ports(gold, gate) {
        return Ok(EquivResult {
            equivalent: false,
            counterexample: None,
            notes: vec!["port lists differ".into()],
        });
    }

    // match flops by q net name, tie present state, and miter next state
    let gf = flop_records(gold)?;
    let tf = flop_records(gate)?;
    if gf.len() != tf.len() {
        notes.push(format!("flop count differs: {} vs {}", gf.len(), tf.len()));
        return Ok(EquivResult {
            equivalent: false,
            counterexample: None,
            notes,
        });
    }
    let mut tf_by_name: HashMap<&str, &FlopRec> =
        tf.iter().map(|f| (f.q_name.as_str(), f)).collect();
    let mut matched: Vec<(&FlopRec, &FlopRec)> = Vec::new();
    for g in &gf {
        if cancellation.is_cancelled() {
            return Err("equivalence checking cancelled".into());
        }
        match tf_by_name.remove(g.q_name.as_str()) {
            Some(t) => matched.push((g, t)),
            None => {
                notes.push(format!("no matching flop for '{}'", g.q_name));
                return Ok(EquivResult {
                    equivalent: false,
                    counterexample: None,
                    notes,
                });
            }
        }
    }

    // Name-based internal correspondence points. Both synthesis views keep
    // net names, so every same-named cell output becomes a cut-point
    // candidate: it is tied in all other queries and verified by its own
    // localized query. Synthesis temporaries are not stable identifiers
    // across the generic and mapped pipelines, so only RTL-derived names are
    // proposed here.
    let mut gate_net_by_name: HashMap<&str, NetId> = HashMap::new();
    for n in 0..gate.net_count() {
        gate_net_by_name.insert(gate.net(n).name.as_str(), n);
    }
    let mut points: Vec<(NetId, NetId, bool)> = Vec::new();
    for n in 0..gold.net_count() {
        let name = gold.net(n).name.as_str();
        let temp = name.starts_with("mux_n")
            || name.starts_with("g_n")
            || name.starts_with("c_n")
            || name.starts_with("not_n")
            || name.starts_with("and_n")
            || name.starts_with("or_n")
            || name.starts_with("xor_n")
            || name.starts_with("q_n")
            || name
                .split('.')
                .next()
                .is_some_and(|base| base.parse::<f64>().is_ok());
        if temp {
            continue;
        }
        if let Some(&t) = gate_net_by_name.get(name) {
            points.push((n, t, false));
        }
    }
    let (gold_values, gate_values) = simulate_shared_values(gold, gate, &gf, &tf, cancellation)?;
    let named: Vec<(NetId, NetId)> = points.iter().map(|(g, t, _)| (*g, *t)).collect();
    let (verified_points, verified_named) = build_correspondence_points(
        gold,
        gate,
        &gf,
        &tf,
        &gold_values,
        &gate_values,
        named,
        cancellation,
    )?;
    notes.push(format!(
        "verified {verified_named}/{} named correspondence points; accepted {} total",
        points.len(),
        verified_points.len()
    ));
    points = verified_points;
    // Prove one localized correspondence-point miter per flop next-state
    // function, per output bit, and per internal cut point. Each query
    // encodes only the fan-in cones of that single bit, which keeps every
    // SAT instance small even for designs with thousands of state bits.
    let mut queries = 0usize;
    let prove = |gold: &Netlist,
                 gate: &Netlist,
                 g_net: NetId,
                 t_net: NetId,
                 what: String,
                 queries: &mut usize|
     -> Result<Option<EquivResult>, String> {
        let verdict = match prove_bit_equal(
            gold,
            gate,
            g_net,
            t_net,
            &matched,
            &points,
            None,
            cancellation,
            true,
        ) {
            Ok(verdict) => verdict,
            // Capacity-limited local proof: fail closed with a structured
            // note instead of aborting the whole flow.
            Err(error) if error.contains("stuck") => {
                return Ok(Some(EquivResult {
                    equivalent: false,
                    counterexample: None,
                    notes: vec![format!(
                        "{what}: local miter exceeded solver capacity ({error}); {} correspondence points",
                        points.len()
                    )],
                }));
            }
            Err(error) => return Err(error),
        };
        let verdict = match verdict {
            MiterVerdict::Distinguished { phases, .. } => {
                match prove_bit_equal(
                    gold,
                    gate,
                    g_net,
                    t_net,
                    &matched,
                    &points,
                    Some(&phases),
                    cancellation,
                    false,
                ) {
                    Ok(retry) => retry,
                    // A capacity-limited re-check must fail closed: the
                    // correspondence point stays unproven, so equivalence
                    // cannot be claimed for the design.
                    Err(error) => {
                        return Ok(Some(EquivResult {
                            equivalent: false,
                            counterexample: None,
                            notes: vec![format!(
                                "{what}: re-check without internal cuts exceeded solver capacity ({error}); {} correspondence points",
                                points.len()
                            )],
                        }));
                    }
                }
            }
            other => other,
        };
        *queries += 1;
        if let MiterVerdict::Distinguished { gold: model, .. } = verdict {
            return Ok(Some(distinguished_result(what, model, gold)));
        }
        Ok(None)
    };

    // Visit structurally deep next-state cones first. This is a fail-fast
    // scheduling policy, not a change to the proof itself: topological driver
    // depth is computed once per design and avoids re-walking every flop cone
    // merely to order the obligations.
    let topo_cell_rank = |nl: &Netlist| -> HashMap<usize, usize> {
        nl.topo_cells()
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(rank, cell_id)| (cell_id, rank))
            .collect()
    };
    let gold_cell_rank = topo_cell_rank(gold);
    let gate_cell_rank = topo_cell_rank(gate);
    let driver_rank = |nl: &Netlist, net: NetId, ranks: &HashMap<usize, usize>| -> usize {
        nl.net(net)
            .drivers
            .first()
            .and_then(|cell_id| ranks.get(cell_id))
            .copied()
            .unwrap_or_default()
    };
    let mut proof_order: Vec<(usize, &str, &FlopRec, &FlopRec)> = matched
        .iter()
        .map(|(g, t)| {
            let rank = driver_rank(gold, g.d, &gold_cell_rank).saturating_add(driver_rank(
                gate,
                t.d,
                &gate_cell_rank,
            ));
            (rank, g.q_name.as_str(), *g, *t)
        })
        .collect();
    proof_order.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    if std::env::var("EDA_LEC_DEBUG").is_ok() {
        let leading: Vec<&str> = proof_order
            .iter()
            .take(12)
            .map(|(_, name, _, _)| *name)
            .collect();
        eprintln!("LEC hardest flop obligations: {leading:?}");
    }
    for (_, _, g, t) in proof_order {
        if let Some(result) = prove(
            gold,
            gate,
            g.d,
            t.d,
            format!("next-state function of '{}'", g.q_name),
            &mut queries,
        )? {
            return Ok(result);
        }
    }
    for p in &gold.ports {
        if p.dir != Dir::Out {
            continue;
        }
        let gp = gate.port_by_name(&p.name).unwrap();
        for k in 0..p.bits.len() {
            if let Some(result) = prove(
                gold,
                gate,
                p.bits[k],
                gp.bits[k],
                format!("output '{}[{}]'", p.name, k),
                &mut queries,
            )? {
                return Ok(result);
            }
        }
    }
    // Internal correspondence points were already discharged while the
    // inductive frontier was built. Replaying those proofs adds no new design
    // obligation: the final flop/output queries still consume the verified
    // points as sound cuts.
    notes.push(format!(
        "proved with {queries} localized correspondence-point miter queries"
    ));
    Ok(EquivResult {
        equivalent: true,
        counterexample: None,
        notes,
    })
}

enum MiterVerdict {
    Equivalent,
    Distinguished {
        gold: HashMap<NetId, bool>,
        phases: HashMap<usize, bool>,
    },
}

fn is_sequential_cell(kind: &CellKind, nl: &Netlist) -> bool {
    matches!(kind, CellKind::Dff { .. } | CellKind::DffR { .. })
        || nl
            .library()
            .is_some_and(|lib| matches!(kind, CellKind::Lib(id) if lib.get(*id).seq.is_some()))
}

fn is_full_cone_boundary(nl: &Netlist, net: NetId) -> bool {
    nl.net(net)
        .drivers
        .iter()
        .any(|&cell_id| is_sequential_cell(&nl.cell(cell_id).kind, nl))
        || nl.net(net).drivers.is_empty()
}

fn full_cone_values(
    nl: &Netlist,
    vars: &HashMap<NetId, i32>,
    model: &[bool],
    shared: &HashMap<String, bool>,
) -> HashMap<NetId, bool> {
    let mut values = HashMap::new();
    let mut logic_values = HashMap::new();
    for (net, lit) in vars {
        if is_full_cone_boundary(nl, *net) {
            let value = model[(*lit - 1) as usize];
            values.insert(*net, value);
            logic_values.insert(*net, eda_core::logic::LogicValue::bit(value));
        }
    }
    for (name, value) in shared {
        if let Some(net) = nl.net_by_name(name) {
            values.insert(net, *value);
            logic_values.insert(net, eda_core::logic::LogicValue::bit(*value));
        }
    }
    for cell_id in nl.topo_cells().unwrap_or_default() {
        let cell = nl.cell(cell_id);
        if is_sequential_cell(&cell.kind, nl) {
            continue;
        }
        let value = nl.eval_cell(cell_id, &logic_values);
        if let Some(bit) = value.as_bool() {
            values.insert(cell.output, bit);
            logic_values.insert(cell.output, value);
        }
    }
    values
}

/// Append encoder clauses created after a miter was combined and return the
/// combined literal assigned to each newly created net.
fn append_encoder_delta(
    cnf: &mut crate::sat::Cnf,
    encoder: &NetlistEncoder,
    base: (usize, usize),
    map_old: &dyn Fn(i32) -> i32,
) -> HashMap<NetId, i32> {
    let new_vars = encoder.cnf.n_vars - base.0;
    let first_new = cnf.n_vars + 1;
    for _ in 0..new_vars {
        cnf.add_var();
    }
    let map_lit = |lit: i32| {
        let var = lit.unsigned_abs() as usize;
        if var <= base.0 {
            map_old(lit)
        } else if lit > 0 {
            (first_new + var - base.0 - 1) as i32
        } else {
            -((first_new + var - base.0 - 1) as i32)
        }
    };
    for clause in &encoder.cnf.clauses[base.1..] {
        let shifted = clause.iter().map(|lit| map_lit(*lit)).collect();
        cnf.add_clause(shifted);
    }
    encoder
        .var
        .iter()
        .filter(|(_, lit)| lit.unsigned_abs() as usize > base.0)
        .map(|(net, lit)| (*net, map_lit(*lit)))
        .collect()
}

fn combined_old_literal_map(
    encoder: &NetlistEncoder,
    combined: &HashMap<NetId, i32>,
) -> HashMap<i32, i32> {
    encoder
        .var
        .iter()
        .filter_map(|(net, local)| combined.get(net).map(|lit| (*local, *lit)))
        .collect()
}

fn sync_incremental_solver(
    solver: &mut IncrementalSolver,
    cnf: &crate::sat::Cnf,
    seen_clauses: &mut usize,
) {
    solver.ensure_variable_count(cnf.n_vars);
    solver.add_clauses(&cnf.clauses[*seen_clauses..]);
    *seen_clauses = cnf.clauses.len();
}

/// Build a two-net XOR miter restricted to the fan-in cones of `g_net`
/// (gold) and `t_net` (gate), tie shared primary inputs and matched state
/// bits, and solve.
#[allow(clippy::too_many_arguments)]
fn prove_bit_equal(
    gold: &Netlist,
    gate: &Netlist,
    g_net: NetId,
    t_net: NetId,
    matched: &[(&FlopRec, &FlopRec)],
    points: &[(NetId, NetId, bool)],
    phase_hints: Option<&HashMap<usize, bool>>,
    cancellation: &FlowCancellation,
    use_internal_cuts: bool,
) -> Result<MiterVerdict, String> {
    // Cut the cones at an antichain of correspondence points. One DFS per
    // side records (pre, post) intervals; `b` is in the fan-in cone of `a`
    // iff interval(b) is nested inside interval(a). Cutting only mutually
    // independent points keeps each local proof quantified over genuinely
    // reachable value combinations (no false positives from dependent cuts).
    let gold_iv = cone_intervals(gold, g_net);
    let gate_iv = cone_intervals(gate, t_net);
    // A flop output is a valid boundary only when the matched pair is present
    // in both query cones. Cutting the gold side alone while optimization
    // removed the corresponding gate-side occurrence leaves a free gold
    // variable that can fabricate a distinction even though the full cones
    // are equivalent.
    let mut gold_cuts: NetIdSet = matched
        .iter()
        .filter(|(g, t)| {
            g.q != g_net && t.q != t_net && gold_iv.contains_key(&g.q) && gate_iv.contains_key(&t.q)
        })
        .map(|(g, _)| g.q)
        .collect();
    let mut gate_cuts: NetIdSet = matched
        .iter()
        .filter(|(g, t)| {
            g.q != g_net && t.q != t_net && gold_iv.contains_key(&g.q) && gate_iv.contains_key(&t.q)
        })
        .map(|(_, t)| t.q)
        .collect();
    if use_internal_cuts {
        // Pick a frontier as close to the query sink as possible. The
        // accepted-point vector is in design topological order, which tends
        // to place near-source points first; using that order greedily would
        // block their numerous descendants and leave most of the deep cone
        // encoded. DFS pre-order from the sink gives the opposite order, so a
        // nested descendant is considered before its ancestor and becomes the
        // cut. The same independence test below keeps the selected frontier
        // an antichain on both sides, preserving sound quantification.
        let mut eligible: Vec<(NetId, NetId, u32, u32)> = Vec::new();
        for (gn, tn, _) in points {
            if *gn == g_net || *tn == t_net {
                continue;
            }
            if let (Some(&(gpre, _)), Some(&(tpre, _))) = (gold_iv.get(gn), gate_iv.get(tn)) {
                eligible.push((*gn, *tn, gpre, tpre));
            }
        }
        eligible.sort_by_key(|(_, _, gpre, tpre)| (*gpre).max(*tpre));
        let mut chosen_gold: Vec<(NetId, u32, u32)> = Vec::new();
        let mut chosen_gate: Vec<(NetId, u32, u32)> = Vec::new();
        for (gn, tn, gpre, tpre) in eligible {
            let (gpost, tpost) = (gold_iv[&gn].1, gate_iv[&tn].1);
            let comparable = |list: &[(NetId, u32, u32)], pre: u32, post: u32| {
                list.iter().any(|&(_, cpre, cpost)| {
                    (cpre <= pre && post <= cpost) || (pre <= cpre && cpost <= post)
                })
            };
            if comparable(&chosen_gold, gpre, gpost) || comparable(&chosen_gate, tpre, tpost) {
                continue;
            }
            chosen_gold.push((gn, gpre, gpost));
            chosen_gate.push((tn, tpre, tpost));
            gold_cuts.insert(gn);
            gate_cuts.insert(tn);
        }
    }
    let mut ge = NetlistEncoder::new(gold);
    let a = ge.enc_cone_with_cuts(g_net, |net| gold_cuts.contains(net))?;
    let mut te = NetlistEncoder::new(gate);
    let b_raw = te.enc_cone_with_cuts(t_net, |net| gate_cuts.contains(net))?;
    if std::env::var("EDA_LEC_DEBUG").is_ok() {
        eprintln!(
            "LEC query: gold '{}' -> {}, gate '{}' -> {} (ge_vars={} te_vars={})",
            gold.net(g_net).name,
            a,
            gate.net(t_net).name,
            b_raw,
            ge.cnf.n_vars,
            te.cnf.n_vars
        );
    }
    if cancellation.is_cancelled() {
        return Err("equivalence checking cancelled".into());
    }

    let offset = ge.cnf.n_vars as i32;
    let b = shift(b_raw, offset);
    // Keep both source encoders live. A false cut model can be refined by
    // expanding only the mismatched cut variables back to their full cones;
    // appending just the newly generated encoder clauses preserves the shared
    // miter while avoiding a from-scratch rebuild.
    let mut ge_refinement_base = (ge.cnf.n_vars, ge.cnf.clauses.len());
    let mut te_refinement_base = (te.cnf.n_vars, te.cnf.clauses.len());
    let mut cnf = ge.cnf.clone();
    while cnf.n_vars < (offset + te.cnf.n_vars as i32) as usize {
        cnf.add_var();
    }
    for cl in &te.cnf.clauses {
        cnf.add_clause(cl.iter().map(|l| shift(*l, offset)).collect());
    }
    let mut gold_lits = ge.var.clone();
    let mut gate_lits: HashMap<NetId, i32> = te
        .var
        .iter()
        .map(|(net, lit)| (*net, shift(*lit, offset)))
        .collect();
    let eq = |cnf: &mut crate::sat::Cnf, x: i32, y: i32| {
        cnf.add_clause(vec![-x, y]);
        cnf.add_clause(vec![x, -y]);
    };

    // Tie primary inputs that appear in both cones.
    for p in &gold.ports {
        if p.dir != Dir::In {
            continue;
        }
        let gp = gate.port_by_name(&p.name).unwrap();
        for k in 0..p.bits.len() {
            if ge.has_lit(p.bits[k]) && te.has_lit(gp.bits[k]) {
                eq(&mut cnf, gold_lits[&p.bits[k]], gate_lits[&gp.bits[k]]);
            }
        }
    }
    // Tie matched present-state bits that appear in both cones.
    for (g, t) in matched {
        if ge.has_lit(g.q) && te.has_lit(t.q) {
            eq(&mut cnf, gold_lits[&g.q], gate_lits[&t.q]);
        }
    }
    // Tie same-named driverless internal nets present in both cones. Such
    // nets (e.g. a hardwired-zero register that no longer has a driver)
    // represent the same RTL signal in both views; leaving them untied lets
    // the miter assign different values to the same logical variable and
    // fabricates a distinction.
    {
        let mut gold_free: HashMap<&str, NetId> = HashMap::new();
        for n in ge.var.keys() {
            if gold.net(*n).drivers.is_empty() {
                gold_free.insert(gold.net(*n).name.as_str(), *n);
            }
        }
        let mut tied = std::collections::HashSet::new();
        for n in te.var.keys() {
            if gate.net(*n).drivers.is_empty() {
                let name = gate.net(*n).name.as_str();
                if let Some(&gn) = gold_free.get(name) {
                    eq(&mut cnf, gold_lits[&gn], gate_lits[n]);
                    tied.insert(name);
                }
            }
        }
        if std::env::var("EDA_LEC_SELFCHECK").is_ok() {
            let untied: Vec<&str> = gold_free
                .keys()
                .copied()
                .filter(|name| !tied.contains(name))
                .collect();
            if !untied.is_empty() {
                eprintln!(
                    "LEC SELFCHECK UNTIED FREE NETS ({}): {:?}",
                    untied.len(),
                    &untied[..untied.len().min(8)]
                );
            }
        }
    }
    // Tie internal correspondence points (never the query sinks themselves).
    for (gn, tn, _) in points {
        if *gn != g_net && *tn != t_net && ge.has_lit(*gn) && te.has_lit(*tn) {
            eq(&mut cnf, gold_lits[gn], gate_lits[tn]);
        }
    }

    // XOR miter asserted true.
    let x = cnf.add_var() as i32;
    cnf.add_clause(vec![-a, -b, -x]);
    cnf.add_clause(vec![-a, b, x]);
    cnf.add_clause(vec![a, -b, x]);
    cnf.add_clause(vec![a, b, -x]);
    cnf.add_clause(vec![x]);

    let mut solver = IncrementalSolver::new(&cnf);
    if let Some(phases) = phase_hints {
        solver.set_phase_hints(phases);
    }
    let mut solver_clause_count = cnf.clauses.len();
    let mut refinements = 0usize;
    let mut sources_materialized = false;
    let solved = loop {
        sync_incremental_solver(&mut solver, &cnf, &mut solver_clause_count);
        let solved_result = if refinements == 0 {
            solver.solve_cancellable(cancellation)
        } else {
            solver.solve_with_limit_cancellable(cancellation, 250_000)
        };
        let solved = match solved_result {
            Ok(result) => result,
            Err(error) if error.contains("stuck") => {
                // A refined query that exhausts its local budget is unknown,
                // not UNSAT. Propagate it as a stuck local proof so the
                // caller fails closed instead of claiming equivalence.
                if std::env::var("EDA_LEC_DEBUG").is_ok() {
                    eprintln!("LEC refinement capacity: {error}");
                }
                return Err(format!("LEC refinement stuck: {error}"));
            }
            Err(error) => return Err(error),
        };
        let SatResult::Sat(mut model) = solved else {
            break solved;
        };

        // A near-sink cut can leave no source literal in the abstract model.
        // Materialize the full-cone boundary variables only after that SAT
        // result, tie corresponding sources, and re-solve. Most provable local
        // queries therefore avoid paying for (and branching on) unrelated
        // source variables; only a candidate false counterexample does.
        if !sources_materialized {
            sources_materialized = true;
            let mut gold_sources: Vec<NetId> = gold_iv
                .keys()
                .copied()
                .filter(|net| is_full_cone_boundary(gold, *net))
                .collect();
            gold_sources.sort_unstable();
            for net in gold_sources {
                ge.lit_of(net);
            }
            let delta = append_encoder_delta(&mut cnf, &ge, ge_refinement_base, &|lit| lit);
            ge_refinement_base = (ge.cnf.n_vars, ge.cnf.clauses.len());
            for (net, lit) in delta {
                gold_lits.insert(net, lit);
            }

            let mut gate_sources: Vec<NetId> = gate_iv
                .keys()
                .copied()
                .filter(|net| is_full_cone_boundary(gate, *net))
                .collect();
            gate_sources.sort_unstable();
            for net in gate_sources {
                te.lit_of(net);
            }
            let delta =
                append_encoder_delta(&mut cnf, &te, te_refinement_base, &|lit| shift(lit, offset));
            te_refinement_base = (te.cnf.n_vars, te.cnf.clauses.len());
            for (net, lit) in delta {
                gate_lits.insert(net, lit);
            }
            if std::env::var("EDA_LEC_TRACE").is_ok() {
                let mut source_debug: Vec<_> = gold_lits
                    .iter()
                    .filter(|(net, _)| is_full_cone_boundary(gold, **net))
                    .map(|(net, lit)| (gold.net(*net).name.as_str(), *lit))
                    .collect();
                source_debug.sort_unstable();
                eprintln!("LEC gold source lits: {source_debug:?}");
            }

            let mut source_ties: Vec<(i32, i32)> = Vec::new();
            for p in &gold.ports {
                if p.dir != Dir::In {
                    continue;
                }
                let gp = gate.port_by_name(&p.name).unwrap();
                for (gn, tn) in p.bits.iter().zip(&gp.bits) {
                    if ge.has_lit(*gn) && te.has_lit(*tn) {
                        source_ties.push((gold_lits[gn], gate_lits[tn]));
                    }
                }
            }
            for (g, t) in matched {
                if ge.has_lit(g.q) && te.has_lit(t.q) {
                    source_ties.push((gold_lits[&g.q], gate_lits[&t.q]));
                }
            }
            for (gn, lit) in &gold_lits {
                if gold.net(*gn).drivers.is_empty() {
                    if let Some(tn) = gate.net_by_name(gold.net(*gn).name.as_str()) {
                        if let Some(target) = gate_lits.get(&tn) {
                            source_ties.push((*lit, *target));
                        }
                    }
                }
            }
            source_ties.sort_unstable();
            source_ties.dedup();
            for (gold_lit, gate_lit) in source_ties {
                eq(&mut cnf, gold_lit, gate_lit);
            }

            sync_incremental_solver(&mut solver, &cnf, &mut solver_clause_count);
            let resolved_result = solver.solve_cancellable(cancellation);
            match resolved_result {
                Ok(SatResult::Unsat) => break SatResult::Unsat,
                Ok(SatResult::Sat(resolved_model)) => model = resolved_model,
                Err(error) if error.contains("stuck") => {
                    if std::env::var("EDA_LEC_DEBUG").is_ok() {
                        eprintln!("LEC source refinement capacity: {error}");
                    }
                    return Err(format!("LEC source refinement stuck: {error}"));
                }
                Err(error) => return Err(error),
            }
        }

        // CEGAR consistency check. A cut model is admissible only if its
        // source assignment also distinguishes the two full cones. If they
        // agree, restore the selected frontier's exact source relations and
        // block only that evaluated source assignment. Both refinements are
        // backed by full-cone evaluation, so neither can turn a real
        // counterexample into UNSAT.
        let mut shared_literals: HashMap<String, (i32, bool)> = HashMap::new();
        for (net, lit) in &gold_lits {
            if is_full_cone_boundary(gold, *net) {
                shared_literals
                    .entry(gold.net(*net).name.clone())
                    .or_insert((*lit, model[(*lit - 1) as usize]));
            }
        }
        for (net, lit) in &gate_lits {
            if is_full_cone_boundary(gate, *net) {
                let shifted = *lit;
                shared_literals
                    .entry(gate.net(*net).name.clone())
                    .or_insert((shifted, model[(shifted - 1) as usize]));
            }
        }
        let shared_values: HashMap<String, bool> = shared_literals
            .iter()
            .map(|(name, (_, value))| (name.clone(), *value))
            .collect();
        let gold_full = full_cone_values(gold, &gold_lits, &model, &shared_values);
        let gate_full = full_cone_values(gate, &gate_lits, &model, &shared_values);
        let gold_full_output = gold_full.get(&g_net).copied().unwrap_or(false);
        let gate_full_output = gate_full.get(&t_net).copied().unwrap_or(false);
        if gold_full_output != gate_full_output {
            break SatResult::Sat(model);
        }

        // Exact functional refinement is stronger than assignment blocking:
        // expanding the selected internal-frontier cuts restores their complete
        // source relations. This is done only after a candidate false model,
        // so ordinary local queries retain the small cut abstraction.
        // Map each gate cut back to its tied gold cut so a shared internal
        // correspondence point is expanded only once.
        let mut gold_source_for_gate: HashMap<NetId, NetId> =
            matched.iter().map(|(g, t)| (t.q, g.q)).collect();
        for (gn, tn, _) in points {
            gold_source_for_gate.insert(*tn, *gn);
        }
        let mut expandable_gold: Vec<NetId> = gold_cuts
            .iter()
            .copied()
            .filter(|net| !is_full_cone_boundary(gold, *net) && gold_lits.contains_key(net))
            .collect();
        expandable_gold.sort_unstable();
        let expanded_gold: NetIdSet = expandable_gold
            .into_iter()
            .filter(|net| bounded_region_len(gold, *net, |_| false, 20_000) != usize::MAX)
            .map(|net| {
                ge.enc_cone(net)?;
                Ok(net)
            })
            .collect::<Result<_, String>>()?;
        let expandable_gate: Vec<NetId> = gate_cuts
            .iter()
            .copied()
            .filter(|net| {
                !is_full_cone_boundary(gate, *net)
                    && !gold_source_for_gate
                        .get(net)
                        .is_some_and(|gold_net| expanded_gold.contains(gold_net))
                    && gate_lits.contains_key(net)
            })
            .collect();
        let expanded_gate: NetIdSet = expandable_gate
            .into_iter()
            .filter(|net| bounded_region_len(gate, *net, |_| false, 20_000) != usize::MAX)
            .map(|net| {
                te.enc_cone(net)?;
                Ok(net)
            })
            .collect::<Result<_, String>>()?;

        let old_lits = combined_old_literal_map(&ge, &gold_lits);
        let delta = append_encoder_delta(&mut cnf, &ge, ge_refinement_base, &|lit| {
            let local_var = lit.unsigned_abs() as i32;
            let mapped = old_lits.get(&local_var).copied();
            match mapped {
                Some(target) if lit > 0 => target,
                Some(target) => -target,
                None => lit,
            }
        });
        ge_refinement_base = (ge.cnf.n_vars, ge.cnf.clauses.len());
        for (net, lit) in delta {
            gold_lits.insert(net, lit);
        }
        let old_lits = combined_old_literal_map(&te, &gate_lits);
        let delta = append_encoder_delta(&mut cnf, &te, te_refinement_base, &|lit| {
            let local_var = lit.unsigned_abs() as i32;
            let mapped = old_lits.get(&local_var).copied();
            match mapped {
                Some(target) if lit > 0 => target,
                Some(target) => -target,
                None => shift(lit, offset),
            }
        });
        te_refinement_base = (te.cnf.n_vars, te.cnf.clauses.len());
        for (net, lit) in delta {
            gate_lits.insert(net, lit);
        }

        let mut guards: Vec<i32> = shared_literals.values().map(|(lit, _)| *lit).collect();
        guards.sort_unstable();
        if std::env::var("EDA_LEC_DEBUG").is_ok() {
            let mut gold_names: Vec<&str> = expanded_gold
                .iter()
                .map(|net| gold.net(*net).name.as_str())
                .collect();
            gold_names.sort_unstable();
            let mut gate_names: Vec<&str> = expanded_gate
                .iter()
                .map(|net| gate.net(*net).name.as_str())
                .collect();
            gate_names.sort_unstable();
            eprintln!(
                "LEC refinement #{refinements}: full={gold_full_output}/{} guards={} expanded_gold={} expanded_gate={}",
                gate_full_output,
                guards.len(),
                expanded_gold.len(),
                expanded_gate.len(),
            );
            eprintln!(
                "LEC expanded gold cuts ({}): {:?}",
                gold_names.len(),
                &gold_names[..gold_names.len().min(12)]
            );
            eprintln!(
                "LEC expanded gate cuts ({}): {:?}",
                gate_names.len(),
                &gate_names[..gate_names.len().min(12)]
            );
        }
        if refinements >= 4 || guards.is_empty() {
            break SatResult::Sat(model);
        }
        let blocking: Vec<i32> = shared_literals
            .values()
            .map(|(lit, value)| if *value { -*lit } else { *lit })
            .collect();
        if std::env::var("EDA_LEC_DEBUG").is_ok() {
            eprintln!(
                "LEC refinement block: clause_len={} vars={} clauses={}",
                blocking.len(),
                cnf.n_vars,
                cnf.clauses.len()
            );
        }
        cnf.add_clause(blocking);
        refinements += 1;
    };
    match solved {
        SatResult::Unsat => Ok(MiterVerdict::Equivalent),
        SatResult::Sat(model) => {
            if let Ok(path) = std::env::var("EDA_LEC_DUMP") {
                use std::fmt::Write as _;
                let mut out = String::new();
                for (n, l) in &gold_lits {
                    let _ = writeln!(
                        out,
                        "{} {}",
                        gold.net(*n).name,
                        model[(*l - 1) as usize] as u8
                    );
                }
                let _ = std::fs::write(path, out);
            }
            if std::env::var("EDA_LEC_TRACE").is_ok() {
                eprintln!("  SAT model len={}", model.len());
                let violated: Vec<&Vec<i32>> = cnf
                    .clauses
                    .iter()
                    .filter(|clause| {
                        !clause.iter().any(|lit| {
                            model[(lit.unsigned_abs() as i32 - 1) as usize] == (*lit > 0)
                        })
                    })
                    .collect();
                eprintln!("  SAT model violated {} clauses", violated.len());
                if violated.len() <= 8 {
                    for clause in violated {
                        eprintln!("    violated {clause:?}");
                    }
                }
                for (n, l) in &gold_lits {
                    eprintln!(
                        "  g {}#{}#{} = {}",
                        gold.net(*n).name,
                        n,
                        l,
                        model[(*l - 1) as usize]
                    );
                }
                for (n, l) in &gate_lits {
                    let shifted = *l;
                    eprintln!(
                        "  t {}#{}#{} = {}",
                        gate.net(*n).name,
                        n,
                        shifted,
                        model[(shifted - 1) as usize]
                    );
                }
                eprintln!("  clauses:");
                for c in &cnf.clauses {
                    eprintln!("    {c:?}");
                }
            }
            if std::env::var("EDA_LEC_SELFCHECK").is_ok() {
                static SELF_CHECKED: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                if SELF_CHECKED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    // already checked once; skip expensive re-evaluation
                } else {
                    // 自检：用模型中的自由变量组合求值两侧锥，比对 miter 字面量。
                    let model_copy = model.clone();
                    let eval_side = move |nl: &Netlist,
                                          vars: &HashMap<NetId, i32>|
                          -> HashMap<NetId, bool> {
                        let mut vals: HashMap<NetId, eda_core::logic::LogicValue> = HashMap::new();
                        // 自由变量 = 锥内无驱动或在割集/时序边界的网络。
                        for (n, l) in vars {
                            let drivers = nl.net(*n).drivers.clone();
                            let boundary = drivers.iter().any(|&d| {
                            let cell = nl.cell(d);
                            matches!(cell.kind, CellKind::Dff { .. } | CellKind::DffR { .. })
                                || nl.library().is_some_and(|lib| {
                                    matches!(cell.kind, CellKind::Lib(id) if lib.get(id).seq.is_some())
                                })
                        });
                            if drivers.is_empty() || boundary {
                                vals.insert(
                                    *n,
                                    eda_core::logic::LogicValue::bit(model_copy[(*l - 1) as usize]),
                                );
                            }
                        }
                        let order = nl.topo_cells().unwrap_or_default();
                        for ci in &order {
                            let cell = nl.cell(*ci);
                            let seq = matches!(cell.kind, CellKind::Dff { .. } | CellKind::DffR { .. })
                            || nl.library().is_some_and(|lib| {
                                matches!(cell.kind, CellKind::Lib(id) if lib.get(id).seq.is_some())
                            });
                            if seq {
                                continue;
                            }
                            let v = nl.eval_cell(*ci, &vals);
                            vals.insert(cell.output, v);
                        }
                        (0..nl.net_count())
                            .map(|n| (n, vals.get(&n).and_then(|v| v.as_bool()).unwrap_or(false)))
                            .collect()
                    };
                    let gval = eval_side(gold, &gold_lits)[&g_net];
                    let tval = eval_side(gate, &gate_lits)[&t_net];
                    // 诊断：仅在单侧锥出现的匹配状态位（未绑定 → 模型可取不同值）。
                    let one_sided: Vec<&str> = matched
                        .iter()
                        .filter(|(g, t)| ge.has_lit(g.q) != te.has_lit(t.q))
                        .map(|(g, _)| gold.net(g.q).name.as_str())
                        .collect();
                    if !one_sided.is_empty() {
                        eprintln!(
                            "LEC SELFCHECK ONE-SIDED STATE VARS ({}): {:?}...",
                            one_sided.len(),
                            &one_sided[..one_sided.len().min(6)]
                        );
                    }
                    let one_sided_pis: Vec<&str> = gold
                        .ports
                        .iter()
                        .filter(|p| p.dir == Dir::In)
                        .filter_map(|p| {
                            let gp = gate.port_by_name(&p.name).unwrap();
                            let g_in: Vec<bool> = p.bits.iter().map(|b| ge.has_lit(*b)).collect();
                            let t_in: Vec<bool> = gp.bits.iter().map(|b| te.has_lit(*b)).collect();
                            if g_in != t_in {
                                Some(p.name.as_str())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !one_sided_pis.is_empty() {
                        eprintln!(
                            "LEC SELFCHECK ONE-SIDED PI PORTS ({}): {:?}",
                            one_sided_pis.len(),
                            &one_sided_pis[..one_sided_pis.len().min(8)]
                        );
                    }
                    let a_lit = model[(a - 1) as usize];
                    let b_lit = model[(b - 1) as usize];
                    if a_lit != gval || b_lit != tval {
                        eprintln!(
                        "LEC SELFCHECK CNF-VS-EVAL: gold '{}' cnf={a_lit} eval={gval}; gate '{}' cnf={b_lit} eval={tval}",
                        gold.net(g_net).name,
                        gate.net(t_net).name
                    );
                    }
                    if a_lit != b_lit && gval == tval {
                        eprintln!(
                        "LEC SELFCHECK ARTIFACT: gold '{}' eval={gval}, gate '{}' eval={tval}, but miter distinguishes (a={a_lit}, b={b_lit})",
                        gold.net(g_net).name,
                        gate.net(t_net).name
                    );
                    }
                }
            }
            let mut assignment = HashMap::new();
            let mut phases = HashMap::new();
            for (net, lit) in &ge.var {
                let value = model[(*lit - 1) as usize];
                assignment.insert(*net, value);
                phases.insert(lit.unsigned_abs() as usize, value);
            }
            for (index, value) in model.iter().enumerate() {
                phases.entry(index + 1).or_insert(*value);
            }
            Ok(MiterVerdict::Distinguished {
                gold: assignment,
                phases,
            })
        }
    }
}

#[cfg(test)]
#[path = "equiv/tests.rs"]
mod tests;
