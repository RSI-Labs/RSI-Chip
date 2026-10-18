//! # eda-sim — event-driven 4-valued simulator
//!
//! Executes an elaborated [`Design`] (RTL, or gate-level via
//! [`eda_core::to_design::netlist_to_design`]) with Verilog-style
//! scheduling: active events, delta cycles for continuous assignments,
//! non-blocking (NBA) updates at the end of each time slot, edge and
//! combinational `always` processes, `#delay` continuations, `forever`,
//! `$display`, `$monitor`, `$finish` and optional VCD dumping.

use eda_core::cancellation::FlowCancellation;
use eda_core::ir::{Design, Dir, Expr, ForkMode, LValue, Range, Sens, Stmt};
use eda_core::logic::{LogicValue, LogicWord};
use std::collections::{BTreeMap, HashMap, HashSet};

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

#[derive(Clone, Debug)]
enum MemoryNbaUpdate {
    Whole {
        memory: usize,
        index: u64,
        value: LogicWord,
    },
    Part {
        memory: usize,
        index: u64,
        low_offset: usize,
        width: usize,
        value: LogicWord,
    },
}

struct Cont<'d> {
    process: Option<usize>,
    join: Option<usize>,
    work: Vec<&'d Stmt>,
}

struct ForkJoin<'d> {
    mode: ForkMode,
    released: bool,
    remaining: usize,
    continuation: Cont<'d>,
}

