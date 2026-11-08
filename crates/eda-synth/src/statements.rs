//! Procedural-statement lowering and sequential-cell construction.

use super::Synth;
use crate::sorted_set;
use eda_core::ir::{Expr, LValue, Range, Stmt, UnOp};
use eda_core::logic::LogicValue;
use eda_core::netlist::{CellKind, NetId};
use std::collections::HashSet;

impl<'a> Synth<'a> {
    pub(crate) fn hook_assign(&mut self, lv: &LValue, val: &[NetId]) -> Result<(), String> {
        let bits: Vec<NetId> = lv_bits(lv)
            .iter()
            .map(|f| self.flat2net_of(*f))
            .collect::<Result<_, _>>()?;
        for (i, b) in bits.iter().enumerate() {
            self.declare_driver(*b, val[i])?;
        }
        self.sym_set(lv, val);
        Ok(())
    }

    fn sym_set(&mut self, lv: &LValue, val: &[NetId]) {
        // `lv.net` may point inside a vector declaration (slang part-select
        // lvalues use the bit offset as the net id). Resolve the declaration
        // base so the symbolic vector has the full declaration width.
        let (base, w, offset) = self
            .design
            .net_decls
            .iter()
            .find(|decl| lv.net >= decl.base && lv.net < decl.base + decl.width)
            .map(|decl| (decl.base, decl.width, lv.net - decl.base))
            .unwrap_or((lv.net, 1, 0));
        if !self.sym.contains_key(&base) {
            let nets: Vec<NetId> = (0..w).map(|i| self.flat2net[base + i]).collect();
            self.sym.insert(base, nets);
        }
        let mut cur = self.sym[&base].clone();
        let (lo, n) = match &lv.range {
            Range::Bit(b) => (*b, 1),
            Range::Span { msb, lsb } => (*lsb, msb - lsb + 1),
        };
        let lo = lo + offset;
        if lo + n > cur.len() {
            return;
        }
        for (index, slot) in cur[lo..lo + n].iter_mut().enumerate() {
            if let Some(value) = val.get(index) {
                *slot = *value;
            }
        }
        self.sym.insert(base, cur);
        self.written.insert(base);
    }

    fn base_nets(&mut self, base: usize) -> Vec<NetId> {
        let w = self.decl_width(base);
        (0..w).map(|i| self.flat2net[base + i]).collect()
    }

