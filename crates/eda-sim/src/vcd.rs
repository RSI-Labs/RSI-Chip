//! VCD (Value Change Dump) waveform writer.

use eda_core::ir::Design;
use eda_core::logic::LogicValue;
use std::collections::HashSet;

pub struct VcdWriter {
    header: String,
    body: String,
    last_time: Option<u64>,
    emitted_initial: bool,
    decls: Vec<DeclInfo>,
    prev: Vec<Vec<LogicValue>>,
}

struct DeclInfo {
    name: String,
    base: usize,
    width: usize,
    id: String,
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
        let mut header = String::new();
        header.push_str("$date\n  today\n$end\n");
        header.push_str("$version\n  eda-sim 0.1\n$end\n");
        header.push_str("$timescale 1ns $end\n");
        header.push_str("$scope module top $end\n");
        for d in &decls {
            if d.width == 1 {
                header.push_str(&format!("$var wire 1 {} {} $end\n", d.id, d.name));
            } else {
                header.push_str(&format!(
                    "$var wire {} {} {} [{}:0] $end\n",
                    d.width,
                    d.id,
                    d.name,
                    d.width - 1
                ));
            }
        }
        header.push_str("$upscope $end\n");
        header.push_str("$enddefinitions $end\n");
        decls.shrink_to_fit();
        VcdWriter {
            header,
            body: String::new(),
            last_time: None,
            emitted_initial: false,
            decls,
            prev: Vec::new(),
        }
    }

    /// Record changes at time `t`. `changed` holds declaration indices whose
    /// bits changed since the previous flush.
    pub fn advance(&mut self, t: u64, vals: &[LogicValue], changed: &HashSet<usize>) {
        if !self.emitted_initial {
            self.last_time = Some(t);
            self.body.push_str(&format!("#{}\n", t));
            self.prev = self.decls.iter().map(|d| read_decl(vals, d)).collect();
            for (di, d) in self.decls.iter().enumerate() {
                self.body.push_str(&dump_value(&self.prev[di], &d.id));
            }
            self.emitted_initial = true;
            return;
        }
        if changed.is_empty() {
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
    }

    pub fn render(&self, vals: &[LogicValue]) -> String {
        let mut out = self.header.clone();
        out.push_str(&self.body);
        if !self.emitted_initial {
            out.push_str("#0\n");
            for d in &self.decls {
                let cur = read_decl(vals, d);
                out.push_str(&dump_value(&cur, &d.id));
            }
        }
        out
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
