//! VCD (Value Change Dump) waveform writer.

use super::MemoryElementStorage;
use eda_core::ir::Design;
use eda_core::logic::LogicValue;
use std::collections::{HashMap, HashSet};

pub struct VcdWriter {
    header_prefix: String,
    memory_var_header: String,
    header_suffix: String,
    body: String,
    last_time: Option<u64>,
    emitted_initial: bool,
    decls: Vec<DeclInfo>,
    prev: Vec<Vec<LogicValue>>,
    memory_decls: Vec<MemoryDeclInfo>,
    memory_lookup: HashMap<(usize, u64), usize>,
    memory_prev: Vec<Vec<LogicValue>>,
    memory_meta: Vec<MemoryMeta>,
    next_memory_id: usize,
}

struct DeclInfo {
    name: String,
    base: usize,
    width: usize,
    id: String,
}

struct MemoryDeclInfo {
    memory: usize,
    address: u64,
    name: String,
    width: usize,
    id: String,
}

struct MemoryMeta {
    name: String,
    width: usize,
}

impl VcdWriter {
    pub fn new(design: &Design) -> Self {
        let mut decls: Vec<DeclInfo> = design
            .net_decls
            .iter()
            .enumerate()
            .map(|(i, d)| DeclInfo {
                name: d.name.clone(),
                base: d.base,
                width: d.width,
                id: id_code(i),
            })
            .collect();
        let memory_meta = design
            .memories
            .iter()
            .map(|memory| MemoryMeta {
                name: memory.name.clone(),
                width: memory.element_width,
            })
            .collect();
        let mut header_prefix = String::new();
        header_prefix.push_str("$date\n  today\n$end\n");
        header_prefix.push_str("$version\n  eda-sim 0.1\n$end\n");
        header_prefix.push_str(&format!(
            "$timescale 1{} $end\n",
            vcd_time_unit(design.time_scale.precision_exponent)
        ));
        header_prefix.push_str("$scope module top $end\n");
        for d in &decls {
            if d.width == 1 {
                header_prefix.push_str(&format!("$var wire 1 {} {} $end\n", d.id, d.name));
            } else {
                header_prefix.push_str(&format!(
                    "$var wire {} {} {} [{}:0] $end\n",
                    d.width,
                    d.id,
                    d.name,
                    d.width - 1
                ));
            }
        }
        let header_suffix = "$upscope $end\n$enddefinitions $end\n".to_string();
        decls.shrink_to_fit();
        let next_memory_id = decls.len();
        VcdWriter {
            header_prefix,
            memory_var_header: String::new(),
            header_suffix,
            body: String::new(),
            last_time: None,
            emitted_initial: false,
            decls,
            prev: Vec::new(),
            memory_decls: Vec::new(),
            memory_lookup: HashMap::new(),
            memory_prev: Vec::new(),
            memory_meta,
            next_memory_id,
        }
    }

    /// Record changes at time `t`. `changed` holds declaration indices whose
    /// bits changed since the previous flush.
    pub(crate) fn advance(
        &mut self,
        t: u64,
        vals: &[LogicValue],
        changed: &HashSet<usize>,
        memory_elements: &[MemoryElementStorage],
        memory_changes: &HashSet<(usize, u64)>,
    ) {
        self.ensure_memory_words(memory_elements, memory_changes);
        if !self.emitted_initial {
            self.last_time = Some(t);
            self.body.push_str(&format!("#{}\n", t));
            self.prev = self.decls.iter().map(|d| read_decl(vals, d)).collect();
            for (di, d) in self.decls.iter().enumerate() {
                self.body.push_str(&dump_value(&self.prev[di], &d.id));
            }
            for (di, d) in self.memory_decls.iter().enumerate() {
                self.body
                    .push_str(&dump_value(&self.memory_prev[di], &d.id));
            }
            self.emitted_initial = true;
            return;
        }
        if changed.is_empty() && memory_changes.is_empty() {
            return;
        }
        if self.last_time != Some(t) {
            self.body.push_str(&format!("#{}\n", t));
            self.last_time = Some(t);
        }
        for &di in changed {
            if di >= self.decls.len() {
                continue;
            }
            let d = &self.decls[di];
            let cur = read_decl(vals, d);
            if cur != self.prev[di] {
                self.body.push_str(&dump_value(&cur, &d.id));
                self.prev[di] = cur;
            }
        }
        for &(memory, address) in memory_changes {
            let Some(&di) = self.memory_lookup.get(&(memory, address)) else {
                continue;
            };
            let cur = read_memory(
                memory_elements,
                memory,
                address,
                self.memory_decls[di].width,
            );
            if cur != self.memory_prev[di] {
                let id = self.memory_decls[di].id.clone();
                self.body.push_str(&dump_value(&cur, &id));
                self.memory_prev[di] = cur;
            }
        }
    }

