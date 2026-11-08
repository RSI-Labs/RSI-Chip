//! # eda-sim — event-driven 4-valued simulator
//!
//! Executes an elaborated [`Design`] (RTL, or gate-level via
//! [`eda_core::to_design::netlist_to_design`]) with Verilog-style
//! scheduling: active events, delta cycles for continuous assignments,
//! non-blocking (NBA) updates at the end of each time slot, edge and
//! combinational `always` processes, `#delay` continuations, `forever`,
//! `$display`, `$monitor`, deterministic unseeded random system functions,
//! `$finish`, and optional VCD dumping.

use eda_core::cancellation::FlowCancellation;
use eda_core::ir::{Design, Dir, EventEdge, EventWatch, Expr, ForkMode, LValue, Range, Sens, Stmt};
use eda_core::logic::{LogicValue, LogicWord};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

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

enum AssocNbaUpdate {
    Write {
        memory: usize,
        key: u64,
        value: LogicWord,
    },
    Replace {
        memory: usize,
        elements: AssociativeElements,
    },
}

#[derive(Clone)]
struct Cont<'d> {
    // Static process identity is used only for zero-delay reentrancy control
    // of `always` procedures.  Every runnable continuation additionally has a
    // runtime process identity, including initial blocks and fork children.
    process: Option<usize>,
    runtime_process: usize,
    join: Option<usize>,
    fork: Option<usize>,
    named_scopes: Vec<usize>,
    work: Vec<&'d Stmt>,
}

#[derive(Clone)]
struct ProcessState {
    children: Vec<usize>,
    active: bool,
    cancelled: bool,
    suspended: bool,
}

const PROCESS_STATE_FINISHED: u64 = 0;
const PROCESS_STATE_RUNNING: u64 = 1;
const PROCESS_STATE_WAITING: u64 = 2;
const PROCESS_STATE_SUSPENDED: u64 = 3;
const PROCESS_STATE_KILLED: u64 = 4;

struct ProcessWaiter<'d> {
    process: usize,
    target: Option<usize>,
    continuation: Cont<'d>,
}

#[derive(Clone)]
struct PastSample {
    time: u64,
    values: Vec<LogicWord>,
    last_result: Option<LogicWord>,
}

#[derive(Clone)]
struct DeferredAssertion<'d> {
    kind: eda_core::ir::ImmediateAssertionKind,
    scope: Option<String>,
    cond: &'d Expr,
    pass_action: Option<&'d Stmt>,
    fail_action: Option<&'d Stmt>,
}

struct NamedScopeState<'d> {
    name: String,
    owner: usize,
    continuation: Cont<'d>,
    processes: Vec<usize>,
    active: bool,
    owner_resumed: bool,
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

struct EventWaiter<'d> {
    watches: Vec<EventWatch>,
    continuation: Cont<'d>,
}

struct OrderedEventWaiter<'d> {
    events: Vec<usize>,
    next: usize,
    success: Option<&'d Stmt>,
    failure: Option<&'d Stmt>,
    continuation: Cont<'d>,
}

struct Monitor<'d> {
    parts: &'d [eda_core::ir::DispPart],
    args: &'d [Expr],
    last: Vec<LogicWord>,
}

struct Strobe<'d> {
    parts: &'d [eda_core::ir::DispPart],
    args: &'d [Expr],
}

struct FileStrobe<'d> {
    fd: &'d Expr,
    parts: &'d [eda_core::ir::DispPart],
    args: &'d [Expr],
    newline: bool,
}

const MAX_STEPS_PER_TIME: u64 = 200_000;
const MAX_EVENTS: u64 = 20_000_000;
static RETURN_BARRIER: Stmt = Stmt::ReturnBarrier;
static NAMED_BLOCK_END: Stmt = Stmt::NamedBlockEnd;

pub(crate) struct OrdinaryArrayElements {
    words: Vec<LogicWord>,
    touched: Vec<bool>,
}

pub(crate) enum MemoryElementStorage {
    Ordinary(OrdinaryArrayElements),
    Sparse(HashMap<u64, LogicWord>),
}

type AssociativeElements = HashMap<u64, LogicWord>;

impl MemoryElementStorage {
    fn get(&self, index: u64) -> Option<&LogicWord> {
        match self {
            Self::Ordinary(elements) => elements.words.get(usize::try_from(index).ok()?),
            Self::Sparse(elements) => elements.get(&index),
        }
    }

    fn touched_addresses(&self) -> Vec<u64> {
        match self {
            Self::Ordinary(elements) => elements
                .touched
                .iter()
                .enumerate()
                .filter(|(_, touched)| **touched)
                .map(|(index, _)| u64::try_from(index).unwrap_or(u64::MAX))
                .collect(),
            Self::Sparse(elements) => elements.keys().copied().collect(),
        }
    }

    fn set(&mut self, index: u64, value: LogicWord) -> bool {
        match self {
            Self::Ordinary(elements) => {
                let Some(index) = usize::try_from(index).ok() else {
                    return false;
                };
                let Some(element) = elements.words.get_mut(index) else {
                    return false;
                };
                if *element == value {
                    return false;
                }
                *element = value;
                elements.touched[index] = true;
                true
            }
            Self::Sparse(elements) => {
                if elements.get(&index) == Some(&value) {
                    return false;
                }
                elements.insert(index, value);
                true
            }
        }
    }

    fn clear(&mut self) {
        match self {
            Self::Ordinary(elements) => {
                let width = elements
                    .words
                    .first()
                    .map(LogicWord::width)
                    .unwrap_or_default();
                elements.words.fill(LogicWord::xes(width));
                elements.touched.fill(false);
            }
            Self::Sparse(elements) => elements.clear(),
        }
    }
}

pub struct Simulator<'d> {
    pub design: &'d Design,
    vals: Vec<LogicValue>,
    events: BTreeMap<u64, Vec<Update>>,
    conts: BTreeMap<u64, Vec<Cont<'d>>>,
    now: u64,
    nba: Vec<(usize, LogicValue)>,
    busy_procs: HashSet<usize>,
    process_states: Vec<ProcessState>,
    process_waiters: Vec<ProcessWaiter<'d>>,
    suspended_conts: Vec<Cont<'d>>,
    current_runtime_process: usize,
    named_scopes: Vec<NamedScopeState<'d>>,
    fork_joins: Vec<Option<ForkJoin<'d>>>,
    while_iterations: HashMap<usize, u64>,
    forever_iterations: HashMap<usize, (u64, u64)>,
    repeat_remaining: HashMap<usize, u64>,
    waiters: Vec<Waiter<'d>>,
    event_waiters: Vec<EventWaiter<'d>>,
    ordered_event_waiters: Vec<OrderedEventWaiter<'d>>,
    forced_values: HashMap<usize, LogicValue>,
    forced_saved_values: HashMap<usize, LogicValue>,
    procedural_drivers: Vec<ProceduralDriver>,
    procedural_wake_nets: HashSet<usize>,
    procedural_wake_memories: HashSet<usize>,
    procedural_time_wake: bool,
    net_change_times: HashMap<usize, u64>,
    memory_elements: Vec<MemoryElementStorage>,
    memory_aliases: Vec<usize>,
    memory_nba: Vec<MemoryNbaUpdate>,
    dynamic_lengths: Vec<u64>,
    dynamic_nba: Vec<(usize, u64)>,
    associative_elements: Vec<AssociativeElements>,
    associative_nba: Vec<AssocNbaUpdate>,
    memory_readers: Vec<Vec<usize>>,
    memory_assign_fanin: Vec<Vec<usize>>,
    assign_fanin: Vec<Vec<usize>>,
    proc_edges: Vec<Vec<(usize, Edge)>>,
    pub displays: Vec<String>,
    timeformat_units: i8,
    timeformat_precision: u8,
    timeformat_suffix: String,
    timeformat_min_width: usize,
    property_covers: BTreeMap<String, u64>,
    severity_counts: BTreeMap<eda_core::ir::SimulationSeverity, u64>,
    event_limit: u64,
    assertions_enabled: bool,
    scoped_assertions_enabled: BTreeMap<String, bool>,
    finished: bool,
    stopped: bool,
    paused_cont: Option<Cont<'d>>,
    final_blocks_executed: bool,
    print_stdout: bool,
    monitors: Vec<Monitor<'d>>,
    strobes: Vec<Strobe<'d>>,
    file_strobes: Vec<FileStrobe<'d>>,
    vcd: Option<VcdWriter>,
    vcd_changes: HashSet<usize>,
    vcd_memory_changes: HashSet<(usize, u64)>,
    event_count: u64,
    random_state: Cell<u64>,
    past_samples: RefCell<HashMap<usize, PastSample>>,
    deferred_assertions: Vec<DeferredAssertion<'d>>,
    final_assertions: Vec<DeferredAssertion<'d>>,
    next_file_descriptor: Cell<u32>,
    file_paths: RefCell<BTreeMap<u32, PathBuf>>,
    file_readers: RefCell<BTreeMap<u32, std::fs::File>>,
    file_writers: RefCell<BTreeMap<u32, std::fs::File>>,
    file_pushbacks: RefCell<BTreeMap<u32, u8>>,
    file_errors: RefCell<BTreeMap<u32, String>>,
    file_eof: RefCell<BTreeSet<u32>>,
    file_write_positions: RefCell<BTreeMap<u32, u64>>,
    has_zero_delay_feedback: bool,
    zero_delay_feedback_nets: Vec<String>,
    cancellation: FlowCancellation,
}