    pub(crate) fn exec_stmt(&mut self, s: &Stmt) -> Result<(), String> {
        match s {
            Stmt::Block(v) => {
                for x in v {
                    self.exec_stmt(x)?;
                }
                Ok(())
            }
            Stmt::Forever(_) => Err("forever is not synthesizable".into()),
            Stmt::Wait { .. } => Err("wait statements are not synthesizable".into()),
            Stmt::EventWait { .. } => Err("event waits are not synthesizable".into()),
            Stmt::WaitOrder { .. } => Err("wait_order is not synthesizable".into()),
            Stmt::Fork { .. } => Err("fork/join blocks are not synthesizable".into()),
            Stmt::While { .. } => Err("while loops are not synthesizable".into()),
            Stmt::Repeat { .. } => Err("repeat loops are not synthesizable".into()),
            Stmt::Force { .. } => Err("force statements are not synthesizable".into()),
            Stmt::DisableFork => Err("disable fork is not synthesizable".into()),
            Stmt::WaitFork => Err("wait fork is not synthesizable".into()),
            Stmt::NamedBlock { body, .. } => self.exec_stmt(body),
            Stmt::NamedBlockEnd => Ok(()),
            Stmt::DisableNamedBlock { .. } => {
                Err("named-block disable is not synthesizable".into())
            }
            Stmt::ProcessKill { .. } => Err("process kill is not synthesizable".into()),
            Stmt::ProcessAwait { .. } => Err("process await is not synthesizable".into()),
            Stmt::ProcessSuspend { .. } => Err("process suspend is not synthesizable".into()),
            Stmt::ProcessResume { .. } => Err("process resume is not synthesizable".into()),
            Stmt::TimeFormat { .. } => Err("time formatting is not synthesizable".into()),
            Stmt::Release { .. } => Err("release statements are not synthesizable".into()),
            Stmt::ProceduralAssign { .. } => {
                Err("procedural continuous assignments are not synthesizable".into())
            }
            Stmt::ProceduralDeassign { .. } => {
                Err("procedural deassignments are not synthesizable".into())
            }
            Stmt::Break | Stmt::Continue => Err("loop jumps are not synthesizable".into()),
            Stmt::ReturnBarrier | Stmt::Return => {
                Err("procedure return is not synthesizable".into())
            }
            Stmt::Scope(body) => self.exec_stmt(body),
            Stmt::Bl { lv, e } | Stmt::NBl { lv, e } => {
                let w = lv.range.width();
                let v = self.blast(e, w)?;
                self.sym_set(lv, &v);
                Ok(())
            }
            Stmt::NBlIndexed {
                net,
                decl_width,
                base_offset,
                element_width,
                start,
                width,
                ascending,
                reversed,
                e,
            }
            | Stmt::BlIndexed {
                net,
                decl_width,
                base_offset,
                element_width,
                start,
                width,
                ascending,
                reversed,
                e,
            } => {
                let existing = self.sym.get(net).cloned();
                let mut current = match existing {
                    Some(current) => current,
                    None => self.base_nets(*net),
                };
                if current.len() != *decl_width {
                    current = self.base_nets(*net);
                }
                let value = self.blast(e, *width)?;
                let start_bits = self.blast(start, start.width())?;
                let mut result = current.clone();
                let whole_reversed_element = *reversed && *width == *element_width && *ascending;
                for offset in 0..=(element_width - width) {
                    let selected_start = if *ascending {
                        offset
                    } else {
                        offset + width - 1
                    };
                    let mut expected = Vec::with_capacity(start_bits.len());
                    for bit in 0..start_bits.len() {
                        expected.push(
                            self.const_net(LogicValue::bit((selected_start >> bit) & 1 == 1)),
                        );
                    }
                    let matches = self.eq_net(&start_bits, &expected);
                    for (bit, new_value) in value.iter().enumerate().take(*width) {
                        let index = if *reversed && !whole_reversed_element {
                            base_offset + element_width - 1 - (offset + bit)
                        } else {
                            base_offset + offset + bit
                        };
                        result[index] = self.mux(matches, *new_value, result[index]);
                    }
                }
                self.sym.insert(*net, result);
                self.written.insert(*net);
                Ok(())
            }
            Stmt::If { c, t, f } => {
                let cond = self.blast(c, 1)?[0];
                let saved_sym = self.sym.clone();
                let saved_written = std::mem::take(&mut self.written);
                self.exec_stmt(t)?;
                let then_sym = self.sym.clone();
                let then_written = std::mem::take(&mut self.written);
                let (else_sym, else_written) = if let Some(fs) = f {
                    self.sym = saved_sym.clone();
                    self.exec_stmt(fs)?;
                    let st = self.sym.clone();
                    let ew = self.written.clone();
                    self.written.clear();
                    (st, ew)
                } else {
                    (saved_sym.clone(), HashSet::new())
                };
                self.sym = saved_sym;
                let mut all: HashSet<usize> = then_written.clone();
                all.extend(else_written);
                all.extend(self.written.iter().copied());
                let all = sorted_set(&all);
                for b in &all {
                    let tv = then_sym
                        .get(b)
                        .cloned()
                        .unwrap_or_else(|| self.base_nets(*b));
                    let fv = else_sym
                        .get(b)
                        .cloned()
                        .unwrap_or_else(|| self.base_nets(*b));
                    let mut merged = Vec::with_capacity(tv.len());
                    for i in 0..tv.len() {
                        merged.push(self.mux(cond, tv[i], fv[i]));
                    }
                    self.sym.insert(*b, merged);
                }
                self.written = saved_written;
                self.written.extend(all);
                Ok(())
            }
            Stmt::Case { e, arms, dflt } => {
                let sel = self.blast(e, e.width())?;
                let saved_sym = self.sym.clone();
                let (default_state, default_written) = if let Some(d) = dflt {
                    self.sym = saved_sym.clone();
                    self.written.clear();
                    self.exec_stmt(d)?;
                    (Some(self.sym.clone()), Some(self.written.clone()))
                } else {
                    (None, None)
                };

                let mut arm_states = Vec::with_capacity(arms.len());
                let mut arm_written_sets = Vec::with_capacity(arms.len());
                for (label, body) in arms {
                    let lv = self.blast(label, e.width())?;
                    self.sym = saved_sym.clone();
                    self.written.clear();
                    self.exec_stmt(body)?;
                    let eq = self.eq_net(&sel, &lv);
                    arm_states.push((eq, self.sym.clone()));
                    arm_written_sets.push(self.written.clone());
                }

                let mut all_written: HashSet<usize> = default_written.clone().unwrap_or_default();
                for written in &arm_written_sets {
                    all_written.extend(written.iter().copied());
                }
                let ordered_all_written = sorted_set(&all_written);

                let exhaustive_without_default =
                    dflt.is_none() && exact_case_is_exhaustive(e, arms);
                let mut result = if let Some(state) = default_state {
                    let mut state = state;
                    for target in ordered_all_written.iter().copied() {
                        state
                            .entry(target)
                            .or_insert_with(|| self.base_nets(target));
                    }
                    state
                } else if exhaustive_without_default {
                    let mut state = saved_sym.clone();
                    // Variables assigned by every arm cannot observably retain
                    // their old value when the constant labels cover the full
                    // selector space. Starting from the old value would create
                    // a false combinational loop through the target itself.
                    for target in ordered_all_written.iter().copied() {
                        if arm_written_sets
                            .iter()
                            .all(|written| written.contains(&target))
                        {
                            let width = self.base_nets(target).len();
                            state.insert(
                                target,
                                (0..width)
                                    .map(|_| self.const_net(LogicValue::Zero))
                                    .collect(),
                            );
                        }
                    }
                    state
                } else {
                    // No default arm: unwritten case values hold current state.
                    let mut state = saved_sym.clone();
                    for target in ordered_all_written.iter().copied() {
                        state
                            .entry(target)
                            .or_insert_with(|| self.base_nets(target));
                    }
                    state
                };

                for ((eq, arm_state), _arm_written) in
                    arm_states.into_iter().zip(arm_written_sets).rev()
                {
                    // Merge every variable written by any arm or the default.
                    // An arm that leaves a variable unassigned retains the
                    // value from before the case, which is exactly the value
                    // captured in `arm_state`.
                    let targets: Vec<usize> = ordered_all_written.clone();
                    for k in targets {
                        let cur = result.get(&k).cloned().unwrap_or_else(|| self.base_nets(k));
                        let arm_v = arm_state
                            .get(&k)
                            .cloned()
                            .unwrap_or_else(|| self.base_nets(k));
                        let mut merged = Vec::with_capacity(cur.len());
                        for i in 0..cur.len() {
                            merged.push(if arm_v[i] == cur[i] {
                                cur[i]
                            } else {
                                self.mux(eq, arm_v[i], cur[i])
                            });
                        }
                        result.insert(k, merged);
                    }
                }
                self.sym = result;
                self.written = all_written;
                Ok(())
            }
            Stmt::Delay { .. } => Err("#delay is not synthesizable".into()),
            Stmt::MemoryBl { .. } | Stmt::MemoryNBl { .. } => {
                Err("first-class sparse-memory writes are not synthesizable yet".into())
            }
            Stmt::MemoryCopy { .. } => {
                Err("first-class memory copies are not synthesizable yet".into())
            }
            Stmt::MemoryNew { .. } | Stmt::MemoryDelete { .. } => {
                Err("dynamic-array allocation is not synthesizable yet".into())
            }
            Stmt::QueuePush { .. }
            | Stmt::QueuePop { .. }
            | Stmt::QueueInsert { .. }
            | Stmt::QueueDelete { .. } => Err("queue operations are not synthesizable yet".into()),
            Stmt::AssocWrite { .. } | Stmt::AssocDelete { .. } => {
                Err("associative-array operations are not synthesizable yet".into())
            }
            Stmt::AssocTraversal { .. } => {
                Err("associative-array traversal methods are not synthesizable yet".into())
            }
            Stmt::MemoryBlPart { .. } | Stmt::MemoryNBlPart { .. } => {
                Err("first-class sparse-memory part-select writes are not synthesizable yet".into())
            }
            Stmt::Display { .. }
            | Stmt::Finish
            | Stmt::Stop
            | Stmt::WriteMem { .. }
            | Stmt::Assert { .. }
            | Stmt::Assume { .. }
            | Stmt::Cover { .. }
            | Stmt::AssertControl(_)
            | Stmt::Severity(_) => Ok(()),
            Stmt::FileClose { .. }
            | Stmt::FileFlush { .. }
            | Stmt::FileGets { .. }
            | Stmt::FileError { .. }
            | Stmt::FileRead { .. }
            | Stmt::FileReadMemory { .. }
            | Stmt::FileScan { .. }
            | Stmt::StringScan { .. }
            | Stmt::FileDisplay { .. } => Err("file I/O is not synthesizable".into()),
            Stmt::RandomSeeded { .. } => Err("seeded randomization is not synthesizable".into()),
            Stmt::ImmediateAssertion {
                deferred:
                    eda_core::ir::ImmediateAssertionDeferred::Observed
                    | eda_core::ir::ImmediateAssertionDeferred::Final,
                ..
            } => Err("deferred assertions are not synthesizable".into()),
            Stmt::ImmediateAssertion {
                pass_action,
                fail_action,
                ..
            } if pass_action.is_some() || fail_action.is_some() => {
                Err("immediate assertion action blocks are not synthesizable".into())
            }
            Stmt::ImmediateAssertion { .. } => Ok(()),
        }
    }

