//! # eda-synth — RTL synthesis
//!
//! Lowers an elaborated [`Design`] to a gate-level [`Netlist`] in three
//! stages: bit-blasting to generic gates, logic optimization (constant
//! folding, buffer collapse, duplicate merging, dead-code elimination), and
//! technology mapping onto the standard-cell library.

use eda_core::ir::{Design, Sens};
use eda_core::library::CellLibrary;
use eda_core::logic::LogicValue;
use eda_core::netlist::{Cell, CellKind, Dir, NetId, Netlist};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use memory_model::{rom_contents_from_design, BlockRamLowering, RomContents};

pub(crate) fn sorted_set(set: &HashSet<usize>) -> Vec<usize> {
    let mut values = set.iter().copied().collect::<Vec<_>>();
    values.sort_unstable();
    values
}

pub mod expressions;
pub mod map;
pub mod memory;
pub mod memory_model;
pub mod memory_plan;
pub mod opt;
pub mod statements;
pub mod writer;

pub use map::map_to_library;
pub use memory_plan::{plan_memories, MemoryMacroCandidate, MemoryMacroPlan};
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

pub(crate) struct Synth<'a> {
    pub(crate) design: &'a Design,
    pub(crate) nl: Netlist,
    pub(crate) flat2net: Vec<NetId>,
    pub(crate) rom_contents: HashMap<usize, RomContents>,
    pub(crate) ram_state: HashMap<usize, Vec<Vec<NetId>>>,
    pub(crate) block_ram_state: HashMap<usize, BlockRamLowering>,
    pub(crate) lowered_memories: HashSet<usize>,
    pub(crate) skipped_processes: HashSet<usize>,
    pub(crate) sym: HashMap<usize, Vec<NetId>>,
    pub(crate) written: HashSet<usize>,
    pub(crate) warnings: Vec<String>,
    pub(crate) tmp: usize,
    pub(crate) driver_log: Vec<(NetId, NetId)>,
    /// Fixed present-state symbols while lowering a clocked process.
    pub(crate) read_sym: Option<HashMap<usize, Vec<NetId>>>,
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
            block_ram_state: HashMap::new(),
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
        self.lower_block_ram_processes()?;
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
                Sens::Comb | Sens::Any(_) => {
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

    pub(crate) fn drivers_list(&self) -> Vec<(NetId, NetId)> {
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

    pub(crate) fn reset_sym(&mut self) {
        self.sym.clear();
        self.written.clear();
        self.read_sym = None;
    }

    pub(crate) fn decl_width(&self, base: usize) -> usize {
        self.design
            .net_decls
            .iter()
            .find(|d| d.base == base)
            .map(|d| d.width)
            .unwrap_or(1)
    }

    pub(crate) fn flat2net_of(&self, flat: usize) -> Result<NetId, String> {
        self.flat2net
            .get(flat)
            .copied()
            .filter(|n| *n != usize::MAX)
            .ok_or_else(|| format!("flat net {} out of range", flat))
    }

    pub(crate) fn present_state_symbol_map(&self) -> HashMap<usize, Vec<NetId>> {
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

    pub(crate) fn declare_driver(&mut self, dst: NetId, src: NetId) -> Result<(), String> {
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

    pub(crate) fn add_cell(
        &mut self,
        hint: &str,
        kind: CellKind,
        inputs: Vec<NetId>,
        output: NetId,
    ) {
        let name = format!("{}_{}", hint, self.tmp);
        self.tmp += 1;
        self.nl.add_cell(Cell {
            name,
            kind,
            inputs,
            output,
        });
    }

    pub(crate) fn new_tmp(&mut self, hint: &str) -> NetId {
        let name = format!("{}_n{}", hint, self.tmp);
        self.tmp += 1;
        self.nl.add_net(name)
    }

    pub(crate) fn const_net(&mut self, v: LogicValue) -> NetId {
        let out = self.new_tmp("c");
        self.add_cell("const", CellKind::Const(v), vec![], out);
        out
    }

    pub(crate) fn gate1(&mut self, hint: &str, kind: CellKind, a: NetId) -> NetId {
        let out = self.new_tmp(hint);
        self.add_cell(hint, kind, vec![a], out);
        out
    }

    pub(crate) fn gate3(
        &mut self,
        hint: &str,
        kind: CellKind,
        a: NetId,
        b: NetId,
        c: NetId,
    ) -> NetId {
        let out = self.new_tmp(hint);
        self.add_cell(hint, kind, vec![a, b, c], out);
        out
    }

    pub(crate) fn const_val(&self, net: NetId) -> Option<LogicValue> {
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
}

#[cfg(test)]
mod tests;