struct Waiter<'d> {
    condition: &'d Expr,
    nets: HashSet<usize>,
    memories: HashSet<usize>,
    continuation: Cont<'d>,
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
    fork_joins: Vec<Option<ForkJoin<'d>>>,
    while_iterations: HashMap<usize, u64>,
    waiters: Vec<Waiter<'d>>,
    memory_elements: Vec<HashMap<u64, LogicWord>>,
    memory_aliases: Vec<usize>,
    memory_nba: Vec<MemoryNbaUpdate>,
    memory_readers: Vec<Vec<usize>>,
    memory_assign_fanin: Vec<Vec<usize>>,
    assign_fanin: Vec<Vec<usize>>,
    proc_edges: Vec<Vec<(usize, Edge)>>,
    pub displays: Vec<String>,
    property_covers: BTreeMap<String, u64>,
    finished: bool,
    print_stdout: bool,
    monitors: Vec<Monitor<'d>>,
    vcd: Option<VcdWriter>,
    vcd_changes: HashSet<usize>,
    vcd_memory_changes: HashSet<(usize, u64)>,
    event_count: u64,
    has_zero_delay_feedback: bool,
    zero_delay_feedback_nets: Vec<String>,
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
        let mut memory_elements = Vec::with_capacity(design.memories.len());
        for memory in &design.memories {
            if memory.flat_length == 0 {
                return Err(format!("memory '{}' is empty", memory.name));
            }
            memory_elements.push(HashMap::new());
        }
        let mut memory_aliases = vec![0_usize; design.memories.len()];
        for (index, alias) in memory_aliases.iter_mut().enumerate() {
            *alias = index;
        }
        for connection in &design.memory_connections {
            if connection.child >= memory_aliases.len() || connection.parent >= memory_aliases.len()
            {
                return Err("memory connection refers to a missing memory object".into());
            }
            memory_aliases[connection.child] = connection.parent;
        }
        // Parent memories are elaborated before child instances, but resolve
        // chains explicitly so nested aliases remain well-formed.
        for index in 0..memory_aliases.len() {
            let mut target = memory_aliases[index];
            let mut steps = 0;
            while target != memory_aliases[target] {
                target = memory_aliases[target];
                steps += 1;
                if steps > memory_aliases.len() {
                    return Err("memory alias cycle detected".into());
                }
            }
            memory_aliases[index] = target;
        }
        let mut memory_readers: Vec<Vec<usize>> = vec![Vec::new(); design.memories.len()];
        let mut memory_assign_fanin: Vec<Vec<usize>> = vec![Vec::new(); design.memories.len()];
        for (ai, assign) in design.assigns.iter().enumerate() {
            for memory in expr_memories(&assign.e) {
                if memory < design.memories.len() {
                    memory_assign_fanin[memory_aliases[memory]].push(ai);
                }
            }
        }
        for (pi, p) in design.processes.iter().enumerate() {
            register_sens(&p.sens, pi, &p.body, &mut proc_edges, n);
            if matches!(p.sens, Sens::Comb) {
                for memory in stmt_memories(&p.body) {
                    if memory < design.memories.len() {
                        memory_readers[memory_aliases[memory]].push(pi);
                    }
                }
            }
        }
        let mut mons = Vec::new();
        for init in &design.initials {
            collect_monitors(init, &mut mons);
        }
        let zero_delay_feedback_nets = design
            .processes
            .iter()
            .filter(|process| matches!(process.sens, Sens::Comb))
            .flat_map(|process| {
                let mut reads = HashSet::new();
                let mut writes = HashSet::new();
                collect_stmt_access(&process.body, &mut reads, &mut writes);
                reads.intersection(&writes).copied().collect::<Vec<_>>()
            })
            .map(|net| design.net_name(net))
            .collect::<Vec<_>>();
        let mut sim = Simulator {
            design,
            vals: vec![LogicValue::X; n],
            events: BTreeMap::new(),
            conts: BTreeMap::new(),
            now: 0,
            nba: Vec::new(),
            busy_procs: HashSet::new(),
            fork_joins: Vec::new(),
            while_iterations: HashMap::new(),
            waiters: Vec::new(),
            memory_elements,
            memory_aliases,
            memory_nba: Vec::new(),
            memory_readers,
            memory_assign_fanin,
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
            vcd_memory_changes: HashSet::new(),
            event_count: 0,
            has_zero_delay_feedback: !zero_delay_feedback_nets.is_empty(),
            zero_delay_feedback_nets,
            cancellation: FlowCancellation::new(),
        };
        for init in &design.initials {
            sim.conts.entry(0).or_default().push(Cont {
                process: None,
                join: None,
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
            if matches!(p.sens, Sens::Comb | Sens::Timed) {
                sim.conts.entry(0).or_default().push(Cont {
                    process: Some(pi),
                    join: None,
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

    pub fn vcd(&mut self) -> Option<String> {
        self.vcd
            .as_mut()
            .map(|v| v.render(&self.vals, &self.memory_elements))
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

    /// Names of nets conservatively detected as same-process combinational
    /// read/write feedback. Empty when no such process exists.
    pub fn zero_delay_feedback_nets(&self) -> &[String] {
        &self.zero_delay_feedback_nets
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
            if !did && (!self.nba.is_empty() || !self.memory_nba.is_empty()) {
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
                let memory_updates = std::mem::take(&mut self.memory_nba);
                for update in memory_updates {
                    if self.cancellation.is_cancelled() {
                        return Err("simulation cancelled".into());
                    }
                    match update {
                        MemoryNbaUpdate::Whole {
                            memory,
                            index,
                            value,
                        } => self.apply_memory_update(memory, index, value)?,
                        MemoryNbaUpdate::Part {
                            memory,
                            index,
                            low_offset,
                            width,
                            value,
                        } => {
                            self.apply_memory_part_update(memory, index, low_offset, width, value)?;
                        }
                    }
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
                            join: None,
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
                join: None,
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
        self.wake_net_waiters(changed_net);
        Ok(())
    }

    fn apply_memory_update(
        &mut self,
        memory: usize,
        index: u64,
        value: LogicWord,
    ) -> Result<(), String> {
        let Some(storage) = self.memory_elements.get_mut(memory) else {
            return Ok(());
        };
        let element_width = self.design.memories[memory].element_width;
        let value = value.sized(element_width);
        if storage.get(&index) == Some(&value) {
            return Ok(());
        }
        storage.insert(index, value);
        self.mark_vcd_memory_change(memory, index);
        self.event_count += 1;
        if self.event_count > MAX_EVENTS {
            return Err("event budget exceeded".into());
        }
        self.notify_memory_write(memory)
    }

    fn apply_memory_part_update(
        &mut self,
        memory: usize,
        index: u64,
        low_offset: usize,
        width: usize,
        value: LogicWord,
    ) -> Result<(), String> {
        let Some(storage) = self.memory_elements.get_mut(memory) else {
            return Ok(());
        };
        let declaration = &self.design.memories[memory];
        let element_width = declaration.element_width;
        let high_offset = low_offset + width - 1;
        if width == 0 || high_offset >= element_width {
            return Ok(());
        }
        let old = storage
            .get(&index)
            .cloned()
            .unwrap_or_else(|| LogicWord::xes(element_width));
        let value = value.sized(width);
        let mut updated = Vec::with_capacity(element_width);
        for bit in 0..element_width {
            if (low_offset..=high_offset).contains(&bit) {
                let selected_bit = if declaration.element_is_reversed() {
                    high_offset - bit
                } else {
                    bit - low_offset
                };
                updated.push(value.bit(selected_bit));
            } else {
                updated.push(old.bit(bit));
            }
        }
        let updated = LogicWord { bits: updated };
        if updated == old {
            return Ok(());
        }
        storage.insert(index, updated);
        self.mark_vcd_memory_change(memory, index);
        self.event_count += 1;
        if self.event_count > MAX_EVENTS {
            return Err("event budget exceeded".into());
        }
        self.notify_memory_write(memory)
    }

    fn notify_memory_write(&mut self, memory: usize) -> Result<(), String> {
        let readers = self.memory_readers.get(memory).cloned().unwrap_or_default();
        for pi in readers {
            let body = &self.design.processes[pi].body;
            self.conts.entry(self.now).or_default().push(Cont {
                process: Some(pi),
                join: None,
                work: vec![body],
            });
        }

        let assign_sinks = self
            .memory_assign_fanin
            .get(memory)
            .cloned()
            .unwrap_or_default();
        for ai in assign_sinks {
            let value = self.eval(&self.design.assigns[ai].e);
            let delay = self.design.assigns[ai].delay;
            let time = self.now + delay;
            for (bit, net) in lv_bits(&self.design.assigns[ai].lv).into_iter().enumerate() {
                let val = value.bit(bit);
                if time == self.now {
                    self.schedule(Update {
                        net,
                        val,
                        driver: Some(ai),
                    });
                } else {
                    self.events.entry(time).or_default().push(Update {
                        net,
                        val,
                        driver: Some(ai),
                    });
                }
            }
        }
        self.wake_memory_waiters(memory);
        Ok(())
    }

    fn wake_net_waiters(&mut self, changed_net: usize) {
        let mut ready = Vec::new();
        let mut index = 0;
        while index < self.waiters.len() {
            let waiter = &self.waiters[index];
            if waiter.nets.contains(&changed_net)
                && self.eval(waiter.condition).reduce_or().as_bool() == Some(true)
            {
                ready.push(self.waiters.remove(index).continuation);
            } else {
                index += 1;
            }
        }
        for continuation in ready {
            self.conts.entry(self.now).or_default().push(continuation);
        }
    }

    fn wake_memory_waiters(&mut self, changed_memory: usize) {
        let mut ready = Vec::new();
        let mut index = 0;
        while index < self.waiters.len() {
            let waiter = &self.waiters[index];
            if waiter.memories.contains(&changed_memory)
                && self.eval(waiter.condition).reduce_or().as_bool() == Some(true)
            {
                ready.push(self.waiters.remove(index).continuation);
            } else {
                index += 1;
            }
        }
        for continuation in ready {
            self.conts.entry(self.now).or_default().push(continuation);
        }
    }

    fn canonical_memory(&self, memory: usize) -> usize {
        self.memory_aliases.get(memory).copied().unwrap_or(memory)
    }

    #[allow(clippy::too_many_arguments)]
    fn write_memory_file(
        &self,
        radix: u32,
        path: &str,
        memory: usize,
        start: Option<&Expr>,
        finish: Option<&Expr>,
    ) -> Result<(), String> {
        let memory = self.canonical_memory(memory);
        let declaration = self
            .design
            .memories
            .get(memory)
            .ok_or_else(|| format!("memory-file dump refers to missing memory {memory}"))?;
        let start = start
            .map(|start| {
                self.eval(start)
                    .to_u64()
                    .ok_or_else(|| "memory-file dump start must be known".to_string())
            })
            .transpose()?
            .unwrap_or(0);
        let finish = finish
            .map(|finish| {
                self.eval(finish)
                    .to_u64()
                    .ok_or_else(|| "memory-file dump finish must be known".to_string())
            })
            .transpose()?
            .unwrap_or(declaration.flat_length.saturating_sub(1));
        if start > finish {
            return Err(format!(
                "memory-file dump finish {finish} is below start {start}"
            ));
        }
        if finish >= declaration.flat_length {
            return Err(format!(
                "memory-file dump address {finish} is outside memory '{}'",
                declaration.name
            ));
        }
        let storage = self
            .memory_elements
            .get(memory)
            .ok_or_else(|| format!("memory '{}' has no sparse storage", declaration.name))?;
        let mut output = String::new();
        for address in start..=finish {
            let value = storage
                .get(&address)
                .cloned()
                .unwrap_or_else(|| LogicWord::xes(declaration.element_width));
            let text = if radix == 16 {
                value.to_hex_str()
            } else {
                value.to_bin_str()
            };
            output.push_str(&text);
            output.push('\n');
        }
        std::fs::write(path, output)
            .map_err(|error| format!("cannot write memory file '{path}': {error}"))
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
                Stmt::While { cond, body, steps } => {
                    const MAX_WHILE_ITERATIONS: u64 = 65_536;
                    let condition = self.eval(cond);
                    if condition.reduce_or().as_bool() != Some(true) {
                        self.while_iterations.remove(&(s as *const Stmt as usize));
                    } else {
                        let iterations = self
                            .while_iterations
                            .entry(s as *const Stmt as usize)
                            .or_insert(0);
                        *iterations += 1;
                        if *iterations > MAX_WHILE_ITERATIONS {
                            return Err(
                                "while loop exceeds the 65536-iteration execution limit".into()
                            );
                        }
                        work.push(s);
                        work.extend(steps.iter().rev());
                        work.push(body);
                    }
                }
                Stmt::Break => {
                    let Some(loop_position) = work
                        .iter()
                        .rposition(|statement| matches!(statement, Stmt::While { .. }))
                    else {
                        return Err("break is outside a runtime loop".into());
                    };
                    work.truncate(loop_position);
                }
                Stmt::Continue => {
                    let Some(loop_position) = work
                        .iter()
                        .rposition(|statement| matches!(statement, Stmt::While { .. }))
                    else {
                        return Err("continue is outside a runtime loop".into());
                    };
                    let loop_statement = work[loop_position];
                    let steps = match loop_statement {
                        Stmt::While { steps, .. } => steps,
                        _ => unreachable!("loop position must point to a While statement"),
                    };
                    work.truncate(loop_position);
                    work.push(loop_statement);
                    work.extend(steps.iter().rev());
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
                Stmt::MemoryBl { memory, indexes, e } => {
                    let Some(index) = self.memory_index(*memory, indexes) else {
                        continue;
                    };
                    let value = self.eval(e);
                    let memory = self.canonical_memory(*memory);
                    self.apply_memory_update(memory, index, value)?;
                }
                Stmt::MemoryNBl { memory, indexes, e } => {
                    let Some(index) = self.memory_index(*memory, indexes) else {
                        continue;
                    };
                    let value = self.eval(e);
                    let memory = self.canonical_memory(*memory);
                    self.memory_nba.push(MemoryNbaUpdate::Whole {
                        memory,
                        index,
                        value,
                    });
                }
                Stmt::MemoryBlPart {
                    memory,
                    indexes,
                    start,
                    width,
                    ascending,
                    e,
                } => {
                    let Some((index, low_offset)) =
                        self.memory_part_location(*memory, indexes, start, *width, *ascending)
                    else {
                        continue;
                    };
                    let value = self.eval(e);
                    let memory = self.canonical_memory(*memory);
                    self.apply_memory_part_update(memory, index, low_offset, *width, value)?;
                }
                Stmt::MemoryNBlPart {
                    memory,
                    indexes,
                    start,
                    width,
                    ascending,
                    e,
                } => {
                    let Some((index, low_offset)) =
                        self.memory_part_location(*memory, indexes, start, *width, *ascending)
                    else {
                        continue;
                    };
                    let value = self.eval(e);
                    let memory = self.canonical_memory(*memory);
                    self.memory_nba.push(MemoryNbaUpdate::Part {
                        memory,
                        index,
                        low_offset,
                        width: *width,
                        value,
                    });
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
                    if let Some(process) = c.process {
                        self.busy_procs.remove(&process);
                    }
                    self.conts.entry(self.now + ns).or_default().push(Cont {
                        process: c.process,
                        join: c.join,
                        work: exec_order,
                    });
                    return Ok(());
                }
                Stmt::Fork {
                    branches,
                    mode: ForkMode::JoinNone,
                } if !branches.is_empty() => {
                    // join_none does not suspend the parent. Its children are
                    // queued behind the parent continuation and retain any
                    // enclosing join obligation.
                    for branch in branches {
                        self.conts.entry(self.now).or_default().push(Cont {
                            process: None,
                            join: c.join,
                            work: vec![branch],
                        });
                    }
                }
                Stmt::Fork { branches, mode } if !branches.is_empty() => {
                    let join_index = self.fork_joins.len();
                    let process = c.process;
                    self.fork_joins.push(Some(ForkJoin {
                        mode: *mode,
                        released: false,
                        remaining: branches.len(),
                        continuation: Cont {
                            process,
                            join: c.join,
                            work,
                        },
                    }));
                    for branch in branches {
                        self.conts.entry(self.now).or_default().push(Cont {
                            process: None,
                            join: Some(join_index),
                            work: vec![branch],
                        });
                    }
                    if let Some(process) = process {
                        self.busy_procs.remove(&process);
                    }
                    return Ok(());
                }
                Stmt::Fork { .. } => {}
                Stmt::Wait { cond, body } => {
                    let condition = self.eval(cond);
                    if condition.reduce_or().as_bool() == Some(true) {
                        work.push(body);
                    } else {
                        let mut exec_order: Vec<&Stmt> = vec![&**body];
                        exec_order.extend(work.iter().rev().copied());
                        exec_order.reverse();
                        let nets = expr_nets(cond)
                            .into_iter()
                            .filter(|net| *net < self.design.n_nets)
                            .collect();
                        let memories = expr_memories(cond)
                            .into_iter()
                            .filter(|memory| *memory < self.design.memories.len())
                            .map(|memory| self.canonical_memory(memory))
                            .collect();
                        let continuation = Cont {
                            process: c.process,
                            join: c.join,
                            work: exec_order,
                        };
                        if let Some(process) = c.process {
                            self.busy_procs.remove(&process);
                        }
                        self.waiters.push(Waiter {
                            condition: cond,
                            nets,
                            memories,
                            continuation,
                        });
                        return Ok(());
                    }
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
                Stmt::WriteMem {
                    radix,
                    path,
                    memory,
                    start,
                    finish,
                } => {
                    self.write_memory_file(*radix, path, *memory, start.as_ref(), finish.as_ref())?
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
        if let Some(process) = c.process {
            self.busy_procs.remove(&process);
        }
        if let Some(join) = c.join {
            self.complete_join(join);
        }
        Ok(())
    }

    fn complete_join(&mut self, index: usize) {
        if index >= self.fork_joins.len() {
            return;
        }
        let Some(state) = self.fork_joins.get_mut(index) else {
            return;
        };
        let Some(state) = state.as_mut() else {
            return;
        };
        state.remaining = state.remaining.saturating_sub(1);
        let release_parent = match state.mode {
            ForkMode::JoinAll => state.remaining == 0,
            ForkMode::JoinAny => !state.released,
            ForkMode::JoinNone => false,
        };
        if release_parent {
            state.released = true;
            if let Some(state) = self.fork_joins[index].take() {
                self.conts
                    .entry(self.now)
                    .or_default()
                    .push(state.continuation);
            }
        }
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

    fn memory_index(&self, memory: usize, indexes: &[Expr]) -> Option<u64> {
        let declaration = self.design.memories.get(memory)?;
        let mut logical = Vec::with_capacity(indexes.len());
        for expression in indexes {
            logical.push(self.eval(expression).to_u64()? as i64);
        }
        declaration.physical_index(&logical)
    }

    fn memory_part_location(
        &self,
        memory: usize,
        indexes: &[Expr],
        start: &Expr,
        width: usize,
        ascending: bool,
    ) -> Option<(u64, usize)> {
        let index = self.memory_index(memory, indexes)?;
        let start = self.eval(start).to_u64()? as i64;
        let (low_offset, _) = self
            .design
            .memories
            .get(memory)?
            .element_part_offsets(start, width, ascending)?;
        Some((index, low_offset))
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
                    BinOp::Pow => av.pow(&bv, b.is_signed()),
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
            Expr::StringLength { e } => match ascii_string_length(&self.eval(e)) {
                Some(length) => LogicWord::from_u64(32, length as u64),
                None => LogicWord::xes(32),
            },
            Expr::StringConcat { left, right } => {
                let left = self.eval(left);
                let right = self.eval(right);
                match (ascii_string_length(&left), ascii_string_length(&right)) {
                    (Some(left_length), Some(right_length)) => {
                        concat_packed_strings(&left, left_length, &right, right_length)
                    }
                    _ => LogicWord::xes(left.width()),
                }
            }
            Expr::StringGetByte { e, index } => {
                let value = self.eval(e);
                let index = self.eval(index);
                match index.to_u64().map(|index| index as usize) {
                    Some(index) => packed_string_byte(&value, index),
                    None => LogicWord::xes(8),
                }
            }
            Expr::StringCase { e, upper } => change_string_case(&self.eval(e), *upper),
            Expr::StringCompare {
                left,
                right,
                ignore_case,
            } => {
                let left = self.eval(left);
                let right = self.eval(right);
                match compare_packed_strings(&left, &right, *ignore_case) {
                    Some(result) => LogicWord::from_u64(32, result as u64),
                    None => LogicWord::xes(32),
                }
            }
            Expr::StringPutc { e, index, value } => {
                let string = self.eval(e);
                let index = self.eval(index);
                let value = self.eval(value);
                putc_packed_string(&string, &index, &value)
            }
            Expr::StringSubstr { e, left, right } => {
                let string = self.eval(e);
                let left = self.eval(left);
                let right = self.eval(right);
                substring_packed_string(&string, &left, &right)
            }
            Expr::StringToRadix { e, radix } => {
                let string = self.eval(e);
                match parse_packed_string_radix(&string, *radix) {
                    Some(value) => LogicWord::from_u64(32, value as u64),
                    None => LogicWord::xes(32),
                }
            }
            Expr::StringFromRadix { value, radix } => {
                let value = self.eval(value);
                format_packed_string_radix(&value, *radix)
            }
            Expr::MemoryRead {
                memory,
                indexes,
                width,
                ..
            } => match self.memory_index(*memory, indexes) {
                Some(index) => self
                    .memory_elements
                    .get(self.canonical_memory(*memory))
                    .and_then(|elements| elements.get(&index))
                    .cloned()
                    .unwrap_or_else(|| LogicWord::xes(*width)),
                None => LogicWord::xes(*width),
            },
            Expr::MemoryReadPart {
                memory,
                indexes,
                start,
                width,
                ascending,
            } => match self.memory_part_location(*memory, indexes, start, *width, *ascending) {
                Some((index, low_offset)) => {
                    let element = self
                        .memory_elements
                        .get(self.canonical_memory(*memory))
                        .and_then(|elements| elements.get(&index))
                        .cloned()
                        .unwrap_or_else(|| {
                            LogicWord::xes(self.design.memories[*memory].element_width)
                        });
                    if self.design.memories[*memory].element_is_reversed() {
                        let high_offset = low_offset + width - 1;
                        let mut bits = Vec::with_capacity(*width);
                        for bit in (low_offset..=high_offset).rev() {
                            bits.push(element.bit(bit));
                        }
                        LogicWord { bits }
                    } else {
                        element.slice(low_offset + width - 1, low_offset)
                    }
                }
                None => LogicWord::xes(*width),
            },
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

    fn mark_vcd_memory_change(&mut self, memory: usize, address: u64) {
        if self.vcd.is_some() {
            self.vcd_memory_changes.insert((memory, address));
        }
    }

    fn flush_vcd(&mut self) {
        if let Some(vcd) = self.vcd.as_mut() {
            let changes = std::mem::take(&mut self.vcd_changes);
            let memory_changes = std::mem::take(&mut self.vcd_memory_changes);
            vcd.advance(
                self.now,
                &self.vals,
                &changes,
                &self.memory_elements,
                &memory_changes,
            );
        } else {
            self.vcd_changes.clear();
            self.vcd_memory_changes.clear();
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
        Sens::Timed => {}
        Sens::Comb => {
            for net in stmt_nets(body) {
                if net < n {
                    out[net].push((pi, Edge::Any));
                }
            }
        }
        Sens::Any(net) => {
            if *net < n {
                out[*net].push((pi, Edge::Any));
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

fn ascii_string_length(word: &LogicWord) -> Option<usize> {
    for (byte, bits) in word.bits.chunks(8).enumerate() {
        let mut value = 0_u8;
        let mut known = true;
        for (bit, state) in bits.iter().enumerate() {
            match state.as_bool() {
                Some(true) => value |= 1 << bit,
                Some(false) => {}
                None => known = false,
            }
        }
        if !known {
            return None;
        }
        if value == 0 {
            return Some(byte);
        }
    }
    Some(word.bits.chunks(8).len())
}

fn packed_string_byte(word: &LogicWord, index: usize) -> LogicWord {
    if index >= word.width() / 8 {
        return LogicWord::zero(8);
    }
    word.slice(index * 8 + 7, index * 8)
}

fn change_string_case(word: &LogicWord, upper: bool) -> LogicWord {
    let Some(length) = ascii_string_length(word) else {
        return LogicWord::xes(word.width());
    };
    let mut bytes = Vec::with_capacity(length);
    for byte in 0..length {
        let value = packed_string_byte(word, byte);
        let Some(value) = value.to_u64().map(|value| value as u8) else {
            return LogicWord::xes(word.width());
        };
        bytes.push(if upper {
            value.to_ascii_uppercase()
        } else {
            value.to_ascii_lowercase()
        });
    }
    LogicWord::from_bytes_le(word.width(), &bytes)
}

fn compare_packed_strings(left: &LogicWord, right: &LogicWord, ignore_case: bool) -> Option<i32> {
    let left_length = ascii_string_length(left)?;
    let right_length = ascii_string_length(right)?;
    for byte in 0..left_length.min(right_length) {
        let left_value = packed_string_byte(left, byte)
            .to_u64()
            .map(|value| value as u8)?;
        let right_value = packed_string_byte(right, byte)
            .to_u64()
            .map(|value| value as u8)?;
        let compared = if ignore_case {
            left_value
                .to_ascii_lowercase()
                .cmp(&right_value.to_ascii_lowercase())
        } else {
            left_value.cmp(&right_value)
        };
        if compared != std::cmp::Ordering::Equal {
            return Some(if compared == std::cmp::Ordering::Less {
                -1
            } else {
                1
            });
        }
    }
    Some((left_length as i64 - right_length as i64).signum() as i32)
}

fn putc_packed_string(word: &LogicWord, index: &LogicWord, value: &LogicWord) -> LogicWord {
    let Some(length) = ascii_string_length(word) else {
        return LogicWord::xes(word.width());
    };
    let Some(index) = index.to_u64() else {
        return LogicWord::xes(word.width());
    };
    let Some(value) = value.to_u64().map(|value| value as u8) else {
        return LogicWord::xes(word.width());
    };
    if index >= length as u64 || value == 0 {
        return word.clone();
    }
    let index = index as usize;
    let mut bytes = Vec::with_capacity(length);
    for byte in 0..length {
        let Some(byte_value) = packed_string_byte(word, byte)
            .to_u64()
            .map(|value| value as u8)
        else {
            return LogicWord::xes(word.width());
        };
        bytes.push(if byte == index { value } else { byte_value });
    }
    LogicWord::from_bytes_le(word.width(), &bytes)
}

fn substring_packed_string(word: &LogicWord, left: &LogicWord, right: &LogicWord) -> LogicWord {
    let Some(length) = ascii_string_length(word) else {
        return LogicWord::xes(word.width());
    };
    let Some(left) = left.to_u64().map(|value| value as i64) else {
        return LogicWord::xes(word.width());
    };
    let Some(right) = right.to_u64().map(|value| value as i64) else {
        return LogicWord::xes(word.width());
    };
    if left < 0 || left > right || right >= length as i64 {
        return LogicWord::zero(word.width());
    }
    let mut bytes = Vec::with_capacity((right - left + 1) as usize);
    for byte in left as usize..=right as usize {
        let Some(byte_value) = packed_string_byte(word, byte)
            .to_u64()
            .map(|value| value as u8)
        else {
            return LogicWord::xes(word.width());
        };
        bytes.push(byte_value);
    }
    LogicWord::from_bytes_le(word.width(), &bytes)
}

fn parse_packed_string_radix(word: &LogicWord, radix: u32) -> Option<u32> {
    let length = ascii_string_length(word)?;
    let mut position = 0_usize;
    while position < length {
        let byte = packed_string_byte(word, position)
            .to_u64()
            .map(|value| value as u8)?;
        if !byte.is_ascii_whitespace() {
            break;
        }
        position += 1;
    }

    let mut negative = false;
    if position < length {
        let byte = packed_string_byte(word, position)
            .to_u64()
            .map(|value| value as u8)?;
        if byte == b'+' || byte == b'-' {
            negative = byte == b'-';
            position += 1;
        }
    }

    let mut value = 0_u32;
    let mut digits = 0_usize;
    while position < length {
        let byte = packed_string_byte(word, position)
            .to_u64()
            .map(|value| value as u8)?;
        let digit = match byte {
            b'0'..=b'9' => u32::from(byte - b'0'),
            b'a'..=b'f' if radix == 16 => u32::from(byte - b'a' + 10),
            b'A'..=b'F' if radix == 16 => u32::from(byte - b'A' + 10),
            _ => break,
        };
        if digit >= radix {
            break;
        }
        value = value.wrapping_mul(radix).wrapping_add(digit);
        digits += 1;
        position += 1;
    }

    if digits == 0 {
        return Some(0);
    }
    Some(if negative {
        value.wrapping_neg()
    } else {
        value
    })
}

fn format_packed_string_radix(value: &LogicWord, radix: u32) -> LogicWord {
    let Some(bits) = value.to_u64() else {
        return LogicWord::xes(512);
    };
    let rendered = match radix {
        10 => {
            let signed = if value.width() == 64 {
                bits as i64
            } else if value.width() > 0 && value.bit(value.width() - 1) == LogicValue::One {
                bits as i64 - (1_i64 << value.width())
            } else {
                bits as i64
            };
            signed.to_string()
        }
        16 => format!("{:x}", bits as u32),
        _ => format!("{:b}", bits as u32),
    };
    LogicWord::from_bytes_le(512, rendered.as_bytes())
}

fn concat_packed_strings(
    left: &LogicWord,
    left_length: usize,
    right: &LogicWord,
    right_length: usize,
) -> LogicWord {
    let width = left.width();
    let mut bits = vec![LogicValue::Zero; width];
    for byte in 0..width / 8 {
        if byte < left_length {
            for bit in 0..8 {
                bits[byte * 8 + bit] = left.bit(byte * 8 + bit);
            }
        } else {
            let right_byte = byte - left_length;
            if right_byte < right_length {
                for bit in 0..8 {
                    bits[byte * 8 + bit] = right.bit(right_byte * 8 + bit);
                }
            }
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
        Expr::StringLength { e } => walk_expr(e, out),
        Expr::StringConcat { left, right } => {
            walk_expr(left, out);
            walk_expr(right, out);
        }
        Expr::StringGetByte { e, index } => {
            walk_expr(e, out);
            walk_expr(index, out);
        }
        Expr::StringCase { e, .. } => walk_expr(e, out),
        Expr::StringCompare { left, right, .. } => {
            walk_expr(left, out);
            walk_expr(right, out);
        }
        Expr::StringPutc { e, index, value } => {
            walk_expr(e, out);
            walk_expr(index, out);
            walk_expr(value, out);
        }
        Expr::StringSubstr { e, left, right } => {
            walk_expr(e, out);
            walk_expr(left, out);
            walk_expr(right, out);
        }
        Expr::StringToRadix { e, .. } => walk_expr(e, out),
        Expr::StringFromRadix { value, .. } => walk_expr(value, out),
        Expr::MemoryRead { indexes, .. } => indexes.iter().for_each(|x| walk_expr(x, out)),
        Expr::MemoryReadPart { indexes, start, .. } => {
            indexes.iter().for_each(|x| walk_expr(x, out));
            walk_expr(start, out);
        }
    }
}

fn expr_memories(e: &Expr) -> Vec<usize> {
    let mut out = Vec::new();
    walk_expr_memories(e, &mut out);
    out
}

fn walk_expr_memories(e: &Expr, out: &mut Vec<usize>) {
    match e {
        Expr::Const(_) => {}
        Expr::Ref { .. } => {}
        Expr::MemoryRead {
            memory, indexes, ..
        } => {
            out.push(*memory);
            indexes.iter().for_each(|x| walk_expr_memories(x, out));
        }
        Expr::Un { a, .. } => walk_expr_memories(a, out),
        Expr::Bin { a, b, .. } => {
            walk_expr_memories(a, out);
            walk_expr_memories(b, out);
        }
        Expr::Cond { c, t, f } => {
            walk_expr_memories(c, out);
            walk_expr_memories(t, out);
            walk_expr_memories(f, out);
        }
        Expr::Concat(v) => v.iter().for_each(|x| walk_expr_memories(x, out)),
        Expr::Repeat { e, .. } => walk_expr_memories(e, out),
        Expr::Index { e, .. } => walk_expr_memories(e, out),
        Expr::Signed { e, .. } => walk_expr_memories(e, out),
        Expr::StringLength { e } => walk_expr_memories(e, out),
        Expr::StringConcat { left, right } => {
            walk_expr_memories(left, out);
            walk_expr_memories(right, out);
        }
        Expr::StringGetByte { e, index } => {
            walk_expr_memories(e, out);
            walk_expr_memories(index, out);
        }
        Expr::StringCase { e, .. } => walk_expr_memories(e, out),
        Expr::StringCompare { left, right, .. } => {
            walk_expr_memories(left, out);
            walk_expr_memories(right, out);
        }
        Expr::StringPutc { e, index, value } => {
            walk_expr_memories(e, out);
            walk_expr_memories(index, out);
            walk_expr_memories(value, out);
        }
        Expr::StringSubstr { e, left, right } => {
            walk_expr_memories(e, out);
            walk_expr_memories(left, out);
            walk_expr_memories(right, out);
        }
        Expr::StringToRadix { e, .. } => walk_expr_memories(e, out),
        Expr::StringFromRadix { value, .. } => walk_expr_memories(value, out),
        Expr::MemoryReadPart {
            memory,
            indexes,
            start,
            ..
        } => {
            out.push(*memory);
            indexes.iter().for_each(|x| walk_expr_memories(x, out));
            walk_expr_memories(start, out);
        }
    }
}

fn stmt_nets(s: &Stmt) -> Vec<usize> {
    let mut out = Vec::new();
    walk_stmt(s, &mut out);
    out
}

fn stmt_memories(s: &Stmt) -> Vec<usize> {
    let mut out = Vec::new();
    walk_stmt_memories(s, &mut out);
    out
}

fn walk_stmt_memories(s: &Stmt, out: &mut Vec<usize>) {
    match s {
        Stmt::Block(v) => v.iter().for_each(|x| walk_stmt_memories(x, out)),
        Stmt::Forever(b) => walk_stmt_memories(b, out),
        Stmt::NBl { e, .. } | Stmt::Bl { e, .. } => walk_expr_memories(e, out),
        Stmt::MemoryBl { memory, indexes, e } | Stmt::MemoryNBl { memory, indexes, e } => {
            out.push(*memory);
            indexes.iter().for_each(|x| walk_expr_memories(x, out));
            walk_expr_memories(e, out);
        }
        Stmt::MemoryBlPart {
            memory,
            indexes,
            start,
            e,
            ..
        }
        | Stmt::MemoryNBlPart {
            memory,
            indexes,
            start,
            e,
            ..
        } => {
            out.push(*memory);
            indexes.iter().for_each(|x| walk_expr_memories(x, out));
            walk_expr_memories(start, out);
            walk_expr_memories(e, out);
        }
        Stmt::NBlIndexed { start, e, .. } | Stmt::BlIndexed { start, e, .. } => {
            walk_expr_memories(start, out);
            walk_expr_memories(e, out);
        }
        Stmt::If { c, t, f } => {
            walk_expr_memories(c, out);
            walk_stmt_memories(t, out);
            if let Some(fs) = f {
                walk_stmt_memories(fs, out);
            }
        }
        Stmt::Case { e, arms, dflt } => {
            walk_expr_memories(e, out);
            for (label, body) in arms {
                walk_expr_memories(label, out);
                walk_stmt_memories(body, out);
            }
            if let Some(body) = dflt {
                walk_stmt_memories(body, out);
            }
        }
        Stmt::Delay { body, .. } => walk_stmt_memories(body, out),
        Stmt::Wait { cond, body } => {
            walk_expr_memories(cond, out);
            walk_stmt_memories(body, out)
        }
        Stmt::Fork { branches, .. } => branches
            .iter()
            .for_each(|branch| walk_stmt_memories(branch, out)),
        Stmt::While { cond, body, steps } => {
            walk_expr_memories(cond, out);
            walk_stmt_memories(body, out);
            steps.iter().for_each(|step| walk_stmt_memories(step, out));
        }
        Stmt::Break | Stmt::Continue => {}
        Stmt::Display { args, .. } => args.iter().for_each(|a| walk_expr_memories(a, out)),
        Stmt::Assert { cond, .. } => walk_expr_memories(cond, out),
        Stmt::Assume { cond, .. } => walk_expr_memories(cond, out),
        Stmt::Cover { cond, .. } => walk_expr_memories(cond, out),
        Stmt::WriteMem {
            memory,
            start,
            finish,
            ..
        } => {
            out.push(*memory);
            if let Some(start) = start {
                walk_expr_memories(start, out);
            }
            if let Some(finish) = finish {
                walk_expr_memories(finish, out);
            }
        }
        Stmt::Finish => {}
    }
}

fn collect_stmt_access(s: &Stmt, reads: &mut HashSet<usize>, writes: &mut HashSet<usize>) {
    match s {
        Stmt::Block(v) => v.iter().for_each(|s| collect_stmt_access(s, reads, writes)),
        Stmt::Forever(b) => collect_stmt_access(b, reads, writes),
        Stmt::NBl { lv, e } | Stmt::Bl { lv, e } => {
            reads.extend(expr_nets(e));
            writes.extend(lv_bits(lv));
        }
        Stmt::MemoryBl { indexes, e, .. } | Stmt::MemoryNBl { indexes, e, .. } => {
            reads.extend(indexes.iter().flat_map(expr_nets));
            reads.extend(expr_nets(e));
        }
        Stmt::MemoryBlPart {
            indexes, start, e, ..
        }
        | Stmt::MemoryNBlPart {
            indexes, start, e, ..
        } => {
            reads.extend(indexes.iter().flat_map(expr_nets));
            reads.extend(expr_nets(start));
            reads.extend(expr_nets(e));
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
        Stmt::Wait { cond, .. } => reads.extend(expr_nets(cond)),
        Stmt::Fork { branches, .. } => branches.iter().for_each(|branch| {
            let mut branch_reads = HashSet::new();
            let mut branch_writes = HashSet::new();
            collect_stmt_access(branch, &mut branch_reads, &mut branch_writes);
            reads.extend(branch_reads);
            writes.extend(branch_writes);
        }),
        Stmt::While { cond, body, steps } => {
            reads.extend(expr_nets(cond));
            collect_stmt_access(body, reads, writes);
            steps
                .iter()
                .for_each(|step| collect_stmt_access(step, reads, writes));
        }
        Stmt::Break | Stmt::Continue => {}
        Stmt::Display { args, .. } => reads.extend(args.iter().flat_map(expr_nets)),
        Stmt::Assert { cond, .. } => reads.extend(expr_nets(cond)),
        Stmt::Assume { cond, .. } => reads.extend(expr_nets(cond)),
        Stmt::Cover { cond, .. } => reads.extend(expr_nets(cond)),
        Stmt::WriteMem { start, finish, .. } => {
            if let Some(start) = start {
                reads.extend(expr_nets(start));
            }
            if let Some(finish) = finish {
                reads.extend(expr_nets(finish));
            }
        }
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
        Stmt::MemoryBl { indexes, e, .. } | Stmt::MemoryNBl { indexes, e, .. } => {
            indexes.iter().for_each(|x| walk_expr(x, out));
            walk_expr(e, out);
        }
        Stmt::MemoryBlPart {
            indexes, start, e, ..
        }
        | Stmt::MemoryNBlPart {
            indexes, start, e, ..
        } => {
            indexes.iter().for_each(|x| walk_expr(x, out));
            walk_expr(start, out);
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
        Stmt::Wait { cond, body } => {
            walk_expr(cond, out);
            walk_stmt(body, out)
        }
        Stmt::Fork { branches, .. } => branches.iter().for_each(|branch| walk_stmt(branch, out)),
        Stmt::While { cond, body, steps } => {
            walk_expr(cond, out);
            walk_stmt(body, out);
            steps.iter().for_each(|step| walk_stmt(step, out));
        }
        Stmt::Break | Stmt::Continue => {}
        Stmt::Display { args, .. } => args.iter().for_each(|a| walk_expr(a, out)),
        Stmt::Assert { cond, .. } => walk_expr(cond, out),
        Stmt::Assume { cond, .. } => walk_expr(cond, out),
        Stmt::Cover { cond, .. } => walk_expr(cond, out),
        Stmt::WriteMem { start, finish, .. } => {
            if let Some(start) = start {
                walk_expr(start, out);
            }
            if let Some(finish) = finish {
                walk_expr(finish, out);
            }
        }
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
#[path = "tests/mod.rs"]
mod tests;
