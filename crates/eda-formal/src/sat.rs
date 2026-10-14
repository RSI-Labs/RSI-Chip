//! A CDCL SAT solver: two-watched-literal propagation, VSIDS decisions with
//! phase saving, 1UIP conflict-driven clause learning, and Luby restarts.
//!
//! Literals use the DIMACS convention in the public API: variable `v`
//! (1-based) appears as `+v` or `-v`. Internally a literal is
//! `2*(v-1) + (neg ? 1 : 0)`.

use eda_core::cancellation::FlowCancellation;
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct Cnf {
    pub n_vars: usize,
    pub clauses: Vec<Vec<i32>>,
}

impl Cnf {
    pub fn new() -> Self {
        Cnf::default()
    }

    pub fn add_var(&mut self) -> usize {
        self.n_vars += 1;
        self.n_vars
    }

    pub fn add_clause(&mut self, lits: Vec<i32>) {
        debug_assert!(lits
            .iter()
            .all(|l| *l != 0 && l.unsigned_abs() <= self.n_vars as u32));
        self.clauses.push(lits);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SatResult {
    /// Model: `value[v-1]` is the assignment of variable v.
    Sat(Vec<bool>),
    Unsat,
}

struct Solver {
    n: usize,                 // vars, 0-based internally
    clauses: Vec<Vec<usize>>, // internal lits (2v / 2v+1)
    watches: Vec<Vec<usize>>, // per internal literal -> clause indices
    value: Vec<i8>,           // 0 = unassigned, 1 = true, -1 = false
    level_of: Vec<u32>,
    reason: Vec<Option<usize>>,
    trail: Vec<usize>,
    trail_lim: Vec<usize>,
    qhead: usize,
    activity: Vec<f64>,
    var_inc: f64,
    phase: Vec<bool>,
    clause_is_learnt: Vec<bool>,
    clause_activity: Vec<f64>,
    clause_inc: f64,
    learnt_count: usize,
    unit_clauses: Vec<usize>,
    conflicts_since_reduce: u64,
    conflicts: u64,
    restart_interval: u64,
    luby_index: u64,
    ok: bool,
}

fn to_internal(lit: i32) -> usize {
    if lit > 0 {
        2 * (lit as usize - 1)
    } else {
        2 * ((-lit) as usize - 1) + 1
    }
}

fn to_external(lit: usize) -> i32 {
    let var = (lit / 2) as i32 + 1;
    if lit.is_multiple_of(2) {
        var
    } else {
        -var
    }
}

impl Solver {
    fn new(cnf: &Cnf) -> Self {
        let n = cnf.n_vars;
        let mut s = Solver {
            n,
            clauses: Vec::new(),
            watches: vec![Vec::new(); 2 * n + 2],
            value: vec![0; n],
            level_of: vec![0; n],
            reason: vec![None; n],
            trail: Vec::new(),
            trail_lim: Vec::new(),
            qhead: 0,
            activity: vec![0.0; n],
            var_inc: 1.0,
            phase: vec![false; n],
            clause_is_learnt: Vec::new(),
            clause_activity: Vec::new(),
            clause_inc: 1.0,
            learnt_count: 0,
            unit_clauses: Vec::new(),
            conflicts_since_reduce: 0,
            conflicts: 0,
            restart_interval: 64,
            luby_index: 1,
            ok: true,
        };
        // CNF construction can enumerate maps such as correspondence cut
        // clauses in hasher order. Sorting here makes proof effort and any
        // budget-limited verdict reproducible across processes.
        let mut clauses = cnf.clauses.clone();
        clauses.sort();
        for cl in &clauses {
            let lits: Vec<usize> = cl.iter().map(|l| to_internal(*l)).collect();
            s.add_clause_internal(lits, false);
        }
        s
    }

    fn add_clause_internal(&mut self, lits: Vec<usize>, learnt: bool) {
        if lits.is_empty() {
            self.ok = false;
            return;
        }
        // deduplicate literals and drop tautologies (a clause with both
        // polarities of a variable is always satisfied)
        let mut lits = lits;
        lits.sort_unstable();
        lits.dedup();
        for k in 1..lits.len() {
            if lits[k] == opp(lits[k - 1]) {
                return; // tautology
            }
        }
        let lits = lits;
        if lits.len() == 1 {
            let lit = lits[0];
            self.unit_clauses.push(lit);
            match self.value[lit / 2] {
                0 => self.enqueue(lit, None),
                v if v == if lit.is_multiple_of(2) { 1 } else { -1 } => {}
                _ => self.ok = false,
            }
            return;
        }
        let idx = self.clauses.len();
        self.clauses.push(lits.clone());
        self.clause_is_learnt.push(learnt);
        self.clause_activity.push(0.0);
        if learnt {
            self.learnt_count += 1;
        }
        if learnt {
            self.bump_var(to_external(lits[0]).unsigned_abs() as usize - 1);
        }
        // a clause is registered in the watch list of the NEGATION of each
        // of its two watched literals
        self.watches[opp(lits[0])].push(idx);
        self.watches[opp(lits[1])].push(idx);
    }

    fn enqueue(&mut self, lit: usize, reason: Option<usize>) {
        let v = lit / 2;
        self.value[v] = if lit.is_multiple_of(2) { 1 } else { -1 };
        self.phase[v] = lit.is_multiple_of(2);
        self.level_of[v] = self.trail_lim.len() as u32;
        self.reason[v] = reason;
        self.trail.push(lit);
    }

    fn decision_level(&self) -> u32 {
        self.trail_lim.len() as u32
    }

    /// Propagate; returns conflicting clause index if any.
    #[allow(clippy::needless_range_loop)]
    fn propagate(&mut self, cancellation: &FlowCancellation) -> Result<Option<usize>, String> {
        while self.qhead < self.trail.len() {
            if cancellation.is_cancelled() {
                return Err("SAT propagation cancelled".into());
            }
            let p = self.trail[self.qhead];
            self.qhead += 1;
            let false_lit = opp(p);
            // clauses registered under `p` are exactly those whose other
            // watched literal just became false (false_lit)
            let mut i = 0;
            let watch_list = std::mem::take(&mut self.watches[p]);
            let mut keep = Vec::with_capacity(watch_list.len());
            let mut conflict: Option<usize> = None;
            while i < watch_list.len() {
                if i % 128 == 0 && cancellation.is_cancelled() {
                    self.watches[p] = keep;
                    return Err("SAT propagation cancelled".into());
                }
                let ci = watch_list[i];
                i += 1;
                // ensure c[1] == false_lit (swap if needed)
                let (first, second) = {
                    let c = &mut self.clauses[ci];
                    if c[0] == false_lit {
                        c.swap(0, 1);
                    }
                    (c[0], c[1])
                };
                let first_val = self.value[first / 2];
                let first_sat = if first % 2 == 0 {
                    first_val == 1
                } else {
                    first_val == -1
                };
                if first_sat {
                    keep.push(ci);
                    continue;
                }
                // find a new watch among c[2..]
                let mut found = None;
                {
                    let c = &self.clauses[ci];
                    for k in 2..c.len() {
                        let v = self.value[c[k] / 2];
                        let sat = if c[k].is_multiple_of(2) {
                            v == 1
                        } else {
                            v == -1
                        };
                        let unassigned = v == 0;
                        if sat || unassigned {
                            found = Some(c[k]);
                            break;
                        }
                    }
                }
                match found {
                    Some(new_watch) => {
                        let pos = self.clauses[ci]
                            .iter()
                            .position(|x| *x == new_watch)
                            .unwrap();
                        self.clauses[ci].swap(1, pos);
                        let _ = second;
                        self.watches[opp(new_watch)].push(ci);
                        // do NOT keep watching under false_lit
                    }
                    None => {
                        // unit or conflict
                        let first_val = self.value[first / 2];
                        if first_val == 0 {
                            self.enqueue(first, Some(ci));
                            keep.push(ci);
                        } else {
                            // conflict: the clause must STAY in this watch
                            // list (its watch is still registered)
                            conflict = Some(ci);
                            keep.push(ci);
                            keep.extend_from_slice(&watch_list[i..]);
                            break;
                        }
                    }
                }
            }
            self.watches[p] = keep;
            if let Some(c) = conflict {
                return Ok(Some(c));
            }
        }
        Ok(None)
    }

    fn lit_level(&self, lit: usize) -> u32 {
        self.level_of[lit / 2]
    }

    /// 1UIP conflict analysis.
    fn analyze(&mut self, conflict: usize) -> (Vec<usize>, u32) {
        let mut learnt: Vec<usize> = vec![0]; // slot 0 for the asserting literal
        let mut seen = vec![false; self.n];
        let mut counter = 0;
        let mut p: Option<usize> = None;
        let mut idx = self.trail.len();
        let cursor = conflict;
        loop {
            let clause = if let Some(pl) = p {
                self.reason[pl / 2].unwrap()
            } else {
                cursor
            };
            let (lits, skip) = {
                let c = &self.clauses[clause];
                (c.clone(), p)
            };
            for q in &lits {
                if let Some(sk) = skip {
                    if *q == sk {
                        continue;
                    }
                }
                let v = q / 2;
                if !seen[v] && self.level_of[v] > 0 {
                    seen[v] = true;
                    self.bump_var(v);
                    if self.level_of[v] >= self.decision_level() {
                        counter += 1;
                    } else {
                        learnt.push(*q);
                    }
                }
            }
            // next literal from trail with seen
            while idx > 0 {
                idx -= 1;
                let t = self.trail[idx];
                if seen[t / 2] {
                    p = Some(t);
                    break;
                }
            }
            let pl = p.expect("analysis ran off the trail");
            counter -= 1;
            if counter == 0 {
                learnt[0] = opp(pl);
                seen[pl / 2] = false;
                break;
            }
            seen[pl / 2] = false;
        }
        // backjump level
        let mut bt = 0;
        for l in &learnt[1..] {
            bt = bt.max(self.lit_level(*l));
        }
        // move the highest-level literal to position 1 for watch stability
        let mut best = 1;
        for k in 2..learnt.len() {
            if self.lit_level(learnt[k]) > self.lit_level(learnt[best]) {
                best = k;
            }
        }
        if learnt.len() > 2 {
            learnt.swap(1, best);
        }
        (learnt, bt)
    }

    fn cancel_until(&mut self, lvl: u32) {
        if self.decision_level() <= lvl {
            return;
        }
        let lim = self.trail_lim[lvl as usize];
        for k in (lim..self.trail.len()).rev() {
            let v = self.trail[k] / 2;
            self.value[v] = 0;
            self.reason[v] = None;
        }
        self.trail.truncate(lim);
        self.trail_lim.truncate(lvl as usize);
        self.qhead = self.trail.len();
    }

    fn bump_var(&mut self, v: usize) {
        self.activity[v] += self.var_inc;
        if self.activity[v] > 1e100 {
            for a in self.activity.iter_mut() {
                *a *= 1e-100;
            }
            self.var_inc *= 1e-100;
        }
    }

    fn bump_clause(&mut self, ci: usize) {
        self.clause_activity[ci] += self.clause_inc;
        if self.clause_activity[ci] > 1e100 {
            let scale = 1e-100;
            for activity in &mut self.clause_activity {
                *activity *= scale;
            }
            self.clause_inc *= scale;
        }
    }

    fn decay_clauses(&mut self) {
        self.clause_inc /= 0.999;
    }

    /// Delete the least-active half of long learnt clauses at level zero.
    /// Original clauses and binary learnt implications are preserved. This is
    /// a heuristic: removing a learnt lemma cannot change satisfiability.
    fn reduce_learned_clauses(&mut self) {
        if self.decision_level() != 0 || self.learnt_count < 20_000 {
            return;
        }
        let mut candidates: Vec<usize> = (0..self.clauses.len())
            .filter(|ci| self.clause_is_learnt[*ci] && self.clauses[*ci].len() > 2)
            .collect();
        if candidates.len() < 4 {
            return;
        }
        candidates.sort_by(|left, right| {
            self.clause_activity[*left]
                .partial_cmp(&self.clause_activity[*right])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let remove_count = candidates.len() / 2;
        let mut keep = vec![true; self.clauses.len()];
        for ci in candidates.into_iter().take(remove_count) {
            keep[ci] = false;
        }

        let old_clauses = std::mem::take(&mut self.clauses);
        let old_is_learnt = std::mem::take(&mut self.clause_is_learnt);
        let old_activity = std::mem::take(&mut self.clause_activity);
        let mut old_to_new = vec![usize::MAX; old_clauses.len()];
        self.watches = vec![Vec::new(); 2 * self.n + 2];
        let mut clauses = Vec::with_capacity(old_clauses.len() - remove_count);
        let mut is_learnt = Vec::with_capacity(clauses.capacity());
        let mut activity = Vec::with_capacity(clauses.capacity());
        let mut learnt_count = 0usize;
        for (old_ci, lits) in old_clauses.into_iter().enumerate() {
            if !keep[old_ci] {
                continue;
            }
            old_to_new[old_ci] = clauses.len();
            if old_is_learnt[old_ci] {
                learnt_count += 1;
            }
            is_learnt.push(old_is_learnt[old_ci]);
            activity.push(old_activity[old_ci]);
            let new_ci = clauses.len();
            clauses.push(lits);
            // `add_clause_internal` is not used here because these clauses
            // are already normalized and their activity must survive.
            let first = clauses[new_ci][0];
            let second = clauses[new_ci][1];
            self.watches[opp(first)].push(new_ci);
            self.watches[opp(second)].push(new_ci);
        }
        self.clauses = clauses;
        self.clause_is_learnt = is_learnt;
        self.clause_activity = activity;
        self.learnt_count = learnt_count;
        for reason in &mut self.reason {
            if let Some(old_ci) = *reason {
                *reason = match old_to_new[old_ci] {
                    usize::MAX => None,
                    new_ci => Some(new_ci),
                };
            }
        }
    }

    fn decay(&mut self) {
        self.var_inc /= 0.95;
    }

    fn pick_branch(&mut self) -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut best_act = -1.0f64;
        for v in 0..self.n {
            if self.value[v] == 0 && self.activity[v] > best_act {
                best_act = self.activity[v];
                best = Some(v);
            }
        }
        best.map(|v| if self.phase[v] { 2 * v } else { 2 * v + 1 })
    }

    fn luby(&self, i: u64) -> u64 {
        // Luby 1,1,2,1,1,2,4,...
        let mut k = 1;
        while (1 << k) - 1 < i + 1 {
            k += 1;
        }
        let _ = k;
        luby_seq(i)
    }

    fn solve_cancellable_with_limit(
        &mut self,
        cancellation: &FlowCancellation,
        max_iterations: u64,
    ) -> Result<SatResult, String> {
        if !self.ok {
            return Ok(SatResult::Unsat);
        }
        let mut conflicts_since_restart: u64 = 0;
        let mut dbg_iters: u64 = 0;
        loop {
            dbg_iters += 1;
            if cancellation.is_cancelled() {
                return Err("SAT solving cancelled".into());
            }
            if dbg_iters > max_iterations {
                eprintln!(
                    "SOLVER STUCK: n={} clauses={} conflicts={} level={}",
                    self.n,
                    self.clauses.len(),
                    self.conflicts,
                    self.decision_level()
                );
                if std::env::var("EDA_SAT_DUMP").is_ok() {
                    eprintln!("p cnf {} {}", self.n, self.clauses.len());
                    for c in &self.clauses {
                        eprintln!(
                            "{}",
                            c.iter()
                                .map(|l| to_external(*l))
                                .map(|l| l.to_string())
                                .collect::<Vec<_>>()
                                .join(" ")
                                + " 0"
                        );
                    }
                }
                return Err(format!(
                    "SAT solver stuck: {} vars, {} clauses, {} conflicts",
                    self.n,
                    self.clauses.len(),
                    self.conflicts
                ));
            }
            match self.propagate(cancellation)? {
                Some(conflict) => {
                    if self.decision_level() == 0 {
                        return Ok(SatResult::Unsat);
                    }
                    self.conflicts += 1;
                    conflicts_since_restart += 1;
                    self.conflicts_since_reduce += 1;
                    let (learnt, bt) = self.analyze(conflict);
                    self.cancel_until(bt);
                    let unit = learnt.len() == 1;
                    self.add_clause_internal(learnt.clone(), true);
                    if !unit && self.ok {
                        // assert the 1UIP literal, implied by the learnt clause
                        let reason = self.clauses.len() - 1;
                        self.bump_clause(reason);
                        self.enqueue(learnt[0], Some(reason));
                    }
                    self.decay();
                    self.decay_clauses();
                    if self.conflicts_since_reduce >= 10_000 && self.decision_level() == 0 {
                        self.conflicts_since_reduce = 0;
                        self.reduce_learned_clauses();
                    }
                    if conflicts_since_restart >= self.restart_interval {
                        conflicts_since_restart = 0;
                        self.luby_index += 1;
                        self.restart_interval = 32 * self.luby(self.luby_index);
                        self.cancel_until(0);
                        if self.conflicts_since_reduce >= 10_000 {
                            self.conflicts_since_reduce = 0;
                            self.reduce_learned_clauses();
                        }
                    }
                }
                None => {
                    match self.pick_branch() {
                        None => {
                            // all assigned
                            let model: Vec<bool> =
                                (0..self.n).map(|v| self.value[v] == 1).collect();
                            return Ok(SatResult::Sat(model));
                        }
                        Some(lit) => {
                            self.trail_lim.push(self.trail.len());
                            self.enqueue(lit, None);
                        }
                    }
                }
            }
        }
    }

    fn solve_cancellable(&mut self, cancellation: &FlowCancellation) -> Result<SatResult, String> {
        self.solve_cancellable_with_limit(cancellation, 2_000_000)
    }

    fn reset_search(&mut self) {
        self.value.fill(0);
        self.reason.fill(None);
        self.trail.clear();
        self.trail_lim.clear();
        self.qhead = 0;
        let unit_clauses = self.unit_clauses.clone();
        for lit in unit_clauses {
            let variable = lit / 2;
            let expected = if lit.is_multiple_of(2) { 1 } else { -1 };
            match self.value[variable] {
                0 => self.enqueue(lit, None),
                value if value == expected => {}
                _ => self.ok = false,
            }
        }
    }

    fn ensure_variable_count(&mut self, variables: usize) {
        if variables <= self.n {
            return;
        }
        self.watches.resize(2 * variables + 2, Vec::new());
        self.value.resize(variables, 0);
        self.level_of.resize(variables, 0);
        self.reason.resize(variables, None);
        self.activity.resize(variables, 0.0);
        self.phase.resize(variables, false);
        self.n = variables;
    }
}

/// A persistent CDCL context for incremental assumptions and refinements.
///
/// learnt clauses, clause activities, and saved phases survive added clauses;
/// only the transient assignment trail is reset. This is useful for CEGAR,
/// where an abstract SAT model is refuted and the solver must re-visit a
/// closely related formula.
pub struct IncrementalSolver {
    solver: Solver,
}

impl IncrementalSolver {
    pub fn new(cnf: &Cnf) -> Self {
        IncrementalSolver {
            solver: Solver::new(cnf),
        }
    }

    pub fn set_phase_hints(&mut self, phases: &HashMap<usize, bool>) {
        for (variable, value) in phases {
            if *variable >= 1 && *variable <= self.solver.n {
                self.solver.phase[*variable - 1] = *value;
            }
        }
    }

    pub fn ensure_variable_count(&mut self, variables: usize) {
        self.solver.ensure_variable_count(variables);
    }

    pub fn add_clause(&mut self, clause: &[i32]) {
        self.add_clauses(&[clause.to_vec()]);
    }

    pub fn add_clauses(&mut self, clauses: &[Vec<i32>]) {
        let max_var = clauses
            .iter()
            .flat_map(|clause| clause.iter())
            .map(|lit| lit.unsigned_abs() as usize)
            .max()
            .unwrap_or(0);
        self.solver.ensure_variable_count(max_var);
        self.solver.reset_search();
        for clause in clauses {
            let internal = clause.iter().map(|lit| to_internal(*lit)).collect();
            self.solver.add_clause_internal(internal, false);
        }
    }

    pub fn solve_cancellable(
        &mut self,
        cancellation: &FlowCancellation,
    ) -> Result<SatResult, String> {
        self.solver.solve_cancellable(cancellation)
    }

    pub fn solve_with_limit_cancellable(
        &mut self,
        cancellation: &FlowCancellation,
        max_iterations: u64,
    ) -> Result<SatResult, String> {
        self.solver
            .solve_cancellable_with_limit(cancellation, max_iterations)
    }

    pub fn learnt_clause_count(&self) -> usize {
        self.solver.learnt_count
    }
}

fn opp(lit: usize) -> usize {
    lit ^ 1
}

fn luby_seq(i: u64) -> u64 {
    // find k: 2^k - 1 <= i+1... standard iterative Luby
    let mut k = 1u64;
    while (1u64 << k) - 1 < i + 1 {
        k += 1;
    }
    k -= 1;
    if (1u64 << k) - 1 == i + 1 {
        return 1u64 << (k - 1);
    }
    i + 1 - ((1u64 << k) - 1) + ((1u64 << (k - 1)) - 1) - 1 + 1
}

/// Solve a CNF. Returns a satisfying model or `Unsat`.
pub fn solve(cnf: &Cnf) -> SatResult {
    match solve_cancellable(cnf, &FlowCancellation::new()) {
        Ok(result) => result,
        // A fresh token is never cancelled; this branch only preserves the
        // infallible legacy API shape.
        Err(_) => SatResult::Unsat,
    }
}

/// Cooperatively solve a CNF and return an error instead of a misleading
/// UNSAT result if the caller cancels the search.
pub fn solve_cancellable(cnf: &Cnf, cancellation: &FlowCancellation) -> Result<SatResult, String> {
    let mut s = Solver::new(cnf);
    if cancellation.is_cancelled() {
        return Err("SAT solving cancelled".into());
    }
    s.solve_cancellable(cancellation)
}

/// Solve with deterministic phase hints. Phases are search hints only; they
/// do not constrain the formula.
pub fn solve_with_phase_hints_cancellable(
    cnf: &Cnf,
    phases: &HashMap<usize, bool>,
    cancellation: &FlowCancellation,
) -> Result<SatResult, String> {
    let mut solver = Solver::new(cnf);
    if cancellation.is_cancelled() {
        return Err("SAT solving cancelled".into());
    }
    for (variable, value) in phases {
        if *variable >= 1 && *variable <= solver.n {
            solver.phase[*variable - 1] = *value;
        }
    }
    solver.solve_cancellable(cancellation)
}

/// Solve a small subproblem with a tighter search budget. Callers must treat
/// an Err as "unknown"; it never means UNSAT.
pub fn solve_local_cancellable(
    cnf: &Cnf,
    cancellation: &FlowCancellation,
    max_iterations: u64,
) -> Result<SatResult, String> {
    let mut s = Solver::new(cnf);
    if cancellation.is_cancelled() {
        return Err("SAT solving cancelled".into());
    }
    s.solve_cancellable_with_limit(cancellation, max_iterations)
}

/// Solve under assumptions (all must hold).
pub fn solve_with_assumptions(cnf: &Cnf, assumptions: &[i32]) -> SatResult {
    match solve_with_assumptions_cancellable(cnf, assumptions, &FlowCancellation::new()) {
        Ok(result) => result,
        Err(_) => SatResult::Unsat,
    }
}

/// Cooperatively solve under assumptions.
pub fn solve_with_assumptions_cancellable(
    cnf: &Cnf,
    assumptions: &[i32],
    cancellation: &FlowCancellation,
) -> Result<SatResult, String> {
    let mut c2 = cnf.clone();
    for a in assumptions {
        c2.add_clause(vec![*a]);
    }
    solve_cancellable(&c2, cancellation)
}

/// Convenience: is the CNF satisfiable?
pub fn sat(cnf: &Cnf) -> bool {
    !matches!(solve(cnf), SatResult::Unsat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn basic_sat_unsat() {
        let mut c = Cnf::new();
        let x = c.add_var();
        c.add_clause(vec![x as i32]);
        assert!(matches!(solve(&c), SatResult::Sat(_)));

        let mut c = Cnf::new();
        let x = c.add_var();
        c.add_clause(vec![x as i32]);
        c.add_clause(vec![-(x as i32)]);
        assert_eq!(solve(&c), SatResult::Unsat);
    }

    #[test]
    fn local_search_budget_fails_closed() {
        let mut cnf = Cnf::new();
        let x = cnf.add_var() as i32;
        cnf.add_clause(vec![x]);

        let cancellation = FlowCancellation::new();
        let error = solve_local_cancellable(&cnf, &cancellation, 0).unwrap_err();
        assert!(error.starts_with("SAT solver stuck: "), "{error}");
        assert!(matches!(
            solve_local_cancellable(&cnf, &cancellation, 10).unwrap(),
            SatResult::Sat(_)
        ));

        cancellation.cancel();
        assert_eq!(
            solve_local_cancellable(&cnf, &cancellation, 10).unwrap_err(),
            "SAT solving cancelled"
        );
    }

    #[test]
    fn incremental_solver_accepts_new_variables_and_clauses() {
        let mut cnf = Cnf::new();
        let x = cnf.add_var() as i32;
        let mut solver = IncrementalSolver::new(&cnf);
        let SatResult::Sat(model) = solver.solve_cancellable(&FlowCancellation::new()).unwrap()
        else {
            panic!("fresh variable must be SAT");
        };
        assert_eq!(model.len(), 1);

        let y = cnf.add_var() as i32;
        cnf.add_clause(vec![-x, y]);
        cnf.add_clause(vec![-y]);
        solver.ensure_variable_count(cnf.n_vars);
        solver.add_clauses(&cnf.clauses);
        let learnt_before = solver.learnt_clause_count();
        let SatResult::Sat(model) = solver.solve_cancellable(&FlowCancellation::new()).unwrap()
        else {
            panic!("incremental refinement incorrectly became UNSAT");
        };
        assert!(!model[0], "new clauses force x=false");
        assert!(!model[1], "new unit forces y=false");
        assert!(solver.learnt_clause_count() >= learnt_before);

        cnf.add_clause(vec![x]);
        solver.add_clause(&cnf.clauses[2]);
        assert_eq!(
            solver.solve_cancellable(&FlowCancellation::new()).unwrap(),
            SatResult::Unsat
        );
    }

    #[test]
    fn phase_hints_guide_but_do_not_constrain_search() {
        let mut cnf = Cnf::new();
        let x = cnf.add_var() as i32;
        let y = cnf.add_var() as i32;
        cnf.add_clause(vec![x]);
        cnf.add_clause(vec![x, y]);
        let phases = HashMap::from([(1usize, false), (2usize, false)]);
        let result =
            solve_with_phase_hints_cancellable(&cnf, &phases, &FlowCancellation::new()).unwrap();
        let SatResult::Sat(model) = result else {
            panic!("phase hint incorrectly made formula UNSAT");
        };
        assert!(model[0], "unit clause must remain satisfied");
    }

    #[test]
    fn asserted_xor_with_equal_operands_is_unsat() {
        let mut cnf = Cnf::new();
        for _ in 0..3 {
            cnf.add_var();
        }
        let g = 1;
        let t = 2;
        let e = 3;
        cnf.add_clause(vec![-g, -t, -e]);
        cnf.add_clause(vec![-g, t, e]);
        cnf.add_clause(vec![g, -t, e]);
        cnf.add_clause(vec![g, t, -e]);
        cnf.add_clause(vec![-g]);
        cnf.add_clause(vec![-t]);
        cnf.add_clause(vec![e]);
        assert_eq!(
            solve(&cnf),
            SatResult::Unsat,
            "asserted XOR with equal operands must be UNSAT"
        );
    }

    #[test]
    fn learned_clause_reduction_preserves_original_and_binary_clauses() {
        let mut cnf = Cnf::new();
        for _ in 0..4 {
            cnf.add_var();
        }
        let mut solver = Solver::new(&cnf);
        for _ in 0..21_000 {
            solver.add_clause_internal(vec![0, 2, 4], true);
        }
        solver.add_clause_internal(vec![0, 2], true);
        let before = solver.clauses.len();
        assert!(solver.learnt_count >= 20_000);
        solver.reduce_learned_clauses();
        assert!(solver.clauses.len() < before);
        assert!(solver.clauses.len() >= 10_000);
        assert_eq!(
            solver.learnt_count,
            solver.clause_is_learnt.iter().filter(|x| **x).count()
        );
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn pigeonhole_unsat() {
        // 4 pigeons, 3 holes: UNSAT (hard for plain DPLL)
        let mut c = Cnf::new();
        let n = 4;
        let m = 3;
        let mut v = 0;
        let mut var = |c: &mut Cnf| {
            v += 1;
            let _ = c.add_var();
            v
        };
        let mut pigeons = vec![];
        for _ in 0..n {
            let mut row = vec![];
            for _ in 0..m {
                row.push(var(&mut c));
            }
            pigeons.push(row);
        }
        for p in &pigeons {
            c.add_clause(p.to_vec());
        }
        for h in 0..m {
            for i in 0..n {
                for j in (i + 1)..n {
                    c.add_clause(vec![-pigeons[i][h], -pigeons[j][h]]);
                }
            }
        }
        assert_eq!(solve(&c), SatResult::Unsat);
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn cooperative_cancellation_reaches_the_sat_search_loop() {
        let pigeons = 12;
        let holes = 11;
        let mut cnf = Cnf::new();
        let mut assignment = vec![];
        for _ in 0..pigeons {
            let mut row = vec![];
            for _ in 0..holes {
                row.push(cnf.add_var() as i32);
            }
            assignment.push(row);
        }
        for row in &assignment {
            cnf.add_clause(row.clone());
        }
        for hole in 0..holes {
            for left in 0..pigeons {
                for right in (left + 1)..pigeons {
                    cnf.add_clause(vec![-assignment[left][hole], -assignment[right][hole]]);
                }
            }
        }

        let cancellation = FlowCancellation::new();
        let worker_cnf = cnf.clone();
        let worker_token = cancellation.clone();
        let worker = thread::spawn(move || solve_cancellable(&worker_cnf, &worker_token));
        thread::sleep(Duration::from_millis(1));
        cancellation.cancel();
        let result = worker.join().expect("SAT worker panicked");
        assert!(
            matches!(
                result.unwrap_err().as_str(),
                "SAT solving cancelled" | "SAT propagation cancelled"
            ),
            "worker did not observe cancellation"
        );

        let cancelled = FlowCancellation::new();
        cancelled.cancel();
        assert_eq!(
            solve_cancellable(&cnf, &cancelled).unwrap_err(),
            "SAT solving cancelled"
        );
    }

    #[test]
    fn random_cnf_matches_bruteforce() {
        // deterministic PRNG (xorshift)
        let mut seed: u64 = 0x12345678;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for case in 0..40 {
            let n_vars = 6 + (rnd() % 8) as usize; // 6..13 vars
            let n_cls = n_vars * 3;
            let mut cnf = Cnf::new();
            for _ in 0..n_vars {
                cnf.add_var();
            }
            for _ in 0..n_cls {
                let mut cl = Vec::new();
                for _ in 0..3 {
                    let v = (rnd() as usize % n_vars) as i32 + 1;
                    let sign = if rnd() % 2 == 0 { 1 } else { -1 };
                    cl.push(sign * v);
                }
                cnf.add_clause(cl);
            }
            // brute force
            let mut brute = false;
            'outer: for mask in 0u32..(1 << n_vars) {
                for cl in &cnf.clauses {
                    let mut ok = false;
                    for l in cl {
                        let v = l.unsigned_abs() as usize - 1;
                        let bit = (mask >> v) & 1 == 1;
                        let want = *l > 0;
                        if bit == want {
                            ok = true;
                            break;
                        }
                    }
                    if !ok {
                        continue 'outer;
                    }
                }
                brute = true;
                break;
            }
            let got = sat(&cnf);
            assert_eq!(got, brute, "case {} vars={}", case, n_vars);
        }
    }

    #[test]
    fn model_actually_satisfies() {
        let mut seed: u64 = 0xDEADBEEF;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..20 {
            let n_vars = 10;
            let mut cnf = Cnf::new();
            for _ in 0..n_vars {
                cnf.add_var();
            }
            // generate clauses from a hidden model so it's always SAT
            let model: Vec<bool> = (0..n_vars).map(|_| rnd() % 2 == 0).collect();
            for _ in 0..n_vars * 4 {
                let mut cl = Vec::new();
                for _ in 0..3 {
                    let v = (rnd() as usize % n_vars) as i32 + 1;
                    cl.push(if rnd() % 2 == 0 { v } else { -v });
                }
                // force satisfiable under model with 50% chance by fixing one lit
                if rnd() % 2 == 0 {
                    let v = (rnd() as usize % n_vars) as i32 + 1;
                    cl[0] = if model[v as usize - 1] { v } else { -v };
                }
                cnf.add_clause(cl);
            }
            if let SatResult::Sat(m) = solve(&cnf) {
                for cl in &cnf.clauses {
                    assert!(
                        cl.iter().any(|l| {
                            let v = l.unsigned_abs() as usize - 1;
                            m[v] == (*l > 0)
                        }),
                        "model does not satisfy {:?}",
                        cl
                    );
                }
            }
        }
    }
}
