//! # eda-synth — RTL synthesis
//!
//! Lowers an elaborated [`Design`] to a gate-level [`Netlist`] in three
//! stages: bit-blasting to generic gates, logic optimization (constant
//! folding, buffer collapse, duplicate merging, dead-code elimination), and
//! technology mapping onto the standard-cell library.

use eda_core::ir::{
    BinOp, Design, Expr, LValue, MemoryByteEnable, MemoryKind, MemoryTemplate, MemoryWriteMask,
    Range, Sens, Stmt, UnOp,
};
use eda_core::library::CellLibrary;
use eda_core::logic::{LogicValue, LogicWord};
use eda_core::netlist::{Cell, CellKind, Dir, NetId, Netlist};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

fn sorted_set(set: &HashSet<usize>) -> Vec<usize> {
    let mut values = set.iter().copied().collect::<Vec<_>>();
    values.sort_unstable();
    values
}

pub mod map;
pub mod opt;
pub mod writer;

pub use map::map_to_library;
pub use writer::write_verilog;

#[derive(Clone, Debug)]
pub struct SynthOptions {
    pub optimize: bool,
    pub map_lib: bool,
}

impl Default for SynthOptions {
    fn default() -> Self {
        SynthOptions {
            optimize: true,
            map_lib: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SynthResult {
    pub netlist: Netlist,
    pub library: Arc<CellLibrary>,
    pub warnings: Vec<String>,
    pub memory_plans: Vec<MemoryMacroPlan>,
}

/// Technology-independent macro candidate.
///
/// This is not a vendor cell selection. The thresholds deliberately separate
/// small structures that can reasonably remain distributed logic from larger
/// structures that require block-RAM/ROM mapping or explicit partitioning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryMacroCandidate {
    DistributedRom,
    BlockRom,
    DistributedRam,
    BlockRam,
    NoStoragePort,
    PartitionRequired,
    UnsupportedAsyncLargeMemory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryMacroPlan {
    pub memory: usize,
    pub name: String,
    pub kind: MemoryKind,
    pub template: MemoryTemplate,
    pub element_width: usize,
    pub bank_count: u64,
    pub bank_depth: u64,
    pub total_bits: u128,
    pub write_mask: Option<MemoryWriteMask>,
    pub byte_enable: Option<MemoryByteEnable>,
    pub candidate: MemoryMacroCandidate,
    pub rationale: String,
}

pub fn plan_memories(design: &Design) -> Vec<MemoryMacroPlan> {
    design
        .memories
        .iter()
        .enumerate()
        .map(|(memory_index, memory)| {
            let total_bits =
                u128::from(memory.flat_length) * u128::try_from(memory.element_width).unwrap_or(0);
            let candidate = macro_candidate(memory.template, total_bits);
            let rationale = match candidate {
                MemoryMacroCandidate::DistributedRom => {
                    "small read-only structure can remain distributed logic"
                }
                MemoryMacroCandidate::BlockRom => {
                    "synchronous read timing and size favor block-ROM mapping"
                }
                MemoryMacroCandidate::DistributedRam => {
                    "small runtime-writable structure can map to distributed/LUT RAM"
                }
                MemoryMacroCandidate::BlockRam => {
                    "synchronous write timing and size favor block-RAM mapping"
                }
                MemoryMacroCandidate::NoStoragePort => {
                    "no hardware storage access was inferred for this object"
                }
                MemoryMacroCandidate::PartitionRequired => {
                    "mixed timing requires explicit port/bank partitioning"
                }
                MemoryMacroCandidate::UnsupportedAsyncLargeMemory => {
                    "large asynchronous access is not covered by the current macro model"
                }
            };
            MemoryMacroPlan {
                memory: memory_index,
                name: memory.name.clone(),
                kind: memory.kind,
                template: memory.template,
                element_width: memory.element_width,
                bank_count: memory.banking.count,
                bank_depth: memory.banking.depth,
                total_bits,
                write_mask: memory.write_mask.clone(),
                byte_enable: memory.byte_enable.clone(),
                candidate,
                rationale: rationale.to_string(),
            }
        })
        .collect()
}

fn macro_candidate(template: MemoryTemplate, total_bits: u128) -> MemoryMacroCandidate {
    const DISTRIBUTED_ROM_LIMIT: u128 = 2048;
    const DISTRIBUTED_RAM_LIMIT: u128 = 4096;
    match template {
        MemoryTemplate::Unmapped => MemoryMacroCandidate::NoStoragePort,
        MemoryTemplate::AsyncRom => {
            if total_bits <= DISTRIBUTED_ROM_LIMIT {
                MemoryMacroCandidate::DistributedRom
            } else {
                MemoryMacroCandidate::UnsupportedAsyncLargeMemory
            }
        }
        MemoryTemplate::SyncRom => {
            if total_bits <= DISTRIBUTED_ROM_LIMIT {
                MemoryMacroCandidate::DistributedRom
            } else {
                MemoryMacroCandidate::BlockRom
            }
        }
        MemoryTemplate::AsyncRam => {
            if total_bits <= DISTRIBUTED_RAM_LIMIT {
                MemoryMacroCandidate::DistributedRam
            } else {
                MemoryMacroCandidate::UnsupportedAsyncLargeMemory
            }
        }
        MemoryTemplate::SyncRam => {
            if total_bits <= DISTRIBUTED_RAM_LIMIT {
                MemoryMacroCandidate::DistributedRam
            } else {
                MemoryMacroCandidate::BlockRam
            }
        }
        MemoryTemplate::MixedRom | MemoryTemplate::MixedRam => {
            MemoryMacroCandidate::PartitionRequired
        }
    }
}

pub fn synthesize(design: &Design, opts: &SynthOptions) -> Result<SynthResult, String> {
    synthesize_with_library(design, opts, &CellLibrary::std())
}

/// Synthesize using an imported/custom standard-cell library.
pub fn synthesize_with_library(
    design: &Design,
    opts: &SynthOptions,
    lib: &CellLibrary,
) -> Result<SynthResult, String> {
    let memory_plans = plan_memories(design);
    let mut s = Synth::new(design)?;
    s.run()?;
    let mut warnings = std::mem::take(&mut s.warnings);
    let unlowered = memory_plans
        .iter()
        .filter(|plan| !s.lowered_memories.contains(&plan.memory))
        .count();
    if unlowered != 0 {
        warnings.push(format!(
            "{unlowered} sparse-memory object(s) have macro plans but no synthesis lowering"
        ));
    }
    let mut nl = s.finish()?;
    if opts.optimize {
        opt::optimize(&mut nl, &mut warnings);
    }
    let lib = Arc::new(lib.clone());
    let mut nl = if opts.map_lib {
        map::map_to_library(&nl, &lib)?
    } else {
        nl
    };
    nl.attach_hierarchy(design);
    Ok(SynthResult {
        netlist: nl,
        library: lib,
        warnings,
        memory_plans,
    })
}

struct Synth<'a> {
    design: &'a Design,
    nl: Netlist,
    flat2net: Vec<NetId>,
    rom_contents: HashMap<usize, RomContents>,
    ram_state: HashMap<usize, Vec<Vec<NetId>>>,
    lowered_memories: HashSet<usize>,
    skipped_processes: HashSet<usize>,
    sym: HashMap<usize, Vec<NetId>>,
    written: HashSet<usize>,
    warnings: Vec<String>,
    tmp: usize,
    driver_log: Vec<(NetId, NetId)>,
    /// Fixed present-state symbols while lowering a clocked process.
    read_sym: Option<HashMap<usize, Vec<NetId>>>,
}

#[derive(Clone, Default)]
struct RomContents {
    values: BTreeMap<Vec<i64>, LogicWord>,
    deterministic: bool,
}

impl RomContents {
    fn deterministic() -> Self {
        Self {
            values: BTreeMap::new(),
            deterministic: true,
        }
    }
}

fn rom_contents_from_design(design: &Design) -> HashMap<usize, RomContents> {
    let mut contents = HashMap::<usize, RomContents>::new();
    for statement in &design.initials {
        collect_rom_initializer(statement, &mut contents);
    }
    contents
}

fn collect_rom_initializer(statement: &Stmt, contents: &mut HashMap<usize, RomContents>) {
    match statement {
        Stmt::Block(statements) => statements
            .iter()
            .for_each(|statement| collect_rom_initializer(statement, contents)),
        Stmt::MemoryBl { memory, indexes, e } | Stmt::MemoryNBl { memory, indexes, e } => {
            let entry = contents
                .entry(*memory)
                .or_insert_with(RomContents::deterministic);
            let Some(indexes) = indexes
                .iter()
                .map(constant_index)
                .collect::<Option<Vec<_>>>()
            else {
                entry.deterministic = false;
                return;
            };
            let Some(value) = constant_word(e) else {
                entry.deterministic = false;
                return;
            };
            entry.values.insert(indexes, value);
        }
        Stmt::If { t, f, .. } => {
            collect_rom_initializer(t, contents);
            if let Some(f) = f {
                collect_rom_initializer(f, contents);
            }
            for entry in contents.values_mut() {
                entry.deterministic = false;
            }
        }
        Stmt::Case { arms, dflt, .. } => {
            for (_, body) in arms {
                collect_rom_initializer(body, contents);
            }
            if let Some(body) = dflt {
                collect_rom_initializer(body, contents);
            }
            for entry in contents.values_mut() {
                entry.deterministic = false;
            }
        }
        _ => {
            for entry in contents.values_mut() {
                entry.deterministic = false;
            }
        }
    }
}

fn constant_index(expression: &Expr) -> Option<i64> {
    constant_word(expression)?
        .to_u64()
        .map(|value| value as i64)
}

fn constant_word(expression: &Expr) -> Option<LogicWord> {
    match expression {
        Expr::Const(word) => Some(word.clone()),
        Expr::Signed { e, .. } => constant_word(e),
        _ => None,
    }
}

fn physical_to_logical(memory: &eda_core::ir::MemoryDecl, physical_index: u64) -> Option<Vec<i64>> {
    if physical_index >= memory.flat_length {
        return None;
    }
    let mut remaining = physical_index;
    let mut stride = memory.flat_length;
    let mut indexes = Vec::with_capacity(memory.dimensions.len());
    for dimension in &memory.dimensions {
        stride /= dimension.length as u64;
        let offset = remaining / stride % dimension.length as u64;
        remaining %= stride;
        indexes.push(dimension.base + dimension.step * offset as i64);
    }
    Some(indexes)
}

fn part_value_index(
    memory: &eda_core::ir::MemoryDecl,
    target: &DistributedRamPartTarget,
    physical_bit: usize,
) -> Option<usize> {
    if physical_bit < target.low || physical_bit > target.high {
        return None;
    }
    Some(if memory.element_is_reversed() {
        target.high - physical_bit
    } else {
        physical_bit - target.low
    })
}

#[derive(Clone)]
struct DistributedRamWrite {
    memory: usize,
    condition: Option<Expr>,
    indexes: Vec<Expr>,
    value: Expr,
    part: Option<DistributedRamWritePart>,
}

#[derive(Clone)]
struct DistributedRamWritePart {
    start: Expr,
    width: usize,
    ascending: bool,
}

struct DistributedRamPartTarget {
    low: usize,
    high: usize,
    start_select: Option<NetId>,
}

impl<'a> Synth<'a> {
    fn new(design: &'a Design) -> Result<Self, String> {
        let mut nl = Netlist::new(design.name.clone());
        let mut flat2net = vec![usize::MAX; design.n_nets];
        for d in &design.net_decls {
            for i in 0..d.width {
                let name = if d.width == 1 {
                    d.name.clone()
                } else {
                    format!("{}[{}]", d.name, i)
                };
                flat2net[d.base + i] = nl.add_net(name);
            }
        }
        for p in &design.ports {
            let bits: Vec<NetId> = (0..p.width).map(|i| flat2net[p.base + i]).rev().collect();
            let dir = if p.dir == eda_core::ir::Dir::In {
                Dir::In
            } else {
                Dir::Out
            };
            nl.add_port(p.name.clone(), dir, bits);
        }
        Ok(Synth {
            design,
            nl,
            flat2net,
            rom_contents: rom_contents_from_design(design),
            ram_state: HashMap::new(),
            lowered_memories: HashSet::new(),
            skipped_processes: HashSet::new(),
            sym: HashMap::new(),
            written: HashSet::new(),
            warnings: Vec::new(),
            tmp: 0,
            driver_log: Vec::new(),
            read_sym: None,
        })
    }

    fn run(&mut self) -> Result<(), String> {
        self.lower_distributed_ram_processes()?;
        for a in &self.design.assigns {
            self.reset_sym();
            let w = a.lv.range.width();
            let v = self.blast(&a.e, w)?;
            self.hook_assign(&a.lv, &v)?;
        }
        for (process_index, p) in self.design.processes.iter().enumerate() {
            if self.skipped_processes.contains(&process_index) {
                continue;
            }
            self.reset_sym();
            match &p.sens {
                Sens::Comb => {
                    self.exec_stmt(&p.body)?;
                    let written = sorted_set(&self.written);
                    let sym = self.sym.clone();
                    for base in written {
                        let width = self.decl_width(base);
                        let vals = sym.get(&base).cloned().unwrap_or_else(|| {
                            (0..width).map(|i| self.flat2net[base + i]).collect()
                        });
                        for (i, v) in vals.iter().enumerate() {
                            self.declare_driver(self.flat2net[base + i], *v)?;
                        }
                    }
                }
                Sens::Posedge(n) | Sens::Negedge(n) => {
                    let clk = self.flat2net_of(*n)?;
                    let pos = matches!(p.sens, Sens::Posedge(_));
                    self.lower_seq(clk, pos, None, &p.body)?;
                }
                Sens::List(v) => {
                    let mut clock: Option<(NetId, bool)> = None;
                    let mut reset: Option<(NetId, bool)> = None;
                    let mut bad = false;
                    for s in v {
                        match s {
                            Sens::Posedge(x) => {
                                let net = self.flat2net_of(*x)?;
                                if clock.is_none() {
                                    clock = Some((net, true));
                                } else if reset.is_none() {
                                    reset = Some((net, true));
                                } else {
                                    bad = true;
                                }
                            }
                            Sens::Negedge(x) => {
                                let net = self.flat2net_of(*x)?;
                                if clock.is_none() {
                                    clock = Some((net, false));
                                } else if reset.is_none() {
                                    reset = Some((net, false));
                                } else {
                                    bad = true;
                                }
                            }
                            _ => bad = true,
                        }
                    }
                    if bad {
                        return Err("unsupported sensitivity list".into());
                    }
                    let Some((clk, pos)) = clock else {
                        return Err("always block has no clock edge".into());
                    };
                    self.lower_seq(clk, pos, reset, &p.body)?;
                }
            }
        }
        // hookup: driven decl bits get their value; when the value is a
        // single-use internal cell output, retarget that cell instead of
        // inserting a buffer.
        let drivers: Vec<(NetId, NetId)> = self.drivers_list();
        let mut src_uses: HashMap<NetId, usize> = HashMap::new();
        let port_nets: HashSet<NetId> = self
            .nl
            .ports
            .iter()
            .flat_map(|p| p.bits.iter().copied())
            .collect();
        for (dst, src) in &drivers {
            if dst != src && !port_nets.contains(src) {
                *src_uses.entry(*src).or_insert(0) += 1;
            }
        }
        for (dst, src) in drivers {
            if dst == src {
                continue;
            }
            let single = self.nl.net(src).drivers.len() == 1
                && src_uses.get(&src).copied().unwrap_or(0) == 1
                && !port_nets.contains(&src);
            if single {
                let ci = self.nl.net(src).drivers[0];
                self.nl.retarget_cell_output(ci, dst);
            } else {
                self.add_cell("hook", CellKind::Buf, vec![src], dst);
            }
        }
        Ok(())
    }

    fn lower_distributed_ram_processes(&mut self) -> Result<(), String> {
        for process_index in 0..self.design.processes.len() {
            let process = self.design.processes[process_index].clone();
            let (clock, posedge) = match &process.sens {
                Sens::Posedge(net) => (*net, true),
                Sens::Negedge(net) => (*net, false),
                _ => continue,
            };
            let Some(write) = self.distributed_ram_write(&process.body) else {
                continue;
            };
            if !self.is_distributed_ram(write.memory) {
                continue;
            }
            if self.ram_state.contains_key(&write.memory) {
                return Err(format!(
                    "memory '{}' has multiple clocked write processes, which distributed-RAM lowering does not support",
                    self.design.memories[write.memory].name
                ));
            }
            self.lower_distributed_ram_process(clock, posedge, write)?;
            self.skipped_processes.insert(process_index);
        }
        Ok(())
    }

    fn distributed_ram_write(&self, statement: &Stmt) -> Option<DistributedRamWrite> {
        match statement {
            Stmt::Block(statements) if statements.len() == 1 => {
                self.distributed_ram_write(&statements[0])
            }
            Stmt::MemoryBl {
                memory, indexes, e, ..
            }
            | Stmt::MemoryNBl {
                memory, indexes, e, ..
            } => Some(DistributedRamWrite {
                memory: *memory,
                condition: None,
                indexes: indexes.clone(),
                value: (*e).clone(),
                part: None,
            }),
            Stmt::MemoryBlPart {
                memory,
                indexes,
                start,
                width,
                ascending,
                e,
                ..
            }
            | Stmt::MemoryNBlPart {
                memory,
                indexes,
                start,
                width,
                ascending,
                e,
                ..
            } => Some(DistributedRamWrite {
                memory: *memory,
                condition: None,
                indexes: indexes.clone(),
                value: (*e).clone(),
                part: Some(DistributedRamWritePart {
                    start: (**start).clone(),
                    width: *width,
                    ascending: *ascending,
                }),
            }),
            Stmt::If { c, t, f: None, .. } => {
                let write = self.distributed_ram_write(t)?;
                write.condition.as_ref().map(|_| None).unwrap_or_else(|| {
                    Some(DistributedRamWrite {
                        condition: Some((*c).clone()),
                        ..write
                    })
                })
            }
            _ => None,
        }
    }

    fn is_distributed_ram(&self, memory_index: usize) -> bool {
        let Some(memory) = self.design.memories.get(memory_index) else {
            return false;
        };
        memory.kind == MemoryKind::Ram
            && macro_candidate(
                memory.template,
                u128::from(memory.flat_length) * u128::try_from(memory.element_width).unwrap_or(0),
            ) == MemoryMacroCandidate::DistributedRam
    }

    fn lower_distributed_ram_process(
        &mut self,
        clock_flat: usize,
        posedge: bool,
        write: DistributedRamWrite,
    ) -> Result<(), String> {
        let memory = self.design.memories[write.memory].clone();
        let clock = self.flat2net_of(clock_flat)?;
        let state = (0..memory.flat_length)
            .map(|_| {
                (0..memory.element_width)
                    .map(|_| self.new_tmp("ram_q"))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        self.ram_state.insert(write.memory, state);

        self.reset_sym();
        let condition = match &write.condition {
            Some(condition) => Some(self.blast(condition, 1)?[0]),
            None => None,
        };
        let value_width = write
            .part
            .as_ref()
            .map(|part| part.width)
            .unwrap_or(memory.element_width);
        let value = self.blast(&write.value, value_width)?;
        let part_targets = match &write.part {
            Some(part) => Some(self.distributed_ram_part_targets(&memory, part)?),
            None => None,
        };
        let old_state = self
            .ram_state
            .get(&write.memory)
            .cloned()
            .unwrap_or_default();
        let mut next_state = Vec::with_capacity(memory.flat_length as usize);
        for (physical_index, old_bits) in old_state.iter().enumerate() {
            let physical_index = physical_index as u64;
            let mut select = self.memory_address_select(&memory, physical_index, &write.indexes)?;
            if let Some(condition) = condition {
                select = self.combine(CellKind::And, select, condition);
            }
            let mut updated = Vec::with_capacity(memory.element_width);
            for (bit, old) in old_bits.iter().enumerate() {
                let mut next = *old;
                match &part_targets {
                    None => {
                        next = self.mux(select, value[bit], next);
                    }
                    Some(targets) => {
                        for target in targets {
                            let Some(value_index) = part_value_index(&memory, target, bit) else {
                                continue;
                            };
                            let bit_select = match target.start_select {
                                Some(start_select) => {
                                    self.combine(CellKind::And, select, start_select)
                                }
                                None => select,
                            };
                            next = self.mux(bit_select, value[value_index], next);
                        }
                    }
                }
                self.add_cell("dxff", CellKind::Dff { posedge }, vec![clock, next], *old);
                updated.push(*old);
            }
            next_state.push(updated);
        }
        self.ram_state.insert(write.memory, next_state);
        self.lowered_memories.insert(write.memory);
        Ok(())
    }

    fn distributed_ram_part_targets(
        &mut self,
        memory: &eda_core::ir::MemoryDecl,
        part: &DistributedRamWritePart,
    ) -> Result<Vec<DistributedRamPartTarget>, String> {
        if part.width == 0 || part.width > memory.element_width {
            return Err(format!(
                "memory '{}' has an invalid distributed-RAM part-select width {}",
                memory.name, part.width
            ));
        }
        if let Some(start) = constant_word(&part.start)
            .and_then(|word| word.to_u64())
            .map(|start| start as i64)
        {
            let (low, high) = memory
                .element_part_offsets(start, part.width, part.ascending)
                .ok_or_else(|| {
                    format!(
                        "memory '{}' distributed-RAM part select start {start} is out of range",
                        memory.name
                    )
                })?;
            return Ok(vec![DistributedRamPartTarget {
                low,
                high,
                start_select: None,
            }]);
        }

        let start_width = part.start.width().max(1);
        let lower = memory.element_msb.min(memory.element_lsb);
        let upper = memory.element_msb.max(memory.element_lsb);
        let first = if part.ascending {
            lower
        } else {
            lower + part.width as i64 - 1
        };
        let last = if part.ascending {
            upper - part.width as i64 + 1
        } else {
            upper
        };
        let mut targets: Vec<DistributedRamPartTarget> = Vec::new();
        for logical_start in first..=last {
            let Some((low, high)) =
                memory.element_part_offsets(logical_start, part.width, part.ascending)
            else {
                continue;
            };
            if targets.iter().any(|target| target.low == low) {
                continue;
            }
            let target = Expr::Const(LogicWord::from_u64(start_width, logical_start as u64));
            let equal = self.blast_bin(BinOp::Eq, &part.start, &target)?;
            let Some(equal) = equal.first() else {
                return Err(format!(
                    "memory '{}' distributed-RAM part-select comparison produced no select",
                    memory.name
                ));
            };
            targets.push(DistributedRamPartTarget {
                low,
                high,
                start_select: Some(*equal),
            });
        }
        if targets.is_empty() {
            return Err(format!(
                "memory '{}' distributed-RAM part select has no legal starts",
                memory.name
            ));
        }
        Ok(targets)
    }

    fn drivers_list(&self) -> Vec<(NetId, NetId)> {
        // stored implicitly on the netlist through drivers map kept in a
        // side table rebuilt each call — see declare_driver
        self.driver_log.to_vec()
    }

    fn finish(self) -> Result<Netlist, String> {
        let nl = self.nl;
        for p in &nl.ports {
            if p.dir == Dir::Out {
                for (i, b) in p.bits.iter().enumerate() {
                    if nl.net(*b).drivers.is_empty() {
                        return Err(format!("output '{}[{}]' is not driven", p.name, i));
                    }
                }
            }
        }
        Ok(nl)
    }

    fn reset_sym(&mut self) {
        self.sym.clear();
        self.written.clear();
        self.read_sym = None;
    }

    fn decl_width(&self, base: usize) -> usize {
        self.design
            .net_decls
            .iter()
            .find(|d| d.base == base)
            .map(|d| d.width)
            .unwrap_or(1)
    }

    fn flat2net_of(&self, flat: usize) -> Result<NetId, String> {
        self.flat2net
            .get(flat)
            .copied()
            .filter(|n| *n != usize::MAX)
            .ok_or_else(|| format!("flat net {} out of range", flat))
    }

    fn present_state_symbol_map(&self) -> HashMap<usize, Vec<NetId>> {
        self.design
            .net_decls
            .iter()
            .map(|decl| {
                let nets = (0..decl.width)
                    .map(|i| self.flat2net[decl.base + i])
                    .collect();
                (decl.base, nets)
            })
            .collect()
    }

    fn declare_driver(&mut self, dst: NetId, src: NetId) -> Result<(), String> {
        if let Some(prev) = self
            .driver_log
            .iter()
            .find(|(d, _)| *d == dst)
            .copied()
            .map(|(_, s)| s)
        {
            if prev != src {
                return Err(format!(
                    "net '{}' has multiple drivers (unsupported)",
                    self.nl.net(dst).name
                ));
            }
            return Ok(());
        }
        self.driver_log.push((dst, src));
        Ok(())
    }

    fn add_cell(&mut self, hint: &str, kind: CellKind, inputs: Vec<NetId>, output: NetId) {
        let name = format!("{}_{}", hint, self.tmp);
        self.tmp += 1;
        self.nl.add_cell(Cell {
            name,
            kind,
            inputs,
            output,
        });
    }

    fn new_tmp(&mut self, hint: &str) -> NetId {
        let name = format!("{}_n{}", hint, self.tmp);
        self.tmp += 1;
        self.nl.add_net(name)
    }

    fn const_net(&mut self, v: LogicValue) -> NetId {
        let out = self.new_tmp("c");
        self.add_cell("const", CellKind::Const(v), vec![], out);
        out
    }

    fn gate1(&mut self, hint: &str, kind: CellKind, a: NetId) -> NetId {
        let out = self.new_tmp(hint);
        self.add_cell(hint, kind, vec![a], out);
        out
    }

    fn gate3(&mut self, hint: &str, kind: CellKind, a: NetId, b: NetId, c: NetId) -> NetId {
        let out = self.new_tmp(hint);
        self.add_cell(hint, kind, vec![a, b, c], out);
        out
    }

    fn const_val(&self, net: NetId) -> Option<LogicValue> {
        let ds = &self.nl.net(net).drivers;
        match ds.len() {
            1 => match &self.nl.cell(ds[0]).kind {
                CellKind::Const(v) => Some(*v),
                CellKind::Buf => self.const_val(self.nl.cell(ds[0]).inputs[0]),
                _ => None,
            },
            _ => None,
        }
    }

    // ---------- expression bit-blasting ----------

    fn blast(&mut self, e: &Expr, width: usize) -> Result<Vec<NetId>, String> {
        let mut out = self.blast_nat(e)?;
        if out.len() < width && e.is_signed() {
            if let Some(&sign) = out.last() {
                while out.len() < width {
                    out.push(sign);
                }
            }
        }
        out.truncate(width);
        while out.len() < width {
            out.push(self.const_net(LogicValue::Zero));
        }
        Ok(out)
    }

    fn blast_nat(&mut self, e: &Expr) -> Result<Vec<NetId>, String> {
        Ok(match e {
            Expr::Const(w) => {
                let mut nets = Vec::new();
                for b in &w.bits {
                    nets.push(self.const_net(*b));
                }
                nets
            }
            Expr::Ref { net, range, .. } => {
                let decl = self.decl_of_flat(*net)?;
                let base = decl.base;
                let w = decl.width;
                let source = self.read_sym.as_ref().unwrap_or(&self.sym);
                if !source.contains_key(&base) {
                    let nets: Vec<NetId> = (0..w).map(|i| self.flat2net[base + i]).collect();
                    if self.read_sym.is_none() {
                        self.sym.insert(base, nets.clone());
                    }
                    if let Some(read) = self.read_sym.as_mut() {
                        read.insert(base, nets.clone());
                    }
                }
                let sym = self
                    .read_sym
                    .as_ref()
                    .unwrap_or(&self.sym)
                    .get(&base)
                    .cloned()
                    .unwrap_or_else(|| (0..w).map(|i| self.flat2net[base + i]).collect());
                match range {
                    Range::Bit(b) => vec![sym[*b]],
                    Range::Span { msb, lsb } => sym[*lsb..=*msb].to_vec(),
                }
            }
            Expr::Index { e, msb, lsb } => {
                let v = self.blast_nat(e)?;
                let hi = (*msb + 1).min(v.len());
                let lo = (*lsb).min(v.len());
                let mut out = if lo < hi { v[lo..hi].to_vec() } else { vec![] };
                while out.len() < msb - lsb + 1 {
                    out.push(self.const_net(LogicValue::Zero));
                }
                out
            }
            Expr::Un { op, a } => {
                let av = self.blast_nat(a)?;
                match op {
                    UnOp::Not => {
                        let mut out = Vec::with_capacity(av.len());
                        for b in av {
                            if let Some(c) = self.const_val(b) {
                                out.push(self.const_net(c.not()));
                            } else {
                                out.push(self.gate1("not", CellKind::Not, b));
                            }
                        }
                        out
                    }
                    UnOp::AndR => vec![self.reduce(&av, CellKind::And, LogicValue::One)],
                    UnOp::OrR => vec![self.reduce(&av, CellKind::Or, LogicValue::Zero)],
                    UnOp::XorR => vec![self.reduce(&av, CellKind::Xor, LogicValue::Zero)],
                }
            }
            Expr::Bin { op, a, b } => self.blast_bin(*op, a, b)?,
            Expr::Cond { c, t, f } => {
                let w = t.width().max(f.width());
                let cv = self.blast(c, 1)?;
                let tv = self.blast(t, w)?;
                let fv = self.blast(f, w)?;
                let mut out = Vec::with_capacity(w);
                for i in 0..w {
                    out.push(self.mux(cv[0], tv[i], fv[i]));
                }
                out
            }
            Expr::Concat(parts) => {
                let mut msb_first: Vec<NetId> = Vec::new();
                for p in parts {
                    let v = self.blast_nat(p)?;
                    msb_first.extend(v.iter().rev());
                }
                msb_first.reverse();
                msb_first
            }
            Expr::Repeat { times, e } => {
                let v = self.blast_nat(e)?;
                let mut out: Vec<NetId> = Vec::new();
                for _ in 0..*times {
                    out.extend(v.iter().rev());
                }
                out.reverse();
                out
            }
            Expr::Signed { e, .. } => self.blast_nat(e)?,
            Expr::MemoryRead {
                memory, indexes, ..
            } => self.blast_distributed_rom(*memory, indexes)?,
            Expr::MemoryReadPart { .. } => {
                return Err(
                    "first-class sparse-memory part-select reads are not synthesizable yet".into(),
                )
            }
        })
    }

    fn blast_distributed_rom(
        &mut self,
        memory_index: usize,
        indexes: &[Expr],
    ) -> Result<Vec<NetId>, String> {
        let memory = self
            .design
            .memories
            .get(memory_index)
            .ok_or_else(|| format!("memory read refers to missing memory {memory_index}"))?
            .clone();
        let total_bits =
            u128::from(memory.flat_length) * u128::try_from(memory.element_width).unwrap_or(0);
        if memory.kind == MemoryKind::Ram {
            if macro_candidate(memory.template, total_bits) != MemoryMacroCandidate::DistributedRam
            {
                return Err(format!(
                    "memory '{}' requires a writable block macro, which is not implemented",
                    memory.name
                ));
            }
            let Some(state) = self.ram_state.get(&memory_index).cloned() else {
                return Err(format!(
                    "memory '{}' has no lowered distributed-RAM state",
                    memory.name
                ));
            };
            let mut output = (0..memory.element_width)
                .map(|_| self.const_net(LogicValue::X))
                .collect::<Vec<_>>();
            for (physical_index, value) in state.iter().enumerate() {
                let select = self.memory_address_select(&memory, physical_index as u64, indexes)?;
                for (bit, new_value) in output.iter_mut().zip(value.iter()) {
                    *bit = self.mux(select, *new_value, *bit);
                }
            }
            return Ok(output);
        }
        if macro_candidate(memory.template, total_bits) == MemoryMacroCandidate::BlockRom {
            if indexes.len() != 1 {
                return Err(format!(
                    "memory '{}' requires exactly one BlockRom address index, got {}",
                    memory.name,
                    indexes.len()
                ));
            }
            if memory.dimensions.len() != 1
                || memory.dimensions[0].base != 0
                || memory.dimensions[0].step != 1
                || memory.dimensions[0].length as u64 != memory.flat_length
            {
                return Err(format!(
                    "memory '{}' uses non-normalized indexing and cannot yet map to one BlockRom address port",
                    memory.name
                ));
            }
            let flat_length = usize::try_from(memory.flat_length).map_err(|_| {
                format!(
                    "memory '{}' is too large for the current BlockRom model",
                    memory.name
                )
            })?;
            if flat_length == 0 {
                return Err(format!(
                    "memory '{}' has an empty BlockRom address space",
                    memory.name
                ));
            }
            let contents = self
                .rom_contents
                .get(&memory_index)
                .cloned()
                .ok_or_else(|| format!("memory '{}' has no ROM contents", memory.name))?;
            if !contents.deterministic {
                return Err(format!(
                    "memory '{}' has conditional ROM initialization and cannot map to BlockRom",
                    memory.name
                ));
            }
            let mut words = vec![LogicWord::xes(memory.element_width); memory.flat_length as usize];
            for (logical, word) in contents.values {
                if logical.len() != 1 {
                    return Err(format!(
                        "memory '{}' has invalid BlockRom index rank {}",
                        memory.name,
                        logical.len()
                    ));
                }
                let Some(logical_index) = logical.first().copied() else {
                    return Err(format!("memory '{}' has invalid ROM index", memory.name));
                };
                let Some(physical) = memory.physical_index(&[logical_index]) else {
                    return Err(format!(
                        "memory '{}' has out-of-range ROM index",
                        memory.name
                    ));
                };
                words[physical as usize] = word;
            }
            let contents = std::sync::Arc::new(words);
            let address_width = flat_length.max(2).next_power_of_two().trailing_zeros() as usize;
            if address_width == 0 || address_width > u32::BITS as usize {
                return Err(format!(
                    "memory '{}' has an invalid BlockRom address width",
                    memory.name
                ));
            }
            let mut address = Vec::with_capacity(address_width);
            for index in indexes {
                address.extend(self.blast(index, address_width)?);
            }
            if address.len() != address_width {
                return Err(format!(
                    "memory '{}' BlockRom address width mismatch",
                    memory.name
                ));
            }
            let mut output = Vec::with_capacity(memory.element_width);
            for bit_index in 0..memory.element_width {
                let out = self.new_tmp("block_rom_q");
                self.add_cell(
                    "block_rom",
                    CellKind::BlockRom {
                        address_width,
                        bit_index,
                        contents: contents.clone(),
                    },
                    address.clone(),
                    out,
                );
                output.push(out);
            }
            self.lowered_memories.insert(memory_index);
            return Ok(output);
        }
        if macro_candidate(memory.template, total_bits) != MemoryMacroCandidate::DistributedRom {
            return Err(format!(
                "memory '{}' requires block macro synthesis, which is not implemented",
                memory.name
            ));
        }
        let contents = self
            .rom_contents
            .get(&memory_index)
            .ok_or_else(|| format!("memory '{}' has no deterministic ROM contents", memory.name))?;
        if !contents.deterministic {
            return Err(format!(
                "memory '{}' has conditional or unsupported initialization and cannot lower to distributed ROM",
                memory.name
            ));
        }
        let contents = contents.values.clone();

        self.lowered_memories.insert(memory_index);
        let mut output = (0..memory.element_width)
            .map(|_| self.const_net(LogicValue::X))
            .collect::<Vec<_>>();
        for physical_index in 0..memory.flat_length {
            let logical = physical_to_logical(&memory, physical_index)
                .ok_or_else(|| format!("memory '{}' has invalid banking geometry", memory.name))?;
            let select = self.memory_address_select(&memory, physical_index, indexes)?;
            let value = contents
                .get(&logical)
                .cloned()
                .unwrap_or_else(|| LogicWord::xes(memory.element_width));
            for (bit, value_bit) in output.iter_mut().zip(value.bits.iter()) {
                let constant = self.const_net(*value_bit);
                *bit = self.mux(select, constant, *bit);
            }
        }
        Ok(output)
    }

    fn memory_address_select(
        &mut self,
        memory: &eda_core::ir::MemoryDecl,
        physical_index: u64,
        indexes: &[Expr],
    ) -> Result<NetId, String> {
        let logical = physical_to_logical(memory, physical_index)
            .ok_or_else(|| format!("memory '{}' has invalid banking geometry", memory.name))?;
        let mut select = None;
        for (index, logical_index) in indexes.iter().zip(logical.iter()) {
            let width = index.width().max(1);
            let target = Expr::Const(LogicWord::from_u64(width, *logical_index as u64));
            let equal = self.blast_bin(BinOp::Eq, index, &target)?;
            let Some(equal) = equal.first() else {
                return Err(format!(
                    "memory '{}' address comparison produced no select bit",
                    memory.name
                ));
            };
            select = Some(match select {
                Some(previous) => self.combine(CellKind::And, previous, *equal),
                None => *equal,
            });
        }
        select.ok_or_else(|| format!("memory '{}' read has no index", memory.name))
    }

    fn decl_of_flat(&self, flat: usize) -> Result<&eda_core::ir::NetDecl, String> {
        self.design
            .net_decls
            .iter()
            .find(|d| flat >= d.base && flat < d.base + d.width)
            .ok_or_else(|| format!("flat net {} not in any declaration", flat))
    }

    fn mux(&mut self, s: NetId, t: NetId, f: NetId) -> NetId {
        if t == f {
            return t;
        }
        if let Some(sv) = self.const_val(s) {
            if let Some(b) = sv.as_bool() {
                return if b { t } else { f };
            }
        }
        if let (Some(tv), Some(fv)) = (self.const_val(t), self.const_val(f)) {
            if tv == LogicValue::One && fv == LogicValue::Zero {
                return s;
            }
            if tv == LogicValue::Zero && fv == LogicValue::One {
                return self.gate1("not", CellKind::Not, s);
            }
        }
        // CellKind::Mux is [sel, a, b] => sel ? b : a
        self.gate3("mux", CellKind::Mux, s, f, t)
    }

    fn reduce(&mut self, bits: &[NetId], kind: CellKind, unit: LogicValue) -> NetId {
        let mut acc = self.const_net(unit);
        for b in bits {
            acc = self.combine(kind.clone(), acc, *b);
        }
        acc
    }

    fn combine(&mut self, kind: CellKind, a: NetId, b: NetId) -> NetId {
        let (ca, cb) = (self.const_val(a), self.const_val(b));
        if let (Some(x), Some(y)) = (ca, cb) {
            return self.const_net(kind.eval(&[x, y]));
        }
        let use_x = ca.map(|v| v.as_bool());
        let use_y = cb.map(|v| v.as_bool());
        if let Some(Some(v)) = use_x {
            match (&kind, v) {
                (CellKind::And, false) => return self.const_net(LogicValue::Zero),
                (CellKind::And, true) => return b,
                (CellKind::Or, true) => return self.const_net(LogicValue::One),
                (CellKind::Or, false) => return b,
                (CellKind::Xor, false) => return b,
                (CellKind::Xor, true) => return self.gate1("not", CellKind::Not, b),
                (CellKind::Nand, false) => return self.const_net(LogicValue::One),
                (CellKind::Nand, true) => return self.gate1("not", CellKind::Not, b),
                (CellKind::Nor, true) => return self.const_net(LogicValue::Zero),
                (CellKind::Nor, false) => return self.gate1("not", CellKind::Not, b),
                (CellKind::Xnor, false) => return self.gate1("not", CellKind::Not, b),
                (CellKind::Xnor, true) => return b,
                _ => {}
            }
        }
        if let Some(Some(v)) = use_y {
            match (&kind, v) {
                (CellKind::And, false) => return self.const_net(LogicValue::Zero),
                (CellKind::And, true) => return a,
                (CellKind::Or, true) => return self.const_net(LogicValue::One),
                (CellKind::Or, false) => return a,
                (CellKind::Xor, false) => return a,
                (CellKind::Xor, true) => return self.gate1("not", CellKind::Not, a),
                _ => {}
            }
        }
        if a == b {
            match &kind {
                CellKind::And | CellKind::Or => return a,
                CellKind::Xor => return self.const_net(LogicValue::Zero),
                CellKind::Xnor => return self.const_net(LogicValue::One),
                CellKind::Nand | CellKind::Nor => return self.gate1("not", CellKind::Not, a),
                _ => {}
            }
        }
        let out = self.new_tmp("g");
        self.add_cell("g", kind, vec![a, b], out);
        out
    }

    fn blast_bin(&mut self, op: BinOp, a: &Expr, b: &Expr) -> Result<Vec<NetId>, String> {
        let (wa, wb) = (a.width(), b.width());
        match op {
            BinOp::Div | BinOp::Mod => {
                Err("synthesis does not support division/modulo operators".into())
            }
            BinOp::And | BinOp::Or | BinOp::Xor => {
                let w = wa.max(wb);
                let av = self.blast(a, w)?;
                let bv = self.blast(b, w)?;
                let kind = match op {
                    BinOp::And => CellKind::And,
                    BinOp::Or => CellKind::Or,
                    _ => CellKind::Xor,
                };
                let mut out = Vec::with_capacity(w);
                for i in 0..w {
                    out.push(self.combine(kind.clone(), av[i], bv[i]));
                }
                Ok(out)
            }
            BinOp::Add | BinOp::Sub => {
                let w = wa.max(wb);
                let av = self.blast(a, w)?;
                let bv = self.blast(b, w)?;
                let cin = if op == BinOp::Sub {
                    let comp: Vec<NetId> = bv.iter().map(|n| self.not(*n)).collect();
                    let one = self.const_net(LogicValue::One);
                    let (sum, _) = self.adder(&av, &comp, one);
                    return Ok(sum);
                } else {
                    self.const_net(LogicValue::Zero)
                };
                let (sum, _) = self.adder(&av, &bv, cin);
                Ok(sum)
            }
            BinOp::Mul => {
                let w = wa.max(wb);
                let av = self.blast(a, w)?;
                let bv = self.blast(b, w)?;
                let mut acc: Vec<NetId> =
                    (0..w).map(|_| self.const_net(LogicValue::Zero)).collect();
                for (i, bbit) in bv.iter().enumerate() {
                    if i >= w {
                        break;
                    }
                    let mut partial: Vec<NetId> = Vec::with_capacity(w);
                    for k in 0..w {
                        if k >= i {
                            partial.push(self.combine(CellKind::And, av[k - i], *bbit));
                        } else {
                            partial.push(self.const_net(LogicValue::Zero));
                        }
                    }
                    let zero = self.const_net(LogicValue::Zero);
                    let (sum, _) = self.adder(&acc, &partial, zero);
                    acc = sum;
                }
                Ok(acc)
            }
            BinOp::Shl | BinOp::Shr => {
                let av = self.blast(a, wa)?;
                let bbits = b.width().min(6);
                let bv = self.blast(b, bbits)?;
                let right = op == BinOp::Shr;
                let sign = if right && a.is_signed() {
                    Some(av[av.len() - 1])
                } else {
                    None
                };
                let mut cur = av;
                for (k, bit) in bv.iter().enumerate() {
                    let sh = 1usize << k;
                    let zero = self.const_net(LogicValue::Zero);
                    let fill = sign.unwrap_or(zero);
                    let mut shifted = vec![zero; cur.len()];
                    for i in 0..cur.len() {
                        if right {
                            if i + sh < cur.len() {
                                shifted[i] = cur[i + sh];
                            } else {
                                shifted[i] = fill;
                            }
                        } else if i >= sh {
                            shifted[i] = cur[i - sh];
                        }
                    }
                    let mut next = Vec::with_capacity(cur.len());
                    for i in 0..cur.len() {
                        next.push(self.mux(*bit, shifted[i], cur[i]));
                    }
                    cur = next;
                }
                Ok(cur)
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let w = wa.max(wb);
                let av = self.blast(a, w)?;
                let bv = self.blast(b, w)?;
                let eq = self.eq_net(&av, &bv);
                let signed_compare = a.is_signed() && b.is_signed();
                let net = match op {
                    BinOp::Eq => eq,
                    BinOp::Ne => self.not(eq),
                    BinOp::Lt => {
                        if signed_compare {
                            self.lt_signed_net(&av, &bv)
                        } else {
                            self.lt_net(&av, &bv)
                        }
                    }
                    BinOp::Le => {
                        let lt = if signed_compare {
                            self.lt_signed_net(&av, &bv)
                        } else {
                            self.lt_net(&av, &bv)
                        };
                        self.or(eq, lt)
                    }
                    BinOp::Gt => {
                        let lt = if signed_compare {
                            self.lt_signed_net(&av, &bv)
                        } else {
                            self.lt_net(&av, &bv)
                        };
                        let le = self.or(eq, lt);
                        self.not(le)
                    }
                    BinOp::Ge => {
                        let lt = if signed_compare {
                            self.lt_signed_net(&av, &bv)
                        } else {
                            self.lt_net(&av, &bv)
                        };
                        self.not(lt)
                    }
                    _ => unreachable!(),
                };
                Ok(vec![net])
            }
            BinOp::CaseEq | BinOp::CaseNe => {
                let w = wa.max(wb);
                let av = self.blast(a, w)?;
                let bv = self.blast(b, w)?;
                let one = self.const_net(LogicValue::One);
                let mut acc = one;
                for i in 0..w {
                    let xnor = self.combine(CellKind::Xnor, av[i], bv[i]);
                    acc = self.combine(CellKind::And, acc, xnor);
                }
                if op == BinOp::CaseNe {
                    Ok(vec![self.not(acc)])
                } else {
                    Ok(vec![acc])
                }
            }
            BinOp::LogAnd | BinOp::LogOr => {
                let av = self.blast(a, 1)?;
                let bv = self.blast(b, 1)?;
                let kind = if op == BinOp::LogAnd {
                    CellKind::And
                } else {
                    CellKind::Or
                };
                Ok(vec![self.combine(kind, av[0], bv[0])])
            }
        }
    }

    fn not(&mut self, n: NetId) -> NetId {
        if let Some(c) = self.const_val(n) {
            self.const_net(c.not())
        } else {
            self.gate1("not", CellKind::Not, n)
        }
    }

    fn or(&mut self, a: NetId, b: NetId) -> NetId {
        self.combine(CellKind::Or, a, b)
    }

    fn eq_net(&mut self, av: &[NetId], bv: &[NetId]) -> NetId {
        let one = self.const_net(LogicValue::One);
        let mut acc = one;
        for i in 0..av.len() {
            let x = self.combine(CellKind::Xnor, av[i], bv[i]);
            acc = self.combine(CellKind::And, acc, x);
        }
        acc
    }

    fn lt_net(&mut self, av: &[NetId], bv: &[NetId]) -> NetId {
        let comp: Vec<NetId> = bv.iter().map(|n| self.not(*n)).collect();
        let one = self.const_net(LogicValue::One);
        let (_, carry) = self.adder(av, &comp, one);
        self.not(carry)
    }

    fn lt_signed_net(&mut self, av: &[NetId], bv: &[NetId]) -> NetId {
        let ax: Vec<NetId> = av
            .iter()
            .enumerate()
            .map(|(i, n)| if i + 1 == av.len() { self.not(*n) } else { *n })
            .collect();
        let bx: Vec<NetId> = bv
            .iter()
            .enumerate()
            .map(|(i, n)| if i + 1 == bv.len() { self.not(*n) } else { *n })
            .collect();
        self.lt_net(&ax, &bx)
    }

    fn adder(&mut self, av: &[NetId], bv: &[NetId], cin: NetId) -> (Vec<NetId>, NetId) {
        let w = av.len().max(bv.len());
        let mut sum = Vec::with_capacity(w);
        let mut carry = cin;
        for i in 0..w {
            let a = if i < av.len() {
                av[i]
            } else {
                self.const_net(LogicValue::Zero)
            };
            let b = if i < bv.len() {
                bv[i]
            } else {
                self.const_net(LogicValue::Zero)
            };
            let ax = self.combine(CellKind::Xor, a, b);
            let s = self.combine(CellKind::Xor, ax, carry);
            let ab = self.combine(CellKind::And, a, b);
            let xc = self.combine(CellKind::And, ax, carry);
            carry = self.or(ab, xc);
            sum.push(s);
        }
        (sum, carry)
    }

    // ---------- statements ----------

    fn hook_assign(&mut self, lv: &LValue, val: &[NetId]) -> Result<(), String> {
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
        let base = lv.net;
        let w = self.decl_width(base);
        if !self.sym.contains_key(&base) {
            let nets: Vec<NetId> = (0..w).map(|i| self.flat2net[base + i]).collect();
            self.sym.insert(base, nets);
        }
        let mut cur = self.sym[&base].clone();
        let (lo, n) = match &lv.range {
            Range::Bit(b) => (*b, 1),
            Range::Span { msb, lsb } => (*lsb, msb - lsb + 1),
        };
        cur[lo..lo + n].copy_from_slice(&val[..n]);
        self.sym.insert(base, cur);
    }

    fn base_nets(&mut self, base: usize) -> Vec<NetId> {
        let w = self.decl_width(base);
        (0..w).map(|i| self.flat2net[base + i]).collect()
    }

    fn exec_stmt(&mut self, s: &Stmt) -> Result<(), String> {
        match s {
            Stmt::Block(v) => {
                for x in v {
                    self.exec_stmt(x)?;
                }
                Ok(())
            }
            Stmt::Forever(_) => Err("forever is not synthesizable".into()),
            Stmt::Bl { lv, e } | Stmt::NBl { lv, e } => {
                let w = lv.range.width();
                let v = self.blast(e, w)?;
                self.sym_set(lv, &v);
                self.written.insert(lv.net);
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
                        let index = if *reversed {
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

                for ((eq, arm_state), arm_written) in
                    arm_states.into_iter().zip(arm_written_sets).rev()
                {
                    for k in sorted_set(&arm_written) {
                        let cur = result.get(&k).cloned().unwrap_or_else(|| self.base_nets(k));
                        let arm_v = arm_state
                            .get(&k)
                            .cloned()
                            .unwrap_or_else(|| self.base_nets(k));
                        let mut merged = Vec::with_capacity(cur.len());
                        for i in 0..cur.len() {
                            merged.push(self.mux(eq, arm_v[i], cur[i]));
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
            Stmt::MemoryBlPart { .. } | Stmt::MemoryNBlPart { .. } => {
                Err("first-class sparse-memory part-select writes are not synthesizable yet".into())
            }
            Stmt::Display { .. }
            | Stmt::Finish
            | Stmt::WriteMem { .. }
            | Stmt::Assert { .. }
            | Stmt::Assume { .. }
            | Stmt::Cover { .. } => Ok(()),
        }
    }

    fn lower_seq(
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
mod tests {
    use super::*;
    use eda_core::logic::LogicWord;
    use eda_hdl::compile;
    use eda_sim::Simulator;

    fn synth(src: &str) -> SynthResult {
        let d = compile(src, None).unwrap();
        synthesize(&d, &SynthOptions::default()).unwrap_or_else(|e| panic!("synth failed: {}", e))
    }

    #[test]
    fn sparse_memory_macro_plans_preserve_timing_and_write_contracts() {
        let rom = compile(
            r#"
module rom(input clk, input [11:0] address, output reg [7:0] q);
  logic [7:0] memory [0:4095];
  initial memory[0] = 8'h1;
  always @(posedge clk) q <= memory[address];
endmodule
"#,
            None,
        )
        .unwrap();
        let rom_plans = plan_memories(&rom);
        assert_eq!(rom_plans.len(), 1);
        assert_eq!(rom_plans[0].candidate, MemoryMacroCandidate::BlockRom);
        assert_eq!(rom_plans[0].bank_count, 1);
        assert_eq!(rom_plans[0].bank_depth, 4096);
        assert_eq!(rom_plans[0].write_mask, None);
        assert_eq!(rom_plans[0].byte_enable, None);

        let ram = compile(
            r#"
module ram(
  input logic clk,
  input logic write,
  input logic [11:0] address,
  input logic [7:0] din,
  output logic [7:0] q
);
  logic [7:0] memory [0:4095];
  always @(posedge clk) if (write) memory[address] <= din;
  assign q = memory[address];
endmodule
"#,
            None,
        )
        .unwrap();
        let ram_plans = plan_memories(&ram);
        assert_eq!(ram_plans.len(), 1);
        assert_eq!(ram_plans[0].candidate, MemoryMacroCandidate::BlockRam);
        assert_eq!(ram_plans[0].bank_count, 1);
        assert_eq!(ram_plans[0].bank_depth, 4096);
        assert_eq!(ram_plans[0].total_bits, 32768);
        assert!(ram_plans[0]
            .write_mask
            .as_ref()
            .expect("RAM write mask")
            .is_full());
        assert!(ram_plans[0]
            .byte_enable
            .as_ref()
            .expect("full byte-enable mask")
            .is_full());
    }

    #[test]
    fn distributed_rom_synthesizes_and_matches_direct_ast_semantics() {
        let Some(slang) = eda_slang::SlangFrontend::discover_optional() else {
            return;
        };
        let source_path = std::env::temp_dir().join(format!(
            "eda-synth-distributed-rom-{}-{}.sv",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &source_path,
            r#"
module rom(input logic [1:0] address, output logic [7:0] q);
  logic [7:0] memory [0:3];
  initial begin
    memory[0] = 8'h12;
    memory[1] = 8'h34;
    memory[2] = 8'h56;
    memory[3] = 8'h78;
  end
  assign q = memory[address];
endmodule
"#,
        )
        .expect("write distributed ROM source");
        let rtl = slang
            .compile_files_via_ast(
                std::slice::from_ref(&source_path),
                Some("rom"),
                &eda_slang::PreprocessOptions::default(),
            )
            .expect("directly lower distributed ROM");
        let result = synthesize(&rtl, &SynthOptions::default())
            .expect("lower distributed ROM to combinational logic");
        assert_eq!(result.memory_plans.len(), 1);
        assert_eq!(
            result.memory_plans[0].candidate,
            MemoryMacroCandidate::DistributedRom
        );
        assert!(!result
            .warnings
            .iter()
            .any(|warning| warning.contains("no synthesis lowering")));

        let gate = eda_core::to_design::netlist_to_design(&result.netlist);
        let mut rtl_sim = Simulator::new(&rtl).unwrap();
        let mut gate_sim = Simulator::new(&gate).unwrap();
        for address in 0..4_u64 {
            rtl_sim
                .set_input("address", &LogicWord::from_u64(2, address))
                .unwrap();
            gate_sim
                .set_input("address", &LogicWord::from_u64(2, address))
                .unwrap();
            assert_eq!(
                rtl_sim.read("q").unwrap(),
                gate_sim.read("q").unwrap(),
                "address {address}"
            );
        }
    }

    #[test]
    fn block_rom_preserves_one_macro_and_matches_direct_ast_semantics() {
        let Some(slang) = eda_slang::SlangFrontend::discover_optional() else {
            return;
        };
        let unique = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let source_path = std::env::temp_dir().join(format!("eda-synth-block-rom-{unique}.sv"));
        let image_path = std::env::temp_dir().join(format!("eda-synth-block-rom-{unique}.hex"));
        let mut image = String::new();
        for address in 0..4096_u64 {
            let value = (address.wrapping_mul(7).wrapping_add(13) & 0xff) as u8;
            image.push_str(&format!("{value:02x}\n"));
        }
        std::fs::write(&image_path, image).expect("write BlockRom image");
        std::fs::write(
            &source_path,
            format!(
                r#"
module block_rom(
  input logic clk,
  input logic [11:0] address,
  output logic [7:0] q
);
  logic [7:0] memory [0:4095];
  initial $readmemh("{}", memory);
  always @(posedge clk) q <= memory[address];
endmodule
"#,
                image_path.display()
            ),
        )
        .expect("write BlockRom source");

        let rtl = slang
            .compile_files_via_ast(
                std::slice::from_ref(&source_path),
                Some("block_rom"),
                &eda_slang::PreprocessOptions::default(),
            )
            .expect("directly lower BlockRom source");
        let result = synthesize(&rtl, &SynthOptions::default())
            .expect("lower large synchronous ROM to one BlockRom macro");
        assert_eq!(result.memory_plans.len(), 1);
        assert_eq!(
            result.memory_plans[0].candidate,
            MemoryMacroCandidate::BlockRom
        );
        assert!(!result
            .warnings
            .iter()
            .any(|warning| warning.contains("no synthesis lowering")));

        let stats = result.netlist.stats();
        assert_eq!(stats.memory_macros, 1);
        assert_eq!(
            result
                .netlist
                .cells()
                .iter()
                .filter(|cell| matches!(cell.kind, CellKind::BlockRom { .. }))
                .count(),
            8
        );

        let gate = eda_core::to_design::netlist_to_design(&result.netlist);
        assert_eq!(gate.memories.len(), 1);
        assert_eq!(gate.memories[0].flat_length, 4096);
        assert_eq!(gate.memories[0].kind, MemoryKind::Rom);

        let mut rtl_sim = Simulator::new(&rtl).unwrap();
        let mut gate_sim = Simulator::new(&gate).unwrap();
        for address in [0_u64, 1, 255, 2048, 4095] {
            for simulator in [&mut rtl_sim, &mut gate_sim] {
                simulator
                    .set_input("clk", &LogicWord::from_u64(1, 0))
                    .unwrap();
                simulator
                    .set_input("address", &LogicWord::from_u64(12, address))
                    .unwrap();
                simulator
                    .set_input("clk", &LogicWord::from_u64(1, 1))
                    .unwrap();
            }
            assert_eq!(
                rtl_sim.read("q").unwrap(),
                gate_sim.read("q").unwrap(),
                "address {address}"
            );
            assert_eq!(
                gate_sim.read("q").unwrap().to_u64(),
                Some(address.wrapping_mul(7).wrapping_add(13) & 0xff),
                "address {address}"
            );
        }

        let verilog = crate::writer::write_verilog(&result.netlist, result.library.as_ref());
        assert_eq!(
            verilog
                .matches("  reg [7:0] block_rom_0 [0:4095];\n")
                .count(),
            1
        );
        assert_eq!(verilog.matches("block_rom_1").count(), 0);

        let _ = std::fs::remove_file(&source_path);
        let _ = std::fs::remove_file(&image_path);
    }

    #[test]
    fn distributed_ram_synthesizes_and_matches_direct_ast_semantics() {
        let Some(slang) = eda_slang::SlangFrontend::discover_optional() else {
            return;
        };
        let source_path = std::env::temp_dir().join(format!(
            "eda-synth-distributed-ram-{}-{}.sv",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &source_path,
            r#"
module distributed_ram(
  input logic clk,
  input logic write,
  input logic [1:0] address,
  input logic [7:0] din,
  output logic [7:0] q
);
  logic [7:0] memory [0:3];
  always @(posedge clk) if (write) memory[address] <= din;
  assign q = memory[address];
endmodule
"#,
        )
        .expect("write distributed RAM source");
        let rtl = slang
            .compile_files_via_ast(
                std::slice::from_ref(&source_path),
                Some("distributed_ram"),
                &eda_slang::PreprocessOptions::default(),
            )
            .expect("directly lower distributed RAM");
        let result = synthesize(&rtl, &SynthOptions::default())
            .expect("lower distributed RAM to registers and multiplexers");
        assert_eq!(result.memory_plans.len(), 1);
        assert_eq!(
            result.memory_plans[0].candidate,
            MemoryMacroCandidate::DistributedRam
        );
        assert!(!result
            .warnings
            .iter()
            .any(|warning| warning.contains("no synthesis lowering")));
        assert_eq!(result.netlist.stats().seq, 32);

        let gate = eda_core::to_design::netlist_to_design(&result.netlist);
        let mut rtl_sim = Simulator::new(&rtl).unwrap();
        let mut gate_sim = Simulator::new(&gate).unwrap();
        for sim in [&mut rtl_sim, &mut gate_sim] {
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            sim.set_input("write", &LogicWord::from_u64(1, 1)).unwrap();
        }
        for (address, value) in [(0_u64, 0x12), (1, 0x34), (2, 0x56), (3, 0x78)] {
            for sim in [&mut rtl_sim, &mut gate_sim] {
                sim.set_input("address", &LogicWord::from_u64(2, address))
                    .unwrap();
                sim.set_input("din", &LogicWord::from_u64(8, value))
                    .unwrap();
                sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
                sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            }
            assert_eq!(
                rtl_sim.read("q").unwrap(),
                gate_sim.read("q").unwrap(),
                "write address {address}"
            );
        }
        for sim in [&mut rtl_sim, &mut gate_sim] {
            sim.set_input("write", &LogicWord::from_u64(1, 0)).unwrap();
        }
        for address in 0..4_u64 {
            for sim in [&mut rtl_sim, &mut gate_sim] {
                sim.set_input("address", &LogicWord::from_u64(2, address))
                    .unwrap();
            }
            assert_eq!(
                rtl_sim.read("q").unwrap(),
                gate_sim.read("q").unwrap(),
                "read address {address}"
            );
        }
    }

    #[test]
    fn distributed_ram_byte_enable_write_preserves_other_lanes() {
        let Some(slang) = eda_slang::SlangFrontend::discover_optional() else {
            return;
        };
        let source_path = std::env::temp_dir().join(format!(
            "eda-synth-byte-enable-ram-{}-{}.sv",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &source_path,
            r#"
module byte_enable_ram(
  input logic clk,
  input logic write,
  input logic [1:0] address,
  input logic [7:0] din,
  output logic [31:0] q
);
  logic [31:0] memory [0:3];
  always @(posedge clk) begin
    if (write) memory[address][23:16] <= din;
  end
  assign q = memory[address];
endmodule
"#,
        )
        .expect("write byte-enable RAM source");
        let rtl = slang
            .compile_files_via_ast(
                std::slice::from_ref(&source_path),
                Some("byte_enable_ram"),
                &eda_slang::PreprocessOptions::default(),
            )
            .expect("directly lower byte-enable RAM");
        assert_eq!(rtl.memories[0].write_mask.as_ref().unwrap().bits.len(), 32);
        assert_eq!(
            rtl.memories[0].byte_enable.as_ref().unwrap().lanes,
            [false, false, true, false].to_vec()
        );
        let result =
            synthesize(&rtl, &SynthOptions::default()).expect("lower byte-enable distributed RAM");
        assert_eq!(result.netlist.stats().seq, 128);

        let gate = eda_core::to_design::netlist_to_design(&result.netlist);
        let mut rtl_sim = Simulator::new(&rtl).unwrap();
        let mut gate_sim = Simulator::new(&gate).unwrap();
        for sim in [&mut rtl_sim, &mut gate_sim] {
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            sim.set_input("write", &LogicWord::from_u64(1, 1)).unwrap();
        }
        for (address, lane_value) in [(0_u64, 0x12), (1, 0x34), (2, 0x56), (3, 0x78)] {
            for sim in [&mut rtl_sim, &mut gate_sim] {
                sim.set_input("address", &LogicWord::from_u64(2, address))
                    .unwrap();
                sim.set_input("din", &LogicWord::from_u64(8, lane_value))
                    .unwrap();
                sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
                sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            }
            let rtl_q = rtl_sim.read("q").unwrap();
            let gate_q = gate_sim.read("q").unwrap();
            assert_eq!(rtl_q, gate_q, "write address {address}");
            assert!(gate_q.to_hex_str().len() >= 8);
            assert_eq!(
                (0..8).map(|bit| gate_q.bit(16 + bit)).collect::<Vec<_>>(),
                {
                    let expected = LogicWord::from_u64(8, lane_value);
                    (0..8).map(|bit| expected.bit(bit)).collect::<Vec<_>>()
                }
            );
        }
    }

    #[test]
    fn memory_macro_thresholds_separate_distributed_and_block_candidates() {
        assert_eq!(
            macro_candidate(MemoryTemplate::SyncRom, 2048),
            MemoryMacroCandidate::DistributedRom
        );
        assert_eq!(
            macro_candidate(MemoryTemplate::SyncRom, 2049),
            MemoryMacroCandidate::BlockRom
        );
        assert_eq!(
            macro_candidate(MemoryTemplate::SyncRam, 4096),
            MemoryMacroCandidate::DistributedRam
        );
        assert_eq!(
            macro_candidate(MemoryTemplate::SyncRam, 4097),
            MemoryMacroCandidate::BlockRam
        );
        assert_eq!(
            macro_candidate(MemoryTemplate::AsyncRom, 2049),
            MemoryMacroCandidate::UnsupportedAsyncLargeMemory
        );
        assert_eq!(
            macro_candidate(MemoryTemplate::MixedRam, 1),
            MemoryMacroCandidate::PartitionRequired
        );
    }

    #[test]
    fn exhaustive_case_without_default_does_not_create_a_false_loop() {
        let r = synth(
            r#"
                module m(input [2:0] sel, input [1:0] d, output reg [1:0] y);
                    always @(*) begin
                        case (sel)
                            3'd0: y = d;
                            3'd1: y = ~d;
                            3'd2: y = d & 2'b10;
                            3'd3: y = d | 2'b01;
                            3'd4: y = d ^ 2'b11;
                            3'd5: y = {d[0], d[1]};
                            3'd6: y = d + 2'b01;
                            3'd7: y = 2'b00;
                        endcase
                    end
                endmodule
            "#,
        );
        assert!(
            r.netlist.topo_cells().is_some(),
            "an exhaustive exact case must not feed its own output back to itself"
        );
        assert!(r.memory_plans.is_empty());
    }

    #[test]
    fn incomplete_case_without_default_still_preserves_the_old_value() {
        let r = synth(
            r#"
                module m(input [1:0] sel, input d, output reg y);
                    always @(*) begin
                        case (sel)
                            2'd0: y = d;
                            2'd1: y = ~d;
                        endcase
                    end
                endmodule
            "#,
        );
        assert!(
            r.netlist.topo_cells().is_none(),
            "an incomplete combinational case intentionally models value retention"
        );
    }

    #[test]
    fn optimization_preserves_named_dead_state_for_sequential_audits() {
        let r = synth(
            r#"
                module m(input clk, d, output reg active);
                    reg hidden;
                    always @(posedge clk) begin
                        hidden <= ~d;
                        active <= d;
                    end
                endmodule
            "#,
        );
        assert!(r.netlist.net_by_name("hidden").is_some());
        assert_eq!(r.netlist.stats().seq, 2);
    }

    #[test]
    fn synthesis_is_deterministic_across_thread_hash_seeds() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        use std::thread;

        const SRC: &str = r#"
            module m(input clk, rst, a, b, sel, output reg [3:0] x, y,
                output reg [1:0] z, output n);
                always @(*) begin
                    x = 4'b0;
                    y = 4'b0;
                    z = 2'b0;
                    case (sel)
                        1'b0: begin
                            x = {a, b, a, b};
                            z = {b, a};
                        end
                        default: begin
                            y = {b, a, b, a};
                            z = {a, b};
                        end
                    endcase
                    if (a) x = x ^ 4'b0101;
                    if (b) y = y | 4'b1010;
                end
                assign n = ~((a & b) | (a & b));
            endmodule
        "#;

        fn signature(nl: &Netlist) -> u64 {
            let mut hasher = DefaultHasher::new();
            for net_id in 0..nl.net_count() {
                nl.net(net_id).name.hash(&mut hasher);
            }
            for cell in nl.cells() {
                cell.name.hash(&mut hasher);
                format!("{:?}", cell.kind).hash(&mut hasher);
                for input in &cell.inputs {
                    nl.net(*input).name.hash(&mut hasher);
                }
                nl.net(cell.output).name.hash(&mut hasher);
            }
            for port in &nl.ports {
                port.name.hash(&mut hasher);
                format!("{:?}", port.dir).hash(&mut hasher);
                for bit in &port.bits {
                    nl.net(*bit).name.hash(&mut hasher);
                }
            }
            hasher.finish()
        }

        let workers = (0..4)
            .map(|_| {
                thread::spawn(|| {
                    let design = compile(SRC, None).unwrap();
                    let result = synthesize(&design, &SynthOptions::default()).unwrap();
                    signature(&result.netlist)
                })
            })
            .collect::<Vec<_>>();
        let signatures = workers
            .into_iter()
            .map(thread::JoinHandle::join)
            .map(|result| result.expect("synthesis worker panicked"))
            .collect::<Vec<_>>();
        assert!(
            signatures.windows(2).all(|pair| pair[0] == pair[1]),
            "synthesis changed with hasher/thread seed: {signatures:?}"
        );
    }

    #[test]
    fn duplicate_merging_preserves_a_stable_rtl_alias_name() {
        let mut nl = Netlist::new("aliases".to_string());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let temporary = nl.add_net("g_n1");
        let stable = nl.add_net("keep");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "and0".into(),
            kind: CellKind::And,
            inputs: vec![a, b],
            output: temporary,
        });
        nl.add_cell(Cell {
            name: "and1".into(),
            kind: CellKind::And,
            inputs: vec![a, b],
            output: stable,
        });
        nl.add_cell(Cell {
            name: "or".into(),
            kind: CellKind::Or,
            inputs: vec![temporary, stable],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);

        let mut warnings = Vec::new();
        opt::optimize(&mut nl, &mut warnings);
        assert!(nl.net_by_name("keep").is_some());
        assert!(nl.net_by_name("g_n1").is_none());
    }

    #[test]
    fn synthesis_preserves_hierarchy_metadata() {
        let src = "module leaf(input a, output y); assign y = ~a; endmodule
            module sub(input a, output y); leaf u_leaf(.a(a), .y(y)); endmodule
            module top(input a, output y); sub u1(.a(a), .y(y)); endmodule";
        let r = synth(src);
        assert_eq!(r.netlist.hierarchy.len(), 3);
        assert_eq!(
            r.netlist
                .hierarchy_instance("top.u1.u_leaf")
                .unwrap()
                .module,
            "leaf"
        );
        let leaf = r.netlist.hierarchy_instance("top.u1.u_leaf").unwrap();
        assert_eq!(leaf.parent.as_deref(), Some("top.u1"));
        assert_eq!(leaf.ports.len(), 2);
        let input = leaf
            .ports
            .iter()
            .find(|port| port.name == "a")
            .expect("leaf input retained");
        assert_eq!(input.direction, Dir::In);
        let net = input
            .nets
            .first()
            .copied()
            .expect("leaf input net retained");
        assert_eq!(
            r.netlist
                .owning_hierarchy_instance(net)
                .map(|instance| instance.path.as_str()),
            Some("top.u1.u_leaf")
        );
        assert!(r.netlist.hierarchy_report().contains("top.u1.u_leaf"));
    }

    #[test]
    fn simple_and_maps_to_and2() {
        let r = synth("module m(input a, b, output y); assign y = a & b; endmodule");
        assert_eq!(r.netlist.stats().lib_cells, 1);
        let c = &r.netlist.cells()[0];
        assert!(matches!(&c.kind, CellKind::Lib(id) if r.library.get(*id).name == "AND2"));
    }

    #[test]
    fn constant_folding_removes_logic() {
        let r = synth("module m(input a, output y); assign y = a & 1'b1; endmodule");
        assert_eq!(r.netlist.stats().lib_cells, 0);
        let y = r.netlist.port_by_name("y").unwrap();
        let a = r.netlist.port_by_name("a").unwrap();
        assert_eq!(y.bits, a.bits, "feedthrough should alias the source net");

        let r0 = synth("module m(input a, output y); assign y = a & 1'b0; endmodule");
        assert_eq!(r0.netlist.stats().lib_cells, 1);
        let c0 = &r0.netlist.cells()[0];
        assert!(matches!(&c0.kind, CellKind::Lib(id) if r0.library.get(*id).name == "TIE0"));
    }

    #[test]
    fn duplicate_logic_merged() {
        // port drivers stay separate, but the shared inner cone must merge
        let r = synth(
            "module m(input a, b, c, d, output y, z);
                wire w; assign w = a & b;
                assign y = w | c; assign z = w | d;
             endmodule",
        );
        let ands = r
            .netlist
            .cells()
            .iter()
            .filter(|c| matches!(&c.kind, CellKind::Lib(id) if r.library.get(*id).name == "AND2"))
            .count();
        assert_eq!(ands, 1);
    }

    #[test]
    fn dead_code_eliminated() {
        let r = synth(
            "module m(input a, b, output y); wire dead; assign dead = a ^ b; assign y = a; endmodule",
        );
        assert!(r.netlist.stats().cells <= 1); // y is a pass-through (0 or 1 BUF)
    }

    #[test]
    fn adder_synthesizes_and_matches_rtl() {
        let src = "module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule";
        let r = synth(src);
        let d = compile(src, None).unwrap();
        let gd = eda_core::to_design::netlist_to_design(&r.netlist);
        let mut rtl = Simulator::new(&d).unwrap();
        let mut gate = Simulator::new(&gd).unwrap();
        for x in 0..16u64 {
            for y in 0..16u64 {
                for sim in [&mut rtl, &mut gate] {
                    sim.set_input("a", &LogicWord::from_u64(4, x)).unwrap();
                    sim.set_input("b", &LogicWord::from_u64(4, y)).unwrap();
                }
                let rv = rtl.read_port("s").unwrap();
                let gv = gate.read_port("s").unwrap();
                assert_eq!(rv.to_u64(), gv.to_u64(), "{}+{}", x, y);
                assert_eq!(rv.to_u64(), Some((x + y) & 0xF));
            }
        }
        assert_eq!(r.netlist.stats().seq, 0);
    }

    #[test]
    fn flop_infers_dff() {
        let r =
            synth("module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule");
        assert_eq!(r.netlist.stats().seq, 1);
        let c = r
            .netlist
            .cells()
            .iter()
            .find(|c| matches!(c.kind, CellKind::Lib(_)))
            .unwrap();
        let name = match &c.kind {
            CellKind::Lib(id) => r.library.get(*id).name.clone(),
            _ => unreachable!(),
        };
        assert_eq!(name, "DFF");
    }

    #[test]
    fn runtime_indexed_nba_synthesizes_and_simulates() {
        let src = "module m(input clk, input logic [3:0] first, second, \
                          input logic [7:0] a, b, output logic [15:0] y); \
                    always @(posedge clk) begin \
                        y[first +: 8] <= a; \
                        y[second -: 8] <= b; \
                    end \
                  endmodule";
        let r = synth(src);
        assert_eq!(
            r.netlist.stats().seq,
            16,
            "stats={:?}, warnings={:?}",
            r.netlist.stats(),
            r.warnings
        );
        let d = compile(src, None).unwrap();
        let gd = eda_core::to_design::netlist_to_design(&r.netlist);
        let mut rtl = Simulator::new(&d).unwrap();
        let mut gate = Simulator::new(&gd).unwrap();
        for sim in [&mut rtl, &mut gate] {
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            sim.set_input("first", &LogicWord::from_u64(4, 0)).unwrap();
            sim.set_input("second", &LogicWord::from_u64(4, 15))
                .unwrap();
            sim.set_input("a", &LogicWord::from_u64(8, 0xa5)).unwrap();
            sim.set_input("b", &LogicWord::from_u64(8, 0x3c)).unwrap();
            sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        }
        let expected = Some(0x3ca5_u64);
        assert_eq!(rtl.read_port("y").unwrap().to_u64(), expected);
        assert_eq!(gate.read_port("y").unwrap().to_u64(), expected);
    }

    #[test]
    fn runtime_array_indexed_nba_synthesizes_and_simulates() {
        let src = "module m(input clk, input logic [2:0] element, \
                          input logic [1:0] packed_selector, input logic [1:0] value, \
                          output logic [3:0] memory [0:3]); \
                    always @(posedge clk) begin \
                        memory[0] <= 4'hA; \
                        memory[1] <= 4'h5; \
                        memory[2] <= 4'h3; \
                        memory[3] <= 4'hC; \
                        memory[element][packed_selector +: 2] <= value ^ 2'h3; \
                    end \
                  endmodule";
        let r = synth(src);
        assert_eq!(r.netlist.stats().seq, 16);
        let d = compile(src, None).unwrap();
        let gd = eda_core::to_design::netlist_to_design(&r.netlist);
        let mut rtl = Simulator::new(&d).unwrap();
        let mut gate = Simulator::new(&gd).unwrap();

        for sim in [&mut rtl, &mut gate] {
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            sim.set_input("element", &LogicWord::from_u64(3, 2))
                .unwrap();
            sim.set_input("packed_selector", &LogicWord::from_u64(2, 1))
                .unwrap();
            sim.set_input("value", &LogicWord::from_u64(2, 0b11))
                .unwrap();
            sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        }
        let expected = Some(0xC15A_u64);
        assert_eq!(rtl.read_port("memory").unwrap().to_u64(), expected);
        assert_eq!(gate.read_port("memory").unwrap().to_u64(), expected);

        for sim in [&mut rtl, &mut gate] {
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            sim.set_input("element", &LogicWord::from_u64(3, 4))
                .unwrap();
            sim.set_input("packed_selector", &LogicWord::from_u64(2, 0))
                .unwrap();
            sim.set_input("value", &LogicWord::from_u64(2, 0b01))
                .unwrap();
            sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        }
        let unchanged = Some(0xC35A_u64);
        assert_eq!(rtl.read_port("memory").unwrap().to_u64(), unchanged);
        assert_eq!(gate.read_port("memory").unwrap().to_u64(), unchanged);
    }

    #[test]
    fn reversed_runtime_indexed_nba_synthesizes_and_simulates() {
        let src = "module m(input clk, input logic [3:0] first, second, \
                          input logic [7:0] a, b, output logic [0:15] y); \
                    always @(posedge clk) begin \
                        y[first +: 8] <= a; \
                        y[second -: 8] <= b; \
                    end \
                  endmodule";
        let r = synth(src);
        assert_eq!(r.netlist.stats().seq, 16);
        let d = compile(src, None).unwrap();
        let gd = eda_core::to_design::netlist_to_design(&r.netlist);
        let mut rtl = Simulator::new(&d).unwrap();
        let mut gate = Simulator::new(&gd).unwrap();
        for sim in [&mut rtl, &mut gate] {
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            sim.set_input("first", &LogicWord::from_u64(4, 0)).unwrap();
            sim.set_input("second", &LogicWord::from_u64(4, 15))
                .unwrap();
            sim.set_input("a", &LogicWord::from_u64(8, 0xa5)).unwrap();
            sim.set_input("b", &LogicWord::from_u64(8, 0x3c)).unwrap();
            sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        }
        let expected = Some(0xa53c_u64);
        assert_eq!(rtl.read_port("y").unwrap().to_u64(), expected);
        assert_eq!(gate.read_port("y").unwrap().to_u64(), expected);
    }

    #[test]
    fn async_reset_infers_dffr() {
        let r = synth(
            "module m(input clk, rst_n, d, output reg q);
                always @(posedge clk or negedge rst_n) if (!rst_n) q <= 1'b0; else q <= d;
             endmodule",
        );
        assert_eq!(r.netlist.stats().seq, 1);
        let names: Vec<String> = r
            .netlist
            .cells()
            .iter()
            .filter_map(|c| match &c.kind {
                CellKind::Lib(id) => Some(r.library.get(*id).name.clone()),
                _ => None,
            })
            .collect();
        assert!(names.contains(&"DFFR".to_string()), "{:?}", names);
    }

    #[test]
    fn counter_synthesizes_and_simulates() {
        let src = "module m(input clk, rst, output reg [3:0] q);
            always @(posedge clk or posedge rst) if (rst) q <= 4'd0; else q <= q + 4'd1;
         endmodule";
        let r = synth(src);
        assert_eq!(r.netlist.stats().seq, 4);
        let d = compile(src, None).unwrap();
        let gd = eda_core::to_design::netlist_to_design(&r.netlist);
        let mut rtl = Simulator::new(&d).unwrap();
        let mut gate = Simulator::new(&gd).unwrap();
        for sim in [&mut rtl, &mut gate] {
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            sim.set_input("rst", &LogicWord::from_u64(1, 1)).unwrap();
            sim.set_input("rst", &LogicWord::from_u64(1, 0)).unwrap();
        }
        for i in 0..16u64 {
            for sim in [&mut rtl, &mut gate] {
                sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
                sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            }
            let rv = rtl.read_port("q").unwrap();
            let gv = gate.read_port("q").unwrap();
            assert_eq!(rv.to_u64(), gv.to_u64(), "cycle {}", i);
            assert_eq!(rv.to_u64(), Some((i + 1) & 0xF)); // 4-bit wrap
        }
    }

    #[test]
    fn mux_case_synthesizes() {
        let src = "module m(input [1:0] sel, [3:0] a, b, c, d, output [3:0] y);
            always @(*) case (sel)
                2'd0: y = a;
                2'd1: y = b;
                2'd2: y = c;
                default: y = d;
            endcase
         endmodule";
        let r = synth(src);
        let d = compile(src, None).unwrap();
        let gd = eda_core::to_design::netlist_to_design(&r.netlist);
        let mut rtl = Simulator::new(&d).unwrap();
        let mut gate = Simulator::new(&gd).unwrap();
        let vals = [0u64, 1, 6, 9];
        for s in 0..4u64 {
            for k in 0..4 {
                let (a, b, c, dd) = (
                    vals[k],
                    vals[(k + 1) % 4],
                    vals[(k + 2) % 4],
                    vals[(k + 3) % 4],
                );
                for sim in [&mut rtl, &mut gate] {
                    sim.set_input("sel", &LogicWord::from_u64(2, s)).unwrap();
                    sim.set_input("a", &LogicWord::from_u64(4, a)).unwrap();
                    sim.set_input("b", &LogicWord::from_u64(4, b)).unwrap();
                    sim.set_input("c", &LogicWord::from_u64(4, c)).unwrap();
                    sim.set_input("d", &LogicWord::from_u64(4, dd)).unwrap();
                }
                let expect = [a, b, c, dd][s as usize];
                assert_eq!(rtl.read_port("y").unwrap().to_u64(), Some(expect));
                assert_eq!(gate.read_port("y").unwrap().to_u64(), Some(expect));
            }
        }
    }

    #[test]
    fn barrel_shifter_synthesizes() {
        let src =
            "module m(input [3:0] a, [1:0] sh, output [3:0] y); assign y = a << sh; endmodule";
        let r = synth(src);
        let d = compile(src, None).unwrap();
        let gd = eda_core::to_design::netlist_to_design(&r.netlist);
        let mut rtl = Simulator::new(&d).unwrap();
        let mut gate = Simulator::new(&gd).unwrap();
        for a in 0..16u64 {
            for sh in 0..4u64 {
                for sim in [&mut rtl, &mut gate] {
                    sim.set_input("a", &LogicWord::from_u64(4, a)).unwrap();
                    sim.set_input("sh", &LogicWord::from_u64(2, sh)).unwrap();
                }
                assert_eq!(
                    rtl.read_port("y").unwrap().to_u64(),
                    gate.read_port("y").unwrap().to_u64()
                );
                assert_eq!(rtl.read_port("y").unwrap().to_u64(), Some(a << sh & 0xF));
            }
        }
    }

    #[test]
    fn multiplier_synthesizes() {
        let src = "module m(input [3:0] a, b, output [3:0] y); assign y = a * b; endmodule";
        let r = synth(src);
        let d = compile(src, None).unwrap();
        let gd = eda_core::to_design::netlist_to_design(&r.netlist);
        let mut rtl = Simulator::new(&d).unwrap();
        let mut gate = Simulator::new(&gd).unwrap();
        for a in 0..16u64 {
            for b in 0..16u64 {
                for sim in [&mut rtl, &mut gate] {
                    sim.set_input("a", &LogicWord::from_u64(4, a)).unwrap();
                    sim.set_input("b", &LogicWord::from_u64(4, b)).unwrap();
                }
                assert_eq!(
                    gate.read_port("y").unwrap().to_u64(),
                    Some((a * b) & 0xF),
                    "{}*{}",
                    a,
                    b
                );
                assert_eq!(
                    rtl.read_port("y").unwrap().to_u64(),
                    gate.read_port("y").unwrap().to_u64()
                );
            }
        }
    }

    #[test]
    fn signed_arithmetic_synthesizes_and_simulates() {
        let src = "module m(input logic signed [7:0] a, b, \
                          input logic [2:0] shift, \
                          input logic signed [5:0] c, \
                          output logic signed [7:0] sum, diff, product, shifted, selected, \
                          output logic signed [7:0] mixed, \
                          output logic lt, le, gt, ge, mixed_lt, eq_debug); \
                    assign sum = a + b; \
                    assign diff = a - b; \
                    assign product = a * b; \
                    assign shifted = a >> shift; \
                    assign selected = (a < b) ? a : b; \
                    assign mixed = $signed(a) + $signed(c); \
                    assign lt = a < b; \
                    assign le = a <= b; \
                    assign gt = a > b; \
                    assign ge = a >= b; \
                    assign mixed_lt = $signed(a) < $signed(c); \
                    assign eq_debug = a == b; \
                  endmodule";
        let r = synth(src);
        assert_eq!(r.netlist.stats().seq, 0);
        let d = compile(src, None).unwrap();
        let gd = eda_core::to_design::netlist_to_design(&r.netlist);
        let mut rtl = Simulator::new(&d).unwrap();
        let mut gate = Simulator::new(&gd).unwrap();
        for sim in [&mut rtl, &mut gate] {
            sim.set_input("a", &LogicWord::from_u64(8, (-5_i8) as u8 as u64))
                .unwrap();
            sim.set_input("b", &LogicWord::from_u64(8, 7)).unwrap();
            sim.set_input("shift", &LogicWord::from_u64(3, 2)).unwrap();
            sim.set_input("c", &LogicWord::from_u64(6, (-7_i8) as u8 as u64))
                .unwrap();
        }
        for (name, expected) in [
            ("sum", 0x02_u64),
            ("diff", 0xf4),
            ("product", 0xdd),
            ("shifted", 0xfe),
            ("selected", 0xfb),
            ("mixed", 0xf4),
            ("lt", 1),
            ("le", 1),
            ("gt", 0),
            ("ge", 0),
            ("mixed_lt", 0),
            ("eq_debug", 0),
        ] {
            assert_eq!(
                rtl.read_port(name).unwrap().to_u64(),
                Some(expected),
                "RTL {name}"
            );
            assert_eq!(
                gate.read_port(name).unwrap().to_u64(),
                Some(expected),
                "gate {name}"
            );
        }
    }

    #[test]
    fn nand_peephole() {
        let r = synth("module m(input a, b, output y); assign y = ~(a & b); endmodule");
        let names: Vec<String> = r
            .netlist
            .cells()
            .iter()
            .filter_map(|c| match &c.kind {
                CellKind::Lib(id) => Some(r.library.get(*id).name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["NAND2".to_string()]);
    }

    #[test]
    fn fsm_synthesizes_and_matches() {
        let src = "module fsm(input clk, rst, go, output reg [1:0] state, output reg done);
            always @(posedge clk or posedge rst) begin
                if (rst) state <= 2'b00;
                else case (state)
                    2'b00: state <= go ? 2'b01 : 2'b00;
                    2'b01: state <= go ? 2'b10 : 2'b00;
                    2'b10: state <= go ? 2'b11 : 2'b00;
                    2'b11: state <= 2'b00;
                endcase
            end
            always @(*) done = (state == 2'b10);
        endmodule";
        let r = synth(src);
        assert_eq!(r.netlist.stats().seq, 2);
        let d = compile(src, None).unwrap();
        let gd = eda_core::to_design::netlist_to_design(&r.netlist);
        let mut rtl = Simulator::new(&d).unwrap();
        let mut gate = Simulator::new(&gd).unwrap();
        for sim in [&mut rtl, &mut gate] {
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            sim.set_input("rst", &LogicWord::from_u64(1, 1)).unwrap();
            sim.set_input("go", &LogicWord::from_u64(1, 1)).unwrap();
            sim.set_input("rst", &LogicWord::from_u64(1, 0)).unwrap();
        }
        for i in 0..6u64 {
            for sim in [&mut rtl, &mut gate] {
                sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
                sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
            }
            assert_eq!(
                rtl.read_port("state").unwrap().to_u64(),
                gate.read_port("state").unwrap().to_u64(),
                "cycle {}",
                i
            );
            assert_eq!(
                rtl.read_port("done").unwrap().to_u64(),
                gate.read_port("done").unwrap().to_u64()
            );
        }
    }

    #[test]
    fn netlist_writer_round_trips() {
        let r = synth(
            "module m(input clk, rst, d, output reg q, nq);
            always @(posedge clk or posedge rst) if (rst) q <= 1'b0; else q <= d;
            assign nq = ~q;
         endmodule",
        );
        let text = write_verilog(&r.netlist, &r.library);
        let d = compile(&text, None).unwrap();
        let r2 = synthesize(&d, &SynthOptions::default()).unwrap();
        assert_eq!(r2.netlist.stats().seq, 1);
    }

    #[test]
    fn unmapped_option_keeps_generic_cells() {
        let d = compile(
            "module m(input a, b, output y); assign y = a & b; endmodule",
            None,
        )
        .unwrap();
        let r = synthesize(
            &d,
            &SynthOptions {
                optimize: true,
                map_lib: false,
            },
        )
        .unwrap();
        let s = r.netlist.stats();
        assert_eq!(s.lib_cells, 0);
        assert!(s.comb >= 1);
    }

    #[test]
    fn xor_x_self_folds_to_zero() {
        let r = synth("module m(input a, output y); assign y = a ^ a; endmodule");
        assert_eq!(r.netlist.stats().lib_cells, 1);
        let c = &r.netlist.cells()[0];
        assert!(matches!(&c.kind, CellKind::Lib(id) if r.library.get(*id).name == "TIE0"));
    }

    #[test]
    fn undriven_output_is_error() {
        let d = compile(
            "module m(input a, output y, z); assign y = a; endmodule",
            None,
        )
        .unwrap();
        assert!(synthesize(&d, &SynthOptions::default()).is_err());
    }

    #[test]
    fn multi_driver_is_error() {
        let d = compile(
            "module m(input a, b, output y); assign y = a; assign y = b; endmodule",
            None,
        )
        .unwrap();
        assert!(synthesize(&d, &SynthOptions::default()).is_err());
    }

    #[test]
    fn conditional_and_logical_ops() {
        let src = "module m(input [1:0] a, b, output y);
            assign y = (a == 2'd3) && (b != 2'd0);
         endmodule";
        let r = synth(src);
        let d = compile(src, None).unwrap();
        let gd = eda_core::to_design::netlist_to_design(&r.netlist);
        let mut rtl = Simulator::new(&d).unwrap();
        let mut gate = Simulator::new(&gd).unwrap();
        for a in 0..4u64 {
            for b in 0..4u64 {
                for sim in [&mut rtl, &mut gate] {
                    sim.set_input("a", &LogicWord::from_u64(2, a)).unwrap();
                    sim.set_input("b", &LogicWord::from_u64(2, b)).unwrap();
                }
                let expect = (a == 3 && b != 0) as u64;
                assert_eq!(rtl.read_port("y").unwrap().to_u64(), Some(expect));
                assert_eq!(gate.read_port("y").unwrap().to_u64(), Some(expect));
            }
        }
    }
}
