//! Elaborator: flattens the module hierarchy into a single [`Design`],
//! resolving parameters, ranges, port directions and hierarchical names.

use crate::ast;
use crate::parser;
use eda_core::ir::{
    Assign, BinOp, Design, Dir, DispPart as DispPartIR, Expr as ExprIR, HierarchicalInstance,
    HierarchicalPort, LValue, MemoryBanking, MemoryConnection, MemoryDecl, MemoryDimension,
    MemoryKind, MemoryStorage, MemoryTemplate, NetDecl, PortDecl, Process, Range, Sens as SensIR,
    Stmt as StmtIR, UnOp,
};
use eda_core::logic::{LogicValue, LogicWord};
use std::collections::{HashMap, HashSet};

mod arrays;
mod class_elab;
mod classes;
mod expressions;
mod functions;
mod modules;
mod statements;
mod strings;
mod substitution;
mod types;

use classes::{
    collect_classes, ctor_body_calls_super, is_cast_assignment, BUILTIN_UVM_CLASS_NAMES,
};
use substitution::*;
use types::*;

/// Parse + elaborate in one step.
pub fn compile(source: &str, top: Option<&str>) -> Result<Design, String> {
    compile_with_source_dir(source, top, None)
}

/// Parses and elaborates with a base directory for compile-time file tasks.
pub fn compile_with_source_dir(
    source: &str,
    top: Option<&str>,
    source_dir: Option<&std::path::Path>,
) -> Result<Design, String> {
    let file = parser::parse(source)?;
    elaborate_with_source_dir(&file, top, source_dir)
}

/// A named compilation unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HdlSource {
    pub name: String,
    pub source: String,
}

impl HdlSource {
    pub fn new(name: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source: source.into(),
        }
    }
}

#[derive(Clone, Debug)]
struct SourceLineRange {
    name: String,
    first_line: usize,
    last_line: usize,
}

/// Maps lines in a deterministic concatenation back to their source files.
///
/// This is the first source-manager boundary for the internal frontend. The
/// full slang frontend remains responsible for include/macro expansion and its
/// original-file diagnostics; this manager at least prevents multi-unit
/// syntax errors from reporting only an anonymous concatenated line number.
#[derive(Clone, Debug)]
pub struct SourceManager {
    sources: Vec<HdlSource>,
    combined: String,
    ranges: Vec<SourceLineRange>,
}

impl SourceManager {
    pub fn new(sources: &[HdlSource]) -> Self {
        let mut lines = Vec::<&str>::new();
        let mut ranges = Vec::new();
        for source in sources {
            if !lines.is_empty() {
                // Keep a deterministic blank separator that belongs to no file.
                lines.push("");
            }
            let first_line = lines.len() + 1;
            let source_lines: Vec<_> = source.source.lines().collect();
            let line_count = source_lines.len().max(1);
            lines.extend(source_lines);
            ranges.push(SourceLineRange {
                name: source.name.clone(),
                first_line,
                last_line: first_line + line_count - 1,
            });
        }
        Self {
            sources: sources.to_vec(),
            combined: lines.join("\n"),
            ranges,
        }
    }

    pub fn sources(&self) -> &[HdlSource] {
        &self.sources
    }

    pub fn combined(&self) -> &str {
        &self.combined
    }

    pub fn locate(&self, line: usize) -> Option<(&str, usize)> {
        self.ranges
            .iter()
            .find(|range| line >= range.first_line && line <= range.last_line)
            .map(|range| (range.name.as_str(), line - range.first_line + 1))
    }
}

/// Parse and elaborate multiple named compilation units.
pub fn compile_sources(sources: &[HdlSource], top: Option<&str>) -> Result<Design, String> {
    let manager = SourceManager::new(sources);
    let file =
        parser::parse(manager.combined()).map_err(|error| remap_parse_error(&error, &manager))?;
    let source_dir = sources
        .first()
        .map(|source| std::path::Path::new(&source.name));
    elaborate_with_source_dir(&file, top, source_dir)
}

fn remap_parse_error(error: &str, manager: &SourceManager) -> String {
    let Some(line_marker) = error.find("line ") else {
        return error.to_string();
    };
    let digits_start = line_marker + "line ".len();
    let digits_end = error[digits_start..]
        .find(|character: char| !character.is_ascii_digit())
        .map(|offset| digits_start + offset)
        .unwrap_or(error.len());
    let Ok(line) = error[digits_start..digits_end].parse::<usize>() else {
        return error.to_string();
    };
    match manager.locate(line) {
        Some((file, file_line)) => {
            let error = error.replacen(&format!("line {line}"), &format!("line {file_line}"), 1);
            format!("{file}:{file_line}: {error}")
        }
        None => error.to_string(),
    }
}

