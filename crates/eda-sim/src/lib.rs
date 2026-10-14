//! # eda-sim — event-driven 4-valued simulator
//!
//! Executes an elaborated [`Design`] (RTL, or gate-level via
//! [`eda_core::to_design::netlist_to_design`]) with Verilog-style
//! scheduling: active events, delta cycles for continuous assignments,
//! non-blocking (NBA) updates at the end of each time slot, edge and
//! combinational `always` processes, `#delay` continuations, `forever`,
//! `$display`, `$monitor`, `$finish` and optional VCD dumping.

use eda_core::cancellation::FlowCancellation;
use eda_core::ir::{Design, Dir, Expr, LValue, Range, Sens, Stmt};
use eda_core::logic::{LogicValue, LogicWord};
use std::collections::{BTreeMap, HashSet};

mod vcd;
pub use vcd::VcdWriter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Pos,
    Neg,
    Any,
}

#[derive(Clone, Debug)]
struct Update {
    net: usize,
    val: LogicValue,
    /// Continuous-assign driver index (for multi-driver resolution).
    driver: Option<usize>,
}

struct Cont<'d> {
    process: Option<usize>,
    work: Vec<&'d Stmt>,
}

struct Monitor<'d> {
    parts: &'d [eda_core::ir::DispPart],
    args: &'d [Expr],
    last: Vec<LogicWord>,
}

const MAX_STEPS_PER_TIME: u64 = 200_000;
const MAX_EVENTS: u64 = 20_000_000;

pub struct Simulator<'d> {
    pub design: &'d Design,
    vals: Vec<LogicValue>,
    events: BTreeMap<u64, Vec<Update>>,
    conts: BTreeMap<u64, Vec<Cont<'d>>>,
    now: u64,
    nba: Vec<(usize, LogicValue)>,
    busy_procs: HashSet<usize>,
    assign_fanin: Vec<Vec<usize>>,
    proc_edges: Vec<Vec<(usize, Edge)>>,
    pub displays: Vec<String>,
    property_covers: BTreeMap<String, u64>,
    finished: bool,
    print_stdout: bool,
    monitors: Vec<Monitor<'d>>,
    vcd: Option<VcdWriter>,
    vcd_changes: HashSet<usize>,
    event_count: u64,
    has_zero_delay_feedback: bool,
    cancellation: FlowCancellation,
}

impl<'d> Simulator<'d> {
    pub fn new(design: &'d Design) -> Result<Self, String> {
        let n = design.n_nets;
        let mut assign_fanin: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (ai, a) in design.assigns.iter().enumerate() {
            for net in expr_nets(&a.e) {
                if net < n {
                    assign_fanin[net].push(ai);
                }
            }
        }
        let mut proc_edges: Vec<Vec<(usize, Edge)>> = vec![Vec::new(); n];
        for (pi, p) in design.processes.iter().enumerate() {
            register_sens(&p.sens, pi, &p.body, &mut proc_edges, n);
        }
        let mut mons = Vec::new();
        for init in &design.initials {
            collect_monitors(init, &mut mons);
        }
        let mut sim = Simulator {
            design,
            vals: vec![LogicValue::X; n],
            events: BTreeMap::new(),
            conts: BTreeMap::new(),
            now: 0,
            nba: Vec::new(),
            busy_procs: HashSet::new(),
            assign_fanin,
            proc_edges,
            displays: Vec::new(),
            property_covers: BTreeMap::new(),
            finished: false,
            print_stdout: false,
            monitors: mons
                .into_iter()
                .map(|(parts, args)| Monitor {
                    parts,
                    args,
                    last: Vec::new(),
                })
                .collect(),
            vcd: None,
            vcd_changes: HashSet::new(),
            event_count: 0,
            has_zero_delay_feedback: design
                .processes
                .iter()
                .any(|p| matches!(p.sens, Sens::Comb) && stmt_has_zero_delay_feedback(&p.body)),
            cancellation: FlowCancellation::new(),
        };
        for init in &design.initials {
            sim.conts.entry(0).or_default().push(Cont {
                process: None,
                work: vec![init],
            });
        }
        // Prime: input-free continuous assigns (constants) evaluate once, and
        // combinational processes execute once at time 0, per Verilog.
        let mut prime_updates = Vec::new();
        for a in &design.assigns {
            if expr_nets(&a.e).is_empty() {
                let v = sim.eval(&a.e);
                for (i, net) in lv_bits(&a.lv).into_iter().enumerate() {
                    if net < n {
                        prime_updates.push(Update {
                            net,
                            val: v.bit(i),
                            driver: None,
                        });
                    }
                }
            }
        }
        for u in prime_updates {
            sim.schedule(u);
        }
        for (pi, p) in design.processes.iter().enumerate() {
            if matches!(p.sens, Sens::Comb) {
                sim.conts.entry(0).or_default().push(Cont {
                    process: Some(pi),
                    work: vec![&p.body],
                });
            }
        }
        Ok(sim)
    }

    // ---------- public API ----------

    pub fn now(&self) -> u64 {
        self.now
    }

    pub fn finished(&self) -> bool {
        self.finished
    }

    pub fn set_print_stdout(&mut self, on: bool) {
        self.print_stdout = on;
    }

    /// Install a shared cancellation token checked inside delta cycles.
    pub fn set_cancellation(&mut self, cancellation: &FlowCancellation) {
        self.cancellation = cancellation.clone();
    }

    pub fn enable_vcd(&mut self) {
        self.vcd = Some(VcdWriter::new(self.design));
    }

    pub fn vcd(&self) -> Option<String> {
        self.vcd.as_ref().map(|v| v.render(&self.vals))
    }

    /// Set an input port by name; propagates immediately (delta cycles).
    pub fn set_input(&mut self, port: &str, value: &LogicWord) -> Result<(), String> {
        if self.cancellation.is_cancelled() {
            return Err("simulation cancelled".into());
        }
        // External stimulus is applied to an already-settled timestamp. In
        // particular, time-zero initial blocks must run before a caller can
        // inject the first clock edge, preventing that edge from sampling X.
        self.settle()?;
        let p = self
            .design
            .port(port)
            .ok_or_else(|| format!("no port named '{}'", port))?;
        if p.dir != Dir::In {
            return Err(format!("port '{}' is not an input", port));
        }
        for i in 0..p.width {
            self.schedule(Update {
                net: p.base + i,
                val: value.bit(i),
                driver: None,
            });
        }
        self.settle()
    }

    /// Set an input using an explicit cooperative cancellation token.
    pub fn set_input_cancellable(
        &mut self,
        port: &str,
        value: &LogicWord,
        cancellation: &FlowCancellation,
    ) -> Result<(), String> {
        self.set_cancellation(cancellation);
        self.set_input(port, value)
    }

    /// Read a declaration (any net in the design) by name.
    pub fn read(&self, name: &str) -> Option<LogicWord> {
        let d = self.design.net_decls.iter().find(|d| d.name == name)?;
        Some(LogicWord {
            bits: self.vals[d.base..d.base + d.width].to_vec(),
        })
    }

    pub fn read_port(&self, name: &str) -> Option<LogicWord> {
        let p = self.design.port(name)?;
        Some(LogicWord {
            bits: self.vals[p.base..p.base + p.width].to_vec(),
        })
    }

    pub fn read_net_bit(&self, flat: usize) -> LogicValue {
        self.vals.get(flat).copied().unwrap_or(LogicValue::X)
    }

    /// Snapshot of every declaration: (name, hex value).
    pub fn snapshot(&self) -> Vec<(String, String)> {
        self.design
            .net_decls
            .iter()
            .map(|d| {
                (
                    d.name.clone(),
                    LogicWord {
                        bits: self.vals[d.base..d.base + d.width].to_vec(),
                    }
                    .to_hex_str(),
                )
            })
            .collect()
    }

    pub fn take_displays(&mut self) -> Vec<String> {
        std::mem::take(&mut self.displays)
    }

    pub fn property_cover_hits(&self, label: &str) -> u64 {
        self.property_covers.get(label).copied().unwrap_or(0)
    }

    pub fn take_property_covers(&mut self) -> BTreeMap<String, u64> {
        std::mem::take(&mut self.property_covers)
    }

    /// Run until absolute time `t` (ns), `$finish`, or quiescence.
    pub fn run_until(&mut self, t: u64) -> Result<(), String> {
        if self.cancellation.is_cancelled() {
            return Err("simulation cancelled".into());
        }
        if self.has_zero_delay_feedback {
            return Err("zero-delay combinational loop detected".into());
        }
        loop {
            if self.cancellation.is_cancelled() {
                return Err("simulation cancelled".into());
            }
            if self.finished {
                return Ok(());
            }
            self.settle()?;
            if self.finished {
                return Ok(());
            }
            let next = self.events.keys().chain(self.conts.keys()).next().copied();
            match next {
                None => {
                    self.now = t;
                    return Ok(());
                }
                Some(nt) if nt > t => {
                    self.now = t;
                    return Ok(());
                }
                Some(nt) => self.now = nt,
            }
        }
    }

    /// Run until time `t` with an explicit cooperative cancellation token.
    pub fn run_until_cancellable(
        &mut self,
        t: u64,
        cancellation: &FlowCancellation,
    ) -> Result<(), String> {
        self.set_cancellation(cancellation);
        self.run_until(t)
    }

    /// Advance exactly one time slot; false when quiescent.
    pub fn step(&mut self) -> Result<bool, String> {
        if self.cancellation.is_cancelled() {
            return Err("simulation cancelled".into());
        }
        self.settle()?;
        let next = self.events.keys().chain(self.conts.keys()).next().copied();
        match next {
            None => Ok(false),
            Some(nt) => {
                self.now = nt;
                self.settle()?;
                Ok(true)
            }
        }
    }

    /// Advance one time slot with an explicit cancellation token.
    pub fn step_cancellable(&mut self, cancellation: &FlowCancellation) -> Result<bool, String> {
        self.set_cancellation(cancellation);
        self.step()
    }

    // ---------- scheduler ----------

    fn schedule(&mut self, u: Update) {
        let queue = self.events.entry(self.now).or_default();
        let has_pending_for_driver = queue
            .iter()
            .any(|existing| existing.net == u.net && existing.driver == u.driver);
        // A driver may be notified several times while related zero-delay
        // updates settle. Keep only its newest update so a stale event cannot
        // overwrite a value scheduled later in the same delta wave. Updates
        // from different drivers are retained for multi-driver resolution.
        queue.retain(|existing| !(existing.net == u.net && existing.driver == u.driver));
        // If this driver has no pending update and the resolved net already
        // has the requested value, the event is a semantic no-op. Skipping it
        // keeps wide vector assigns linear rather than enqueueing every LHS
        // bit for every RHS-bit notification.
        if !has_pending_for_driver && self.vals[u.net] == u.val {
            return;
        }
        queue.push(u);
    }

    fn settle(&mut self) -> Result<(), String> {
        if self.cancellation.is_cancelled() {
            return Err("simulation cancelled".into());
        }
        let mut steps_this_settle = 0u64;
        loop {
            if self.cancellation.is_cancelled() {
                return Err("simulation cancelled".into());
            }
            let mut did = false;
            loop {
                if self.cancellation.is_cancelled() {
                    return Err("simulation cancelled".into());
                }
                let Some(queue) = self.events.get_mut(&self.now) else {
                    break;
                };
                let Some(u) = queue.pop() else {
                    self.events.remove(&self.now);
                    break;
                };
                // A continuous vector assignment is re-evaluated for every
                // source-bit change and may enqueue many bits whose values did
                // not change. Such no-op events must not consume the
                // zero-delay-loop budget; otherwise a legitimate wide vector
                // load can require O(width^2) popped events.
                if self.vals[u.net] == u.val {
                    continue;
                }
                steps_this_settle += 1;
                if steps_this_settle > MAX_STEPS_PER_TIME {
                    return Err("simulation stalled: zero-delay activity loop".into());
                }
                self.apply_update(u)?;
                did = true;
                if self.finished {
                    break;
                }
            }
            // Run procedural continuations only after the current zero-delay
            // event wave has drained. Combinational processes then observe a
            // consistent set of continuous-assign values rather than a stale
            // mixture from the middle of the wave.
            if let Some(cs) = self.conts.remove(&self.now) {
                for c in cs {
                    if self.cancellation.is_cancelled() {
                        return Err("simulation cancelled".into());
                    }
                    self.run_cont(c)?;
                    did = true;
                }
                if self.finished {
                    break;
                }
            }
            if self.finished {
                break;
            }
            if !did && !self.nba.is_empty() {
                let updates = std::mem::take(&mut self.nba);
                for (net, val) in updates {
                    if self.cancellation.is_cancelled() {
                        return Err("simulation cancelled".into());
                    }
                    self.schedule(Update {
                        net,
                        val,
                        driver: None,
                    });
                }
                continue;
            }
            if !did {
                break;
            }
            steps_this_settle += 1;
            if steps_this_settle > MAX_STEPS_PER_TIME {
                return Err("simulation stalled: zero-delay activity loop".into());
            }
        }
        self.fire_monitors();
        self.flush_vcd();
        Ok(())
    }

    fn apply_update(&mut self, u: Update) -> Result<(), String> {
        self.event_count += 1;
        if self.event_count > MAX_EVENTS {
            return Err("event budget exceeded".into());
        }
        let old = self.vals[u.net];
        if old == u.val {
            return Ok(());
        }
        let mut newv = u.val;
        if u.driver.is_some() {
            let others: Vec<usize> = self
                .design
                .assigns
                .iter()
                .enumerate()
                .filter(|(ai, a)| Some(*ai) != u.driver && covers(&a.lv, u.net))
                .map(|(ai, _)| ai)
                .collect();
            for ai in others {
                let v = self.eval(&self.design.assigns[ai].e);
                let bit = lv_bit(&self.design.assigns[ai].lv, u.net)
                    .map(|i| v.bit(i))
                    .unwrap_or(LogicValue::X);
                newv = newv.resolve(bit);
            }
        }
        if old == newv {
            return Ok(());
        }
        self.vals[u.net] = newv;
        self.mark_vcd_change(u.net);
        self.notify_change(u.net, old, newv)
    }

