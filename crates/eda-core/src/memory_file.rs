//! Shared `$readmemh`/`$readmemb` data-file parsing.

use crate::ir::MemoryDecl;
use crate::logic::LogicWord;
use std::collections::BTreeMap;
use std::path::Path;

pub fn read_memory_file(
    path: &Path,
    radix: u32,
    element_width: usize,
) -> Result<BTreeMap<u64, LogicWord>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read sparse-memory file {}: {error}", path.display()))?;
    let text = strip_memory_file_comments(&text);
    let mut values = BTreeMap::new();
    let mut address = 0_u64;
    for token in text.split_whitespace() {
        if let Some(address_token) = token.strip_prefix('@') {
            address = u64::from_str_radix(address_token, 16).map_err(|_| {
                format!(
                    "invalid hexadecimal address '@{address_token}' in {}",
                    path.display()
                )
            })?;
            continue;
        }
        let prefix = if radix == 16 { "'h" } else { "'b" };
        let word =
            LogicWord::parse(element_width, &format!("{prefix}{token}")).ok_or_else(|| {
                format!(
                    "invalid base-{radix} memory token '{token}' in {}",
                    path.display()
                )
            })?;
        values.insert(address, word);
        address = address
            .checked_add(1)
            .ok_or_else(|| "sparse-memory file address overflow".to_string())?;
    }
    Ok(values)
}

pub fn strip_memory_file_comments(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '/' && characters.peek() == Some(&'/') {
            characters.next();
            for nested in characters.by_ref() {
                if nested == '\n' {
                    output.push('\n');
                    break;
                }
            }
        } else if character == '/' && characters.peek() == Some(&'*') {
            characters.next();
            let mut previous = '\0';
            for nested in characters.by_ref() {
                if previous == '*' && nested == '/' {
                    break;
                }
                previous = nested;
            }
            output.push(' ');
        } else {
            output.push(character);
        }
    }
    output
}

pub fn physical_to_logical_indexes(
    memory: &MemoryDecl,
    physical_index: u64,
) -> Result<Vec<i64>, String> {
    if physical_index >= memory.flat_length {
        return Err(format!(
            "sparse-memory file address {physical_index} is out of range"
        ));
    }
    let mut remaining = physical_index;
    let mut stride = memory.flat_length;
    let mut indexes = Vec::with_capacity(memory.dimensions.len());
    for dimension in &memory.dimensions {
        stride /= dimension.length as u64;
        let offset = remaining / stride % dimension.length as u64;
        remaining %= stride;
        let logical = dimension.base + dimension.step * offset as i64;
        if logical < 0 {
            return Err("negative sparse-memory file indexes are not yet lowered".to_string());
        }
        indexes.push(logical);
    }
    Ok(indexes)
}
