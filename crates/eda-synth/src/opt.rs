//! Logic optimization passes: constant folding, buffer collapse, duplicate
//! cell merging and dead-code elimination, iterated to a fixpoint.

use eda_core::netlist::{CellKind, Dir, NetId, Netlist};
use std::collections::HashMap;

pub fn optimize(nl: &mut Netlist, warnings: &mut Vec<String>) {
    loop {
        fold_consts(nl);
        let (c0, nl1) = cancel_double_inverters(nl);
        *nl = nl1;
        let (c1, nl2) = collapse_bufs(nl);
        *nl = nl2;
        let (c2, nl3) = dedup(nl);
        *nl = nl3;
        let (c3, nl4, dead) = remove_dead(nl, warnings);
        let _ = dead;
        *nl = nl4;
        if !(c0 || c1 || c2 || c3) {
            break;
        }
    }
}

/// Rewrite `NOT(NOT(x))` as a buffer. The following buffer-collapse pass
/// removes that buffer, including the port-connected final net when possible.
fn cancel_double_inverters(nl: &mut Netlist) -> (bool, Netlist) {
    for ci in 0..nl.cell_count() {
        let (inner, mid) = {
            let outer = nl.cell(ci);
            if !matches!(outer.kind, CellKind::Not) || outer.inputs.len() != 1 {
                continue;
            }
            let mid = outer.inputs[0];
            let drivers = &nl.net(mid).drivers;
            if drivers.len() != 1 || drivers[0] == ci {
                continue;
            }
            // The intermediate inverter may be shared by other logic. It can
            // only be deleted when this outer inverter is its sole sink.
            if nl.net(mid).sinks != vec![ci] {
                continue;
            }
            (drivers[0], mid)
        };
        let inner_cell = nl.cell(inner);
        if !matches!(inner_cell.kind, CellKind::Not) || inner_cell.inputs.len() != 1 {
            continue;
        }
        let source = inner_cell.inputs[0];
        nl.replace_cell(
            ci,
            eda_core::netlist::Cell {
                name: nl.cell(ci).name.clone(),
                kind: CellKind::Buf,
                inputs: vec![source],
                output: nl.cell(ci).output,
            },
        );
        nl.disconnect_cell(inner);
        let mut out = nl.clone();
        out.compact(&[inner]);
        let _ = mid;
        return (true, out);
    }
    (false, nl.clone())
}

fn port_bits(nl: &Netlist) -> Vec<NetId> {
    nl.ports
        .iter()
        .flat_map(|p| p.bits.iter().copied())
        .collect()
}

fn is_commutative(kind: &CellKind) -> bool {
    matches!(
        kind,
        CellKind::And
            | CellKind::Nand
            | CellKind::Or
            | CellKind::Nor
            | CellKind::Xor
            | CellKind::Xnor
    )
}

fn is_comb(kind: &CellKind) -> bool {
    !matches!(
        kind,
        CellKind::Dff { .. }
            | CellKind::DffR { .. }
            | CellKind::BlockRamWrite { .. }
            | CellKind::BlockRamRead { .. }
    )
}

