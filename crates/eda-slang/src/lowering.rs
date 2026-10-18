//! Direct lowering from slang's post-elaboration AST JSON into the repository IR.

use crate::ast_util::{
    bool_field, normalize_detailed_types, normalize_instance_bodies, string_field, with_source_span,
};
use eda_core::ir::Design;
use eda_core::logic::LogicWord;

mod declarations;
mod expressions;
mod functions;
mod hierarchy;
mod statements;

fn collect_compilation_unit_definitions(
    value: &serde_json::Value,
    definitions: &mut Vec<serde_json::Value>,
) {
    if let Some(fields) = value.as_object() {
        if matches!(
            fields.get("kind").and_then(serde_json::Value::as_str),
            Some("Parameter" | "Subroutine" | "TypeAlias" | "EnumType")
        ) {
            definitions.push(value.clone());
        }
        for nested in fields.values() {
            collect_compilation_unit_definitions(nested, definitions);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_compilation_unit_definitions(nested, definitions);
        }
    }
}

pub(super) fn parse_ast_root(json: &str) -> Result<serde_json::Value, String> {
    let start = json
        .find('{')
        .ok_or_else(|| "slang AST output did not contain a JSON object".to_string())?;
    let root: serde_json::Value = serde_json::from_str(&json[start..])
        .map_err(|error| format!("invalid slang AST JSON: {error}"))?;
    let design = root.get("design").unwrap_or(&root);
    let root = if string_field(design, "kind").as_deref() == Some("Root") {
        design
            .get("members")
            .and_then(serde_json::Value::as_array)
            .and_then(|members| {
                members
                    .iter()
                    .find(|member| string_field(member, "kind").as_deref() == Some("Instance"))
            })
            .ok_or_else(|| "slang AST Root has no elaborated Instance".to_string())?
    } else {
        design
    };
    if string_field(root, "kind").as_deref() != Some("Instance") {
        return Err("slang AST root is not an elaborated Instance".into());
    }
    let mut root = root.clone();
    normalize_instance_bodies(&mut root);
    normalize_detailed_types(&mut root);
    let mut compilation_unit_members = Vec::new();
    let imported_package_addresses: std::collections::BTreeSet<u64> = root
        .get("body")
        .and_then(|body| body.get("members"))
        .and_then(serde_json::Value::as_array)
        .map(|members| {
            members
                .iter()
                .filter(|member| {
                    matches!(
                        string_field(member, "kind").as_deref(),
                        Some("WildcardImport" | "ExplicitImport")
                    )
                })
                .filter_map(|member| string_field(member, "package"))
                .filter_map(|package| package.split_whitespace().next()?.parse::<u64>().ok())
                .collect()
        })
        .unwrap_or_default();
    if let Some(members) = design.get("members").and_then(serde_json::Value::as_array) {
        for member in members {
            if string_field(member, "kind").as_deref() != Some("CompilationUnit") {
                continue;
            }
            let empty_packages = Vec::new();
            for package in member
                .get("members")
                .and_then(serde_json::Value::as_array)
                .unwrap_or(&empty_packages)
            {
                let package_address = package
                    .get("addr")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or_default();
                if imported_package_addresses.contains(&package_address) {
                    collect_compilation_unit_definitions(package, &mut compilation_unit_members);
                }
            }
        }
    }
    for member in &mut compilation_unit_members {
        normalize_instance_bodies(member);
        normalize_detailed_types(member);
    }
    if !compilation_unit_members.is_empty() {
        root.as_object_mut()
            .expect("elaborated slang Instance is an object")
            .insert(
                "eda_compilation_unit_members".into(),
                serde_json::Value::Array(compilation_unit_members),
            );
    }
    Ok(root)
}

#[derive(Clone)]
pub(super) struct AstLoweredSymbol {
    base: usize,
    width: usize,
    signed: bool,
    /// Declared bounds for a simple packed vector and all packed dimensions
    /// used to map logical selects onto canonical physical bits.
    bounds: Option<(i64, i64)>,
    dimensions: Vec<crate::types::AstPackedDimension>,
    memory: Option<usize>,
    struct_fields: std::collections::BTreeMap<String, AstStructField>,
}

