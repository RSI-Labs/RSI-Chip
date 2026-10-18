//! Packed scalar/vector and sparse-memory type parsing for slang AST JSON.

use eda_core::ir::MemoryDimension;

pub(super) const STRING_WIDTH: usize = 64 * 8;

#[derive(Clone, Copy)]
pub(super) struct AstPackedDimension {
    pub msb: i64,
    pub lsb: i64,
    pub length: usize,
    /// Width remaining after this outer dimension, including all inner packed
    /// dimensions. Selecting one element of this dimension has this width.
    pub element_width: usize,
}

#[derive(Clone)]
pub(super) struct AstTypeShape {
    pub width: usize,
    pub bounds: Option<(i64, i64)>,
    pub dimensions: Vec<AstPackedDimension>,
}

pub(super) struct AstMemoryType {
    pub element_width: usize,
    pub element_msb: i64,
    pub element_lsb: i64,
    pub dimensions: Vec<MemoryDimension>,
    pub flat_length: u64,
    pub signed: bool,
}

pub(super) fn simple_type_signed(type_name: &str) -> Option<bool> {
    simple_type_width(type_name).map(|_| {
        let tokens = type_name
            .split('[')
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>();
        if tokens.contains(&"signed") {
            true
        } else if tokens.contains(&"unsigned") {
            false
        } else {
            // byte, shortint, int, integer, longint, and time are signed by
            // definition; logic, reg, and bit are unsigned.
            matches!(
                tokens.last().copied(),
                Some("byte" | "shortint" | "int" | "integer" | "longint" | "time")
            )
        }
    })
}

fn parse_type_range(text: &str) -> Option<(i64, i64)> {
    let text = text.trim();
    let start = text.rfind('[')?;
    if !text.ends_with(']') || start + 1 >= text.len() - 1 {
        return None;
    }
    let (left, right) = text[start + 1..text.len() - 1].split_once(':')?;
    Some((
        left.trim().parse::<i64>().ok()?,
        right.trim().parse::<i64>().ok()?,
    ))
}

pub(super) fn memory_type(type_name: &str) -> Option<AstMemoryType> {
    let separator = type_name.find('$')?;
    let element_type = type_name[..separator].trim();
    let element = simple_type_shape(element_type)?;
    memory_type_with_element(
        type_name,
        element.width,
        element.bounds?,
        simple_type_signed(element_type)?,
    )
}

pub(super) fn memory_type_with_element(
    type_name: &str,
    element_width: usize,
    element_bounds: (i64, i64),
    signed: bool,
) -> Option<AstMemoryType> {
    let separator = type_name.find('$')?;
    let dimension_type = type_name[separator + 1..].trim();
    let (element_msb, element_lsb) = element_bounds;
    let mut dimensions = Vec::new();
    let mut remaining = dimension_type;
    while !remaining.is_empty() {
        remaining = remaining.trim_start();
        if !remaining.starts_with('[') {
            return None;
        }
        let end = remaining.find(']')?;
        let (base, last) = parse_type_range(&remaining[..=end])?;
        let step = if base <= last { 1 } else { -1 };
        let length = usize::try_from((base as i128 - last as i128).unsigned_abs() + 1).ok()?;
        dimensions.push(MemoryDimension { base, step, length });
        remaining = &remaining[end + 1..];
    }
    if dimensions.is_empty() {
        return None;
    }
    let mut flat_length = 1_u64;
    for dimension in &dimensions {
        flat_length = flat_length.checked_mul(u64::try_from(dimension.length).ok()?)?;
    }
    Some(AstMemoryType {
        element_width,
        element_msb,
        element_lsb,
        dimensions,
        flat_length,
        signed,
    })
}

