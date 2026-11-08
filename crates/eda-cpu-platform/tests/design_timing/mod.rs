//! Fast evaluator for the `Design` produced by `netlist_to_design_with_timing`.
//!
//! The event-driven simulator is far too slow for a full opcode matrix on a
//! 90k-cell netlist. This evaluator consumes the post-synthesis IR (one
//! `Assign` per gate with its SDF delay, plus one edge-triggered process per
//! flop), levelizes the assigns once, and evaluates them in topological order
//! per clock cycle. It therefore exercises the literal
//! `netlist_to_design_with_timing` path while remaining practical for the full
//! matrices.

use eda_core::ir::{BinOp, Design, Expr, LValue, Range, Sens, Stmt, UnOp};
use eda_core::logic::{LogicValue, LogicWord};
use std::collections::{HashMap, HashSet};

struct Flop {
    q: usize,
    d: Expr,
    rst: Option<usize>,
    clk_to_q: f64,
}

pub struct DesignTiming<'a> {
    d: &'a Design,
    values: Vec<LogicValue>,
    arrival: Vec<f64>,
    order: Vec<usize>,
    flops: Vec<Flop>,
    ports: HashMap<String, (usize, usize)>,
    setup_by_data: HashMap<usize, f64>,
    hold_by_data: HashMap<usize, f64>,
    period: f64,
    worst_setup: f64,
    worst_hold: f64,
    max_arrival: f64,
}

fn lhs_bits(lv: &LValue) -> Vec<usize> {
    match lv.range {
        Range::Bit(b) => vec![lv.net + b],
        Range::Span { msb, lsb } => (lsb..=msb).map(|i| lv.net + i).collect(),
    }
}

fn collect_refs(e: &Expr, out: &mut Vec<usize>) {
    match e {
        Expr::Ref { net, range, .. } => match range {
            Range::Bit(b) => out.push(*net + b),
            Range::Span { msb, lsb } => out.extend((*lsb..=*msb).map(|i| net + i)),
        },
        Expr::Const(_) => {}
        Expr::Un { a, .. } => collect_refs(a, out),
        Expr::Bin { a, b, .. } => {
            collect_refs(a, out);
            collect_refs(b, out);
        }
        Expr::Cond { c, t, f } => {
            collect_refs(c, out);
            collect_refs(t, out);
            collect_refs(f, out);
        }
        Expr::Concat(parts) => parts.iter().for_each(|p| collect_refs(p, out)),
        Expr::Repeat { e, .. } => collect_refs(e, out),
        Expr::Index { e, .. } | Expr::Signed { e, .. } => collect_refs(e, out),
        _ => {}
    }
}

fn ref_net(e: &Expr) -> Option<usize> {
    match e {
        Expr::Ref { net, .. } => Some(*net),
        _ => None,
    }
}

/// Result of parsing a statement that assigns a register at a clock edge:
/// (destination net, next-state expression, optional active-high reset, delay).
type EdgeAssign = (usize, Expr, Option<usize>, u64);

fn parse_edge_assign(s: &Stmt) -> Option<EdgeAssign> {
    match s {
        Stmt::NBl { lv, e } => Some((lv.net, e.clone(), None, 0)),
        Stmt::If { c, t, f } => {
            let rst = ref_net(c);
            let f = f.as_deref()?;
            // `if (rst) q <= 0 else q <= d`
            let (_, zero, _, _) = parse_edge_assign(t)?;
            let (q, d, _, ns) = parse_edge_assign(f)?;
            let _ = zero;
            Some((q, d, rst, ns))
        }
        Stmt::Delay { ns, body } => {
            let (q, d, rst, _) = parse_edge_assign(body)?;
            Some((q, d, rst, *ns))
        }
        Stmt::Block(items) => items.iter().find_map(parse_edge_assign),
        _ => None,
    }
}

/// Flop parser: requires an edge assignment that is either direct or wrapped in
/// a `Delay`, or a `Block` whose *delay terminator* assigns a register. This
/// excludes setup/hold shadow processes (which only emit severity checks).
fn find_flop(process: &eda_core::ir::Process) -> Option<Flop> {
    match &process.sens {
        Sens::Posedge(_) | Sens::List(_) => {}
        _ => return None,
    }
    let (q, d, rst, clk_to_q) = match &process.body {
        Stmt::NBl { .. } | Stmt::If { .. } => parse_edge_assign(&process.body)?,
        Stmt::Delay { ns, body } => {
            let (q, d, rst, _) = parse_edge_assign(body)?;
            (q, d, rst, *ns)
        }
        Stmt::Block(items) => {
            // Sample-at-edge form: Block([sample, Delay { ns, NBl(q, Ref(shadow)) }])
            let delay = items.iter().find_map(|s| match s {
                Stmt::Delay { ns, body } => Some((*ns, body.as_ref())),
                _ => None,
            })?;
            let (q, _, _, _) = parse_edge_assign(delay.1)?;
            let sample = items
                .iter()
                .find(|s| !matches!(s, Stmt::Delay { .. }))
                .and_then(parse_edge_assign)?;
            (q, sample.1, sample.2, delay.0)
        }
        _ => return None,
    };
    Some(Flop {
        q,
        d,
        rst,
        clk_to_q: clk_to_q as f64,
    })
}