pub fn elaborate(file: &ast::SourceFile, top: Option<&str>) -> Result<Design, String> {
    elaborate_with_source_dir(file, top, None)
}

pub fn elaborate_with_source_dir(
    file: &ast::SourceFile,
    top: Option<&str>,
    source_dir: Option<&std::path::Path>,
) -> Result<Design, String> {
    let mut mods: HashMap<String, &ast::Module> = HashMap::new();
    for m in &file.modules {
        if mods.insert(m.name.clone(), m).is_some() {
            return Err(format!("duplicate module '{}'", m.name));
        }
    }
    let top_name = match top {
        Some(t) => {
            if !mods.contains_key(t) {
                return Err(format!("top module '{}' not found", t));
            }
            t.to_string()
        }
        None => pick_top(file)?,
    };
    let mut el = Elaborator {
        mods: &mods,
        packages: &resolve_packages(&file.packages)?,
        file_imports: &file.imports,
        design: Design {
            name: top_name.clone(),
            ..Default::default()
        },
        source_dir: source_dir.map(std::path::Path::to_path_buf),
        depth: 0,
        classes: collect_classes(file)?,
        next_object_id: 1,
    };
    el.elab_module(&top_name, "", &HashMap::new(), &HashMap::new())?;
    el.design.infer_memory_kinds();
    el.design
        .hierarchy
        .sort_by_key(|instance| (instance.depth, instance.path.clone()));
    Ok(el.design)
}

