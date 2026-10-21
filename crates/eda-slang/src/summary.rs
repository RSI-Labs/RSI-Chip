//! Typed summaries and structural validation for slang's elaborated AST.

use super::ast_util::{
    expression_target_names, instance_parameters, normalize_detailed_types,
    normalize_instance_bodies, parameter_summary, source_span, string_field,
};
use crate::types::simple_type_width;
use eda_core::ir::Design;
use expressions::{
    collect_generate_summary, collect_kinds, empty_expression_summary, expression_summary,
    procedural_assignments,
};

mod expressions;
mod validation;
use serde_json::Value;

fn collect_class_handle_names(
    value: &Value,
    class_types: &std::collections::BTreeSet<String>,
    handles: &mut std::collections::BTreeSet<String>,
) {
    if let Some(fields) = value.as_object() {
        let type_name = string_field(value, "type").unwrap_or_default();
        let class_name = type_name
            .split_whitespace()
            .next_back()
            .and_then(|raw_name| raw_name.split(['#', '$']).next())
            .map(str::trim)
            .filter(|name| !name.is_empty());
        if class_type_is_selected(value, class_name, class_types) {
            if let Some(symbol) = string_field(value, "symbol").filter(|symbol| !symbol.is_empty())
            {
                handles.insert(symbol);
            } else if let Some(name) = string_field(value, "name").filter(|name| !name.is_empty()) {
                if let Some(address) = value.get("addr").and_then(Value::as_u64) {
                    handles.insert(format!("{address} {name}"));
                } else {
                    handles.insert(name);
                }
            }
        }
        for nested in fields.values() {
            collect_class_handle_names(nested, class_types, handles);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_class_handle_names(nested, class_types, handles);
        }
    }
}

fn collect_class_static_property_symbols(
    value: &Value,
    symbols: &mut std::collections::BTreeSet<String>,
) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("ClassType") {
            let empty_members = Vec::new();
            let members = value
                .get("members")
                .and_then(Value::as_array)
                .unwrap_or(&empty_members);
            for member in members {
                if string_field(member, "kind").as_deref() == Some("ClassProperty")
                    && string_field(member, "lifetime").as_deref() == Some("Static")
                {
                    let Some(name) = string_field(member, "name") else {
                        continue;
                    };
                    if let Some(address) = member.get("addr").and_then(Value::as_u64) {
                        symbols.insert(format!("{address} {name}"));
                    }
                }
            }
        }
        for nested in fields.values() {
            collect_class_static_property_symbols(nested, symbols);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_class_static_property_symbols(nested, symbols);
        }
    }
}

fn collect_class_type_names(value: &Value, class_types: &mut std::collections::BTreeSet<String>) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("ClassType") {
            if let Some(name) = string_field(value, "name") {
                class_types.insert(name);
            }
        }
        for nested in fields.values() {
            collect_class_type_names(nested, class_types);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_class_type_names(nested, class_types);
        }
    }
}

fn class_type_is_selected(
    value: &Value,
    class_name: Option<&str>,
    class_types: &std::collections::BTreeSet<String>,
) -> bool {
    let Some(class_name) = class_name else {
        return false;
    };
    matches!(
        string_field(value, "kind").as_deref(),
        Some("Net" | "Variable" | "Port" | "NamedValue" | "HierarchicalValue")
    ) && class_types.contains(class_name)
}

