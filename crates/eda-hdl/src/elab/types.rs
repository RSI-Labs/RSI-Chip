//! Elaborator state, net/array models, package tables, and type resolution.

use super::const_eval;
use super::const_eval_packages;
use super::substitution::validate_function_body;
use crate::ast;
use eda_core::ir::Design;
use std::collections::HashMap;

#[derive(Clone)]
pub(super) struct NetEntry {
    pub(super) base: usize,
    pub(super) width: usize,
    pub(super) msb: i64,
    pub(super) lsb: i64,
    pub(super) type_name: Option<String>,
    pub(super) is_signed: bool,
    pub(super) array: Option<NetArray>,
    /// Index into `Design::memories` when this declaration has a first-class
    /// sparse-memory object.
    pub(super) memory: Option<usize>,
}

#[derive(Clone)]
pub(super) struct NetArray {
    pub(super) dimensions: Vec<NetArrayDimension>,
    pub(super) length: usize,
    pub(super) element_width: usize,
    pub(super) element_msb: i64,
    pub(super) element_lsb: i64,
    pub(super) dynamic: bool,
    pub(super) dynamic_size_base: usize,
}

#[derive(Clone, Copy)]
pub(super) struct NetArrayDimension {
    pub(super) base: i64,
    pub(super) step: i64,
    pub(super) length: usize,
}

impl NetEntry {
    /// Declaration-relative offset of a source index (handles reversed
    /// declarations like `[0:3]`).
    pub(super) fn offset(&self, index: i64) -> Result<usize, String> {
        let off = if self.msb >= self.lsb {
            index - self.lsb
        } else {
            self.lsb - index
        };
        if off < 0 || off as usize >= self.width {
            return Err(format!(
                "index {} out of range [{}:{}]",
                index, self.msb, self.lsb
            ));
        }
        Ok(off as usize)
    }

    pub(super) fn array_offset(&self, indexes: &[i64]) -> Result<usize, String> {
        let array = self
            .array
            .as_ref()
            .ok_or_else(|| "identifier is not an unpacked array".to_string())?;
        if indexes.len() != array.dimensions.len() {
            return Err(format!(
                "array expects {} index(es), received {}",
                array.dimensions.len(),
                indexes.len()
            ));
        }
        let mut element = 0_usize;
        for (index, dimension) in indexes.iter().zip(&array.dimensions) {
            let delta = index - dimension.base;
            if delta % dimension.step != 0 {
                return Err(format!(
                    "array index {} is not on declaration stride {}",
                    index, dimension.step
                ));
            }
            let offset = (delta / dimension.step) as usize;
            if offset >= dimension.length {
                return Err(format!(
                    "array index {} is out of range; length is {}",
                    index, dimension.length
                ));
            }
            element = element * dimension.length + offset;
        }
        if element >= array.length {
            return Err(format!(
                "array element calculation reached {element}; length is {}",
                array.length
            ));
        }
        Ok(element * array.element_width)
    }

    pub(super) fn array_slice(&self, left: i64, right: i64) -> Result<(usize, usize), String> {
        let array = self
            .array
            .as_ref()
            .ok_or_else(|| "identifier is not an unpacked array".to_string())?;
        if array.dimensions.len() != 1 {
            return Err("unpacked array slices support one-dimensional arrays only".to_string());
        }
        let dimension = &array.dimensions[0];
        let follows_direction = if dimension.step > 0 {
            left <= right
        } else {
            left >= right
        };
        if !follows_direction {
            return Err(format!(
                "array slice [{left}:{right}] does not follow declaration direction with stride {}",
                dimension.step
            ));
        }
        let first = (left - dimension.base) / dimension.step;
        let last = (right - dimension.base) / dimension.step;
        if first < 0 || last < 0 {
            return Err(format!("array slice [{left}:{right}] is out of range"));
        }
        let first = first as usize;
        let last = last as usize;
        if first >= dimension.length || last >= dimension.length {
            return Err(format!(
                "array slice [{left}:{right}] is out of range; length is {}",
                dimension.length
            ));
        }
        let low = first.min(last);
        let high = first.max(last);
        let element_count = (left - right).unsigned_abs() as usize + 1;
        if high - low + 1 != element_count {
            return Err("array slice is not contiguous".to_string());
        }
        Ok((
            low * array.element_width,
            element_count * array.element_width,
        ))
    }
}

/// Stable per-elaboration class ids used for runtime virtual dispatch.
pub(super) fn class_type_ids(classes: &HashMap<String, ast::ClassDef>) -> HashMap<String, u32> {
    let mut names: Vec<&String> = classes.keys().collect();
    names.sort();
    names
        .into_iter()
        .enumerate()
        .map(|(id, name)| (name.clone(), id as u32))
        .collect()
}