fn pick_top(file: &ast::SourceFile) -> Result<String, String> {
    let mut instantiated = std::collections::HashSet::new();
    for m in &file.modules {
        collect_module_references(&m.items, &mut instantiated);
    }
    let cands: Vec<&ast::Module> = file
        .modules
        .iter()
        .filter(|m| !m.is_interface && !instantiated.contains(&m.name))
        .collect();
    match cands.len() {
        0 => Err("no top module: every module is instantiated".into()),
        1 => Ok(cands[0].name.clone()),
        _ => Err(format!(
            "ambiguous top module: {}; specify the top explicitly",
            cands
                .iter()
                .map(|module| module.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn collect_module_references(
    items: &[ast::Item],
    references: &mut std::collections::HashSet<String>,
) {
    for item in items {
        match item {
            ast::Item::Inst { module, .. } => {
                references.insert(module.clone());
            }
            ast::Item::Generate(constructs) => {
                for construct in constructs {
                    collect_generate_module_references(construct, references);
                }
            }
            _ => {}
        }
    }
}

fn collect_generate_module_references(
    construct: &ast::GenerateConstruct,
    references: &mut std::collections::HashSet<String>,
) {
    match construct {
        ast::GenerateConstruct::Genvar(_) => {}
        ast::GenerateConstruct::Block(block) => collect_module_references(&block.items, references),
        ast::GenerateConstruct::Case { arms, default, .. } => {
            for arm in arms {
                collect_module_references(&arm.body.items, references);
            }
            if let Some(block) = default {
                collect_module_references(&block.items, references);
            }
        }
        ast::GenerateConstruct::If {
            then, otherwise, ..
        } => {
            collect_module_references(&then.items, references);
            if let Some(block) = otherwise {
                collect_module_references(&block.items, references);
            }
        }
        ast::GenerateConstruct::For { body, .. } => {
            collect_module_references(&body.items, references);
        }
    }
}

fn wildcard_case_as_if(
    selector: ExprIR,
    arms: Vec<(ExprIR, StmtIR)>,
    default: Option<StmtIR>,
    kind: ast::CaseKind,
) -> StmtIR {
    let mut result = default.unwrap_or_else(|| StmtIR::Block(Vec::new()));
    for (label, body) in arms.into_iter().rev() {
        let condition = wildcard_label_condition(selector.clone(), &label, kind);
        result = StmtIR::If {
            c: condition,
            t: Box::new(body),
            f: Some(Box::new(result)),
        };
    }
    result
}

fn wildcard_label_condition(selector: ExprIR, label: &ExprIR, kind: ast::CaseKind) -> ExprIR {
    let width = selector.width().max(label.width());
    let mut conditions = Vec::new();
    for bit in (0..width).rev() {
        let selector_bit = if bit < selector.width() {
            ExprIR::Index {
                e: Box::new(selector.clone()),
                msb: bit,
                lsb: bit,
            }
        } else {
            ExprIR::Const(LogicWord::zero(1))
        };
        let label_bit = label_bit(label, bit, width);
        let wildcard = match kind {
            ast::CaseKind::Exact => ExprIR::Const(LogicWord::zero(1)),
            ast::CaseKind::CaseZ => {
                let selector_is_z = case_equal(selector_bit.clone(), const_bit("'bz"));
                let label_is_z = case_equal(label_bit.clone(), const_bit("'bz"));
                bit_or(selector_is_z, label_is_z)
            }
            ast::CaseKind::CaseX => {
                let selector_is_z = case_equal(selector_bit.clone(), const_bit("'bz"));
                let selector_is_x = case_equal(selector_bit.clone(), const_bit("'bx"));
                let label_is_z = case_equal(label_bit.clone(), const_bit("'bz"));
                let label_is_x = case_equal(label_bit.clone(), const_bit("'bx"));
                bit_or(
                    bit_or(selector_is_z, selector_is_x),
                    bit_or(label_is_z, label_is_x),
                )
            }
        };
        let equal = case_equal(selector_bit, label_bit);
        conditions.push(bit_or(wildcard, equal));
    }
    conditions
        .into_iter()
        .reduce(bit_and)
        .unwrap_or_else(|| ExprIR::Const(LogicWord::ones(1)))
}

fn label_bit(label: &ExprIR, bit: usize, width: usize) -> ExprIR {
    if bit < label.width() {
        ExprIR::Index {
            e: Box::new(label.clone()),
            msb: bit,
            lsb: bit,
        }
    } else {
        let _ = width;
        ExprIR::Const(LogicWord::zero(1))
    }
}

fn const_bit(text: &str) -> ExprIR {
    ExprIR::Const(LogicWord::parse(1, text).expect("one-bit case literal"))
}

fn case_equal(left: ExprIR, right: ExprIR) -> ExprIR {
    ExprIR::Bin {
        op: BinOp::CaseEq,
        a: Box::new(left),
        b: Box::new(right),
    }
}

fn bit_or(left: ExprIR, right: ExprIR) -> ExprIR {
    ExprIR::Bin {
        op: BinOp::Or,
        a: Box::new(left),
        b: Box::new(right),
    }
}

fn bit_and(left: ExprIR, right: ExprIR) -> ExprIR {
    ExprIR::Bin {
        op: BinOp::And,
        a: Box::new(left),
        b: Box::new(right),
    }
}

fn flatten_generate_items(
    items: &[ast::Item],
    params: &HashMap<String, i64>,
    module_name: &str,
    packages: &PackageTables,
) -> Result<Vec<ast::Item>, String> {
    let mut out = Vec::new();
    for item in items {
        match item {
            ast::Item::Genvar(_) => {}
            ast::Item::Generate(constructs) => {
                for construct in constructs {
                    flatten_generate_construct(construct, params, module_name, packages, &mut out)?;
                }
            }
            other => out.push(other.clone()),
        }
    }
    Ok(out)
}

fn flatten_generate_construct(
    construct: &ast::GenerateConstruct,
    params: &HashMap<String, i64>,
    module_name: &str,
    packages: &PackageTables,
    out: &mut Vec<ast::Item>,
) -> Result<(), String> {
    match construct {
        ast::GenerateConstruct::Genvar(_) => {}
        ast::GenerateConstruct::Block(block) => {
            let expanded = flatten_generate_items(&block.items, params, module_name, packages)?;
            out.extend(expanded);
        }
        ast::GenerateConstruct::Case {
            selector,
            arms,
            default,
        } => {
            let selector_value = const_eval_packages(selector, params, module_name, packages)?;
            let mut selected = None;
            for arm in arms {
                for label in &arm.labels {
                    if const_eval_packages(label, params, module_name, packages)? == selector_value
                    {
                        selected = Some(&arm.body);
                        break;
                    }
                }
                if selected.is_some() {
                    break;
                }
            }
            let selected = selected.or(default.as_ref()).ok_or_else(|| {
                format!(
                    "module '{}': generate case selector {selector_value} has no matching item or default",
                    module_name
                )
            })?;
            let expanded = flatten_generate_items(&selected.items, params, module_name, packages)?;
            out.extend(expanded);
        }
        ast::GenerateConstruct::If {
            cond,
            then,
            otherwise,
        } => {
            let selected = if const_eval_packages(cond, params, module_name, packages)? != 0 {
                then
            } else {
                match otherwise.as_ref() {
                    Some(branch) => branch,
                    // A false generate-if without an else generates no items.
                    None => return Ok(()),
                }
            };
            let expanded = flatten_generate_items(&selected.items, params, module_name, packages)?;
            out.extend(expanded);
        }
        ast::GenerateConstruct::For {
            var,
            start,
            cond,
            step,
            body,
        } => {
            let mut value = const_eval_packages(start, params, module_name, packages)?;
            let mut loop_params = params.clone();
            const MAX_GENERATE_STEPS: u64 = 131_072;
            let mut steps = 0_u64;
            loop {
                steps += 1;
                if steps > MAX_GENERATE_STEPS {
                    return Err(format!(
                        "module '{}': generate for-loop exceeds {MAX_GENERATE_STEPS} steps",
                        module_name
                    ));
                }
                loop_params.insert(var.clone(), value);
                if const_eval_packages(cond, &loop_params, module_name, packages)? == 0 {
                    break;
                }
                let replaced: Vec<_> = body
                    .items
                    .iter()
                    .map(|item| substitute_generate_var(item, var, value))
                    .collect();
                let expanded =
                    flatten_generate_items(&replaced, &loop_params, module_name, packages)?;
                out.extend(expanded);
                let next = const_eval_packages(step, &loop_params, module_name, packages)?;
                if next == value {
                    return Err(format!(
                        "module '{}': generate for-loop does not advance '{var}'",
                        module_name
                    ));
                }
                value = next;
            }
        }
    }
    Ok(())
}

impl<'a> Elaborator<'a> {
    fn apply_import(
        &self,
        import: &ast::Import,
        params: &mut HashMap<String, i64>,
        types: &mut HashMap<String, ResolvedType>,
        functions: &mut HashMap<String, ast::FunctionDef>,
        parent_params: &HashMap<String, i64>,
        module_name: &str,
    ) -> Result<(), String> {
        let values = self
            .packages
            .constants
            .get(&import.package)
            .ok_or_else(|| {
                format!(
                    "module '{}': unknown import package '{}'",
                    module_name, import.package
                )
            })?;
        let package_types = self.packages.types.get(&import.package).ok_or_else(|| {
            format!(
                "module '{}': import package '{}' has no type table",
                module_name, import.package
            )
        })?;
        let package_functions = self
            .packages
            .functions
            .get(&import.package)
            .ok_or_else(|| {
                format!(
                    "module '{}': import package '{}' has no function table",
                    module_name, import.package
                )
            })?;
        if import.names.is_empty() {
            for (name, value) in values {
                if !parent_params.contains_key(name) {
                    params.insert(name.clone(), *value);
                }
            }
            for (name, definition) in package_types {
                types.insert(name.clone(), definition.clone());
            }
            for (name, definition) in package_functions {
                functions.insert(name.clone(), definition.clone());
            }
        } else {
            for name in &import.names {
                if let Some(value) = values.get(name).copied() {
                    if !parent_params.contains_key(name) {
                        params.insert(name.clone(), value);
                    }
                } else if let Some(definition) = package_types.get(name).cloned() {
                    if let ResolvedType::Enum(enum_definition) = &definition {
                        for (constant, value) in &enum_definition.constants {
                            if !parent_params.contains_key(constant) {
                                params.insert(constant.clone(), *value);
                            }
                        }
                    }
                    types.insert(name.clone(), definition);
                } else if let Some(definition) = package_functions.get(name).cloned() {
                    functions.insert(name.clone(), definition);
                } else {
                    return Err(format!(
                        "module '{}': package '{}' has no imported name '{}'",
                        module_name, import.package, name
                    ));
                }
            }
        }
        Ok(())
    }

    fn elab_module(
        &mut self,
        module_name: &str,
        prefix: &str,
        parent_params: &HashMap<String, i64>,
        interface_ports: &HashMap<String, HashMap<String, NetEntry>>,
    ) -> Result<(), String> {
        if self.depth > 64 {
            return Err("module instantiation too deep (recursive instance?)".into());
        }
        let module = self.mods[module_name];
        self.depth += 1;
        let r = self.elab_module_inner(module, prefix, parent_params, interface_ports);
        self.depth -= 1;
        r
    }
}

impl<'a> Elaborator<'a> {}

impl<'a> Elaborator<'a> {
    fn expand_module_instance_arrays(
        &self,
        items: &[ast::Item],
        env: &Env<'_>,
    ) -> Result<Vec<ast::Item>, String> {
        let mut expanded = Vec::new();
        for item in items {
            let ast::Item::Inst {
                module: sub_name,
                name: inst_name,
                array_ranges,
                params: overrides,
                conns,
            } = item
            else {
                expanded.push(item.clone());
                continue;
            };
            if array_ranges.is_empty() {
                expanded.push(item.clone());
                continue;
            }
            let Some(sub) = self.mods.get(sub_name.as_str()) else {
                expanded.push(item.clone());
                continue;
            };
            if sub.is_interface {
                expanded.push(item.clone());
                continue;
            }
            let mut dimensions = Vec::with_capacity(array_ranges.len());
            let mut length = 1_usize;
            for range in array_ranges {
                let (left, right) = range;
                let base = self.const_eval(left, env.params, env.module_name)?;
                let last = self.const_eval(right, env.params, env.module_name)?;
                let step = if base <= last { 1 } else { -1 };
                let dimension_length = (base - last).unsigned_abs() as usize + 1;
                if dimension_length == 0 || dimension_length > 256 {
                    return Err(format!(
                        "instance '{}': each module array length must be between 1 and 256",
                        inst_name
                    ));
                }
                length = length
                    .checked_mul(dimension_length)
                    .filter(|total| *total <= 4096)
                    .ok_or_else(|| {
                        format!(
                            "instance '{}': total module array elements exceed 4096",
                            inst_name
                        )
                    })?;
                dimensions.push(InterfaceArrayDimension {
                    base,
                    step,
                    length: dimension_length,
                });
            }
            let mut index_sets = vec![Vec::<i64>::new()];
            for dimension in &dimensions {
                let mut next_index_sets = Vec::new();
                for index_set in &index_sets {
                    for element in 0..dimension.length {
                        let mut next = index_set.clone();
                        next.push(dimension.base + dimension.step * element as i64);
                        next_index_sets.push(next);
                    }
                }
                index_sets = next_index_sets;
            }

            let mut child_params = HashMap::new();
            let mut declared_parameters = Vec::new();
            for child_item in &sub.items {
                if let ast::Item::Params(list) = child_item {
                    for (parameter, value) in list {
                        let value = self.const_eval(value, &child_params, &sub.name)?;
                        child_params.insert(parameter.clone(), value);
                        declared_parameters.push(parameter.clone());
                    }
                }
            }
            for (index, override_value) in overrides.iter().enumerate() {
                let value = self.const_eval(&override_value.1, env.params, env.module_name)?;
                match override_value.0.as_deref() {
                    Some(parameter) => {
                        child_params.insert(parameter.to_string(), value);
                    }
                    None => {
                        if let Some(parameter) = declared_parameters.get(index) {
                            child_params.insert(parameter.clone(), value);
                        }
                    }
                }
            }

            let mut ports = Vec::new();
            for child_item in &sub.items {
                if let ast::Item::Port {
                    dir,
                    interface_name,
                    type_name,
                    range,
                    array_ranges,
                    names,
                    ..
                } = child_item
                {
                    if interface_name.is_some() {
                        return Err(format!(
                            "instance '{}': module arrays with interface ports are not yet supported",
                            inst_name
                        ));
                    }
                    if type_name.is_some() {
                        return Err(format!(
                            "instance '{}': module arrays with custom-typed ports are not yet supported",
                            inst_name
                        ));
                    }
                    let width = match range {
                        Some((msb, lsb)) => {
                            let hi = self.const_eval(msb, &child_params, &sub.name)?;
                            let lo = self.const_eval(lsb, &child_params, &sub.name)?;
                            (hi - lo).unsigned_abs() as usize + 1
                        }
                        None => 1,
                    };
                    if width == 0 {
                        return Err(format!(
                            "instance '{}': module array child port width cannot be zero",
                            inst_name
                        ));
                    }
                    let array = self.resolve_unpacked_array(
                        array_ranges,
                        width,
                        &child_params,
                        &sub.name,
                        &names[0],
                        "port",
                    )?;
                    let total_width = width * array.as_ref().map_or(1, |resolved| resolved.length);
                    for port_name in names {
                        ports.push((port_name.clone(), *dir, total_width));
                    }
                }
            }

            for (element, index_set) in index_sets.into_iter().enumerate() {
                let index = index_set
                    .iter()
                    .map(|index| index.to_string())
                    .collect::<Vec<_>>()
                    .join("_");
                let bit_offset = length - 1 - element;
                let mut element_conns = Vec::new();
                let mut positional_port = 0_usize;
                for connection in conns {
                    let (port_name, direction, port_width) = match connection {
                        ast::Conn::Named(name, _) => ports
                            .iter()
                            .find(|(candidate, _, _)| candidate == name)
                            .map(|(name, direction, width)| (name.clone(), *direction, *width))
                            .ok_or_else(|| {
                                format!(
                                    "instance '{}': unknown module array port '{}'",
                                    inst_name, name
                                )
                            })?,
                        ast::Conn::Pos(_) => {
                            let port = ports
                                .get(positional_port)
                                .ok_or_else(|| {
                                    format!(
                                        "instance '{}': too many positional module array connections",
                                        inst_name
                                    )
                                })?
                                .clone();
                            positional_port += 1;
                            port
                        }
                    };
                    let connection = match connection {
                        ast::Conn::Named(name, expression) => {
                            ast::Conn::Named(name.clone(), expression.clone())
                        }
                        ast::Conn::Pos(expression) => ast::Conn::Pos(expression.clone()),
                    };
                    let expression = match &connection {
                        ast::Conn::Named(_, Some(expression))
                        | ast::Conn::Pos(Some(expression)) => expression,
                        _ => {
                            element_conns.push(connection);
                            continue;
                        }
                    };
                    let expression_width = self.elab_expr(expression, env)?.width();
                    let slice_offset = bit_offset * port_width;
                    let msb = slice_offset + port_width - 1;
                    let lsb = slice_offset;
                    let sliced = match direction {
                        ast::DirSrc::In => {
                            if expression_width == port_width {
                                expression.clone()
                            } else if expression_width >= length * port_width {
                                ast::Expr::Slice {
                                    value: Box::new(expression.clone()),
                                    msb: Box::new(loop_value(msb as i64)),
                                    lsb: Box::new(loop_value(lsb as i64)),
                                }
                            } else {
                                return Err(format!(
                                    "instance '{}': input port '{}' width {} cannot feed {} array elements of width {}",
                                    inst_name,
                                    port_name,
                                    expression_width,
                                    length,
                                    port_width
                                ));
                            }
                        }
                        ast::DirSrc::Out => {
                            let ast::Expr::Ident(target) = expression else {
                                return Err(format!(
                                    "instance '{}': module array output port '{}' must connect an identifier",
                                    inst_name, port_name
                                ));
                            };
                            if expression_width < length * port_width {
                                return Err(format!(
                                    "instance '{}': output port '{}' width {} cannot receive {} array elements of width {}",
                                    inst_name,
                                    port_name,
                                    expression_width,
                                    length,
                                    port_width
                                ));
                            }
                            ast::Expr::Part(
                                target.clone(),
                                Box::new(loop_value(msb as i64)),
                                Box::new(loop_value(lsb as i64)),
                            )
                        }
                        ast::DirSrc::InOut => {
                            return Err(format!(
                                "instance '{}': module arrays do not support inout port '{}'",
                                inst_name, port_name
                            ));
                        }
                    };
                    element_conns.push(match connection {
                        ast::Conn::Named(name, _) => ast::Conn::Named(name, Some(sliced)),
                        ast::Conn::Pos(_) => ast::Conn::Pos(Some(sliced)),
                    });
                }
                expanded.push(ast::Item::Inst {
                    module: sub_name.clone(),
                    name: format!("{inst_name}_{index}"),
                    array_ranges: Vec::new(),
                    params: overrides.clone(),
                    conns: element_conns,
                });
            }
        }
        Ok(expanded)
    }
}

impl<'a> Elaborator<'a> {}

impl<'a> Elaborator<'a> {}

fn reduce(items: &[ExprIR], op: BinOp) -> ExprIR {
    items
        .iter()
        .cloned()
        .reduce(|a, b| ExprIR::Bin {
            op,
            a: Box::new(a),
            b: Box::new(b),
        })
        .unwrap_or(ExprIR::Const(LogicWord::zero(1)))
}

fn full_range(width: usize) -> Range {
    if width == 1 {
        Range::Bit(0)
    } else {
        Range::Span {
            msb: width - 1,
            lsb: 0,
        }
    }
}

fn range_bits(net: &usize, range: &Range) -> Vec<usize> {
    match range {
        Range::Bit(offset) => vec![net + offset],
        Range::Span { msb, lsb } => {
            if msb >= lsb {
                (*lsb..=*msb).map(|offset| net + offset).collect()
            } else {
                (*msb..=*lsb).rev().map(|offset| net + offset).collect()
            }
        }
    }
}

fn find_conn<'a>(conns: &'a [ast::Conn], name: &str, index: usize) -> Option<&'a ast::Expr> {
    let mut pos = 0;
    for c in conns {
        match c {
            ast::Conn::Named(n, e) => {
                if n == name {
                    return e.as_ref();
                }
            }
            ast::Conn::Pos(e) => {
                if pos == index {
                    return e.as_ref();
                }
                pos += 1;
            }
        }
    }
    None
}

fn const_eval_packages(
    expression: &ast::Expr,
    params: &HashMap<String, i64>,
    module_name: &str,
    packages: &PackageTables,
) -> Result<i64, String> {
    let resolved = resolve_package_expressions(expression, packages)?;
    const_eval(&resolved, params, module_name)
}

fn resolve_package_expressions(
    expression: &ast::Expr,
    packages: &PackageTables,
) -> Result<ast::Expr, String> {
    Ok(match expression {
        ast::Expr::ScopedIdent { package, name } => {
            let value = packages
                .constants
                .get(package)
                .and_then(|values| values.get(name))
                .copied()
                .ok_or_else(|| format!("unknown package constant '{}::{}'", package, name))?;
            ast::Expr::Num {
                width: Some(32),
                word: LogicWord::from_u64(32, value as u64),
                orig: value.to_string(),
            }
        }
        ast::Expr::Ident(_)
        | ast::Expr::Member(_, _)
        | ast::Expr::MemberChain(_)
        | ast::Expr::Num { .. }
        | ast::Expr::Str(_) => expression.clone(),
        ast::Expr::Select(name, index) => ast::Expr::Select(
            name.clone(),
            Box::new(resolve_package_expressions(index, packages)?),
        ),
        ast::Expr::ArraySelect(name, indexes) => ast::Expr::ArraySelect(
            name.clone(),
            indexes
                .iter()
                .map(|index| resolve_package_expressions(index, packages))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        ast::Expr::ArrayIndexedPart {
            name,
            indexes,
            start,
            width,
            ascending,
        } => ast::Expr::ArrayIndexedPart {
            name: name.clone(),
            indexes: indexes
                .iter()
                .map(|index| resolve_package_expressions(index, packages))
                .collect::<Result<Vec<_>, _>>()?,
            start: Box::new(resolve_package_expressions(start, packages)?),
            width: Box::new(resolve_package_expressions(width, packages)?),
            ascending: *ascending,
        },
        ast::Expr::ArrayPart {
            name,
            indexes,
            msb,
            lsb,
        } => ast::Expr::ArrayPart {
            name: name.clone(),
            indexes: indexes
                .iter()
                .map(|index| resolve_package_expressions(index, packages))
                .collect::<Result<Vec<_>, _>>()?,
            msb: Box::new(resolve_package_expressions(msb, packages)?),
            lsb: Box::new(resolve_package_expressions(lsb, packages)?),
        },
        ast::Expr::Part(name, msb, lsb) => ast::Expr::Part(
            name.clone(),
            Box::new(resolve_package_expressions(msb, packages)?),
            Box::new(resolve_package_expressions(lsb, packages)?),
        ),
        ast::Expr::MethodCall {
            object,
            method,
            arguments,
        } => ast::Expr::MethodCall {
            object: object.clone(),
            method: method.clone(),
            arguments: arguments
                .iter()
                .map(|argument| resolve_package_expressions(argument, packages))
                .collect::<Result<Vec<_>, _>>()?,
        },
        ast::Expr::MethodCallChain {
            path,
            method,
            arguments,
        } => ast::Expr::MethodCallChain {
            path: path.clone(),
            method: method.clone(),
            arguments: arguments
                .iter()
                .map(|argument| resolve_package_expressions(argument, packages))
                .collect::<Result<Vec<_>, _>>()?,
        },
        ast::Expr::New(size) => {
            ast::Expr::New(Box::new(resolve_package_expressions(size, packages)?))
        }
        ast::Expr::NewClass(arguments) => ast::Expr::NewClass(
            arguments
                .iter()
                .map(|argument| resolve_package_expressions(argument, packages))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        ast::Expr::IndexedPart {
            name,
            start,
            width,
            ascending,
        } => ast::Expr::IndexedPart {
            name: name.clone(),
            start: Box::new(resolve_package_expressions(start, packages)?),
            width: Box::new(resolve_package_expressions(width, packages)?),
            ascending: *ascending,
        },
        ast::Expr::Un(op, a) => {
            ast::Expr::Un(*op, Box::new(resolve_package_expressions(a, packages)?))
        }
        ast::Expr::Bin(op, a, b) => ast::Expr::Bin(
            *op,
            Box::new(resolve_package_expressions(a, packages)?),
            Box::new(resolve_package_expressions(b, packages)?),
        ),
        ast::Expr::Cond(c, t, f) => ast::Expr::Cond(
            Box::new(resolve_package_expressions(c, packages)?),
            Box::new(resolve_package_expressions(t, packages)?),
            Box::new(resolve_package_expressions(f, packages)?),
        ),
        ast::Expr::Concat(parts) => ast::Expr::Concat(
            parts
                .iter()
                .map(|part| resolve_package_expressions(part, packages))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        ast::Expr::Replicate(count, value) => ast::Expr::Replicate(
            Box::new(resolve_package_expressions(count, packages)?),
            Box::new(resolve_package_expressions(value, packages)?),
        ),
        ast::Expr::Call(name, args) => ast::Expr::Call(
            name.clone(),
            args.iter()
                .map(|arg| resolve_package_expressions(arg, packages))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        ast::Expr::Slice { value, msb, lsb } => ast::Expr::Slice {
            value: Box::new(resolve_package_expressions(value, packages)?),
            msb: Box::new(resolve_package_expressions(msb, packages)?),
            lsb: Box::new(resolve_package_expressions(lsb, packages)?),
        },
        ast::Expr::Resize { width, value } => ast::Expr::Resize {
            width: *width,
            value: Box::new(resolve_package_expressions(value, packages)?),
        },
        ast::Expr::WildcardMatch {
            kind,
            selector,
            label,
        } => ast::Expr::WildcardMatch {
            kind: *kind,
            selector: Box::new(resolve_package_expressions(selector, packages)?),
            label: Box::new(resolve_package_expressions(label, packages)?),
        },
    })
}

fn const_eval(
    e: &ast::Expr,
    params: &HashMap<String, i64>,
    module_name: &str,
) -> Result<i64, String> {
    match e {
        ast::Expr::Num { word, .. } => word
            .to_u64()
            .map(|v| v as i64)
            .ok_or_else(|| "constant contains x/z".to_string()),
        ast::Expr::Ident(n) => params
            .get(n)
            .copied()
            .ok_or_else(|| format!("'{}' is not a parameter (in module '{}')", n, module_name)),
        ast::Expr::Call(name, args) if name == "$signed" || name == "$unsigned" => {
            if args.len() != 1 {
                return Err(format!("'{}' constant expects one argument", name));
            }
            let value = const_eval(&args[0], params, module_name)?;
            if name == "$unsigned" {
                return Ok(value);
            }
            match &args[0] {
                ast::Expr::Num {
                    width: Some(width), ..
                } if *width < 64 => {
                    let bit = (value as u64 >> (*width - 1)) & 1;
                    if bit == 1 {
                        Ok(value - (1_i64 << *width))
                    } else {
                        Ok(value)
                    }
                }
                _ => Ok(value),
            }
        }
        ast::Expr::Resize { width, value } => {
            let value = const_eval(value, params, module_name)?;
            if *width < 64 {
                let modulo = 1_i64 << *width;
                Ok(value.rem_euclid(modulo))
            } else {
                Ok(value)
            }
        }
        ast::Expr::Un(op, a) => {
            let v = const_eval(a, params, module_name)?;
            Ok(match op {
                ast::UnOpSrc::Minus => -v,
                ast::UnOpSrc::Plus => v,
                ast::UnOpSrc::LogNot => (v == 0) as i64,
                _ => return Err("unsupported constant unary operator".into()),
            })
        }
        ast::Expr::Bin(op, a, b) => {
            let x = const_eval(a, params, module_name)?;
            let y = const_eval(b, params, module_name)?;
            Ok(match op {
                ast::BinOpSrc::Add => x.wrapping_add(y),
                ast::BinOpSrc::Sub => x.wrapping_sub(y),
                ast::BinOpSrc::Mul => x.wrapping_mul(y),
                ast::BinOpSrc::Div => {
                    if y == 0 {
                        return Err("division by zero in constant".into());
                    }
                    x / y
                }
                ast::BinOpSrc::Shl => x.wrapping_shl(y as u32),
                ast::BinOpSrc::Shr => x.wrapping_shr(y as u32),
                ast::BinOpSrc::Eq => (x == y) as i64,
                ast::BinOpSrc::Ne => (x != y) as i64,
                ast::BinOpSrc::Lt => (x < y) as i64,
                ast::BinOpSrc::Le => (x <= y) as i64,
                ast::BinOpSrc::Gt => (x > y) as i64,
                ast::BinOpSrc::Ge => (x >= y) as i64,
                _ => return Err("unsupported constant binary operator".into()),
            })
        }
        _ => Err("expression is not constant (parameters/ranges/indices must be constant)".into()),
    }
}

struct Env<'a> {
    nets: &'a HashMap<String, NetEntry>,
    interface_ports: &'a HashMap<String, HashMap<String, NetEntry>>,
    interface_directions: &'a HashMap<String, HashMap<String, ast::DirSrc>>,
    params: &'a HashMap<String, i64>,
    types: &'a HashMap<String, ResolvedType>,
    functions: &'a HashMap<String, ast::FunctionDef>,
    module_name: &'a str,
    prefix: &'a str,
    /// Handle variable name -> class name for class-typed nets.
    class_handles: &'a HashMap<String, String>,
    /// 引用别名：句柄 -> 组代表句柄。`b = d` 后写 b 可经 d 观察。
    /// Rc 共享：方法体内建立的别名对后续语句可见（connect 阶段语义）。
    class_aliases: std::rc::Rc<std::cell::RefCell<HashMap<String, String>>>,
    /// While elaborating a class method: (handle, dynamic class of the
    /// allocated object).
    this_handle: Option<(&'a str, &'a str)>,
    /// Class in which the currently elaborated method was found; used to
    /// resolve `super` dispatch.
    method_owner: Option<&'a str>,
    /// Elaborated actuals bound to the current method's formal arguments.
    method_args: &'a HashMap<String, ExprIR>,
    /// Scratch return-value net for a value-returning method invoked as a
    /// statement: (method name, net base, width).
    method_return: Option<(&'a str, usize, usize)>,
}
impl<'a> Env<'a> {
    /// 句柄的组代表（无别名时为自身）。
    fn canon(&self, handle: &str) -> String {
        self.class_aliases
            .borrow()
            .get(handle)
            .cloned()
            .unwrap_or_else(|| handle.to_string())
    }

    fn net(&self, name: &str) -> Result<&NetEntry, String> {
        self.nets.get(name).ok_or_else(|| {
            format!(
                "module '{}': undeclared identifier '{}'",
                self.module_name, name
            )
        })
    }
}

#[cfg(test)]
#[path = "elab/tests.rs"]
mod tests;