    pub(crate) fn lower_seq(
        &mut self,
        clk: NetId,
        posedge: bool,
        reset: Option<(NetId, bool)>,
        body: &Stmt,
    ) -> Result<(), String> {
        let body = strip_block(body);
        if let (Some((rst_net, rst_high)), Stmt::If { c, t, f }) = (reset, body) {
            if (self.cond_is_net(c, rst_net) == Some(true) && rst_high
                || self.cond_is_net(c, rst_net) == Some(false) && !rst_high)
                && self.assigns_const_zero(t)?
            {
                self.reset_sym();
                let present = self.present_state_symbol_map();
                self.sym = present.clone();
                self.read_sym = Some(present);
                if let Some(fs) = f {
                    self.exec_stmt(fs)?;
                }
                let written = sorted_set(&self.written);
                let sym = self.sym.clone();
                for base in written {
                    let vals = sym
                        .get(&base)
                        .cloned()
                        .unwrap_or_else(|| self.base_nets(base));
                    for (i, value) in vals.iter().enumerate() {
                        let q = self.mk_dffr(clk, posedge, *value, rst_net, rst_high);
                        self.declare_driver(self.flat2net[base + i], q)?;
                    }
                }
                self.read_sym = None;
                return Ok(());
            }
        }
        self.reset_sym();
        let present = self.present_state_symbol_map();
        self.sym = present.clone();
        self.read_sym = Some(present);
        self.exec_stmt(body)?;
        let written = sorted_set(&self.written);
        let sym = self.sym.clone();
        for base in written {
            let vals = sym
                .get(&base)
                .cloned()
                .unwrap_or_else(|| self.base_nets(base));
            for (i, value) in vals.iter().enumerate() {
                let q = self.mk_dff(clk, posedge, *value);
                self.declare_driver(self.flat2net[base + i], q)?;
            }
        }
        self.read_sym = None;
        Ok(())
    }