fn data_net(e: &Expr) -> usize {
    ref_net(e).unwrap_or(usize::MAX)
}

impl<'a> DesignTiming<'a> {
    pub fn new(d: &'a Design, period_ns: f64) -> Result<Self, String> {
        let mut ports = HashMap::new();
        for p in &d.ports {
            ports.insert(p.name.clone(), (p.base, p.width));
        }
        let mut drivers = vec![None; d.n_nets];
        for (index, a) in d.assigns.iter().enumerate() {
            for b in lhs_bits(&a.lv) {
                drivers[b] = Some(index);
            }
        }
        let mut indegree = vec![0usize; d.assigns.len()];
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); d.assigns.len()];
        for (index, a) in d.assigns.iter().enumerate() {
            let mut refs = Vec::new();
            collect_refs(&a.e, &mut refs);
            let mut seen = HashSet::new();
            for r in refs {
                if let Some(Some(driver)) = drivers.get(r) {
                    if seen.insert(*driver) {
                        indegree[index] += 1;
                        dependents[*driver].push(index);
                    }
                }
            }
        }
        let mut ready: Vec<usize> = (0..d.assigns.len()).filter(|i| indegree[*i] == 0).collect();
        let mut order = Vec::with_capacity(d.assigns.len());
        while let Some(index) = ready.pop() {
            order.push(index);
            for &next in &dependents[index] {
                indegree[next] -= 1;
                if indegree[next] == 0 {
                    ready.push(next);
                }
            }
        }
        if order.len() != d.assigns.len() {
            return Err("combinational cycle in design-level timing netlist".into());
        }

        let mut setup_by_data = HashMap::new();
        let mut hold_by_data = HashMap::new();
        for a in &d.assigns {
            if d.net_name(a.lv.net).ends_with("__setup_shadow") {
                if let Expr::Ref { net, .. } = &a.e {
                    setup_by_data.insert(*net, a.delay as f64);
                }
            }
        }
        for p in &d.processes {
            if let Stmt::Block(items) = &p.body {
                if let Some(Stmt::NBl {
                    lv,
                    e: Expr::Ref { net, .. },
                }) = items.first()
                {
                    if d.net_name(lv.net).ends_with("__hold_sample") {
                        if let Some(Stmt::Delay { ns, .. }) = items.get(1) {
                            hold_by_data.insert(*net, *ns as f64);
                        }
                    }
                }
            }
        }

        let mut flops = Vec::new();
        for p in &d.processes {
            if let Some(f) = find_flop(p) {
                flops.push(f);
            }
        }

        Ok(Self {
            d,
            values: vec![LogicValue::X; d.n_nets],
            arrival: vec![0.0; d.n_nets],
            order,
            flops,
            ports,
            setup_by_data,
            hold_by_data,
            period: period_ns,
            worst_setup: f64::INFINITY,
            worst_hold: f64::INFINITY,
            max_arrival: 0.0,
        })
    }

    pub fn set_port(&mut self, name: &str, value: u64) {
        let Some(&(base, width)) = self.ports.get(name) else {
            return;
        };
        for k in 0..width {
            self.values[base + k] = if (value >> k) & 1 == 1 {
                LogicValue::One
            } else {
                LogicValue::Zero
            };
        }
    }

    pub fn read_port(&self, name: &str) -> Option<u64> {
        let &(base, width) = self.ports.get(name)?;
        let mut v = 0u64;
        for k in 0..width {
            match self.values[base + k] {
                LogicValue::One => v |= 1 << k,
                LogicValue::Zero => {}
                _ => return None,
            }
        }
        Some(v)
    }

    pub fn read_bit(&self, name: &str) -> bool {
        self.read_port(name) == Some(1)
    }

    #[allow(dead_code)]
    pub fn read_decl(&self, name: &str) -> Option<u64> {
        let decl = self.d.net_decls.iter().find(|d| d.name == name)?;
        let mut v = 0u64;
        for k in 0..decl.width {
            match self.values[decl.base + k] {
                LogicValue::One => v |= 1 << k,
                LogicValue::Zero => {}
                _ => return None,
            }
        }
        Some(v)
    }

    fn word(&self, e: &Expr) -> LogicWord {
        match e {
            Expr::Const(w) => w.clone(),
            Expr::Ref { net, range, .. } => LogicWord {
                bits: match range {
                    Range::Bit(b) => {
                        vec![self.values.get(net + b).copied().unwrap_or(LogicValue::X)]
                    }
                    Range::Span { msb, lsb } => (*lsb..=*msb)
                        .map(|i| self.values.get(net + i).copied().unwrap_or(LogicValue::X))
                        .collect(),
                },
            },
            Expr::Un { op, a } => {
                let a = self.word(a);
                match op {
                    UnOp::Not => LogicWord {
                        bits: a.bits.iter().map(|b| b.not()).collect(),
                    },
                    UnOp::AndR => LogicWord::new_const(
                        1,
                        a.bits
                            .iter()
                            .copied()
                            .reduce(|x, y| x.and(y))
                            .unwrap_or(LogicValue::One),
                    ),
                    UnOp::OrR => LogicWord::new_const(1, a.reduce_or()),
                    UnOp::XorR => LogicWord::new_const(
                        1,
                        a.bits
                            .iter()
                            .copied()
                            .reduce(|x, y| x.xor(y))
                            .unwrap_or(LogicValue::Zero),
                    ),
                }
            }
            Expr::Bin { op, a, b } => {
                let a = self.word(a);
                let b = self.word(b);
                let w = a.width().max(b.width());
                let a = a.sized(w);
                let b = b.sized(w);
                match op {
                    BinOp::And | BinOp::Or | BinOp::Xor => LogicWord {
                        bits: a
                            .bits
                            .iter()
                            .zip(&b.bits)
                            .map(|(x, y)| match op {
                                BinOp::And => x.and(*y),
                                BinOp::Or => x.or(*y),
                                _ => x.xor(*y),
                            })
                            .collect(),
                    },
                    BinOp::Eq | BinOp::Ne => {
                        let eq = a
                            .bits
                            .iter()
                            .zip(&b.bits)
                            .fold(LogicValue::One, |acc, (x, y)| acc.and(x.xor(*y).not()));
                        LogicWord::new_const(1, if *op == BinOp::Ne { eq.not() } else { eq })
                    }
                    _ => LogicWord::xes(w),
                }
            }
            Expr::Cond { c, t, f } => {
                let c = self.word(c).reduce_or();
                match c.as_bool() {
                    Some(true) => self.word(t),
                    Some(false) => self.word(f),
                    None => {
                        let t = self.word(t);
                        let f = self.word(f);
                        let w = t.width().max(f.width());
                        let t = t.sized(w);
                        let f = f.sized(w);
                        LogicWord {
                            bits: t
                                .bits
                                .iter()
                                .zip(&f.bits)
                                .map(|(x, y)| if x == y { *x } else { LogicValue::X })
                                .collect(),
                        }
                    }
                }
            }
            Expr::Concat(parts) => {
                let words: Vec<LogicWord> = parts.iter().map(|p| self.word(p)).collect();
                let refs: Vec<&LogicWord> = words.iter().collect();
                LogicWord::concat(&refs)
            }
            Expr::Repeat { times, e } => {
                let w = self.word(e);
                let refs: Vec<&LogicWord> = (0..*times).map(|_| &w).collect();
                LogicWord::concat(&refs)
            }
            Expr::Index { e, msb, lsb } => self.word(e).slice(*msb, *lsb),
            Expr::Signed { e, .. } => self.word(e),
            _ => LogicWord::xes(1),
        }
    }

    pub fn eval(&mut self) {
        for &index in &self.order {
            let a = &self.d.assigns[index];
            let value = self.word(&a.e);
            let bits = lhs_bits(&a.lv);
            for (i, net) in bits.iter().enumerate() {
                self.values[*net] = value.bits.get(i).copied().unwrap_or(LogicValue::X);
            }
            let mut refs = Vec::new();
            collect_refs(&a.e, &mut refs);
            let mut arrival = 0.0_f64;
            for r in refs {
                arrival = arrival.max(self.arrival.get(r).copied().unwrap_or(0.0));
            }
            let arrived = arrival + a.delay as f64;
            for net in bits {
                self.arrival[net] = arrived;
            }
        }
    }

    pub fn timing_check(&mut self) {
        for f in &self.flops {
            let data = data_net(&f.d);
            let arrival = self.arrival.get(data).copied().unwrap_or(0.0);
            let setup = self.setup_by_data.get(&data).copied().unwrap_or(0.0);
            let hold = self.hold_by_data.get(&data).copied().unwrap_or(0.0);
            self.max_arrival = self.max_arrival.max(arrival);
            self.worst_setup = self.worst_setup.min(self.period - (arrival + setup));
            self.worst_hold = self.worst_hold.min(arrival - hold);
        }
    }

    pub fn edge(&mut self) {
        let mut updates = Vec::with_capacity(self.flops.len());
        for f in &self.flops {
            let reset = f.rst.is_some_and(|net| self.values[net] == LogicValue::One);
            let next = if reset {
                LogicValue::Zero
            } else {
                self.word(&f.d)
                    .bits
                    .first()
                    .copied()
                    .unwrap_or(LogicValue::X)
            };
            updates.push((f.q, next, f.clk_to_q));
        }
        for (q, value, delay) in updates {
            self.values[q] = value;
            self.arrival[q] = delay;
        }
    }

    pub fn reset_timing(&mut self) {
        self.worst_setup = f64::INFINITY;
        self.worst_hold = f64::INFINITY;
        self.max_arrival = 0.0;
        self.arrival.fill(0.0);
    }

    pub fn worst_setup_slack(&self) -> f64 {
        self.worst_setup
    }
    pub fn worst_hold_slack(&self) -> f64 {
        self.worst_hold
    }
    pub fn max_arrival(&self) -> f64 {
        self.max_arrival
    }
}