fn collect_instance_array_summary(
    member: &Value,
    prefix: &str,
    instances: &mut Vec<SlangInstanceSummary>,
) -> Result<(), String> {
    let range = string_field(member, "range").unwrap_or_default();
    let range = range.trim().trim_start_matches('[').trim_end_matches(']');
    let Some((left, right)) = range.split_once(':') else {
        return Err(format!("instance array has invalid range '{range}'"));
    };
    let left = left
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("instance array has invalid left bound '{left}'"))?;
    let right = right
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("instance array has invalid right bound '{right}'"))?;
    let indexes = if left <= right {
        (left..=right).collect::<Vec<_>>()
    } else {
        (right..=left).rev().collect::<Vec<_>>()
    };
    let elements = member
        .get("members")
        .and_then(Value::as_array)
        .ok_or_else(|| "slang AST InstanceArray has no members".to_string())?;
    if indexes.len() != elements.len() {
        return Err(format!(
            "instance array range has {} indexes but {} elaborated elements",
            indexes.len(),
            elements.len()
        ));
    }
    for (index, element) in indexes.into_iter().zip(elements) {
        let instance_name = format!("{prefix}[{index}]");
        match string_field(element, "kind").as_deref() {
            Some("Instance") => {
                let definition_name = element
                    .get("body")
                    .and_then(|body| string_field(body, "name"))
                    .unwrap_or_default();
                let parameters = element
                    .get("body")
                    .map(instance_parameters)
                    .transpose()?
                    .unwrap_or_default();
                instances.push(SlangInstanceSummary {
                    instance_name,
                    definition_name,
                    parameters,
                });
            }
            Some("InstanceArray") => {
                collect_instance_array_summary(element, &instance_name, instances)?;
            }
            other => {
                return Err(format!(
                    "instance array contains unsupported member kind {other:?}"
                ))
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangAstSummary {
    pub top_name: String,
    pub source_spans: std::collections::BTreeMap<String, SlangSourceSpan>,
    pub ports: Vec<SlangPortSummary>,
    pub declarations: Vec<SlangDeclarationSummary>,
    pub parameters: Vec<SlangParameterSummary>,
    pub continuous_targets: Vec<SlangContinuousTarget>,
    pub procedural_blocks: Vec<SlangProceduralBlockSummary>,
    pub instances: Vec<SlangInstanceSummary>,
    pub kinds: std::collections::BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangSourceSpan {
    pub file: String,
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangPortSummary {
    pub name: String,
    pub direction: String,
    pub type_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangDeclarationSummary {
    pub name: String,
    pub kind: String,
    pub type_name: String,
    /// Packed width for simple scalar/vector types. Complex aggregate,
    /// unpacked-array, string, class, and special SystemVerilog types are left
    /// unresolved rather than guessed.
    pub width: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangInstanceSummary {
    pub instance_name: String,
    pub definition_name: String,
    /// Post-elaboration parameters visible in this particular instance body.
    /// The flattened repository IR does not yet retain parameter overrides, so
    /// this inventory is authoritative slang metadata rather than a fully
    /// validated equivalence field.
    pub parameters: Vec<SlangParameterSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangParameterSummary {
    pub name: String,
    pub type_name: String,
    pub value: String,
    pub is_local: bool,
    pub is_port: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangContinuousTarget {
    pub name: String,
    /// `"ContinuousAssign"` or `"Initializer"`.
    pub source: String,
    pub rhs: SlangExpressionSummary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangProceduralBlockSummary {
    pub procedure_kind: String,
    pub assignment_targets: Vec<String>,
    pub assignments: Vec<SlangAssignmentSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangAssignmentSummary {
    pub target_names: Vec<String>,
    pub rhs: SlangExpressionSummary,
    pub nonblocking: bool,
    /// Selector of a dynamic packed lvalue. Direct lowering canonicalizes the
    /// update into a whole-vector concatenation of per-bit conditionals.
    pub dynamic_packed_lvalue: bool,
    pub dynamic_packed_lvalue_selector: Option<SlangExpressionSummary>,
    /// Member writes through a sparse-memory element are canonicalized into a
    /// whole-memory-element concat; exact structural equivalence is deferred.
    pub memory_member_lvalue: bool,
    /// Class static storage uses an internal IR name, so exact RHS symbol
    /// equivalence is deferred while assignment inventory remains checked.
    pub uses_class_static: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangExpressionSummary {
    pub kind: String,
    pub type_name: String,
    pub operator: String,
    /// `Simple`, `IndexedUp`, or `IndexedDown` for slang range selects.
    pub selection_kind: String,
    /// Packed width for simple slang expression types. Complex/aggregate types
    /// remain unresolved.
    pub type_width: Option<usize>,
    /// Signedness for simple slang expression types. Complex/aggregate types
    /// and unresolved widths remain unresolved.
    pub type_signed: Option<bool>,
    pub symbol: Option<String>,
    /// Canonical integer-literal fingerprint (`width:bits`, MSB first) for
    /// expressions slang represented as `IntegerLiteral`. Other literal forms
    /// remain unresolved until typed lowering preserves their exact type.
    pub literal: Option<String>,
    /// Elaborated replication count for slang `Replication` nodes.
    pub repeat_count: Option<usize>,
    pub operands: Vec<SlangExpressionSummary>,
}

impl SlangAstSummary {
    pub fn parse(json: &str) -> Result<Self, String> {
        let start = json
            .find('{')
            .ok_or_else(|| "slang AST output did not contain a JSON object".to_string())?;
        let document: Value = serde_json::from_str(&json[start..])
            .map_err(|error| format!("invalid slang AST JSON: {error}"))?;
        let root = document.get("design").unwrap_or(&document);
        let root = if string_field(root, "kind").as_deref() == Some("Root") {
            root.get("members")
                .and_then(Value::as_array)
                .and_then(|members| {
                    members
                        .iter()
                        .find(|member| string_field(member, "kind").as_deref() == Some("Instance"))
                })
                .unwrap_or(root)
        } else {
            root
        };
        let mut root = root.clone();
        normalize_instance_bodies(&mut root);
        normalize_detailed_types(&mut root);
        let root = &root;
        if string_field(root, "kind").as_deref() != Some("Instance") {
            return Err("slang AST root is not an elaborated Instance".into());
        }
        let body = root
            .get("body")
            .ok_or_else(|| "slang AST Instance has no body".to_string())?;
        if string_field(body, "kind").as_deref() != Some("InstanceBody") {
            return Err("slang AST Instance body has unexpected kind".into());
        }
        let top_name = string_field(root, "name")
            .or_else(|| string_field(body, "name"))
            .ok_or_else(|| "slang AST Instance has no name".to_string())?;
        let mut source_spans = std::collections::BTreeMap::new();
        if let Some(span) = source_span(root) {
            source_spans.insert("top".to_string(), span);
        }
        let members = body
            .get("members")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut class_types = std::collections::BTreeSet::new();
        collect_class_type_names(&document, &mut class_types);
        let mut class_handles = std::collections::BTreeSet::new();
        collect_class_handle_names(root, &class_types, &mut class_handles);
        collect_class_static_property_symbols(&document, &mut class_handles);
        let mut ports = Vec::new();
        let mut declarations = Vec::new();
        let mut parameters = Vec::new();
        let mut continuous_targets = Vec::new();
        let mut procedural_blocks = Vec::new();
        let mut instances = Vec::new();
        for member in &members {
            let Some(kind) = string_field(member, "kind") else {
                continue;
            };
            match kind.as_str() {
                "Port" => {
                    let name = string_field(member, "name")
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| "slang AST Port has no name".to_string())?;
                    ports.push(SlangPortSummary {
                        name: name.clone(),
                        direction: string_field(member, "direction").unwrap_or_default(),
                        type_name: string_field(member, "type").unwrap_or_default(),
                    });
                    declarations.push(SlangDeclarationSummary {
                        name: name.clone(),
                        kind: "Port".into(),
                        type_name: string_field(member, "type").unwrap_or_default(),
                        width: simple_type_width(&string_field(member, "type").unwrap_or_default()),
                    });
                    if let Some(span) = source_span(member) {
                        source_spans.insert(format!("port:{name}"), span.clone());
                        source_spans.insert(format!("declaration:{name}"), span);
                    }
                }
                "Net" | "Variable" => {
                    let name = string_field(member, "name")
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| "slang AST declaration has no name".to_string())?;
                    declarations.push(SlangDeclarationSummary {
                        name: name.clone(),
                        kind: kind.clone(),
                        type_name: string_field(member, "type").unwrap_or_default(),
                        width: simple_type_width(&string_field(member, "type").unwrap_or_default()),
                    });
                    if member.get("initializer").is_some() {
                        let rhs = member
                            .get("initializer")
                            .map(expression_summary)
                            .unwrap_or_else(empty_expression_summary);
                        continuous_targets.push(SlangContinuousTarget {
                            name: name.clone(),
                            source: "Initializer".into(),
                            rhs,
                        });
                        if let Some(span) = source_span(member) {
                            source_spans.insert(format!("continuous:{name}:0"), span);
                        }
                    }
                    if let Some(span) = source_span(member) {
                        source_spans.insert(format!("declaration:{name}"), span);
                    }
                }
                "Parameter" => {
                    let name = string_field(member, "name")
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| "slang AST Parameter has no name".to_string())?;
                    parameters.push(parameter_summary(member)?);
                    if let Some(span) = source_span(member) {
                        source_spans.insert(format!("parameter:{name}"), span);
                    }
                }
                "ContinuousAssign" => {
                    let Some(assignment) = member.get("assignment") else {
                        continue;
                    };
                    let Some(left) = assignment.get("left") else {
                        continue;
                    };
                    let rhs = assignment
                        .get("right")
                        .map(expression_summary)
                        .unwrap_or_else(empty_expression_summary);
                    for name in expression_target_names(left) {
                        let occurrence = continuous_targets.len();
                        continuous_targets.push(SlangContinuousTarget {
                            name: name.clone(),
                            source: "ContinuousAssign".into(),
                            rhs: rhs.clone(),
                        });
                        if let Some(span) = source_span(member) {
                            source_spans.insert(format!("continuous:{name}:{occurrence}"), span);
                        }
                    }
                }
                "ProceduralBlock" => {
                    let procedure_kind = string_field(member, "procedureKind").unwrap_or_default();
                    let assignments = procedural_assignments(member, &class_handles);
                    let assignment_targets = assignments
                        .iter()
                        .flat_map(|assignment| assignment.target_names.iter().cloned())
                        .collect();
                    procedural_blocks.push(SlangProceduralBlockSummary {
                        procedure_kind,
                        assignment_targets,
                        assignments,
                    });
                    if let Some(span) = source_span(member) {
                        source_spans
                            .insert(format!("procedural:{}", procedural_blocks.len() - 1), span);
                    }
                }
                "Instance" => {
                    let instance_name = string_field(member, "name")
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| "slang AST Instance has no name".to_string())?;
                    let definition_name = member
                        .get("body")
                        .and_then(|body| string_field(body, "name"))
                        .unwrap_or_default();
                    let child_parameters = member
                        .get("body")
                        .map(instance_parameters)
                        .transpose()?
                        .unwrap_or_default();
                    instances.push(SlangInstanceSummary {
                        instance_name: instance_name.clone(),
                        definition_name,
                        parameters: child_parameters,
                    });
                    if let Some(span) = source_span(member) {
                        source_spans.insert(format!("instance:{instance_name}"), span);
                    }
                }
                "InstanceArray" => {
                    let array_name = string_field(member, "name")
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| "slang AST InstanceArray has no name".to_string())?;
                    collect_instance_array_summary(member, &array_name, &mut instances)?;
                }
                generate_kind if generate_kind.starts_with("GenerateBlock") => {
                    collect_generate_summary(
                        member,
                        &[],
                        &mut instances,
                        &mut continuous_targets,
                        &mut procedural_blocks,
                        &class_handles,
                    )?;
                }
                _ => {}
            }
        }
        if let Some(members) = root
            .get("eda_compilation_unit_members")
            .and_then(serde_json::Value::as_array)
        {
            for member in members {
                if string_field(member, "kind").as_deref() == Some("Parameter") {
                    parameters.push(parameter_summary(member)?);
                }
            }
        }
        let mut kinds = std::collections::BTreeSet::new();
        collect_kinds(root, &mut kinds);
        Ok(Self {
            top_name,
            source_spans,
            ports,
            declarations,
            parameters,
            continuous_targets,
            procedural_blocks,
            instances,
            kinds,
        })
    }
}