    fn cond_is_net(&self, e: &Expr, net: NetId) -> Option<bool> {
        match e {
            Expr::Ref {
                net: flat, range, ..
            } if range.width() == 1 => {
                let off = match range {
                    Range::Bit(b) => *b,
                    Range::Span { lsb, .. } => *lsb,
                };
                if self.flat2net_of(flat + off) == Ok(net) {
                    Some(true)
                } else {
                    None
                }
            }
            // `!x` elaborates to Not(OrR(x)) for single-bit x
            Expr::Un { op: UnOp::Not, a } => {
                let inner: &Expr = match &**a {
                    Expr::Un {
                        op: UnOp::OrR,
                        a: inner,
                    } if inner.width() == 1 => inner,
                    other => other,
                };
                self.cond_is_net(inner, net).map(|x| !x)
            }
            // a bare 1-bit reduction wrapper
            Expr::Un { op: UnOp::OrR, a } if a.width() == 1 => self.cond_is_net(a, net),
            _ => None,
        }
    }

    fn assigns_const_zero(&mut self, s: &Stmt) -> Result<bool, String> {
        Ok(match s {
            Stmt::Block(v) => {
                let mut acc = true;
                for x in v {
                    acc = acc && self.assigns_const_zero(x)?;
                }
                acc
            }
            Stmt::NBl { lv: _, e } | Stmt::Bl { lv: _, e } => match e {
                Expr::Const(w) => w.bits.iter().all(|b| *b == LogicValue::Zero),
                _ => false,
            },
            Stmt::If { t, f: Some(f2), .. } => {
                self.assigns_const_zero(t)? && self.assigns_const_zero(f2)?
            }
            _ => false,
        })
    }

    fn mk_dff(&mut self, clk: NetId, posedge: bool, d: NetId) -> NetId {
        let q = self.new_tmp("q");
        self.add_cell("dxff", CellKind::Dff { posedge }, vec![clk, d], q);
        q
    }