pub(super) fn simple_type_shape(type_name: &str) -> Option<AstTypeShape> {
    let type_name = type_name.trim();
    let base = type_name.split('[').next()?.trim();
    let event_array = type_name.starts_with("event$")
        && type_name["event$".len()..].trim_start().starts_with('[');
    if base == "event" || event_array {
        let ranges = explicit_type_ranges(type_name)?;
        if ranges.is_empty() {
            return Some(AstTypeShape {
                width: 1,
                bounds: Some((0, 0)),
                dimensions: Vec::new(),
            });
        }
        let mut dimensions = Vec::<AstPackedDimension>::new();
        let mut width = 1_usize;
        for (left, right) in ranges.iter().rev() {
            let range_width =
                usize::try_from(((*left as i128) - (*right as i128)).unsigned_abs() + 1).ok()?;
            dimensions.push(AstPackedDimension {
                msb: *left,
                lsb: *right,
                length: range_width,
                element_width: width,
            });
            width = width.checked_mul(range_width)?;
        }
        dimensions.reverse();
        let bounds = match ranges.as_slice() {
            [only] => Some(*only),
            _ => None,
        };
        return Some(AstTypeShape {
            width,
            bounds,
            dimensions,
        });
    }
    if type_name.contains('$')
        || type_name.contains('{')
        || type_name.contains("class")
        || type_name.contains("iterator")
    {
        return None;
    }
    let element_width = if base.contains("logic") || base.contains("reg") || base.contains("bit") {
        1
    } else if base == "event" {
        return Some(AstTypeShape {
            width: 1,
            bounds: Some((0, 0)),
            dimensions: Vec::new(),
        });
    } else if base == "string" {
        return Some(AstTypeShape {
            width: STRING_WIDTH,
            bounds: Some((STRING_WIDTH as i64 - 1, 0)),
            dimensions: Vec::new(),
        });
    } else {
        let data_type = base
            .split_whitespace()
            .rev()
            .find(|token| !matches!(*token, "signed" | "unsigned"))?;
        match data_type {
            "byte" => 8,
            "shortint" => 16,
            "int" | "integer" => 32,
            "longint" => 64,
            "time" => 64,
            _ => return None,
        }
    };
    let ranges = explicit_type_ranges(type_name)?;
    let mut dimensions = Vec::<AstPackedDimension>::new();
    let mut suffix_width: usize = element_width;
    for (left, right) in ranges.iter().rev() {
        let range_width =
            usize::try_from(((*left as i128) - (*right as i128)).unsigned_abs() + 1).ok()?;
        dimensions.push(AstPackedDimension {
            msb: *left,
            lsb: *right,
            length: range_width,
            element_width: suffix_width,
        });
        suffix_width = suffix_width.checked_mul(range_width)?;
    }
    dimensions.reverse();
    let width = suffix_width;
    let bounds = match ranges.as_slice() {
        [] => Some((width as i64 - 1, 0)),
        [only] => Some(*only),
        _ => None,
    };
    Some(AstTypeShape {
        width,
        bounds,
        dimensions,
    })
}

fn explicit_type_ranges(type_name: &str) -> Option<Vec<(i64, i64)>> {
    let mut ranges = Vec::new();
    let bytes = type_name.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'[' {
            let end = (index + 1..bytes.len()).find(|position| bytes[*position] == b']')?;
            let range = type_name.get(index + 1..end)?;
            if range.contains('[') || range.contains(']') {
                return None;
            }
            let (left, right) = range.split_once(':')?;
            ranges.push((
                left.trim().parse::<i64>().ok()?,
                right.trim().parse::<i64>().ok()?,
            ));
            index = end + 1;
        } else if bytes[index].is_ascii_whitespace() {
            index += 1;
        } else if index > 0 && ranges.is_empty() {
            // The text before the first bracket is the base type and has
            // already been validated by `simple_type_shape`.
            index += 1;
        } else if index > 0 && !ranges.is_empty() && bytes[index].is_ascii_alphanumeric() {
            // Allow only whitespace between packed dimensions.
            return None;
        } else {
            index += 1;
        }
    }
    Some(ranges)
}

pub(super) fn simple_type_width(type_name: &str) -> Option<usize> {
    simple_type_shape(type_name).map(|shape| shape.width)
}