pub(super) fn type_id_width(class_count: usize) -> usize {
    (usize::BITS - (class_count.max(1) - 1).leading_zeros()) as usize
}

pub(super) fn type_id_net(handle: &str) -> String {
    format!("__{handle}__tid")
}

pub(super) fn object_id_net(handle: &str) -> String {
    format!("__{handle}__oid")
}

/// Classes whose inheritance chain contains `base` (including itself):
/// every runtime type a `base`-typed handle may hold.
pub(super) fn derived_classes<'a>(
    classes: &'a HashMap<String, ast::ClassDef>,
    base: &str,
) -> Vec<&'a str> {
    let mut out = Vec::new();
    for name in classes.keys() {
        let mut cursor = Some(name.as_str());
        let mut depth_left = 32;
        while let Some(current) = cursor {
            if current == base {
                out.push(name.as_str());
                break;
            }
            if depth_left == 0 {
                break;
            }
            depth_left -= 1;
            cursor = classes.get(current).and_then(|d| d.extends.as_deref());
        }
    }
    out.sort_unstable();
    out
}

/// Resolution of a dotted object path against the class object model.
#[derive(Clone, Debug)]
pub(super) enum ClassPathTarget {
    /// A value property; contains the flattened storage net name.
    Field(String),
    /// A class handle (top-level or nested); contains the allocation
    /// flag net, the storage prefix, and the static class type.
    Handle {
        flag_net: String,
        prefix: String,
        class: String,
    },
}

pub(super) struct Elaborator<'a> {
    pub(super) mods: &'a HashMap<String, &'a ast::Module>,
    pub(super) packages: &'a PackageTables,
    pub(super) file_imports: &'a [ast::Import],
    pub(super) design: Design,
    pub(super) source_dir: Option<std::path::PathBuf>,
    pub(super) depth: usize,
    /// SystemVerilog class definitions visible to the design (module scope,
    /// package scope, and the built-in UVM-lite base classes).
    pub(super) classes: HashMap<String, ast::ClassDef>,
    /// Compile-time serial assigned to each elaborated class construction.
    pub(super) next_object_id: u64,
}

#[derive(Default)]
pub(super) struct PackageTables {
    pub(super) constants: HashMap<String, HashMap<String, i64>>,
    pub(super) types: HashMap<String, HashMap<String, ResolvedType>>,
    pub(super) functions: HashMap<String, HashMap<String, ast::FunctionDef>>,
}

#[derive(Clone, Debug)]
pub(super) struct ResolvedEnumType {
    pub(super) width: usize,
    pub(super) constants: HashMap<String, i64>,
}

#[derive(Clone, Debug)]
pub(super) struct ResolvedStructType {
    pub(super) width: usize,
    pub(super) fields: HashMap<String, (usize, usize)>,
}

#[derive(Clone, Debug)]
pub(super) enum ResolvedType {
    Enum(ResolvedEnumType),
    Struct(ResolvedStructType),
    Union(ResolvedStructType),
}