    fn mk_dffr(
        &mut self,
        clk: NetId,
        posedge: bool,
        d: NetId,
        rst: NetId,
        rst_high: bool,
    ) -> NetId {
        let q = self.new_tmp("q");
        self.add_cell(
            "dxffr",
            CellKind::DffR {
                posedge,
                reset_high: rst_high,
            },
            vec![clk, d, rst],
            q,
        );
        q
    }
}

/// Unwrap `begin stmt end` wrappers.
fn strip_block(s: &Stmt) -> &Stmt {
    match s {
        Stmt::Block(v) if v.len() == 1 => strip_block(&v[0]),
        Stmt::Scope(body) => strip_block(body),
        other => other,
    }
}

/// Recognize a small, exact, constant-label case selector that covers every
/// binary value. This is conservative: unknown labels, non-constant labels,
/// width changes, and selectors wider than 24 bits do not claim full coverage.
fn exact_case_is_exhaustive(e: &Expr, arms: &[(Expr, Stmt)]) -> bool {
    let width = e.width();
    if width == 0 || width > 24 || arms.len() < (1usize << width) {
        return false;
    }
    let mut values = HashSet::with_capacity(arms.len());
    for (label, _) in arms {
        if label.width() != width {
            return false;
        }
        let Expr::Const(word) = label else {
            return false;
        };
        if !word.bits.iter().all(|bit| bit.is_01()) {
            return false;
        }
        values.insert(word.bits.clone());
    }
    values.len() == (1usize << width)
}

fn lv_bits(lv: &LValue) -> Vec<usize> {
    match &lv.range {
        Range::Bit(b) => vec![lv.net + b],
        Range::Span { msb, lsb } => (*lsb..=*msb).map(|i| lv.net + i).collect(),
    }
}

#[cfg(test)]
mod process_tests {
    use super::*;
    use eda_core::ir::Design;
    use eda_core::logic::LogicWord;

    #[test]
    fn process_control_statements_fail_closed_in_synthesis() {
        let design = Design::default();
        let mut synthesizer = Synth::new(&design).expect("create empty synthesizer");
        let handle = Expr::Const(LogicWord::from_u64(64, 1));
        for (statement, diagnostic) in [
            (
                Stmt::ProcessSuspend(Box::new(handle.clone())),
                "process suspend is not synthesizable",
            ),
            (
                Stmt::ProcessResume(Box::new(handle.clone())),
                "process resume is not synthesizable",
            ),
        ] {
            assert_eq!(synthesizer.exec_stmt(&statement).unwrap_err(), diagnostic);
        }
    }

    #[test]
    fn past_sampled_values_fail_closed_in_synthesis() {
        let design = Design::default();
        let mut synthesizer = Synth::new(&design).expect("create empty synthesizer");
        let expression = Expr::Past {
            value: Box::new(Expr::Const(LogicWord::from_u64(8, 0xa5))),
            ticks: 1,
            gating: None,
        };
        assert_eq!(
            synthesizer.blast_nat(&expression).unwrap_err(),
            "$past is not synthesizable"
        );
        let expression = Expr::Sampled(Box::new(Expr::Const(LogicWord::from_u64(8, 0xa5))));
        assert_eq!(
            synthesizer.blast_nat(&expression).unwrap_err(),
            "$sampled is not synthesizable"
        );
        for op in [
            eda_core::ir::SampledValueOp::Rose,
            eda_core::ir::SampledValueOp::Fell,
            eda_core::ir::SampledValueOp::Stable,
            eda_core::ir::SampledValueOp::Changed,
        ] {
            let expression = Expr::SampledValue {
                op,
                value: Box::new(Expr::Const(LogicWord::from_u64(1, 1))),
            };
            assert_eq!(
                synthesizer.blast_nat(&expression).unwrap_err(),
                "sampled-value functions are not synthesizable"
            );
        }
    }

    #[test]
    fn deferred_assertions_fail_closed_in_synthesis() {
        let design = Design::default();
        let mut synthesizer = Synth::new(&design).expect("create empty synthesizer");
        for deferred in [
            eda_core::ir::ImmediateAssertionDeferred::Observed,
            eda_core::ir::ImmediateAssertionDeferred::Final,
        ] {
            let statement = Stmt::ImmediateAssertion {
                kind: eda_core::ir::ImmediateAssertionKind::Assert,
                deferred,
                scope: None,
                cond: Expr::Const(LogicWord::from_u64(1, 1)),
                pass_action: None,
                fail_action: None,
            };
            assert_eq!(
                synthesizer.exec_stmt(&statement).unwrap_err(),
                "deferred assertions are not synthesizable"
            );
        }
    }
}