    /// Run the event consequences of a value that has already changed.
    /// Splitting this from `apply_update` lets blocking procedural
    /// assignments take effect before the next statement (required by
    /// Verilog's active region semantics).
    fn notify_change(
        &mut self,
        changed_net: usize,
        old: LogicValue,
        newv: LogicValue,
    ) -> Result<(), String> {
        let pos = newv == LogicValue::One && old != LogicValue::One;
        let neg = newv == LogicValue::Zero && old != LogicValue::Zero;
        let triggers: Vec<(usize, Edge)> = self
            .proc_edges
            .get(changed_net)
            .cloned()
            .unwrap_or_default();
        let mut deferred_processes = Vec::new();
        for (pi, edge) in triggers {
            let fire = match edge {
                Edge::Pos => pos,
                Edge::Neg => neg,
                Edge::Any => true,
            };
            if !fire {
                continue;
            }
            match edge {
                // Level-sensitive work is queued rather than run inside the
                // value-change notification. This prevents a combinational
                // process from observing an intermediate mixture of old and
                // new continuous-assign values.
                Edge::Any => deferred_processes.push(pi),
                Edge::Pos | Edge::Neg => {
                    if self.busy_procs.contains(&pi) {
                        // A zero-delay feedback edge arrived while this process
                        // was active. Requeue it; bounded scheduler iteration
                        // will diagnose a genuine oscillation.
                        deferred_processes.push(pi);
                    } else {
                        let body = &self.design.processes[pi].body;
                        self.busy_procs.insert(pi);
                        self.run_cont(Cont {
                            process: Some(pi),
                            work: vec![body],
                        })?;
                    }
                }
            }
        }
        for pi in deferred_processes {
            let body = &self.design.processes[pi].body;
            self.conts.entry(self.now).or_default().push(Cont {
                process: Some(pi),
                work: vec![body],
            });
        }
        let sinks: Vec<usize> = self
            .assign_fanin
            .get(changed_net)
            .cloned()
            .unwrap_or_default();
        for ai in sinks {
            let v = self.eval(&self.design.assigns[ai].e);
            let delay = self.design.assigns[ai].delay;
            let t = self.now + delay;
            for (i, net) in lv_bits(&self.design.assigns[ai].lv).into_iter().enumerate() {
                let val = v.bit(i);
                if t == self.now {
                    self.schedule(Update {
                        net,
                        val,
                        driver: Some(ai),
                    });
                } else {
                    self.events.entry(t).or_default().push(Update {
                        net,
                        val,
                        driver: Some(ai),
                    });
                }
            }
        }
        Ok(())
    }

    fn run_cont(&mut self, c: Cont<'d>) -> Result<(), String> {
        let mut work = c.work;
        while let Some(s) = work.pop() {
            if self.cancellation.is_cancelled() {
                return Err("simulation cancelled".into());
            }
            match s {
                Stmt::Block(v) => {
                    for st in v.iter().rev() {
                        work.push(st);
                    }
                }
                Stmt::Forever(inner) => {
                    work.push(s);
                    work.push(inner);
                }
                Stmt::Bl { lv, e } => {
                    let v = self.eval(e);
                    // Assignment context sizes the RHS to the lvalue:
                    // narrower values zero-extend (a 1-bit comparison stored
                    // into an `int`), wider values truncate.
                    let v = v.sized(lv_bits(lv).len());
                    // Blocking assignment updates state immediately, then
                    // wakes dependent processes/assignments. NBA remains in
                    // self.nba until the active region completes.
                    let mut changes = Vec::new();
                    for (i, net) in lv_bits(lv).into_iter().enumerate() {
                        let newv = v.bit(i);
                        let old = self.vals[net];
                        if old != newv {
                            self.vals[net] = newv;
                            self.mark_vcd_change(net);
                            changes.push((net, old, newv));
                        }
                    }
                    for (net, old, newv) in changes {
                        self.event_count += 1;
                        if self.event_count > MAX_EVENTS {
                            return Err("event budget exceeded".into());
                        }
                        self.notify_change(net, old, newv)?;
                    }
                }
                Stmt::NBl { lv, e } => {
                    let v = self.eval(e);
                    let v = v.sized(lv_bits(lv).len());
                    for (i, net) in lv_bits(lv).into_iter().enumerate() {
                        self.nba.push((net, v.bit(i)));
                    }
                }
                Stmt::BlIndexed {
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
                    let value = self.eval(e);
                    let Some(targets) = self.indexed_targets(
                        *net,
                        *decl_width,
                        *base_offset,
                        *element_width,
                        start,
                        *width,
                        *ascending,
                        *reversed,
                        &value,
                    ) else {
                        continue;
                    };
                    let mut changes = Vec::new();
                    for (net, newv) in targets {
                        let old = self.vals[net];
                        if old != newv {
                            self.vals[net] = newv;
                            self.mark_vcd_change(net);
                            changes.push((net, old, newv));
                        }
                    }
                    for (net, old, newv) in changes {
                        self.event_count += 1;
                        if self.event_count > MAX_EVENTS {
                            return Err("event budget exceeded".into());
                        }
                        self.notify_change(net, old, newv)?;
                    }
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
                } => {
                    let value = self.eval(e);
                    let Some(targets) = self.indexed_targets(
                        *net,
                        *decl_width,
                        *base_offset,
                        *element_width,
                        start,
                        *width,
                        *ascending,
                        *reversed,
                        &value,
                    ) else {
                        continue;
                    };
                    for (net, value) in targets {
                        self.nba.push((net, value));
                    }
                }
                Stmt::If { c: cond, t, f } => {
                    let cv = self.eval(cond);
                    match cv.reduce_or().as_bool() {
                        Some(true) => work.push(t),
                        _ => {
                            if let Some(fs) = f {
                                work.push(fs);
                            }
                        }
                    }
                }
                Stmt::Case { e, arms, dflt } => {
                    let v = self.eval(e);
                    let mut chosen: Option<&Stmt> = None;
                    for (label, body) in arms {
                        let lv = self.eval(label);
                        if case_eq_words(&v, &lv) {
                            chosen = Some(body);
                            break;
                        }
                    }
                    if let Some(body) = chosen.or(dflt.as_deref()) {
                        work.push(body);
                    }
                }
                Stmt::Delay { ns, body } => {
                    // Wake at now+ns, run `body`, then the pending work.
                    let mut exec_order: Vec<&Stmt> = vec![&**body];
                    exec_order.extend(work.iter().rev().copied());
                    exec_order.reverse(); // LIFO: pop() returns body first
                    self.conts.entry(self.now + ns).or_default().push(Cont {
                        process: c.process,
                        work: exec_order,
                    });
                    return Ok(());
                }
                Stmt::Display { parts, args, .. } => {
                    let line = self.format_display(parts, args);
                    if self.print_stdout {
                        println!("{}", line);
                    }
                    self.displays.push(line);
                }
                Stmt::Assert { cond, message } => {
                    let value = self.eval(cond);
                    if value.reduce_or().as_bool() != Some(true) {
                        let detail = message
                            .as_deref()
                            .unwrap_or("immediate assertion condition is false or unknown");
                        return Err(format!("assertion failed at t={}: {}", self.now, detail));
                    }
                }
                Stmt::Assume { cond, message } => {
                    let value = self.eval(cond);
                    if value.reduce_or().as_bool() != Some(true) {
                        let detail = message
                            .as_deref()
                            .unwrap_or("assumption condition is false or unknown");
                        return Err(format!("assumption failed at t={}: {}", self.now, detail));
                    }
                }
                Stmt::Cover { label, cond } => {
                    let value = self.eval(cond);
                    if value.reduce_or().as_bool() == Some(true) {
                        *self.property_covers.entry(label.clone()).or_insert(0) += 1;
                    }
                }
                Stmt::Finish => {
                    self.finished = true;
                    return Ok(());
                }
            }
            if self.finished {
                return Ok(());
            }
        }
        if let Some(pi) = c.process {
            self.busy_procs.remove(&pi);
        }
        Ok(())
    }

    fn format_display(&self, parts: &[eda_core::ir::DispPart], args: &[Expr]) -> String {
        use eda_core::ir::DispPart;
        let mut out = String::new();
        for p in parts {
            match p {
                DispPart::Str(s) => out.push_str(s),
                DispPart::Time => out.push_str(&self.now.to_string()),
                DispPart::Arg(i, spec) => {
                    let v = self.eval(&args[*i]);
                    match spec {
                        'b' => out.push_str(&v.to_bin_str()),
                        'h' => out.push_str(&v.to_hex_str()),
                        's' => {
                            // 512 位打包字符串：首字符在最低字节，NUL 截断。
                            out.push_str(&v.to_ascii_string_nul_terminated());
                        }
                        _ => match v.to_u64() {
                            Some(u) => out.push_str(&u.to_string()),
                            None => out.push('x'),
                        },
                    }
                }
            }
        }
        out
    }

    // ---------- expression evaluation ----------

    #[allow(clippy::too_many_arguments)]
    fn indexed_targets(
        &self,
        net: usize,
        _decl_width: usize,
        base_offset: usize,
        element_width: usize,
        start: &Expr,
        width: usize,
        ascending: bool,
        reversed: bool,
        value: &LogicWord,
    ) -> Option<Vec<(usize, LogicValue)>> {
        let start = self.eval(start).to_u64()?;
        let offset = if ascending {
            start as i128
        } else {
            start as i128 - (width as i128 - 1)
        };
        let max_offset = element_width as i128 - width as i128;
        if offset < 0 || offset > max_offset {
            return None;
        }
        let offset = offset as usize;
        let mut targets = Vec::with_capacity(width);
        for (bit, new_value) in value.bits.iter().enumerate().take(width) {
            let physical_offset = if reversed {
                base_offset + element_width - 1 - (offset + bit)
            } else {
                base_offset + offset + bit
            };
            targets.push((net + physical_offset, *new_value));
        }
        Some(targets)
    }

    pub fn eval(&self, e: &Expr) -> LogicWord {
        use eda_core::ir::{BinOp, UnOp};
        match e {
            Expr::Const(w) => w.clone(),
            Expr::Ref { net, range, .. } => {
                let (lsb, w) = match range {
                    Range::Bit(b) => (*b, 1),
                    Range::Span { msb, lsb } => (*lsb, msb - lsb + 1),
                };
                let mut bits = Vec::with_capacity(w);
                for i in 0..w {
                    bits.push(
                        self.vals
                            .get(net + lsb + i)
                            .copied()
                            .unwrap_or(LogicValue::X),
                    );
                }
                LogicWord { bits }
            }
            Expr::Un { op, a } => {
                let av = self.eval(a);
                match op {
                    UnOp::Not => av.map_bits(|b| b.not()),
                    UnOp::AndR => LogicWord {
                        bits: vec![av.reduce_and()],
                    },
                    UnOp::OrR => LogicWord {
                        bits: vec![av.reduce_or()],
                    },
                    UnOp::XorR => LogicWord {
                        bits: vec![av.reduce_xor()],
                    },
                }
            }
            Expr::Bin { op, a, b } => {
                let av = self.eval(a);
                let bv = self.eval(b);
                match op {
                    BinOp::And | BinOp::Or | BinOp::Xor | BinOp::Add | BinOp::Sub | BinOp::Mul => {
                        let w = av.width().max(bv.width());
                        let signed = a.is_signed() && b.is_signed();
                        let av = if signed { sext_word(&av, w) } else { av };
                        let bv = if signed { sext_word(&bv, w) } else { bv };
                        match op {
                            BinOp::And => av.zip_bits(&bv, |x, y| x.and(y)),
                            BinOp::Or => av.zip_bits(&bv, |x, y| x.or(y)),
                            BinOp::Xor => av.zip_bits(&bv, |x, y| x.xor(y)),
                            BinOp::Add => av.add(&bv),
                            BinOp::Sub => av.sub(&bv),
                            _ => av.mul(&bv),
                        }
                    }
                    BinOp::Shl => av.shl(&bv),
                    BinOp::Shr if a.is_signed() => {
                        if !bv.is_fully_known() {
                            LogicWord::xes(av.width())
                        } else {
                            let sh = bv.to_u64().unwrap_or(0) as usize;
                            let sign = av.bit(av.width() - 1);
                            if sh >= av.width() {
                                LogicWord::new_const(av.width(), sign)
                            } else {
                                let mut bits = vec![sign; av.width()];
                                bits[..av.width() - sh].copy_from_slice(&av.bits[sh..]);
                                LogicWord { bits }
                            }
                        }
                    }
                    BinOp::Shr => av.shr(&bv),
                    BinOp::Div | BinOp::Mod => {
                        if !av.is_fully_known() || !bv.is_fully_known() {
                            LogicWord::xes(av.width().max(bv.width()))
                        } else {
                            let w = av.width().max(bv.width());
                            let quotient = *op == BinOp::Div;
                            let result = if a.is_signed() && b.is_signed() {
                                let x = sext_i64(&av, w);
                                let y = sext_i64(&bv, w);
                                if y == 0 {
                                    None
                                } else if quotient {
                                    Some(x.wrapping_div(y) as u64)
                                } else {
                                    Some(x.wrapping_rem(y) as u64)
                                }
                            } else {
                                let x = zext_u64(&av, w);
                                let y = zext_u64(&bv, w);
                                if y == 0 {
                                    None
                                } else if quotient {
                                    Some(x / y)
                                } else {
                                    Some(x % y)
                                }
                            };
                            match result {
                                Some(value) => LogicWord::from_u64(w, value),
                                None => LogicWord::xes(w),
                            }
                        }
                    }
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        if !av.is_fully_known() || !bv.is_fully_known() {
                            LogicWord {
                                bits: vec![LogicValue::X],
                            }
                        } else {
                            let w = av.width().max(bv.width());
                            let signed = a.is_signed() && b.is_signed();
                            let r = match op {
                                // Equality must not collapse words wider than
                                // u64 (for example 512-bit strings).
                                BinOp::Eq | BinOp::Ne => {
                                    let x = if signed {
                                        sext_word(&av, w)
                                    } else {
                                        av.sized(w)
                                    };
                                    let y = if signed {
                                        sext_word(&bv, w)
                                    } else {
                                        bv.sized(w)
                                    };
                                    let equal = x == y;
                                    (*op == BinOp::Eq) == equal
                                }
                                _ => {
                                    let (x, y) = if signed {
                                        (sext_i64(&av, w), sext_i64(&bv, w))
                                    } else {
                                        (zext_u64(&av, w) as i64, zext_u64(&bv, w) as i64)
                                    };
                                    match op {
                                        BinOp::Lt => x < y,
                                        BinOp::Le => x <= y,
                                        BinOp::Gt => x > y,
                                        _ => x >= y,
                                    }
                                }
                            };
                            LogicWord {
                                bits: vec![LogicValue::bit(r)],
                            }
                        }
                    }
                    BinOp::CaseEq => LogicWord {
                        bits: vec![LogicValue::bit(case_eq_words(&av, &bv))],
                    },
                    BinOp::CaseNe => LogicWord {
                        bits: vec![LogicValue::bit(!case_eq_words(&av, &bv))],
                    },
                    BinOp::LogAnd => {
                        let x = LogicWord {
                            bits: vec![av.reduce_or()],
                        };
                        let y = LogicWord {
                            bits: vec![bv.reduce_or()],
                        };
                        x.zip_bits(&y, |a, b| a.and(b))
                    }
                    BinOp::LogOr => {
                        let x = LogicWord {
                            bits: vec![av.reduce_or()],
                        };
                        let y = LogicWord {
                            bits: vec![bv.reduce_or()],
                        };
                        x.zip_bits(&y, |a, b| a.or(b))
                    }
                }
            }
            Expr::Cond { c, t, f } => {
                let cv = self.eval(c);
                match cv.reduce_or().as_bool() {
                    Some(true) => self.eval(t),
                    Some(false) => self.eval(f),
                    None => {
                        let tv = self.eval(t);
                        let fv = self.eval(f);
                        tv.zip_bits(&fv, |x, y| if x == y { x } else { LogicValue::X })
                    }
                }
            }
            Expr::Concat(parts) => {
                let words: Vec<LogicWord> = parts.iter().map(|p| self.eval(p)).collect();
                let refs: Vec<&LogicWord> = words.iter().collect();
                LogicWord::concat(&refs)
            }
            Expr::Repeat { times, e } => {
                let w = self.eval(e);
                let refs: Vec<&LogicWord> = (0..*times).map(|_| &w).collect();
                LogicWord::concat(&refs)
            }
            Expr::Index { e, msb, lsb } => self.eval(e).slice(*msb, *lsb),
            Expr::Signed { e, .. } => self.eval(e),
        }
    }

    fn fire_monitors(&mut self) {
        if self.monitors.is_empty() {
            return;
        }
        let mut lines = Vec::new();
        let mut updates = Vec::new();
        for m in &self.monitors {
            let cur: Vec<LogicWord> = m.args.iter().map(|a| self.eval(a)).collect();
            let changed = m.last.len() != cur.len()
                || m.last.iter().zip(cur.iter()).any(|(a, b)| a.bits != b.bits);
            if changed {
                lines.push(self.format_display(m.parts, m.args));
            }
            updates.push(cur);
        }
        for (m, cur) in self.monitors.iter_mut().zip(updates) {
            m.last = cur;
        }
        for l in lines {
            if self.print_stdout {
                println!("{}", l);
            }
            self.displays.push(l);
        }
    }

    fn mark_vcd_change(&mut self, flat: usize) {
        if self.vcd.is_some() {
            if let Some(di) = decl_index_of(self.design, flat) {
                self.vcd_changes.insert(di);
            }
        }
    }

    fn flush_vcd(&mut self) {
        if let Some(vcd) = self.vcd.as_mut() {
            let changes = std::mem::take(&mut self.vcd_changes);
            vcd.advance(self.now, &self.vals, &changes);
        } else {
            self.vcd_changes.clear();
        }
    }
}