pub(super) struct AstEventArraySelection {
    pub(super) base: usize,
    pub(super) dimensions: Vec<crate::types::AstPackedDimension>,
    pub(super) selectors: Vec<serde_json::Value>,
}

pub(super) struct AstEventArrayCandidate {
    pub(super) net: usize,
    pub(super) conditions: Vec<eda_core::ir::Expr>,
}

pub(super) struct AstClockingVariable {
    pub(super) direction: eda_core::ir::Dir,
    pub(super) symbol: AstLoweredSymbol,
}

pub(super) struct AstInterfaceModportSymbol {
    pub(super) direction: eda_core::ir::Dir,
    pub(super) symbol: AstLoweredSymbol,
}

pub(super) type InterfaceModportSymbols =
    std::collections::BTreeMap<String, AstInterfaceModportSymbol>;

#[derive(Clone, Copy)]
struct AstStructField {
    offset: usize,
    width: usize,
    field_index: usize,
}

#[derive(Clone)]
struct AstStructType {
    width: usize,
    signed: bool,
    fields: std::collections::BTreeMap<String, AstStructField>,
    is_union: bool,
}

#[derive(Clone)]
struct AstEnumType {
    width: usize,
    signed: bool,
    values: Vec<LogicWord>,
    names: Vec<String>,
}

pub(super) struct SlangAstLowerer {
    design: Design,
    symbols: std::collections::BTreeMap<String, AstLoweredSymbol>,
    local_symbols: std::collections::BTreeMap<String, AstLoweredSymbol>,
    external_symbols: std::collections::BTreeMap<String, AstLoweredSymbol>,
    external_symbol_directions: std::collections::BTreeMap<String, eda_core::ir::Dir>,
    local_initializers: std::collections::BTreeMap<String, serde_json::Value>,
    local_blocks: std::collections::BTreeMap<String, Vec<String>>,
    constants: std::collections::BTreeMap<String, LogicWord>,
    subroutines: std::collections::BTreeMap<String, serde_json::Value>,
    type_aliases: std::collections::BTreeMap<String, AstStructType>,
    type_enums: std::collections::BTreeMap<String, AstEnumType>,
    enum_constants: std::collections::BTreeMap<String, LogicWord>,
    named_block_flags: std::collections::BTreeMap<String, usize>,
    named_disable_seen: bool,
    clocking_events: std::collections::BTreeMap<String, eda_core::ir::EventWatch>,
    default_clocking_event: Option<eda_core::ir::EventWatch>,
    clocking_variables: std::collections::BTreeMap<String, AstClockingVariable>,
    interface_modport_symbols: std::collections::BTreeMap<String, InterfaceModportSymbols>,
    active_named_blocks: std::cell::RefCell<Vec<String>>,
    generate_path: Vec<String>,
    inline_stack: std::cell::RefCell<Vec<String>>,
}

impl SlangAstLowerer {
    fn collect_compilation_unit_members(&mut self, root: &serde_json::Value) -> Result<(), String> {
        let Some(members) = root
            .get("eda_compilation_unit_members")
            .and_then(serde_json::Value::as_array)
        else {
            return Ok(());
        };
        let mut package_symbols = std::collections::BTreeSet::new();
        for member in members {
            let name = declaration_name(member, "package definition")?;
            let symbol_kind = match string_field(member, "kind").as_deref() {
                Some("Parameter") => "param",
                Some("Subroutine") => "subroutine",
                Some("EnumType") => "enum",
                Some("TypeAlias") => "alias",
                _ => "definition",
            };
            if !package_symbols.insert(format!("{symbol_kind}:{name}"))
                && (symbol_kind != "alias" || !package_symbols.contains(&format!("enum:{name}")))
            {
                return Err(format!("imported package symbol collision for '{name}'"));
            }
            match string_field(member, "kind").as_deref() {
                Some("Subroutine") => self
                    .collect_subroutine(member)
                    .map_err(|error| with_source_span(member, error))?,
                Some("TypeAlias") => self
                    .collect_type_alias(member)
                    .map_err(|error| with_source_span(member, error))?,
                Some("EnumType") => self
                    .collect_enum_type(member)
                    .map_err(|error| with_source_span(member, error))?,
                Some("Parameter") => self
                    .lower_parameter(member)
                    .map_err(|error| with_source_span(member, error))?,
                _ => {}
            }
        }
        Ok(())
    }

