//! Combinational & sequential equivalence checking via miter construction
//! over a shared SAT instance.

use crate::cnf::NetlistEncoder;
use crate::sat::{solve_local_cancellable, IncrementalSolver, SatResult};
use eda_core::cancellation::FlowCancellation;
use eda_core::library::LibFunc;
use eda_core::logic::LogicWord;
use eda_core::netlist::{CellKind, Dir, NetId, Netlist};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct EquivResult {
    pub equivalent: bool,
    /// When not equivalent: an input assignment (per port name) that
    /// distinguishes the two netlists.
    pub counterexample: Option<HashMap<String, LogicWord>>,
    pub notes: Vec<String>,
}

struct Encoded {
    cnf: crate::sat::Cnf,
    lit: HashMap<NetId, i32>,
}

/// Encode the whole netlist: every net gets a literal; combinational cells
/// are constrained; flip-flop outputs stay free (state) with their d-input
/// cones encoded.
fn encode_netlist(nl: &Netlist, cancellation: &FlowCancellation) -> Result<Encoded, String> {
    let mut e = NetlistEncoder::new(nl);
    for n in 0..nl.net_count() {
        if cancellation.is_cancelled() {
            return Err("equivalence checking cancelled".into());
        }
        e.lit_of(n);
    }
    for ci in 0..nl.cell_count() {
        if cancellation.is_cancelled() {
            return Err("equivalence checking cancelled".into());
        }
        let c = nl.cell(ci);
        match &c.kind {
            CellKind::Dff { .. } | CellKind::DffR { .. } => {
                // state element: q is free; encode the d cone only
                e.enc_cone(c.inputs[1])?;
                if c.inputs.len() > 2 {
                    e.enc_cone(c.inputs[2])?; // reset net cone (if driven)
                }
                e.enc_cone(c.inputs[0])?; // clock cone
            }
            CellKind::Lib(lid) => {
                let lib = nl
                    .library()
                    .ok_or("library cell without attached library")?;
                let def = lib.get(*lid);
                if let Some(seq) = def.seq.as_ref() {
                    let clk_pin = seq.clk_pin;
                    let reset_pin = seq.reset_pin;
                    e.enc_cone(c.inputs[1])?; // d
                    e.enc_cone(c.inputs[clk_pin])?;
                    if let Some(rp) = reset_pin {
                        e.enc_cone(c.inputs[rp])?;
                    }
                } else {
                    for &i in &c.inputs {
                        e.enc_cone(i)?;
                    }
                    e.enc_cell(ci)?;
                }
            }
            _ => {
                for &i in &c.inputs {
                    e.enc_cone(i)?;
                }
                e.enc_cell(ci)?;
            }
        }
    }
    let cnf = std::mem::take(&mut e.cnf);
    let lit = e.var.clone();
    Ok(Encoded { cnf, lit })
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

    let ge = encode_netlist(gold, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("equivalence checking cancelled".into());
    }
    let te = encode_netlist(gate, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("equivalence checking cancelled".into());
    }
    let offset = ge.cnf.n_vars as i32;

    let mut cnf = ge.cnf.clone();
    while cnf.n_vars < ge.cnf.n_vars + te.cnf.n_vars {
        cnf.add_var();
    }
    for cl in &te.cnf.clauses {
        cnf.add_clause(cl.iter().map(|l| shift(*l, offset)).collect());
    }

    let glit = |n: NetId| ge.lit[&n];
    let tlit = |n: NetId| shift(te.lit[&n], offset);
    let eq = |cnf: &mut crate::sat::Cnf, a: i32, b: i32| {
        cnf.add_clause(vec![-a, b]);
        cnf.add_clause(vec![a, -b]);
    };

    // tie primary inputs together
    for p in &gold.ports {
        if cancellation.is_cancelled() {
            return Err("equivalence checking cancelled".into());
        }
        if p.dir != Dir::In {
            continue;
        }
        let gp = gate.port_by_name(&p.name).unwrap();
        for k in 0..p.bits.len() {
            eq(&mut cnf, glit(p.bits[k]), tlit(gp.bits[k]));
        }
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

    // Visit intrinsically large next-state cones first. This is a fail-fast
    // scheduling policy, not a change to the proof itself: if a deep RF/ALU
    // obligation is capacity-limited, reporting it before thousands of easier
    // storage bits avoids spending the entire budget walking toward the same
    // conservative verdict.
    let no_cuts = std::collections::HashSet::new();
    let mut proof_order: Vec<(usize, &str, &FlopRec, &FlopRec)> = matched
        .iter()
        .map(|(g, t)| {
            let gold_rank = bounded_region_len(gold, g.d, &no_cuts, 20_000);
            let gate_rank = bounded_region_len(gate, t.d, &no_cuts, 20_000);
            let rank = if gold_rank == usize::MAX || gate_rank == usize::MAX {
                usize::MAX
            } else {
                gold_rank.saturating_add(gate_rank)
            };
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
    for (g_net, t_net, _) in &points {
        if let Some(result) = prove(
            gold,
            gate,
            *g_net,
            *t_net,
            format!("internal point '{}'", gold.net(*g_net).name),
            &mut queries,
        )? {
            return Ok(result);
        }
    }
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
    for (net, lit) in vars {
        if is_full_cone_boundary(nl, *net) {
            values.insert(*net, model[(*lit - 1) as usize]);
        }
    }
    for (name, value) in shared {
        if let Some(net) = nl.net_by_name(name) {
            values.insert(net, *value);
        }
    }
    for cell_id in nl.topo_cells().unwrap_or_default() {
        let cell = nl.cell(cell_id);
        if is_sequential_cell(&cell.kind, nl) {
            continue;
        }
        let value = nl.eval_cell(
            cell_id,
            &values
                .iter()
                .map(|(net, value)| (*net, eda_core::logic::LogicValue::bit(*value)))
                .collect(),
        );
        if let Some(bit) = value.as_bool() {
            values.insert(cell.output, bit);
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
    let mut gold_cuts: std::collections::HashSet<NetId> = matched
        .iter()
        .filter(|(g, t)| {
            g.q != g_net && t.q != t_net && gold_iv.contains_key(&g.q) && gate_iv.contains_key(&t.q)
        })
        .map(|(g, _)| g.q)
        .collect();
    let mut gate_cuts: std::collections::HashSet<NetId> = matched
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
    let a = ge.enc_cone_with_cuts(g_net, &gold_cuts)?;
    let mut te = NetlistEncoder::new(gate);
    let b_raw = te.enc_cone_with_cuts(t_net, &gate_cuts)?;
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
        let expanded_gold: std::collections::HashSet<NetId> = expandable_gold
            .into_iter()
            .filter(|net| {
                bounded_region_len(gold, *net, &std::collections::HashSet::new(), 20_000)
                    != usize::MAX
            })
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
        let expanded_gate: std::collections::HashSet<NetId> = expandable_gate
            .into_iter()
            .filter(|net| {
                bounded_region_len(gate, *net, &std::collections::HashSet::new(), 20_000)
                    != usize::MAX
            })
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

/// Per-net boolean traces from shared random simulation.
type ValueTraces = Vec<Vec<bool>>;

/// Evaluate both netlists over K identical random PI/state rounds and
/// return per-net value traces (signature material shared by inference and
/// region verification).
#[allow(clippy::type_complexity)]
fn simulate_shared_values(
    gold: &Netlist,
    gate: &Netlist,
    gf: &[FlopRec],
    tf: &[FlopRec],
    cancellation: &FlowCancellation,
) -> Result<(ValueTraces, ValueTraces), String> {
    if cancellation.is_cancelled() {
        return Err("equivalence checking cancelled".into());
    }
    const ROUNDS: usize = 48;
    let mut rng: u64 = 0x9e3779b97f4a7c15;
    let next_bit = |rng: &mut u64| {
        *rng ^= *rng << 13;
        *rng ^= *rng >> 7;
        *rng ^= *rng << 17;
        *rng & 1 == 1
    };

    let gate_flops: HashMap<&str, NetId> = tf.iter().map(|f| (f.q_name.as_str(), f.q)).collect();
    let evaluate = |nl: &Netlist, seeds: &Vec<HashMap<NetId, bool>>| -> Vec<Vec<bool>> {
        let order = nl.topo_cells().unwrap_or_default();
        seeds
            .iter()
            .map(|seed| {
                let mut vals: HashMap<NetId, eda_core::logic::LogicValue> = seed
                    .iter()
                    .map(|(n, b)| (*n, eda_core::logic::LogicValue::bit(*b)))
                    .collect();
                for ci in &order {
                    let cell = nl.cell(*ci);
                    let sequential = matches!(
                        cell.kind,
                        CellKind::Dff { .. } | CellKind::DffR { .. }
                    ) || nl.library().is_some_and(
                        |lib| matches!(cell.kind, CellKind::Lib(id) if lib.get(id).seq.is_some()),
                    );
                    if sequential {
                        continue;
                    }
                    let v = nl.eval_cell(*ci, &vals);
                    vals.insert(cell.output, v);
                }
                (0..nl.net_count())
                    .map(|n| vals.get(&n).and_then(|v| v.as_bool()).unwrap_or(false))
                    .collect()
            })
            .collect()
    };

    let mut seeds = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let mut gold_seed = HashMap::new();
        let mut gate_seed = HashMap::new();
        for p in &gold.ports {
            if p.dir != Dir::In {
                continue;
            }
            let bit = next_bit(&mut rng);
            for b in &p.bits {
                gold_seed.insert(*b, bit);
            }
            if let Some(gp) = gate.port_by_name(&p.name) {
                for b in &gp.bits {
                    gate_seed.insert(*b, bit);
                }
            }
        }
        for gold_flop in gf {
            let bit = next_bit(&mut rng);
            gold_seed.insert(gold_flop.q, bit);
            if let Some(&tn) = gate_flops.get(gold_flop.q_name.as_str()) {
                gate_seed.insert(tn, bit);
            }
        }
        seeds.push((gold_seed, gate_seed));
    }
    let gold_seeds: Vec<_> = seeds.iter().map(|(g, _)| g.clone()).collect();
    let gate_seeds: Vec<_> = seeds.iter().map(|(_, t)| t.clone()).collect();
    Ok((evaluate(gold, &gold_seeds), evaluate(gate, &gate_seeds)))
}

/// Local-region equivalence of a candidate pair: encode each side's cone
/// cut at every already-accepted correspondence point (plus PIs/flops),
/// tie the cut variables, and require the XOR miter to be UNSAT. Processing
/// candidates in topological order keeps each region small even where the
/// mapper restructured several gates into one library cell (or vice versa).
fn verify_local_region(
    gold: &Netlist,
    gate: &Netlist,
    gn: NetId,
    tn: NetId,
    accepted: &HashMap<NetId, NetId>,
    result_cache: &mut HashMap<Vec<i32>, bool>,
    cancellation: &FlowCancellation,
) -> Result<bool, String> {
    let gold_cuts: std::collections::HashSet<NetId> =
        accepted.keys().copied().filter(|n| *n != gn).collect();
    let gate_cuts: std::collections::HashSet<NetId> =
        accepted.values().copied().filter(|n| *n != tn).collect();
    // Reject oversized proposals before CNF encoding. Induction regions are
    // normally small; a deep region means the accepted map has not reached
    // that area yet, and encoding every ambiguous proposal is too expensive.
    const MAX_REGION_NETS: usize = 128;
    let gold_region_len = bounded_region_len(gold, gn, &gold_cuts, MAX_REGION_NETS);
    let gate_region_len = bounded_region_len(gate, tn, &gate_cuts, MAX_REGION_NETS);
    if gold_region_len == usize::MAX
        || gate_region_len == usize::MAX
        || gold_region_len + gate_region_len > MAX_REGION_NETS
    {
        return Ok(false);
    }
    let mut ge = NetlistEncoder::new(gold);
    let a = ge.enc_cone_with_cuts(gn, &gold_cuts)?;
    let mut te = NetlistEncoder::new(gate);
    let b = te.enc_cone_with_cuts(tn, &gate_cuts)?;
    let offset = ge.cnf.n_vars as i32;
    let mut cnf = ge.cnf.clone();
    while cnf.n_vars < (offset + te.cnf.n_vars as i32) as usize {
        cnf.add_var();
    }
    for cl in &te.cnf.clauses {
        cnf.add_clause(cl.iter().map(|l| shift(*l, offset)).collect());
    }
    // Driverless nets with the same RTL name are the same logical free
    // variable. Failing to tie them makes a valid local candidate look
    // inequivalent when SAT assigns the two copies different values.
    let mut gold_free: HashMap<&str, NetId> = HashMap::new();
    for n in ge.var.keys() {
        if gold.net(*n).drivers.is_empty() {
            gold_free.insert(gold.net(*n).name.as_str(), *n);
        }
    }
    for n in te.var.keys() {
        if gate.net(*n).drivers.is_empty() {
            if let Some(&gn) = gold_free.get(gate.net(*n).name.as_str()) {
                let gl = ge.var[&gn];
                let tl = shift(te.var[n], offset);
                cnf.add_clause(vec![-gl, tl]);
                cnf.add_clause(vec![gl, -tl]);
            }
        }
    }
    for (gin, tin) in accepted {
        if *gin != gn && *tin != tn && ge.has_lit(*gin) && te.has_lit(*tin) {
            let gl = ge.var[gin];
            let tl = shift(te.var[tin], offset);
            cnf.add_clause(vec![-gl, tl]);
            cnf.add_clause(vec![gl, -tl]);
        }
    }
    let b = shift(b, offset);
    let x = cnf.add_var() as i32;
    cnf.add_clause(vec![-a, -b, -x]);
    cnf.add_clause(vec![-a, b, x]);
    cnf.add_clause(vec![a, -b, x]);
    cnf.add_clause(vec![a, b, -x]);
    cnf.add_clause(vec![x]);
    let cache_key = canonical_cnf_key(&cnf);
    if let Some(&verdict) = result_cache.get(&cache_key) {
        return Ok(verdict);
    }
    let verdict = match solve_local_cancellable(&cnf, cancellation, 10_000) {
        Ok(SatResult::Unsat) => true,
        // SAT = real local difference; stuck = cannot conclude. Both reject
        // the candidate without affecting soundness.
        _ => false,
    };
    if result_cache.len() < 512 {
        result_cache.insert(cache_key, verdict);
    }
    Ok(verdict)
}

/// Canonicalize variable numbers in first-occurrence order. Different bit
/// slices often produce byte-for-byte isomorphic local CNFs; this exact key
/// lets repeated regions reuse a prior verdict without relying on a hash
/// collision.
fn canonical_cnf_key(cnf: &crate::sat::Cnf) -> Vec<i32> {
    let mut var_map: HashMap<usize, usize> = HashMap::new();
    let mut next_var = 1usize;
    let mut key = Vec::new();
    let mut clause_order = (0..cnf.clauses.len()).collect::<Vec<_>>();
    clause_order.sort_by(|left, right| cnf.clauses[*left].cmp(&cnf.clauses[*right]));
    for ci in clause_order {
        let clause = &cnf.clauses[ci];
        key.push(clause.len() as i32);
        for lit in clause {
            let old_var = lit.unsigned_abs() as usize;
            let new_var = *var_map.entry(old_var).or_insert_with(|| {
                let v = next_var;
                next_var += 1;
                v
            });
            key.push(if *lit > 0 {
                new_var as i32
            } else {
                -(new_var as i32)
            });
        }
    }
    key
}

fn vector_base_and_index(name: &str) -> Option<(&str, usize)> {
    let (base, suffix) = name.rsplit_once('[')?;
    if base.is_empty() || !suffix.ends_with(']') {
        return None;
    }
    let index = suffix.trim_end_matches(']').parse().ok()?;
    Some((base, index))
}

/// Prove a complete RTL-named vector as one shared local miter. Bit-by-bit
/// proofs duplicate wide arithmetic cones; encoding all bits over one pair of
/// cone encoders shares every common subexpression.
#[allow(clippy::too_many_arguments)]
fn verify_vector_region(
    gold: &Netlist,
    gate: &Netlist,
    pairs: &[(NetId, NetId)],
    accepted: &HashMap<NetId, NetId>,
    cancellation: &FlowCancellation,
) -> Result<bool, String> {
    verify_vector_region_with_cap(gold, gate, pairs, accepted, cancellation, 4_000)
}

fn verify_vector_region_with_cap(
    gold: &Netlist,
    gate: &Netlist,
    pairs: &[(NetId, NetId)],
    accepted: &HashMap<NetId, NetId>,
    cancellation: &FlowCancellation,
    vector_cap: usize,
) -> Result<bool, String> {
    if pairs.len() < 2 {
        return Ok(false);
    }
    let gold_sinks: std::collections::HashSet<NetId> = pairs.iter().map(|(gn, _)| *gn).collect();
    let gate_sinks: std::collections::HashSet<NetId> = pairs.iter().map(|(_, tn)| *tn).collect();
    let gold_cuts: std::collections::HashSet<NetId> = accepted
        .keys()
        .copied()
        .filter(|net| !gold_sinks.contains(net))
        .collect();
    let gate_cuts: std::collections::HashSet<NetId> = accepted
        .values()
        .copied()
        .filter(|net| !gate_sinks.contains(net))
        .collect();

    let mut ge = NetlistEncoder::new(gold);
    let mut gold_literals = Vec::with_capacity(pairs.len());
    for (gn, _) in pairs {
        gold_literals.push(ge.enc_cone_with_cuts(*gn, &gold_cuts)?);
    }
    let mut te = NetlistEncoder::new(gate);
    let mut gate_literals = Vec::with_capacity(pairs.len());
    for (_, tn) in pairs {
        gate_literals.push(te.enc_cone_with_cuts(*tn, &gate_cuts)?);
    }
    if ge.cnf.n_vars + te.cnf.n_vars > vector_cap {
        if std::env::var("EDA_LEC_DEBUG").is_ok() {
            eprintln!(
                "LEC vector region oversized: base='{}' pairs={} vars={} cap={}",
                gold.net(pairs[0].0).name,
                pairs.len(),
                ge.cnf.n_vars + te.cnf.n_vars,
                vector_cap
            );
        }
        return Ok(false);
    }

    let offset = ge.cnf.n_vars as i32;
    let mut ge_refinement_base = (ge.cnf.n_vars, ge.cnf.clauses.len());
    let mut te_refinement_base = (te.cnf.n_vars, te.cnf.clauses.len());
    let mut cnf = ge.cnf.clone();
    while cnf.n_vars < (offset + te.cnf.n_vars as i32) as usize {
        cnf.add_var();
    }
    for clause in &te.cnf.clauses {
        cnf.add_clause(clause.iter().map(|lit| shift(*lit, offset)).collect());
    }
    let mut gold_combined_lits = ge.var.clone();
    let mut gate_combined_lits: HashMap<NetId, i32> = te
        .var
        .iter()
        .map(|(net, lit)| (*net, shift(*lit, offset)))
        .collect();
    for (gin, tin) in accepted {
        if gold_sinks.contains(gin) || gate_sinks.contains(tin) {
            continue;
        }
        if let (Some(gl), Some(tl)) = (
            gold_combined_lits.get(gin).filter(|lit| **lit != 0),
            gate_combined_lits.get(tin).filter(|lit| **lit != 0),
        ) {
            cnf.add_clause(vec![-*gl, *tl]);
            cnf.add_clause(vec![*gl, -*tl]);
        }
    }
    for port in &gold.ports {
        if port.dir != Dir::In {
            continue;
        }
        let Some(target_port) = gate.port_by_name(&port.name) else {
            continue;
        };
        for (gb, tb) in port.bits.iter().zip(&target_port.bits) {
            if let (Some(gl), Some(tl)) = (
                gold_combined_lits.get(gb).filter(|lit| **lit != 0),
                gate_combined_lits.get(tb).filter(|lit| **lit != 0),
            ) {
                cnf.add_clause(vec![-*gl, *tl]);
                cnf.add_clause(vec![*gl, -*tl]);
            }
        }
    }
    let mut differences = Vec::with_capacity(pairs.len());
    for (gl, tl) in gold_literals.iter().zip(&gate_literals) {
        let tl = shift(*tl, offset);
        let difference = cnf.add_var() as i32;
        cnf.add_clause(vec![-*gl, -tl, -difference]);
        cnf.add_clause(vec![-*gl, tl, difference]);
        cnf.add_clause(vec![*gl, -tl, difference]);
        cnf.add_clause(vec![*gl, tl, -difference]);
        differences.push(difference);
    }
    cnf.add_clause(differences);

    let mut solver = IncrementalSolver::new(&cnf);
    let mut solver_clause_count = cnf.clauses.len();
    let mut sources_materialized = false;
    let mut refinements = 0usize;
    let vector_verdict = loop {
        sync_incremental_solver(&mut solver, &cnf, &mut solver_clause_count);
        let vector_verdict = solver.solve_with_limit_cancellable(cancellation, 250_000);
        let vector_verdict_label = match &vector_verdict {
            Ok(SatResult::Unsat) => "unsat".to_string(),
            Ok(SatResult::Sat(_)) => "sat".to_string(),
            Err(error) => format!("unknown ({error})"),
        };
        if std::env::var("EDA_LEC_DEBUG").is_ok() {
            eprintln!(
                "LEC vector region: base='{}' pairs={} vars={} clauses={} verdict={}",
                gold.net(pairs[0].0).name,
                pairs.len(),
                cnf.n_vars,
                cnf.clauses.len(),
                vector_verdict_label
            );
        }
        let vector_verdict = match vector_verdict {
            Ok(verdict) => verdict,
            Err(error) => break Err(error),
        };
        let SatResult::Sat(mut model) = vector_verdict else {
            break Ok(vector_verdict);
        };

        if !sources_materialized {
            sources_materialized = true;
            let mut gold_sources: Vec<NetId> = pairs
                .iter()
                .flat_map(|(gn, _)| cone_intervals(gold, *gn).into_keys())
                .filter(|net| is_full_cone_boundary(gold, *net))
                .collect();
            gold_sources.sort_unstable();
            gold_sources.dedup();
            for net in gold_sources {
                ge.lit_of(net);
            }
            let delta = append_encoder_delta(&mut cnf, &ge, ge_refinement_base, &|lit| lit);
            ge_refinement_base = (ge.cnf.n_vars, ge.cnf.clauses.len());
            for (net, lit) in delta {
                gold_combined_lits.insert(net, lit);
            }

            let mut gate_sources: Vec<NetId> = pairs
                .iter()
                .flat_map(|(_, tn)| cone_intervals(gate, *tn).into_keys())
                .filter(|net| is_full_cone_boundary(gate, *net))
                .collect();
            gate_sources.sort_unstable();
            gate_sources.dedup();
            for net in gate_sources {
                te.lit_of(net);
            }
            let delta =
                append_encoder_delta(&mut cnf, &te, te_refinement_base, &|lit| shift(lit, offset));
            te_refinement_base = (te.cnf.n_vars, te.cnf.clauses.len());
            for (net, lit) in delta {
                gate_combined_lits.insert(net, lit);
            }

            let mut source_ties: Vec<(i32, i32)> = Vec::new();
            for (gin, tin) in accepted {
                if let (Some(gl), Some(tl)) = (
                    gold_combined_lits.get(gin).filter(|lit| **lit != 0),
                    gate_combined_lits.get(tin).filter(|lit| **lit != 0),
                ) {
                    source_ties.push((*gl, *tl));
                }
            }
            for port in &gold.ports {
                if port.dir != Dir::In {
                    continue;
                }
                let Some(target_port) = gate.port_by_name(&port.name) else {
                    continue;
                };
                for (gb, tb) in port.bits.iter().zip(&target_port.bits) {
                    if let (Some(gl), Some(tl)) = (
                        gold_combined_lits.get(gb).filter(|lit| **lit != 0),
                        gate_combined_lits.get(tb).filter(|lit| **lit != 0),
                    ) {
                        source_ties.push((*gl, *tl));
                    }
                }
            }
            for (gn, gl) in &gold_combined_lits {
                if gold.net(*gn).drivers.is_empty() {
                    if let Some(tn) = gate.net_by_name(gold.net(*gn).name.as_str()) {
                        if let Some(tl) = gate_combined_lits.get(&tn) {
                            source_ties.push((*gl, *tl));
                        }
                    }
                }
            }
            source_ties.sort_unstable();
            source_ties.dedup();
            for (gl, tl) in source_ties {
                cnf.add_clause(vec![-gl, tl]);
                cnf.add_clause(vec![gl, -tl]);
            }
            sync_incremental_solver(&mut solver, &cnf, &mut solver_clause_count);
            let resolved = solver.solve_with_limit_cancellable(cancellation, 250_000);
            match resolved {
                Ok(SatResult::Unsat) => break Ok(SatResult::Unsat),
                Ok(SatResult::Sat(resolved_model)) => model = resolved_model,
                Err(_) => break resolved,
            }
        }

        let mut shared_literals: HashMap<String, (i32, bool)> = HashMap::new();
        for (net, lit) in &gold_combined_lits {
            if is_full_cone_boundary(gold, *net) {
                shared_literals
                    .entry(gold.net(*net).name.clone())
                    .or_insert((*lit, model[(*lit - 1) as usize]));
            }
        }
        for (net, lit) in &gate_combined_lits {
            if is_full_cone_boundary(gate, *net) {
                shared_literals
                    .entry(gate.net(*net).name.clone())
                    .or_insert((*lit, model[(*lit - 1) as usize]));
            }
        }
        let shared_values: HashMap<String, bool> = shared_literals
            .iter()
            .map(|(name, (_, value))| (name.clone(), *value))
            .collect();
        let gold_full = full_cone_values(gold, &gold_combined_lits, &model, &shared_values);
        let gate_full = full_cone_values(gate, &gate_combined_lits, &model, &shared_values);
        if pairs
            .iter()
            .any(|(gn, tn)| gold_full.get(gn) != gate_full.get(tn))
        {
            break Ok(SatResult::Sat(model));
        }

        let gold_source_for_gate: HashMap<NetId, NetId> =
            accepted.iter().map(|(gin, tin)| (*tin, *gin)).collect();
        let mut expandable_gold: Vec<NetId> = gold_cuts
            .iter()
            .copied()
            .filter(|net| {
                !is_full_cone_boundary(gold, *net) && gold_combined_lits.contains_key(net)
            })
            .collect();
        expandable_gold.sort_unstable();
        let expanded_gold: std::collections::HashSet<NetId> = expandable_gold
            .into_iter()
            .filter(|net| {
                bounded_region_len(gold, *net, &std::collections::HashSet::new(), 20_000)
                    != usize::MAX
            })
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
                    && gate_combined_lits.contains_key(net)
            })
            .collect();
        let expanded_gate: std::collections::HashSet<NetId> = expandable_gate
            .into_iter()
            .filter(|net| {
                bounded_region_len(gate, *net, &std::collections::HashSet::new(), 20_000)
                    != usize::MAX
            })
            .map(|net| {
                te.enc_cone(net)?;
                Ok(net)
            })
            .collect::<Result<_, String>>()?;

        if std::env::var("EDA_LEC_DEBUG").is_ok() {
            eprintln!(
                "LEC vector refinement: expanded_gold={} expanded_gate={}",
                expanded_gold.len(),
                expanded_gate.len()
            );
        }

        let old_lits = combined_old_literal_map(&ge, &gold_combined_lits);
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
            gold_combined_lits.insert(net, lit);
        }
        let old_lits = combined_old_literal_map(&te, &gate_combined_lits);
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
            gate_combined_lits.insert(net, lit);
        }

        let mut guards: Vec<i32> = shared_literals.values().map(|(lit, _)| *lit).collect();
        guards.sort_unstable();
        if refinements >= 4 || guards.is_empty() {
            break Ok(SatResult::Sat(model));
        }
        let blocking: Vec<i32> = shared_literals
            .values()
            .map(|(lit, value)| if *value { -*lit } else { *lit })
            .collect();
        cnf.add_clause(blocking);
        refinements += 1;
    };
    if std::env::var("EDA_LEC_DEBUG").is_ok() && std::env::var("EDA_SAT_DUMP").is_ok() {
        for clause in &cnf.clauses {
            eprintln!("VEC {clause:?}");
        }
        eprintln!("GOLD LITS {gold_literals:?} GATE LITS {gate_literals:?} OFFSET {offset}");
    }
    match vector_verdict {
        Ok(SatResult::Unsat) => Ok(true),
        _ => Ok(false),
    }
}

fn bounded_region_len(
    nl: &Netlist,
    sink: NetId,
    cuts: &std::collections::HashSet<NetId>,
    cap: usize,
) -> usize {
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![sink];
    while let Some(n) = stack.pop() {
        if seen.len() > cap {
            return usize::MAX;
        }
        if !seen.insert(n) {
            continue;
        }
        if cuts.contains(&n) {
            continue;
        }
        for &cell_id in &nl.net(n).drivers {
            let cell = nl.cell(cell_id);
            let sequential = matches!(cell.kind, CellKind::Dff { .. } | CellKind::DffR { .. })
                || nl.library().is_some_and(
                    |lib| matches!(cell.kind, CellKind::Lib(id) if lib.get(id).seq.is_some()),
                );
            if !sequential {
                stack.extend(cell.inputs.iter().copied());
            }
        }
    }
    seen.len()
}

/// Propose output pairs from the immediate fanout of a newly accepted pair.
/// The proposal is only a hint: every pair still passes the same structural
/// or local-SAT verification before becoming a cut. This bridges topological
/// gaps where optimization removed all stable RTL names but left equivalent
/// downstream logic.
fn direct_fanout_candidates(
    gold: &Netlist,
    gate: &Netlist,
    gn: NetId,
    tn: NetId,
    accepted: &HashMap<NetId, NetId>,
) -> Vec<(NetId, NetId)> {
    let mut out = Vec::new();
    for &gold_sink in &gold.net(gn).sinks {
        let gold_cell = gold.cell(gold_sink);
        if gold_cell.output == gn {
            continue;
        }
        let Some(gold_class) = cell_class(gold, &gold_cell.kind) else {
            continue;
        };
        for &gate_sink in &gate.net(tn).sinks {
            let gate_cell = gate.cell(gate_sink);
            if gate_cell.output == tn {
                continue;
            }
            let Some(gate_class) = cell_class(gate, &gate_cell.kind) else {
                continue;
            };
            if gold_class != gate_class
                || gold_cell.inputs.len() != gate_cell.inputs.len()
                || !gold_cell.inputs.contains(&gn)
                || !gate_cell.inputs.contains(&tn)
            {
                continue;
            }
            // For noncommutative cells preserve pin position. Commutative
            // cells may pair through any pin; local proof rejects a wrong
            // permutation.
            let commutative = matches!(
                gold_class,
                FuncClass::And
                    | FuncClass::Nand
                    | FuncClass::Or
                    | FuncClass::Nor
                    | FuncClass::Xor
                    | FuncClass::Xnor
            );
            let pin_compatible = commutative
                || gold_cell
                    .inputs
                    .iter()
                    .zip(&gate_cell.inputs)
                    .any(|(g_input, t_input)| {
                        accepted
                            .get(g_input)
                            .is_some_and(|mapped| *mapped == *t_input)
                    });
            if pin_compatible {
                out.push((gold_cell.output, gate_cell.output));
            }
        }
    }
    out
}

/// Build the verified correspondence-point set by topological induction.
/// Seeds are primary inputs and flop outputs matched by name; candidates
/// are RTL-named pairs plus 1:1 simulation-signature pairs; a candidate is
/// accepted only when its cut-bounded local-region miter passes with
/// inputs tied through the already-accepted map.
fn explicit_constant(nl: &Netlist, n: NetId) -> Option<bool> {
    if nl.net(n).drivers.len() != 1 {
        return None;
    }
    match &nl.cell(nl.net(n).drivers[0]).kind {
        CellKind::Const(v) => v.as_bool(),
        CellKind::Lib(id) => match &nl.library()?.get(*id).func {
            LibFunc::Const(v) => v.as_bool(),
            _ => None,
        },
        _ => None,
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn build_correspondence_points(
    gold: &Netlist,
    gate: &Netlist,
    gf: &[FlopRec],
    tf: &[FlopRec],
    gold_values: &[Vec<bool>],
    gate_values: &[Vec<bool>],
    named: Vec<(NetId, NetId)>,
    cancellation: &FlowCancellation,
) -> Result<(Vec<(NetId, NetId, bool)>, usize), String> {
    let mut accepted: HashMap<NetId, NetId> = HashMap::new();
    for p in &gold.ports {
        if p.dir != Dir::In {
            continue;
        }
        if let Some(gp) = gate.port_by_name(&p.name) {
            for (gb, tb) in p.bits.iter().zip(&gp.bits) {
                accepted.insert(*gb, *tb);
            }
        }
    }
    let tf_by_name: HashMap<&str, &FlopRec> = tf.iter().map(|f| (f.q_name.as_str(), f)).collect();
    for g in gf {
        if let Some(&t) = tf_by_name.get(g.q_name.as_str()) {
            accepted.insert(g.q, t.q);
        }
    }
    // Explicit constants are safe many-to-one cut seeds. The unoptimized
    // view can contain thousands of constant cells in memory/decode cones,
    // while optimization retains one TIE cell per value; injective point
    // matching alone would leave those cones uncut.
    let mut gate_const: HashMap<bool, NetId> = HashMap::new();
    for n in 0..gate.net_count() {
        if let Some(value) = explicit_constant(gate, n) {
            gate_const.entry(value).or_insert(n);
        }
    }
    for n in 0..gold.net_count() {
        if let Some(value) = explicit_constant(gold, n) {
            if let Some(&t) = gate_const.get(&value) {
                accepted.entry(n).or_insert(t);
            }
        }
    }
    let is_free = |nl: &Netlist, n: NetId| -> bool {
        nl.net(n).drivers.is_empty()
            || nl.net(n).drivers.iter().any(|&d| {
                let cell = nl.cell(d);
                matches!(cell.kind, CellKind::Dff { .. } | CellKind::DffR { .. })
                    || nl.library().is_some_and(
                        |lib| matches!(cell.kind, CellKind::Lib(id) if lib.get(id).seq.is_some()),
                    )
            })
    };
    let signature = |values: &[Vec<bool>], n: NetId| -> Vec<u8> {
        values.iter().map(|round| round[n] as u8).collect()
    };
    let mut gold_sigs: HashMap<Vec<u8>, Vec<NetId>> = HashMap::new();
    for n in 0..gold.net_count() {
        if is_free(gold, n) {
            continue;
        }
        gold_sigs
            .entry(signature(gold_values, n))
            .or_default()
            .push(n);
    }
    let mut gate_sigs: HashMap<Vec<u8>, Vec<NetId>> = HashMap::new();
    for n in 0..gate.net_count() {
        if is_free(gate, n) {
            continue;
        }
        gate_sigs
            .entry(signature(gate_values, n))
            .or_default()
            .push(n);
    }

    let mut inferred_candidates: Vec<(NetId, NetId)> = Vec::new();
    // Only a unique simulation signature identifies a useful unlabelled
    // candidate. Ambiguous signatures can propose many false pairs, each of
    // which burns a local SAT query; they are intentionally left unaccepted.
    for (sig, gnets) in &gold_sigs {
        if gnets.len() != 1 {
            continue;
        }
        if let Some(tnets) = gate_sigs.get(sig) {
            if tnets.len() == 1 {
                inferred_candidates.push((gnets[0], tnets[0]));
            }
        }
    }
    let _named_count = named.len();
    let mut vector_groups: HashMap<String, Vec<(NetId, NetId, usize)>> = HashMap::new();
    for &(gn, tn) in &named {
        if let Some((base, index)) = vector_base_and_index(gold.net(gn).name.as_str()) {
            vector_groups
                .entry(base.to_string())
                .or_default()
                .push((gn, tn, index));
        }
    }
    let vector_groups: HashMap<String, Vec<(NetId, NetId)>> = vector_groups
        .into_iter()
        .filter_map(|(base, mut members)| {
            members.sort_by_key(|(_, _, index)| *index);
            if members.len() < 4 {
                return None;
            }
            let pairs = members
                .into_iter()
                .map(|(gn, tn, _)| (gn, tn))
                .collect::<Vec<_>>();
            Some((base, pairs))
        })
        .collect();
    let mut attempted_vector_groups: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    let mut points: Vec<(NetId, NetId, bool)> = Vec::new();
    let order = gold.topo_cells().unwrap_or_default();
    let mut rank: HashMap<NetId, usize> = HashMap::new();
    for (r, ci) in order.iter().enumerate() {
        rank.insert(gold.cell(*ci).output, r);
    }
    let rank_of = |n: NetId| rank.get(&n).copied().unwrap_or(usize::MAX);
    let mut local_result_cache: HashMap<Vec<i32>, bool> = HashMap::new();
    let candidates: Vec<(NetId, NetId, bool)> = named
        .into_iter()
        .map(|(gn, tn)| (gn, tn, false))
        .chain(
            inferred_candidates
                .into_iter()
                .map(|(gn, tn)| (gn, tn, true)),
        )
        .collect();
    let mut candidate_pairs: std::collections::HashSet<(NetId, NetId)> =
        candidates.iter().map(|(gn, tn, _)| (*gn, *tn)).collect();
    let mut queued_pairs: std::collections::HashSet<(NetId, NetId)> =
        std::collections::HashSet::new();
    let mut candidate_queue: std::collections::BinaryHeap<
        std::cmp::Reverse<(usize, bool, NetId, NetId, bool)>,
    > = candidates
        .into_iter()
        .filter(|(gn, tn, _)| queued_pairs.insert((*gn, *tn)))
        .map(|(gn, tn, inferred)| std::cmp::Reverse((rank_of(gn), inferred, gn, tn, inferred)))
        .collect();

    while let Some(std::cmp::Reverse((_, _, gn, tn, named_flag))) = candidate_queue.pop() {
        if !named_flag {
            if let Some((base, _)) = vector_base_and_index(gold.net(gn).name.as_str()) {
                if let Some(group_pairs) = vector_groups.get(base) {
                    if attempted_vector_groups.insert(base.to_string()) {
                        let mut group_verified =
                            verify_vector_region(gold, gate, group_pairs, &accepted, cancellation)?;
                        if !group_verified && group_pairs.len() >= 8 {
                            // A wide arithmetic vector can be too large as one
                            // miter while adjacent bit slices share enough logic
                            // to prove cleanly. Four-bit slices keep neighbouring
                            // carry/shift bits together without recreating the
                            // full RF write cone.
                            let mut slices_verified = true;
                            for chunk in group_pairs.chunks(4) {
                                if !verify_vector_region_with_cap(
                                    gold,
                                    gate,
                                    chunk,
                                    &accepted,
                                    cancellation,
                                    12_000,
                                )? {
                                    slices_verified = false;
                                    break;
                                }
                            }
                            group_verified = slices_verified;
                        }
                        if group_verified {
                            for (g, t) in group_pairs {
                                if accepted.contains_key(g) {
                                    continue;
                                }
                                accepted.insert(*g, *t);
                                points.push((*g, *t, false));
                            }
                            for (g, t) in group_pairs {
                                for (next_g, next_t) in
                                    direct_fanout_candidates(gold, gate, *g, *t, &accepted)
                                        .into_iter()
                                        .take(8)
                                {
                                    if candidate_pairs.insert((next_g, next_t)) {
                                        candidate_queue.push(std::cmp::Reverse((
                                            rank_of(next_g),
                                            true,
                                            next_g,
                                            next_t,
                                            true,
                                        )));
                                    }
                                }
                            }
                            continue;
                        }
                    }
                }
            }
        }
        let propagate_fanout;
        let structural_probe = structural_probe(gold, gate, gn, tn, &accepted);
        if let Some(matches) = structural_probe.root {
            for (g, t) in matches {
                if accepted.contains_key(&g) {
                    continue;
                }
                accepted.insert(g, t);
                points.push((g, t, named_flag));
            }
            propagate_fanout = true;
        } else {
            let mut added_partial_candidate = false;
            // Technology optimization often replaces one equivalent decode
            // cone with another temp name. If the roots have the same cell
            // class, use equal simulation traces to propose corresponding
            // immediate inputs. These pairs still require local proof.
            let root_gold_driver = gold.net(gn).drivers.first().copied();
            let root_gate_driver = gate.net(tn).drivers.first().copied();
            if let (Some(gold_driver), Some(gate_driver)) = (root_gold_driver, root_gate_driver) {
                let gold_cell = gold.cell(gold_driver);
                let gate_cell = gate.cell(gate_driver);
                if cell_class(gold, &gold_cell.kind).is_some_and(|left| {
                    cell_class(gate, &gate_cell.kind).is_some_and(|right| left == right)
                }) {
                    let gold_signatures = gold_cell
                        .inputs
                        .iter()
                        .map(|input| {
                            gold_values
                                .iter()
                                .map(|round| round[*input] as u8)
                                .collect::<Vec<_>>()
                        })
                        .collect::<Vec<_>>();
                    let gate_signatures = gate_cell
                        .inputs
                        .iter()
                        .map(|input| {
                            gate_values
                                .iter()
                                .map(|round| round[*input] as u8)
                                .collect::<Vec<_>>()
                        })
                        .collect::<Vec<_>>();
                    for (gold_index, gold_signature) in gold_signatures.iter().enumerate() {
                        let nonconstant = gold_signature
                            .iter()
                            .any(|value| *value != gold_signature[0]);
                        let unique_gold = gold_signatures
                            .iter()
                            .filter(|signature| signature == &gold_signature)
                            .count()
                            == 1;
                        if !nonconstant || !unique_gold {
                            continue;
                        }
                        let gold_input = gold_cell.inputs[gold_index];
                        for (gate_index, gate_signature) in gate_signatures.iter().enumerate() {
                            let unique_gate = gate_signatures
                                .iter()
                                .filter(|signature| signature == &gate_signature)
                                .count()
                                == 1;
                            if gold_signature == gate_signature
                                && unique_gate
                                && candidate_pairs
                                    .insert((gold_input, gate_cell.inputs[gate_index]))
                            {
                                let gate_input = gate_cell.inputs[gate_index];
                                candidate_queue.push(std::cmp::Reverse((
                                    rank_of(gold_input),
                                    true,
                                    gold_input,
                                    gate_input,
                                    true,
                                )));
                                added_partial_candidate = true;
                            }
                        }
                    }
                }
            }
            let mut frontier = structural_probe
                .partial
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            frontier.sort_by(|left, right| rank_of(right.0).cmp(&rank_of(left.0)));
            frontier.dedup();
            for (next_g, next_t) in frontier.into_iter().take(32) {
                if candidate_pairs.insert((next_g, next_t)) {
                    candidate_queue.push(std::cmp::Reverse((
                        rank_of(next_g),
                        true,
                        next_g,
                        next_t,
                        true,
                    )));
                    added_partial_candidate = true;
                }
            }
            if added_partial_candidate {
                // The failed root remains unproved. Its independently proved
                // subgraph is queued first and may provide the missing cuts.
                continue;
            }
            let verified = verify_local_region(
                gold,
                gate,
                gn,
                tn,
                &accepted,
                &mut local_result_cache,
                cancellation,
            )?;
            if verified {
                accepted.insert(gn, tn);
                points.push((gn, tn, named_flag));
                propagate_fanout = true;
            } else {
                propagate_fanout = false;
            }
        }
        if propagate_fanout {
            for (next_g, next_t) in direct_fanout_candidates(gold, gate, gn, tn, &accepted)
                .into_iter()
                .take(8)
            {
                if candidate_pairs.insert((next_g, next_t)) {
                    candidate_queue.push(std::cmp::Reverse((
                        rank_of(next_g),
                        true,
                        next_g,
                        next_t,
                        true,
                    )));
                }
            }
        }
    }
    let verified_named = points.iter().filter(|(_, _, n)| !*n).count();
    Ok((points, verified_named))
}

/// Normalized functional class used by the structural fast path. Library
/// cells collapse to the same classes as generic primitives so that 1:1
/// technology mapping (AND2 for And, MUX2 for Mux, ...) is recognized as
/// isomorphic without SAT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FuncClass {
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

fn cell_class(nl: &Netlist, kind: &CellKind) -> Option<FuncClass> {
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

fn direct_pin(expr: &LibFunc, pin: usize) -> bool {
    matches!(expr, LibFunc::Id(i) if *i == pin)
}

fn direct_pins(exprs: &[LibFunc], input_count: usize) -> bool {
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
fn library_cell_class(func: &LibFunc, input_count: usize) -> Option<FuncClass> {
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

fn commutative(class: FuncClass) -> bool {
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

fn merge_pairs(
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

struct StructuralProbe {
    root: Option<Vec<(NetId, NetId)>>,
    /// Subgraphs that were independently proved even if the requested roots
    /// could not be connected structurally. They are candidates, never
    /// assumptions; induction still proves each root before acceptance.
    partial: Vec<Vec<(NetId, NetId)>>,
}

/// Return all structurally proved pairs, including the requested roots.
#[cfg(test)]
fn structural_matches(
    gold: &Netlist,
    gate: &Netlist,
    gn: NetId,
    tn: NetId,
    accepted: &HashMap<NetId, NetId>,
) -> Option<Vec<(NetId, NetId)>> {
    structural_probe(gold, gate, gn, tn, accepted).root
}

fn structural_probe(
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
fn region_isomorphic(
    gold: &Netlist,
    gate: &Netlist,
    gn: NetId,
    tn: NetId,
    accepted: &HashMap<NetId, NetId>,
) -> bool {
    structural_matches(gold, gate, gn, tn, accepted).is_some()
}

fn distinguished_result(what: String, model: HashMap<NetId, bool>, gold: &Netlist) -> EquivResult {
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

fn shift(l: i32, off: i32) -> i32 {
    if l > 0 {
        l + off
    } else {
        l - off
    }
}

#[cfg(test)]
mod tests {
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
            verify_vector_region(&gold, &gate, &pairs, &accepted, &FlowCancellation::new())
                .unwrap()
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
            verify_vector_region(&gold, &gate, &pairs, &accepted, &FlowCancellation::new())
                .unwrap()
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
        let cuts = std::collections::HashSet::from([a]);
        assert_eq!(bounded_region_len(&nl, y, &cuts, 128), 3);
        assert_eq!(bounded_region_len(&nl, y, &cuts, 1), usize::MAX);

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
}
