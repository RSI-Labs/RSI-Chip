//! Gate-level netlist IR shared by synthesis, formal verification, STA and PnR.
//!
//! A netlist is flat and bit-level: every net carries one bit, vectors are
//! grouped only at ports. Cells are either generic primitives ([`CellKind`])
//! or instances of library cells ([`CellKind::Lib`]).

use crate::ir::Design;
use crate::library::{CellLibrary, LibCellId};
use crate::logic::{LogicValue, LogicWord};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub type NetId = usize;
pub type CellId = usize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CellKind {
    /// Constant driver.
    Const(LogicValue),
    Buf,
    Not,
    /// n-input gates (n >= 1).
    And,
    Nand,
    Or,
    Nor,
    Xor,
    Xnor,
    /// inputs = [sel, in0, in1]: sel==0 selects in0.
    Mux,
    /// inputs = [clk, d]; output q.
    Dff {
        posedge: bool,
    },
    /// inputs = [clk, d, rst]; async reset to 0 when rst is at its active level.
    DffR {
        posedge: bool,
        reset_high: bool,
    },
    /// One output bit of a synchronous/asynchronous block-ROM read port.
    ///
    /// Inputs are address bits in LSB-first order. Each output-bit cell
    /// shares the full contents, allowing downstream RTL/formal consumers to
    /// reconstruct one macro while preserving the netlist's one-bit output
    /// contract.
    BlockRom {
        address_width: usize,
        bit_index: usize,
        contents: std::sync::Arc<Vec<LogicWord>>,
    },
    /// Standard-cell instance; behaviour comes from the [`CellLibrary`].
    Lib(LibCellId),
}

impl CellKind {
    /// Evaluate a combinational generic primitive.
    pub fn eval(&self, ins: &[LogicValue]) -> LogicValue {
        match self {
            CellKind::Const(v) => *v,
            CellKind::Buf => ins.first().copied().unwrap_or(LogicValue::X),
            CellKind::Not => ins.first().copied().unwrap_or(LogicValue::X).not(),
            CellKind::And | CellKind::Nand => {
                let v = ins
                    .iter()
                    .copied()
                    .reduce(|a, b| a.and(b))
                    .unwrap_or(LogicValue::One);
                if matches!(self, CellKind::Nand) {
                    v.not()
                } else {
                    v
                }
            }
            CellKind::Or | CellKind::Nor => {
                let v = ins
                    .iter()
                    .copied()
                    .reduce(|a, b| a.or(b))
                    .unwrap_or(LogicValue::Zero);
                if matches!(self, CellKind::Nor) {
                    v.not()
                } else {
                    v
                }
            }
            CellKind::Xor | CellKind::Xnor => {
                let v = ins
                    .iter()
                    .copied()
                    .reduce(|a, b| a.xor(b))
                    .unwrap_or(LogicValue::Zero);
                if matches!(self, CellKind::Xnor) {
                    v.not()
                } else {
                    v
                }
            }
            CellKind::Mux => {
                let sel = ins[0];
                match sel.as_bool() {
                    Some(false) => ins[1],
                    Some(true) => ins[2],
                    None => {
                        let merged = ins[1].resolve(ins[2]);
                        if merged.is_unknown() {
                            LogicValue::X
                        } else {
                            merged
                        }
                    }
                }
            }
            CellKind::BlockRom {
                address_width,
                bit_index,
                contents,
            } => {
                if ins.len() < *address_width || *address_width > u32::BITS as usize {
                    return LogicValue::X;
                }
                let mut address = 0_u32;
                for (bit, value) in ins.iter().take(*address_width).enumerate() {
                    let Some(set) = value.as_bool() else {
                        return LogicValue::X;
                    };
                    if set {
                        address |= 1 << bit;
                    }
                }
                contents
                    .get(address as usize)
                    .map(|word| word.bit(*bit_index))
                    .unwrap_or(LogicValue::X)
            }
            _ => LogicValue::X, // sequential / lib cells are not combinational
        }
    }
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub name: String,
    pub kind: CellKind,
    pub inputs: Vec<NetId>,
    pub output: NetId,
}