/// Constant folding: any combinational cell whose inputs are all constants
/// becomes a Const cell (in place, same output net).
fn fold_consts(nl: &mut Netlist) {
    loop {
        let mut changed = false;
        // value of each net if it has a single const-ish driver
        let mut val: HashMap<NetId, eda_core::logic::LogicValue> = HashMap::new();
        // iterate repeatedly until stable (cheap for our sizes)
        for _ in 0..nl.cell_count() + 1 {
            let mut progressed = false;
            for ci in 0..nl.cell_count() {
                if !is_comb(&nl.cell(ci).kind) {
                    continue;
                }
                let out = nl.cell(ci).output;
                if val.contains_key(&out) {
                    continue;
                }
                let ins = nl.cell(ci).inputs.clone();
                if ins.is_empty() {
                    if let CellKind::Const(v) = nl.cell(ci).kind {
                        val.insert(out, v);
                        progressed = true;
                    }
                    continue;
                }
                let mut known = true;
                let mut vals = Vec::new();
                for i in &ins {
                    match val.get(i) {
                        Some(v) => vals.push(*v),
                        None => {
                            known = false;
                            break;
                        }
                    }
                }
                if known {
                    let v = nl.cell(ci).kind.eval(&vals);
                    val.insert(out, v);
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }
        // rewrite gates with fully-known inputs into Const cells
        for ci in 0..nl.cell_count() {
            let (kind, out, ins) = {
                let c = nl.cell(ci);
                (c.kind.clone(), c.output, c.inputs.clone())
            };
            if !is_comb(&kind) || matches!(kind, CellKind::Const(_) | CellKind::Buf) {
                continue;
            }
            if ins.is_empty() {
                continue;
            }
            if ins.iter().all(|i| val.contains_key(i)) {
                let vs: Vec<_> = ins.iter().map(|i| val[i]).collect();
                let v = kind.eval(&vs);
                let c = eda_core::netlist::Cell {
                    name: format!("folded_{}", ci),
                    kind: CellKind::Const(v),
                    inputs: vec![],
                    output: out,
                };
                nl.replace_cell(ci, c);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Collapse buffers: `Buf(in=I, out=O)` where O is not a port bit is removed
/// and all references to O are redirected to I. Returns a new netlist.
fn collapse_bufs(nl: &Netlist) -> (bool, Netlist) {
    let mut map: HashMap<NetId, NetId> = HashMap::new();
    let mut dead = vec![false; nl.cell_count()];
    for (ci, dead_cell) in dead.iter_mut().enumerate() {
        let c = nl.cell(ci);
        if let CellKind::Buf = c.kind {
            // Alias the buffer output to its input. Port bits are resolved
            // below, so a feedthrough port can legally share its source net.
            map.insert(c.output, c.inputs[0]);
            *dead_cell = true;
        }
    }
    if map.is_empty() {
        return (false, nl.clone());
    }
    // resolve chains
    let resolve = |mut n: NetId| -> NetId {
        let mut guard = 0;
        while let Some(&m) = map.get(&n) {
            n = m;
            guard += 1;
            if guard > 10_000 {
                break;
            }
        }
        n
    };
    let mut out = Netlist::new(nl.name.clone());
    for _ in 0..nl.net_count() {
        out.add_net("");
    }
    for (i, n) in nl.nets().iter().enumerate() {
        out.set_net_name(i, n.name.clone());
    }
    for (ci, dead_cell) in dead.iter_mut().enumerate() {
        if *dead_cell {
            continue;
        }
        let c = nl.cell(ci);
        out.add_cell(eda_core::netlist::Cell {
            name: c.name.clone(),
            kind: c.kind.clone(),
            inputs: c.inputs.iter().map(|n| resolve(*n)).collect(),
            output: c.output,
        });
    }
    for p in &nl.ports {
        let bits = p.bits.iter().map(|&n| resolve(n)).collect();
        out.add_port(p.name.clone(), p.dir, bits);
    }
    let mut compacted = out;
    compacted.compact(&[]);
    (true, compacted)
}

/// Merge structurally identical cells (commutative inputs normalized).
/// Cells driving port bits are never merged away (port nets must keep a
/// driver).
fn stable_rtl_name(name: &str) -> bool {
    !(name.starts_with("mux_n")
        || name.starts_with("g_n")
        || name.starts_with("c_n")
        || name.starts_with("not_n")
        || name.starts_with("and_n")
        || name.starts_with("or_n")
        || name.starts_with("xor_n")
        || name.starts_with("q_n")
        || name
            .split('.')
            .next()
            .is_some_and(|base| base.parse::<f64>().is_ok()))
}

fn better_alias_name(current: &(u8, String), candidate_name: &str, is_port: bool) -> bool {
    let candidate_priority = if is_port {
        2
    } else if stable_rtl_name(candidate_name) {
        1
    } else {
        0
    };
    candidate_priority > current.0
        || (candidate_priority == current.0 && candidate_name < current.1.as_str())
}

fn dedup(nl: &Netlist) -> (bool, Netlist) {
    let ports = port_bits(nl);
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut map: HashMap<NetId, NetId> = HashMap::new();
    let mut preferred_alias_names: HashMap<NetId, (u8, String)> = HashMap::new();
    let mut dead = vec![false; nl.cell_count()];
    for (ci, dead_cell) in dead.iter_mut().enumerate() {
        let c = nl.cell(ci);
        if ports.contains(&c.output) {
            // register so later duplicates of a port driver still merge INTO
            // nothing; simply keep this cell as canonical
            let mut key_ins = c.inputs.clone();
            if is_commutative(&c.kind) {
                key_ins.sort_unstable();
            }
            seen.insert(format!("{:?}|{:?}", c.kind, key_ins), ci);
            continue;
        }
        let commutative = is_commutative(&c.kind);
        let mut key_ins = c.inputs.clone();
        if commutative {
            key_ins.sort_unstable();
        }
        let key = format!("{:?}|{:?}", c.kind, key_ins);
        if let Some(&keep) = seen.get(&key) {
            let keep_output = nl.cell(keep).output;
            map.insert(c.output, keep_output);
            let candidate_name = nl.net(c.output).name.clone();
            let current = preferred_alias_names
                .entry(keep_output)
                .or_insert_with(|| (0, nl.net(keep_output).name.clone()));
            if better_alias_name(current, &candidate_name, ports.contains(&c.output)) {
                *current = (
                    if ports.contains(&c.output) {
                        2
                    } else if stable_rtl_name(&candidate_name) {
                        1
                    } else {
                        0
                    },
                    candidate_name,
                );
            }
            *dead_cell = true;
        } else {
            seen.insert(key, ci);
        }
    }
    if map.is_empty() {
        return (false, nl.clone());
    }
    let resolve = |mut n: NetId| -> NetId {
        let mut guard = 0;
        while let Some(&m) = map.get(&n) {
            n = m;
            guard += 1;
            if guard > 10_000 {
                break;
            }
        }
        n
    };
    let mut out = Netlist::new(nl.name.clone());
    for _ in 0..nl.net_count() {
        out.add_net("");
    }
    for (i, n) in nl.nets().iter().enumerate() {
        out.set_net_name(i, n.name.clone());
    }
    for (net, (_, name)) in &preferred_alias_names {
        out.set_net_name(*net, name.clone());
    }
    for (ci, dead_cell) in dead.iter_mut().enumerate() {
        if *dead_cell {
            continue;
        }
        let c = nl.cell(ci);
        out.add_cell(eda_core::netlist::Cell {
            name: c.name.clone(),
            kind: c.kind.clone(),
            inputs: c.inputs.iter().map(|n| resolve(*n)).collect(),
            output: c.output,
        });
    }
    for p in &nl.ports {
        out.add_port(p.name.clone(), p.dir, p.bits.clone());
    }
    (true, out)
}

/// Remove cells not reachable from output ports while preserving sequential
/// state for name-based equivalence checking and auditable physical netlists.
fn remove_dead(nl: &Netlist, warnings: &mut Vec<String>) -> (bool, Netlist, usize) {
    let mut keep_cell = vec![false; nl.cell_count()];
    let mut stack: Vec<NetId> = Vec::new();
    for (ci, keep) in keep_cell.iter_mut().enumerate() {
        if nl.cell_is_sequential(&nl.cell(ci).kind) {
            *keep = true;
            stack.extend(nl.cell(ci).inputs.iter().copied());
        }
    }
    for p in &nl.ports {
        if p.dir == Dir::Out {
            for &b in &p.bits {
                stack.push(b);
            }
        }
    }
    let mut seen_nets = std::collections::HashSet::new();
    while let Some(n) = stack.pop() {
        if !seen_nets.insert(n) {
            continue;
        }
        for &c in &nl.net(n).drivers {
            if !keep_cell[c] {
                keep_cell[c] = true;
                for &i in &nl.cell(c).inputs {
                    stack.push(i);
                }
            }
        }
    }
    let dead: Vec<usize> = (0..nl.cell_count()).filter(|i| !keep_cell[*i]).collect();
    let n_dead = dead.len();
    if n_dead > 0 {
        warnings.push(format!("removed {} dead cells", n_dead));
    }
    let mut out = nl.clone();
    out.compact(&dead);
    (n_dead > 0, out, n_dead)
}