    pub(super) fn lower(root: &serde_json::Value) -> Result<Design, String> {
        let body = root
            .get("body")
            .ok_or_else(|| "slang AST Instance has no body".to_string())?;
        if string_field(body, "kind").as_deref() != Some("InstanceBody") {
            return Err("slang AST Instance body has unexpected kind".into());
        }
        let top_name = string_field(root, "name")
            .or_else(|| string_field(body, "name"))
            .ok_or_else(|| "slang AST Instance has no name".to_string())?;
        let mut lowerer = Self {
            design: Design {
                name: top_name.clone(),
                ..Design::default()
            },
            symbols: std::collections::BTreeMap::new(),
            local_symbols: std::collections::BTreeMap::new(),
            external_symbols: std::collections::BTreeMap::new(),
            external_symbol_directions: std::collections::BTreeMap::new(),
            local_initializers: std::collections::BTreeMap::new(),
            local_blocks: std::collections::BTreeMap::new(),
            constants: std::collections::BTreeMap::new(),
            subroutines: std::collections::BTreeMap::new(),
            type_aliases: std::collections::BTreeMap::new(),
            type_enums: std::collections::BTreeMap::new(),
            enum_constants: std::collections::BTreeMap::new(),
            named_block_flags: std::collections::BTreeMap::new(),
            named_disable_seen: false,
            clocking_events: std::collections::BTreeMap::new(),
            default_clocking_event: None,
            clocking_variables: std::collections::BTreeMap::new(),
            interface_modport_symbols: std::collections::BTreeMap::new(),
            active_named_blocks: std::cell::RefCell::new(Vec::new()),
            generate_path: Vec::new(),
            inline_stack: std::cell::RefCell::new(Vec::new()),
        };
        let root_hierarchy_index = lowerer.design.hierarchy.len();
        lowerer
            .design
            .hierarchy
            .push(eda_core::ir::HierarchicalInstance {
                path: top_name.clone(),
                module: top_name.clone(),
                parent: None,
                depth: 0,
                net_start: 0,
                net_end: 0,
                ports: Vec::new(),
            });
        lowerer.collect_compilation_unit_members(root)?;
        lowerer.lower_body(body)?;
        let top_ports = lowerer
            .design
            .ports
            .iter()
            .map(|port| eda_core::ir::HierarchicalPort {
                name: port.name.clone(),
                direction: port.dir,
                base: port.base,
                width: port.width,
                connections: vec![None; port.width],
            })
            .collect::<Vec<_>>();
        let root = &mut lowerer.design.hierarchy[root_hierarchy_index];
        root.net_end = lowerer.design.n_nets;
        root.ports = top_ports;
        Ok(lowerer.design)
    }