#[derive(Clone, Debug)]
pub struct Net {
    pub name: String,
    pub drivers: Vec<CellId>,
    pub sinks: Vec<CellId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    In,
    Out,
}

#[derive(Clone, Debug)]
pub struct Port {
    pub name: String,
    pub dir: Dir,
    /// MSB-first net ids of the port bits.
    pub bits: Vec<NetId>,
}

/// A hierarchy port resolved to post-synthesis net IDs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetlistHierarchyPort {
    pub name: String,
    pub direction: Dir,
    /// Post-synthesis instance-local port nets before boundary resolution.
    pub local_nets: Vec<NetId>,
    /// Post-synthesis parent/connection nets after boundary resolution.
    pub nets: Vec<NetId>,
    pub width: usize,
}

/// Module-instance metadata preserved after synthesis.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetlistHierarchyInstance {
    pub path: String,
    pub module: String,
    pub parent: Option<String>,
    pub depth: usize,
    pub nets: Vec<NetId>,
    pub ports: Vec<NetlistHierarchyPort>,
}

#[derive(Clone, Debug, Default)]
pub struct Netlist {
    pub name: String,
    nets: Vec<Net>,
    cells: Vec<Cell>,
    pub ports: Vec<Port>,
    /// Post-synthesis mapping of the elaborated source hierarchy.
    pub hierarchy: Vec<NetlistHierarchyInstance>,
    /// Library reference for resolving `Lib` cells.
    pub lib_cache: Option<Arc<CellLibrary>>,
}

fn resolved_hierarchy_net(design: &Design, flat: usize, depth: usize) -> usize {
    if depth > 64 {
        return flat;
    }
    let Some(instance) = design.owning_instance(flat) else {
        return flat;
    };
    let Some(port) = instance
        .ports
        .iter()
        .find(|port| flat >= port.base && flat < port.base + port.width)
    else {
        return flat;
    };
    match port.connections.get(flat - port.base).copied().flatten() {
        Some(parent) => resolved_hierarchy_net(design, parent, depth + 1),
        None => flat,
    }
}

impl Netlist {
    pub fn new(name: impl Into<String>) -> Self {
        Netlist {
            name: name.into(),
            nets: Vec::new(),
            cells: Vec::new(),
            ports: Vec::new(),
            hierarchy: Vec::new(),
            lib_cache: None,
        }
    }

    pub fn add_net(&mut self, name: impl Into<String>) -> NetId {
        self.nets.push(Net {
            name: name.into(),
            drivers: Vec::new(),
            sinks: Vec::new(),
        });
        self.nets.len() - 1
    }

    pub fn net_count(&self) -> usize {
        self.nets.len()
    }

    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    pub fn net(&self, id: NetId) -> &Net {
        &self.nets[id]
    }

    pub fn nets(&self) -> &[Net] {
        &self.nets
    }