fn decl_index_of(d: &Design, flat: usize) -> Option<usize> {
    d.net_decls
        .iter()
        .position(|x| flat >= x.base && flat < x.base + x.width)
}

fn register_sens(sens: &Sens, pi: usize, body: &Stmt, out: &mut Vec<Vec<(usize, Edge)>>, n: usize) {
    match sens {
        Sens::Comb => {
            for net in stmt_nets(body) {
                if net < n {
                    out[net].push((pi, Edge::Any));
                }
            }
        }
        Sens::Posedge(net) => {
            if *net < n {
                out[*net].push((pi, Edge::Pos));
            }
        }
        Sens::Negedge(net) => {
            if *net < n {
                out[*net].push((pi, Edge::Neg));
            }
        }
        Sens::List(v) => {
            for s in v {
                match s {
                    Sens::Posedge(x) => {
                        if *x < n {
                            out[*x].push((pi, Edge::Pos));
                        }
                    }
                    Sens::Negedge(x) => {
                        if *x < n {
                            out[*x].push((pi, Edge::Neg));
                        }
                    }
                    _ => register_sens(s, pi, body, out, n),
                }
            }
        }
    }
}

fn zext_u64(w: &LogicWord, width: usize) -> u64 {
    let mut v = w.to_u64().unwrap_or(0);
    if width < 64 {
        v &= (1u64 << width) - 1;
    }
    v
}

fn sext_i64(w: &LogicWord, _width: usize) -> i64 {
    let original_width = w.width();
    let v = zext_u64(w, original_width);
    if original_width < 64 && w.bit(original_width - 1) == LogicValue::One {
        v as i64 - (1i64 << original_width)
    } else {
        v as i64
    }
}

fn sext_word(w: &LogicWord, width: usize) -> LogicWord {
    let mut bits = w.bits.clone();
    if let Some(&sign) = bits.last() {
        while bits.len() < width {
            bits.push(sign);
        }
    }
    LogicWord { bits }
}

fn case_eq_words(a: &LogicWord, b: &LogicWord) -> bool {
    let w = a.width().max(b.width());
    (0..w).all(|i| {
        let av = if i < a.width() {
            a.bit(i)
        } else {
            LogicValue::Zero
        };
        let bv = if i < b.width() {
            b.bit(i)
        } else {
            LogicValue::Zero
        };
        av == bv
    })
}

fn covers(lv: &LValue, net: usize) -> bool {
    lv_bits(lv).contains(&net)
}

fn lv_bit(lv: &LValue, net: usize) -> Option<usize> {
    lv_bits(lv).iter().position(|n| *n == net)
}

fn lv_bits(lv: &LValue) -> Vec<usize> {
    match &lv.range {
        Range::Bit(b) => vec![lv.net + b],
        Range::Span { msb, lsb } => (*lsb..=*msb).map(|i| lv.net + i).collect(),
    }
}

fn expr_nets(e: &Expr) -> Vec<usize> {
    let mut out = Vec::new();
    walk_expr(e, &mut out);
    out
}

fn walk_expr(e: &Expr, out: &mut Vec<usize>) {
    match e {
        Expr::Const(_) => {}
        Expr::Ref { net, range, .. } => match range {
            Range::Bit(b) => out.push(net + b),
            Range::Span { msb, lsb } => {
                for i in *lsb..=*msb {
                    out.push(net + i);
                }
            }
        },
        Expr::Un { a, .. } => walk_expr(a, out),
        Expr::Bin { a, b, .. } => {
            walk_expr(a, out);
            walk_expr(b, out);
        }
        Expr::Cond { c, t, f } => {
            walk_expr(c, out);
            walk_expr(t, out);
            walk_expr(f, out);
        }
        Expr::Concat(v) => v.iter().for_each(|x| walk_expr(x, out)),
        Expr::Repeat { e, .. } => walk_expr(e, out),
        Expr::Index { e, .. } => walk_expr(e, out),
        Expr::Signed { e, .. } => walk_expr(e, out),
    }
}

fn stmt_nets(s: &Stmt) -> Vec<usize> {
    let mut out = Vec::new();
    walk_stmt(s, &mut out);
    out
}

/// Conservative zero-delay feedback detector for `always @(*)`: does the
/// process both read and assign the same net without an intervening positive
/// delay? Catching this statically avoids burning the scheduler budget on a
/// guaranteed oscillation.
fn stmt_has_zero_delay_feedback(s: &Stmt) -> bool {
    let mut reads = HashSet::new();
    let mut writes = HashSet::new();
    collect_stmt_access(s, &mut reads, &mut writes);
    reads.intersection(&writes).next().is_some()
}

fn collect_stmt_access(s: &Stmt, reads: &mut HashSet<usize>, writes: &mut HashSet<usize>) {
    match s {
        Stmt::Block(v) => v.iter().for_each(|s| collect_stmt_access(s, reads, writes)),
        Stmt::Forever(b) => collect_stmt_access(b, reads, writes),
        Stmt::NBl { lv, e } | Stmt::Bl { lv, e } => {
            reads.extend(expr_nets(e));
            writes.extend(lv_bits(lv));
        }
        Stmt::NBlIndexed {
            net,
            decl_width,
            start,
            e,
            ..
        } => {
            reads.extend(expr_nets(start));
            reads.extend(expr_nets(e));
            writes.extend(*net..*net + *decl_width);
        }
        Stmt::BlIndexed {
            net,
            decl_width,
            start,
            e,
            ..
        } => {
            reads.extend(expr_nets(start));
            reads.extend(expr_nets(e));
            writes.extend(*net..*net + *decl_width);
        }
        Stmt::If { c, t, f } => {
            reads.extend(expr_nets(c));
            collect_stmt_access(t, reads, writes);
            if let Some(fs) = f {
                collect_stmt_access(fs, reads, writes);
            }
        }
        Stmt::Case { e, arms, dflt } => {
            reads.extend(expr_nets(e));
            for (label, body) in arms {
                reads.extend(expr_nets(label));
                collect_stmt_access(body, reads, writes);
            }
            if let Some(body) = dflt {
                collect_stmt_access(body, reads, writes);
            }
        }
        Stmt::Delay { ns, body } => {
            if *ns == 0 {
                collect_stmt_access(body, reads, writes);
            }
        }
        Stmt::Display { args, .. } => reads.extend(args.iter().flat_map(expr_nets)),
        Stmt::Assert { cond, .. } => reads.extend(expr_nets(cond)),
        Stmt::Assume { cond, .. } => reads.extend(expr_nets(cond)),
        Stmt::Cover { cond, .. } => reads.extend(expr_nets(cond)),
        Stmt::Finish => {}
    }
}

fn walk_stmt(s: &Stmt, out: &mut Vec<usize>) {
    match s {
        Stmt::Block(v) => v.iter().for_each(|x| walk_stmt(x, out)),
        Stmt::Forever(b) => walk_stmt(b, out),
        Stmt::NBl { e, .. } | Stmt::Bl { e, .. } => {
            walk_expr(e, out);
        }
        Stmt::NBlIndexed { start, e, .. } => {
            walk_expr(start, out);
            walk_expr(e, out);
        }
        Stmt::BlIndexed { start, e, .. } => {
            walk_expr(start, out);
            walk_expr(e, out);
        }
        Stmt::If { c, t, f } => {
            walk_expr(c, out);
            walk_stmt(t, out);
            if let Some(fs) = f {
                walk_stmt(fs, out);
            }
        }
        Stmt::Case { e, arms, dflt } => {
            walk_expr(e, out);
            for (l, b) in arms {
                walk_expr(l, out);
                walk_stmt(b, out);
            }
            if let Some(d) = dflt {
                walk_stmt(d, out);
            }
        }
        Stmt::Delay { body, .. } => walk_stmt(body, out),
        Stmt::Display { args, .. } => args.iter().for_each(|a| walk_expr(a, out)),
        Stmt::Assert { cond, .. } => walk_expr(cond, out),
        Stmt::Assume { cond, .. } => walk_expr(cond, out),
        Stmt::Cover { cond, .. } => walk_expr(cond, out),
        Stmt::Finish => {}
    }
}

