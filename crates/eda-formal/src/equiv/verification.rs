//! Shared simulation, local/vector CNF verification, and correspondence inference.

use super::structural::{cell_class, structural_probe, FuncClass};
use super::*;

type ValueTraces = Vec<Vec<bool>>;

/// Evaluate both netlists over K identical random PI/state rounds and
/// return per-net value traces (signature material shared by inference and
/// region verification).
#[allow(clippy::type_complexity)]
pub(super) fn simulate_shared_values(
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
#[allow(clippy::too_many_arguments)]
pub(super) fn verify_local_region(
    gold: &Netlist,
    gate: &Netlist,
    gn: NetId,
    tn: NetId,
    accepted: &HashMap<NetId, NetId>,
    gate_accepted: &NetIdSet,
    result_cache: &mut HashMap<Vec<i32>, bool>,
    cancellation: &FlowCancellation,
) -> Result<bool, String> {
    // Reject oversized proposals before CNF encoding. Induction regions are
    // normally small; a deep region means the accepted map has not reached
    // that area yet, and encoding every ambiguous proposal is too expensive.
    const MAX_REGION_NETS: usize = 128;
    let gold_is_cut = |net: &NetId| -> bool { *net != gn && accepted.contains_key(net) };
    let gate_is_cut = |net: &NetId| -> bool { *net != tn && gate_accepted.contains(net) };
    let gold_region_len = bounded_region_len(gold, gn, |net| gold_is_cut(net), MAX_REGION_NETS);
    let gate_region_len = bounded_region_len(gate, tn, |net| gate_is_cut(net), MAX_REGION_NETS);
    if gold_region_len == usize::MAX
        || gate_region_len == usize::MAX
        || gold_region_len + gate_region_len > MAX_REGION_NETS
    {
        return Ok(false);
    }
    let mut ge = NetlistEncoder::new(gold);
    let a = ge.enc_cone_with_cuts(gn, |net| gold_is_cut(net))?;
    let mut te = NetlistEncoder::new(gate);
    let b = te.enc_cone_with_cuts(tn, |net| gate_is_cut(net))?;
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
    for gin in ge.var.keys() {
        if *gin == gn {
            continue;
        }
        if let Some(&tin) = accepted.get(gin) {
            if tin != tn && te.has_lit(tin) {
                let gl = ge.var[gin];
                let tl = shift(te.var[&tin], offset);
                cnf.add_clause(vec![-gl, tl]);
                cnf.add_clause(vec![gl, -tl]);
            }
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
pub(super) fn canonical_cnf_key(cnf: &crate::sat::Cnf) -> Vec<i32> {
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

pub(super) fn vector_base_and_index(name: &str) -> Option<(&str, usize)> {
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
pub(super) fn verify_vector_region(
    gold: &Netlist,
    gate: &Netlist,
    pairs: &[(NetId, NetId)],
    accepted: &HashMap<NetId, NetId>,
    cancellation: &FlowCancellation,
) -> Result<bool, String> {
    verify_vector_region_with_cap(gold, gate, pairs, accepted, cancellation, 4_000)
}

pub(super) fn verify_vector_region_with_cap(
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
    let gold_sinks: NetIdSet = pairs.iter().map(|(gn, _)| *gn).collect();
    let gate_sinks: NetIdSet = pairs.iter().map(|(_, tn)| *tn).collect();
    let gold_cuts: NetIdSet = accepted
        .keys()
        .copied()
        .filter(|net| !gold_sinks.contains(net))
        .collect();
    let gate_cuts: NetIdSet = accepted
        .values()
        .copied()
        .filter(|net| !gate_sinks.contains(net))
        .collect();

    let mut ge = NetlistEncoder::new(gold);
    let mut gold_literals = Vec::with_capacity(pairs.len());
    for (gn, _) in pairs {
        gold_literals.push(ge.enc_cone_with_cuts(*gn, |net| gold_cuts.contains(net))?);
    }
    let mut te = NetlistEncoder::new(gate);
    let mut gate_literals = Vec::with_capacity(pairs.len());
    for (_, tn) in pairs {
        gate_literals.push(te.enc_cone_with_cuts(*tn, |net| gate_cuts.contains(net))?);
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
                    && gate_combined_lits.contains_key(net)
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
    if std::env::var("EDA_VEC_DUMP").is_ok() {
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

pub(super) fn bounded_region_len<F: Fn(&NetId) -> bool>(
    nl: &Netlist,
    sink: NetId,
    cuts: F,
    cap: usize,
) -> usize {
    let mut seen = NetIdSet::default();
    let mut stack = vec![sink];
    while let Some(n) = stack.pop() {
        if seen.len() > cap {
            return usize::MAX;
        }
        if !seen.insert(n) {
            continue;
        }
        if cuts(&n) {
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
pub(super) fn direct_fanout_candidates(
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
pub(super) fn explicit_constant(nl: &Netlist, n: NetId) -> Option<bool> {
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
pub(super) fn build_correspondence_points(
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
    let mut gate_accepted: NetIdSet = accepted.values().copied().collect();
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
                                gate_accepted.insert(*t);
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
                gate_accepted.insert(t);
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
                &gate_accepted,
                &mut local_result_cache,
                cancellation,
            )?;
            if verified {
                accepted.insert(gn, tn);
                gate_accepted.insert(tn);
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