    pub(crate) fn render(
        &mut self,
        vals: &[LogicValue],
        memory_elements: &[MemoryElementStorage],
    ) -> String {
        let all_memory_changes = memory_elements
            .iter()
            .enumerate()
            .flat_map(|(memory, elements)| {
                elements
                    .touched_addresses()
                    .into_iter()
                    .map(move |address| (memory, address))
            })
            .collect::<HashSet<_>>();
        self.ensure_memory_words(memory_elements, &all_memory_changes);
        let mut out = self.header();
        out.push_str(&self.body);
        if !self.emitted_initial {
            out.push_str("#0\n");
            for d in &self.decls {
                let cur = read_decl(vals, d);
                out.push_str(&dump_value(&cur, &d.id));
            }
            for (di, d) in self.memory_decls.iter().enumerate() {
                let cur = read_memory(memory_elements, d.memory, d.address, d.width);
                out.push_str(&dump_value(&cur, &d.id));
                self.memory_prev[di] = cur;
            }
        }
        out
    }

    fn header(&self) -> String {
        let mut out = self.header_prefix.clone();
        out.push_str(&self.memory_var_header);
        out.push_str(&self.header_suffix);
        out
    }

    fn ensure_memory_words(
        &mut self,
        memory_elements: &[MemoryElementStorage],
        changes: &HashSet<(usize, u64)>,
    ) {
        for &(memory, address) in changes {
            if self.memory_lookup.contains_key(&(memory, address)) {
                continue;
            }
            let Some(elements) = memory_elements.get(memory) else {
                continue;
            };
            let Some(value) = elements.get(address) else {
                continue;
            };
            let Some(declaration) = self.memory_meta.get(memory) else {
                continue;
            };
            let id = id_code(self.next_memory_id);
            self.next_memory_id += 1;
            let info = MemoryDeclInfo {
                memory,
                address,
                name: format!("{}[{}]", declaration.name, address),
                width: declaration.width,
                id: id.clone(),
            };
            if info.width == 1 {
                self.memory_var_header
                    .push_str(&format!("$var reg 1 {} {} $end\n", info.id, info.name));
            } else {
                self.memory_var_header.push_str(&format!(
                    "$var reg {} {} {} [{}:0] $end\n",
                    info.width,
                    info.id,
                    info.name,
                    info.width - 1
                ));
            }
            let index = self.memory_decls.len();
            self.memory_lookup.insert((memory, address), index);
            self.memory_decls.push(info);
            self.memory_prev
                .push(vec![LogicValue::X; self.memory_decls[index].width]);
            let _ = value;
        }
    }
}

fn vcd_time_unit(exponent: i8) -> &'static str {
    match exponent {
        0 => "s",
        -3 => "ms",
        -6 => "us",
        -12 => "ps",
        -15 => "fs",
        _ => "ns",
    }
}

fn read_decl(vals: &[LogicValue], d: &DeclInfo) -> Vec<LogicValue> {
    (0..d.width)
        .map(|i| vals.get(d.base + i).copied().unwrap_or(LogicValue::X))
        .collect()
}

fn dump_value(bits: &[LogicValue], id: &str) -> String {
    if bits.len() == 1 {
        format!("{}{}\n", bits[0].to_char(), id)
    } else {
        let s: String = bits.iter().rev().map(|b| b.to_char()).collect();
        format!("b{} {}\n", s, id)
    }
}

fn read_memory(
    memory_elements: &[MemoryElementStorage],
    memory: usize,
    address: u64,
    width: usize,
) -> Vec<LogicValue> {
    memory_elements
        .get(memory)
        .and_then(|elements| elements.get(address))
        .map(|word| (0..width).map(|bit| word.bit(bit)).collect())
        .unwrap_or_else(|| vec![LogicValue::X; width])
}

fn id_code(mut n: usize) -> String {
    // base-94 over printable ASCII 33..=126
    let mut s = String::new();
    loop {
        s.insert(0, char::from_u32(33 + (n % 94) as u32).unwrap());
        n /= 94;
        if n == 0 {
            break;
        }
        n -= 1;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_codes_unique_and_printable() {
        let mut seen = std::collections::HashSet::new();
        for i in 0..500 {
            let c = id_code(i);
            assert!(c.chars().all(|ch| (33..=126).contains(&(ch as u32))));
            assert!(seen.insert(c), "duplicate id for {}", i);
        }
        assert_eq!(id_code(0), "!");
        assert_eq!(id_code(1), "\"");
    }

    #[test]
    fn dump_value_formats() {
        let b = vec![LogicValue::One, LogicValue::Zero];
        assert_eq!(dump_value(&b, "x"), "b01 x\n");
        let s = vec![LogicValue::X];
        assert_eq!(dump_value(&s, "!"), "x!\n");
    }
}
