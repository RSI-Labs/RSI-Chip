//! Shared sparse-memory contents and write-contract models.

use eda_core::ir::{Design, Expr, Stmt};
use eda_core::logic::LogicWord;
use eda_core::netlist::NetId;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

#[derive(Clone, Default)]
pub(crate) struct RomContents {
    pub(crate) values: BTreeMap<Vec<i64>, LogicWord>,
    pub(crate) deterministic: bool,
}

#[derive(Clone)]
pub(crate) struct BlockRamLowering {
    pub(crate) address_width: usize,
    pub(crate) initial: Arc<Vec<LogicWord>>,
}

impl RomContents {
    pub(crate) fn deterministic() -> Self {
        Self {
            values: BTreeMap::new(),
            deterministic: true,
        }
    }
}

pub(crate) fn rom_contents_from_design(design: &Design) -> HashMap<usize, RomContents> {
    let mut contents = HashMap::<usize, RomContents>::new();
    for statement in &design.initials {
        collect_rom_initializer(statement, &mut contents);
    }
    contents
}

pub(crate) fn collect_rom_initializer(
    statement: &Stmt,
    contents: &mut HashMap<usize, RomContents>,
) {
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

pub(crate) fn constant_index(expression: &Expr) -> Option<i64> {
    constant_word(expression)?
        .to_u64()
        .map(|value| value as i64)
}

pub(crate) fn constant_word(expression: &Expr) -> Option<LogicWord> {
    match expression {
        Expr::Const(word) => Some(word.clone()),
        Expr::Signed { e, .. } => constant_word(e),
        _ => None,
    }
}

pub(crate) fn physical_to_logical(
    memory: &eda_core::ir::MemoryDecl,
    physical_index: u64,
) -> Option<Vec<i64>> {
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

pub(crate) fn part_value_index(
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
pub(crate) struct DistributedRamWrite {
    pub(crate) memory: usize,
    pub(crate) condition: Option<Expr>,
    pub(crate) indexes: Vec<Expr>,
    pub(crate) value: Expr,
    pub(crate) part: Option<DistributedRamWritePart>,
}

#[derive(Clone)]
pub(crate) struct DistributedRamWritePart {
    pub(crate) start: Expr,
    pub(crate) width: usize,
    pub(crate) ascending: bool,
}

pub(crate) struct DistributedRamPartTarget {
    pub(crate) low: usize,
    pub(crate) high: usize,
    pub(crate) start_select: Option<NetId>,
}