    pub fn cell(&self, id: CellId) -> &Cell {
        &self.cells[id]
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn cells_mut(&mut self) -> &mut [Cell] {
        &mut self.cells
    }

    pub fn net_by_name(&self, name: &str) -> Option<NetId> {
        self.nets.iter().position(|n| n.name == name)
    }

    pub fn cell_by_name(&self, name: &str) -> Option<CellId> {
        self.cells.iter().position(|cell| cell.name == name)
    }

    /// Attach elaborated hierarchy metadata using stable net names.
    ///
    /// Synthesis may remove or compact source nets, so flat design IDs are
    /// remapped by name. Implementation-generated nets can be attributed
    /// through the instance owning their driver output.
    pub fn attach_hierarchy(&mut self, design: &Design) {
        self.hierarchy = design
            .hierarchy
            .iter()
            .map(|instance| NetlistHierarchyInstance {
                path: instance.path.clone(),
                module: instance.module.clone(),
                parent: instance.parent.clone(),
                depth: instance.depth,
                nets: (instance.net_start..instance.net_end)
                    .filter_map(|flat| {
                        let resolved = resolved_hierarchy_net(design, flat, 0);
                        self.net_by_name(&design.net_name(resolved))
                    })
                    .collect(),
                ports: instance
                    .ports
                    .iter()
                    .map(|port| NetlistHierarchyPort {
                        name: port.name.clone(),
                        direction: match port.direction {
                            crate::ir::Dir::In => Dir::In,
                            crate::ir::Dir::Out => Dir::Out,
                        },
                        local_nets: (0..port.width)
                            .filter_map(|offset| {
                                self.net_by_name(&design.net_name(port.base + offset))
                            })
                            .collect(),
                        nets: (0..port.width)
                            .filter_map(|offset| {
                                let resolved =
                                    resolved_hierarchy_net(design, port.base + offset, 0);
                                self.net_by_name(&design.net_name(resolved))
                            })
                            .collect(),
                        width: port.width,
                    })
                    .collect(),
            })
            .collect();
    }

    pub fn hierarchy_instance(&self, path: &str) -> Option<&NetlistHierarchyInstance> {
        self.hierarchy.iter().find(|instance| instance.path == path)
    }

    /// Find the deepest hierarchy instance containing a post-synthesis net.
    pub fn owning_hierarchy_instance(&self, net: NetId) -> Option<&NetlistHierarchyInstance> {
        self.hierarchy
            .iter()
            .filter(|instance| instance.nets.contains(&net))
            .min_by(|left, right| {
                right
                    .depth
                    .cmp(&left.depth)
                    .then_with(|| left.nets.len().cmp(&right.nets.len()))
                    .then_with(|| left.path.cmp(&right.path))
            })
    }

    /// Attribute a cell to the deepest instance owning its output net.
    pub fn cell_hierarchy_instance(&self, cell: CellId) -> Option<&NetlistHierarchyInstance> {
        let output = self.cell(cell).output;
        self.hierarchy
            .iter()
            .filter(|instance| {
                instance.nets.contains(&output)
                    || instance.ports.iter().any(|port| {
                        port.direction == Dir::Out
                            && (port.local_nets.contains(&output) || port.nets.contains(&output))
                    })
            })
            .min_by(|left, right| {
                right
                    .depth
                    .cmp(&left.depth)
                    .then_with(|| left.nets.len().cmp(&right.nets.len()))
                    .then_with(|| left.path.cmp(&right.path))
            })
    }

    /// Render the post-synthesis hierarchy and retained net mapping.
    pub fn hierarchy_report(&self) -> String {
        let mut out = format!("netlist hierarchy: {} instance(s)\n", self.hierarchy.len());
        for instance in &self.hierarchy {
            out.push_str(&format!(
                "  {}{}: module '{}', nets {}, ports {}\n",
                "  ".repeat(instance.depth),
                instance.path,
                instance.module,
                instance.nets.len(),
                instance.ports.len()
            ));
        }
        out
    }

    pub fn add_cell(&mut self, cell: Cell) -> CellId {
        let out = cell.output;
        let id = self.cells.len();
        for &i in &cell.inputs {
            self.nets[i].sinks.push(id);
        }
        self.nets[out].drivers.push(id);
        self.cells.push(cell);
        id
    }

    /// Move a cell's output to another net (single-use hookup retarget).
    pub fn retarget_cell_output(&mut self, id: CellId, new_out: NetId) {
        let old = self.cells[id].output;
        self.nets[old].drivers.retain(|d| *d != id);
        self.cells[id].output = new_out;
        self.nets[new_out].drivers.push(id);
    }

    /// Unregister a cell's adjacency (drivers/sinks) so compaction can drop it.
    pub fn disconnect_cell(&mut self, id: CellId) {
        let (out, ins) = {
            let c = &self.cells[id];
            (c.output, c.inputs.clone())
        };
        self.nets[out].drivers.retain(|d| *d != id);
        for i in ins {
            self.nets[i].sinks.retain(|s| *s != id);
        }
    }

    /// Drop the given cells and any nets that become unused, renumbering
    /// everything (including ports) consistently.
    pub fn compact(&mut self, dead: &[CellId]) {
        let keep_cell: Vec<bool> = {
            let mut k = vec![true; self.cells.len()];
            for d in dead {
                k[*d] = false;
            }
            k
        };
        let mut used_net = vec![false; self.nets.len()];
        for (i, c) in self.cells.iter().enumerate() {
            if keep_cell[i] {
                used_net[c.output] = true;
                for n in &c.inputs {
                    used_net[*n] = true;
                }
            }
        }
        for p in &self.ports {
            for n in &p.bits {
                used_net[*n] = true;
            }
        }
        let mut net_map = vec![usize::MAX; self.nets.len()];
        let mut new_nets: Vec<Net> = Vec::new();
        for (i, n) in self.nets.iter().enumerate() {
            if used_net[i] {
                net_map[i] = new_nets.len();
                new_nets.push(Net {
                    name: n.name.clone(),
                    drivers: Vec::new(),
                    sinks: Vec::new(),
                });
            }
        }
        let mut new_cells: Vec<Cell> = Vec::new();
        for (i, c) in self.cells.iter().enumerate() {
            if keep_cell[i] {
                new_cells.push(Cell {
                    name: c.name.clone(),
                    kind: c.kind.clone(),
                    output: net_map[c.output],
                    inputs: c.inputs.iter().map(|n| net_map[*n]).collect(),
                });
            }
        }
        for (ci, c) in new_cells.iter().enumerate() {
            new_nets[c.output].drivers.push(ci);
            for &i in &c.inputs {
                new_nets[i].sinks.push(ci);
            }
        }
        for p in self.ports.iter_mut() {
            for b in p.bits.iter_mut() {
                *b = net_map[*b];
            }
        }
        self.nets = new_nets;
        self.cells = new_cells;
    }

    pub fn set_net_name(&mut self, id: NetId, name: String) {
        self.nets[id].name = name;
    }

    /// Atomically replace a cell: old adjacency is removed, the new cell's
    /// is registered.
    pub fn replace_cell(&mut self, id: CellId, cell: Cell) {
        self.disconnect_cell(id);
        self.cells[id] = cell;
        let out = self.cells[id].output;
        let ins = self.cells[id].inputs.clone();
        self.nets[out].drivers.push(id);
        for i in ins {
            self.nets[i].sinks.push(id);
        }
    }

    pub fn add_port(&mut self, name: impl Into<String>, dir: Dir, bits: Vec<NetId>) {
        self.ports.push(Port {
            name: name.into(),
            dir,
            bits,
        });
    }

    pub fn port_by_name(&self, name: &str) -> Option<&Port> {
        self.ports.iter().find(|p| p.name == name)
    }

    /// Topological order of all cells over the data dependency graph,
    /// treating sequential cells as boundaries (their inputs are sinks, their
    /// outputs are sources). Returns None when a combinational loop exists.
    pub fn topo_cells(&self) -> Option<Vec<CellId>> {
        use std::collections::VecDeque;

        let n = self.cells.len();
        let mut indeg = vec![0usize; n];
        let mut succ: Vec<Vec<CellId>> = vec![Vec::new(); n];
        for (ci, c) in self.cells.iter().enumerate() {
            if self.cell_is_sequential(&c.kind) {
                continue; // sequential boundary: do not propagate through
            }
            let mut deg = 0;
            for &i in &c.inputs {
                for &d in &self.nets[i].drivers {
                    if d != ci && !self.cell_is_sequential(&self.cells[d].kind) {
                        deg += 1;
                        succ[d].push(ci);
                    }
                }
            }
            indeg[ci] = deg;
        }
        let mut order = Vec::with_capacity(n);
        let mut queue: VecDeque<CellId> = (0..n).filter(|i| indeg[*i] == 0).collect();
        while let Some(c) = queue.pop_front() {
            order.push(c);
            for &s in &succ[c] {
                indeg[s] -= 1;
                if indeg[s] == 0 {
                    queue.push_back(s);
                }
            }
        }
        if order.len() != n {
            return None;
        }
        Some(order)
    }

    /// Is this net a sequential cell (using the library when needed)?
    pub fn cell_is_sequential(&self, kind: &CellKind) -> bool {
        match kind {
            CellKind::Dff { .. } | CellKind::DffR { .. } => true,
            CellKind::Lib(id) => self
                .lib_cache
                .as_ref()
                .is_some_and(|l| l.get(*id).seq.is_some()),
            _ => false,
        }
    }

    pub fn set_library(&mut self, lib: Arc<CellLibrary>) {
        self.lib_cache = Some(lib);
    }

    pub fn library(&self) -> Option<&Arc<CellLibrary>> {
        self.lib_cache.as_ref()
    }

    /// Evaluate one cell given a net value map; resolves library cells.
    pub fn eval_cell(&self, id: CellId, vals: &HashMap<NetId, LogicValue>) -> LogicValue {
        let c = &self.cells[id];
        let ins: Vec<LogicValue> = c
            .inputs
            .iter()
            .map(|n| *vals.get(n).unwrap_or(&LogicValue::X))
            .collect();
        match &c.kind {
            CellKind::Lib(lid) => match &self.lib_cache {
                Some(lib) => lib.eval(*lid, &ins),
                None => LogicValue::X,
            },
            k => k.eval(&ins),
        }
    }

    pub fn stats(&self) -> NetlistStats {
        let mut s = NetlistStats {
            nets: self.nets.len(),
            cells: self.cells.len(),
            ..Default::default()
        };
        let mut block_rom_macros = HashSet::<(usize, Vec<NetId>, usize)>::new();
        for c in &self.cells {
            match &c.kind {
                CellKind::Const(_) => s.consts += 1,
                CellKind::Dff { .. } | CellKind::DffR { .. } => s.seq += 1,
                CellKind::BlockRom {
                    address_width,
                    contents,
                    ..
                } => {
                    block_rom_macros.insert((
                        Arc::as_ptr(contents) as *const _ as usize,
                        c.inputs.clone(),
                        *address_width,
                    ));
                }
                CellKind::Lib(id) => {
                    s.lib_cells += 1;
                    if self
                        .lib_cache
                        .as_ref()
                        .is_some_and(|l| l.get(*id).seq.is_some())
                    {
                        s.seq += 1;
                    }
                }
                _ => s.comb += 1,
            }
        }
        s.memory_macros = block_rom_macros.len();
        s
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NetlistStats {
    pub nets: usize,
    pub cells: usize,
    pub comb: usize,
    pub seq: usize,
    pub consts: usize,
    pub lib_cells: usize,
    pub memory_macros: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_and_inv() -> Netlist {
        let mut nl = Netlist::new("t");
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        let n1 = nl.add_net("n1");
        let a_and_b = nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::And,
            inputs: vec![a, b],
            output: n1,
        });
        let _u2 = nl.add_cell(Cell {
            name: "u2".into(),
            kind: CellKind::Not,
            inputs: vec![n1],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        assert_eq!(nl.net(n1).drivers, vec![a_and_b]);
        nl
    }

    #[test]
    fn build_and_query() {
        let nl = build_and_inv();
        assert_eq!(nl.cell_count(), 2);
        assert_eq!(nl.net_count(), 4);
        assert_eq!(nl.net_by_name("n1"), Some(3));
        let topo = nl.topo_cells().unwrap();
        assert_eq!(topo.len(), 2);
        assert_eq!(topo[0], 0); // and feeds not
    }

    #[test]
    fn eval_primitives() {
        let z = LogicValue::Zero;
        let o = LogicValue::One;
        let x = LogicValue::X;
        assert_eq!(CellKind::And.eval(&[o, o]), o);
        assert_eq!(CellKind::And.eval(&[o, x]), x);
        assert_eq!(CellKind::Nand.eval(&[o, o]), z);
        assert_eq!(CellKind::Or.eval(&[z, x]), x);
        assert_eq!(CellKind::Nor.eval(&[z, z]), o);
        assert_eq!(CellKind::Xnor.eval(&[o, z]), z);
        assert_eq!(CellKind::Mux.eval(&[z, o, x]), o);
        assert_eq!(CellKind::Mux.eval(&[o, z, x]), x);
        assert_eq!(CellKind::Mux.eval(&[x, o, z]), x);
        assert_eq!(CellKind::Mux.eval(&[x, o, o]), o);
    }

    #[test]
    fn combinational_loop_detected() {
        let mut nl = Netlist::new("loop");
        let a = nl.add_net("a");
        let w1 = nl.add_net("w1");
        let w2 = nl.add_net("w2");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "i1".into(),
            kind: CellKind::Not,
            inputs: vec![a],
            output: w1,
        });
        nl.add_cell(Cell {
            name: "l1".into(),
            kind: CellKind::And,
            inputs: vec![w1, w2],
            output: y,
        });
        nl.add_cell(Cell {
            name: "l2".into(),
            kind: CellKind::Buf,
            inputs: vec![y],
            output: w2,
        });
        assert!(nl.topo_cells().is_none());
    }

    #[test]
    fn compact_removes_dead() {
        let mut nl = build_and_inv();
        nl.disconnect_cell(1);
        nl.compact(&[1]);
        assert_eq!(nl.cell_count(), 1);
        assert_eq!(nl.net_count(), 4); // a, b, n1, y (y survives as a port)
        assert!(nl.net_by_name("n1").is_some());
        let y = nl.port_by_name("y").unwrap().bits[0];
        assert_eq!(nl.net(y).drivers.len(), 0); // NOT removed; nothing else drives y
        assert_eq!(nl.net_by_name("y"), Some(y));
    }

    #[test]
    fn stats() {
        let nl = build_and_inv();
        let s = nl.stats();
        assert_eq!(s.comb, 2);
        assert_eq!(s.seq, 0);
    }

    #[test]
    fn block_rom_evaluates_addresses_and_counts_one_macro_per_port() {
        let contents = Arc::new(vec![
            LogicWord::from_u64(2, 0b01),
            LogicWord::from_u64(2, 0b10),
            LogicWord::from_u64(2, 0b11),
        ]);
        let make_cell = |address_width, bit_index| CellKind::BlockRom {
            address_width,
            bit_index,
            contents: contents.clone(),
        };

        let zero = LogicValue::Zero;
        let one = LogicValue::One;
        let x = LogicValue::X;
        assert_eq!(make_cell(2, 0).eval(&[zero, zero]), one);
        assert_eq!(make_cell(2, 0).eval(&[one, zero]), zero);
        assert_eq!(make_cell(2, 1).eval(&[zero, zero]), zero);
        assert_eq!(make_cell(2, 1).eval(&[one, zero]), one);
        assert_eq!(make_cell(2, 0).eval(&[zero, one]), one);
        assert_eq!(make_cell(2, 0).eval(&[one, one]), x);
        assert_eq!(make_cell(2, 0).eval(&[x, zero]), x);

        let mut nl = Netlist::new("rom");
        let address0 = nl.add_net("address0");
        let address1 = nl.add_net("address1");
        for bit_index in 0..2 {
            let output = nl.add_net(format!("q{bit_index}"));
            nl.add_cell(Cell {
                name: format!("rom_bit{bit_index}"),
                kind: make_cell(2, bit_index),
                inputs: vec![address0, address1],
                output,
            });
        }
        let stats = nl.stats();
        assert_eq!(stats.memory_macros, 1);
        assert_eq!(stats.cells, 2);
    }
}