fn collect_monitors<'d>(s: &'d Stmt, out: &mut Vec<(&'d [eda_core::ir::DispPart], &'d [Expr])>) {
    match s {
        Stmt::Display {
            parts,
            args,
            monitor,
        } if *monitor => {
            out.push((parts, args));
        }
        Stmt::Block(v) => v.iter().for_each(|x| collect_monitors(x, out)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_hdl::compile;

    fn sim_for(src: &str) -> Simulator<'static> {
        let d: &'static Design = Box::leak(Box::new(compile(src, None).unwrap()));
        Simulator::new(d).unwrap()
    }

    #[test]
    fn simulation_cancellation_reaches_delta_scheduling() {
        let mut sim = sim_for(
            "module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule",
        );
        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            sim.set_input_cancellable("d", &LogicWord::from_u64(1, 1), &cancellation)
                .unwrap_err(),
            "simulation cancelled"
        );
        assert_eq!(
            sim.run_until_cancellable(10, &cancellation).unwrap_err(),
            "simulation cancelled"
        );
        assert_eq!(
            sim.step_cancellable(&cancellation).unwrap_err(),
            "simulation cancelled"
        );
    }

    #[test]
    fn comb_assign_propagates() {
        let mut sim = sim_for("module m(input a, b, output y); assign y = a & b; endmodule");
        sim.set_input("a", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("b", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(1));
        sim.set_input("b", &LogicWord::from_u64(1, 0)).unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0));
    }

    #[test]
    fn adder_exhaustive() {
        let mut sim =
            sim_for("module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule");
        for x in 0..16u64 {
            for y in 0..16u64 {
                sim.set_input("a", &LogicWord::from_u64(4, x)).unwrap();
                sim.set_input("b", &LogicWord::from_u64(4, y)).unwrap();
                assert_eq!(sim.read("s").unwrap().to_u64(), Some((x + y) & 0xF));
            }
        }
    }

    #[test]
    fn x_pessimism() {
        let mut sim = sim_for("module m(input a, b, output y); assign y = a & b; endmodule");
        assert_eq!(sim.read("y").unwrap().bit(0), LogicValue::X);
        sim.set_input("a", &LogicWord::from_u64(1, 0)).unwrap();
        assert_eq!(sim.read("y").unwrap().bit(0), LogicValue::Zero);
    }

    #[test]
    fn mux_conditional() {
        let mut sim = sim_for(
            "module m(input s, [3:0] a, b, output [3:0] y); assign y = s ? a : b; endmodule",
        );
        sim.set_input("s", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("a", &LogicWord::from_u64(4, 0b1010)).unwrap();
        sim.set_input("b", &LogicWord::from_u64(4, 0b0101)).unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0b1010));
        sim.set_input("s", &LogicWord::from_u64(1, 0)).unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0b0101));
    }

    #[test]
    fn clocked_process_and_nba() {
        let mut sim = sim_for(
            "module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule",
        );
        sim.set_input("d", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(sim.read("q").unwrap().bit(0), LogicValue::X);
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(sim.read("q").unwrap().bit(0), LogicValue::One);
        sim.set_input("d", &LogicWord::from_u64(1, 0)).unwrap();
        assert_eq!(sim.read("q").unwrap().bit(0), LogicValue::One);
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(sim.read("q").unwrap().bit(0), LogicValue::Zero);
    }

    #[test]
    fn counter_sequence() {
        let mut sim = sim_for(
            "module m(input clk, rst, output reg [3:0] q);
                always @(posedge clk or posedge rst) if (rst) q <= 4'd0; else q <= q + 4'd1;
             endmodule",
        );
        sim.set_input("rst", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("rst", &LogicWord::from_u64(1, 0)).unwrap();
        for i in 0..10u64 {
            sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
            assert_eq!(sim.read("q").unwrap().to_u64(), Some(i + 1));
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        }
    }

    #[test]
    fn comb_always_case() {
        let mut sim = sim_for(
            "module m(input [1:0] sel, output reg [1:0] y);
                always @(*) case (sel)
                    2'b00: y = 2'd1;
                    2'b01: y = 2'd2;
                    2'b10: y = 2'd3;
                    default: y = 2'd0;
                endcase
             endmodule",
        );
        for s in 0..4u64 {
            sim.set_input("sel", &LogicWord::from_u64(2, s)).unwrap();
            let expect = match s {
                0 => 1,
                1 => 2,
                2 => 3,
                _ => 0,
            };
            assert_eq!(sim.read("y").unwrap().to_u64(), Some(expect));
        }
    }

    #[test]
    fn comb_always_case_with_continuous_loadpipe() {
        let mut sim = sim_for(
            "module m(input a, b, input [1:0] sel, output y);
                wire axb = a ^ b;
                reg r;
                always @(*) begin
                    r = axb;
                    case (sel)
                        2'b00: r = axb;
                        2'b01: r = a & b;
                        default: r = a | b;
                    endcase
                end
                assign y = r;
             endmodule",
        );
        for (a, b) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            sim.set_input("a", &LogicWord::from_u64(1, a)).unwrap();
            sim.set_input("b", &LogicWord::from_u64(1, b)).unwrap();
            for sel in 0..3u64 {
                sim.set_input("sel", &LogicWord::from_u64(2, sel)).unwrap();
                let expected = match sel {
                    0 => a ^ b,
                    1 => a & b,
                    _ => a | b,
                };
                assert_eq!(
                    sim.read_port("y").unwrap().to_u64(),
                    Some(expected),
                    "a={a} b={b} sel={sel}"
                );
            }
        }
    }

    #[test]
    fn testbench_display_and_finish() {
        let mut sim = sim_for(
            "module tb;
                reg a;
                initial begin
                    a = 0;
                    #10 a = 1;
                    #5 $display(\"a=%b t=%t\", a);
                    $display(\"done\");
                    $finish;
                end
             endmodule",
        );
        sim.run_until(100).unwrap();
        assert!(sim.finished());
        let out = sim.take_displays();
        assert!(out.iter().any(|l| l.starts_with("a=1 t=15")));
        assert!(out.iter().any(|l| l == "done"));
    }

    #[test]
    fn clock_generator_forever() {
        let mut sim = sim_for(
            "module tb; reg clk; initial begin clk = 0; forever #5 clk = ~clk; end endmodule",
        );
        sim.run_until(22).unwrap();
        // toggles at 5,10,15,20 -> after 22 clk is 0 (4 toggles)
        assert_eq!(sim.read("clk").unwrap().to_u64(), Some(0));
        assert_eq!(sim.now(), 22);
    }

    #[test]
    fn scheduler_step_budget_is_per_settle_not_cumulative() {
        let mut sim = sim_for(
            "module tb; reg clk; initial begin clk = 0; forever #1 clk = ~clk; end endmodule",
        );
        // More timestamp slots than MAX_STEPS_PER_TIME. Each individual settle
        // is tiny; treating the guard as a cumulative counter would incorrectly
        // report a zero-delay loop after long but legitimate activity.
        sim.run_until(MAX_STEPS_PER_TIME + 1).unwrap();
        assert_eq!(sim.now(), MAX_STEPS_PER_TIME + 1);
    }

    #[test]
    fn delays_and_time() {
        let mut sim = sim_for(
            "module tb; reg a; wire y; assign #7 y = a; initial begin #20 a = 1; end endmodule",
        );
        sim.run_until(19).unwrap();
        assert_eq!(sim.read("a").unwrap().bit(0), LogicValue::X);
        sim.run_until(26).unwrap();
        assert_eq!(sim.read("y").unwrap().bit(0), LogicValue::X);
        sim.run_until(27).unwrap();
        assert_eq!(sim.read("y").unwrap().bit(0), LogicValue::One);
    }

    #[test]
    fn monitor_fires_on_change() {
        let mut sim = sim_for(
            "module tb; reg a; initial a = 0; initial #10 a = 1; initial $monitor(\"a=%b\", a); endmodule",
        );
        sim.run_until(20).unwrap();
        assert!(sim.displays.iter().any(|l| l == "a=0"));
        assert!(sim.displays.iter().any(|l| l == "a=1"));
    }

    #[test]
    fn structural_gate_netlist_sim() {
        use eda_core::netlist::{Cell, CellKind, Dir, Netlist};
        use eda_core::to_design::netlist_to_design;
        let mut nl = Netlist::new("g");
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let w1 = nl.add_net("w1");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Xor,
            inputs: vec![a, b],
            output: w1,
        });
        nl.add_cell(Cell {
            name: "u2".into(),
            kind: CellKind::Not,
            inputs: vec![w1],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let d: &'static Design = Box::leak(Box::new(netlist_to_design(&nl)));
        let mut sim = Simulator::new(d).unwrap();
        for (x, y_) in [(0u64, 0u64), (0, 1), (1, 0), (1, 1)] {
            sim.set_input("a", &LogicWord::from_u64(1, x)).unwrap();
            sim.set_input("b", &LogicWord::from_u64(1, y_)).unwrap();
            let expect = ((x ^ y_) == 0) as u64;
            assert_eq!(sim.read_port("y").unwrap().to_u64(), Some(expect));
        }
    }

    #[test]
    fn fsm_state_sequence() {
        let src = "module fsm(input clk, rst, in, output reg [1:0] state, output reg out);
            always @(posedge clk or posedge rst) begin
                if (rst) state <= 2'b00;
                else case (state)
                    2'b00: state <= in ? 2'b01 : 2'b00;
                    2'b01: state <= in ? 2'b10 : 2'b00;
                    2'b10: state <= in ? 2'b11 : 2'b00;
                    2'b11: state <= 2'b00;
                endcase
            end
            always @(*) out = (state == 2'b10);
        endmodule";
        let mut sim = sim_for(src);
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("rst", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("in", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("rst", &LogicWord::from_u64(1, 0)).unwrap();
        let seq = [0u64, 1, 2, 3, 0];
        for (i, expect) in seq.iter().enumerate() {
            if i > 0 {
                sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
                sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            }
            assert_eq!(
                sim.read("state").unwrap().to_u64(),
                Some(*expect),
                "step {}",
                i
            );
        }
        assert_eq!(sim.read("out").unwrap().to_u64(), Some(0));
    }

    #[test]
    fn zero_delay_loop_detected() {
        let mut sim = sim_for("module m; reg y; initial y = 1'b0; always @(*) y = ~y; endmodule");
        assert!(sim.run_until(5).is_err());
    }

    #[test]
    fn hierarchical_flatten_sim() {
        let mut sim = sim_for(
            "module inv(input a, output y); assign y = ~a; endmodule
             module top(input x, output z); wire w; inv u1(.a(x), .y(w)); inv u2(.a(w), .y(z)); endmodule",
        );
        sim.set_input("x", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(sim.read_port("z").unwrap().to_u64(), Some(1));
        sim.set_input("x", &LogicWord::from_u64(1, 0)).unwrap();
        assert_eq!(sim.read_port("z").unwrap().to_u64(), Some(0));
    }

    #[test]
    fn hierarchical_clocked_nba() {
        let mut sim = sim_for(
            "module d(input clk, rst, output reg [1:0] q);
                always @(posedge clk) if (rst) q <= 2'd0; else q <= q + 2'd1;
             endmodule
             module top(input clk, rst, output [1:0] q); d u(.clk(clk), .rst(rst), .q(q)); endmodule",
        );
        sim.set_input("rst", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("rst", &LogicWord::from_u64(1, 0)).unwrap();
        for i in 0..4u64 {
            sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
            assert_eq!(sim.read_port("q").unwrap().to_u64(), Some((i + 1) & 3));
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        }
    }

    #[test]
    fn wide_hierarchical_vector_load_stays_within_step_budget() {
        let mut sim = sim_for(
            "module leaf(input [1023:0] p, output [1023:0] q); assign q = p; endmodule
             module top(input [1023:0] p, output [1023:0] q); leaf u(.p(p), .q(q)); endmodule",
        );
        let value = LogicWord {
            bits: (0..1024)
                .map(|i| {
                    if i % 3 == 0 {
                        LogicValue::One
                    } else {
                        LogicValue::Zero
                    }
                })
                .collect(),
        };
        sim.set_input("p", &value).unwrap();
        assert_eq!(sim.read_port("q"), Some(value));
    }

    #[test]
    fn clocked_case_with_localparam_label() {
        let mut sim = sim_for(
            "module m(input clk, rst, output reg [2:0] s);
                localparam A = 3'd0;
                always @(posedge clk)
                    if (rst) s <= 3'd0;
                    else case (s)
                        A: s <= 3'd1;
                        default: s <= 3'd0;
                    endcase
             endmodule",
        );
        sim.set_input("rst", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("rst", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(sim.read_port("s").unwrap().to_u64(), Some(1));
    }

    #[test]
    fn time_zero_initial_settles_before_input_edge() {
        let mut sim = sim_for(
            "module m(input clk, output reg [3:0] n);
                initial n = 4'd0;
                always @(posedge clk) n <= n + 4'd1;
             endmodule",
        );
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(sim.read_port("n").unwrap().to_u64(), Some(1));
        sim.set_input("clk", &LogicWord::xes(1)).unwrap();
        assert_eq!(sim.read_port("n").unwrap().to_u64(), Some(1));
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(sim.read_port("n").unwrap().to_u64(), Some(2));
    }

    #[test]
    fn shifts_and_compares() {
        let mut sim = sim_for(
            "module m(input [3:0] a, [1:0] sh, output [3:0] l, r, output lt, eq);
                assign l = a << sh;
                assign r = a >> sh;
                assign lt = a < 4'd9;
                assign eq = (a == 4'd9);
             endmodule",
        );
        sim.set_input("a", &LogicWord::from_u64(4, 0b1001)).unwrap();
        sim.set_input("sh", &LogicWord::from_u64(2, 1)).unwrap();
        assert_eq!(sim.read("l").unwrap().to_u64(), Some(0b0010));
        assert_eq!(sim.read("r").unwrap().to_u64(), Some(0b0100));
        assert_eq!(sim.read("lt").unwrap().to_u64(), Some(0));
        assert_eq!(sim.read("eq").unwrap().to_u64(), Some(1));
    }

    #[test]
    fn signed_declarations_literals_arithmetic_and_shift() {
        let mut sim = sim_for(
            "module m(input logic signed [7:0] a, b, \
             output logic signed [7:0] sum, shifted, quotient, remainder, \
             output logic less);
                assign sum = a + b;
                assign shifted = a >> 1;
                assign quotient = a / b;
                assign remainder = a % b;
                assign less = a < b;
             endmodule",
        );
        sim.set_input("a", &LogicWord::from_u64(8, 0xf9)).unwrap();
        sim.set_input("b", &LogicWord::from_u64(8, 2)).unwrap();
        assert_eq!(sim.read("sum").unwrap().to_u64(), Some(0xfb));
        assert_eq!(sim.read("shifted").unwrap().to_u64(), Some(0xfc));
        assert_eq!(sim.read("quotient").unwrap().to_u64(), Some(0xfd));
        assert_eq!(sim.read("remainder").unwrap().to_u64(), Some(0xff));
        assert_eq!(sim.read("less").unwrap().to_u64(), Some(1));

        let mut sim = sim_for(
            "module m(output logic [7:0] y, output logic less);
                assign y = $signed(8'sd250) + $signed(8'sd7);
                assign less = $signed(8'sh80) < $signed(8'sd0);
             endmodule",
        );
        sim.run_until(1).unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(1));
        assert_eq!(sim.read("less").unwrap().to_u64(), Some(1));
    }

    #[test]
    fn concurrent_property_samples_on_clock_edge() {
        let mut sim = sim_for(
            "module m(input clk, req, ready, output grant);
                assign grant = req & ready;
                assert property (@(posedge clk) req |-> grant)
                    else $error(\"request must be granted\");
             endmodule",
        );
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("ready", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("ready", &LogicWord::from_u64(1, 0)).unwrap();
        let error = sim
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap_err();
        assert!(error.contains("assertion failed"));
        assert!(error.contains("request must be granted"));
    }

    #[test]
    fn nonoverlapping_property_checks_the_next_sample() {
        let source = "module m(input clk, req, grant);
            assert property (@(posedge clk) req |=> grant)
                else $error(\"grant must follow request\");
         endmodule";

        let mut passing = sim_for(source);
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("grant", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("req", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("grant", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();

        let mut failing = sim_for(source);
        failing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        failing
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        failing
            .set_input("grant", &LogicWord::from_u64(1, 0))
            .unwrap();
        failing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        failing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        failing
            .set_input("req", &LogicWord::from_u64(1, 0))
            .unwrap();
        let error = failing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap_err();
        assert!(error.contains("grant must follow request"));
    }

    #[test]
    fn sequence_composition_properties_execute() {
        let mut and_property = sim_for(
            "module m(input clk, req, first, second);
                assert property (@(posedge clk) req |-> first and second)
                    else $error(\"both sequences must match\");
             endmodule",
        );
        and_property
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        and_property
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        and_property
            .set_input("first", &LogicWord::from_u64(1, 1))
            .unwrap();
        and_property
            .set_input("second", &LogicWord::from_u64(1, 1))
            .unwrap();
        and_property
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        and_property
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        and_property
            .set_input("first", &LogicWord::from_u64(1, 0))
            .unwrap();
        let error = and_property
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap_err();
        assert!(error.contains("both sequences must match"));

        let mut or_property = sim_for(
            "module m(input clk, req, first, second);
                assert property (@(posedge clk) req |-> first or second)
                    else $error(\"one sequence must match\");
             endmodule",
        );
        or_property
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        or_property
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        or_property
            .set_input("first", &LogicWord::from_u64(1, 0))
            .unwrap();
        or_property
            .set_input("second", &LogicWord::from_u64(1, 1))
            .unwrap();
        or_property
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        or_property
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        or_property
            .set_input("first", &LogicWord::from_u64(1, 0))
            .unwrap();
        or_property
            .set_input("second", &LogicWord::from_u64(1, 0))
            .unwrap();
        let error = or_property
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap_err();
        assert!(error.contains("one sequence must match"));

        let mut delayed_sequence = sim_for(
            "module m(input clk, req, first, second);
                assert property (@(posedge clk) req |-> first ##[2] second)
                    else $error(\"delayed sequence must match\");
             endmodule",
        );
        delayed_sequence
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        delayed_sequence
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        delayed_sequence
            .set_input("first", &LogicWord::from_u64(1, 1))
            .unwrap();
        delayed_sequence
            .set_input("second", &LogicWord::from_u64(1, 0))
            .unwrap();
        delayed_sequence
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        for edge in 0..2 {
            delayed_sequence
                .set_input("clk", &LogicWord::from_u64(1, 0))
                .unwrap();
            delayed_sequence
                .set_input("req", &LogicWord::from_u64(1, 0))
                .unwrap();
            delayed_sequence
                .set_input("first", &LogicWord::from_u64(1, 0))
                .unwrap();
            delayed_sequence
                .set_input("second", &LogicWord::from_u64(1, u64::from(edge == 1)))
                .unwrap();
            delayed_sequence
                .set_input("clk", &LogicWord::from_u64(1, 1))
                .unwrap();
        }

        let mut delayed_or = sim_for(
            "module m(input clk, req, first, second);
                assert property (@(posedge clk) req |-> first or ##[2] second)
                    else $error(\"delayed alternative must match\");
             endmodule",
        );
        delayed_or
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        delayed_or
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        delayed_or
            .set_input("first", &LogicWord::from_u64(1, 0))
            .unwrap();
        delayed_or
            .set_input("second", &LogicWord::from_u64(1, 0))
            .unwrap();
        delayed_or
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        for edge in 0..2 {
            delayed_or
                .set_input("clk", &LogicWord::from_u64(1, 0))
                .unwrap();
            delayed_or
                .set_input("req", &LogicWord::from_u64(1, 0))
                .unwrap();
            delayed_or
                .set_input("first", &LogicWord::from_u64(1, 0))
                .unwrap();
            delayed_or
                .set_input("second", &LogicWord::from_u64(1, u64::from(edge == 1)))
                .unwrap();
            delayed_or
                .set_input("clk", &LogicWord::from_u64(1, 1))
                .unwrap();
        }

        let mut delayed_cover = sim_for(
            "module m(input clk, req, first, second);
                composed: cover property (@(posedge clk) req |-> ##[2] first and second);
             endmodule",
        );
        delayed_cover
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        delayed_cover
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        delayed_cover
            .set_input("first", &LogicWord::from_u64(1, 0))
            .unwrap();
        delayed_cover
            .set_input("second", &LogicWord::from_u64(1, 0))
            .unwrap();
        delayed_cover
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        for edge in 0..2 {
            delayed_cover
                .set_input("clk", &LogicWord::from_u64(1, 0))
                .unwrap();
            delayed_cover
                .set_input("req", &LogicWord::from_u64(1, 0))
                .unwrap();
            let final_edge = edge == 1;
            delayed_cover
                .set_input("first", &LogicWord::from_u64(1, u64::from(final_edge)))
                .unwrap();
            delayed_cover
                .set_input("second", &LogicWord::from_u64(1, u64::from(final_edge)))
                .unwrap();
            delayed_cover
                .set_input("clk", &LogicWord::from_u64(1, 1))
                .unwrap();
        }
        assert_eq!(delayed_cover.property_cover_hits("composed"), 1);
    }

    #[test]
    fn assume_and_cover_properties_execute() {
        let mut assumption = sim_for(
            "module m(input clk, req, grant);
                assume property (@(posedge clk) req |-> grant)
                    else $error(\"grant is required\");
             endmodule",
        );
        assumption
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        assumption
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        assumption
            .set_input("grant", &LogicWord::from_u64(1, 0))
            .unwrap();
        let error = assumption
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap_err();
        assert!(error.starts_with("assumption failed"));
        assert!(error.contains("grant is required"));

        let mut negedge_assumption = sim_for(
            "module m(input clk, req, grant);
                assume property (@(negedge clk) req |-> grant);
             endmodule",
        );
        negedge_assumption
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        negedge_assumption
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        negedge_assumption
            .set_input("grant", &LogicWord::from_u64(1, 0))
            .unwrap();
        let error = negedge_assumption
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap_err();
        assert!(error.starts_with("assumption failed"));

        let mut cover = sim_for(
            "module m(input clk, req, grant);
                sample_grant: cover property (@(posedge clk) req |=> grant);
             endmodule",
        );
        cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        cover.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
        cover
            .set_input("grant", &LogicWord::from_u64(1, 0))
            .unwrap();
        cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(cover.property_cover_hits("sample_grant"), 0);
        cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        cover.set_input("req", &LogicWord::from_u64(1, 0)).unwrap();
        cover
            .set_input("grant", &LogicWord::from_u64(1, 1))
            .unwrap();
        cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(cover.property_cover_hits("sample_grant"), 1);
    }

    #[test]
    fn fixed_delay_property_checks_after_specified_edges() {
        let source = "module m(input clk, req, grant);
            assert property (@(posedge clk) req |-> ##[2] grant)
                else $error(\"grant must arrive two edges later\");
         endmodule";

        let mut passing = sim_for(source);
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("grant", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("req", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("grant", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();

        let mut failing = sim_for(source);
        failing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        failing
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        failing
            .set_input("grant", &LogicWord::from_u64(1, 0))
            .unwrap();
        failing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        failing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        failing
            .set_input("req", &LogicWord::from_u64(1, 0))
            .unwrap();
        failing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        failing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        let error = failing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap_err();
        assert!(error.contains("grant must arrive two edges later"));
    }

    #[test]
    fn delay_range_property_accepts_any_cycle_in_window() {
        let source = "module m(input clk, req, grant);
            assert property (@(posedge clk) req |-> ##[1:3] grant)
                else $error(\"grant must arrive within three edges\");
         endmodule";

        let mut passing = sim_for(source);
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("grant", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("req", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("grant", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("grant", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();

        let mut failing = sim_for(source);
        failing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        failing
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        failing
            .set_input("grant", &LogicWord::from_u64(1, 0))
            .unwrap();
        for _ in 0..3 {
            failing
                .set_input("clk", &LogicWord::from_u64(1, 1))
                .unwrap();
            failing
                .set_input("clk", &LogicWord::from_u64(1, 0))
                .unwrap();
        }
        let error = failing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap_err();
        assert!(error.contains("grant must arrive within three edges"));
    }

    #[test]
    fn consecutive_repetition_requires_successive_samples() {
        let source = "module m(input clk, req, grant);
            assert property (@(posedge clk) req |-> grant[*3])
                else $error(\"grant must remain high for three edges\");
         endmodule";

        let mut passing = sim_for(source);
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("grant", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        for _ in 0..2 {
            passing
                .set_input("clk", &LogicWord::from_u64(1, 0))
                .unwrap();
            passing
                .set_input("req", &LogicWord::from_u64(1, 0))
                .unwrap();
            passing
                .set_input("grant", &LogicWord::from_u64(1, 1))
                .unwrap();
            passing
                .set_input("clk", &LogicWord::from_u64(1, 1))
                .unwrap();
        }
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("grant", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();

        let mut failing = sim_for(source);
        failing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        failing
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        failing
            .set_input("grant", &LogicWord::from_u64(1, 1))
            .unwrap();
        failing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        for grant in [1, 0] {
            failing
                .set_input("clk", &LogicWord::from_u64(1, 0))
                .unwrap();
            failing
                .set_input("req", &LogicWord::from_u64(1, 0))
                .unwrap();
            failing
                .set_input("grant", &LogicWord::from_u64(1, grant))
                .unwrap();
            if grant == 0 {
                let error = failing
                    .set_input("clk", &LogicWord::from_u64(1, 1))
                    .unwrap_err();
                assert!(error.contains("grant must remain high for three edges"));
            } else {
                failing
                    .set_input("clk", &LogicWord::from_u64(1, 1))
                    .unwrap();
            }
        }
    }

    #[test]
    fn repetition_range_matches_at_minimum_consecutive_count() {
        let source = "module m(input clk, req, grant);
            assert property (@(posedge clk) req |-> grant[*2:3])
                else $error(\"grant needs two successive edges\");
         endmodule";

        let mut passing = sim_for(source);
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("grant", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("req", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("grant", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();

        let mut failing = sim_for(source);
        failing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        failing
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        failing
            .set_input("grant", &LogicWord::from_u64(1, 1))
            .unwrap();
        failing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        failing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        failing
            .set_input("req", &LogicWord::from_u64(1, 0))
            .unwrap();
        failing
            .set_input("grant", &LogicWord::from_u64(1, 0))
            .unwrap();
        let error = failing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap_err();
        assert!(error.contains("grant needs two successive edges"));
    }

    #[test]
    fn goto_repetition_counts_nonconsecutive_samples() {
        let source = "module m(input clk, req, grant);
            assert property (@(posedge clk) req |-> grant[->3])
                else $error(\"grant must be seen three times\");
         endmodule";

        let mut passing = sim_for(source);
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("req", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("grant", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        for grant in [0, 1, 0, 1] {
            passing
                .set_input("clk", &LogicWord::from_u64(1, 0))
                .unwrap();
            passing
                .set_input("req", &LogicWord::from_u64(1, 0))
                .unwrap();
            passing
                .set_input("grant", &LogicWord::from_u64(1, grant))
                .unwrap();
            passing
                .set_input("clk", &LogicWord::from_u64(1, 1))
                .unwrap();
        }

        let mut cover = sim_for(
            "module m(input clk, req, grant);
                seen_three: cover property (@(posedge clk) req |-> grant[->3]);
             endmodule",
        );
        cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        cover.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
        cover
            .set_input("grant", &LogicWord::from_u64(1, 1))
            .unwrap();
        cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(cover.property_cover_hits("seen_three"), 0);
        for grant in [0, 1, 0, 1] {
            cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            cover.set_input("req", &LogicWord::from_u64(1, 0)).unwrap();
            cover
                .set_input("grant", &LogicWord::from_u64(1, grant))
                .unwrap();
            cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        }
        assert_eq!(cover.property_cover_hits("seen_three"), 1);
    }

    #[test]
    fn goto_repetition_range_matches_at_minimum_count() {
        let source = "module m(input clk, req, grant);
            assert property (@(posedge clk) req |-> grant[->2:4])
                else $error(\"grant must be seen twice to four times\");
         endmodule";

        let mut sim = sim_for(source);
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        for grant in [0, 1] {
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            sim.set_input("req", &LogicWord::from_u64(1, 0)).unwrap();
            sim.set_input("grant", &LogicWord::from_u64(1, grant))
                .unwrap();
            sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        }

        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("grant", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

        let mut cover = sim_for(
            "module m(input clk, req, grant);
                range_seen: cover property (@(posedge clk) req |-> grant[->2:4]);
             endmodule",
        );
        cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        cover.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
        cover
            .set_input("grant", &LogicWord::from_u64(1, 1))
            .unwrap();
        cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(cover.property_cover_hits("range_seen"), 0);
        cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        cover.set_input("req", &LogicWord::from_u64(1, 0)).unwrap();
        cover
            .set_input("grant", &LogicWord::from_u64(1, 0))
            .unwrap();
        cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        cover
            .set_input("grant", &LogicWord::from_u64(1, 1))
            .unwrap();
        cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(cover.property_cover_hits("range_seen"), 1);
    }

    #[test]
    fn nonconsecutive_repetition_matches_and_retires() {
        let mut sim = sim_for(
            "module m(input clk, req, grant);
                assert property (@(posedge clk) req |-> grant[=2:4])
                    else $error(\"grant must be nonconsecutively seen twice\");
             endmodule",
        );
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("req", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("grant", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

        for _ in 0..3 {
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
            sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        }

        let mut cover = sim_for(
            "module m(input clk, req, grant);
                nonconsecutive_seen: cover property (@(posedge clk) req |-> grant[=2]);
             endmodule",
        );
        cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        cover.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
        cover
            .set_input("grant", &LogicWord::from_u64(1, 1))
            .unwrap();
        cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(cover.property_cover_hits("nonconsecutive_seen"), 0);
        cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        cover.set_input("req", &LogicWord::from_u64(1, 0)).unwrap();
        cover
            .set_input("grant", &LogicWord::from_u64(1, 0))
            .unwrap();
        cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        cover
            .set_input("grant", &LogicWord::from_u64(1, 1))
            .unwrap();
        cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(cover.property_cover_hits("nonconsecutive_seen"), 1);
    }

    #[test]
    fn interface_array_elements_remain_distinct() {
        let mut sim = sim_for(
            "interface bus;
                logic [7:0] data;
                logic valid;
                modport source(output valid, data);
                modport sink(input valid, data);
             endinterface

             module producer(interface bus.source b, input logic [7:0] value);
                assign b.valid = 1'b1;
                assign b.data = value;
             endmodule

             module consumer(interface bus.sink b, output logic [7:0] result);
                assign result = b.valid ? b.data : 8'd0;
             endmodule

             module top(input logic [7:0] first, second,
                 output logic [7:0] first_result, second_result);
                bus shared[0:1]();
                producer p0(.b(shared[0]), .value(first));
                producer p1(.b(shared[1]), .value(second));
                consumer c0(.b(shared[0]), .result(first_result));
                consumer c1(.b(shared[1]), .result(second_result));
             endmodule",
        );
        assert_eq!(
            sim.design
                .hierarchy
                .iter()
                .filter(|instance| instance.path.ends_with("shared[0]")
                    || instance.path.ends_with("shared[1]"))
                .count(),
            2
        );
        sim.set_input("first", &LogicWord::from_u64(8, 0x3c))
            .unwrap();
        sim.set_input("second", &LogicWord::from_u64(8, 0xa5))
            .unwrap();
        assert_eq!(sim.read("first_result").unwrap().to_u64(), Some(0x3c));
        assert_eq!(sim.read("second_result").unwrap().to_u64(), Some(0xa5));
    }

    #[test]
    fn multidimensional_interface_array_elements_remain_distinct() {
        let mut sim = sim_for(
            "interface bus;
                logic [7:0] data;
                logic valid;
                modport source(output valid, data);
                modport sink(input valid, data);
             endinterface

             module producer(interface bus.source b, input logic [7:0] value);
                assign b.valid = 1'b1;
                assign b.data = value;
             endmodule

             module consumer(interface bus.sink b, output logic [7:0] result);
                assign result = b.valid ? b.data : 8'd0;
             endmodule

             module top(input logic [7:0] first, second,
                 output logic [7:0] first_result, second_result);
                bus shared[0:1][0:2]();
                producer p0(.b(shared[0][1]), .value(first));
                producer p1(.b(shared[1][2]), .value(second));
                consumer c0(.b(shared[0][1]), .result(first_result));
                consumer c1(.b(shared[1][2]), .result(second_result));
             endmodule",
        );
        assert_eq!(
            sim.design
                .hierarchy
                .iter()
                .filter(|instance| instance.module == "bus")
                .count(),
            6
        );
        sim.set_input("first", &LogicWord::from_u64(8, 0x47))
            .unwrap();
        sim.set_input("second", &LogicWord::from_u64(8, 0xb9))
            .unwrap();
        assert_eq!(sim.read("first_result").unwrap().to_u64(), Some(0x47));
        assert_eq!(sim.read("second_result").unwrap().to_u64(), Some(0xb9));
    }

    #[test]
    fn module_instance_array_slices_vector_connections() {
        let mut sim = sim_for(
            "module leaf(input a, b, output y);
                assign y = a & b;
             endmodule

             module top(input [3:0] a, b, output [3:0] y);
                leaf u[0:3](.a(a), .b(b), .y(y));
             endmodule",
        );
        assert_eq!(
            sim.design
                .hierarchy
                .iter()
                .filter(|instance| instance.module == "leaf")
                .count(),
            4
        );
        sim.set_input("a", &LogicWord::from_u64(4, 0b1101)).unwrap();
        sim.set_input("b", &LogicWord::from_u64(4, 0b1011)).unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0b1001));

        let mut vector_sim = sim_for(
            "module leaf(input [1:0] a, output [1:0] y);
                assign y = ~a;
             endmodule

             module top(input [3:0] a, output [3:0] y);
                leaf u[0:1](.a(a), .y(y));
             endmodule",
        );
        vector_sim
            .set_input("a", &LogicWord::from_u64(4, 0b0110))
            .unwrap();
        assert_eq!(vector_sim.read("y").unwrap().to_u64(), Some(0b1001));
    }

    #[test]
    fn multidimensional_module_array_slices_row_major_connections() {
        let mut sim = sim_for(
            "module leaf(input [1:0] a, output [1:0] y);
                assign y = ~a;
             endmodule

             module top(input [11:0] a, output [11:0] y);
                leaf u[0:1][0:2](.a(a), .y(y));
             endmodule",
        );
        assert_eq!(
            sim.design
                .hierarchy
                .iter()
                .filter(|instance| instance.module == "leaf")
                .count(),
            6
        );
        sim.set_input("a", &LogicWord::from_u64(12, 0b011010100110))
            .unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0b100101011001));

        let mut descending = sim_for(
            "module leaf(input a, output y);
                assign y = ~a;
             endmodule

             module top(input [3:0] a, output [3:0] y);
                leaf u[1:0][1:0](.a(a), .y(y));
             endmodule",
        );
        descending
            .set_input("a", &LogicWord::from_u64(4, 0b0110))
            .unwrap();
        assert_eq!(descending.read("y").unwrap().to_u64(), Some(0b1001));
    }

    #[test]
    fn module_unpacked_net_array_elements_update_independently() {
        let mut sim = sim_for(
            "module top(input [7:0] a, b, output [31:0] y);
                logic [7:0] memory [0:3];
                assign memory[0] = a & b;
                assign memory[1] = a | b;
                assign memory[2] = a ^ b;
                assign memory[3] = a + b;
                assign y = {memory[0], memory[1], memory[2], memory[3]};
             endmodule",
        );
        let declaration = sim
            .design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "memory")
            .expect("memory declaration");
        assert_eq!(declaration.width, 32);
        sim.set_input("a", &LogicWord::from_u64(8, 0x5a)).unwrap();
        sim.set_input("b", &LogicWord::from_u64(8, 0xa5)).unwrap();
        let expected = ((0x5a_u64 + 0xa5) & 0xff) | ((0x5a ^ 0xa5) << 8) | ((0x5a | 0xa5) << 16);
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(expected));

        let mut descending = sim_for(
            "module top(input [7:0] a, output [15:0] y);
                logic [7:0] memory [3:0];
                assign memory[0] = a;
                assign memory[3] = ~a;
                assign y = {memory[3], memory[0]};
             endmodule",
        );
        descending
            .set_input("a", &LogicWord::from_u64(8, 0x3c))
            .unwrap();
        assert_eq!(
            descending.read("y").unwrap().to_u64(),
            Some((!0x3c_u64 & 0xff) << 8 | 0x3c)
        );
    }

    #[test]
    fn module_multidimensional_unpacked_net_array_selects_elements() {
        let mut sim = sim_for(
            "module top(input [7:0] a, b, output [47:0] y);
                logic [7:0] memory [0:1][0:2];
                assign memory[0][0] = a & b;
                assign memory[0][1] = a | b;
                assign memory[0][2] = a ^ b;
                assign memory[1][0] = a + b;
                assign memory[1][1] = a - b;
                assign memory[1][2] = ~a;
                assign y = {
                    memory[0][0], memory[0][1], memory[0][2],
                    memory[1][0], memory[1][1], memory[1][2]
                };
             endmodule",
        );
        let declaration = sim
            .design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "memory")
            .expect("memory declaration");
        assert_eq!(declaration.width, 48);
        sim.set_input("a", &LogicWord::from_u64(8, 0x5a)).unwrap();
        sim.set_input("b", &LogicWord::from_u64(8, 0xa5)).unwrap();
        let expected = ((0x5a_u64 | 0xa5) << 32)
            | ((0x5a ^ 0xa5) << 24)
            | (((0x5a + 0xa5) & 0xff) << 16)
            | ((((0x5a + 0x100) - 0xa5) & 0xff) << 8)
            | (!0x5a_u64 & 0xff);
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(expected));
    }

    #[test]
    fn unpacked_array_elements_support_indexed_part_selects() {
        let mut sim = sim_for(
            "module top(input logic clk, input logic [3:0] selector, \
                          input logic [1:0] element_selector, \
                          input logic [3:0] a, output logic [3:0] y, z, q, dynamic); \
                 logic [7:0] memory [0:3]; \
                 initial begin \
                     memory[1] = 8'ha5; \
                     memory[2] = 8'h3c; \
                 end \
                 assign y = memory[1][selector +: 4]; \
                 assign memory[0][3 -: 2] = a & 2'd3; \
                 assign q = memory[0][3 -: 2]; \
                 always @(posedge clk) \
                     memory[2][selector -: 4] <= a ^ 4'd3; \
                 assign z = memory[2][selector -: 4]; \
                 assign dynamic = memory[element_selector][selector +: 4]; \
             endmodule",
        );
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("selector", &LogicWord::from_u64(4, 4))
            .unwrap();
        sim.set_input("element_selector", &LogicWord::from_u64(2, 1))
            .unwrap();
        sim.set_input("a", &LogicWord::from_u64(4, 0xc)).unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0xa));
        assert_eq!(sim.read("dynamic").unwrap().to_u64(), Some(0xa));
        assert_eq!(sim.read("q").unwrap().to_u64(), Some(0x0));
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(sim.read("z").unwrap().to_u64(), Some(0xf));
        sim.set_input("element_selector", &LogicWord::from_u64(2, 2))
            .unwrap();
        assert_eq!(sim.read("dynamic").unwrap().to_u64(), Some(0x3));
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("selector", &LogicWord::from_u64(4, 13))
            .unwrap();
        assert!(sim.read("y").unwrap().to_u64().is_none());
        sim.set_input("element_selector", &LogicWord::from_u64(2, 3))
            .unwrap();
        assert!(sim.read("dynamic").unwrap().to_u64().is_none());
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert!(sim.read("z").unwrap().to_u64().is_none());
    }

    #[test]
    fn fixed_unpacked_array_ports_connect_as_flat_vectors() {
        let mut sim = sim_for(
            "module child(
                input  logic [7:0] in_mem  [0:3],
                output logic [7:0] out_mem [0:3]
            );
                assign out_mem[0] = in_mem[0] + 8'd1;
                assign out_mem[1] = in_mem[1] & 8'hff;
                assign out_mem[2] = in_mem[2] | 8'h0f;
                assign out_mem[3] = in_mem[3] ^ 8'h5a;
             endmodule

             module top(input logic [31:0] a, output logic [31:0] y);
                child c(.in_mem(a), .out_mem(y));
             endmodule",
        );
        let child_input = sim
            .design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "c.in_mem")
            .expect("child array input declaration");
        assert_eq!(child_input.width, 32);
        let child_output = sim
            .design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "c.out_mem")
            .expect("child array output declaration");
        assert_eq!(child_output.width, 32);
        let child_ports = sim
            .design
            .hierarchy
            .iter()
            .find(|instance| instance.module == "child")
            .expect("child hierarchy record");
        assert_eq!(child_ports.ports.len(), 2);
        assert!(child_ports.ports.iter().all(|port| port.width == 32));

        sim.set_input("a", &LogicWord::from_u64(32, 0x4030_2010))
            .unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x1a3f_2011));
    }

    #[test]
    fn non_ansi_unpacked_array_port_merges_body_declaration() {
        let mut sim = sim_for(
            "module top(mem, y);
                input logic [7:0] mem [3:0];
                output logic [31:0] y;
                assign y = {mem[3], mem[2], mem[1], mem[0]};
             endmodule",
        );
        let mem = sim
            .design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "mem")
            .expect("non-ANSI array port declaration");
        assert_eq!(mem.width, 32);
        assert_eq!(
            sim.design
                .ports
                .iter()
                .find(|port| port.name == "mem")
                .expect("non-ANSI array top port")
                .width,
            32
        );
        sim.set_input("mem", &LogicWord::from_u64(32, 0x4030_2010))
            .unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x1020_3040));
    }

    #[test]
    fn module_instance_arrays_slice_child_array_ports() {
        let mut sim = sim_for(
            "module child(input logic [7:0] in_mem [0:1], \
                          output logic [7:0] out_mem [0:1]); \
                 assign out_mem[0] = in_mem[0] + 8'd1; \
                 assign out_mem[1] = in_mem[1] ^ 8'h0f; \
             endmodule \
             module top(input logic [31:0] a, output logic [31:0] y); \
                 child c[0:1](.in_mem(a), .out_mem(y)); \
             endmodule",
        );
        let children = sim
            .design
            .hierarchy
            .iter()
            .filter(|instance| instance.module == "child")
            .count();
        assert_eq!(children, 2);
        assert!(sim
            .design
            .hierarchy
            .iter()
            .filter(|instance| instance.module == "child")
            .all(|instance| instance.ports.len() == 2
                && instance.ports.iter().all(|port| port.width == 16)));
        sim.set_input("a", &LogicWord::from_u64(32, 0x3020_1000))
            .unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x3f21_1f01));
    }

    #[test]
    fn unpacked_array_slices_connect_array_ports() {
        let mut sim = sim_for(
            "module child(input logic [7:0] in_mem [0:1], \
                          output logic [7:0] out_mem [0:1]); \
                 assign out_mem[0] = in_mem[0] + 8'd1; \
                 assign out_mem[1] = in_mem[1] ^ 8'h0f; \
             endmodule \
             module top(input logic [31:0] a, output logic [31:0] y); \
                 logic [7:0] source [0:3]; \
                 logic [7:0] result [0:3]; \
                 assign source = a; \
                 initial result = 32'd0; \
                 child c(.in_mem(source[1:2]), .out_mem(result[2:3])); \
                 assign y = result; \
             endmodule",
        );
        sim.set_input("a", &LogicWord::from_u64(32, 0x4030_2010))
            .unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x3f21_0000));

        let error = compile(
            "module top(input logic [7:0] a, output logic [15:0] y); \
                 logic [7:0] source [3:0]; \
                 assign source = {a, a, a, a}; \
                 assign y = source[1:2]; \
             endmodule",
            None,
        )
        .unwrap_err();
        assert!(
            error.contains("does not follow declaration direction"),
            "{error}"
        );
    }

    #[test]
    fn indexed_part_selects_read_and_write_contiguous_ranges() {
        let mut sim = sim_for(
            "module top(input logic [15:0] a, output logic [15:0] y); \
                 always @(*) begin \
                     y = 16'd0; \
                     y[6 +: 4] = a[11 -: 4] ^ 4'd3; \
                     y[5 -: 4] = a[8 +: 4] & 4'd5; \
                 end \
             endmodule",
        );
        sim.set_input("a", &LogicWord::from_u64(16, 0x9c3f))
            .unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x03d0));

        let error = compile(
            "module top(input logic [7:0] a, output logic [7:0] y); \
                 assign y = a[6 +: 0]; \
             endmodule",
            None,
        )
        .unwrap_err();
        assert!(error.contains("width must be positive"), "{error}");
    }

    #[test]
    fn runtime_indexed_part_select_reads_use_variable_start() {
        let mut sim = sim_for(
            "module top(input logic [15:0] a, input logic [3:0] selector, \
                          output logic [3:0] plus, minus); \
                 assign plus = a[selector +: 4]; \
                 assign minus = a[selector -: 4]; \
             endmodule",
        );
        sim.set_input("a", &LogicWord::from_u64(16, 0xa4c7))
            .unwrap();
        sim.set_input("selector", &LogicWord::from_u64(4, 8))
            .unwrap();
        assert_eq!(sim.read("plus").unwrap().to_u64(), Some(0x4));
        assert_eq!(sim.read("minus").unwrap().to_u64(), Some(0x6));

        sim.set_input("selector", &LogicWord::from_u64(4, 13))
            .unwrap();
        assert!(sim.read("plus").unwrap().to_u64().is_none());
        sim.set_input("selector", &LogicWord::from_u64(4, 2))
            .unwrap();
        assert!(sim.read("minus").unwrap().to_u64().is_none());
    }

    #[test]
    fn runtime_indexed_part_select_writes_blocking_targets() {
        let source = |direction: &str| {
            format!(
                "module top(input logic trigger, input logic [15:0] a, input logic [3:0] selector, \
                              output logic [15:0] y); \
                     initial y = 16'h3210; \
                     always @(posedge trigger) begin \
                         y[selector {direction}: 4] = a[3:0] ^ 4'd3; \
                     end \
                 endmodule"
            )
        };
        let mut plus = sim_for(&source("+"));
        plus.set_input("trigger", &LogicWord::from_u64(1, 0))
            .unwrap();
        plus.set_input("a", &LogicWord::from_u64(16, 0x00a5))
            .unwrap();
        plus.set_input("selector", &LogicWord::from_u64(4, 6))
            .unwrap();
        plus.set_input("trigger", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(plus.read("y").unwrap().to_u64(), Some(0x3190));

        let mut minus = sim_for(&source("-"));
        minus
            .set_input("trigger", &LogicWord::from_u64(1, 0))
            .unwrap();
        minus
            .set_input("a", &LogicWord::from_u64(16, 0x00a5))
            .unwrap();
        minus
            .set_input("selector", &LogicWord::from_u64(4, 7))
            .unwrap();
        minus
            .set_input("trigger", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(minus.read("y").unwrap().to_u64(), Some(0x3260));

        plus.set_input("trigger", &LogicWord::from_u64(1, 0))
            .unwrap();
        plus.set_input("selector", &LogicWord::from_u64(4, 13))
            .unwrap();
        plus.set_input("trigger", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(plus.read("y").unwrap().to_u64(), Some(0x3190));
        minus
            .set_input("trigger", &LogicWord::from_u64(1, 0))
            .unwrap();
        minus
            .set_input("selector", &LogicWord::from_u64(4, 2))
            .unwrap();
        minus
            .set_input("trigger", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(minus.read("y").unwrap().to_u64(), Some(0x3260));

        let mut nonblocking = sim_for(
            "module top(input logic clk, input logic [3:0] first, second, a, b, \
                          output logic [15:0] y); \
                 initial y = 16'h3210; \
                 always @(posedge clk) begin \
                     y[first +: 4] <= a ^ 4'd3; \
                     y[second -: 4] <= b & 4'd5; \
                 end \
             endmodule",
        );
        nonblocking
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        nonblocking
            .set_input("first", &LogicWord::from_u64(4, 6))
            .unwrap();
        nonblocking
            .set_input("second", &LogicWord::from_u64(4, 14))
            .unwrap();
        nonblocking
            .set_input("a", &LogicWord::from_u64(4, 0xc))
            .unwrap();
        nonblocking
            .set_input("b", &LogicWord::from_u64(4, 0xa))
            .unwrap();
        nonblocking
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(nonblocking.read("y").unwrap().to_u64(), Some(0x03d0));

        nonblocking
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        nonblocking
            .set_input("first", &LogicWord::from_u64(4, 13))
            .unwrap();
        nonblocking
            .set_input("second", &LogicWord::from_u64(4, 2))
            .unwrap();
        nonblocking
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(nonblocking.read("y").unwrap().to_u64(), Some(0x03d0));
    }

    #[test]
    fn runtime_indexed_part_selects_work_inside_inline_functions() {
        let mut sim = sim_for(
            "module top(input logic [15:0] a, input logic [3:0] selector, \
                          output logic [3:0] plus, minus, local_slice); \
                 function logic [3:0] pick(input logic [15:0] value, \
                                           input logic [3:0] selector); \
                     pick = value[selector +: 4]; \
                 endfunction \
                 function logic [3:0] pick_reverse(input logic [15:0] value, \
                                                   input logic [3:0] selector); \
                     pick_reverse = value[selector -: 4]; \
                 endfunction \
                 function logic [3:0] use_local(input logic [15:0] value, \
                                                input logic [3:0] selector); \
                     logic [15:0] copy; \
                     copy = value; \
                     use_local = copy[selector +: 4]; \
                 endfunction \
                 assign plus = pick(a, selector); \
                 assign minus = pick_reverse(a, selector); \
                 assign local_slice = use_local(a, selector); \
             endmodule",
        );
        sim.set_input("a", &LogicWord::from_u64(16, 0xa4c7))
            .unwrap();
        sim.set_input("selector", &LogicWord::from_u64(4, 8))
            .unwrap();
        assert_eq!(sim.read("plus").unwrap().to_u64(), Some(0x4));
        assert_eq!(sim.read("minus").unwrap().to_u64(), Some(0x6));
        assert_eq!(sim.read("local_slice").unwrap().to_u64(), Some(0x4));

        sim.set_input("selector", &LogicWord::from_u64(4, 13))
            .unwrap();
        assert!(sim.read("plus").unwrap().to_u64().is_none());
        assert!(sim.read("local_slice").unwrap().to_u64().is_none());
        sim.set_input("selector", &LogicWord::from_u64(4, 2))
            .unwrap();
        assert!(sim.read("minus").unwrap().to_u64().is_none());
    }

    #[test]
    fn runtime_indexed_part_selects_read_reversed_packed_vectors() {
        let mut sim = sim_for(
            "module top(input logic [0:15] a, input logic [3:0] selector, \
                          output logic [3:0] plus, minus); \
                 assign plus = a[selector +: 4]; \
                 assign minus = a[selector -: 4]; \
             endmodule",
        );
        sim.set_input("a", &LogicWord::from_u64(16, 0xa4c7))
            .unwrap();
        sim.set_input("selector", &LogicWord::from_u64(4, 8))
            .unwrap();
        assert_eq!(sim.read("plus").unwrap().to_u64(), Some(3));
        assert_eq!(sim.read("minus").unwrap().to_u64(), Some(9));

        sim.set_input("selector", &LogicWord::from_u64(4, 13))
            .unwrap();
        assert!(sim.read("plus").unwrap().to_u64().is_none());
        sim.set_input("selector", &LogicWord::from_u64(4, 2))
            .unwrap();
        assert!(sim.read("minus").unwrap().to_u64().is_none());
    }

    #[test]
    fn runtime_indexed_part_selects_work_in_reversed_function_declarations() {
        let mut sim = sim_for(
            "module top(input logic [0:15] a, input logic [3:0] selector, \
                          output logic [3:0] plus, minus, local_slice); \
                 function logic [3:0] pick(input logic [0:15] value, \
                                           input logic [3:0] selector); \
                     pick = value[selector +: 4]; \
                 endfunction \
                 function logic [3:0] pick_reverse(input logic [0:15] value, \
                                                   input logic [3:0] selector); \
                     pick_reverse = value[selector -: 4]; \
                 endfunction \
                 function logic [3:0] use_local(input logic [0:15] value, \
                                                input logic [3:0] selector); \
                     logic [0:15] copy; \
                     copy = value; \
                     use_local = copy[selector +: 4]; \
                 endfunction \
                 assign plus = pick(a, selector); \
                 assign minus = pick_reverse(a, selector); \
                 assign local_slice = use_local(a, selector); \
             endmodule",
        );
        sim.set_input("a", &LogicWord::from_u64(16, 0xa4c7))
            .unwrap();
        sim.set_input("selector", &LogicWord::from_u64(4, 8))
            .unwrap();
        assert_eq!(sim.read("plus").unwrap().to_u64(), Some(3));
        assert_eq!(sim.read("minus").unwrap().to_u64(), Some(9));
        assert_eq!(sim.read("local_slice").unwrap().to_u64(), Some(3));

        sim.set_input("selector", &LogicWord::from_u64(4, 13))
            .unwrap();
        assert!(sim.read("plus").unwrap().to_u64().is_none());
        assert!(sim.read("local_slice").unwrap().to_u64().is_none());
        sim.set_input("selector", &LogicWord::from_u64(4, 2))
            .unwrap();
        assert!(sim.read("minus").unwrap().to_u64().is_none());
    }

    #[test]
    fn reversed_indexed_part_selects_drive_continuous_lvalues() {
        let mut sim = sim_for(
            "module top(input logic [3:0] a, output logic [0:15] y, \
                          output logic [3:0] selected); \
                 assign y[0 +: 4] = 4'd2; \
                 assign y[4 +: 4] = a ^ 4'd3; \
                 assign y[8 +: 4] = 4'd5; \
                 assign y[15 -: 4] = 4'd9; \
                 assign selected = y[4 +: 4]; \
             endmodule",
        );
        sim.set_input("a", &LogicWord::from_u64(4, 0xa)).unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x49a9));
        assert_eq!(sim.read("selected").unwrap().to_u64(), Some(0x9));
    }

    #[test]
    fn runtime_indexed_part_select_writes_reversed_packed_vectors() {
        let blocking = |direction: &str| {
            format!(
                "module top(input logic trigger, input logic [3:0] selector, \
                              input logic [3:0] a, output logic [0:15] y); \
                     initial y = 16'h3210; \
                     always @(posedge trigger) \
                     y[selector {direction}: 4] = a; \
                 endmodule"
            )
        };
        let mut plus = sim_for(&blocking("+"));
        plus.set_input("trigger", &LogicWord::from_u64(1, 0))
            .unwrap();
        plus.set_input("selector", &LogicWord::from_u64(4, 6))
            .unwrap();
        plus.set_input("a", &LogicWord::from_u64(4, 0xc)).unwrap();
        plus.set_input("trigger", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(plus.read("y").unwrap().to_u64(), Some(0x30d0));
        plus.set_input("trigger", &LogicWord::from_u64(1, 0))
            .unwrap();
        plus.set_input("selector", &LogicWord::from_u64(4, 13))
            .unwrap();
        plus.set_input("trigger", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(plus.read("y").unwrap().to_u64(), Some(0x30d0));

        let mut minus = sim_for(&blocking("-"));
        minus
            .set_input("trigger", &LogicWord::from_u64(1, 0))
            .unwrap();
        minus
            .set_input("selector", &LogicWord::from_u64(4, 9))
            .unwrap();
        minus.set_input("a", &LogicWord::from_u64(4, 0xc)).unwrap();
        minus
            .set_input("trigger", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(minus.read("y").unwrap().to_u64(), Some(0x30d0));
        minus
            .set_input("trigger", &LogicWord::from_u64(1, 0))
            .unwrap();
        minus
            .set_input("selector", &LogicWord::from_u64(4, 2))
            .unwrap();
        minus
            .set_input("trigger", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(minus.read("y").unwrap().to_u64(), Some(0x30d0));

        let mut nonblocking = sim_for(
            "module top(input logic trigger, input logic [3:0] first, second, \
                          input logic [3:0] a, b, output logic [0:15] y); \
                 initial y = 16'h3210; \
                 always @(posedge trigger) begin \
                     y[first +: 4] <= a; \
                     y[second -: 4] <= b; \
                 end \
             endmodule",
        );
        nonblocking
            .set_input("trigger", &LogicWord::from_u64(1, 0))
            .unwrap();
        nonblocking
            .set_input("first", &LogicWord::from_u64(4, 2))
            .unwrap();
        nonblocking
            .set_input("second", &LogicWord::from_u64(4, 14))
            .unwrap();
        nonblocking
            .set_input("a", &LogicWord::from_u64(4, 0xa))
            .unwrap();
        nonblocking
            .set_input("b", &LogicWord::from_u64(4, 0x5))
            .unwrap();
        nonblocking
            .set_input("trigger", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(nonblocking.read("y").unwrap().to_u64(), Some(0x1614));
    }

    #[test]
    fn function_multidimensional_unpacked_array_local_updates_and_reads() {
        let mut sim = sim_for(
            "module top(input logic [7:0] a, b, output logic [31:0] y); \
                 function logic [31:0] combine(input logic [7:0] left, right); \
                     logic [7:0] table [1:0][2:3]; \
                     table[1][3] = left + right; \
                     table[1][2] = left ^ right; \
                     table[0][3] = left & right; \
                     table[0][2] = left | right; \
                     combine = {table[1][3], table[1][2], \
                                table[0][3], table[0][2]}; \
                 endfunction \
                 assign y = combine(a, b); \
             endmodule",
        );
        sim.set_input("a", &LogicWord::from_u64(8, 0x5a)).unwrap();
        sim.set_input("b", &LogicWord::from_u64(8, 0xa5)).unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0xffff_00ff));
    }

    #[test]
    fn function_unpacked_array_elements_support_indexed_part_selects() {
        let mut sim = sim_for(
            "module top(input logic [3:0] selector, input logic [3:0] a, \
                          input logic [1:0] element_selector, \
                          output logic [3:0] selected, dynamic, \
                          output logic [7:0] updated); \
                 function logic [3:0] pick(input logic [1:0] element_selector, \
                                           input logic [3:0] selector); \
                     logic [7:0] table [0:2]; \
                     table[0] = 8'h5a; \
                     table[1] = 8'ha5; \
                     table[2] = 8'h3c; \
                     pick = table[element_selector][selector +: 4]; \
                 endfunction \
                 function logic [7:0] update(input logic [3:0] selector, \
                                             input logic [3:0] value); \
                     logic [7:0] table [0:1]; \
                     table[0] = 8'h5a; \
                     table[0][selector -: 4] = value ^ 4'd3; \
                     update = table[0]; \
                 endfunction \
                 assign selected = pick(element_selector, selector); \
                 assign dynamic = pick(element_selector, selector); \
                 assign updated = update(selector, a); \
             endmodule",
        );
        sim.set_input("selector", &LogicWord::from_u64(4, 4))
            .unwrap();
        sim.set_input("element_selector", &LogicWord::from_u64(2, 0))
            .unwrap();
        sim.set_input("a", &LogicWord::from_u64(4, 0xc)).unwrap();
        assert_eq!(sim.read("selected").unwrap().to_u64(), Some(0x5));
        assert_eq!(sim.read("dynamic").unwrap().to_u64(), Some(0x5));
        assert_eq!(sim.read("updated").unwrap().to_u64(), Some(0x5e));

        sim.set_input("selector", &LogicWord::from_u64(4, 3))
            .unwrap();
        sim.set_input("element_selector", &LogicWord::from_u64(2, 1))
            .unwrap();
        assert_eq!(sim.read("selected").unwrap().to_u64(), Some(0x4));
        assert_eq!(sim.read("dynamic").unwrap().to_u64(), Some(0x4));
        assert_eq!(sim.read("updated").unwrap().to_u64(), Some(0x5f));

        sim.set_input("selector", &LogicWord::from_u64(4, 13))
            .unwrap();
        assert!(sim.read("selected").unwrap().to_u64().is_none());
        sim.set_input("selector", &LogicWord::from_u64(4, 4))
            .unwrap();
        sim.set_input("element_selector", &LogicWord::from_u64(2, 3))
            .unwrap();
        assert!(sim.read("selected").unwrap().to_u64().is_none());
        assert_eq!(sim.read("updated").unwrap().to_u64(), Some(0x5e));
    }

    #[test]
    fn function_runtime_array_elements_support_indexed_assignments() {
        let mut sim = sim_for(
            "module top(input logic [1:0] element_selector, \
                          input logic [3:0] packed_selector, \
                          input logic [3:0] value, output logic [23:0] whole); \
                 function logic [23:0] update(input logic [1:0] element_selector, \
                                              input logic [3:0] packed_selector, \
                                              input logic [3:0] value); \
                     logic [7:0] memory [0:2]; \
                     memory[0] = 8'h12; \
                     memory[1] = 8'h34; \
                     memory[2] = 8'h56; \
                     memory[element_selector][packed_selector -: 4] = value ^ 4'd3; \
                     update = {memory[2], memory[1], memory[0]}; \
                 endfunction \
                 assign whole = update(element_selector, packed_selector, value); \
             endmodule",
        );
        sim.set_input("value", &LogicWord::from_u64(4, 0xc))
            .unwrap();
        sim.set_input("packed_selector", &LogicWord::from_u64(4, 3))
            .unwrap();
        sim.set_input("element_selector", &LogicWord::from_u64(2, 1))
            .unwrap();
        assert_eq!(sim.read("whole").unwrap().to_u64(), Some(0x56_3f_12));

        sim.set_input("element_selector", &LogicWord::from_u64(2, 3))
            .unwrap();
        sim.set_input("packed_selector", &LogicWord::from_u64(4, 4))
            .unwrap();
        sim.set_input("value", &LogicWord::from_u64(4, 0)).unwrap();
        assert_eq!(sim.read("whole").unwrap().to_u64(), Some(0x56_34_12));

        sim.set_input("element_selector", &LogicWord::from_u64(2, 2))
            .unwrap();
        sim.set_input("packed_selector", &LogicWord::from_u64(4, 13))
            .unwrap();
        sim.set_input("value", &LogicWord::from_u64(4, 0xa))
            .unwrap();
        assert_eq!(sim.read("whole").unwrap().to_u64(), Some(0x56_34_12));
    }

    #[test]
    fn runtime_array_element_targets_support_indexed_assignments() {
        let source = |assignment: &str| {
            format!(
                "module top(input logic clk, input logic [1:0] element_selector, \
                              input logic [3:0] packed_selector, \
                              input logic [3:0] value, output logic [23:0] whole); \
                     logic [7:0] memory [0:2]; \
                     initial begin \
                         memory[0] = 8'h12; \
                         memory[1] = 8'h34; \
                         memory[2] = 8'h56; \
                     end \
                     always @(posedge clk) \
                         memory[element_selector][packed_selector -: 4] \
                         {assignment} value ^ 4'd3; \
                     assign whole = {{memory[2], memory[1], memory[0]}}; \
                 endmodule"
            )
        };
        for assignment in ["=", "<="] {
            let mut sim = sim_for(&source(assignment));
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            sim.set_input("element_selector", &LogicWord::from_u64(2, 1))
                .unwrap();
            sim.set_input("packed_selector", &LogicWord::from_u64(4, 3))
                .unwrap();
            sim.set_input("value", &LogicWord::from_u64(4, 0xc))
                .unwrap();
            sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
            assert_eq!(sim.read("whole").unwrap().to_u64(), Some(0x56_3f_12));

            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            sim.set_input("element_selector", &LogicWord::from_u64(2, 3))
                .unwrap();
            sim.set_input("packed_selector", &LogicWord::from_u64(4, 13))
                .unwrap();
            sim.set_input("value", &LogicWord::from_u64(4, 0)).unwrap();
            sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
            assert_eq!(sim.read("whole").unwrap().to_u64(), Some(0x56_3f_12));
        }
    }

    #[test]
    fn runtime_indexed_part_selects_drive_continuous_lvalues() {
        let source = |direction: &str, reversed: bool| {
            let vector_range = if reversed { "[0:15]" } else { "[15:0]" };
            format!(
                "module top(input logic [3:0] selector, input logic [3:0] value, \
                              output logic {vector_range} y, output logic [3:0] selected); \
                     initial y = 16'h3210; \
                     assign y[selector {direction}: 4] = value ^ 4'd3; \
                     assign selected = y[selector {direction}: 4]; \
                 endmodule"
            )
        };

        let mut plus = sim_for(&source("+", false));
        assert_eq!(plus.design.processes.len(), 1);
        assert_eq!(plus.design.assigns.len(), 1);
        plus.set_input("value", &LogicWord::from_u64(4, 0xc))
            .unwrap();
        plus.set_input("selector", &LogicWord::from_u64(4, 6))
            .unwrap();
        assert_eq!(plus.read("y").unwrap().to_u64(), Some(0x33d0));
        assert_eq!(plus.read("selected").unwrap().to_u64(), Some(0xf));

        plus.set_input("selector", &LogicWord::from_u64(4, 13))
            .unwrap();
        assert_eq!(plus.read("y").unwrap().to_u64(), Some(0x33d0));
        assert!(plus.read("selected").unwrap().to_u64().is_none());

        let mut minus = sim_for(&source("-", false));
        minus
            .set_input("value", &LogicWord::from_u64(4, 0xc))
            .unwrap();
        minus
            .set_input("selector", &LogicWord::from_u64(4, 7))
            .unwrap();
        assert_eq!(minus.read("y").unwrap().to_u64(), Some(0x32f0));
        assert_eq!(minus.read("selected").unwrap().to_u64(), Some(0xf));

        minus
            .set_input("value", &LogicWord::from_u64(4, 0))
            .unwrap();
        assert_eq!(minus.read("y").unwrap().to_u64(), Some(0x3230));
        assert_eq!(minus.read("selected").unwrap().to_u64(), Some(0x3));

        minus
            .set_input("selector", &LogicWord::from_u64(4, 2))
            .unwrap();
        assert_eq!(minus.read("y").unwrap().to_u64(), Some(0x3230));
        assert!(minus.read("selected").unwrap().to_u64().is_none());

        let mut reversed = sim_for(&source("+", true));
        reversed
            .set_input("value", &LogicWord::from_u64(4, 0xc))
            .unwrap();
        reversed
            .set_input("selector", &LogicWord::from_u64(4, 6))
            .unwrap();
        assert_eq!(reversed.read("y").unwrap().to_u64(), Some(0x33d0));
        assert_eq!(reversed.read("selected").unwrap().to_u64(), Some(0xf));

        reversed
            .set_input("selector", &LogicWord::from_u64(4, 13))
            .unwrap();
        assert_eq!(reversed.read("y").unwrap().to_u64(), Some(0x33d0));
        assert!(reversed.read("selected").unwrap().to_u64().is_none());
    }

    #[test]
    fn runtime_array_element_targets_support_continuous_drives() {
        let mut sim = sim_for(
            "module top(input logic [2:0] element_selector, read_element, \
                          input logic [1:0] value, output logic [3:0] y); \
                 logic [3:0] memory [0:3]; \
                 initial begin \
                     memory[0] = 4'hA; \
                     memory[1] = 4'h5; \
                     memory[2] = 4'h3; \
                     memory[3] = 4'hC; \
                 end \
                 assign memory[element_selector][1 +: 2] = value ^ 2'h3; \
                 assign y = memory[read_element][0 +: 4]; \
             endmodule",
        );
        assert_eq!(sim.design.processes.len(), 1);
        assert_eq!(sim.design.assigns.len(), 1);

        sim.set_input("element_selector", &LogicWord::from_u64(3, 2))
            .unwrap();
        sim.set_input("read_element", &LogicWord::from_u64(3, 2))
            .unwrap();
        sim.set_input("value", &LogicWord::from_u64(2, 3)).unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x1));

        sim.set_input("value", &LogicWord::from_u64(2, 0)).unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x7));

        sim.set_input("element_selector", &LogicWord::from_u64(3, 1))
            .unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x7));

        sim.set_input("element_selector", &LogicWord::from_u64(3, 4))
            .unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x7));

        sim.set_input("read_element", &LogicWord::from_u64(3, 1))
            .unwrap();
        sim.set_input("element_selector", &LogicWord::from_u64(3, 1))
            .unwrap();
        sim.set_input("value", &LogicWord::from_u64(2, 3)).unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x1));

        sim.set_input("element_selector", &LogicWord::from_u64(3, 4))
            .unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0x1));
    }

    #[test]
    fn function_multidimensional_array_whole_local_layout() {
        let mut sim = sim_for(
            "module top(input logic dummy, output logic [7:0] y); \
                 function logic [7:0] whole(input logic unused); \
                     logic [1:0] table [0:1][0:1]; \
                     table[0][0] = 0; table[0][1] = 1; \
                     table[1][0] = 2; table[1][1] = 3; \
                     whole = table; \
                 endfunction \
                 assign y = whole(dummy); \
             endmodule",
        );
        sim.set_input("dummy", &LogicWord::from_u64(1, 0)).unwrap();
        assert_eq!(sim.read("y").unwrap().to_u64(), Some(0b0001_1011));
    }

    #[test]
    fn disable_iff_clears_pending_property_attempts() {
        let mut sim = sim_for(
            "module m(input clk, reset, req, grant);
                assert property (@(posedge clk) disable iff (reset)
                    req |-> ##[2] grant)
                    else $error(\"disabled attempt must not fire\");
             endmodule",
        );
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("reset", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("grant", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("reset", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("req", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("reset", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    }

    #[test]
    fn disable_iff_aborts_goto_repetition_attempts() {
        let mut sim = sim_for(
            "module m(input clk, reset, req, grant);
                assert property (@(posedge clk) disable iff (reset)
                    req |-> grant[->3])
                    else $error(\"disabled goto attempt must not fire\");
             endmodule",
        );
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("reset", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("req", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("reset", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("reset", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    }

    #[test]
    fn static_virtual_interface_ports_share_typed_bindings() {
        let mut sim = sim_for(
            "interface bus;
                logic [7:0] data;
                logic valid;
                modport source(output valid, data);
                modport sink(input valid, data);
             endinterface

             module producer(virtual interface bus.source b,
                 input logic [7:0] value);
                assign b.valid = 1'b1;
                assign b.data = value;
             endmodule

             module consumer(virtual interface bus.sink b,
                 output logic [7:0] result);
                assign result = b.valid ? b.data : 8'd0;
             endmodule

             module top(input logic [7:0] value, output logic [7:0] result);
                bus shared();
                producer p(.b(shared), .value(value));
                consumer c(.b(shared), .result(result));
             endmodule",
        );
        sim.set_input("value", &LogicWord::from_u64(8, 0x5a))
            .unwrap();
        assert_eq!(sim.read("result").unwrap().to_u64(), Some(0x5a));
    }

    #[test]
    fn snapshot_and_vcd() {
        let mut sim = sim_for("module m(input a, output y); assign y = ~a; endmodule");
        sim.enable_vcd();
        sim.set_input("a", &LogicWord::from_u64(1, 1)).unwrap();
        sim.run_until(5).unwrap();
        let v = sim.vcd().unwrap();
        assert!(v.contains("$enddefinitions"));
        assert!(v.contains("$var"));
        let snap = sim.snapshot();
        assert!(snap.iter().any(|(n, _)| n == "y"));
    }

    #[test]
    fn instance_with_delay_feedback() {
        // D flip-flop built structurally from processes across module boundary
        let mut sim = sim_for(
            "module dff(input clk, d, output reg q);
                always @(posedge clk) q <= d;
             endmodule
             module top(input clk, d, output q);
                wire w; dff u1(.clk(clk), .d(d), .q(w));
                assign q = w;
             endmodule",
        );
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("d", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        assert_eq!(sim.read_port("q").unwrap().to_u64(), Some(1));
    }
}