#[derive(Clone)]
struct ProceduralDriver {
    lv: eda_core::ir::LValue,
    e: Expr,
    sources: HashSet<usize>,
    source_memories: HashSet<usize>,
    time_dependent: bool,
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
            let total_bits =
                u128::from(memory.flat_length) * u128::try_from(memory.element_width).unwrap_or(0);
            if memory.associative || memory.dynamic {
                memory_elements.push(MemoryElementStorage::Sparse(HashMap::new()));
            } else if memory.storage == eda_core::ir::MemoryStorage::Ordinary {
                if total_bits > 4096 {
                    return Err(format!(
                        "ordinary array '{}' exceeds the 4096-bit dense-storage limit",
                        memory.name
                    ));
                }
                let length = usize::try_from(memory.flat_length)
                    .map_err(|_| format!("ordinary array '{}' is too large", memory.name))?;
                memory_elements.push(MemoryElementStorage::Ordinary(OrdinaryArrayElements {
                    words: vec![LogicWord::xes(memory.element_width); length],
                    touched: vec![false; length],
                }));
            } else {
                memory_elements.push(MemoryElementStorage::Sparse(HashMap::new()));
            }
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
        for connection in &design.array_connections {
            if connection.child >= memory_aliases.len() || connection.parent >= memory_aliases.len()
            {
                return Err("array connection refers to a missing array object".into());
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
        let foreach_iterator_nets = design
            .net_decls
            .iter()
            .filter(|declaration| declaration.name.starts_with("__foreach_iterator_"))
            .flat_map(|declaration| declaration.base..declaration.base + declaration.width)
            .collect::<HashSet<_>>();
        for net in &foreach_iterator_nets {
            if *net < n {
                proc_edges[*net].clear();
            }
        }
        for attempt in &design.assertion_attempts {
            if attempt.nets.iter().any(|&net| net >= n) {
                return Err("assertion attempt state refers to a missing net".into());
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
                reads.retain(|net| !foreach_iterator_nets.contains(net));
                writes.retain(|net| !foreach_iterator_nets.contains(net));
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
            process_states: design
                .processes
                .iter()
                .map(|_| ProcessState {
                    children: Vec::new(),
                    active: false,
                    cancelled: false,
                    suspended: false,
                })
                .collect(),
            process_waiters: Vec::new(),
            suspended_conts: Vec::new(),
            current_runtime_process: 0,
            named_scopes: Vec::new(),
            fork_joins: Vec::new(),
            while_iterations: HashMap::new(),
            forever_iterations: HashMap::new(),
            repeat_remaining: HashMap::new(),
            waiters: Vec::new(),
            event_waiters: Vec::new(),
            ordered_event_waiters: Vec::new(),
            forced_values: HashMap::new(),
            forced_saved_values: HashMap::new(),
            procedural_drivers: Vec::new(),
            procedural_wake_nets: HashSet::new(),
            procedural_wake_memories: HashSet::new(),
            procedural_time_wake: false,
            net_change_times: HashMap::new(),
            memory_elements,
            memory_aliases,
            memory_nba: Vec::new(),
            dynamic_lengths: vec![0; design.memories.len()],
            dynamic_nba: Vec::new(),
            associative_elements: vec![HashMap::new(); design.memories.len()],
            associative_nba: Vec::new(),
            memory_readers,
            memory_assign_fanin,
            assign_fanin,
            proc_edges,
            displays: Vec::new(),
            timeformat_units: -9,
            timeformat_precision: 0,
            timeformat_suffix: String::new(),
            timeformat_min_width: 0,
            property_covers: BTreeMap::new(),
            severity_counts: BTreeMap::new(),
            event_limit: MAX_EVENTS,
            assertions_enabled: true,
            scoped_assertions_enabled: BTreeMap::new(),
            finished: false,
            stopped: false,
            paused_cont: None,
            final_blocks_executed: false,
            print_stdout: false,
            monitors: mons
                .into_iter()
                .map(|(parts, args)| Monitor {
                    parts,
                    args,
                    last: Vec::new(),
                })
                .collect(),
            strobes: Vec::new(),
            file_strobes: Vec::new(),
            vcd: None,
            vcd_changes: HashSet::new(),
            vcd_memory_changes: HashSet::new(),
            event_count: 0,
            random_state: Cell::new(0x9E37_79B9_7F4A_7C15),
            past_samples: RefCell::new(HashMap::new()),
            deferred_assertions: Vec::new(),
            final_assertions: Vec::new(),
            next_file_descriptor: Cell::new(1),
            file_paths: RefCell::new(BTreeMap::new()),
            file_readers: RefCell::new(BTreeMap::new()),
            file_writers: RefCell::new(BTreeMap::new()),
            file_pushbacks: RefCell::new(BTreeMap::new()),
            file_errors: RefCell::new(BTreeMap::new()),
            file_eof: RefCell::new(BTreeSet::new()),
            file_write_positions: RefCell::new(BTreeMap::new()),
            has_zero_delay_feedback: !zero_delay_feedback_nets.is_empty(),
            zero_delay_feedback_nets,
            cancellation: FlowCancellation::new(),
        };
        for net in design.plusargs.values() {
            if *net >= n {
                return Err("plusarg state refers to a missing net".into());
            }
            sim.vals[*net] = LogicValue::Zero;
        }
        for net in design.plusarg_values.values() {
            if net.checked_add(512).is_none_or(|end| end > n) {
                return Err("plusarg value state refers to a missing net".into());
            }
            for (offset, bit) in LogicWord::zero(512).bits.into_iter().enumerate() {
                sim.vals[*net + offset] = bit;
            }
        }
        for init in &design.initials {
            let runtime_process = sim.new_process(None);
            sim.conts.entry(0).or_default().push(Cont {
                process: None,
                runtime_process,
                join: None,
                fork: None,
                named_scopes: Vec::new(),
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
                    runtime_process: pi,
                    join: None,
                    fork: None,
                    named_scopes: Vec::new(),
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

    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// Override the total event budget for long-running platform simulations.
    pub fn set_event_limit(&mut self, limit: u64) {
        self.event_limit = limit.max(1);
    }

    pub fn set_print_stdout(&mut self, on: bool) {
        self.print_stdout = on;
    }

    /// Install runtime `+ARG` strings before time-zero processes execute.
    pub fn set_plusargs<I, S>(&mut self, plusargs: I) -> Result<(), String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if self.now != 0 || self.finished {
            return Err("plusargs must be installed before simulation starts".into());
        }
        let names = plusargs
            .into_iter()
            .map(|argument| {
                argument
                    .as_ref()
                    .trim_start_matches('+')
                    .trim_start()
                    .to_string()
            })
            .collect::<Vec<_>>();
        for (query, net) in &self.design.plusargs {
            if *net >= self.vals.len() {
                return Err("plusarg state refers to a missing net".into());
            }
            self.vals[*net] = LogicValue::bit(
                names
                    .iter()
                    .any(|candidate| candidate.starts_with(query.as_str())),
            );
        }
        for (query, net) in &self.design.plusarg_values {
            if *net >= self.vals.len() {
                return Err("plusarg value state refers to a missing net".into());
            }
            let value = names.iter().find_map(|candidate| {
                candidate
                    .strip_prefix(query.as_str())
                    .and_then(|remainder| remainder.strip_prefix('='))
            });
            let value = value.unwrap_or_default();
            if value.len() > 64 {
                return Err(format!("plusarg '{query}' value exceeds 64 bytes"));
            }
            let word = LogicWord::from_bytes_le(512, value.as_bytes());
            for (index, bit) in word.bits.into_iter().enumerate() {
                self.vals[*net + index] = bit;
            }
        }
        Ok(())
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

    fn element_port(
        &self,
        port: &str,
        ordinary: bool,
    ) -> Result<(usize, eda_core::ir::MemoryPortDirection), String> {
        if ordinary {
            let Some(array_port) = self.design.array_ports.iter().find(|p| p.name == port) else {
                if self.design.memory_ports.iter().any(|p| p.name == port) {
                    return Err(format!(
                        "array port '{port}' is sparse, not an ordinary array"
                    ));
                }
                return Err(format!("no ordinary-array port named '{port}'"));
            };
            let array = self.canonical_memory(array_port.array);
            let declaration = self
                .design
                .memories
                .get(array)
                .ok_or_else(|| format!("ordinary-array port '{port}' has no array object"))?;
            if declaration.storage != eda_core::ir::MemoryStorage::Ordinary {
                return Err(format!(
                    "array port '{port}' is sparse, not an ordinary array"
                ));
            }
            Ok((array, array_port.direction))
        } else {
            let Some(memory_port) = self.design.memory_ports.iter().find(|p| p.name == port) else {
                if self.design.array_ports.iter().any(|p| p.name == port) {
                    return Err(format!(
                        "sparse-memory port '{port}' is ordinary, not a sparse memory"
                    ));
                }
                return Err(format!("no sparse-memory port named '{port}'"));
            };
            let memory = self.canonical_memory(memory_port.memory);
            let declaration = self
                .design
                .memories
                .get(memory)
                .ok_or_else(|| format!("sparse-memory port '{port}' has no memory object"))?;
            if declaration.storage != eda_core::ir::MemoryStorage::Sparse {
                return Err(format!(
                    "sparse-memory port '{port}' is ordinary, not a sparse memory"
                ));
            }
            Ok((memory, memory_port.direction))
        }
    }

    fn element_port_label(ordinary: bool) -> &'static str {
        if ordinary {
            "ordinary-array"
        } else {
            "sparse-memory"
        }
    }

    fn set_element_input(
        &mut self,
        port: &str,
        indexes: &[i64],
        value: &LogicWord,
        ordinary: bool,
    ) -> Result<(), String> {
        if self.cancellation.is_cancelled() {
            return Err("simulation cancelled".into());
        }
        self.settle()?;
        let (memory, direction) = self.element_port(port, ordinary)?;
        if !matches!(
            direction,
            eda_core::ir::MemoryPortDirection::In | eda_core::ir::MemoryPortDirection::InOut
        ) {
            return Err(format!(
                "{} port '{port}' is not an input",
                Self::element_port_label(ordinary)
            ));
        }
        let physical = self
            .design
            .memories
            .get(memory)
            .and_then(|declaration| declaration.physical_index(indexes))
            .ok_or_else(|| {
                format!(
                    "{} port '{port}' index is out of range",
                    Self::element_port_label(ordinary)
                )
            })?;
        self.apply_memory_update(memory, physical, value.clone())?;
        self.settle()
    }

    fn set_element_input_bulk(
        &mut self,
        port: &str,
        elements: &[(Vec<i64>, LogicWord)],
        ordinary: bool,
    ) -> Result<(), String> {
        if self.cancellation.is_cancelled() {
            return Err("simulation cancelled".into());
        }
        self.settle()?;
        let (memory, direction) = self.element_port(port, ordinary)?;
        if !matches!(
            direction,
            eda_core::ir::MemoryPortDirection::In | eda_core::ir::MemoryPortDirection::InOut
        ) {
            return Err(format!(
                "{} port '{port}' is not an input",
                Self::element_port_label(ordinary)
            ));
        }
        let label = Self::element_port_label(ordinary);
        let declaration = self
            .design
            .memories
            .get(memory)
            .ok_or_else(|| format!("{label} port '{port}' has no storage"))?;
        let physical = elements
            .iter()
            .map(|(indexes, _)| {
                declaration
                    .physical_index(indexes)
                    .ok_or_else(|| format!("{label} port '{port}' index is out of range"))
            })
            .collect::<Result<Vec<_>, String>>()?;
        for ((_, value), physical) in elements.iter().zip(physical) {
            self.apply_memory_update(memory, physical, value.clone())?;
        }
        self.settle()
    }

    fn set_element_input_range(
        &mut self,
        port: &str,
        ranges: &[(i64, i64)],
        values: &[LogicWord],
        ordinary: bool,
    ) -> Result<(), String> {
        if self.cancellation.is_cancelled() {
            return Err("simulation cancelled".into());
        }
        self.settle()?;
        let (memory, direction) = self.element_port(port, ordinary)?;
        if !matches!(
            direction,
            eda_core::ir::MemoryPortDirection::In | eda_core::ir::MemoryPortDirection::InOut
        ) {
            return Err(format!(
                "{} port '{port}' is not an input",
                Self::element_port_label(ordinary)
            ));
        }
        let label = Self::element_port_label(ordinary);
        let declaration = self
            .design
            .memories
            .get(memory)
            .ok_or_else(|| format!("{label} port '{port}' has no storage"))?;
        let physical = memory_range_physical_indexes(declaration, ranges, port)?;
        if physical.len() != values.len() {
            return Err(format!(
                "{label} port '{port}' range has {} elements but {} values were supplied",
                physical.len(),
                values.len()
            ));
        }
        for (value, physical) in values.iter().zip(physical) {
            self.apply_memory_update(memory, physical, value.clone())?;
        }
        self.settle()
    }

    fn read_element_output(
        &mut self,
        port: &str,
        indexes: &[i64],
        ordinary: bool,
    ) -> Result<LogicWord, String> {
        self.settle()?;
        let (memory, direction) = self.element_port(port, ordinary)?;
        if !matches!(
            direction,
            eda_core::ir::MemoryPortDirection::Out | eda_core::ir::MemoryPortDirection::InOut
        ) {
            return Err(format!(
                "{} port '{port}' is not an output",
                Self::element_port_label(ordinary)
            ));
        }
        let label = Self::element_port_label(ordinary);
        let declaration = self
            .design
            .memories
            .get(memory)
            .ok_or_else(|| format!("{label} port '{port}' has no storage"))?;
        let physical = declaration
            .physical_index(indexes)
            .ok_or_else(|| format!("{label} port '{port}' index is out of range"))?;
        Ok(self
            .memory_elements
            .get(memory)
            .and_then(|elements| elements.get(physical))
            .cloned()
            .unwrap_or_else(|| LogicWord::xes(declaration.element_width)))
    }

    fn read_element_output_bulk(
        &mut self,
        port: &str,
        indexes: &[Vec<i64>],
        ordinary: bool,
    ) -> Result<Vec<LogicWord>, String> {
        self.settle()?;
        let (memory, direction) = self.element_port(port, ordinary)?;
        if !matches!(
            direction,
            eda_core::ir::MemoryPortDirection::Out | eda_core::ir::MemoryPortDirection::InOut
        ) {
            return Err(format!(
                "{} port '{port}' is not an output",
                Self::element_port_label(ordinary)
            ));
        }
        let label = Self::element_port_label(ordinary);
        let declaration = self
            .design
            .memories
            .get(memory)
            .ok_or_else(|| format!("{label} port '{port}' has no storage"))?;
        let physical = indexes
            .iter()
            .map(|indexes| {
                declaration
                    .physical_index(indexes)
                    .ok_or_else(|| format!("{label} port '{port}' index is out of range"))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(physical
            .into_iter()
            .map(|physical| {
                self.memory_elements
                    .get(memory)
                    .and_then(|elements| elements.get(physical))
                    .cloned()
                    .unwrap_or_else(|| LogicWord::xes(declaration.element_width))
            })
            .collect())
    }

    fn read_element_output_range(
        &mut self,
        port: &str,
        ranges: &[(i64, i64)],
        ordinary: bool,
    ) -> Result<Vec<LogicWord>, String> {
        self.settle()?;
        let (memory, direction) = self.element_port(port, ordinary)?;
        if !matches!(
            direction,
            eda_core::ir::MemoryPortDirection::Out | eda_core::ir::MemoryPortDirection::InOut
        ) {
            return Err(format!(
                "{} port '{port}' is not an output",
                Self::element_port_label(ordinary)
            ));
        }
        let label = Self::element_port_label(ordinary);
        let declaration = self
            .design
            .memories
            .get(memory)
            .ok_or_else(|| format!("{label} port '{port}' has no storage"))?;
        let physical = memory_range_physical_indexes(declaration, ranges, port)?;
        Ok(physical
            .into_iter()
            .map(|physical| {
                self.memory_elements
                    .get(memory)
                    .and_then(|elements| elements.get(physical))
                    .cloned()
                    .unwrap_or_else(|| LogicWord::xes(declaration.element_width))
            })
            .collect())
    }

    pub fn set_ordinary_array_input(
        &mut self,
        port: &str,
        indexes: &[i64],
        value: &LogicWord,
    ) -> Result<(), String> {
        self.set_element_input(port, indexes, value, true)
    }

    pub fn set_ordinary_array_input_bulk(
        &mut self,
        port: &str,
        stimulus: &[(Vec<i64>, LogicWord)],
    ) -> Result<(), String> {
        self.set_element_input_bulk(port, stimulus, true)
    }

    pub fn set_ordinary_array_input_range(
        &mut self,
        port: &str,
        ranges: &[(i64, i64)],
        values: &[LogicWord],
    ) -> Result<(), String> {
        self.set_element_input_range(port, ranges, values, true)
    }

    pub fn read_ordinary_array_output(
        &mut self,
        port: &str,
        indexes: &[i64],
    ) -> Result<LogicWord, String> {
        self.read_element_output(port, indexes, true)
    }

    pub fn read_ordinary_array_output_bulk(
        &mut self,
        port: &str,
        indexes: &[Vec<i64>],
    ) -> Result<Vec<LogicWord>, String> {
        self.read_element_output_bulk(port, indexes, true)
    }

    pub fn read_ordinary_array_output_range(
        &mut self,
        port: &str,
        ranges: &[(i64, i64)],
    ) -> Result<Vec<LogicWord>, String> {
        self.read_element_output_range(port, ranges, true)
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

    /// Set one element of a top-level input or inout sparse-memory port.
    pub fn set_memory_input(
        &mut self,
        port: &str,
        indexes: &[i64],
        value: &LogicWord,
    ) -> Result<(), String> {
        self.set_element_input(port, indexes, value, false)
    }

    /// Set multiple elements of a top-level input or inout sparse-memory port.
    ///
    /// All indices are validated before any element is written, making a bulk
    /// stimulus update transactional with respect to index errors.
    pub fn set_memory_input_bulk(
        &mut self,
        port: &str,
        elements: &[(Vec<i64>, LogicWord)],
    ) -> Result<(), String> {
        self.set_element_input_bulk(port, elements, false)
    }

    /// Set a rectangular range of top-level input or inout sparse-memory
    /// elements in row-major logical-index order.
    pub fn set_memory_input_range(
        &mut self,
        port: &str,
        ranges: &[(i64, i64)],
        values: &[LogicWord],
    ) -> Result<(), String> {
        self.set_element_input_range(port, ranges, values, false)
    }

    /// Read one element from a top-level output or inout sparse-memory port.
    pub fn read_memory_output(&mut self, port: &str, indexes: &[i64]) -> Result<LogicWord, String> {
        self.read_element_output(port, indexes, false)
    }

    /// Read multiple elements from a top-level output or inout sparse-memory
    /// port in the order requested by the caller.
    pub fn read_memory_output_bulk(
        &mut self,
        port: &str,
        indexes: &[Vec<i64>],
    ) -> Result<Vec<LogicWord>, String> {
        self.read_element_output_bulk(port, indexes, false)
    }

    /// Read a rectangular range of top-level output or inout sparse-memory
    /// elements in row-major logical-index order.
    pub fn read_memory_output_range(
        &mut self,
        port: &str,
        ranges: &[(i64, i64)],
    ) -> Result<Vec<LogicWord>, String> {
        self.read_element_output_range(port, ranges, false)
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

    pub fn severity_hits(&self, severity: eda_core::ir::SimulationSeverity) -> u64 {
        self.severity_counts.get(&severity).copied().unwrap_or(0)
    }

    pub fn take_severity_counts(&mut self) -> BTreeMap<eda_core::ir::SimulationSeverity, u64> {
        std::mem::take(&mut self.severity_counts)
    }

    /// Deterministic in-process status for severity accounting: fatal or error
    /// reports are nonzero; warnings and informational reports are zero.
    pub fn severity_exit_status(&self) -> i32 {
        if self.severity_hits(eda_core::ir::SimulationSeverity::Fatal) > 0
            || self.severity_hits(eda_core::ir::SimulationSeverity::Error) > 0
        {
            1
        } else {
            0
        }
    }

    fn assertion_enabled(&self, scope: Option<&str>) -> bool {
        self.assertions_enabled
            && scope
                .and_then(|scope| self.scoped_assertions_enabled.get(scope).copied())
                .unwrap_or(true)
    }

    fn kill_assertion_attempts(&mut self, scopes: Option<&[String]>) {
        let targets = self
            .design
            .assertion_attempts
            .iter()
            .filter(|attempt| {
                scopes.is_none_or(|scopes| {
                    attempt
                        .scope
                        .as_deref()
                        .is_some_and(|scope| scopes.iter().any(|selected| selected == scope))
                })
            })
            .flat_map(|attempt| attempt.nets.iter().copied())
            .collect::<HashSet<_>>();
        let target_bits = targets
            .iter()
            .flat_map(|&net| {
                let width = self
                    .design
                    .net_decls
                    .iter()
                    .find(|declaration| declaration.base == net)
                    .map(|declaration| declaration.width)
                    .unwrap_or(1);
                net..net + width
            })
            .collect::<HashSet<_>>();
        self.deferred_assertions.retain(|assertion| {
            !scopes.is_some_and(|scopes| {
                assertion
                    .scope
                    .as_deref()
                    .is_some_and(|scope| scopes.iter().any(|selected| selected == scope))
            })
        });
        self.final_assertions.retain(|assertion| {
            !scopes.is_some_and(|scopes| {
                assertion
                    .scope
                    .as_deref()
                    .is_some_and(|scope| scopes.iter().any(|selected| selected == scope))
            })
        });
        self.nba.retain(|(net, _)| !target_bits.contains(net));
        if let Some(queue) = self.events.get_mut(&self.now) {
            queue.retain(|update| !target_bits.contains(&update.net));
        }
        for net in target_bits {
            self.schedule(Update {
                net,
                val: LogicValue::Zero,
                driver: None,
            });
        }
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
        if self.stopped {
            return Ok(());
        }
        loop {
            if self.cancellation.is_cancelled() {
                return Err("simulation cancelled".into());
            }
            if self.finished {
                return Ok(());
            }
            if self.stopped {
                return Ok(());
            }
            self.settle()?;
            if self.finished {
                return Ok(());
            }
            if self.stopped {
                return Ok(());
            }
            let next = self.events.keys().chain(self.conts.keys()).next().copied();
            match next {
                None => {
                    self.enter_time(t);
                    self.settle()?;
                    return Ok(());
                }
                Some(nt) if nt > t => {
                    self.enter_time(t);
                    self.settle()?;
                    return Ok(());
                }
                Some(nt) => self.enter_time(nt),
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

    /// Resumes a `$stop` pause and runs until absolute time `t`.
    pub fn resume_until(&mut self, t: u64) -> Result<(), String> {
        if !self.stopped {
            return self.run_until(t);
        }
        let continuation = self
            .paused_cont
            .take()
            .ok_or_else(|| "$stop has no resumable procedural continuation".to_string())?;
        self.stopped = false;
        self.conts.entry(self.now).or_default().push(continuation);
        self.run_until(t)
    }

    fn enter_time(&mut self, next: u64) {
        if next == self.now {
            return;
        }
        self.now = next;
        if self
            .procedural_drivers
            .iter()
            .any(|driver| driver.time_dependent)
        {
            self.procedural_time_wake = true;
        }
    }

    /// Advance exactly one time slot; false when quiescent.
    pub fn step(&mut self) -> Result<bool, String> {
        if self.cancellation.is_cancelled() {
            return Err("simulation cancelled".into());
        }
        if self.stopped {
            return Ok(false);
        }
        self.settle()?;
        if self.stopped {
            return Ok(false);
        }
        let next = self.events.keys().chain(self.conts.keys()).next().copied();
        match next {
            None => Ok(false),
            Some(nt) => {
                self.enter_time(nt);
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
        if self.forced_values.contains_key(&u.net) {
            return;
        }
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
        if self.stopped {
            return Ok(());
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
                if self.stopped {
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
                if self.stopped {
                    break;
                }
            }
            if self.finished {
                break;
            }
            if !self.procedural_wake_nets.is_empty()
                || !self.procedural_wake_memories.is_empty()
                || self.procedural_time_wake
            {
                if self.cancellation.is_cancelled() {
                    return Err("simulation cancelled".into());
                }
                steps_this_settle += 1;
                if steps_this_settle > MAX_STEPS_PER_TIME {
                    return Err("simulation stalled: zero-delay activity loop".into());
                }
                self.refresh_procedural_drivers()?;
                did = true;
            }
            if self.finished {
                break;
            }
            if !did
                && (!self.nba.is_empty()
                    || !self.memory_nba.is_empty()
                    || !self.dynamic_nba.is_empty()
                    || !self.associative_nba.is_empty())
            {
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
                let dynamic_updates = std::mem::take(&mut self.dynamic_nba);
                let associative_updates = std::mem::take(&mut self.associative_nba);
                for (memory, size) in dynamic_updates {
                    if self.cancellation.is_cancelled() {
                        return Err("simulation cancelled".into());
                    }
                    self.allocate_dynamic_memory(memory, size)?;
                }
                for update in associative_updates {
                    if self.cancellation.is_cancelled() {
                        return Err("simulation cancelled".into());
                    }
                    match update {
                        AssocNbaUpdate::Write { memory, key, value } => {
                            self.apply_associative_write(memory, key, value)?
                        }
                        AssocNbaUpdate::Replace { memory, elements } => {
                            self.apply_associative_copy(memory, elements)?
                        }
                    }
                }
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
        self.run_deferred_assertions(false)?;
        self.run_deferred_assertions(true)?;
        self.flush_strobes()?;
        self.fire_monitors();
        self.flush_vcd();
        Ok(())
    }

    fn run_deferred_assertions(&mut self, final_region: bool) -> Result<(), String> {
        loop {
            let queue = if final_region {
                std::mem::take(&mut self.final_assertions)
            } else {
                std::mem::take(&mut self.deferred_assertions)
            };
            if queue.is_empty() {
                return Ok(());
            }
            for assertion in queue {
                if self.cancellation.is_cancelled() {
                    return Err("simulation cancelled".into());
                }
                if !self.assertion_enabled(assertion.scope.as_deref()) {
                    continue;
                }
                let value = self.eval(assertion.cond);
                let is_true = value.reduce_or().as_bool() == Some(true);
                let action = match assertion.kind {
                    eda_core::ir::ImmediateAssertionKind::Cover => {
                        if is_true {
                            *self
                                .property_covers
                                .entry("immediate".to_string())
                                .or_insert(0) += 1;
                            assertion.pass_action
                        } else {
                            None
                        }
                    }
                    eda_core::ir::ImmediateAssertionKind::Assert
                    | eda_core::ir::ImmediateAssertionKind::Assume => {
                        if is_true {
                            assertion.pass_action
                        } else {
                            assertion.fail_action
                        }
                    }
                };
                let Some(action) = action else {
                    if !is_true {
                        let kind = if matches!(
                            assertion.kind,
                            eda_core::ir::ImmediateAssertionKind::Assert
                        ) {
                            "assertion"
                        } else {
                            "assumption"
                        };
                        return Err(format!(
                            "deferred {kind} failed at t={}: condition is false or unknown",
                            self.now
                        ));
                    }
                    continue;
                };
                let runtime_process = self.new_process(None);
                self.run_cont(Cont {
                    process: None,
                    runtime_process,
                    join: None,
                    fork: None,
                    named_scopes: Vec::new(),
                    work: vec![action],
                })?;
                if self.finished || self.stopped {
                    return Ok(());
                }
            }
        }
    }

    fn apply_update(&mut self, u: Update) -> Result<(), String> {
        if self.forced_values.contains_key(&u.net) {
            return Ok(());
        }
        let old = self.vals[u.net];
        if old == u.val {
            return Ok(());
        }
        self.event_count += 1;
        if self.event_count > self.event_limit {
            return Err("event budget exceeded".into());
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

    fn notify_direct_changes(
        &mut self,
        changes: Vec<(usize, LogicValue, LogicValue)>,
    ) -> Result<(), String> {
        for (net, old, newv) in changes {
            self.event_count += 1;
            if self.event_count > self.event_limit {
                return Err("event budget exceeded".into());
            }
            self.notify_change(net, old, newv)?;
        }
        Ok(())
    }

    fn install_procedural_driver(
        &mut self,
        lv: &eda_core::ir::LValue,
        e: &Expr,
    ) -> Result<(), String> {
        let targets = lv_bits(lv);
        let target_set = targets.iter().copied().collect::<HashSet<_>>();
        self.procedural_drivers.retain(|driver| {
            !lv_bits(&driver.lv)
                .iter()
                .any(|net| target_set.contains(net))
        });

        let value = self.eval(e).sized(targets.len());
        let mut changes = Vec::new();
        for (bit, net) in targets.iter().copied().enumerate() {
            let newv = value.bit(bit);
            let old = self.vals[net];
            self.forced_saved_values.entry(net).or_insert(old);
            self.forced_values.insert(net, newv);
            if old != newv {
                self.vals[net] = newv;
                self.mark_vcd_change(net);
                changes.push((net, old, newv));
            }
        }
        self.procedural_drivers.push(ProceduralDriver {
            lv: lv.clone(),
            e: e.clone(),
            sources: expr_nets(e).into_iter().collect(),
            source_memories: expr_memories(e)
                .into_iter()
                .map(|memory| self.canonical_memory(memory))
                .collect(),
            time_dependent: matches!(
                e,
                Expr::EventTriggered { .. } | Expr::SimulationTime { .. } | Expr::RealTime { .. }
            ),
        });
        self.notify_direct_changes(changes)
    }

    fn refresh_procedural_drivers(&mut self) -> Result<(), String> {
        let wakes = std::mem::take(&mut self.procedural_wake_nets);
        let memory_wakes = std::mem::take(&mut self.procedural_wake_memories);
        let time_wake = std::mem::take(&mut self.procedural_time_wake);
        if wakes.is_empty() && memory_wakes.is_empty() && !time_wake {
            return Ok(());
        }

        let mut updates = Vec::new();
        for driver in &self.procedural_drivers {
            if (!time_wake || !driver.time_dependent)
                && driver.sources.is_disjoint(&wakes)
                && driver.source_memories.is_disjoint(&memory_wakes)
            {
                continue;
            }
            let value = self.eval(&driver.e);
            for (bit, net) in lv_bits(&driver.lv).into_iter().enumerate() {
                let newv = value.bit(bit);
                let old = self.vals[net];
                if old == newv {
                    continue;
                }
                updates.push((net, old, newv));
            }
        }
        let mut changes = Vec::new();
        for (net, old, newv) in updates {
            {
                self.forced_values.insert(net, newv);
                self.vals[net] = newv;
                self.mark_vcd_change(net);
                changes.push((net, old, newv));
            }
        }
        self.notify_direct_changes(changes)
    }

    fn remove_procedural_driver(&mut self, lv: &eda_core::ir::LValue) {
        let targets = lv_bits(lv);
        let target_set = targets.iter().copied().collect::<HashSet<_>>();
        self.procedural_drivers.retain(|driver| {
            !lv_bits(&driver.lv)
                .iter()
                .any(|net| target_set.contains(net))
        });
        for net in targets {
            self.forced_values.remove(&net);
            self.forced_saved_values.remove(&net);
        }
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
        self.net_change_times.insert(changed_net, self.now);
        self.procedural_wake_nets.insert(changed_net);
        let pos = newv == LogicValue::One && old != LogicValue::One;
        let neg = newv == LogicValue::Zero && old != LogicValue::Zero;
        self.sample_clocking_inputs(changed_net, pos, neg)?;
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
                            runtime_process: pi,
                            join: None,
                            fork: None,
                            named_scopes: Vec::new(),
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
                runtime_process: pi,
                join: None,
                fork: None,
                named_scopes: Vec::new(),
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
                    // Delayed continuous-assign updates must also keep only
                    // the newest value for a (net, driver) pair. Two input
                    // changes in the same delta wave can queue two updates at
                    // the same future time; without this the older value may
                    // be applied last (LIFO) and clobber the newer one.
                    let queue = self.events.entry(t).or_default();
                    queue.retain(|existing| !(existing.net == net && existing.driver == Some(ai)));
                    queue.push(Update {
                        net,
                        val,
                        driver: Some(ai),
                    });
                }
            }
        }
        self.wake_net_waiters(changed_net, old, newv);
        Ok(())
    }

    fn sample_clocking_inputs(
        &mut self,
        changed_net: usize,
        pos: bool,
        neg: bool,
    ) -> Result<(), String> {
        let mut updates = Vec::new();
        for block in &self.design.clocking_blocks {
            let event_matches = block.event.net == changed_net
                && match block.event.edge {
                    EventEdge::Any => true,
                    EventEdge::Posedge => pos,
                    EventEdge::Negedge => neg,
                };
            if !event_matches {
                continue;
            }
            updates.extend(block.inputs.iter().cloned());
        }
        for input in updates {
            let target_bits = lv_bits(&input.target);
            let sample_bits = lv_bits(&input.sample);
            for (target, sample) in target_bits.into_iter().zip(sample_bits) {
                let newv = self.vals[target];
                let old = self.vals[sample];
                if old == newv {
                    continue;
                }
                self.vals[sample] = newv;
                self.mark_vcd_change(sample);
                self.event_count += 1;
                if self.event_count > self.event_limit {
                    return Err("event budget exceeded".into());
                }
                self.notify_change(sample, old, newv)?;
            }
        }
        Ok(())
    }

    fn allocate_dynamic_memory(&mut self, memory: usize, size: u64) -> Result<(), String> {
        let memory = self.canonical_memory(memory);
        let declaration = self
            .design
            .memories
            .get(memory)
            .ok_or_else(|| "dynamic-array allocation target is missing".to_string())?;
        if !declaration.dynamic {
            return Err(format!(
                "dynamic-array allocation target '{}' is not dynamic",
                declaration.name
            ));
        }
        if size > declaration.flat_length {
            return Err(format!(
                "dynamic-array '{}' allocation size {size} exceeds capacity {}",
                declaration.name, declaration.flat_length
            ));
        }
        if let Some(storage) = self.memory_elements.get_mut(memory) {
            storage.clear();
        }
        self.dynamic_lengths[memory] = size;
        self.notify_memory_write(memory)
    }

    fn queue_push(&mut self, memory: usize, value: &LogicWord, front: bool) -> Result<(), String> {
        let memory = self.canonical_memory(memory);
        let (name, capacity, element_width, queue, queue_bound) = {
            let Some(declaration) = self.design.memories.get(memory) else {
                return Err("queue push target is missing".into());
            };
            (
                declaration.name.clone(),
                declaration.flat_length,
                declaration.element_width,
                declaration.queue,
                declaration.queue_bound,
            )
        };
        if !queue {
            return Err(format!("queue push target '{name}' is not a queue"));
        }
        let length = self.dynamic_lengths[memory];
        let capacity = usize::try_from(capacity).unwrap_or(usize::MAX);
        if length >= capacity as u64 {
            if queue_bound.is_some() {
                // IEEE 1800-2023 7.10.5: writes beyond a bounded queue's
                // declared bound discard the out-of-bounds tail. Pushing at
                // the back therefore leaves a full bounded queue unchanged,
                // while pushing at the front drops its final element.
                if !front {
                    return Ok(());
                }
                let old_values = (0..length)
                    .map(|index| {
                        self.memory_elements
                            .get(memory)
                            .and_then(|elements| elements.get(index))
                            .cloned()
                            .unwrap_or_else(|| LogicWord::xes(element_width))
                    })
                    .collect::<Vec<_>>();
                for (index, old_value) in old_values.into_iter().enumerate().rev() {
                    if index + 1 < capacity {
                        self.apply_memory_update(memory, index as u64 + 1, old_value)?;
                    }
                }
                self.apply_memory_update(memory, 0, value.sized(element_width))?;
                return Ok(());
            }
            return Err(format!("queue '{name}' is full; capacity is {capacity}"));
        }
        let value = value.sized(element_width);
        let old_values = if front {
            (0..length)
                .map(|index| {
                    self.memory_elements
                        .get(memory)
                        .and_then(|elements| elements.get(index))
                        .cloned()
                        .unwrap_or_else(|| LogicWord::xes(element_width))
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        self.dynamic_lengths[memory] = length + 1;
        if front {
            for (index, old_value) in old_values.into_iter().enumerate().rev() {
                self.apply_memory_update(memory, index as u64 + 1, old_value)?;
            }
            self.apply_memory_update(memory, 0, value)?;
        } else {
            self.apply_memory_update(memory, length, value)?;
        }
        Ok(())
    }

    fn queue_pop(
        &mut self,
        memory: usize,
        destination: &LValue,
        front: bool,
    ) -> Result<(), String> {
        let memory = self.canonical_memory(memory);
        let (name, element_width, queue) = {
            let Some(declaration) = self.design.memories.get(memory) else {
                return Err("queue pop target is missing".into());
            };
            (
                declaration.name.clone(),
                declaration.element_width,
                declaration.queue,
            )
        };
        if !queue {
            return Err(format!("queue pop target '{name}' is not a queue"));
        }
        let length = self.dynamic_lengths[memory];
        if length == 0 {
            let value = LogicWord::xes(destination.range.width());
            self.assign_scalar(destination, value);
            return Ok(());
        }

        let physical_index = if front { 0 } else { length - 1 };
        let popped = self
            .memory_elements
            .get(memory)
            .and_then(|elements| elements.get(physical_index))
            .cloned()
            .unwrap_or_else(|| LogicWord::xes(element_width))
            .sized(destination.range.width());
        let old_values = if front {
            (1..length)
                .map(|index| {
                    self.memory_elements
                        .get(memory)
                        .and_then(|elements| elements.get(index))
                        .cloned()
                        .unwrap_or_else(|| LogicWord::xes(element_width))
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        self.dynamic_lengths[memory] = length - 1;
        if front {
            for (index, old_value) in old_values.into_iter().enumerate() {
                self.apply_memory_update(memory, index as u64, old_value)?;
            }
        }
        self.apply_memory_update(memory, length - 1, LogicWord::xes(element_width))?;
        self.assign_scalar(destination, popped);
        Ok(())
    }

    fn queue_index(&self, index: &Expr) -> Result<Option<i64>, String> {
        let value = self.eval(index);
        if value.width() > u64::BITS as usize {
            return Err("queue index exceeds 64 bits".into());
        }
        if !value.is_fully_known() {
            return Ok(None);
        }
        let bits = value.to_u64().expect("known queue index is numeric");
        let signed =
            if value.width() < u64::BITS as usize && bits & (1_u64 << (value.width() - 1)) != 0 {
                bits as i64 - (1_i64 << value.width())
            } else {
                bits as i64
            };
        Ok(Some(signed))
    }

    fn queue_insert(
        &mut self,
        memory: usize,
        index: &Expr,
        value: &LogicWord,
    ) -> Result<(), String> {
        let memory = self.canonical_memory(memory);
        let (name, capacity, element_width, queue, queue_bound) = {
            let Some(declaration) = self.design.memories.get(memory) else {
                return Err("queue insert target is missing".into());
            };
            (
                declaration.name.clone(),
                declaration.flat_length,
                declaration.element_width,
                declaration.queue,
                declaration.queue_bound,
            )
        };
        if !queue {
            return Err(format!("queue insert target '{name}' is not a queue"));
        }
        let Some(index) = self.queue_index(index)? else {
            return Ok(());
        };
        let length = self.dynamic_lengths[memory];
        let Some(length) = i64::try_from(length).ok() else {
            return Err(format!("queue '{name}' length is too large"));
        };
        if index < 0 || index > length {
            return Ok(());
        }
        let capacity = usize::try_from(capacity).unwrap_or(usize::MAX);
        let index =
            usize::try_from(index).map_err(|_| format!("queue '{name}' index is too large"))?;
        let length =
            usize::try_from(length).map_err(|_| format!("queue '{name}' length is too large"))?;
        let value = value.sized(element_width);

        if length < capacity {
            let moved = (index..length)
                .map(|old_index| {
                    self.memory_elements
                        .get(memory)
                        .and_then(|elements| elements.get(old_index as u64))
                        .cloned()
                        .unwrap_or_else(|| LogicWord::xes(element_width))
                })
                .collect::<Vec<_>>();
            self.dynamic_lengths[memory] = length as u64 + 1;
            for (offset, old_value) in moved.into_iter().enumerate().rev() {
                self.apply_memory_update(memory, index as u64 + offset as u64 + 1, old_value)?;
            }
            self.apply_memory_update(memory, index as u64, value)?;
        } else if queue_bound.is_some() {
            if index == capacity {
                return Ok(());
            }
            let last_retained = length.saturating_sub(1);
            let moved = (index..last_retained)
                .map(|old_index| {
                    self.memory_elements
                        .get(memory)
                        .and_then(|elements| elements.get(old_index as u64))
                        .cloned()
                        .unwrap_or_else(|| LogicWord::xes(element_width))
                })
                .collect::<Vec<_>>();
            for (offset, old_value) in moved.into_iter().enumerate().rev() {
                self.apply_memory_update(memory, index as u64 + offset as u64 + 1, old_value)?;
            }
            self.apply_memory_update(memory, index as u64, value)?;
        } else {
            return Err(format!("queue '{name}' is full; capacity is {capacity}"));
        }
        Ok(())
    }

    fn queue_delete(&mut self, memory: usize, index: &Expr) -> Result<(), String> {
        let memory = self.canonical_memory(memory);
        let (name, element_width, queue) = {
            let Some(declaration) = self.design.memories.get(memory) else {
                return Err("queue delete target is missing".into());
            };
            (
                declaration.name.clone(),
                declaration.element_width,
                declaration.queue,
            )
        };
        if !queue {
            return Err(format!("queue delete target '{name}' is not a queue"));
        }
        let Some(index) = self.queue_index(index)? else {
            return Ok(());
        };
        let length = self.dynamic_lengths[memory];
        if index < 0 || index >= length as i64 {
            return Ok(());
        }
        let index =
            usize::try_from(index).map_err(|_| format!("queue '{name}' index is too large"))?;
        let moved = (index as u64 + 1..length)
            .map(|old_index| {
                self.memory_elements
                    .get(memory)
                    .and_then(|elements| elements.get(old_index))
                    .cloned()
                    .unwrap_or_else(|| LogicWord::xes(element_width))
            })
            .collect::<Vec<_>>();
        self.dynamic_lengths[memory] = length - 1;
        for (offset, old_value) in moved.into_iter().enumerate() {
            self.apply_memory_update(memory, index as u64 + offset as u64, old_value)?;
        }
        self.apply_memory_update(memory, length - 1, LogicWord::xes(element_width))?;
        Ok(())
    }

    fn associative_key(&self, memory: usize, index: &Expr) -> Result<Option<u64>, String> {
        let memory = self.canonical_memory(memory);
        let declaration = self
            .design
            .memories
            .get(memory)
            .ok_or_else(|| "associative-array operation target is missing".to_string())?;
        if !declaration.associative {
            return Err(format!(
                "associative-array operation target '{}' is not associative",
                declaration.name
            ));
        }
        let value = self.eval(index).sized(declaration.index_width);
        if !value.is_fully_known() {
            return Ok(None);
        }

        Ok(value.to_u64())
    }

    fn apply_associative_write(
        &mut self,
        memory: usize,
        key: u64,
        value: LogicWord,
    ) -> Result<(), String> {
        let memory = self.canonical_memory(memory);
        let (name, capacity, element_width, associative) = {
            let Some(declaration) = self.design.memories.get(memory) else {
                return Err("associative-array write target is missing".into());
            };
            (
                declaration.name.clone(),
                declaration.flat_length,
                declaration.element_width,
                declaration.associative,
            )
        };
        if !associative {
            return Err(format!(
                "associative-array write target '{name}' is not associative"
            ));
        }
        let elements = self
            .associative_elements
            .get_mut(memory)
            .ok_or_else(|| format!("associative array '{name}' has no storage"))?;
        let capacity = usize::try_from(capacity).unwrap_or(usize::MAX);
        if !elements.contains_key(&key) && elements.len() >= capacity {
            return Err(format!(
                "associative array '{name}' is full; capacity is {capacity}"
            ));
        }
        elements.insert(key, value.sized(element_width));
        self.dynamic_lengths[memory] = elements.len() as u64;
        self.notify_memory_write(memory)
    }

    fn apply_associative_delete(&mut self, memory: usize, key: Option<u64>) -> Result<(), String> {
        let memory = self.canonical_memory(memory);
        let (name, associative) = {
            let Some(declaration) = self.design.memories.get(memory) else {
                return Err("associative-array delete target is missing".into());
            };
            (declaration.name.clone(), declaration.associative)
        };
        if !associative {
            return Err(format!(
                "associative-array delete target '{name}' is not associative"
            ));
        }
        let elements = self
            .associative_elements
            .get_mut(memory)
            .ok_or_else(|| format!("associative array '{name}' has no storage"))?;
        match key {
            Some(key) => {
                elements.remove(&key);
            }
            None => elements.clear(),
        }
        self.dynamic_lengths[memory] = elements.len() as u64;
        self.notify_memory_write(memory)
    }

    fn read_lvalue(&self, destination: &LValue) -> LogicWord {
        let bits = lv_bits(destination)
            .into_iter()
            .map(|net| self.vals.get(net).copied().unwrap_or(LogicValue::X))
            .collect();
        LogicWord { bits }
    }

    fn assign_scalar_immediate(
        &mut self,
        destination: &LValue,
        value: LogicWord,
    ) -> Result<(), String> {
        let width = destination.range.width();
        let mut changes = Vec::new();
        for (bit, net) in lv_bits(destination).into_iter().enumerate() {
            let new_value = value.bit(bit.min(width.saturating_sub(1)));
            if self.forced_values.contains_key(&net) {
                continue;
            }
            if let Some(current) = self.vals.get_mut(net) {
                let old = *current;
                if old != new_value {
                    *current = new_value;
                    self.mark_vcd_change(net);
                    changes.push((net, old, new_value));
                }
            }
        }
        self.notify_direct_changes(changes)
    }

    fn apply_associative_traversal(
        &mut self,
        memory: usize,
        operation: eda_core::ir::AssocTraversalOp,
        key: &LValue,
        destination: &LValue,
    ) -> Result<(), String> {
        let memory = self.canonical_memory(memory);
        let (name, index_width, index_signed, associative) = {
            let Some(declaration) = self.design.memories.get(memory) else {
                return Err("associative traversal target is missing".into());
            };
            (
                declaration.name.clone(),
                declaration.index_width,
                declaration.index_signed,
                declaration.associative,
            )
        };
        if !associative {
            return Err(format!(
                "associative traversal target '{name}' is not associative"
            ));
        }
        if key.range.width() != index_width {
            return Err(format!(
                "associative traversal key width {} does not match index width {index_width}",
                key.range.width()
            ));
        }

        let compare = |left: &u64, right: &u64| {
            associative_key_compare(*left, *right, index_width, index_signed)
        };
        let keys = self
            .associative_elements
            .get(memory)
            .ok_or_else(|| format!("associative array '{name}' has no storage"))?
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let found = match operation {
            eda_core::ir::AssocTraversalOp::First => keys.into_iter().min_by(compare),
            eda_core::ir::AssocTraversalOp::Last => keys.into_iter().max_by(compare),
            eda_core::ir::AssocTraversalOp::Next | eda_core::ir::AssocTraversalOp::Prev => {
                let current = self.read_lvalue(key);
                match current.to_u64() {
                    Some(current) => {
                        let wanted = match operation {
                            eda_core::ir::AssocTraversalOp::Next => std::cmp::Ordering::Greater,
                            _ => std::cmp::Ordering::Less,
                        };
                        let candidates = keys
                            .into_iter()
                            .filter(|candidate| compare(candidate, &current) == wanted);
                        match operation {
                            eda_core::ir::AssocTraversalOp::Next => candidates.min_by(compare),
                            _ => candidates.max_by(compare),
                        }
                    }
                    None => None,
                }
            }
        };

        let status = u64::from(found.is_some());
        self.assign_scalar_immediate(
            destination,
            LogicWord::from_u64(destination.range.width(), status),
        )?;
        if let Some(found) = found {
            self.assign_scalar_immediate(key, LogicWord::from_u64(index_width, found))?;
        }
        Ok(())
    }

    fn assign_scalar(&mut self, destination: &LValue, value: LogicWord) {
        let width = destination.range.width();
        for (bit, net) in lv_bits(destination).into_iter().enumerate() {
            self.schedule(Update {
                net,
                val: value.bit(bit.min(width.saturating_sub(1))),
                driver: None,
            });
        }
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
        if !storage.set(index, value) {
            return Ok(());
        }
        self.mark_vcd_memory_change(memory, index);
        self.event_count += 1;
        if self.event_count > self.event_limit {
            return Err("event budget exceeded".into());
        }
        self.notify_memory_write(memory)
    }

    fn sample_memory_copy(
        &self,
        source: usize,
        destination: usize,
    ) -> Result<Vec<(u64, LogicWord)>, String> {
        let source = self.canonical_memory(source);
        let destination = self.canonical_memory(destination);
        let source_declaration = self
            .design
            .memories
            .get(source)
            .ok_or_else(|| "memory copy source is missing".to_string())?;
        let destination_declaration = self
            .design
            .memories
            .get(destination)
            .ok_or_else(|| "memory copy destination is missing".to_string())?;
        if source_declaration.flat_length != destination_declaration.flat_length
            || source_declaration.element_width != destination_declaration.element_width
            || source_declaration.dimensions != destination_declaration.dimensions
        {
            return Err("memory copy source and destination shapes do not match".into());
        }
        let element_width = destination_declaration.element_width;
        let mut values = Vec::new();
        for index in 0..destination_declaration.flat_length {
            let value = self
                .memory_elements
                .get(source)
                .and_then(|elements| elements.get(index))
                .cloned()
                .unwrap_or_else(|| LogicWord::xes(element_width));
            values.push((index, value.sized(element_width)));
        }
        Ok(values)
    }

    fn sample_dense_dynamic_copy(
        &self,
        source: usize,
        destination: usize,
        kind: eda_core::ir::MemoryCopyKind,
    ) -> Result<Vec<LogicWord>, String> {
        let source = self.canonical_memory(source);
        let destination = self.canonical_memory(destination);
        let source_declaration = self
            .design
            .memories
            .get(source)
            .ok_or_else(|| "dynamic copy source is missing".to_string())?;
        let destination_declaration = self
            .design
            .memories
            .get(destination)
            .ok_or_else(|| "dynamic copy destination is missing".to_string())?;
        let category_matches = match kind {
            eda_core::ir::MemoryCopyKind::DynamicArray => {
                source_declaration.dynamic
                    && !source_declaration.queue
                    && !source_declaration.associative
                    && destination_declaration.dynamic
                    && !destination_declaration.queue
                    && !destination_declaration.associative
            }
            eda_core::ir::MemoryCopyKind::Queue => {
                source_declaration.queue && destination_declaration.queue
            }
            _ => false,
        };
        if !category_matches
            || source_declaration.element_width != destination_declaration.element_width
            || source_declaration.element_msb != destination_declaration.element_msb
            || source_declaration.element_lsb != destination_declaration.element_lsb
        {
            return Err("dynamic copy source and destination shapes do not match".into());
        }

        let source_length = self.dynamic_lengths[source];
        if source_length > source_declaration.flat_length {
            return Err(format!(
                "dynamic copy source '{}' has invalid length {source_length}",
                source_declaration.name
            ));
        }
        let mut length = usize::try_from(source_length).map_err(|_| {
            format!(
                "dynamic copy source '{}' is too large",
                source_declaration.name
            )
        })?;
        let destination_capacity =
            usize::try_from(destination_declaration.flat_length).unwrap_or(usize::MAX);
        if length > destination_capacity {
            if kind == eda_core::ir::MemoryCopyKind::Queue
                && destination_declaration.queue_bound.is_some()
            {
                length = destination_capacity;
            } else {
                return Err(format!(
                    "dynamic copy destination '{}' cannot hold {length} elements",
                    destination_declaration.name
                ));
            }
        }

        let element_width = source_declaration.element_width;
        Ok((0..length)
            .map(|index| {
                self.memory_elements
                    .get(source)
                    .and_then(|elements| elements.get(index as u64))
                    .cloned()
                    .unwrap_or_else(|| LogicWord::xes(element_width))
                    .sized(element_width)
            })
            .collect())
    }

    fn apply_dense_dynamic_copy(
        &mut self,
        destination: usize,
        values: Vec<LogicWord>,
    ) -> Result<(), String> {
        let destination = self.canonical_memory(destination);
        let length = u64::try_from(values.len())
            .map_err(|_| "dynamic copy destination length is too large".to_string())?;
        self.allocate_dynamic_memory(destination, length)?;
        for (index, value) in values.into_iter().enumerate() {
            self.apply_memory_update(destination, index as u64, value)?;
        }
        Ok(())
    }

    fn sample_associative_copy(
        &self,
        source: usize,
        destination: usize,
    ) -> Result<AssociativeElements, String> {
        let source = self.canonical_memory(source);
        let destination = self.canonical_memory(destination);
        let source_declaration = self
            .design
            .memories
            .get(source)
            .ok_or_else(|| "associative copy source is missing".to_string())?;
        let destination_declaration = self
            .design
            .memories
            .get(destination)
            .ok_or_else(|| "associative copy destination is missing".to_string())?;
        if !source_declaration.associative
            || !destination_declaration.associative
            || source_declaration.element_width != destination_declaration.element_width
            || source_declaration.element_msb != destination_declaration.element_msb
            || source_declaration.element_lsb != destination_declaration.element_lsb
            || source_declaration.index_width != destination_declaration.index_width
            || source_declaration.index_signed != destination_declaration.index_signed
        {
            return Err("associative copy source and destination shapes do not match".into());
        }
        Ok(self
            .associative_elements
            .get(source)
            .cloned()
            .unwrap_or_default())
    }

    fn apply_associative_copy(
        &mut self,
        destination: usize,
        elements: AssociativeElements,
    ) -> Result<(), String> {
        let destination = self.canonical_memory(destination);
        let (name, capacity, associative) = {
            let Some(declaration) = self.design.memories.get(destination) else {
                return Err("associative copy destination is missing".into());
            };
            (
                declaration.name.clone(),
                declaration.flat_length,
                declaration.associative,
            )
        };
        if !associative {
            return Err(format!(
                "associative copy target '{name}' is not associative"
            ));
        }
        let capacity = usize::try_from(capacity).unwrap_or(usize::MAX);
        if elements.len() > capacity {
            return Err(format!(
                "associative copy destination '{name}' cannot hold {} elements",
                elements.len()
            ));
        }
        let destination_elements = self
            .associative_elements
            .get_mut(destination)
            .ok_or_else(|| format!("associative array '{name}' has no storage"))?;
        destination_elements.clear();
        destination_elements.extend(elements);
        self.dynamic_lengths[destination] = destination_elements.len() as u64;
        self.notify_memory_write(destination)
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
            .get(index)
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
        if !storage.set(index, updated) {
            return Ok(());
        }
        self.mark_vcd_memory_change(memory, index);
        self.event_count += 1;
        if self.event_count > self.event_limit {
            return Err("event budget exceeded".into());
        }
        self.notify_memory_write(memory)
    }

    fn notify_memory_write(&mut self, memory: usize) -> Result<(), String> {
        self.procedural_wake_memories
            .insert(self.canonical_memory(memory));
        let readers = self.memory_readers.get(memory).cloned().unwrap_or_default();
        for pi in readers {
            let body = &self.design.processes[pi].body;
            self.conts.entry(self.now).or_default().push(Cont {
                process: Some(pi),
                runtime_process: pi,
                join: None,
                fork: None,
                named_scopes: Vec::new(),
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

    fn wake_net_waiters(&mut self, changed_net: usize, old: LogicValue, newv: LogicValue) {
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

        let mut ready_events = Vec::new();
        let mut index = 0;
        while index < self.event_waiters.len() {
            let edge_matches = self.event_waiters[index].watches.iter().any(|watch| {
                watch.net == changed_net
                    && match watch.edge {
                        EventEdge::Any => true,
                        EventEdge::Posedge => newv == LogicValue::One && old != LogicValue::One,
                        EventEdge::Negedge => newv == LogicValue::Zero && old != LogicValue::Zero,
                    }
            });
            if edge_matches {
                ready_events.push(self.event_waiters.remove(index).continuation);
            } else {
                index += 1;
            }
        }
        for continuation in ready_events {
            self.conts.entry(self.now).or_default().push(continuation);
        }

        let mut ready_ordered = Vec::new();
        let mut index = 0;
        while index < self.ordered_event_waiters.len() {
            let waiter = &mut self.ordered_event_waiters[index];
            if !waiter.events.contains(&changed_net) {
                index += 1;
                continue;
            }
            let mut successful_completion = false;
            let complete_or_failed = if waiter.events[waiter.next] == changed_net {
                waiter.next += 1;
                successful_completion = waiter.next == waiter.events.len();
                successful_completion
            } else {
                true
            };
            if complete_or_failed {
                let waiter = self.ordered_event_waiters.remove(index);
                let action = if successful_completion {
                    waiter.success
                } else {
                    waiter.failure
                };
                ready_ordered.push((action, waiter.continuation));
            } else {
                index += 1;
            }
        }
        for (action, continuation) in ready_ordered {
            let continuation = match action {
                Some(action) => {
                    let mut work = vec![action];
                    work.extend(continuation.work);
                    Cont {
                        process: continuation.process,
                        runtime_process: continuation.runtime_process,
                        join: continuation.join,
                        fork: continuation.fork,
                        named_scopes: continuation.named_scopes,
                        work,
                    }
                }
                None => continuation,
            };
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
                .get(address)
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

    fn run_final_blocks(&mut self) -> Result<(), String> {
        if self.final_blocks_executed {
            return Ok(());
        }
        self.final_blocks_executed = true;
        let final_blocks = self.final_blocks();
        self.finished = false;
        for body in final_blocks {
            if self.cancellation.is_cancelled() {
                return Err("simulation cancelled".into());
            }
            let runtime_process = self.new_process(None);
            self.run_cont(Cont {
                process: None,
                runtime_process,
                join: None,
                fork: None,
                named_scopes: Vec::new(),
                work: vec![body],
            })?;
        }
        self.finished = true;
        Ok(())
    }

    fn final_blocks(&self) -> &'d [Stmt] {
        self.design.final_blocks.as_slice()
    }

    fn new_process(&mut self, parent: Option<usize>) -> usize {
        let id = self.process_states.len();
        self.process_states.push(ProcessState {
            children: Vec::new(),
            active: true,
            cancelled: false,
            suspended: false,
        });
        if let Some(parent) = parent {
            if let Some(state) = self.process_states.get_mut(parent) {
                state.children.push(id);
            }
        }
        id
    }

    fn new_named_scope(&mut self, name: String, owner: usize, continuation: Cont<'d>) -> usize {
        let id = self.named_scopes.len();
        self.named_scopes.push(NamedScopeState {
            name,
            owner,
            continuation,
            processes: Vec::new(),
            active: true,
            owner_resumed: false,
        });
        id
    }

    fn close_named_scope(&mut self, scope: usize) {
        if let Some(state) = self.named_scopes.get_mut(scope) {
            state.active = false;
        }
    }

    fn close_named_scopes(&mut self, scopes: &[usize]) {
        for scope in scopes {
            self.close_named_scope(*scope);
        }
    }

    fn named_scope_has_active_processes(&self, scope: usize) -> bool {
        self.named_scopes.get(scope).is_some_and(|state| {
            state.processes.iter().any(|process| {
                self.process_states
                    .get(*process)
                    .is_some_and(|state| state.active)
            })
        })
    }

    fn retire_named_scopes(&mut self, scopes: &[usize], process: usize) {
        for scope in scopes {
            let Some(state) = self.named_scopes.get(*scope) else {
                continue;
            };
            let owned = state.owner == process;
            let member = state.processes.contains(&process);
            if owned {
                if let Some(state) = self.named_scopes.get_mut(*scope) {
                    state.owner_resumed = true;
                }
            }
            if !owned && !member {
                continue;
            }
            if !self.named_scope_has_active_processes(*scope) {
                if let Some(state) = self.named_scopes.get_mut(*scope) {
                    state.active = false;
                }
            }
        }
    }

    fn process_is_cancelled(&self, process: usize) -> bool {
        self.process_states
            .get(process)
            .is_some_and(|state| state.cancelled)
    }

    fn process_is_suspended(&self, process: usize) -> bool {
        self.process_states
            .get(process)
            .is_some_and(|state| state.suspended)
    }

    fn process_is_waiting(&self, process: usize) -> bool {
        self.conts
            .values()
            .any(|continuations| continuations.iter().any(|c| c.runtime_process == process))
            || self
                .waiters
                .iter()
                .any(|w| w.continuation.runtime_process == process)
            || self
                .event_waiters
                .iter()
                .any(|w| w.continuation.runtime_process == process)
            || self
                .ordered_event_waiters
                .iter()
                .any(|w| w.continuation.runtime_process == process)
            || self
                .process_waiters
                .iter()
                .any(|w| w.continuation.runtime_process == process)
            || self
                .fork_joins
                .iter()
                .flatten()
                .any(|state| state.continuation.runtime_process == process)
            || self
                .paused_cont
                .as_ref()
                .is_some_and(|c| c.runtime_process == process)
    }

    fn set_process_active(&mut self, process: usize, active: bool) {
        if let Some(state) = self.process_states.get_mut(process) {
            state.active = active;
            if !active {
                state.suspended = false;
            }
        }
    }

    fn active_immediate_children(&self, process: usize) -> bool {
        self.process_states.get(process).is_some_and(|state| {
            state.children.iter().any(|child| {
                self.process_states
                    .get(*child)
                    .is_some_and(|state| state.active && !state.cancelled)
            })
        })
    }

    fn process_is_active(&self, process: usize) -> bool {
        self.process_states
            .get(process)
            .is_some_and(|state| state.active && !state.cancelled)
    }

    fn process_handle(&self, handle: &Expr) -> Result<usize, String> {
        let word = self.eval(handle);
        let raw = word
            .to_u64()
            .ok_or_else(|| "process handle is unknown".to_string())?;
        let process =
            usize::try_from(raw).map_err(|_| "process handle is out of range".to_string())?;
        if process >= self.process_states.len() {
            return Err(format!("process handle {raw} does not identify a process"));
        }
        Ok(process)
    }

    fn suspend_process(&mut self, process: usize) {
        if let Some(state) = self.process_states.get_mut(process) {
            if state.active && !state.cancelled {
                state.suspended = true;
            }
        }
    }

    fn resume_process(&mut self, process: usize) {
        if let Some(state) = self.process_states.get_mut(process) {
            if state.active && !state.cancelled && state.suspended {
                state.suspended = false;
            } else {
                return;
            }
        }

        let mut index = 0;
        while index < self.suspended_conts.len() {
            if self.suspended_conts[index].runtime_process == process {
                let continuation = self.suspended_conts.remove(index);
                self.conts.entry(self.now).or_default().push(continuation);
            } else {
                index += 1;
            }
        }
    }

    fn process_tree(&self, process: usize) -> Vec<usize> {
        let mut tree = vec![process];
        let mut index = 0;
        while index < tree.len() {
            let current = tree[index];
            if let Some(state) = self.process_states.get(current) {
                let children = state.children.clone();
                let new_children = children
                    .into_iter()
                    .filter(|child| !tree.contains(child))
                    .collect::<Vec<_>>();
                tree.extend(new_children);
            }
            index += 1;
        }
        tree
    }

    fn cancel_process_tree(&mut self, process: usize) {
        let owned_scopes = self
            .named_scopes
            .iter()
            .enumerate()
            .filter(|(_, state)| state.owner == process && state.active)
            .map(|(scope, _)| scope)
            .collect::<Vec<_>>();
        self.close_named_scopes(&owned_scopes);
        let Some(state) = self.process_states.get_mut(process) else {
            return;
        };
        state.cancelled = true;
        state.active = false;
        state.suspended = false;
        let children = self
            .process_states
            .get(process)
            .map(|state| state.children.clone());
        for child in children.into_iter().flatten() {
            self.cancel_process_tree(child);
        }
        let tree = self.process_tree(process);
        self.suspended_conts
            .retain(|continuation| !tree.contains(&continuation.runtime_process));
    }

    fn wake_process_waiters(&mut self) {
        let mut ready = Vec::new();
        let mut index = 0;
        while index < self.process_waiters.len() {
            let owner_cancelled = self.process_is_cancelled(self.process_waiters[index].process);
            if owner_cancelled {
                self.process_waiters.remove(index);
            } else if match self.process_waiters[index].target {
                None => self.active_immediate_children(self.process_waiters[index].process),
                Some(target) => self.process_is_active(target),
            } {
                index += 1;
            } else {
                ready.push(self.process_waiters.remove(index).continuation);
            }
        }
        for continuation in ready {
            self.conts.entry(self.now).or_default().push(continuation);
        }
    }

    fn continuation_belongs_to_named_scope(
        continuation: &Cont<'d>,
        owner: usize,
        scope: usize,
    ) -> bool {
        continuation.runtime_process == owner && continuation.named_scopes.contains(&scope)
    }

    fn remove_named_scope_wakeups(&mut self, owner: usize, scope: usize) {
        for continuations in self.conts.values_mut() {
            continuations.retain(|continuation| {
                !Self::continuation_belongs_to_named_scope(continuation, owner, scope)
            });
        }
        self.waiters.retain(|waiter| {
            !Self::continuation_belongs_to_named_scope(&waiter.continuation, owner, scope)
        });
        self.event_waiters.retain(|waiter| {
            !Self::continuation_belongs_to_named_scope(&waiter.continuation, owner, scope)
        });
        self.ordered_event_waiters.retain(|waiter| {
            !Self::continuation_belongs_to_named_scope(&waiter.continuation, owner, scope)
        });
        self.process_waiters.retain(|waiter| {
            !Self::continuation_belongs_to_named_scope(&waiter.continuation, owner, scope)
        });
        self.suspended_conts.retain(|continuation| {
            !Self::continuation_belongs_to_named_scope(continuation, owner, scope)
        });
        if self.paused_cont.as_ref().is_some_and(|continuation| {
            Self::continuation_belongs_to_named_scope(continuation, owner, scope)
        }) {
            self.paused_cont = None;
        }
    }

    fn disable_named_block(&mut self, name: &str, flag: usize, current_process: usize) -> bool {
        let targets = self
            .named_scopes
            .iter()
            .enumerate()
            .filter(|(_, state)| state.active && state.name == name)
            .map(|(scope, state)| {
                (
                    scope,
                    state.owner,
                    state.continuation.clone(),
                    state.processes.clone(),
                )
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return false;
        }

        if flag < self.vals.len() {
            self.vals[flag] = LogicValue::One;
            self.mark_vcd_change(flag);
        }
        let mut terminate_current = false;
        for (scope, owner, continuation, processes) in targets {
            let should_resume_owner = self
                .named_scopes
                .get(scope)
                .is_some_and(|state| state.active && !state.owner_resumed);
            let owner_was_resumed = !should_resume_owner;
            self.close_named_scope(scope);
            self.remove_named_scope_wakeups(owner, scope);
            for process in processes {
                if process != owner {
                    self.cancel_process_tree(process);
                }
            }
            if should_resume_owner && !self.process_is_cancelled(owner) {
                if let Some(state) = self.named_scopes.get_mut(scope) {
                    state.owner_resumed = true;
                }
                self.conts.entry(self.now).or_default().push(continuation);
            }
            if (owner == current_process && !owner_was_resumed)
                || self.process_is_cancelled(current_process)
            {
                terminate_current = true;
            }
        }
        self.wake_process_waiters();
        terminate_current
    }

    fn run_cont(&mut self, c: Cont<'d>) -> Result<(), String> {
        let Cont {
            process,
            runtime_process,
            join,
            mut fork,
            mut named_scopes,
            mut work,
        } = c;
        if self.process_is_cancelled(runtime_process) {
            self.retire_named_scopes(&named_scopes, runtime_process);
            return Ok(());
        }
        if self.process_is_suspended(runtime_process) {
            self.suspended_conts.push(Cont {
                process,
                runtime_process,
                join,
                fork,
                named_scopes,
                work,
            });
            return Ok(());
        }
        self.set_process_active(runtime_process, true);
        self.current_runtime_process = runtime_process;
        let mut terminate_current = false;
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
                    const MAX_FOREVER_ITERATIONS: u64 = 65_536;
                    let key = s as *const Stmt as usize;
                    let iterations = self.forever_iterations.entry(key).or_insert((self.now, 0));
                    if iterations.0 != self.now {
                        *iterations = (self.now, 0);
                    }
                    iterations.1 += 1;
                    if iterations.1 > MAX_FOREVER_ITERATIONS {
                        return Err(
                            "forever loop exceeds the 65536-iteration execution limit".into()
                        );
                    }
                    work.push(s);
                    work.push(inner);
                }
                Stmt::Scope(inner) => {
                    work.push(&RETURN_BARRIER);
                    work.push(inner);
                }
                Stmt::ReturnBarrier => {}
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
                Stmt::Repeat { count, body } => {
                    const MAX_REPEAT_ITERATIONS: u64 = 65_536;
                    let key = s as *const Stmt as usize;
                    if !self.repeat_remaining.contains_key(&key) {
                        let value = self.eval(count);
                        if value.width() > u64::BITS as usize {
                            return Err("runtime repeat count exceeds 64 bits".into());
                        }
                        let Some(raw_count) = value.to_u64() else {
                            return Err("runtime repeat count must be known".into());
                        };
                        let iterations = if count.is_signed() {
                            let signed_count = sext_i64(&value, value.width());
                            u64::try_from(signed_count).unwrap_or(0)
                        } else {
                            raw_count
                        };
                        if iterations > MAX_REPEAT_ITERATIONS {
                            return Err(
                                "repeat loop exceeds the 65536-iteration execution limit".into()
                            );
                        }
                        if iterations == 0 {
                            continue;
                        }
                        self.repeat_remaining.insert(key, iterations);
                    }

                    let Some(mut remaining) = self.repeat_remaining.get(&key).copied() else {
                        unreachable!("active repeat loop must have a remaining count");
                    };
                    if remaining == 0 {
                        self.repeat_remaining.remove(&key);
                    } else {
                        remaining -= 1;
                        self.repeat_remaining.insert(key, remaining);
                        work.push(s);
                        work.push(body);
                    }
                }
                Stmt::Force { lv, e } => {
                    let value = self.eval(e).sized(lv_bits(lv).len());
                    let mut changes = Vec::new();
                    for (bit, net) in lv_bits(lv).into_iter().enumerate() {
                        let newv = value.bit(bit);
                        let old = self.vals[net];
                        self.forced_saved_values.entry(net).or_insert(old);
                        self.forced_values.insert(net, newv);
                        if old != newv {
                            self.vals[net] = newv;
                            self.mark_vcd_change(net);
                            changes.push((net, old, newv));
                        }
                    }
                    self.notify_direct_changes(changes)?;
                }
                Stmt::ProceduralAssign { lv, e } => {
                    self.install_procedural_driver(lv, e)?;
                }
                Stmt::Release { lv } => {
                    let targets = lv_bits(lv);
                    let mut restored = Vec::new();
                    for net in targets.iter().copied() {
                        if self.forced_values.remove(&net).is_some() {
                            if let Some(old) = self.forced_saved_values.remove(&net) {
                                restored.push((net, old));
                            }
                        }
                    }

                    let mut continuous_values = HashMap::new();
                    for assignment in &self.design.assigns {
                        let value = self.eval(&assignment.e);
                        for net in targets.iter().copied() {
                            if covers(&assignment.lv, net) {
                                let bit = lv_bit(&assignment.lv, net)
                                    .map(|index| value.bit(index))
                                    .unwrap_or(LogicValue::X);
                                let resolved = continuous_values.entry(net).or_insert(bit);
                                *resolved = resolved.resolve(bit);
                            }
                        }
                    }

                    let mut changes = Vec::new();
                    for (net, old) in restored {
                        let newv = continuous_values.get(&net).copied().unwrap_or(old);
                        let current = self.vals[net];
                        if current != newv {
                            self.vals[net] = newv;
                            self.mark_vcd_change(net);
                            changes.push((net, current, newv));
                        }
                    }
                    self.notify_direct_changes(changes)?;
                }
                Stmt::ProceduralDeassign { lv } => {
                    self.remove_procedural_driver(lv);
                }
                Stmt::Break => {
                    let Some(loop_position) = work.iter().rposition(|statement| {
                        matches!(statement, Stmt::While { .. } | Stmt::Repeat { .. })
                    }) else {
                        return Err("break is outside a runtime loop".into());
                    };
                    if let Stmt::Repeat { .. } = work[loop_position] {
                        self.repeat_remaining
                            .remove(&(work[loop_position] as *const Stmt as usize));
                    }
                    work.truncate(loop_position);
                }
                Stmt::Continue => {
                    let Some(loop_position) = work.iter().rposition(|statement| {
                        matches!(statement, Stmt::While { .. } | Stmt::Repeat { .. })
                    }) else {
                        return Err("continue is outside a runtime loop".into());
                    };
                    let loop_statement = work[loop_position];
                    work.truncate(loop_position);
                    match loop_statement {
                        Stmt::While { steps, .. } => {
                            work.push(loop_statement);
                            work.extend(steps.iter().rev());
                        }
                        Stmt::Repeat { .. } => {
                            let key = loop_statement as *const Stmt as usize;
                            let remaining = self.repeat_remaining.get(&key).copied();
                            if remaining.is_some_and(|remaining| remaining > 1) {
                                work.push(loop_statement);
                            } else {
                                self.repeat_remaining.remove(&key);
                            }
                        }
                        _ => unreachable!("loop position must point to a runtime loop"),
                    }
                }
                Stmt::Return => {
                    if let Some(scope_position) = work
                        .iter()
                        .rposition(|statement| matches!(statement, Stmt::ReturnBarrier))
                    {
                        work.truncate(scope_position);
                    } else {
                        work.clear();
                    }
                }
                Stmt::NamedBlock { name, body } => {
                    let continuation = Cont {
                        process,
                        runtime_process,
                        join,
                        fork,
                        named_scopes: named_scopes.clone(),
                        work: work.clone(),
                    };
                    let scope = self.new_named_scope(name.clone(), runtime_process, continuation);
                    named_scopes.push(scope);
                    work.push(&NAMED_BLOCK_END);
                    work.push(body);
                }
                Stmt::NamedBlockEnd => {
                    if let Some(scope) = named_scopes.pop() {
                        if let Some(state) = self.named_scopes.get_mut(scope) {
                            state.owner_resumed = true;
                        }
                        if !self.named_scope_has_active_processes(scope) {
                            self.close_named_scope(scope);
                        }
                    }
                }
                Stmt::DisableNamedBlock { name, flag } => {
                    if self.disable_named_block(name, *flag, runtime_process) {
                        terminate_current = true;
                    }
                }
                Stmt::DisableFork => self.disable_fork(runtime_process)?,
                Stmt::WaitFork => {
                    if self.active_immediate_children(runtime_process) {
                        let continuation = Cont {
                            process,
                            runtime_process,
                            join,
                            fork,
                            named_scopes,
                            work,
                        };
                        if let Some(process) = process {
                            self.busy_procs.remove(&process);
                        }
                        self.process_waiters.push(ProcessWaiter {
                            process: runtime_process,
                            target: None,
                            continuation,
                        });
                        return Ok(());
                    }
                }
                Stmt::ProcessKill(handle) => {
                    let target = self.process_handle(handle)?;
                    self.cancel_process_tree(target);
                    self.wake_process_waiters();
                    if target == runtime_process {
                        terminate_current = true;
                    }
                }
                Stmt::ProcessAwait(handle) => {
                    let target = self.process_handle(handle)?;
                    if self.process_is_active(target) {
                        let continuation = Cont {
                            process,
                            runtime_process,
                            join,
                            fork,
                            named_scopes,
                            work,
                        };
                        if let Some(process) = process {
                            self.busy_procs.remove(&process);
                        }
                        self.process_waiters.push(ProcessWaiter {
                            process: runtime_process,
                            target: Some(target),
                            continuation,
                        });
                        return Ok(());
                    }
                }
                Stmt::ProcessSuspend(handle) => {
                    let target = self.process_handle(handle)?;
                    if target == runtime_process {
                        self.suspend_process(target);
                        let continuation = Cont {
                            process,
                            runtime_process,
                            join,
                            fork,
                            named_scopes,
                            work,
                        };
                        if let Some(process) = process {
                            self.busy_procs.remove(&process);
                        }
                        self.suspended_conts.push(continuation);
                        return Ok(());
                    }
                    self.suspend_process(target);
                }
                Stmt::ProcessResume(handle) => {
                    let target = self.process_handle(handle)?;
                    self.resume_process(target);
                    self.wake_process_waiters();
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
                        if self.forced_values.contains_key(&net) {
                            continue;
                        }
                        let old = self.vals[net];
                        if old != newv {
                            self.vals[net] = newv;
                            self.mark_vcd_change(net);
                            changes.push((net, old, newv));
                        }
                    }
                    for (net, old, newv) in changes {
                        self.event_count += 1;
                        if self.event_count > self.event_limit {
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
                Stmt::MemoryCopy {
                    source,
                    destination,
                    nonblocking,
                    kind,
                } => match kind {
                    eda_core::ir::MemoryCopyKind::FixedArray => {
                        let destination = self.canonical_memory(*destination);
                        let values = self.sample_memory_copy(*source, destination)?;
                        if *nonblocking {
                            self.memory_nba.reserve(values.len());
                            for (index, value) in values {
                                self.memory_nba.push(MemoryNbaUpdate::Whole {
                                    memory: destination,
                                    index,
                                    value,
                                });
                            }
                        } else {
                            for (index, value) in values {
                                self.apply_memory_update(destination, index, value)?;
                            }
                        }
                    }
                    eda_core::ir::MemoryCopyKind::DynamicArray
                    | eda_core::ir::MemoryCopyKind::Queue => {
                        let values =
                            self.sample_dense_dynamic_copy(*source, *destination, *kind)?;
                        if *nonblocking {
                            let destination = self.canonical_memory(*destination);
                            self.dynamic_nba.push((destination, values.len() as u64));
                            self.memory_nba.reserve(values.len());
                            for (index, value) in values.into_iter().enumerate() {
                                self.memory_nba.push(MemoryNbaUpdate::Whole {
                                    memory: destination,
                                    index: index as u64,
                                    value,
                                });
                            }
                        } else {
                            self.apply_dense_dynamic_copy(*destination, values)?;
                        }
                    }
                    eda_core::ir::MemoryCopyKind::Associative => {
                        let elements = self.sample_associative_copy(*source, *destination)?;
                        if *nonblocking {
                            self.associative_nba.push(AssocNbaUpdate::Replace {
                                memory: self.canonical_memory(*destination),
                                elements,
                            });
                        } else {
                            self.apply_associative_copy(*destination, elements)?;
                        }
                    }
                },
                Stmt::MemoryNew {
                    memory,
                    size,
                    nonblocking,
                } => {
                    let value = self.eval(size);
                    if value.width() > u64::BITS as usize {
                        return Err("dynamic-array allocation size exceeds 64 bits".into());
                    }
                    let Some(size) = value.to_u64() else {
                        return Err("dynamic-array allocation size must be known".into());
                    };
                    let memory = self.canonical_memory(*memory);
                    if *nonblocking {
                        self.dynamic_nba.push((memory, size));
                    } else {
                        self.allocate_dynamic_memory(memory, size)?;
                    }
                }
                Stmt::MemoryDelete { memory } => {
                    let memory = self.canonical_memory(*memory);
                    self.allocate_dynamic_memory(memory, 0)?;
                }
                Stmt::QueuePush {
                    memory,
                    value,
                    front,
                } => {
                    let value = self.eval(value);
                    self.queue_push(*memory, &value, *front)?;
                }
                Stmt::QueuePop {
                    memory,
                    destination,
                    front,
                } => self.queue_pop(*memory, destination, *front)?,
                Stmt::QueueInsert {
                    memory,
                    index,
                    value,
                } => {
                    let value = self.eval(value);
                    self.queue_insert(*memory, index, &value)?;
                }
                Stmt::QueueDelete { memory, index } => self.queue_delete(*memory, index)?,
                Stmt::AssocWrite {
                    memory,
                    index,
                    value,
                    nonblocking,
                } => {
                    let key = self.associative_key(*memory, index)?;
                    let Some(key) = key else {
                        return Err("associative-array write key must be known".into());
                    };
                    let value = self.eval(value);
                    if *nonblocking {
                        self.associative_nba.push(AssocNbaUpdate::Write {
                            memory: self.canonical_memory(*memory),
                            key,
                            value,
                        });
                    } else {
                        self.apply_associative_write(*memory, key, value)?;
                    }
                }
                Stmt::AssocDelete { memory, index } => {
                    let key = match index {
                        Some(index) => {
                            let key = self.associative_key(*memory, index)?;
                            let Some(key) = key else {
                                return Err("associative-array delete key must be known".into());
                            };
                            Some(key)
                        }
                        None => None,
                    };
                    self.apply_associative_delete(*memory, key)?;
                }
                Stmt::AssocTraversal {
                    memory,
                    operation,
                    key,
                    destination,
                } => self.apply_associative_traversal(*memory, *operation, key, destination)?,
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
                        if self.event_count > self.event_limit {
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
                    // `work` is LIFO, so cloning it and pushing the delay body
                    // keeps the body first and then resumes the pending stack.
                    let mut exec_order = work.clone();
                    exec_order.push(body);
                    if let Some(process) = c.process {
                        self.busy_procs.remove(&process);
                    }
                    self.conts.entry(self.now + ns).or_default().push(Cont {
                        process: c.process,
                        runtime_process,
                        join: c.join,
                        fork,
                        named_scopes,
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
                    let join_index = self.fork_joins.len();
                    self.fork_joins.push(Some(ForkJoin {
                        mode: ForkMode::JoinNone,
                        released: true,
                        remaining: branches.len(),
                        continuation: Cont {
                            process: None,
                            runtime_process,
                            join: None,
                            fork: None,
                            named_scopes: named_scopes.clone(),
                            work: Vec::new(),
                        },
                    }));
                    fork = Some(join_index);
                    for branch in branches {
                        let child_process = self.new_process(Some(runtime_process));
                        for scope in &named_scopes {
                            if let Some(state) = self.named_scopes.get_mut(*scope) {
                                state.processes.push(child_process);
                            }
                        }
                        self.conts.entry(self.now).or_default().push(Cont {
                            process: None,
                            runtime_process: child_process,
                            join: c.join,
                            fork: Some(join_index),
                            named_scopes: named_scopes.clone(),
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
                            runtime_process,
                            join: c.join,
                            fork: Some(join_index),
                            named_scopes: named_scopes.clone(),
                            work,
                        },
                    }));
                    for branch in branches {
                        let child_process = self.new_process(Some(runtime_process));
                        for scope in &named_scopes {
                            if let Some(state) = self.named_scopes.get_mut(*scope) {
                                state.processes.push(child_process);
                            }
                        }
                        self.conts.entry(self.now).or_default().push(Cont {
                            process: None,
                            runtime_process: child_process,
                            join: Some(join_index),
                            fork: Some(join_index),
                            named_scopes: named_scopes.clone(),
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
                            runtime_process,
                            join: c.join,
                            fork,
                            named_scopes,
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
                Stmt::EventWait { watches, body } => {
                    let mut exec_order: Vec<&Stmt> = vec![&**body];
                    exec_order.extend(work.iter().rev().copied());
                    exec_order.reverse();
                    let continuation = Cont {
                        process: c.process,
                        runtime_process,
                        join: c.join,
                        fork,
                        named_scopes,
                        work: exec_order,
                    };
                    if let Some(process) = c.process {
                        self.busy_procs.remove(&process);
                    }
                    self.event_waiters.push(EventWaiter {
                        watches: watches.clone(),
                        continuation,
                    });
                    return Ok(());
                }
                Stmt::WaitOrder {
                    events,
                    success,
                    failure,
                } => {
                    let continuation = Cont {
                        process: c.process,
                        runtime_process,
                        join: c.join,
                        fork,
                        named_scopes,
                        work,
                    };
                    if let Some(process) = c.process {
                        self.busy_procs.remove(&process);
                    }
                    self.ordered_event_waiters.push(OrderedEventWaiter {
                        events: events.clone(),
                        next: 0,
                        success: success.as_deref(),
                        failure: failure.as_deref(),
                        continuation,
                    });
                    return Ok(());
                }
                Stmt::Display {
                    parts,
                    args,
                    strobe,
                    ..
                } => {
                    if *strobe {
                        self.strobes.push(Strobe { parts, args });
                    } else {
                        let line = self.format_display(parts, args);
                        if self.print_stdout {
                            print!("{}", line);
                        }
                        self.displays.push(line);
                    }
                }
                Stmt::TimeFormat {
                    units,
                    precision,
                    suffix,
                    min_width,
                } => {
                    self.timeformat_units = *units;
                    self.timeformat_precision = *precision;
                    self.timeformat_suffix = suffix.clone();
                    self.timeformat_min_width = *min_width;
                }
                Stmt::Assert { cond, message } => {
                    if !self.assertions_enabled {
                        continue;
                    }
                    let value = self.eval(cond);
                    if value.reduce_or().as_bool() != Some(true) {
                        let detail = message
                            .as_deref()
                            .unwrap_or("immediate assertion condition is false or unknown");
                        return Err(format!("assertion failed at t={}: {}", self.now, detail));
                    }
                }
                Stmt::Assume { cond, message } => {
                    if !self.assertions_enabled {
                        continue;
                    }
                    let value = self.eval(cond);
                    if value.reduce_or().as_bool() != Some(true) {
                        let detail = message
                            .as_deref()
                            .unwrap_or("assumption condition is false or unknown");
                        return Err(format!("assumption failed at t={}: {}", self.now, detail));
                    }
                }
                Stmt::Cover { label, cond } => {
                    if !self.assertions_enabled {
                        continue;
                    }
                    let value = self.eval(cond);
                    if value.reduce_or().as_bool() == Some(true) {
                        *self.property_covers.entry(label.clone()).or_insert(0) += 1;
                    }
                }
                Stmt::Severity(severity) => {
                    *self.severity_counts.entry(*severity).or_insert(0) += 1;
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
                Stmt::AssertControl(control) => match control {
                    eda_core::ir::AssertControl::On => self.assertions_enabled = true,
                    eda_core::ir::AssertControl::Off => self.assertions_enabled = false,
                    eda_core::ir::AssertControl::ScopedOn(scopes) => {
                        for scope in scopes {
                            self.scoped_assertions_enabled.insert(scope.clone(), true);
                        }
                    }
                    eda_core::ir::AssertControl::ScopedOff(scopes) => {
                        for scope in scopes {
                            self.scoped_assertions_enabled.insert(scope.clone(), false);
                        }
                    }
                    eda_core::ir::AssertControl::ScopedKill(scopes) => {
                        for scope in scopes {
                            self.scoped_assertions_enabled.insert(scope.clone(), false);
                        }
                        self.kill_assertion_attempts(Some(scopes));
                    }
                    eda_core::ir::AssertControl::Kill => {
                        self.assertions_enabled = false;
                        self.kill_assertion_attempts(None);
                    }
                },
                Stmt::ImmediateAssertion {
                    kind,
                    deferred,
                    scope,
                    cond,
                    pass_action,
                    fail_action,
                } => {
                    if !self.assertion_enabled(scope.as_deref()) {
                        continue;
                    }
                    if *deferred != eda_core::ir::ImmediateAssertionDeferred::None {
                        let assertion = DeferredAssertion {
                            kind: *kind,
                            scope: scope.clone(),
                            cond,
                            pass_action: pass_action.as_deref(),
                            fail_action: fail_action.as_deref(),
                        };
                        if *deferred == eda_core::ir::ImmediateAssertionDeferred::Final {
                            self.final_assertions.push(assertion);
                        } else {
                            self.deferred_assertions.push(assertion);
                        }
                        continue;
                    }
                    let value = self.eval(cond);
                    let is_true = value.reduce_or().as_bool() == Some(true);
                    match kind {
                        eda_core::ir::ImmediateAssertionKind::Cover => {
                            if is_true {
                                *self
                                    .property_covers
                                    .entry("immediate".to_string())
                                    .or_insert(0) += 1;
                                if let Some(action) = pass_action {
                                    work.push(action);
                                }
                            }
                        }
                        eda_core::ir::ImmediateAssertionKind::Assert
                        | eda_core::ir::ImmediateAssertionKind::Assume => {
                            if is_true {
                                if let Some(action) = pass_action {
                                    work.push(action);
                                }
                            } else if let Some(action) = fail_action {
                                work.push(action);
                            } else {
                                let kind =
                                    if matches!(kind, eda_core::ir::ImmediateAssertionKind::Assert)
                                    {
                                        "assertion"
                                    } else {
                                        "assumption"
                                    };
                                return Err(format!(
                                    "{kind} failed at t={}: condition is false or unknown",
                                    self.now
                                ));
                            }
                        }
                    }
                }
                Stmt::FileClose { fd } => {
                    let descriptor = self.file_descriptor(fd)?;
                    self.file_readers.borrow_mut().remove(&descriptor);
                    self.file_writers.borrow_mut().remove(&descriptor);
                    self.file_pushbacks.borrow_mut().remove(&descriptor);
                    self.file_errors.borrow_mut().remove(&descriptor);
                    self.file_eof.borrow_mut().remove(&descriptor);
                    self.file_write_positions.borrow_mut().remove(&descriptor);
                    if self.file_paths.borrow_mut().remove(&descriptor).is_none() {
                        return Err(format!(
                            "cannot close unknown file descriptor {descriptor} at t={}",
                            self.now
                        ));
                    }
                }
                Stmt::RandomSeeded { seed, destination } => {
                    let old_seed = self.read_lvalue(seed);
                    let Some(old_seed) = old_seed
                        .to_u64()
                        .and_then(|value| u32::try_from(value).ok())
                    else {
                        return Err(format!(
                            "seeded $random requires a known 32-bit seed at t={}",
                            self.now
                        ));
                    };
                    self.random_state.set(u64::from(old_seed));
                    let sample = self.next_u64();
                    let result = LogicWord::from_u64(
                        lv_bits(destination).len().max(1),
                        u64::from(sample as u32),
                    );
                    let next_seed = LogicWord::from_u64(32, sample >> 32);
                    self.assign_lvalue_word(destination, result)?;
                    self.assign_lvalue_word(seed, next_seed)?;
                }
                Stmt::FileFlush { fd } => {
                    let descriptor = self.file_descriptor(fd)?;
                    if let Some(writer) = self.file_writers.borrow_mut().get_mut(&descriptor) {
                        writer.flush().map_err(|error| {
                            format!(
                                "cannot flush file descriptor {descriptor} at t={}: {error}",
                                self.now
                            )
                        })?;
                    } else if !self.file_readers.borrow().contains_key(&descriptor) {
                        return Err(format!(
                            "cannot flush unknown file descriptor {descriptor} at t={}",
                            self.now
                        ));
                    }
                }
                Stmt::FileGets { fd, string, status } => {
                    let descriptor = self.file_descriptor(fd)?;
                    let mut bytes = Vec::new();
                    let mut reached_eof = false;
                    {
                        let mut readers = self.file_readers.borrow_mut();
                        let Some(reader) = readers.get_mut(&descriptor) else {
                            return Err(format!(
                                "cannot read write-mode or unknown file descriptor {descriptor} at t={}",
                                self.now
                            ));
                        };
                        loop {
                            if bytes.len() >= 63 {
                                if self.file_pushbacks.borrow().contains_key(&descriptor) {
                                    return Err(
                                        "$fgets line exceeds the 63-byte bounded string capacity"
                                            .into(),
                                    );
                                }
                                let mut overflow = [0_u8; 1];
                                match reader.read(&mut overflow) {
                                    Ok(1) => return Err(
                                        "$fgets line exceeds the 63-byte bounded string capacity"
                                            .into(),
                                    ),
                                    Ok(_) => {
                                        reached_eof = true;
                                        break;
                                    }
                                    Err(error) => {
                                        return Err(format!(
                                            "cannot read file descriptor {descriptor} at t={}: {error}",
                                            self.now
                                        ));
                                    }
                                }
                            }
                            if let Some(byte) = self.file_pushbacks.borrow_mut().remove(&descriptor)
                            {
                                bytes.push(byte);
                                if byte == b'\n' {
                                    break;
                                }
                                continue;
                            }
                            let mut byte = [0_u8; 1];
                            match reader.read(&mut byte) {
                                Ok(1) => {
                                    bytes.push(byte[0]);
                                    if byte[0] == b'\n' {
                                        break;
                                    }
                                }
                                Ok(_) => {
                                    reached_eof = true;
                                    break;
                                }
                                Err(error) => {
                                    return Err(format!(
                                        "cannot read file descriptor {descriptor} at t={}: {error}",
                                        self.now
                                    ));
                                }
                            }
                        }
                    }
                    if reached_eof {
                        self.file_eof.borrow_mut().insert(descriptor);
                    } else {
                        self.file_eof.borrow_mut().remove(&descriptor);
                    }
                    let count = bytes.len() as u64;
                    let string_value = LogicWord::from_bytes_le(string.decl_width, &bytes);
                    let status_width = lv_bits(status).len();
                    let status_value = LogicWord::from_u64(status_width, count);
                    self.assign_lvalue_word(string, string_value)?;
                    self.assign_lvalue_word(status, status_value)?;
                }
                Stmt::FileError {
                    fd,
                    message,
                    status,
                } => {
                    let descriptor = self.file_descriptor(fd)?;
                    if !self.file_readers.borrow().contains_key(&descriptor)
                        && !self.file_writers.borrow().contains_key(&descriptor)
                    {
                        return Err(format!(
                            "cannot query unknown file descriptor {descriptor} at t={}",
                            self.now
                        ));
                    }
                    let error = self
                        .file_errors
                        .borrow()
                        .get(&descriptor)
                        .cloned()
                        .unwrap_or_default();
                    let message_value =
                        LogicWord::from_bytes_le(message.decl_width, error.as_bytes());
                    let status_width = lv_bits(status).len();
                    let status_value = if error.is_empty() {
                        LogicWord::zero(status_width)
                    } else {
                        LogicWord::from_u64(status_width, u64::from(u32::MAX))
                    };
                    self.assign_lvalue_word(message, message_value)?;
                    self.assign_lvalue_word(status, status_value)?;
                }
                Stmt::FileRead { fd, target, status } => {
                    let descriptor = self.file_descriptor(fd)?;
                    if !self.file_readers.borrow().contains_key(&descriptor) {
                        self.file_errors.borrow_mut().insert(
                            descriptor,
                            "file descriptor is not open for reading".to_string(),
                        );
                        let status_width = lv_bits(status).len();
                        self.assign_lvalue_word(
                            status,
                            LogicWord::from_u64(status_width, u64::from(u32::MAX)),
                        )?;
                        continue;
                    }
                    let byte_count = target.decl_width / 8;
                    let mut bytes = Vec::with_capacity(byte_count);
                    let mut reached_eof = false;
                    let mut read_failed = false;
                    {
                        let mut readers = self.file_readers.borrow_mut();
                        let Some(reader) = readers.get_mut(&descriptor) else {
                            unreachable!("read descriptor existence was checked");
                        };
                        while bytes.len() < byte_count {
                            if let Some(byte) = self.file_pushbacks.borrow_mut().remove(&descriptor)
                            {
                                bytes.push(byte);
                                continue;
                            }
                            let mut byte = [0_u8; 1];
                            match reader.read(&mut byte) {
                                Ok(1) => bytes.push(byte[0]),
                                Ok(_) => {
                                    reached_eof = true;
                                    break;
                                }
                                Err(error) => {
                                    self.file_errors
                                        .borrow_mut()
                                        .insert(descriptor, format!("binary read failed: {error}"));
                                    read_failed = true;
                                    break;
                                }
                            }
                        }
                    }
                    if read_failed {
                        let status_width = lv_bits(status).len();
                        let failure = LogicWord::from_u64(status_width, u64::from(u32::MAX));
                        self.assign_lvalue_word(status, failure)?;
                        continue;
                    }
                    if reached_eof {
                        self.file_eof.borrow_mut().insert(descriptor);
                    } else {
                        self.file_eof.borrow_mut().remove(&descriptor);
                    }
                    let target_value = LogicWord::from_bytes_le(target.decl_width, &bytes);
                    let status_width = lv_bits(status).len();
                    let status_value = LogicWord::from_u64(status_width, bytes.len() as u64);
                    self.assign_lvalue_word(target, target_value)?;
                    self.assign_lvalue_word(status, status_value)?;
                }
                Stmt::FileReadMemory {
                    fd,
                    memory,
                    start,
                    count,
                    status,
                } => {
                    let descriptor = self.file_descriptor(fd)?;
                    if !self.file_readers.borrow().contains_key(&descriptor) {
                        self.file_errors.borrow_mut().insert(
                            descriptor,
                            "file descriptor is not open for reading".to_string(),
                        );
                        let status_width = lv_bits(status).len();
                        self.assign_lvalue_word(
                            status,
                            LogicWord::from_u64(status_width, u64::from(u32::MAX)),
                        )?;
                        continue;
                    }

                    let declaration = self
                        .design
                        .memories
                        .get(*memory)
                        .cloned()
                        .ok_or_else(|| "$fread target memory is missing".to_string())?;
                    let dimension = declaration.dimensions.first().cloned().ok_or_else(|| {
                        "$fread target memory has no unpacked dimension".to_string()
                    })?;
                    let last_logical =
                        dimension.base + dimension.step * (dimension.length as i64 - 1);
                    let lower_logical = dimension.base.min(last_logical);
                    let upper_logical = dimension.base.max(last_logical);
                    let logical_start = if let Some(start) = start {
                        self.eval(start).to_u64().ok_or_else(|| {
                            "$fread start index is unknown or out of range".to_string()
                        })? as i64
                    } else {
                        dimension.base
                    };
                    if logical_start < lower_logical || logical_start > upper_logical {
                        return Err(format!(
                            "$fread start index {logical_start} is outside memory '{}'",
                            declaration.name
                        ));
                    }
                    let remaining = if dimension.step >= 0 {
                        (upper_logical - logical_start) / dimension.step + 1
                    } else {
                        (logical_start - lower_logical) / -dimension.step + 1
                    };
                    let requested_elements = if let Some(count) = count {
                        self.eval(count).to_u64().ok_or_else(|| {
                            "$fread element count is unknown or out of range".to_string()
                        })?
                    } else {
                        u64::try_from(remaining)
                            .map_err(|_| "$fread memory length is too large".to_string())?
                    };
                    if requested_elements
                        > u64::try_from(remaining).map_err(|_| {
                            "$fread element count exceeds the remaining memory elements".to_string()
                        })?
                    {
                        return Err(format!(
                            "$fread element count {requested_elements} exceeds the {} elements remaining in memory '{}'",
                            remaining,
                            declaration.name
                        ));
                    }

                    let status_width = lv_bits(status).len();
                    if requested_elements == 0 {
                        self.assign_lvalue_word(status, LogicWord::from_u64(status_width, 0))?;
                        continue;
                    }

                    let bytes_per_element = declaration.element_width / 8;
                    let mut byte_count = 0_u64;
                    for element_offset in 0..requested_elements {
                        let logical_index = logical_start + dimension.step * element_offset as i64;
                        let physical_index = declaration
                            .physical_index(&[logical_index])
                            .ok_or_else(|| {
                                "$fread generated an invalid physical memory index".to_string()
                            })?;
                        let (mut bytes, reached_eof) =
                            match self.read_file_bytes(descriptor, bytes_per_element) {
                                Ok(result) => result,
                                Err(_) => {
                                    let failure =
                                        LogicWord::from_u64(status_width, u64::from(u32::MAX));
                                    self.assign_lvalue_word(status, failure)?;
                                    continue;
                                }
                            };
                        byte_count += bytes.len() as u64;
                        let partial_element = bytes.len() < bytes_per_element;
                        if bytes.len() < bytes_per_element {
                            bytes.resize(bytes_per_element, 0);
                        }
                        let value = LogicWord::from_bytes_le(declaration.element_width, &bytes);
                        self.apply_memory_update(*memory, physical_index, value)?;
                        if partial_element || reached_eof {
                            break;
                        }
                    }
                    let status_value = LogicWord::from_u64(status_width, byte_count);
                    self.assign_lvalue_word(status, status_value)?;
                }
                Stmt::FileScan {
                    fd,
                    format,
                    targets,
                    status,
                } => {
                    let descriptor = self.file_descriptor(fd)?;
                    if !self.file_readers.borrow().contains_key(&descriptor) {
                        self.file_errors.borrow_mut().insert(
                            descriptor,
                            "file descriptor is not open for reading".to_string(),
                        );
                        let status_width = lv_bits(status).len();
                        let failure = LogicWord::from_u64(status_width, u64::from(u32::MAX));
                        self.assign_lvalue_word(status, failure)?;
                        continue;
                    }
                    let targets = targets.clone();
                    let target_signed = scan_target_signedness(self.design, &targets);
                    let (converted, input_failure) = {
                        let mut input = FileScanInput {
                            simulator: self,
                            descriptor,
                        };
                        scan_fields(&mut input, format, &targets, &target_signed, "$fscanf")?
                    };
                    for (target, value) in &converted {
                        self.assign_lvalue_word(target, value.clone())?;
                    }
                    let status_width = lv_bits(status).len();
                    let return_value = if input_failure && converted.is_empty() {
                        u64::from(u32::MAX)
                    } else {
                        converted.len() as u64
                    };
                    self.assign_lvalue_word(
                        status,
                        LogicWord::from_u64(status_width, return_value),
                    )?;
                }
                Stmt::StringScan {
                    source,
                    format,
                    targets,
                    status,
                } => {
                    let source_value = self.eval(source);
                    let Some(mut input) = StringScanInput::new(&source_value) else {
                        return Err("$sscanf input string contains unknown bits".to_string());
                    };
                    let targets = targets.clone();
                    let target_signed = scan_target_signedness(self.design, &targets);
                    let (converted, input_failure) =
                        scan_fields(&mut input, format, &targets, &target_signed, "$sscanf")?;
                    for (target, value) in &converted {
                        self.assign_lvalue_word(target, value.clone())?;
                    }
                    let status_width = lv_bits(status).len();
                    let return_value = if input_failure && converted.is_empty() {
                        u64::from(u32::MAX)
                    } else {
                        converted.len() as u64
                    };
                    self.assign_lvalue_word(
                        status,
                        LogicWord::from_u64(status_width, return_value),
                    )?;
                }
                Stmt::FileDisplay {
                    fd,
                    parts,
                    args,
                    newline,
                    strobe,
                } => {
                    if *strobe {
                        self.file_strobes.push(FileStrobe {
                            fd,
                            parts,
                            args,
                            newline: *newline,
                        });
                    } else {
                        self.write_file_display(fd, parts, args, *newline)?;
                    }
                }
                Stmt::Finish => {
                    self.finished = true;
                    self.close_named_scopes(&named_scopes);
                    self.set_process_active(runtime_process, false);
                    self.wake_process_waiters();
                    self.run_final_blocks()?;
                    return Ok(());
                }
                Stmt::Stop => {
                    self.stopped = true;
                    self.paused_cont = Some(Cont {
                        process,
                        runtime_process,
                        join,
                        fork,
                        named_scopes,
                        work,
                    });
                    return Ok(());
                }
            }
            if self.finished {
                return Ok(());
            }
            if self.stopped {
                return Ok(());
            }
            if terminate_current {
                break;
            }
        }
        if let Some(process) = process {
            self.busy_procs.remove(&process);
        }
        if let Some(join) = join {
            self.complete_join(join);
        }
        self.set_process_active(runtime_process, false);
        if self.process_is_cancelled(runtime_process) || !terminate_current {
            self.retire_named_scopes(&named_scopes, runtime_process);
        }
        self.wake_process_waiters();
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
            let mode = state.mode;
            let continuation = state.continuation.clone();
            if mode != ForkMode::JoinAny {
                self.fork_joins[index] = None;
            }
            {
                self.conts.entry(self.now).or_default().push(continuation);
            }
        }
    }

    fn disable_fork(&mut self, current_process: usize) -> Result<(), String> {
        // IEEE 1800 process semantics are broader than the innermost lexical
        // fork: `disable fork` terminates every direct or indirect descendant
        // of the process executing the statement.  It does not terminate the
        // current process itself.
        let children = self
            .process_states
            .get(current_process)
            .map(|state| state.children.clone())
            .unwrap_or_default();
        for child in children {
            self.cancel_process_tree(child);
        }
        self.wake_process_waiters();
        Ok(())
    }

    fn format_display(&self, parts: &[eda_core::ir::DispPart], args: &[Expr]) -> String {
        use eda_core::ir::DispPart;
        let mut out = String::new();
        for p in parts {
            match p {
                DispPart::Str(s) => out.push_str(s),
                DispPart::Time(index) => out.push_str(&self.format_time_value(&args[*index])),
                DispPart::Arg(i, spec) => {
                    out.push_str(&self.format_display_argument(&args[*i], *spec, None));
                }
                DispPart::FormattedArg {
                    index,
                    spec,
                    width,
                    precision,
                    left_justify,
                } => {
                    let rendered = if *spec == 't' {
                        self.format_time_value(&args[*index])
                    } else {
                        self.format_display_argument(&args[*index], *spec, *precision)
                    };
                    let padding = if matches!(spec, 'b' | 'h' | 'o') {
                        '0'
                    } else {
                        ' '
                    };
                    let padded = pad_formatted_text(&rendered, *width, *left_justify, padding);
                    out.push_str(&padded);
                }
            }
        }
        out
    }

    fn format_display_argument(
        &self,
        argument: &Expr,
        spec: char,
        precision: Option<usize>,
    ) -> String {
        if let Expr::RealConst(real) = argument {
            let precision = precision.unwrap_or(6);
            return match spec {
                'p' | 'P' => format!("{real}"),
                'e' => format_real_exponent(*real, precision),
                'f' => format!("{real:.*}", precision),
                'g' => format_real_general(*real, precision),
                _ => format!("{real}"),
            };
        }
        if matches!(
            argument,
            Expr::ShortRealConst(_)
                | Expr::ShortRealRef { .. }
                | Expr::ShortRealBinary { .. }
                | Expr::ShortRealNeg(_)
                | Expr::ShortRealToReal(_)
                | Expr::RealToShortReal(_)
                | Expr::BitsToShortReal(_)
        ) {
            let Some(real) = self.eval_real(argument) else {
                return "x".to_string();
            };
            let precision = precision.unwrap_or(6);
            return match spec {
                'p' | 'P' => format!("{real}"),
                'e' => format_real_exponent(real, precision),
                'f' => format!("{real:.*}", precision),
                'g' => format_real_general(real, precision),
                _ => format!("{real}"),
            };
        }
        if matches!(argument, Expr::IntToReal { .. } | Expr::BitsToReal(_)) {
            let Some(real) = self.eval_real(argument) else {
                return "x".to_string();
            };
            let precision = precision.unwrap_or(6);
            return match spec {
                'p' | 'P' => format!("{real}"),
                'e' => format_real_exponent(real, precision),
                'f' => format!("{real:.*}", precision),
                'g' => format_real_general(real, precision),
                _ => format!("{real}"),
            };
        }
        if let Expr::RealRef { .. } = argument {
            let bits = self.eval(argument);
            if !bits.is_fully_known() {
                return "x".to_string();
            }
            let real = f64::from_bits(bits.to_u64().unwrap_or_default());
            let precision = precision.unwrap_or(6);
            return match spec {
                'e' => format_real_exponent(real, precision),
                'f' => format!("{real:.*}", precision),
                'g' => format_real_general(real, precision),
                _ => format!("{real}"),
            };
        }
        if let Expr::RealBinary { .. } = argument {
            let bits = self.eval(argument);
            if !bits.is_fully_known() {
                return "x".to_string();
            }
            let real = f64::from_bits(bits.to_u64().unwrap_or_default());
            if !real.is_finite() {
                return "x".to_string();
            }
            let precision = precision.unwrap_or(6);
            return match spec {
                'e' => format_real_exponent(real, precision),
                'f' => format!("{real:.*}", precision),
                'g' => format_real_general(real, precision),
                _ => format!("{real}"),
            };
        }
        if let Expr::RealNeg(_) = argument {
            let bits = self.eval(argument);
            if !bits.is_fully_known() {
                return "x".to_string();
            }
            let real = f64::from_bits(bits.to_u64().unwrap_or_default());
            let precision = precision.unwrap_or(6);
            return match spec {
                'e' => format_real_exponent(real, precision),
                'f' => format!("{real:.*}", precision),
                'g' => format_real_general(real, precision),
                _ => format!("{real}"),
            };
        }
        if let Expr::RealTime { .. } = argument {
            let bits = self.eval(argument);
            if !bits.is_fully_known() {
                return "x".to_string();
            }
            let real = f64::from_bits(bits.to_u64().unwrap_or_default());
            let precision = precision.unwrap_or(6);
            return match spec {
                'e' => format_real_exponent(real, precision),
                'f' => format!("{real:.*}", precision),
                'g' => format_real_general(real, precision),
                _ => format!("{real}"),
            };
        }
        if let Expr::EnumPattern {
            value,
            values,
            names,
        } = argument
        {
            let word = self.eval(value);
            if let Some(index) = values.iter().position(|candidate| *candidate == word) {
                return names[index].clone();
            }
            return format_pattern_word(word.width(), value.is_signed(), &word);
        }
        if let Expr::StructPattern {
            value,
            fields,
            abbreviated,
        } = argument
        {
            let word = self.eval(value);
            let rendered = Self::format_pattern_struct_fields(&word, fields, *abbreviated);
            return format!("'{{{rendered}}}");
        }
        if let Expr::MemoryPattern {
            memory,
            element,
            abbreviated,
        } = argument
        {
            return self.format_pattern_memory(*memory, element, *abbreviated);
        }
        if matches!(spec, 'e' | 'E') {
            let value = self.eval(argument);
            return format_pattern_integral(argument, &value);
        }
        let value = self.eval(argument);
        match spec {
            'b' => value.to_bin_str(),
            'h' => value.to_hex_str(),
            'o' => value.to_oct_str(),
            's' => value.to_ascii_string_nul_terminated(),
            'c' => match value.to_u64() {
                Some(value) => (u8::try_from(value & 0xff).unwrap_or_default() as char).to_string(),
                None => "x".to_string(),
            },
            'v' => {
                let mut labels = Vec::with_capacity(value.width());
                for bit in (0..value.width()).rev() {
                    labels.push(match value.bit(bit) {
                        eda_core::logic::LogicValue::Zero => "St0",
                        eda_core::logic::LogicValue::One => "St1",
                        eda_core::logic::LogicValue::X => "StX",
                        eda_core::logic::LogicValue::Z => "HiZ",
                    });
                }
                labels.join(" ")
            }
            'q' => format!("{:?}", value.to_ascii_string_nul_terminated()),
            'p' => format_pattern_integral(argument, &value),
            _ => match value.to_u64() {
                Some(value) => {
                    if argument.is_signed() {
                        let width = argument.width().min(64);
                        let sign = 1_u128 << (width - 1);
                        let magnitude = u128::from(value);
                        if width > 0 && magnitude & sign != 0 {
                            let signed = magnitude as i128 - (1_i128 << width);
                            signed.to_string()
                        } else {
                            magnitude.to_string()
                        }
                    } else {
                        value.to_string()
                    }
                }
                None => "x".to_string(),
            },
        }
    }

    fn format_pattern_struct_fields(
        word: &LogicWord,
        fields: &[eda_core::ir::StructPatternField],
        abbreviated: bool,
    ) -> String {
        let separator = if abbreviated { "," } else { ", " };
        fields
            .iter()
            .map(|field| {
                let slice = word.slice(field.offset + field.width - 1, field.offset);
                let rendered = match &field.value {
                    eda_core::ir::StructPatternValue::Scalar { signed } => {
                        format_pattern_word(field.width, *signed, &slice)
                    }
                    eda_core::ir::StructPatternValue::Struct { fields } => {
                        format!(
                            "'{{{}}}",
                            Self::format_pattern_struct_fields(word, fields, abbreviated)
                        )
                    }
                };
                if abbreviated {
                    rendered
                } else {
                    format!("{}:{}", field.name, rendered)
                }
            })
            .collect::<Vec<_>>()
            .join(separator)
    }

    fn format_pattern_memory(
        &self,
        memory: usize,
        element: &eda_core::ir::MemoryPatternElement,
        abbreviated: bool,
    ) -> String {
        let Some(declaration) = self.design.memories.get(memory) else {
            return "<missing-memory>".to_string();
        };
        let canonical_memory = self.canonical_memory(memory);
        let element_width = declaration.element_width;
        if declaration.associative {
            return self.format_pattern_associative_memory(canonical_memory, element, abbreviated);
        }
        let length = if declaration.dynamic {
            self.dynamic_lengths
                .get(canonical_memory)
                .copied()
                .unwrap_or_default()
        } else {
            declaration.flat_length
        };
        if length > declaration.flat_length {
            return "<invalid-memory-length>".to_string();
        }
        let Some(shape_length) = Self::memory_shape_length(&declaration.dimensions) else {
            return "<invalid-memory-shape>".to_string();
        };
        if shape_length != declaration.flat_length {
            return "<invalid-memory-shape>".to_string();
        }
        self.format_pattern_memory_slice(
            canonical_memory,
            element_width,
            element,
            &declaration.dimensions,
            (0, length),
            abbreviated,
        )
    }

    fn memory_shape_length(dimensions: &[eda_core::ir::MemoryDimension]) -> Option<u64> {
        dimensions.iter().try_fold(1_u64, |length, dimension| {
            length.checked_mul(dimension.length as u64)
        })
    }

    fn format_pattern_associative_memory(
        &self,
        memory: usize,
        element: &eda_core::ir::MemoryPatternElement,
        abbreviated: bool,
    ) -> String {
        let Some(declaration) = self.design.memories.get(memory) else {
            return "<missing-memory>".to_string();
        };
        let Some(elements) = self.associative_elements.get(memory) else {
            return "<missing-memory>".to_string();
        };
        if self
            .dynamic_lengths
            .get(memory)
            .is_some_and(|length| *length != elements.len() as u64)
        {
            return "<invalid-memory-length>".to_string();
        }
        let mut entries = elements.iter().collect::<Vec<_>>();
        entries.sort_by(|(left, _), (right, _)| {
            associative_key_compare(
                **left,
                **right,
                declaration.index_width,
                declaration.index_signed,
            )
        });
        let rendered = entries
            .into_iter()
            .map(|(key, value)| {
                let key = LogicWord::from_u64(declaration.index_width, *key);
                let key =
                    format_pattern_word(declaration.index_width, declaration.index_signed, &key);
                let value = self.format_pattern_memory_element_value(
                    declaration.element_width,
                    element,
                    value,
                    abbreviated,
                );
                format!("{key}:{value}")
            })
            .collect::<Vec<_>>();
        let separator = if abbreviated { "," } else { ", " };
        format!("'{{{}}}", rendered.join(separator))
    }

    fn format_pattern_memory_slice(
        &self,
        canonical_memory: usize,
        element_width: usize,
        element: &eda_core::ir::MemoryPatternElement,
        dimensions: &[eda_core::ir::MemoryDimension],
        range: (u64, u64),
        abbreviated: bool,
    ) -> String {
        let Some(dimension) = dimensions.first() else {
            return self.format_pattern_memory_element(
                canonical_memory,
                element_width,
                element,
                range.0,
                abbreviated,
            );
        };
        let stride = Self::memory_shape_length(&dimensions[1..]).unwrap_or(0);
        let dimension_length = u64::try_from(dimension.length).unwrap_or(u64::MAX);
        let count = dimension_length.min(range.1);
        let count = usize::try_from(count).unwrap_or(usize::MAX);
        let mut elements = Vec::with_capacity(count);
        for index in 0..count {
            let index = u64::try_from(index).ok().and_then(|index| {
                index
                    .checked_mul(stride)
                    .and_then(|offset| range.0.checked_add(offset))
            });
            let Some(index) = index else {
                return "<invalid-memory-shape>".to_string();
            };
            elements.push(self.format_pattern_memory_slice(
                canonical_memory,
                element_width,
                element,
                &dimensions[1..],
                (index, range.1),
                abbreviated,
            ));
        }
        let separator = if abbreviated { "," } else { ", " };
        format!("'{{{}}}", elements.join(separator))
    }

    fn format_pattern_memory_element(
        &self,
        canonical_memory: usize,
        element_width: usize,
        element: &eda_core::ir::MemoryPatternElement,
        index: u64,
        abbreviated: bool,
    ) -> String {
        let value = self
            .memory_elements
            .get(canonical_memory)
            .and_then(|storage| storage.get(index))
            .cloned()
            .unwrap_or_else(|| LogicWord::xes(element_width));
        self.format_pattern_memory_element_value(element_width, element, &value, abbreviated)
    }

    fn format_pattern_memory_element_value(
        &self,
        element_width: usize,
        element: &eda_core::ir::MemoryPatternElement,
        value: &LogicWord,
        abbreviated: bool,
    ) -> String {
        match element {
            eda_core::ir::MemoryPatternElement::Scalar { signed } => {
                format_pattern_word(element_width, *signed, value)
            }
            eda_core::ir::MemoryPatternElement::Struct { fields } => format!(
                "'{{{}}}",
                Self::format_pattern_struct_fields(value, fields, abbreviated)
            ),
            eda_core::ir::MemoryPatternElement::PackedUnion { signed } => {
                format_pattern_word(element_width, *signed, value)
            }
            eda_core::ir::MemoryPatternElement::UnpackedUnion { fields } => format!(
                "'{{{}}}",
                Self::format_pattern_struct_fields(value, fields, abbreviated)
            ),
        }
    }

    fn format_file_display(&self, parts: &[eda_core::ir::DispPart], args: &[Expr]) -> Vec<u8> {
        use eda_core::ir::DispPart;
        let mut out = Vec::new();
        for part in parts {
            match part {
                DispPart::Str(text) => out.extend_from_slice(text.as_bytes()),
                DispPart::Time(index) => {
                    out.extend_from_slice(self.format_time_value(&args[*index]).as_bytes())
                }
                DispPart::Arg(index, spec) => match spec {
                    'u' => out.extend_from_slice(&raw_two_state_bytes(&self.eval(&args[*index]))),
                    'z' => out.extend_from_slice(&raw_four_state_bytes(&self.eval(&args[*index]))),
                    _ => out.extend_from_slice(
                        self.format_display_argument(&args[*index], *spec, None)
                            .as_bytes(),
                    ),
                },
                DispPart::FormattedArg {
                    index,
                    spec,
                    width,
                    precision,
                    left_justify,
                } => {
                    let rendered = if *spec == 't' {
                        self.format_time_value(&args[*index])
                    } else {
                        self.format_display_argument(&args[*index], *spec, *precision)
                    };
                    let padding = if matches!(spec, 'b' | 'h' | 'o') {
                        '0'
                    } else {
                        ' '
                    };
                    out.extend_from_slice(
                        pad_formatted_text(&rendered, *width, *left_justify, padding).as_bytes(),
                    );
                }
            }
        }
        out
    }

    fn format_time_value(&self, argument: &Expr) -> String {
        let ticks = match self.eval(argument).to_u64() {
            Some(value) => value,
            None => return "x".to_string(),
        };
        let baseline_units =
            simulation_time_unit(argument).unwrap_or(self.design.time_scale.precision_exponent);
        let exponent = i16::from(baseline_units) - i16::from(self.timeformat_units);
        let (numerator, denominator) = if exponent >= 0 {
            let scale = 10_u128
                .checked_pow(u32::try_from(exponent).unwrap_or(u32::MAX))
                .unwrap_or(u128::MAX);
            (u128::from(ticks).saturating_mul(scale), 1_u128)
        } else {
            let scale = 10_u128
                .checked_pow(u32::try_from(-exponent).unwrap_or(u32::MAX))
                .unwrap_or(u128::MAX);
            (u128::from(ticks), scale)
        };
        let precision = u32::from(self.timeformat_precision);
        let decimal_scale = 10_u128.checked_pow(precision).unwrap_or(u128::MAX);
        let scaled_numerator = numerator.saturating_mul(decimal_scale);
        let scaled_denominator = denominator;
        let mut quotient = scaled_numerator / scaled_denominator;
        let remainder = scaled_numerator % scaled_denominator;
        if remainder * 2 >= scaled_denominator {
            quotient = quotient.saturating_add(1);
        }
        let integer = quotient / decimal_scale.max(1);
        let fraction = if precision == 0 {
            0
        } else {
            quotient % decimal_scale
        };
        let mut text = if precision == 0 {
            integer.to_string()
        } else {
            format!("{integer}.{fraction:0width$}", width = precision as usize)
        };
        text.push_str(&self.timeformat_suffix);
        if text.len() < self.timeformat_min_width {
            let padding = self.timeformat_min_width - text.len();
            text = format!("{}{text}", " ".repeat(padding));
        }
        text
    }

    fn eval_random(
        &self,
        function: eda_core::ir::RandomFunction,
        args: &[Expr],
        width: usize,
    ) -> LogicWord {
        match function {
            eda_core::ir::RandomFunction::Random | eda_core::ir::RandomFunction::Urandom => {
                LogicWord::from_u64(width, u64::from(self.next_u32()))
            }
            eda_core::ir::RandomFunction::UrandomRange => {
                let max = args
                    .first()
                    .map(|argument| self.eval(argument))
                    .and_then(|value| value.to_u64())
                    .and_then(|value| u32::try_from(value).ok());
                let min = args
                    .get(1)
                    .map(|argument| self.eval(argument))
                    .and_then(|value| value.to_u64())
                    .and_then(|value| u32::try_from(value).ok())
                    .unwrap_or(0);
                let Some(max) = max else {
                    return LogicWord::xes(width);
                };
                let (low, high) = if max < min { (max, min) } else { (min, max) };
                let modulo = u64::from(high) - u64::from(low) + 1;
                let rejection_limit = u64::MAX - (u64::MAX % modulo);
                let mut sample = self.next_u64();
                while sample >= rejection_limit {
                    sample = self.next_u64();
                }
                LogicWord::from_u64(width, u64::from(low) + sample % modulo)
            }
        }
    }

    fn next_u32(&self) -> u32 {
        self.next_u64() as u32
    }

    fn next_u64(&self) -> u64 {
        let state = self
            .random_state
            .replace(self.random_state.get().wrapping_add(0x9E37_79B9_7F4A_7C15));
        let mut z = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn file_descriptor(&self, expression: &Expr) -> Result<u32, String> {
        let value = self
            .eval(expression)
            .to_u64()
            .and_then(|value| u32::try_from(value).ok());
        value.filter(|descriptor| *descriptor != 0).ok_or_else(|| {
            format!(
                "file operation requires a known nonzero descriptor at t={}",
                self.now
            )
        })
    }

    fn open_file_descriptor(
        &self,
        path: &str,
        append: bool,
        read: bool,
        truncate: bool,
    ) -> LogicWord {
        let mixed_read_write = read && (append || truncate);
        let mut options = std::fs::OpenOptions::new();
        if mixed_read_write {
            options.read(true).write(true);
            if truncate {
                options.create(true);
                if append {
                    options.append(true);
                } else {
                    options.truncate(true);
                }
            }
        } else if read {
            options.read(true);
        } else {
            options.write(true).create(true);
            if append {
                options.append(true);
            } else {
                options.truncate(true);
            }
        }
        let Ok(file) = options.open(path) else {
            return LogicWord::zero(32);
        };
        let (mut reader, mut writer) = if mixed_read_write {
            let writer = match file.try_clone() {
                Ok(writer) => writer,
                Err(_) => return LogicWord::zero(32),
            };
            (Some(file), Some(writer))
        } else if read {
            (Some(file), None)
        } else {
            (None, Some(file))
        };
        loop {
            let candidate = self.next_file_descriptor.get();
            let Some(next) = candidate.checked_add(1) else {
                return LogicWord::zero(32);
            };
            self.next_file_descriptor.set(next);
            if candidate != 0
                && self
                    .file_paths
                    .borrow_mut()
                    .insert(candidate, PathBuf::from(path))
                    .is_none()
            {
                if mixed_read_write {
                    let Some(read_handle) = reader.take() else {
                        unreachable!("mixed file reader is inserted once");
                    };
                    let Some(write_handle) = writer.take() else {
                        unreachable!("mixed file writer is inserted once");
                    };
                    self.file_readers
                        .borrow_mut()
                        .insert(candidate, read_handle);
                    self.file_writers
                        .borrow_mut()
                        .insert(candidate, write_handle);
                    let initial_position = if append && truncate {
                        std::fs::metadata(path)
                            .map(|metadata| metadata.len())
                            .unwrap_or_default()
                    } else {
                        0
                    };
                    self.file_write_positions
                        .borrow_mut()
                        .insert(candidate, initial_position);
                    self.file_eof.borrow_mut().remove(&candidate);
                    self.file_errors.borrow_mut().remove(&candidate);
                } else if read {
                    let Some(file) = reader.take() else {
                        unreachable!("file reader is inserted once");
                    };
                    self.file_readers.borrow_mut().insert(candidate, file);
                    self.file_eof.borrow_mut().remove(&candidate);
                    self.file_errors.borrow_mut().remove(&candidate);
                } else {
                    let Some(file) = writer.take() else {
                        unreachable!("file writer is inserted once");
                    };
                    let position = if append {
                        std::fs::metadata(path)
                            .map(|metadata| metadata.len())
                            .unwrap_or_default()
                    } else {
                        0
                    };
                    self.file_write_positions
                        .borrow_mut()
                        .insert(candidate, position);
                    self.file_writers.borrow_mut().insert(candidate, file);
                    self.file_errors.borrow_mut().remove(&candidate);
                }
                return LogicWord::from_u64(32, u64::from(candidate));
            }
        }
    }

    fn assign_lvalue_word(&mut self, lv: &LValue, word: LogicWord) -> Result<(), String> {
        let value = word.sized(lv_bits(lv).len());
        let mut changes = Vec::new();
        for (index, net) in lv_bits(lv).into_iter().enumerate() {
            let newv = value.bit(index);
            if self.forced_values.contains_key(&net) {
                continue;
            }
            let old = self.vals[net];
            if old != newv {
                self.vals[net] = newv;
                self.mark_vcd_change(net);
                changes.push((net, old, newv));
            }
        }
        for (net, old, newv) in changes {
            self.event_count += 1;
            if self.event_count > self.event_limit {
                return Err("event budget exceeded".into());
            }
            self.notify_change(net, old, newv)?;
        }
        Ok(())
    }

    fn read_file_input_byte(&mut self, descriptor: u32) -> Result<Option<u8>, String> {
        if let Some(byte) = self.file_pushbacks.borrow_mut().remove(&descriptor) {
            self.file_eof.borrow_mut().remove(&descriptor);
            return Ok(Some(byte));
        }
        let mut readers = self.file_readers.borrow_mut();
        let Some(reader) = readers.get_mut(&descriptor) else {
            self.file_errors.borrow_mut().insert(
                descriptor,
                "file descriptor is not open for reading".to_string(),
            );
            return Ok(None);
        };
        let mut byte = [0_u8; 1];
        match reader.read(&mut byte) {
            Ok(1) => {
                self.file_eof.borrow_mut().remove(&descriptor);
                Ok(Some(byte[0]))
            }
            Ok(_) => {
                self.file_eof.borrow_mut().insert(descriptor);
                Ok(None)
            }
            Err(error) => {
                self.file_errors
                    .borrow_mut()
                    .insert(descriptor, format!("formatted read failed: {error}"));
                Err(format!(
                    "cannot read file descriptor {descriptor} at t={}: {error}",
                    self.now
                ))
            }
        }
    }

    fn push_file_input_byte(&mut self, descriptor: u32, byte: u8) -> Result<(), String> {
        let mut pushbacks = self.file_pushbacks.borrow_mut();
        if pushbacks.contains_key(&descriptor) {
            return Err(format!(
                "cannot preserve more than one lookahead byte for file descriptor {descriptor}"
            ));
        }
        pushbacks.insert(descriptor, byte);
        self.file_eof.borrow_mut().remove(&descriptor);
        Ok(())
    }

    fn read_file_bytes(
        &self,
        descriptor: u32,
        byte_count: usize,
    ) -> Result<(Vec<u8>, bool), String> {
        let mut bytes = Vec::with_capacity(byte_count);
        let mut reached_eof = false;
        let mut readers = self.file_readers.borrow_mut();
        let Some(reader) = readers.get_mut(&descriptor) else {
            return Err("file descriptor is not open for reading".to_string());
        };
        while bytes.len() < byte_count {
            if let Some(byte) = self.file_pushbacks.borrow_mut().remove(&descriptor) {
                bytes.push(byte);
                continue;
            }
            let mut byte = [0_u8; 1];
            match reader.read(&mut byte) {
                Ok(1) => bytes.push(byte[0]),
                Ok(_) => {
                    reached_eof = true;
                    break;
                }
                Err(error) => {
                    self.file_errors
                        .borrow_mut()
                        .insert(descriptor, format!("binary read failed: {error}"));
                    return Err(error.to_string());
                }
            }
        }
        if reached_eof {
            self.file_eof.borrow_mut().insert(descriptor);
        } else {
            self.file_eof.borrow_mut().remove(&descriptor);
        }
        Ok((bytes, reached_eof))
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
        let whole_reversed_element = reversed && offset == 0 && width == element_width;
        let mut targets = Vec::with_capacity(width);
        for (bit, new_value) in value.bits.iter().enumerate().take(width) {
            let physical_offset = if reversed && !whole_reversed_element {
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
        let physical = declaration.physical_index(&logical)?;
        if declaration.dynamic && physical >= self.dynamic_lengths[self.canonical_memory(memory)] {
            return None;
        }
        Some(physical)
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
            Expr::RealConst(real) => LogicWord::from_u64(64, real.to_bits()),
            Expr::RealRef { net, decl_width } => {
                let mut bits = Vec::with_capacity(*decl_width);
                for bit in 0..*decl_width {
                    bits.push(self.vals[net + bit]);
                }
                LogicWord { bits }
            }
            Expr::ShortRealConst(real) => LogicWord::from_u64(32, u64::from(real.to_bits())),
            Expr::ShortRealRef { net } => {
                let mut bits = Vec::with_capacity(32);
                for bit in 0..32 {
                    bits.push(self.vals[net + bit]);
                }
                LogicWord { bits }
            }
            Expr::ProcessSelf => LogicWord::from_u64(
                64,
                u64::try_from(self.current_runtime_process).unwrap_or(u64::MAX),
            ),
            Expr::ProcessStatus(handle) => {
                let process = match self
                    .eval(handle)
                    .to_u64()
                    .and_then(|raw| usize::try_from(raw).ok())
                    .filter(|process| *process < self.process_states.len())
                {
                    Some(process) => process,
                    None => return LogicWord::xes(32),
                };
                let state = &self.process_states[process];
                let status = if state.cancelled {
                    PROCESS_STATE_KILLED
                } else if !state.active {
                    PROCESS_STATE_FINISHED
                } else if state.suspended {
                    PROCESS_STATE_SUSPENDED
                } else if process == self.current_runtime_process {
                    PROCESS_STATE_RUNNING
                } else if self.process_is_waiting(process) {
                    PROCESS_STATE_WAITING
                } else {
                    PROCESS_STATE_RUNNING
                };
                LogicWord::from_u64(32, status)
            }
            Expr::Past {
                value,
                ticks,
                gating,
            } => {
                let current = self.eval(value);
                let gating = gating
                    .as_deref()
                    .map(|gating| self.eval(gating).reduce_or().as_bool());
                let key = e as *const Expr as usize;
                let mut samples = self.past_samples.borrow_mut();
                let sample = samples.entry(key).or_insert(PastSample {
                    time: u64::MAX,
                    values: Vec::new(),
                    last_result: None,
                });
                if sample.time != self.now {
                    sample.time = self.now;
                    match gating {
                        Some(Some(true)) => {
                            sample.values.push(current.clone());
                            let retained = usize::try_from(*ticks)
                                .map(|ticks| ticks + 1)
                                .unwrap_or(sample.values.len());
                            while sample.values.len() > retained {
                                sample.values.remove(0);
                            }
                            let offset = usize::try_from(*ticks)
                                .map(|ticks| ticks + 1)
                                .unwrap_or(usize::MAX);
                            sample.last_result = sample
                                .values
                                .len()
                                .checked_sub(offset)
                                .and_then(|index| sample.values.get(index).cloned());
                        }
                        Some(Some(false)) => {}
                        Some(None) => {
                            sample.last_result = Some(LogicWord::xes(current.width()));
                        }
                        None => {
                            sample.values.push(current.clone());
                            let offset = usize::try_from(*ticks)
                                .map(|ticks| ticks + 1)
                                .unwrap_or(usize::MAX);
                            sample.last_result = Some(
                                sample
                                    .values
                                    .len()
                                    .checked_sub(offset)
                                    .and_then(|index| sample.values.get(index).cloned())
                                    .unwrap_or_else(|| LogicWord::xes(current.width())),
                            );
                        }
                    }
                }
                sample
                    .last_result
                    .clone()
                    .unwrap_or_else(|| LogicWord::xes(current.width()))
            }
            Expr::Sampled(value) => self.eval(value),
            Expr::SampledValue { op, value } => {
                let current = self.eval(value);
                let key = e as *const Expr as usize;
                let mut samples = self.past_samples.borrow_mut();
                let sample = samples.entry(key).or_insert(PastSample {
                    time: u64::MAX,
                    values: Vec::new(),
                    last_result: None,
                });
                if sample.time != self.now {
                    sample.time = self.now;
                    sample.values.push(current.clone());
                    while sample.values.len() > 2 {
                        sample.values.remove(0);
                    }
                }
                if sample.values.len() < 2 {
                    return LogicWord::xes(1);
                }
                let previous = &sample.values[sample.values.len() - 2];
                let current = &sample.values[sample.values.len() - 1];
                let result = match op {
                    eda_core::ir::SampledValueOp::Rose => {
                        let previous = previous.reduce_or().as_bool();
                        let current = current.reduce_or().as_bool();
                        match (previous, current) {
                            (Some(false), Some(true)) => Some(true),
                            (Some(_), Some(_)) => Some(false),
                            _ => None,
                        }
                    }
                    eda_core::ir::SampledValueOp::Fell => {
                        let previous = previous.reduce_or().as_bool();
                        let current = current.reduce_or().as_bool();
                        match (previous, current) {
                            (Some(true), Some(false)) => Some(true),
                            (Some(_), Some(_)) => Some(false),
                            _ => None,
                        }
                    }
                    eda_core::ir::SampledValueOp::Stable => {
                        let known = previous.bits.iter().all(|bit| bit.is_01())
                            && current.bits.iter().all(|bit| bit.is_01());
                        known.then(|| previous == current)
                    }
                    eda_core::ir::SampledValueOp::Changed => {
                        let known = previous.bits.iter().all(|bit| bit.is_01())
                            && current.bits.iter().all(|bit| bit.is_01());
                        known.then(|| previous != current)
                    }
                };
                match result {
                    Some(true) => LogicWord::from_u64(1, 1),
                    Some(false) => LogicWord::from_u64(1, 0),
                    None => LogicWord::xes(1),
                }
            }
            Expr::ShortRealBinary { .. } => match self.eval_shortreal(e) {
                Some(real) => LogicWord::from_u64(32, u64::from(real.to_bits())),
                None => LogicWord::xes(32),
            },
            Expr::ShortRealNeg(operand) => match self.eval_shortreal(operand) {
                Some(real) => LogicWord::from_u64(32, u64::from((-real).to_bits())),
                None => LogicWord::xes(32),
            },
            Expr::ShortRealToReal(operand) => match self.eval_shortreal(operand) {
                Some(real) => LogicWord::from_u64(64, f64::from(real).to_bits()),
                None => LogicWord::xes(64),
            },
            Expr::RealToShortReal(operand) => match self.eval_real(operand) {
                Some(real) => LogicWord::from_u64(32, u64::from((real as f32).to_bits())),
                None => LogicWord::xes(32),
            },
            Expr::BitsToReal(_) => match self.eval_real(e) {
                Some(real) => LogicWord::from_u64(64, real.to_bits()),
                None => LogicWord::xes(64),
            },
            Expr::BitsToShortReal(_) => match self.eval_shortreal(e) {
                Some(real) => LogicWord::from_u64(32, u64::from(real.to_bits())),
                None => LogicWord::xes(32),
            },
            Expr::RealBinary { .. } => match self.eval_real(e) {
                Some(real) => LogicWord::from_u64(64, real.to_bits()),
                None => LogicWord::xes(64),
            },
            Expr::RealNeg(operand) => match self.eval_real(operand) {
                Some(real) => LogicWord::from_u64(64, (-real).to_bits()),
                _ => LogicWord::xes(64),
            },
            Expr::RealToInt {
                value,
                width,
                signed,
            } => match self
                .eval_real(value)
                .and_then(|real| real_to_int(real, *width, *signed))
            {
                Some(bits) => LogicWord::from_u64(*width, bits),
                None => LogicWord::xes(*width),
            },
            Expr::RealTruncToInt {
                value,
                width,
                signed,
            } => match self
                .eval_real(value)
                .and_then(|real| real_trunc_to_int(real, *width, *signed))
            {
                Some(bits) => LogicWord::from_u64(*width, bits),
                None => LogicWord::xes(*width),
            },
            Expr::IntToReal {
                value,
                width,
                signed,
            } => {
                let word = self.eval(value);
                let Some(raw) = word.to_u64() else {
                    return LogicWord::xes(64);
                };
                let real = if *signed {
                    sext_i64(&word, *width) as f64
                } else {
                    let mask = if *width < u64::BITS as usize {
                        (1_u64 << *width) - 1
                    } else {
                        u64::MAX
                    };
                    (raw & mask) as f64
                };
                LogicWord::from_u64(64, real.to_bits())
            }
            Expr::RealTime { unit_exponent } => {
                let precision = i16::from(self.design.time_scale.precision_exponent);
                let scale_exponent = i16::from(*unit_exponent) - precision;
                let real = if scale_exponent <= 0 {
                    (self.now as f64) * 10_f64.powi(-i32::from(scale_exponent))
                } else {
                    (self.now as f64) / 10_f64.powi(i32::from(scale_exponent))
                };
                if real.is_finite() {
                    LogicWord::from_u64(64, real.to_bits())
                } else {
                    LogicWord::xes(64)
                }
            }
            Expr::RealCompare { op, left, right } => {
                let Some(left) = self.eval_real(left) else {
                    return LogicWord::xes(1);
                };
                let Some(right) = self.eval_real(right) else {
                    return LogicWord::xes(1);
                };
                let result = match op {
                    eda_core::ir::BinOp::Eq => left == right,
                    eda_core::ir::BinOp::Ne => left != right,
                    eda_core::ir::BinOp::Lt => left < right,
                    eda_core::ir::BinOp::Le => left <= right,
                    eda_core::ir::BinOp::Gt => left > right,
                    eda_core::ir::BinOp::Ge => left >= right,
                    _ => return LogicWord::xes(1),
                };
                LogicWord {
                    bits: vec![LogicValue::bit(result)],
                }
            }
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
                                    let ordering = if signed {
                                        compare_known_words(
                                            &sext_word(&av, w),
                                            &sext_word(&bv, w),
                                            true,
                                        )
                                    } else {
                                        compare_known_words(&av.sized(w), &bv.sized(w), false)
                                    };
                                    match op {
                                        BinOp::Lt => ordering.is_lt(),
                                        BinOp::Le => !ordering.is_gt(),
                                        BinOp::Gt => ordering.is_gt(),
                                        _ => !ordering.is_lt(),
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
            Expr::StringFormat { parts, args, .. } => {
                let mut rendered = self.format_file_display(parts, args);
                // A SystemVerilog string result cannot contain NUL; slang also
                // removes the zero padding emitted by raw `%u` and `%z`.
                rendered.retain(|byte| *byte != 0);
                if rendered.len() > 63 {
                    LogicWord::xes(512)
                } else {
                    LogicWord::from_bytes_le(512, &rendered)
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
            Expr::EventTriggered { net } => LogicWord {
                bits: vec![LogicValue::bit(
                    self.net_change_times
                        .get(net)
                        .is_some_and(|time| *time == self.now),
                )],
            },
            Expr::SimulationTime {
                width,
                unit_exponent,
                ..
            } => {
                let precision = i16::from(self.design.time_scale.precision_exponent);
                let scale_exponent = i16::from(*unit_exponent) - precision;
                let value = if scale_exponent <= 0 {
                    self.now
                } else {
                    let scale = 10_u64
                        .checked_pow(u32::try_from(scale_exponent).unwrap_or(u32::MAX))
                        .unwrap_or(u64::MAX);
                    self.now / scale
                };
                LogicWord::from_u64(*width, value)
            }
            Expr::Random {
                function,
                args,
                width,
                ..
            } => self.eval_random(*function, args, *width),
            Expr::CountBits { value, controls } => {
                let value = self.eval(value);
                let count = value
                    .bits
                    .iter()
                    .filter(|bit| match bit {
                        LogicValue::Zero => controls & 1 != 0,
                        LogicValue::One => controls & 2 != 0,
                        LogicValue::X => controls & 4 != 0,
                        LogicValue::Z => controls & 8 != 0,
                    })
                    .count();
                LogicWord::from_u64(32, count as u64)
            }
            Expr::IsUnknown(value) => {
                let value = self.eval(value);
                LogicWord::from_u64(1, u64::from(value.bits.iter().any(|bit| bit.is_unknown())))
            }
            Expr::OneHot(value) => {
                let evaluated = self.eval(value);
                let ones = evaluated
                    .bits
                    .iter()
                    .filter(|bit| **bit == LogicValue::One)
                    .count();
                let unknown = evaluated
                    .bits
                    .iter()
                    .any(|bit| matches!(bit, LogicValue::X | LogicValue::Z));
                if unknown {
                    LogicWord::xes(1)
                } else {
                    LogicWord::from_u64(1, u64::from(ones == 1))
                }
            }
            Expr::OneHot0(value) => {
                let evaluated = self.eval(value);
                let ones = evaluated
                    .bits
                    .iter()
                    .filter(|bit| **bit == LogicValue::One)
                    .count();
                let unknown = evaluated
                    .bits
                    .iter()
                    .any(|bit| matches!(bit, LogicValue::X | LogicValue::Z));
                if unknown {
                    LogicWord::xes(1)
                } else {
                    LogicWord::from_u64(1, u64::from(ones <= 1))
                }
            }
            Expr::FileOpen {
                path,
                append,
                read,
                truncate,
            } => self.open_file_descriptor(path, *append, *read, *truncate),
            Expr::FileOpenDynamic { path, mode } => {
                let path = self.eval(path).to_ascii_string_nul_terminated();
                let mode = self.eval(mode).to_ascii_string_nul_terminated();
                if path.is_empty() {
                    return LogicWord::zero(32);
                }
                let (append, read, truncate) = match mode.as_str() {
                    "r" | "rb" => (false, true, false),
                    "r+" | "r+b" | "rb+" => (true, true, false),
                    "w" | "wb" => (false, false, true),
                    "w+" | "w+b" | "wb+" => (false, true, true),
                    "a" | "ab" => (true, false, false),
                    "a+" | "a+b" | "ab+" => (true, true, true),
                    _ => return LogicWord::zero(32),
                };
                self.open_file_descriptor(&path, append, read, truncate)
            }
            Expr::FileGetc { fd } => {
                let Ok(descriptor) = self.file_descriptor(fd) else {
                    return LogicWord::from_u64(32, u64::from(u32::MAX));
                };
                let mut readers = self.file_readers.borrow_mut();
                let Some(reader) = readers.get_mut(&descriptor) else {
                    self.file_errors.borrow_mut().insert(
                        descriptor,
                        "file descriptor is not open for reading".to_string(),
                    );
                    return LogicWord::from_u64(32, u64::from(u32::MAX));
                };
                if let Some(byte) = self.file_pushbacks.borrow_mut().remove(&descriptor) {
                    self.file_eof.borrow_mut().remove(&descriptor);
                    return LogicWord::from_u64(32, u64::from(byte));
                }
                let mut byte = [0_u8; 1];
                match reader.read(&mut byte) {
                    Ok(1) => LogicWord::from_u64(32, u64::from(byte[0])),
                    Ok(_) => {
                        self.file_eof.borrow_mut().insert(descriptor);
                        LogicWord::from_u64(32, u64::from(u32::MAX))
                    }
                    Err(_) => {
                        self.file_eof.borrow_mut().insert(descriptor);
                        LogicWord::from_u64(32, u64::from(u32::MAX))
                    }
                }
            }
            Expr::FileUngetc { value, fd } => {
                let failure = LogicWord::from_u64(32, u64::from(u32::MAX));
                let Ok(descriptor) = self.file_descriptor(fd) else {
                    return failure;
                };
                let Some(raw_value) = self.eval(value).to_u64() else {
                    return failure;
                };
                let Ok(byte) = u8::try_from(raw_value) else {
                    self.file_errors.borrow_mut().insert(
                        descriptor,
                        "$ungetc byte value is outside 0..255".to_string(),
                    );
                    return failure;
                };
                if !self.file_readers.borrow().contains_key(&descriptor) {
                    self.file_errors.borrow_mut().insert(
                        descriptor,
                        "file descriptor is not open for reading".to_string(),
                    );
                    return failure;
                }
                let mut pushbacks = self.file_pushbacks.borrow_mut();
                if pushbacks.contains_key(&descriptor) {
                    self.file_errors.borrow_mut().insert(
                        descriptor,
                        "$ungetc pushback slot is already in use".to_string(),
                    );
                    return failure;
                }
                pushbacks.insert(descriptor, byte);
                self.file_eof.borrow_mut().remove(&descriptor);
                LogicWord::from_u64(32, u64::from(byte))
            }
            Expr::FileEof { fd } => {
                let Ok(descriptor) = self.file_descriptor(fd) else {
                    return LogicWord::zero(32);
                };
                if self.file_readers.borrow().contains_key(&descriptor)
                    && self.file_eof.borrow().contains(&descriptor)
                {
                    LogicWord::from_u64(32, u64::from(u32::MAX))
                } else {
                    LogicWord::zero(32)
                }
            }
            Expr::FileTell { fd } => {
                let Ok(descriptor) = self.file_descriptor(fd) else {
                    return LogicWord::from_u64(32, u64::from(u32::MAX));
                };
                if let Some(reader) = self.file_readers.borrow_mut().get_mut(&descriptor) {
                    return match reader.stream_position() {
                        Ok(position) if position <= u64::from(u32::MAX) => {
                            let logical_position =
                                if self.file_pushbacks.borrow().contains_key(&descriptor) {
                                    position.saturating_sub(1)
                                } else {
                                    position
                                };
                            LogicWord::from_u64(32, logical_position)
                        }
                        _ => LogicWord::from_u64(32, u64::from(u32::MAX)),
                    };
                }
                match self
                    .file_write_positions
                    .borrow()
                    .get(&descriptor)
                    .copied()
                    .filter(|position| *position <= u64::from(u32::MAX))
                {
                    Some(position) => LogicWord::from_u64(32, position),
                    None => LogicWord::from_u64(32, u64::from(u32::MAX)),
                }
            }
            Expr::FileSeek { fd, offset, whence } => {
                let failure = LogicWord::from_u64(32, u64::from(u32::MAX));
                let Ok(descriptor) = self.file_descriptor(fd) else {
                    return failure;
                };
                let Some(raw_offset) = self.eval(offset).to_u64() else {
                    return failure;
                };
                if raw_offset > u64::from(u32::MAX) {
                    return failure;
                }
                let offset = raw_offset as u32 as i32;
                let Some(whence) = self.eval(whence).to_u64() else {
                    return failure;
                };
                let seek_from = match whence {
                    0 => {
                        let Ok(start) = u64::try_from(offset) else {
                            return failure;
                        };
                        SeekFrom::Start(start)
                    }
                    1 => SeekFrom::Current(i64::from(offset)),
                    2 => SeekFrom::End(i64::from(offset)),
                    _ => return failure,
                };
                if let Some(reader) = self.file_readers.borrow_mut().get_mut(&descriptor) {
                    return match reader.seek(seek_from) {
                        Ok(_) => {
                            self.file_pushbacks.borrow_mut().remove(&descriptor);
                            self.file_eof.borrow_mut().remove(&descriptor);
                            LogicWord::zero(32)
                        }
                        Err(_) => failure,
                    };
                }
                let mut writers = self.file_writers.borrow_mut();
                let Some(writer) = writers.get_mut(&descriptor) else {
                    return failure;
                };
                match writer.seek(seek_from) {
                    Ok(position) => {
                        self.file_pushbacks.borrow_mut().remove(&descriptor);
                        self.file_write_positions
                            .borrow_mut()
                            .insert(descriptor, position);
                        LogicWord::zero(32)
                    }
                    Err(_) => failure,
                }
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
            Expr::MemoryPattern { .. } => LogicWord::xes(32),
            Expr::EnumPattern { .. } => LogicWord::xes(32),
            Expr::StructPattern { .. } => LogicWord::xes(32),
            Expr::MemoryRead {
                memory,
                indexes,
                width,
                ..
            } => match self.memory_index(*memory, indexes) {
                Some(index) => self
                    .memory_elements
                    .get(self.canonical_memory(*memory))
                    .and_then(|elements| elements.get(index))
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
                        .and_then(|elements| elements.get(index))
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
            Expr::MemorySize { memory, width, .. } => {
                let memory = self.canonical_memory(*memory);
                let size = self.dynamic_lengths.get(memory).copied().unwrap_or(0);
                LogicWord::from_u64(*width, size)
            }
            Expr::AssocRead {
                memory,
                index,
                width,
            } => {
                let key = self.associative_key(*memory, index).ok().flatten();
                match key {
                    Some(key) => self
                        .associative_elements
                        .get(self.canonical_memory(*memory))
                        .and_then(|elements| elements.get(&key))
                        .cloned()
                        .unwrap_or_else(|| LogicWord::xes(*width)),
                    None => LogicWord::xes(*width),
                }
            }
            Expr::AssocExists { memory, index } => {
                let key = self.associative_key(*memory, index).ok().flatten();
                let exists = key.is_some_and(|key| {
                    self.associative_elements
                        .get(self.canonical_memory(*memory))
                        .is_some_and(|elements| elements.contains_key(&key))
                });
                LogicWord::from_u64(1, u64::from(exists))
            }
        }
    }

    fn eval_shortreal(&self, e: &Expr) -> Option<f32> {
        match e {
            Expr::ShortRealConst(real) => Some(*real),
            Expr::ShortRealRef { .. } => {
                let bits = self.eval(e);
                let raw = u32::try_from(bits.to_u64()?).ok()?;
                Some(f32::from_bits(raw))
            }
            Expr::MemoryRead { width: 32, .. } => {
                let bits = self.eval(e);
                let raw = u32::try_from(bits.to_u64()?).ok()?;
                Some(f32::from_bits(raw))
            }
            Expr::Index { msb, lsb, .. } if msb - lsb + 1 == 32 => {
                let bits = self.eval(e);
                let raw = u32::try_from(bits.to_u64()?).ok()?;
                Some(f32::from_bits(raw))
            }
            Expr::AssocRead { width: 32, .. } => {
                let bits = self.eval(e);
                let raw = u32::try_from(bits.to_u64()?).ok()?;
                Some(f32::from_bits(raw))
            }
            Expr::Signed { e: operand, .. } => self.eval_shortreal(operand),
            Expr::ShortRealBinary { op, left, right } => {
                let left = self.eval_shortreal(left)?;
                let right = self.eval_shortreal(right)?;
                let result = match op {
                    eda_core::ir::BinOp::Add => left + right,
                    eda_core::ir::BinOp::Sub => left - right,
                    eda_core::ir::BinOp::Mul => left * right,
                    eda_core::ir::BinOp::Div => left / right,
                    _ => return None,
                };
                Some(result)
            }
            Expr::ShortRealNeg(operand) => Some(-self.eval_shortreal(operand)?),
            Expr::ShortRealToReal(operand) => Some(self.eval_shortreal(operand)?),
            Expr::RealToShortReal(operand) => Some(self.eval_real(operand)? as f32),
            Expr::BitsToShortReal(operand) => {
                let bits = self.eval(operand);
                let raw = u32::try_from(bits.to_u64()?).ok()?;
                Some(f32::from_bits(raw))
            }
            _ => None,
        }
    }

    fn eval_real(&self, e: &Expr) -> Option<f64> {
        match e {
            Expr::RealConst(real) => Some(*real),
            Expr::RealRef { .. } => {
                let bits = self.eval(e);
                bits.to_u64().map(f64::from_bits)
            }
            Expr::MemoryRead { width: 64, .. } => {
                let bits = self.eval(e);
                bits.to_u64().map(f64::from_bits)
            }
            Expr::Index { msb, lsb, .. } if msb - lsb + 1 == 64 => {
                let bits = self.eval(e);
                bits.to_u64().map(f64::from_bits)
            }
            Expr::AssocRead { width: 64, .. } => {
                let bits = self.eval(e);
                bits.to_u64().map(f64::from_bits)
            }
            Expr::Signed { e: operand, .. } => self.eval_real(operand),
            Expr::ShortRealConst(_)
            | Expr::ShortRealRef { .. }
            | Expr::ShortRealBinary { .. }
            | Expr::ShortRealNeg(_)
            | Expr::ShortRealToReal(_)
            | Expr::RealToShortReal(_)
            | Expr::BitsToShortReal(_) => self.eval_shortreal(e).map(f64::from),
            Expr::BitsToReal(operand) => {
                let bits = self.eval(operand);
                let raw = bits.to_u64()?;
                Some(f64::from_bits(raw))
            }
            Expr::RealBinary { op, left, right } => {
                let left = self.eval_real(left)?;
                let right = self.eval_real(right)?;
                let result = match op {
                    eda_core::ir::BinOp::Add => left + right,
                    eda_core::ir::BinOp::Sub => left - right,
                    eda_core::ir::BinOp::Mul => left * right,
                    eda_core::ir::BinOp::Div => left / right,
                    _ => return None,
                };
                Some(result)
            }
            Expr::RealCompare { op, left, right } => {
                let left = self.eval_real(left)?;
                let right = self.eval_real(right)?;
                let result = match op {
                    eda_core::ir::BinOp::Eq => left == right,
                    eda_core::ir::BinOp::Ne => left != right,
                    eda_core::ir::BinOp::Lt => left < right,
                    eda_core::ir::BinOp::Le => left <= right,
                    eda_core::ir::BinOp::Gt => left > right,
                    eda_core::ir::BinOp::Ge => left >= right,
                    _ => return None,
                };
                Some(f64::from(result as u8))
            }
            Expr::RealNeg(operand) => {
                let real = self.eval_real(operand)?;
                Some(-real)
            }
            Expr::IntToReal {
                value,
                width,
                signed,
            } => {
                let word = self.eval(value);
                let raw = word.to_u64()?;
                let real = if *signed {
                    sext_i64(&word, *width) as f64
                } else {
                    let mask = if *width < u64::BITS as usize {
                        (1_u64 << *width) - 1
                    } else {
                        u64::MAX
                    };
                    (raw & mask) as f64
                };
                real.is_finite().then_some(real)
            }
            Expr::RealTime { .. } => {
                let bits = self.eval(e);
                bits.to_u64().map(f64::from_bits)
            }
            _ => None,
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

    fn flush_strobes(&mut self) -> Result<(), String> {
        let strobes = std::mem::take(&mut self.strobes);
        for strobe in strobes {
            let line = self.format_display(strobe.parts, strobe.args);
            if self.print_stdout {
                print!("{}", line);
            }
            self.displays.push(line);
        }
        let file_strobes = std::mem::take(&mut self.file_strobes);
        for strobe in file_strobes {
            self.write_file_display(strobe.fd, strobe.parts, strobe.args, strobe.newline)?;
        }
        Ok(())
    }

    fn write_file_display(
        &mut self,
        fd: &Expr,
        parts: &[eda_core::ir::DispPart],
        args: &[Expr],
        newline: bool,
    ) -> Result<(), String> {
        let descriptor = self.file_descriptor(fd)?;
        let Some(path) = self.file_paths.borrow().get(&descriptor).cloned() else {
            return Err(format!(
                "cannot write unknown file descriptor {descriptor} at t={}",
                self.now
            ));
        };
        let is_reader = self.file_readers.borrow().contains_key(&descriptor);
        let is_writer = self.file_writers.borrow().contains_key(&descriptor);
        if is_reader && !is_writer {
            return Err(format!(
                "cannot write read-mode file descriptor {descriptor} at t={}",
                self.now
            ));
        }
        if self.file_pushbacks.borrow().contains_key(&descriptor) {
            return Err(format!(
                "cannot write mixed file descriptor {descriptor} while an ungetc byte is pending at t={}",
                self.now
            ));
        }
        let mut output = self.format_file_display(parts, args);
        if newline {
            output.push(b'\n');
        }
        let mut writers = self.file_writers.borrow_mut();
        let Some(file) = writers.get_mut(&descriptor) else {
            return Err(format!(
                "cannot write unknown file descriptor {descriptor} at t={}",
                self.now
            ));
        };
        file.write_all(&output).map_err(|error| {
            format!(
                "cannot write file '{}' for descriptor {descriptor}: {error}",
                path.display()
            )
        })?;
        let written = match file.stream_position() {
            Ok(position) => position,
            Err(_) => self
                .file_write_positions
                .borrow()
                .get(&descriptor)
                .copied()
                .unwrap_or_default()
                .saturating_add(u64::try_from(output.len()).unwrap_or(u64::MAX)),
        };
        self.file_write_positions
            .borrow_mut()
            .insert(descriptor, written);
        self.file_eof.borrow_mut().remove(&descriptor);
        self.file_errors.borrow_mut().remove(&descriptor);
        Ok(())
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

fn real_to_int(real: f64, width: usize, signed: bool) -> Option<u64> {
    real_to_int_with(real, width, signed, true)
}

fn real_trunc_to_int(real: f64, width: usize, signed: bool) -> Option<u64> {
    real_to_int_with(real, width, signed, false)
}

fn real_to_int_with(real: f64, width: usize, signed: bool, round: bool) -> Option<u64> {
    if !real.is_finite() || width == 0 || width > u64::BITS as usize {
        return None;
    }
    let magnitude = real.abs();
    let fractional = magnitude - magnitude.floor();
    let rounded = if round && fractional >= 0.5 {
        magnitude.floor() + 1.0
    } else {
        magnitude.floor()
    };
    let mut result = rounded as i128;
    if real < 0.0 {
        result = -result;
    }
    let bits = if signed {
        let min = if width == u64::BITS as usize {
            i64::MIN as i128
        } else {
            -(1_i128 << (width - 1))
        };
        let max = if width == u64::BITS as usize {
            i64::MAX as i128
        } else {
            (1_i128 << (width - 1)) - 1
        };
        if result < min || result > max {
            return None;
        }
        (result as u64) & mask_for_width(width)
    } else {
        if result < 0 || result > u64::MAX as i128 {
            return None;
        }
        result as u64 & mask_for_width(width)
    };
    Some(bits)
}

fn mask_for_width(width: usize) -> u64 {
    if width == u64::BITS as usize {
        u64::MAX
    } else {
        (1_u64 << width) - 1
    }
}

fn compare_known_words(left: &LogicWord, right: &LogicWord, signed: bool) -> std::cmp::Ordering {
    debug_assert_eq!(left.width(), right.width());
    let mut pairs = left.bits.iter().rev().zip(right.bits.iter().rev());
    if signed {
        let Some((left_sign, right_sign)) = pairs.next() else {
            return std::cmp::Ordering::Equal;
        };
        let ordering = (*right_sign == LogicValue::One).cmp(&(*left_sign == LogicValue::One));
        if ordering != std::cmp::Ordering::Equal {
            return ordering;
        }
    }
    for (a, b) in pairs {
        let ordering = (*a == LogicValue::One).cmp(&(*b == LogicValue::One));
        if ordering != std::cmp::Ordering::Equal {
            return ordering;
        }
    }
    std::cmp::Ordering::Equal
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

fn pad_formatted_text(text: &str, width: usize, left_justify: bool, padding: char) -> String {
    let length = text.chars().count();
    if length >= width {
        return text.to_string();
    }
    let padding_count = width - length;
    if left_justify {
        format!("{text}{}", padding.to_string().repeat(padding_count))
    } else {
        format!("{}{text}", padding.to_string().repeat(padding_count))
    }
}

fn format_pattern_integral(argument: &Expr, value: &LogicWord) -> String {
    format_pattern_word(value.width(), argument.is_signed(), value)
}

fn format_pattern_word(width: usize, signed: bool, value: &LogicWord) -> String {
    if !value.is_fully_known() {
        return format!("{}'b{}", width, value.to_bin_str());
    }
    let raw = value.to_u64().unwrap_or_default();
    if signed {
        if width == 32 {
            return (raw as i32).to_string();
        }
        let sign = 1_u128 << (width - 1);
        let magnitude = u128::from(raw);
        let signed = if magnitude & sign != 0 {
            magnitude as i128 - (1_i128 << width)
        } else {
            magnitude as i128
        };
        return format!("{}'sd{}", width, signed);
    }
    if width < 8 {
        return format!("{}'b{}", width, value.to_bin_str());
    }
    if width <= 32 {
        return format!("{}'d{}", width, raw);
    }
    format!("{}'h{}", width, value.to_hex_str())
}

fn format_real_exponent(real: f64, precision: usize) -> String {
    let scientific = format!("{:.*e}", precision, real);
    let Some((mantissa, exponent)) = scientific.split_once('e') else {
        return format!("{real:.6}");
    };
    let exponent = exponent.parse::<i32>().unwrap_or_default();
    let sign = if exponent < 0 { '-' } else { '+' };
    let exponent = exponent.unsigned_abs();
    format!("{mantissa}e{sign}{exponent:02}")
}

fn format_real_general(real: f64, precision: usize) -> String {
    let precision = precision.max(1);
    if real == 0.0 {
        return "0".to_string();
    }
    let scientific = format!("{:.*e}", precision - 1, real);
    let Some((mantissa, exponent)) = scientific.split_once('e') else {
        return format!("{real:.6}");
    };
    let exponent = exponent.parse::<i32>().unwrap_or_default();
    if exponent < -4 || exponent >= precision as i32 {
        let mantissa = trim_real_zeros(mantissa);
        let sign = if exponent < 0 { '-' } else { '+' };
        let exponent = exponent.unsigned_abs();
        return format!("{mantissa}e{sign}{exponent:02}");
    }
    let decimals =
        usize::try_from(i64::try_from(precision).unwrap_or(i64::MAX) - 1 - i64::from(exponent))
            .unwrap_or_default();
    trim_real_zeros(&format!("{real:.decimals$}")).to_string()
}

fn trim_real_zeros(value: &str) -> &str {
    if value.contains('.') {
        value.trim_end_matches('0').trim_end_matches('.')
    } else {
        value
    }
}

fn raw_two_state_bytes(word: &LogicWord) -> Vec<u8> {
    let width = word.width();
    if width == 0 {
        return Vec::new();
    }
    let words = width.div_ceil(64);
    let mut output = Vec::with_capacity(words * 8);
    for word_index in 0..words {
        let first_bit = word_index * 64;
        let chunk_width = (width - first_bit).min(64);
        let byte_count = if word_index == words - 1 && chunk_width <= 32 {
            4
        } else {
            8
        };
        for byte_index in 0..byte_count {
            let mut byte = 0_u8;
            for bit in 0..8 {
                let offset = first_bit + byte_index * 8 + bit;
                if offset < width && word.bit(offset) == LogicValue::One {
                    byte |= 1 << bit;
                }
            }
            output.push(byte);
        }
    }
    output
}

fn append_raw_vpi_entry(output: &mut Vec<u8>, aval: u32, bval: u32) {
    output.extend_from_slice(&aval.to_le_bytes());
    output.extend_from_slice(&bval.to_le_bytes());
}

fn raw_four_state_bytes(word: &LogicWord) -> Vec<u8> {
    let width = word.width();
    if width == 0 {
        return Vec::new();
    }
    let words = width.div_ceil(64);
    let mut output = Vec::with_capacity(words * 8);
    for word_index in 0..words {
        let first_bit = word_index * 64;
        let chunk_width = (width - first_bit).min(64);
        let entry = |entry_first_bit: usize| {
            let mut aval = 0_u32;
            let mut bval = 0_u32;
            for entry_bit in 0..32 {
                let offset = entry_first_bit + entry_bit;
                if offset >= width {
                    break;
                }
                let bit = 1_u32 << entry_bit;
                match word.bit(offset) {
                    LogicValue::Zero => {}
                    LogicValue::One => aval |= bit,
                    LogicValue::X => {
                        aval |= bit;
                        bval |= bit;
                    }
                    LogicValue::Z => bval |= bit,
                }
            }
            (aval, bval)
        };

        let (aval, bval) = entry(first_bit);
        append_raw_vpi_entry(&mut output, aval, bval);
        if word_index != words - 1 || chunk_width > 32 {
            let (aval, bval) = entry(first_bit + 32);
            append_raw_vpi_entry(&mut output, aval, bval);
        }
    }
    output
}

trait ScanInput {
    fn read_byte(&mut self) -> Result<Option<u8>, String>;
    fn push_byte(&mut self, byte: u8) -> Result<(), String>;
}

struct FileScanInput<'a, 'd> {
    simulator: &'a mut Simulator<'d>,
    descriptor: u32,
}

impl ScanInput for FileScanInput<'_, '_> {
    fn read_byte(&mut self) -> Result<Option<u8>, String> {
        self.simulator.read_file_input_byte(self.descriptor)
    }

    fn push_byte(&mut self, byte: u8) -> Result<(), String> {
        self.simulator.push_file_input_byte(self.descriptor, byte)
    }
}

struct StringScanInput {
    bytes: Vec<u8>,
    position: usize,
    pushback: Option<u8>,
}

impl StringScanInput {
    fn new(word: &LogicWord) -> Option<Self> {
        let length = ascii_string_length(word)?;
        let mut bytes = Vec::with_capacity(length);
        for index in 0..length {
            let byte = packed_string_byte(word, index)
                .to_u64()
                .map(|byte| byte as u8)?;
            if byte == 0 {
                break;
            }
            bytes.push(byte);
        }
        Some(Self {
            bytes,
            position: 0,
            pushback: None,
        })
    }
}

impl ScanInput for StringScanInput {
    fn read_byte(&mut self) -> Result<Option<u8>, String> {
        if let Some(byte) = self.pushback.take() {
            return Ok(Some(byte));
        }
        if self.position >= self.bytes.len() {
            return Ok(None);
        }
        let byte = self.bytes[self.position];
        self.position += 1;
        Ok(Some(byte))
    }

    fn push_byte(&mut self, byte: u8) -> Result<(), String> {
        if self.pushback.is_some() {
            return Err("cannot preserve more than one formatted-input lookahead byte".into());
        }
        self.pushback = Some(byte);
        Ok(())
    }
}

fn scan_target_signedness(design: &eda_core::ir::Design, targets: &[LValue]) -> Vec<bool> {
    targets
        .iter()
        .map(|target| {
            design
                .net_decls
                .iter()
                .find(|declaration| declaration.base == target.net)
                .is_some_and(|declaration| declaration.is_signed)
        })
        .collect()
}

fn scan_fields(
    input: &mut dyn ScanInput,
    format: &str,
    targets: &[LValue],
    targets_signed: &[bool],
    task_name: &str,
) -> Result<(Vec<(LValue, LogicWord)>, bool), String> {
    fn scan_digit_is_valid(byte: u8, specifier: char) -> bool {
        match specifier {
            'd' => byte.is_ascii_digit(),
            'h' => byte.is_ascii_hexdigit(),
            'o' => (b'0'..=b'7').contains(&byte),
            _ => byte == b'0' || byte == b'1',
        }
    }

    fn scan_radix(specifier: char) -> u32 {
        match specifier {
            'h' => 16,
            'o' => 8,
            'b' => 2,
            _ => 10,
        }
    }

    fn scan_word(width: usize, value: i128) -> LogicWord {
        let raw = value as u128;
        let mut bits = Vec::with_capacity(width);
        for bit in 0..width {
            bits.push(LogicValue::bit(
                bit < u128::BITS as usize && (raw >> bit) & 1 == 1,
            ));
        }
        LogicWord { bits }
    }

    fn next_scan_specifier(
        characters: &mut std::str::Chars<'_>,
        task_name: &str,
    ) -> Result<(char, Option<usize>), String> {
        let mut width = String::new();
        while matches!(characters.clone().next(), Some(c) if c.is_ascii_digit()) {
            width.push(characters.next().expect("peeked width digit"));
        }
        let field_width = if width.is_empty() {
            None
        } else {
            let parsed = width
                .parse::<usize>()
                .map_err(|_| format!("{task_name} width is too large"))?;
            if parsed == 0 || parsed > 64 {
                return Err(format!("{task_name} width must be 1..=64"));
            }
            Some(parsed)
        };
        let Some(specifier) = characters.next() else {
            return Err(format!(
                "{task_name} format ends with an incomplete conversion"
            ));
        };
        if field_width.is_some() && specifier == '%' {
            return Err(format!("{task_name} literal % cannot take a width"));
        }
        Ok((specifier, field_width))
    }

    let mut converted: Vec<(LValue, LogicWord)> = Vec::new();
    let mut target_index = 0_usize;
    let mut input_failure = false;
    let mut characters = format.chars();

    'format_loop: while let Some(part) = characters.next() {
        if part.is_whitespace() {
            loop {
                match input.read_byte()? {
                    Some(byte) if (byte as char).is_ascii_whitespace() => {}
                    Some(byte) => {
                        input.push_byte(byte)?;
                        break;
                    }
                    None => {
                        input_failure = true;
                        break 'format_loop;
                    }
                }
            }
            continue;
        }

        if part != '%' {
            match input.read_byte()? {
                Some(byte) if byte.to_ascii_lowercase() as char == part.to_ascii_lowercase() => {}
                Some(byte) => {
                    input.push_byte(byte)?;
                    break;
                }
                None => {
                    input_failure = true;
                    break;
                }
            }
            continue;
        }

        let (specifier, field_width) = next_scan_specifier(&mut characters, task_name)?;
        if specifier == '%' {
            match input.read_byte()? {
                Some(b'%') => {}
                Some(byte) => {
                    input.push_byte(byte)?;
                    break;
                }
                None => {
                    input_failure = true;
                    break;
                }
            }
            continue;
        }
        if !matches!(specifier, 'd' | 'h' | 'o' | 'b' | 's') {
            return Err(format!(
                "unsupported {task_name} conversion '%{specifier}' reached simulation"
            ));
        }
        let Some(target) = targets.get(target_index).cloned() else {
            return Err(format!("{task_name} has no target for a format conversion"));
        };
        let target_signed = targets_signed.get(target_index).copied().unwrap_or(false);
        target_index += 1;

        if specifier == 's' {
            if target.decl_width != 512 {
                return Err(format!("{task_name} %s requires a 512-bit string target"));
            }
            loop {
                match input.read_byte()? {
                    Some(byte) if (byte as char).is_ascii_whitespace() => {}
                    Some(byte) => {
                        input.push_byte(byte)?;
                        break;
                    }
                    None => {
                        input_failure = true;
                        break 'format_loop;
                    }
                }
            }
            let byte_limit = field_width.unwrap_or(63).min(63);
            let mut bytes = Vec::new();
            loop {
                match input.read_byte()? {
                    Some(byte) if (byte as char).is_ascii_whitespace() => {
                        input.push_byte(byte)?;
                        break;
                    }
                    Some(byte) => {
                        if bytes.len() >= byte_limit {
                            input.push_byte(byte)?;
                            break;
                        }
                        bytes.push(byte);
                    }
                    None => break,
                }
            }
            if bytes.is_empty() {
                break;
            }
            let value = LogicWord::from_bytes_le(target.decl_width, &bytes);
            converted.push((target, value));
            continue;
        }

        loop {
            match input.read_byte()? {
                Some(byte) if (byte as char).is_ascii_whitespace() => {}
                Some(byte) => {
                    input.push_byte(byte)?;
                    break;
                }
                None => break,
            }
        }

        let first = input.read_byte()?;
        let Some(first) = first else {
            input_failure = true;
            break;
        };
        let byte_limit = field_width.unwrap_or(usize::MAX);
        let mut consumed = 1_usize;
        let mut negative = false;
        let first = if first == b'-' || first == b'+' {
            negative = first == b'-';
            let next = match input.read_byte()? {
                Some(byte) => byte,
                None => {
                    input_failure = true;
                    break;
                }
            };
            if consumed >= byte_limit {
                input.push_byte(next)?;
                break;
            }
            consumed += 1;
            next
        } else {
            first
        };

        if !scan_digit_is_valid(first, specifier) {
            input.push_byte(first)?;
            break;
        }

        let mut digits = String::new();
        if specifier == 'h' && first == b'0' {
            match input.read_byte()? {
                Some(b'x') | Some(b'X') => {
                    if consumed >= byte_limit {
                        input.push_byte(b'x')?;
                    } else {
                        consumed += 1;
                    }
                }
                Some(byte) => {
                    digits.push(first as char);
                    input.push_byte(byte)?;
                }
                None => {
                    digits.push(first as char);
                    input_failure = true;
                }
            }
        } else {
            digits.push(first as char);
        }

        loop {
            if consumed >= byte_limit {
                break;
            }
            let Some(byte) = input.read_byte()? else {
                input_failure = true;
                break;
            };
            if byte == b'_' && !digits.is_empty() {
                consumed += 1;
                continue;
            }
            if scan_digit_is_valid(byte, specifier) {
                digits.push(byte as char);
                consumed += 1;
            } else {
                input.push_byte(byte)?;
                break;
            }
        }

        if digits.is_empty() {
            break;
        }
        let magnitude = u128::from_str_radix(&digits, scan_radix(specifier))
            .map_err(|_| format!("{task_name} integer field overflows the parser"))?;
        let magnitude = i128::try_from(magnitude)
            .map_err(|_| format!("{task_name} integer field overflows the parser"))?;
        let signed_min = -1_i128 << (target.decl_width - 1);
        let signed_max = (1_i128 << (target.decl_width - 1)) - 1;
        let unsigned_max = (1_i128 << target.decl_width) - 1;
        let value = if negative { -magnitude } else { magnitude };
        let in_range = if target_signed {
            (signed_min..=signed_max).contains(&value)
        } else {
            value >= 0 && value <= unsigned_max
        };
        if !in_range {
            return Err(format!(
                "{task_name} field {} does not fit the {}-bit target",
                if negative {
                    format!("-{magnitude}")
                } else {
                    magnitude.to_string()
                },
                target.decl_width
            ));
        }
        let target_width = target.decl_width;
        converted.push((target, scan_word(target_width, value)));
    }

    Ok((converted, input_failure))
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
        Expr::Const(_) | Expr::RealConst(_) | Expr::ShortRealConst(_) | Expr::ProcessSelf => {}
        Expr::ProcessStatus(handle) => walk_expr(handle, out),
        Expr::Past { value, gating, .. } => {
            walk_expr(value, out);
            if let Some(gating) = gating {
                walk_expr(gating, out);
            }
        }
        Expr::Sampled(value) => walk_expr(value, out),
        Expr::SampledValue { value, .. } => walk_expr(value, out),
        Expr::RealRef { net, decl_width } => out.extend(*net..*net + decl_width),
        Expr::ShortRealRef { net } => out.extend(*net..*net + 32),
        Expr::RealBinary { left, right, .. } => {
            walk_expr(left, out);
            walk_expr(right, out);
        }
        Expr::RealCompare { left, right, .. } => {
            walk_expr(left, out);
            walk_expr(right, out);
        }
        Expr::RealNeg(operand) => walk_expr(operand, out),
        Expr::ShortRealBinary { left, right, .. } => {
            walk_expr(left, out);
            walk_expr(right, out);
        }
        Expr::ShortRealNeg(operand)
        | Expr::ShortRealToReal(operand)
        | Expr::RealToShortReal(operand)
        | Expr::BitsToReal(operand)
        | Expr::BitsToShortReal(operand) => walk_expr(operand, out),
        Expr::RealToInt { value, .. }
        | Expr::RealTruncToInt { value, .. }
        | Expr::IntToReal { value, .. } => walk_expr(value, out),
        Expr::RealTime { .. } => {}
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
        Expr::StringFormat { args, .. } => {
            args.iter().for_each(|argument| walk_expr(argument, out))
        }
        Expr::StringSubstr { e, left, right } => {
            walk_expr(e, out);
            walk_expr(left, out);
            walk_expr(right, out);
        }
        Expr::EventTriggered { net } => {
            out.push(*net);
        }
        Expr::SimulationTime { .. } => {}
        Expr::StringToRadix { e, .. } => walk_expr(e, out),
        Expr::StringFromRadix { value, .. } => walk_expr(value, out),
        Expr::MemoryPattern { .. } => {}
        Expr::EnumPattern { value, .. } => walk_expr(value, out),
        Expr::StructPattern { value, .. } => walk_expr(value, out),
        Expr::MemoryRead { indexes, .. } => indexes.iter().for_each(|x| walk_expr(x, out)),
        Expr::MemoryReadPart { indexes, start, .. } => {
            indexes.iter().for_each(|x| walk_expr(x, out));
            walk_expr(start, out);
        }
        Expr::MemorySize { .. } => {}
        Expr::AssocRead { index, .. } | Expr::AssocExists { index, .. } => walk_expr(index, out),
        Expr::Random { args, .. } => args.iter().for_each(|argument| walk_expr(argument, out)),
        Expr::CountBits { value, .. }
        | Expr::IsUnknown(value)
        | Expr::OneHot(value)
        | Expr::OneHot0(value) => walk_expr(value, out),
        Expr::FileOpen { .. } => {}
        Expr::FileOpenDynamic { path, mode } => {
            walk_expr(path, out);
            walk_expr(mode, out);
        }
        Expr::FileGetc { fd } => walk_expr(fd, out),
        Expr::FileUngetc { value, fd } => {
            walk_expr(value, out);
            walk_expr(fd, out);
        }
        Expr::FileEof { fd } => walk_expr(fd, out),
        Expr::FileTell { fd } => walk_expr(fd, out),
        Expr::FileSeek { fd, offset, whence } => {
            walk_expr(fd, out);
            walk_expr(offset, out);
            walk_expr(whence, out);
        }
    }
}

fn expr_memories(e: &Expr) -> Vec<usize> {
    let mut out = Vec::new();
    walk_expr_memories(e, &mut out);
    out
}

fn memory_range_physical_indexes(
    declaration: &eda_core::ir::MemoryDecl,
    ranges: &[(i64, i64)],
    port: &str,
) -> Result<Vec<u64>, String> {
    if ranges.len() != declaration.dimensions.len() {
        return Err(format!(
            "sparse-memory port '{port}' range has {} dimensions but the memory has {}",
            ranges.len(),
            declaration.dimensions.len()
        ));
    }
    if ranges.is_empty() {
        return Err("sparse-memory ranges cannot be empty".into());
    }

    let mut expected = 1_usize;
    for (range, dimension) in ranges.iter().zip(&declaration.dimensions) {
        let (first, last) = *range;
        let last_logical = dimension.base + dimension.step * (dimension.length as i64 - 1);
        let lower = dimension.base.min(last_logical);
        let upper = dimension.base.max(last_logical);
        if !(first..=last).all(|index| index >= lower && index <= upper)
            && !(last..=first).all(|index| index >= lower && index <= upper)
        {
            return Err(format!(
                "sparse-memory port '{port}' range ({first}, {last}) is out of bounds"
            ));
        }
        let span = (i128::from(first) - i128::from(last)).unsigned_abs() + 1;
        let span = usize::try_from(span)
            .map_err(|_| format!("sparse-memory port '{port}' range is too large"))?;
        expected = expected
            .checked_mul(span)
            .ok_or_else(|| format!("sparse-memory port '{port}' range is too large"))?;
    }

    let starts = ranges.iter().map(|range| range.0).collect::<Vec<_>>();
    let mut current = starts.clone();
    let mut physical = Vec::with_capacity(expected);
    for _ in 0..expected {
        physical.push(
            declaration
                .physical_index(&current)
                .ok_or_else(|| format!("sparse-memory port '{port}' range is out of bounds"))?,
        );
        let mut incremented = false;
        for (dimension, index) in (0..ranges.len()).rev().zip((0..current.len()).rev()) {
            let (first, last) = ranges[dimension];
            if current[index] == last {
                current[index] = first;
                continue;
            }
            let step = if first <= last { 1 } else { -1 };
            current[index] = current[index]
                .checked_add(step)
                .ok_or_else(|| format!("sparse-memory port '{port}' range overflow"))?;
            incremented = true;
            break;
        }
        if !incremented {
            break;
        }
    }
    if physical.len() != expected {
        return Err(format!(
            "sparse-memory port '{port}' range produced {} elements instead of {expected}",
            physical.len()
        ));
    }
    Ok(physical)
}

fn simulation_time_unit(expression: &Expr) -> Option<i8> {
    match expression {
        Expr::SimulationTime { unit_exponent, .. } => Some(*unit_exponent),
        Expr::Signed { e: operand, .. } => simulation_time_unit(operand),
        _ => None,
    }
}

fn walk_expr_memories(e: &Expr, out: &mut Vec<usize>) {
    match e {
        Expr::Const(_)
        | Expr::RealConst(_)
        | Expr::RealTime { .. }
        | Expr::RealRef { .. }
        | Expr::ShortRealConst(_)
        | Expr::ShortRealRef { .. }
        | Expr::ProcessSelf => {}
        Expr::ProcessStatus(handle) => walk_expr_memories(handle, out),
        Expr::Past { value, gating, .. } => {
            walk_expr_memories(value, out);
            if let Some(gating) = gating {
                walk_expr_memories(gating, out);
            }
        }
        Expr::Sampled(value) => walk_expr_memories(value, out),
        Expr::SampledValue { value, .. } => walk_expr_memories(value, out),
        Expr::RealBinary { left, right, .. } | Expr::RealCompare { left, right, .. } => {
            walk_expr_memories(left, out);
            walk_expr_memories(right, out);
        }
        Expr::RealNeg(operand) => walk_expr_memories(operand, out),
        Expr::ShortRealBinary { left, right, .. } => {
            walk_expr_memories(left, out);
            walk_expr_memories(right, out);
        }
        Expr::ShortRealNeg(operand)
        | Expr::ShortRealToReal(operand)
        | Expr::RealToShortReal(operand)
        | Expr::BitsToReal(operand)
        | Expr::BitsToShortReal(operand) => walk_expr_memories(operand, out),
        Expr::RealToInt { value, .. }
        | Expr::RealTruncToInt { value, .. }
        | Expr::IntToReal { value, .. } => walk_expr_memories(value, out),
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
        Expr::StringFormat { args, .. } => args
            .iter()
            .for_each(|argument| walk_expr_memories(argument, out)),
        Expr::StringSubstr { e, left, right } => {
            walk_expr_memories(e, out);
            walk_expr_memories(left, out);
            walk_expr_memories(right, out);
        }
        Expr::EventTriggered { .. } | Expr::SimulationTime { .. } => {}
        Expr::StringToRadix { e, .. } => walk_expr_memories(e, out),
        Expr::StringFromRadix { value, .. } => walk_expr_memories(value, out),
        Expr::MemoryPattern { memory, .. } => out.push(*memory),
        Expr::EnumPattern { value, .. } => walk_expr_memories(value, out),
        Expr::StructPattern { value, .. } => walk_expr_memories(value, out),
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
        Expr::MemorySize { memory, .. } => out.push(*memory),
        Expr::AssocRead { memory, index, .. } | Expr::AssocExists { memory, index } => {
            out.push(*memory);
            walk_expr_memories(index, out);
        }
        Expr::CountBits { value, .. }
        | Expr::IsUnknown(value)
        | Expr::OneHot(value)
        | Expr::OneHot0(value) => walk_expr_memories(value, out),
        Expr::Random { args, .. } => args
            .iter()
            .for_each(|argument| walk_expr_memories(argument, out)),
        Expr::FileOpen { .. } => {}
        Expr::FileOpenDynamic { path, mode } => {
            walk_expr_memories(path, out);
            walk_expr_memories(mode, out);
        }
        Expr::FileGetc { fd } => walk_expr_memories(fd, out),
        Expr::FileUngetc { value, fd } => {
            walk_expr_memories(value, out);
            walk_expr_memories(fd, out);
        }
        Expr::FileEof { fd } => walk_expr_memories(fd, out),
        Expr::FileTell { fd } => walk_expr_memories(fd, out),
        Expr::FileSeek { fd, offset, whence } => {
            walk_expr_memories(fd, out);
            walk_expr_memories(offset, out);
            walk_expr_memories(whence, out);
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
        Stmt::MemoryCopy {
            source,
            destination,
            ..
        } => {
            out.push(*source);
            out.push(*destination);
        }
        Stmt::MemoryNew { memory, size, .. } => {
            out.push(*memory);
            walk_expr_memories(size, out);
        }
        Stmt::MemoryDelete { memory } => out.push(*memory),
        Stmt::AssocTraversal { memory, .. } => out.push(*memory),
        Stmt::QueuePush { memory, value, .. } => {
            out.push(*memory);
            walk_expr_memories(value, out);
        }
        Stmt::QueuePop { memory, .. } => out.push(*memory),
        Stmt::QueueInsert {
            memory,
            index,
            value,
        } => {
            out.push(*memory);
            walk_expr_memories(index, out);
            walk_expr_memories(value, out);
        }
        Stmt::QueueDelete { memory, index } => {
            out.push(*memory);
            walk_expr_memories(index, out);
        }
        Stmt::AssocWrite {
            memory,
            index,
            value,
            ..
        } => {
            out.push(*memory);
            walk_expr_memories(index, out);
            walk_expr_memories(value, out);
        }
        Stmt::AssocDelete { memory, index } => {
            out.push(*memory);
            if let Some(index) = index {
                walk_expr_memories(index, out);
            }
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
        Stmt::EventWait { body, .. } => walk_stmt_memories(body, out),
        Stmt::WaitOrder {
            success, failure, ..
        } => {
            if let Some(success) = success {
                walk_stmt_memories(success, out);
            }
            if let Some(failure) = failure {
                walk_stmt_memories(failure, out);
            }
        }
        Stmt::Fork { branches, .. } => branches
            .iter()
            .for_each(|branch| walk_stmt_memories(branch, out)),
        Stmt::While { cond, body, steps } => {
            walk_expr_memories(cond, out);
            walk_stmt_memories(body, out);
            steps.iter().for_each(|step| walk_stmt_memories(step, out));
        }
        Stmt::Repeat { count, body } => {
            walk_expr_memories(count, out);
            walk_stmt_memories(body, out)
        }
        Stmt::Force { e, .. } => walk_expr_memories(e, out),
        Stmt::ProceduralAssign { e, .. } => walk_expr_memories(e, out),
        Stmt::Release { .. } => {}
        Stmt::ProceduralDeassign { .. } => {}
        Stmt::Break | Stmt::Continue | Stmt::ReturnBarrier | Stmt::Return => {}
        Stmt::Scope(body) => walk_stmt_memories(body, out),
        Stmt::Display { args, .. } => args.iter().for_each(|a| walk_expr_memories(a, out)),
        Stmt::FileClose { fd } => walk_expr_memories(fd, out),
        Stmt::RandomSeeded { .. } => {}
        Stmt::FileFlush { fd } => walk_expr_memories(fd, out),
        Stmt::FileGets { fd, .. }
        | Stmt::FileError { fd, .. }
        | Stmt::FileRead { fd, .. }
        | Stmt::FileScan { fd, .. } => walk_expr_memories(fd, out),
        Stmt::StringScan { source, .. } => walk_expr_memories(source, out),
        Stmt::FileReadMemory {
            fd, start, count, ..
        } => {
            walk_expr_memories(fd, out);
            if let Some(start) = start {
                walk_expr_memories(start, out);
            }
            if let Some(count) = count {
                walk_expr_memories(count, out);
            }
        }
        Stmt::ImmediateAssertion {
            cond,
            pass_action,
            fail_action,
            ..
        } => {
            walk_expr_memories(cond, out);
            for action in pass_action.iter().chain(fail_action.iter()) {
                walk_stmt_memories(action, out);
            }
        }
        Stmt::FileDisplay { fd, args, .. } => {
            walk_expr_memories(fd, out);
            args.iter().for_each(|a| walk_expr_memories(a, out));
        }
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
        Stmt::TimeFormat { .. }
        | Stmt::AssertControl(_)
        | Stmt::Severity(_)
        | Stmt::Finish
        | Stmt::Stop => {}
        Stmt::NamedBlock { body, .. } => walk_stmt_memories(body, out),
        Stmt::DisableFork | Stmt::WaitFork | Stmt::NamedBlockEnd => {}
        Stmt::DisableNamedBlock { .. } => {}
        Stmt::ProcessKill(handle)
        | Stmt::ProcessAwait(handle)
        | Stmt::ProcessSuspend(handle)
        | Stmt::ProcessResume(handle) => walk_expr_memories(handle, out),
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
        Stmt::MemoryCopy { .. } => {}
        Stmt::MemoryNew { size, .. } => reads.extend(expr_nets(size)),
        Stmt::MemoryDelete { .. } => {}
        Stmt::QueuePush { value, .. } => reads.extend(expr_nets(value)),
        Stmt::QueuePop { destination, .. } => writes.extend(lv_bits(destination)),
        Stmt::QueueInsert { index, value, .. } => {
            reads.extend(expr_nets(index));
            reads.extend(expr_nets(value));
        }
        Stmt::QueueDelete { index, .. } => reads.extend(expr_nets(index)),
        Stmt::AssocTraversal {
            key, destination, ..
        } => {
            reads.extend(lv_bits(key));
            writes.extend(lv_bits(destination));
        }
        Stmt::AssocWrite { index, value, .. } => {
            reads.extend(expr_nets(index));
            reads.extend(expr_nets(value));
        }
        Stmt::AssocDelete { index, .. } => {
            if let Some(index) = index {
                reads.extend(expr_nets(index));
            }
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
        Stmt::EventWait { watches, body } => {
            reads.extend(watches.iter().map(|watch| watch.net));
            collect_stmt_access(body, reads, writes);
        }
        Stmt::WaitOrder {
            success, failure, ..
        } => {
            if let Some(success) = success {
                collect_stmt_access(success, reads, writes);
            }
            if let Some(failure) = failure {
                collect_stmt_access(failure, reads, writes);
            }
        }
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
        Stmt::Repeat { count, body } => {
            reads.extend(expr_nets(count));
            collect_stmt_access(body, reads, writes);
        }
        Stmt::Force { e, .. } => reads.extend(expr_nets(e)),
        Stmt::ProceduralAssign { e, .. } => reads.extend(expr_nets(e)),
        Stmt::Release { .. } => {}
        Stmt::ProceduralDeassign { .. } => {}
        Stmt::Break | Stmt::Continue | Stmt::ReturnBarrier | Stmt::Return => {}
        Stmt::Scope(body) => collect_stmt_access(body, reads, writes),
        Stmt::Display { args, .. } => reads.extend(args.iter().flat_map(expr_nets)),
        Stmt::FileClose { fd } => reads.extend(expr_nets(fd)),
        Stmt::RandomSeeded { seed, destination } => {
            reads.extend(lv_bits(seed));
            writes.extend(lv_bits(seed));
            writes.extend(lv_bits(destination));
        }
        Stmt::FileFlush { fd } => reads.extend(expr_nets(fd)),
        Stmt::FileGets { fd, string, status } => {
            reads.extend(expr_nets(fd));
            writes.extend(lv_bits(string));
            writes.extend(lv_bits(status));
        }
        Stmt::FileError {
            fd,
            message,
            status,
        } => {
            reads.extend(expr_nets(fd));
            writes.extend(lv_bits(message));
            writes.extend(lv_bits(status));
        }
        Stmt::FileRead { fd, target, status } => {
            reads.extend(expr_nets(fd));
            writes.extend(lv_bits(target));
            writes.extend(lv_bits(status));
        }
        Stmt::FileReadMemory {
            fd,
            start,
            count,
            status,
            ..
        } => {
            reads.extend(expr_nets(fd));
            if let Some(start) = start {
                reads.extend(expr_nets(start));
            }
            if let Some(count) = count {
                reads.extend(expr_nets(count));
            }
            writes.extend(lv_bits(status));
        }
        Stmt::FileScan {
            fd,
            targets,
            status,
            ..
        } => {
            reads.extend(expr_nets(fd));
            for target in targets {
                writes.extend(lv_bits(target));
            }
            writes.extend(lv_bits(status));
        }
        Stmt::StringScan {
            source,
            targets,
            status,
            ..
        } => {
            reads.extend(expr_nets(source));
            for target in targets {
                writes.extend(lv_bits(target));
            }
            writes.extend(lv_bits(status));
        }
        Stmt::ImmediateAssertion {
            cond,
            pass_action,
            fail_action,
            ..
        } => {
            reads.extend(expr_nets(cond));
            for action in pass_action.iter().chain(fail_action.iter()) {
                collect_stmt_access(action, reads, writes);
            }
        }
        Stmt::FileDisplay { fd, args, .. } => {
            reads.extend(expr_nets(fd));
            reads.extend(args.iter().flat_map(expr_nets));
        }
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
        Stmt::TimeFormat { .. }
        | Stmt::AssertControl(_)
        | Stmt::Severity(_)
        | Stmt::Finish
        | Stmt::Stop => {}
        Stmt::NamedBlock { body, .. } => collect_stmt_access(body, reads, writes),
        Stmt::DisableFork | Stmt::WaitFork | Stmt::NamedBlockEnd => {}
        Stmt::DisableNamedBlock { .. } => {}
        Stmt::ProcessKill(handle)
        | Stmt::ProcessAwait(handle)
        | Stmt::ProcessSuspend(handle)
        | Stmt::ProcessResume(handle) => reads.extend(expr_nets(handle)),
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
        Stmt::MemoryCopy { .. } => {}
        Stmt::MemoryNew { size, .. } => walk_expr(size, out),
        Stmt::MemoryDelete { .. } => {}
        Stmt::AssocTraversal {
            key, destination, ..
        } => {
            out.extend(lv_bits(key));
            out.extend(lv_bits(destination));
        }
        Stmt::QueuePush { value, .. } => walk_expr(value, out),
        Stmt::QueuePop { .. } => {}
        Stmt::QueueInsert { index, value, .. } => {
            walk_expr(index, out);
            walk_expr(value, out);
        }
        Stmt::QueueDelete { index, .. } => walk_expr(index, out),
        Stmt::AssocWrite { index, value, .. } => {
            walk_expr(index, out);
            walk_expr(value, out);
        }
        Stmt::AssocDelete { index, .. } => {
            if let Some(index) = index {
                walk_expr(index, out);
            }
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
        Stmt::EventWait { watches, body } => {
            out.extend(watches.iter().map(|watch| watch.net));
            walk_stmt(body, out);
        }
        Stmt::WaitOrder {
            events,
            success,
            failure,
            ..
        } => {
            out.extend(events.iter().copied());
            if let Some(success) = success {
                walk_stmt(success, out);
            }
            if let Some(failure) = failure {
                walk_stmt(failure, out);
            }
        }
        Stmt::Fork { branches, .. } => branches.iter().for_each(|branch| walk_stmt(branch, out)),
        Stmt::While { cond, body, steps } => {
            walk_expr(cond, out);
            walk_stmt(body, out);
            steps.iter().for_each(|step| walk_stmt(step, out));
        }
        Stmt::Repeat { count, body } => {
            walk_expr(count, out);
            walk_stmt(body, out)
        }
        Stmt::Force { e, .. } => walk_expr(e, out),
        Stmt::ProceduralAssign { e, .. } => walk_expr(e, out),
        Stmt::Release { .. } => {}
        Stmt::ProceduralDeassign { .. } => {}
        Stmt::Break | Stmt::Continue | Stmt::ReturnBarrier | Stmt::Return => {}
        Stmt::Scope(body) => walk_stmt(body, out),
        Stmt::Display { args, .. } => args.iter().for_each(|a| walk_expr(a, out)),
        Stmt::FileClose { fd } => walk_expr(fd, out),
        Stmt::RandomSeeded { seed, destination } => {
            out.extend(lv_bits(seed));
            out.extend(lv_bits(destination));
        }
        Stmt::FileFlush { fd } => walk_expr(fd, out),
        Stmt::FileGets { fd, string, status } => {
            walk_expr(fd, out);
            out.extend(lv_bits(string));
            out.extend(lv_bits(status));
        }
        Stmt::FileError {
            fd,
            message,
            status,
        } => {
            walk_expr(fd, out);
            out.extend(lv_bits(message));
            out.extend(lv_bits(status));
        }
        Stmt::FileRead { fd, target, status } => {
            walk_expr(fd, out);
            out.extend(lv_bits(target));
            out.extend(lv_bits(status));
        }
        Stmt::FileReadMemory {
            fd,
            start,
            count,
            status,
            ..
        } => {
            walk_expr(fd, out);
            if let Some(start) = start {
                walk_expr(start, out);
            }
            if let Some(count) = count {
                walk_expr(count, out);
            }
            out.extend(lv_bits(status));
        }
        Stmt::FileScan {
            fd,
            targets,
            status,
            ..
        } => {
            walk_expr(fd, out);
            for target in targets {
                out.extend(lv_bits(target));
            }
            out.extend(lv_bits(status));
        }
        Stmt::StringScan {
            source,
            targets,
            status,
            ..
        } => {
            walk_expr(source, out);
            for target in targets {
                out.extend(lv_bits(target));
            }
            out.extend(lv_bits(status));
        }
        Stmt::ImmediateAssertion {
            cond,
            pass_action,
            fail_action,
            ..
        } => {
            walk_expr(cond, out);
            for action in pass_action.iter().chain(fail_action.iter()) {
                walk_stmt(action, out);
            }
        }
        Stmt::FileDisplay { fd, args, .. } => {
            walk_expr(fd, out);
            args.iter().for_each(|a| walk_expr(a, out));
        }
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
        Stmt::TimeFormat { .. }
        | Stmt::AssertControl(_)
        | Stmt::Severity(_)
        | Stmt::Finish
        | Stmt::Stop => {}
        Stmt::NamedBlock { body, .. } => walk_stmt(body, out),
        Stmt::DisableFork | Stmt::WaitFork | Stmt::NamedBlockEnd => {}
        Stmt::DisableNamedBlock { .. } => {}
        Stmt::ProcessKill(handle)
        | Stmt::ProcessAwait(handle)
        | Stmt::ProcessSuspend(handle)
        | Stmt::ProcessResume(handle) => walk_expr(handle, out),
    }
}

fn collect_monitors<'d>(s: &'d Stmt, out: &mut Vec<(&'d [eda_core::ir::DispPart], &'d [Expr])>) {
    match s {
        Stmt::Display {
            parts,
            args,
            monitor,
            ..
        } if *monitor => {
            out.push((parts, args));
        }
        Stmt::Block(v) => v.iter().for_each(|x| collect_monitors(x, out)),
        _ => {}
    }
}

fn associative_key_compare(
    left: u64,
    right: u64,
    width: usize,
    signed: bool,
) -> std::cmp::Ordering {
    if signed {
        signed_associative_key(left, width).cmp(&signed_associative_key(right, width))
    } else {
        left.cmp(&right)
    }
}

fn signed_associative_key(key: u64, width: usize) -> i64 {
    if width >= 64 {
        return key as i64;
    }
    let sign = 1_u64 << width;
    if key & (sign >> 1) != 0 {
        key as i64 - sign as i64
    } else {
        key as i64
    }
}

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