    fn lower_body(&mut self, body: &serde_json::Value) -> Result<(), String> {
        if string_field(body, "kind").as_deref() != Some("InstanceBody") {
            return Err("slang AST Instance body has unexpected kind".into());
        }
        let members = body
            .get("members")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "slang AST InstanceBody has no members".to_string())?;
        self.lower_members(members)
    }

    fn lower_members(&mut self, members: &[serde_json::Value]) -> Result<(), String> {
        for member in members {
            match string_field(member, "kind").as_deref() {
                Some("Subroutine") => self
                    .collect_subroutine(member)
                    .map_err(|error| with_source_span(member, error))?,
                Some("TypeAlias") => self
                    .collect_type_alias(member)
                    .map_err(|error| with_source_span(member, error))?,
                Some("EnumType") => self
                    .collect_enum_type(member)
                    .map_err(|error| with_source_span(member, error))?,
                Some("StatementBlock") => self
                    .collect_procedure_locals(member)
                    .map_err(|error| with_source_span(member, error))?,
                _ => {}
            }
        }
        for member in members {
            let kind = string_field(member, "kind").unwrap_or_default();
            match kind.as_str() {
                "Port" => self
                    .lower_port(member)
                    .map_err(|error| with_source_span(member, error))?,
                "InterfacePort" => {}
                "Net" | "Variable" => self
                    .lower_declaration(member)
                    .map_err(|error| with_source_span(member, error))?,
                "Parameter" => self
                    .lower_parameter(member)
                    .map_err(|error| with_source_span(member, error))?,
                "ContinuousAssign" => self
                    .lower_continuous_assignment(member)
                    .map_err(|error| with_source_span(member, error))?,
                "ProceduralBlock" => self
                    .lower_procedural_block(member)
                    .map_err(|error| with_source_span(member, error))?,
                "Instance" => self
                    .lower_instance(member)
                    .map_err(|error| with_source_span(member, error))?,
                "InstanceArray" => self
                    .lower_instance_array(member)
                    .map_err(|error| with_source_span(member, error))?,
                "Subroutine" => {}
                "TypeAlias" => {}
                "EnumType" => {}
                "StatementBlock" => {}
                "Modport" => {}
                "WildcardImport" => {}
                "ExplicitImport" => {}
                "ClockingBlock" => self
                    .lower_clocking_block(member)
                    .map_err(|error| with_source_span(member, error))?,
                // slang has already elaborated the loop; its per-iteration
                // GenerateBlock members carry the materialized localparam.
                "Genvar" => {}
                "GenerateBlockArray" => self
                    .lower_generate_array(member)
                    .map_err(|error| with_source_span(member, error))?,
                generate_kind if generate_kind.starts_with("GenerateBlock") => {
                    if bool_field(member, "isUninstantiated") {
                        continue;
                    }
                    let members = member
                        .get("members")
                        .and_then(serde_json::Value::as_array)
                        .ok_or_else(|| "slang GenerateBlock has no members".to_string())?;
                    if let Some(name) = string_field(member, "name").filter(|name| !name.is_empty())
                    {
                        self.generate_path.push(name);
                    }
                    let outer_subroutines = self.subroutines.clone();
                    let outer_type_aliases = self.type_aliases.clone();
                    let outer_type_enums = self.type_enums.clone();
                    let outer_enum_constants = self.enum_constants.clone();
                    let result = self.lower_members(members);
                    self.subroutines = outer_subroutines;
                    self.type_aliases = outer_type_aliases;
                    self.type_enums = outer_type_enums;
                    self.enum_constants = outer_enum_constants;
                    if !self.generate_path.is_empty()
                        && self.generate_path.last().is_some_and(|last| {
                            member
                                .get("name")
                                .and_then(serde_json::Value::as_str)
                                .is_some_and(|name| name == last)
                        })
                    {
                        self.generate_path.pop();
                    }
                    result.map_err(|error| with_source_span(member, error))?;
                }
                _ => {
                    return Err(with_source_span(
                        member,
                        format!("direct slang-AST lowering does not support member kind '{kind}'"),
                    ))
                }
            }
        }
        Ok(())
    }

    fn lower_generate_array(&mut self, member: &serde_json::Value) -> Result<(), String> {
        let array_name = declaration_name(member, "GenerateBlockArray")?;
        let members = member
            .get("members")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "slang GenerateBlockArray has no members".to_string())?;
        for (position, nested) in members.iter().enumerate() {
            let kind = string_field(nested, "kind").unwrap_or_default();
            if kind == "Genvar" {
                continue;
            }
            if !kind.starts_with("GenerateBlock") {
                return Err(format!(
                    "direct slang-AST lowering does not support generated member kind '{kind}'"
                ));
            }
            if bool_field(nested, "isUninstantiated") {
                continue;
            }
            let construct_index = nested
                .get("constructIndex")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(position as u64);
            self.generate_path
                .push(format!("{array_name}[{construct_index}]"));
            let outer_subroutines = self.subroutines.clone();
            let outer_type_aliases = self.type_aliases.clone();
            let outer_type_enums = self.type_enums.clone();
            let outer_enum_constants = self.enum_constants.clone();
            let nested_members = nested
                .get("members")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| "slang GenerateBlock has no members".to_string())?;
            let result = self.lower_members(nested_members);
            self.subroutines = outer_subroutines;
            self.type_aliases = outer_type_aliases;
            self.type_enums = outer_type_enums;
            self.enum_constants = outer_enum_constants;
            self.generate_path.pop();
            result.map_err(|error| with_source_span(member, error))?;
        }
        Ok(())
    }
}

fn declaration_name(member: &serde_json::Value, kind: &str) -> Result<String, String> {
    string_field(member, "name")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("slang AST {kind} has no name"))
}