impl ResolvedType {
    pub(super) fn width(&self) -> usize {
        match self {
            Self::Enum(definition) => definition.width,
            Self::Struct(definition) => definition.width,
            Self::Union(definition) => definition.width,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct FunctionArrayDimension {
    pub(super) base: i64,
    pub(super) step: i64,
    pub(super) length: usize,
}

#[derive(Clone, Debug)]
pub(super) struct FunctionArrayLocal {
    pub(super) dimensions: Vec<FunctionArrayDimension>,
    pub(super) element_width: usize,
    pub(super) element_msb: i64,
    pub(super) element_lsb: i64,
    pub(super) length: usize,
}

impl FunctionArrayLocal {
    pub(super) fn element_index(&self, indexes: &[i64]) -> Result<usize, String> {
        if indexes.len() != self.dimensions.len() {
            return Err(format!(
                "array expects {} index(es), received {}",
                self.dimensions.len(),
                indexes.len()
            ));
        }
        let mut element = 0_usize;
        for (index, dimension) in indexes.iter().zip(&self.dimensions) {
            let delta = index - dimension.base;
            if delta % dimension.step != 0 {
                return Err(format!(
                    "array index {} is not on declaration stride {}",
                    index, dimension.step
                ));
            }
            let offset = (delta / dimension.step) as usize;
            if offset >= dimension.length {
                return Err(format!(
                    "array index {} is out of range; length is {}",
                    index, dimension.length
                ));
            }
            element = element * dimension.length + offset;
        }
        if element >= self.length {
            return Err(format!(
                "array element calculation reached {element}; length is {}",
                self.length
            ));
        }
        Ok(element)
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct FunctionAggregateState {
    pub(super) types: HashMap<String, String>,
    pub(super) arrays: HashMap<String, FunctionArrayLocal>,
    pub(super) array_values: HashMap<String, Vec<ast::Expr>>,
    pub(super) scalar_widths: HashMap<String, usize>,
    pub(super) scalar_ranges: HashMap<String, (i64, i64)>,
}

#[derive(Clone)]
pub(super) enum InterfaceInstanceBinding {
    Single(HashMap<String, NetEntry>),
    Array {
        dimensions: Vec<InterfaceArrayDimension>,
        elements: Vec<HashMap<String, NetEntry>>,
    },
}

#[derive(Clone, Copy)]
pub(super) struct InterfaceArrayDimension {
    pub(super) base: i64,
    pub(super) step: i64,
    pub(super) length: usize,
}

impl InterfaceInstanceBinding {
    pub(super) fn select(&self, indexes: &[i64]) -> Result<&HashMap<String, NetEntry>, String> {
        match self {
            Self::Single(fields) if indexes.is_empty() => Ok(fields),
            Self::Single(_) => Err("scalar interface object is not an array".to_string()),
            Self::Array {
                dimensions,
                elements,
            } => {
                if indexes.len() != dimensions.len() {
                    return Err(format!(
                        "interface array expects {} index(es), received {}",
                        dimensions.len(),
                        indexes.len()
                    ));
                }
                let mut element = 0_usize;
                for (index, dimension) in indexes.iter().zip(dimensions) {
                    let delta = index - dimension.base;
                    if delta % dimension.step != 0 {
                        return Err(format!(
                            "interface array index {} is not on declaration stride {}",
                            index, dimension.step
                        ));
                    }
                    let offset = (delta / dimension.step) as usize;
                    if offset >= dimension.length {
                        return Err(format!(
                            "interface array index {} is out of range; length is {}",
                            index, dimension.length
                        ));
                    }
                    element = element * dimension.length + offset;
                }
                elements.get(element).ok_or_else(|| {
                    format!(
                        "interface array element calculation reached {element}; length is {}",
                        elements.len()
                    )
                })
            }
        }
    }
}

pub(super) fn resolve_packages(packages: &[ast::Package]) -> Result<PackageTables, String> {
    let mut out = PackageTables::default();
    for package in packages {
        if out.constants.contains_key(&package.name) {
            return Err(format!("duplicate package '{}'", package.name));
        }
        let mut values = HashMap::new();
        for (name, expression) in &package.params {
            if values.contains_key(name) {
                return Err(format!(
                    "package '{}': duplicate parameter '{}'",
                    package.name, name
                ));
            }
            let value = package_const_eval(expression, &out, &package.name, &values)?;
            values.insert(name.clone(), value);
        }
        let mut types = HashMap::new();
        let mut functions = HashMap::new();
        for definition in &package.types {
            let resolved = resolve_type(definition, &values, &package.name, &types)?;
            if let ResolvedType::Enum(enum_definition) = &resolved {
                for (constant, value) in &enum_definition.constants {
                    values.insert(constant.clone(), *value);
                }
            }
            let type_name = match definition {
                ast::TypeDef::Enum(definition) => &definition.name,
                ast::TypeDef::Struct(definition) => &definition.name,
                ast::TypeDef::Union(definition) => &definition.name,
            };
            if types.contains_key(type_name) {
                return Err(format!(
                    "package '{}': duplicate type '{}'",
                    package.name, type_name
                ));
            }
            types.insert(type_name.clone(), resolved);
        }
        for definition in &package.functions {
            if functions.contains_key(&definition.name) {
                return Err(format!(
                    "package '{}': duplicate function '{}'",
                    package.name, definition.name
                ));
            }
            validate_function_body(definition)?;
            functions.insert(definition.name.clone(), definition.clone());
        }
        out.constants.insert(package.name.clone(), values);
        out.types.insert(package.name.clone(), types);
        out.functions.insert(package.name.clone(), functions);
    }
    Ok(out)
}

pub(super) fn resolve_enum_type(
    definition: &ast::EnumTypeDef,
    params: &HashMap<String, i64>,
    module_name: &str,
) -> Result<ResolvedEnumType, String> {
    let width = definition
        .range
        .as_ref()
        .map(|(msb, lsb)| -> Result<usize, String> {
            let hi = const_eval(msb, params, module_name)?;
            let lo = const_eval(lsb, params, module_name)?;
            Ok((hi - lo).unsigned_abs() as usize + 1)
        })
        .transpose()?
        .unwrap_or(32);
    if width == 0 || width > 64 {
        return Err(format!(
            "enum typedef '{}' width must be between 1 and 64",
            definition.name
        ));
    }
    let mut constants = HashMap::new();
    let mut next = 0_i64;
    for (name, value) in &definition.constants {
        let explicit = value.is_some();
        let value = match value {
            Some(expression) => const_eval(expression, params, module_name)?,
            None => next,
        };
        if explicit && (value < 0 || (value as u64) >= (1_u64 << width)) {
            return Err(format!(
                "enum constant '{}::{}' value {} does not fit width {}",
                definition.name, name, value, width
            ));
        }
        let mask: u64 = if width == 64 {
            u64::MAX
        } else {
            (1_u64 << width) - 1
        };
        let stored = value & mask as i64;
        constants.insert(name.clone(), stored);
        next = stored.wrapping_add(1) & mask as i64;
    }
    Ok(ResolvedEnumType { width, constants })
}

pub(super) fn resolve_type(
    definition: &ast::TypeDef,
    params: &HashMap<String, i64>,
    module_name: &str,
    existing_types: &HashMap<String, ResolvedType>,
) -> Result<ResolvedType, String> {
    match definition {
        ast::TypeDef::Enum(definition) => Ok(ResolvedType::Enum(resolve_enum_type(
            definition,
            params,
            module_name,
        )?)),
        ast::TypeDef::Struct(definition) => Ok(ResolvedType::Struct(resolve_struct_type(
            definition,
            params,
            module_name,
            existing_types,
        )?)),
        ast::TypeDef::Union(definition) => Ok(ResolvedType::Union(resolve_union_type(
            definition,
            params,
            module_name,
            existing_types,
        )?)),
    }
}

pub(super) fn resolve_struct_type(
    definition: &ast::StructTypeDef,
    params: &HashMap<String, i64>,
    module_name: &str,
    existing_types: &HashMap<String, ResolvedType>,
) -> Result<ResolvedStructType, String> {
    let mut field_widths = Vec::new();
    let mut width = 0_usize;
    for field in &definition.fields {
        let field_width = if let Some((msb, lsb)) = &field.range {
            let hi = const_eval(msb, params, module_name)?;
            let lo = const_eval(lsb, params, module_name)?;
            (hi - lo).unsigned_abs() as usize + 1
        } else {
            let type_name = field
                .type_name
                .as_deref()
                .ok_or_else(|| format!("struct field '{}' has no width", field.name))?;
            existing_types
                .get(type_name)
                .map(ResolvedType::width)
                .ok_or_else(|| {
                    format!(
                        "struct typedef '{}': field '{}' uses unknown type '{}'",
                        definition.name, field.name, type_name
                    )
                })?
        };
        if field_width == 0 || width + field_width > 4096 {
            return Err(format!(
                "struct typedef '{}' exceeds the packed-width limit",
                definition.name
            ));
        }
        width += field_width;
        field_widths.push(field_width);
    }
    let mut fields = HashMap::new();
    let mut consumed = 0_usize;
    for (field, field_width) in definition.fields.iter().zip(field_widths) {
        let offset = width - consumed - field_width;
        fields.insert(field.name.clone(), (offset, field_width));
        consumed += field_width;
    }
    Ok(ResolvedStructType { width, fields })
}

pub(super) fn resolve_union_type(
    definition: &ast::StructTypeDef,
    params: &HashMap<String, i64>,
    module_name: &str,
    existing_types: &HashMap<String, ResolvedType>,
) -> Result<ResolvedStructType, String> {
    let mut fields = HashMap::new();
    let mut width = 0_usize;
    for field in &definition.fields {
        let field_width = if let Some((msb, lsb)) = &field.range {
            let hi = const_eval(msb, params, module_name)?;
            let lo = const_eval(lsb, params, module_name)?;
            (hi - lo).unsigned_abs() as usize + 1
        } else {
            let type_name = field
                .type_name
                .as_deref()
                .ok_or_else(|| format!("union field '{}' has no width", field.name))?;
            existing_types
                .get(type_name)
                .map(ResolvedType::width)
                .ok_or_else(|| {
                    format!(
                        "union typedef '{}': field '{}' uses unknown type '{}'",
                        definition.name, field.name, type_name
                    )
                })?
        };
        if field_width == 0 || field_width > 4096 {
            return Err(format!(
                "union typedef '{}' exceeds the packed-width limit",
                definition.name
            ));
        }
        width = width.max(field_width);
        fields.insert(field.name.clone(), (0, field_width));
    }
    Ok(ResolvedStructType { width, fields })
}

pub(super) fn package_const_eval(
    expression: &ast::Expr,
    packages: &PackageTables,
    current_package: &str,
    current_values: &HashMap<String, i64>,
) -> Result<i64, String> {
    match expression {
        ast::Expr::ScopedIdent { package, name } => packages
            .constants
            .get(package)
            .and_then(|values| values.get(name))
            .copied()
            .ok_or_else(|| format!("unknown package constant '{}::{}'", package, name)),
        ast::Expr::Ident(name) if current_values.contains_key(name) => Ok(current_values[name]),
        other => const_eval_packages(other, current_values, current_package, packages),
    }
}
