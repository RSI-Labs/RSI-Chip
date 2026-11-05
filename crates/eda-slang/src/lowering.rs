//! Direct lowering from slang's post-elaboration AST JSON into the repository IR.

use crate::ast_util::{
    bool_field, normalize_detailed_types, normalize_instance_bodies, string_field, with_source_span,
};
use eda_core::ir::Design;
use eda_core::logic::LogicWord;

mod classes;
mod declarations;
mod expressions;
mod functions;
mod hierarchy;
mod interface_ports;
mod statements;
mod uvm;

pub(crate) const SUPPORTED_SLANG_AST_SCHEMA: &str =
    "slang-ast-json-v1:root+definitions+source-info";

pub(crate) use interface_ports::{
    annotate_interface_array_port_connections, interface_array_port_member_count,
    interface_array_port_plan_has_members, interface_array_port_scope_paths,
    plan_interface_array_port_scopes,
};

fn collect_compilation_unit_definitions(
    value: &serde_json::Value,
    definitions: &mut Vec<serde_json::Value>,
) {
    let empty_members = Vec::new();
    let members = value
        .get("members")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty_members);
    for member in members {
        if matches!(
            string_field(member, "kind").as_deref(),
            Some("Parameter" | "Subroutine" | "TypeAlias" | "EnumType" | "Variable" | "Net")
        ) {
            let mut definition = member.clone();
            if string_field(member, "kind").as_deref() == Some("TypeAlias") {
                if let Some(fields) = definition.as_object_mut() {
                    fields.insert(
                        "eda_package_scope".into(),
                        serde_json::Value::String(string_field(value, "name").unwrap_or_default()),
                    );
                }
            }
            definitions.push(definition);
        }
    }
}

pub(crate) fn parse_ast_root(json: &str) -> Result<serde_json::Value, String> {
    let start = json
        .find('{')
        .ok_or_else(|| "slang AST output did not contain a JSON object".to_string())?;
    let document: serde_json::Value = serde_json::from_str(&json[start..])
        .map_err(|error| format!("invalid slang AST JSON: {error}"))?;
    let design = if let Some(design) = document.get("design") {
        validate_ast_schema_contract(design)?;
        if document
            .get("definitions")
            .and_then(serde_json::Value::as_array)
            .is_none()
        {
            return Err(format!(
                "unsupported slang AST schema ({SUPPORTED_SLANG_AST_SCHEMA}): definitions are not an array"
            ));
        }
        design
    } else if string_field(&document, "kind").as_deref() == Some("Instance") {
        validate_instance_schema(&document).map_err(|error| {
            format!("unsupported slang AST schema ({SUPPORTED_SLANG_AST_SCHEMA}): {error}")
        })?;
        &document
    } else {
        return Err("slang AST schema has no design object or bare root Instance".into());
    };
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
    let mut class_types = Vec::new();
    collect_class_types(&document, &mut class_types);
    let definition_source_libraries: serde_json::Map<String, serde_json::Value> = document
        .get("definitions")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|definition| {
            let library = string_field(definition, "sourceLibrary")?;
            let name = string_field(definition, "name")?;
            Some((
                name.to_string(),
                serde_json::Value::String(library.to_string()),
            ))
        })
        .collect();
    if !class_types.is_empty() {
        if let Some(fields) = root.as_object_mut() {
            fields.insert(
                "eda_class_types".into(),
                serde_json::Value::Array(class_types),
            );
        }
    }
    if !definition_source_libraries.is_empty() {
        if let Some(fields) = root.as_object_mut() {
            fields.insert(
                "eda_definition_source_libraries".into(),
                serde_json::Value::Object(definition_source_libraries),
            );
        }
    }
    normalize_instance_bodies(&mut root);
    normalize_detailed_types(&mut root);
    let definition_time_scales: std::collections::BTreeMap<String, AstTimeScale> = document
        .get("definitions")
        .and_then(serde_json::Value::as_array)
        .map(|definitions| collect_definition_time_scales(definitions))
        .unwrap_or_default();
    let global_time_precision = definition_time_scales
        .values()
        .map(|scale| scale.precision_exponent)
        .min()
        .unwrap_or(-9);
    annotate_time_scales(&mut root, &definition_time_scales);
    if let Some(fields) = root.as_object_mut() {
        fields.insert(
            "eda_global_time_precision".into(),
            serde_json::Value::from(global_time_precision),
        );
    }
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
    let explicit_import_addresses: std::collections::BTreeSet<u64> = root
        .get("body")
        .and_then(|body| body.get("members"))
        .and_then(serde_json::Value::as_array)
        .map(|members| {
            members
                .iter()
                .filter(|member| string_field(member, "kind").as_deref() == Some("ExplicitImport"))
                .filter_map(|member| string_field(member, "import"))
                .filter_map(|symbol| symbol.split_whitespace().next()?.parse::<u64>().ok())
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
                if let Some(members) = package.get("members").and_then(serde_json::Value::as_array)
                {
                    for definition in members {
                        let Some(address) =
                            definition.get("addr").and_then(serde_json::Value::as_u64)
                        else {
                            continue;
                        };
                        if explicit_import_addresses.contains(&address)
                            && !compilation_unit_members.iter().any(|existing| {
                                existing.get("addr").and_then(serde_json::Value::as_u64)
                                    == Some(address)
                            })
                        {
                            let mut exact = definition.clone();
                            if string_field(&exact, "kind").as_deref() == Some("TypeAlias") {
                                if let Some(fields) = exact.as_object_mut() {
                                    fields.insert(
                                        "eda_package_scope".into(),
                                        serde_json::Value::String(
                                            string_field(package, "name").unwrap_or_default(),
                                        ),
                                    );
                                }
                            }
                            compilation_unit_members.push(exact);
                        }
                    }
                }
            }
            // Unscoped compilation-unit declarations are visible to the selected
            // design just like imported package definitions. slang stores them
            // directly in the CompilationUnit while packages are nested members.
            collect_compilation_unit_definitions(member, &mut compilation_unit_members);
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

fn validate_ast_schema_contract(design: &serde_json::Value) -> Result<(), String> {
    let invalid = |message: String| {
        format!("unsupported slang AST schema ({SUPPORTED_SLANG_AST_SCHEMA}): {message}")
    };
    if !design.is_object() {
        return Err(invalid("design is not an object".into()));
    }
    if string_field(design, "kind").as_deref() != Some("Root") {
        return Err(invalid("design kind is not Root".into()));
    }
    if string_field(design, "name").is_none() {
        return Err(invalid("design has no name".into()));
    }
    let members = design
        .get("members")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid("design members are not an array".into()))?;
    let Some(root_instance) = members
        .iter()
        .find(|member| string_field(member, "kind").as_deref() == Some("Instance"))
    else {
        return Err(invalid("design has no elaborated root Instance".into()));
    };
    validate_instance_schema(root_instance).map_err(invalid)?;
    Ok(())
}

fn validate_instance_schema(instance: &serde_json::Value) -> Result<(), String> {
    if string_field(instance, "kind").as_deref() != Some("Instance") {
        return Err("root member kind is not Instance".into());
    }
    if string_field(instance, "name").is_none() {
        return Err("root Instance has no name".into());
    }
    for field in ["source_file", "source_line", "source_column"] {
        if instance.get(field).is_none() {
            return Err(format!("root Instance has no {field}"));
        }
    }
    let body = instance
        .get("body")
        .ok_or_else(|| "root Instance has no body".to_string())?;
    if string_field(body, "kind").as_deref() != Some("InstanceBody") {
        return Err("root Instance body kind is not InstanceBody".into());
    }
    if body
        .get("members")
        .and_then(serde_json::Value::as_array)
        .is_none()
    {
        return Err("root Instance body members are not an array".into());
    }
    Ok(())
}

pub(crate) fn lower_root(root: &serde_json::Value) -> Result<Design, String> {
    SlangAstLowerer::lower(root, None)
}

pub(crate) fn lower_root_with_recursive_depth(
    root: &serde_json::Value,
    recursive_property_depth: usize,
) -> Result<Design, String> {
    SlangAstLowerer::lower_with_recursive_depth(root, None, recursive_property_depth)
}

pub(crate) fn lower_root_with_unit_ir_cache_and_recursive_depth(
    root: &serde_json::Value,
    cache: std::sync::Arc<std::sync::Mutex<crate::SlangUnitIrCache>>,
    recursive_property_depth: usize,
) -> Result<Design, String> {
    SlangAstLowerer::lower_with_recursive_depth(root, Some(cache), recursive_property_depth)
}

fn collect_class_types(value: &serde_json::Value, classes: &mut Vec<serde_json::Value>) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("ClassType") {
            classes.push(value.clone());
        }
        for nested in fields.values() {
            collect_class_types(nested, classes);
        }
        return;
    }
    if let Some(values) = value.as_array() {
        for nested in values {
            collect_class_types(nested, classes);
        }
    }
}

fn class_definitions(
    root: &serde_json::Value,
) -> Result<std::collections::BTreeMap<String, serde_json::Value>, String> {
    let empty_classes = Vec::new();
    let classes = root
        .get("eda_class_types")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty_classes);
    let mut definitions: std::collections::BTreeMap<String, serde_json::Value> =
        std::collections::BTreeMap::new();
    for class in classes {
        let Some(name) = string_field(class, "name") else {
            continue;
        };
        let address = class.get("addr").and_then(serde_json::Value::as_u64);
        if let Some(address) = address {
            definitions.insert(format!("{name}@{address}"), class.clone());
        }
        definitions.entry(name).or_insert_with(|| class.clone());
    }
    Ok(definitions)
}

#[derive(Clone, Copy)]
pub(super) struct AstTimeScale {
    unit_magnitude: u64,
    unit_exponent: i8,
    precision_exponent: i8,
}

impl Default for AstTimeScale {
    fn default() -> Self {
        Self {
            unit_magnitude: 1,
            unit_exponent: -9,
            precision_exponent: -9,
        }
    }
}

impl AstTimeScale {
    pub(super) fn normalized_unit_exponent(&self) -> i8 {
        let zeros = match self.unit_magnitude {
            1 => 0,
            10 => 1,
            100 => 2,
            _ => 0,
        };
        self.unit_exponent.saturating_add(zeros)
    }
}

pub(super) fn body_time_scale(body: &serde_json::Value) -> AstTimeScale {
    AstTimeScale {
        unit_magnitude: body
            .get("eda_time_unit_magnitude")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1),
        unit_exponent: body
            .get("eda_time_unit_exponent")
            .and_then(serde_json::Value::as_i64)
            .and_then(|value| i8::try_from(value).ok())
            .unwrap_or(-9),
        precision_exponent: body
            .get("eda_time_precision_exponent")
            .and_then(serde_json::Value::as_i64)
            .and_then(|value| i8::try_from(value).ok())
            .unwrap_or(-9),
    }
}

fn collect_definition_time_scales(
    definitions: &[serde_json::Value],
) -> std::collections::BTreeMap<String, AstTimeScale> {
    let mut scales = std::collections::BTreeMap::new();
    for definition in definitions {
        let Some(address) = definition.get("addr").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let Some(name) = string_field(definition, "name") else {
            continue;
        };
        let scale = definition
            .get("timeScale")
            .and_then(serde_json::Value::as_str)
            .and_then(parse_time_scale)
            .unwrap_or_default();
        scales.insert(format!("{address} {name}"), scale);
    }
    scales
}

fn parse_time_scale(text: &str) -> Option<AstTimeScale> {
    let (unit, precision) = text.split_once('/')?;
    let (unit_magnitude, unit_exponent) = parse_time_component(unit.trim())?;
    let (precision_magnitude, precision_base_exponent) = parse_time_component(precision.trim())?;
    // Normalize 10ps/100ps precision into a single decimal exponent
    // (`100ps == 10^-10s`) so the global precision can be compared directly.
    let precision_exponent = precision_base_exponent + trailing_decimal_zeros(precision_magnitude)?;
    let normalized_unit_exponent = unit_exponent + trailing_decimal_zeros(unit_magnitude)?;
    if precision_exponent > normalized_unit_exponent {
        return None;
    }
    Some(AstTimeScale {
        unit_magnitude,
        unit_exponent,
        precision_exponent,
    })
}

fn parse_time_component(text: &str) -> Option<(u64, i8)> {
    let digits_end = text
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(text.len());
    let magnitude = text.get(..digits_end)?.parse::<u64>().ok()?;
    if !matches!(magnitude, 1 | 10 | 100) {
        return None;
    }
    let exponent = match text.get(digits_end..)?.trim() {
        "s" | "sec" | "seconds" => 0,
        "ms" | "millisecond" | "milliseconds" => -3,
        "us" | "microsecond" | "microseconds" => -6,
        "ns" | "nanosecond" | "nanoseconds" => -9,
        "ps" | "picosecond" | "picoseconds" => -12,
        "fs" | "femtosecond" | "femtoseconds" => -15,
        _ => return None,
    };
    Some((magnitude, exponent))
}

fn trailing_decimal_zeros(value: u64) -> Option<i8> {
    match value {
        1 => Some(0),
        10 => Some(1),
        100 => Some(2),
        _ => None,
    }
}

fn annotate_time_scales(
    value: &mut serde_json::Value,
    definitions: &std::collections::BTreeMap<String, AstTimeScale>,
) {
    if let Some(fields) = value.as_object_mut() {
        let is_instance_body = fields
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|kind| kind == "InstanceBody");
        if is_instance_body {
            if let Some(reference) = fields.get("definition").and_then(|value| value.as_str()) {
                let scale = definitions.get(reference).copied().unwrap_or_default();
                fields.insert(
                    "eda_time_unit_magnitude".into(),
                    serde_json::Value::from(scale.unit_magnitude),
                );
                fields.insert(
                    "eda_time_unit_exponent".into(),
                    serde_json::Value::from(scale.unit_exponent),
                );
                fields.insert(
                    "eda_time_precision_exponent".into(),
                    serde_json::Value::from(scale.precision_exponent),
                );
            }
        }
        for nested in fields.values_mut() {
            annotate_time_scales(nested, definitions);
        }
        return;
    }
    if let Some(values) = value.as_array_mut() {
        for nested in values {
            annotate_time_scales(nested, definitions);
        }
    }
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

#[derive(Clone)]
pub(super) struct DynamicClassSource {
    pub(super) object_key: String,
    pub(super) class_name: String,
}

pub(super) struct AstRunTestObject {
    pub(super) class_name: String,
    pub(super) root_object_key: String,
    pub(super) object_key: String,
    pub(super) phase_key: String,
    pub(super) objection_count: AstLoweredSymbol,
    pub(super) objection_drained: AstLoweredSymbol,
    pub(super) objection_timeout: u64,
    pub(super) objection_owner_counts: std::collections::BTreeMap<String, AstLoweredSymbol>,
}

pub(super) struct AstUvmInstanceOverride {
    pub(super) pattern: String,
    pub(super) original: String,
    pub(super) override_name: String,
}

pub(super) struct AstUvmInstanceAlias {
    pub(super) alias_name: String,
    pub(super) original_name: String,
    pub(super) pattern: String,
}

pub(super) struct AstUvmTypeOverride {
    pub(super) original: String,
    pub(super) override_name: String,
    pub(super) replace: bool,
}

pub(super) struct AstUvmTypeAlias {
    pub(super) alias_name: String,
    pub(super) original_name: String,
}

pub(super) struct AstUvmConfigEntry {
    pub(super) type_name: String,
    pub(super) scope: String,
    pub(super) field_name: String,
    pub(super) precedence: u32,
    pub(super) name_priority: bool,
    pub(super) type_priority: bool,
    pub(super) value: serde_json::Value,
}

#[derive(Clone)]
pub(super) struct AstUvmConfigWait {
    pub(super) type_name: String,
    pub(super) scope: String,
    pub(super) field_name: String,
    pub(super) net: usize,
}

#[derive(Clone)]
pub(super) struct AstUvmReportControls {
    pub(super) default_verbosity: Option<u64>,
    pub(super) id_verbosity: std::collections::BTreeMap<String, u64>,
    pub(super) severity_verbosity: std::collections::BTreeMap<String, u64>,
    pub(super) severity_id_verbosity: std::collections::BTreeMap<(String, String), u64>,
    pub(super) id_actions: std::collections::BTreeMap<String, u64>,
    pub(super) severity_actions: std::collections::BTreeMap<String, u64>,
    pub(super) severity_id_actions: std::collections::BTreeMap<(String, String), u64>,
    pub(super) max_quit_count: u64,
    pub(super) max_quit_overridable: bool,
    pub(super) quit_count: u64,
    pub(super) severity_counts: std::collections::BTreeMap<String, u64>,
    pub(super) id_counts: std::collections::BTreeMap<String, u64>,
}

#[derive(Clone)]
pub(super) struct AstUvmReportCatcher {
    pub(super) context_key: Option<String>,
    pub(super) object_key: String,
    pub(super) class_name: String,
    pub(super) enabled: bool,
    pub(super) method: serde_json::Value,
}

#[derive(Clone)]
pub(super) struct AstUvmCallback {
    pub(super) context_key: Option<String>,
    pub(super) target_type: String,
    pub(super) callback_type: String,
    pub(super) object_key: String,
    pub(super) class_name: String,
    pub(super) enabled: bool,
}

#[derive(Clone)]
pub(super) struct AstUvmAnalysisConnection {
    pub(super) port_key: String,
    pub(super) subscriber_key: String,
    pub(super) subscriber_class: String,
}

#[derive(Clone)]
pub(super) struct AstUvmTlmFifo {
    pub(super) capacity: usize,
    pub(super) count: AstLoweredSymbol,
    pub(super) slots: Vec<AstLoweredSymbol>,
}

#[derive(Clone)]
pub(super) struct AstUvmTlmEndpointConnection {
    pub(super) source_key: String,
    pub(super) target_key: String,
}

#[derive(Clone)]
pub(super) struct AstUvmSequencer {
    pub(super) request_ready: AstLoweredSymbol,
    pub(super) request_done: AstLoweredSymbol,
    pub(super) response_head: AstLoweredSymbol,
    pub(super) response_count: AstLoweredSymbol,
    pub(super) response_entries: Vec<AstUvmSequencerResponseEntry>,
}

#[derive(Clone)]
pub(super) struct AstUvmSequencerResponseEntry {
    pub(super) transaction_id: AstLoweredSymbol,
    pub(super) tag: AstLoweredSymbol,
    pub(super) id: AstLoweredSymbol,
    pub(super) name: AstLoweredSymbol,
    pub(super) full_name: AstLoweredSymbol,
    pub(super) parent_id: AstLoweredSymbol,
    pub(super) slots: Vec<AstLoweredSymbol>,
}

impl Default for AstUvmReportControls {
    fn default() -> Self {
        Self {
            default_verbosity: None,
            id_verbosity: std::collections::BTreeMap::new(),
            severity_verbosity: std::collections::BTreeMap::new(),
            severity_id_verbosity: std::collections::BTreeMap::new(),
            id_actions: std::collections::BTreeMap::new(),
            severity_actions: std::collections::BTreeMap::new(),
            severity_id_actions: std::collections::BTreeMap::new(),
            max_quit_count: 0,
            max_quit_overridable: true,
            quit_count: 0,
            severity_counts: std::collections::BTreeMap::new(),
            id_counts: std::collections::BTreeMap::new(),
        }
    }
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

pub(super) struct AstManualCovergroupBin {
    pub(super) name: String,
    pub(super) values: Vec<LogicWord>,
}

pub(super) struct AstManualCoverpoint {
    pub(super) sample_name: String,
    pub(super) bins: Vec<AstManualCovergroupBin>,
}

pub(super) struct AstManualCovergroup {
    pub(super) name: String,
    pub(super) sample_names: Vec<String>,
    pub(super) coverpoints: Vec<AstManualCoverpoint>,
}

pub(super) struct AstInterfaceModportSymbol {
    pub(super) direction: eda_core::ir::Dir,
    pub(super) symbol: AstLoweredSymbol,
}

pub(super) type InterfaceModportSymbols =
    std::collections::BTreeMap<String, AstInterfaceModportSymbol>;

#[derive(Clone, Copy)]
pub(super) enum DelayConstant {
    Integer(u64),
    Real(f64),
}

#[derive(Clone, PartialEq, Eq)]
struct AstStructField {
    offset: usize,
    width: usize,
    field_index: usize,
    signed: bool,
    /// Struct/union-typed field whose alias or inline definition can render recursively.
    nested_type: Option<AstNestedStruct>,
}

#[derive(Clone, PartialEq, Eq)]
enum AstNestedStruct {
    Alias(String),
    Inline(Box<AstStructType>),
}

#[derive(Clone, PartialEq, Eq)]
struct AstStructType {
    width: usize,
    signed: bool,
    fields: std::collections::BTreeMap<String, AstStructField>,
    is_union: bool,
    is_unpacked: bool,
    /// Every field is a scalar integral value or recursively renderable aggregate.
    flat_pattern: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct AstEnumType {
    width: usize,
    signed: bool,
    values: Vec<LogicWord>,
    names: Vec<String>,
}

pub(super) struct SlangAstLowerer {
    design: Design,
    unit_ir_cache: Option<std::sync::Arc<std::sync::Mutex<crate::SlangUnitIrCache>>>,
    symbols: std::collections::BTreeMap<String, AstLoweredSymbol>,
    local_symbols: std::collections::BTreeMap<String, AstLoweredSymbol>,
    external_symbols: std::collections::BTreeMap<String, AstLoweredSymbol>,
    external_symbol_directions: std::collections::BTreeMap<String, eda_core::ir::Dir>,
    local_initializers: std::collections::BTreeMap<String, Option<serde_json::Value>>,
    local_blocks: std::collections::BTreeMap<String, Vec<String>>,
    local_static_symbols: std::collections::BTreeSet<String>,
    unbounded_parameters: std::collections::BTreeSet<String>,
    constants: std::collections::BTreeMap<String, LogicWord>,
    subroutines: std::collections::BTreeMap<String, serde_json::Value>,
    type_aliases: std::collections::BTreeMap<String, AstStructType>,
    type_alias_type_names: std::collections::BTreeMap<String, String>,
    package_type_aliases: std::collections::BTreeMap<String, AstStructType>,
    package_type_alias_type_names: std::collections::BTreeMap<String, String>,
    type_enums: std::collections::BTreeMap<String, AstEnumType>,
    ambiguous_type_aliases: std::collections::BTreeSet<String>,
    ambiguous_type_enums: std::collections::BTreeSet<String>,
    class_definitions: std::collections::BTreeMap<String, serde_json::Value>,
    class_objects:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, AstLoweredSymbol>>,
    class_object_types: std::collections::BTreeMap<String, String>,
    dynamic_class_sources: std::collections::BTreeMap<String, Vec<DynamicClassSource>>,
    runtime_dynamic_class_sources:
        std::cell::RefCell<std::collections::BTreeMap<String, Vec<DynamicClassSource>>>,
    class_object_parent_keys: std::cell::RefCell<std::collections::BTreeMap<String, String>>,
    class_object_children: std::cell::RefCell<std::collections::BTreeMap<String, Vec<String>>>,
    class_object_declared_names: std::cell::RefCell<std::collections::BTreeMap<String, String>>,
    class_object_declared_full_names:
        std::cell::RefCell<std::collections::BTreeMap<String, String>>,
    initialized_class_objects: std::collections::BTreeSet<String>,
    constructed_class_objects: std::collections::BTreeSet<String>,
    class_object_tags: std::collections::BTreeMap<String, AstLoweredSymbol>,
    class_object_ids: std::collections::BTreeMap<String, AstLoweredSymbol>,
    class_object_names: std::collections::BTreeMap<String, AstLoweredSymbol>,
    class_object_full_names: std::collections::BTreeMap<String, AstLoweredSymbol>,
    class_object_parent_ids: std::collections::BTreeMap<String, AstLoweredSymbol>,
    class_object_serial: Option<AstLoweredSymbol>,
    class_static_methods: std::collections::BTreeMap<String, String>,
    class_static_properties: std::collections::BTreeMap<String, AstLoweredSymbol>,
    class_static_initializer_count: usize,
    class_object_arrays:
        std::collections::BTreeMap<String, std::collections::BTreeMap<i64, String>>,
    class_array_constructor_temps:
        std::collections::BTreeMap<String, Vec<(String, AstLoweredSymbol)>>,
    class_property_owners: std::collections::BTreeMap<usize, (String, String)>,
    class_property_temps: std::collections::BTreeMap<(String, String), AstLoweredSymbol>,
    class_property_initializers:
        std::collections::BTreeMap<String, Vec<(String, serde_json::Value)>>,
    run_test_objects: std::collections::BTreeMap<String, AstRunTestObject>,
    uvm_factory_type_aliases: Vec<AstUvmTypeAlias>,
    uvm_factory_instance_aliases: Vec<AstUvmInstanceAlias>,
    uvm_factory_overrides: Vec<AstUvmTypeOverride>,
    uvm_factory_instance_overrides: Vec<AstUvmInstanceOverride>,
    uvm_config_entries: std::cell::RefCell<Vec<AstUvmConfigEntry>>,
    uvm_config_waits:
        std::cell::RefCell<std::collections::BTreeMap<(String, u64, u64), AstUvmConfigWait>>,
    uvm_report_controls: std::cell::RefCell<AstUvmReportControls>,
    uvm_report_catcher: std::cell::RefCell<Vec<AstUvmReportCatcher>>,
    uvm_callbacks: std::cell::RefCell<Vec<AstUvmCallback>>,
    uvm_analysis_connections: std::cell::RefCell<Vec<AstUvmAnalysisConnection>>,
    uvm_tlm_fifos: std::cell::RefCell<std::collections::BTreeMap<String, AstUvmTlmFifo>>,
    uvm_tlm_endpoint_connections: std::cell::RefCell<Vec<AstUvmTlmEndpointConnection>>,
    uvm_sequencers: std::cell::RefCell<std::collections::BTreeMap<String, AstUvmSequencer>>,
    uvm_sequence_contexts: std::cell::RefCell<std::collections::BTreeMap<String, String>>,
    uvm_sequence_items: std::cell::RefCell<std::collections::BTreeMap<String, String>>,
    uvm_sequence_response_types: std::cell::RefCell<std::collections::BTreeMap<String, String>>,
    uvm_sequence_transaction_ids:
        std::cell::RefCell<std::collections::BTreeMap<String, AstLoweredSymbol>>,
    active_uvm_phase: std::cell::RefCell<Option<String>>,
    class_composition_children: std::collections::BTreeMap<(String, String), String>,
    class_composition_parents: std::collections::BTreeMap<String, String>,
    active_composition_classes: std::collections::BTreeSet<String>,
    active_class_objects: std::cell::RefCell<Vec<String>>,
    next_synthetic_class_handle: u64,
    enum_constants: std::collections::BTreeMap<String, LogicWord>,
    named_block_flags: std::collections::BTreeMap<String, usize>,
    named_disable_seen: bool,
    clocking_events: std::collections::BTreeMap<String, eda_core::ir::EventWatch>,
    default_clocking_event: Option<eda_core::ir::EventWatch>,
    clocking_variables: std::collections::BTreeMap<String, AstClockingVariable>,
    interface_modport_symbols: std::collections::BTreeMap<String, InterfaceModportSymbols>,
    interface_modport_names: std::collections::BTreeMap<String, String>,
    plusargs: std::collections::BTreeMap<String, AstLoweredSymbol>,
    plusarg_values: std::collections::BTreeMap<String, AstLoweredSymbol>,
    time_scale: AstTimeScale,
    global_time_precision: i8,
    active_named_blocks: std::cell::RefCell<Vec<String>>,
    active_assertion_scopes: std::cell::RefCell<Vec<String>>,
    known_assertion_scopes: std::collections::BTreeSet<String>,
    manual_covergroups: std::collections::BTreeMap<String, AstManualCovergroup>,
    covergroup_instances: std::collections::BTreeMap<String, String>,
    constructed_covergroups: std::cell::RefCell<std::collections::BTreeSet<String>>,
    generate_path: Vec<String>,
    scope_path: String,
    library_scopes: std::collections::BTreeMap<String, String>,
    format_library_scope: String,
    inline_stack: std::cell::RefCell<Vec<String>>,
    past_sampling_allowed: bool,
    past_sampling_event: Option<eda_core::ir::Sens>,
    recursive_property_depth: usize,
}

impl SlangAstLowerer {
    pub(super) fn current_format_hierarchy_path(&self) -> String {
        let mut path = self.scope_path.clone();
        for component in self
            .generate_path
            .iter()
            .chain(self.active_named_blocks.borrow().iter())
        {
            let component = component.split_whitespace().next_back().unwrap_or_default();
            if component.is_empty() {
                continue;
            }
            if !path.is_empty() {
                path.push('.');
            }
            path.push_str(component);
        }
        path
    }

    pub(super) fn current_format_library_scope(&self) -> String {
        self.format_library_scope.clone()
    }

    pub(super) fn scale_delay(&self, delay: u64) -> Result<u64, String> {
        let exponent =
            i16::from(self.time_scale.unit_exponent) - i16::from(self.global_time_precision);
        if exponent < 0 {
            return Err("module time unit is less precise than the global time precision".into());
        }
        let scale = 10_u64
            .checked_pow(
                u32::try_from(exponent).map_err(|_| "time scale exponent overflows".to_string())?,
            )
            .and_then(|scale| scale.checked_mul(self.time_scale.unit_magnitude))
            .and_then(|scale| delay.checked_mul(scale))
            .ok_or_else(|| "scaled delay overflows".to_string())?;
        Ok(scale)
    }

    pub(super) fn scale_delay_constant(&self, delay: DelayConstant) -> Result<u64, String> {
        match delay {
            DelayConstant::Integer(delay) => self.scale_delay(delay),
            DelayConstant::Real(delay) => self.scale_real_delay(delay),
        }
    }

    fn scale_real_delay(&self, delay: f64) -> Result<u64, String> {
        if !delay.is_finite() || delay < 0.0 {
            return Err("delay control must be a nonnegative finite real constant".into());
        }
        let exponent =
            i16::from(self.time_scale.unit_exponent) - i16::from(self.global_time_precision);
        if exponent < 0 {
            return Err("module time unit is less precise than the global time precision".into());
        }
        let decimal = format!("{delay:?}");
        let mut numerator = 0_u128;
        let mut fractional_digits = 0_usize;
        let mut scientific_exponent = 0_i32;
        let mut seen_digit = false;
        let mut seen_point = false;
        let mut negative = false;
        let mut index = 0_usize;
        let bytes = decimal.as_bytes();
        while index < bytes.len() {
            let byte = bytes[index];
            match byte {
                b'-' => {
                    if index != 0 || negative {
                        return Err(invalid_delay_decimal(&decimal));
                    }
                    negative = true;
                }
                b'0'..=b'9' => {
                    seen_digit = true;
                    numerator = numerator
                        .checked_mul(10)
                        .and_then(|value| value.checked_add(u128::from(byte - b'0')))
                        .ok_or_else(|| invalid_delay_decimal(&decimal))?;
                    if seen_point {
                        fractional_digits += 1;
                    }
                }
                b'.' => {
                    if seen_point || index == 0 {
                        return Err(invalid_delay_decimal(&decimal));
                    }
                    seen_point = true;
                }
                b'e' | b'E' => {
                    let suffix = decimal
                        .get(index + 1..)
                        .ok_or_else(|| invalid_delay_decimal(&decimal))?;
                    scientific_exponent = suffix
                        .parse::<i32>()
                        .map_err(|_| invalid_delay_decimal(&decimal))?;
                    break;
                }
                _ => return Err(invalid_delay_decimal(&decimal)),
            }
            index += 1;
        }
        if !seen_digit || negative {
            return Err(invalid_delay_decimal(&decimal));
        }
        let effective_exponent = i64::from(exponent)
            - i64::try_from(fractional_digits).unwrap_or(i64::MAX)
            + i64::from(scientific_exponent);
        let (scale, denominator) = if effective_exponent >= 0 {
            (
                10_u128
                    .checked_pow(
                        u32::try_from(effective_exponent)
                            .map_err(|_| "fractional delay scale exponent overflows".to_string())?,
                    )
                    .and_then(|scale| scale.checked_mul(u128::from(self.time_scale.unit_magnitude)))
                    .ok_or_else(|| "scaled fractional delay overflows".to_string())?,
                1_u128,
            )
        } else {
            (
                u128::from(self.time_scale.unit_magnitude),
                10_u128
                    .checked_pow(
                        u32::try_from(-effective_exponent)
                            .map_err(|_| "fractional delay scale exponent overflows".to_string())?,
                    )
                    .ok_or_else(|| "fractional delay denominator overflows".to_string())?,
            )
        };
        let scaled_numerator = numerator
            .checked_mul(scale)
            .ok_or_else(|| "scaled fractional delay overflows".to_string())?;
        let quotient = scaled_numerator / denominator;
        let remainder = scaled_numerator % denominator;
        let rounded = if remainder
            .checked_add(remainder)
            .is_some_and(|doubled| doubled >= denominator)
        {
            quotient.checked_add(1)
        } else {
            Some(quotient)
        }
        .ok_or_else(|| "scaled fractional delay overflows".to_string())?;
        u64::try_from(rounded).map_err(|_| "scaled fractional delay overflows".to_string())
    }

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
                Some("Variable") => "variable",
                Some("Net") => "net",
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
        }
        for member in members {
            if string_field(member, "kind").as_deref() == Some("TypeAlias") {
                self.collect_type_alias(member)
                    .map_err(|error| with_source_span(member, error))?;
            }
        }
        for member in members {
            match string_field(member, "kind").as_deref() {
                Some("Subroutine") => self
                    .collect_subroutine(member)
                    .map_err(|error| with_source_span(member, error))?,
                Some("EnumType") => self
                    .collect_enum_type(member)
                    .map_err(|error| with_source_span(member, error))?,
                Some("Parameter") => self
                    .lower_parameter(member)
                    .map_err(|error| with_source_span(member, error))?,
                Some("Variable") | Some("Net") => self
                    .lower_declaration(member)
                    .map_err(|error| with_source_span(member, error))?,
                _ => {}
            }
        }
        Ok(())
    }

    pub(super) fn lower(
        root: &serde_json::Value,
        unit_ir_cache: Option<std::sync::Arc<std::sync::Mutex<crate::SlangUnitIrCache>>>,
    ) -> Result<Design, String> {
        Self::lower_with_recursive_depth(root, unit_ir_cache, 2)
    }

    pub(super) fn lower_with_recursive_depth(
        root: &serde_json::Value,
        unit_ir_cache: Option<std::sync::Arc<std::sync::Mutex<crate::SlangUnitIrCache>>>,
        recursive_property_depth: usize,
    ) -> Result<Design, String> {
        if recursive_property_depth > 16 {
            return Err("recursive property depth must stay within 0..=16".into());
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
        let class_definitions = class_definitions(root)?;
        let library_scopes = definition_library_scopes(root);
        let format_library_scope = library_scopes
            .get(&top_name)
            .cloned()
            .unwrap_or_else(|| top_name.clone());
        let mut lowerer = Self {
            design: Design {
                name: top_name.clone(),
                time_scale: eda_core::ir::TimeScale {
                    precision_exponent: root
                        .get("eda_global_time_precision")
                        .and_then(serde_json::Value::as_i64)
                        .and_then(|value| i8::try_from(value).ok())
                        .unwrap_or(-9),
                },
                ..Design::default()
            },
            unit_ir_cache,
            symbols: std::collections::BTreeMap::new(),
            local_symbols: std::collections::BTreeMap::new(),
            external_symbols: std::collections::BTreeMap::new(),
            external_symbol_directions: std::collections::BTreeMap::new(),
            local_initializers: std::collections::BTreeMap::new(),
            local_blocks: std::collections::BTreeMap::new(),
            local_static_symbols: std::collections::BTreeSet::new(),
            unbounded_parameters: std::collections::BTreeSet::new(),
            constants: std::collections::BTreeMap::new(),
            subroutines: std::collections::BTreeMap::new(),
            type_aliases: std::collections::BTreeMap::new(),
            type_alias_type_names: std::collections::BTreeMap::new(),
            package_type_aliases: std::collections::BTreeMap::new(),
            package_type_alias_type_names: std::collections::BTreeMap::new(),
            type_enums: std::collections::BTreeMap::new(),
            ambiguous_type_aliases: std::collections::BTreeSet::new(),
            ambiguous_type_enums: std::collections::BTreeSet::new(),
            class_definitions,
            class_objects: std::collections::BTreeMap::new(),
            class_object_types: std::collections::BTreeMap::new(),
            dynamic_class_sources: std::collections::BTreeMap::new(),
            runtime_dynamic_class_sources: std::cell::RefCell::new(
                std::collections::BTreeMap::new(),
            ),
            class_object_parent_keys: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            class_object_children: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            class_object_declared_names: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            class_object_declared_full_names: std::cell::RefCell::new(
                std::collections::BTreeMap::new(),
            ),
            initialized_class_objects: std::collections::BTreeSet::new(),
            constructed_class_objects: std::collections::BTreeSet::new(),
            class_object_tags: std::collections::BTreeMap::new(),
            class_object_ids: std::collections::BTreeMap::new(),
            class_object_names: std::collections::BTreeMap::new(),
            class_object_full_names: std::collections::BTreeMap::new(),
            class_object_parent_ids: std::collections::BTreeMap::new(),
            class_object_serial: None,
            class_static_methods: std::collections::BTreeMap::new(),
            class_static_properties: std::collections::BTreeMap::new(),
            class_static_initializer_count: 0,
            class_object_arrays: std::collections::BTreeMap::new(),
            class_array_constructor_temps: std::collections::BTreeMap::new(),
            class_property_owners: std::collections::BTreeMap::new(),
            class_property_temps: std::collections::BTreeMap::new(),
            class_property_initializers: std::collections::BTreeMap::new(),
            run_test_objects: std::collections::BTreeMap::new(),
            uvm_factory_type_aliases: Vec::new(),
            uvm_factory_instance_aliases: Vec::new(),
            uvm_factory_overrides: Vec::new(),
            uvm_factory_instance_overrides: Vec::new(),
            uvm_config_entries: std::cell::RefCell::new(Vec::new()),
            uvm_config_waits: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            uvm_report_controls: std::cell::RefCell::new(AstUvmReportControls::default()),
            uvm_report_catcher: std::cell::RefCell::new(Vec::new()),
            uvm_callbacks: std::cell::RefCell::new(Vec::new()),
            uvm_analysis_connections: std::cell::RefCell::new(Vec::new()),
            uvm_tlm_fifos: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            uvm_tlm_endpoint_connections: std::cell::RefCell::new(Vec::new()),
            uvm_sequencers: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            uvm_sequence_contexts: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            uvm_sequence_items: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            uvm_sequence_response_types: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            uvm_sequence_transaction_ids: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            active_uvm_phase: std::cell::RefCell::new(None),
            class_composition_children: std::collections::BTreeMap::new(),
            class_composition_parents: std::collections::BTreeMap::new(),
            active_composition_classes: std::collections::BTreeSet::new(),
            active_class_objects: std::cell::RefCell::new(Vec::new()),
            next_synthetic_class_handle: u64::MAX / 2,
            enum_constants: std::collections::BTreeMap::new(),
            named_block_flags: std::collections::BTreeMap::new(),
            named_disable_seen: false,
            clocking_events: std::collections::BTreeMap::new(),
            default_clocking_event: None,
            clocking_variables: std::collections::BTreeMap::new(),
            interface_modport_symbols: std::collections::BTreeMap::new(),
            interface_modport_names: std::collections::BTreeMap::new(),
            plusargs: std::collections::BTreeMap::new(),
            plusarg_values: std::collections::BTreeMap::new(),
            time_scale: body_time_scale(body),
            global_time_precision: root
                .get("eda_global_time_precision")
                .and_then(serde_json::Value::as_i64)
                .and_then(|value| i8::try_from(value).ok())
                .unwrap_or(-9),
            active_named_blocks: std::cell::RefCell::new(Vec::new()),
            active_assertion_scopes: std::cell::RefCell::new(Vec::new()),
            known_assertion_scopes: std::collections::BTreeSet::new(),
            manual_covergroups: std::collections::BTreeMap::new(),
            covergroup_instances: std::collections::BTreeMap::new(),
            constructed_covergroups: std::cell::RefCell::new(std::collections::BTreeSet::new()),
            generate_path: Vec::new(),
            scope_path: top_name.clone(),
            library_scopes,
            format_library_scope,
            inline_stack: std::cell::RefCell::new(Vec::new()),
            past_sampling_allowed: false,
            past_sampling_event: None,
            recursive_property_depth,
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
        lowerer.prepare_plusargs(body)?;
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
        self.collect_dynamic_class_sources(body)?;
        self.validate_dynamic_class_sources()?;
        self.collect_class_static_members()?;
        self.prepare_run_test_objects(body)?;
        let members = body
            .get("members")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "slang AST InstanceBody has no members".to_string())?;
        self.lower_members(members)
    }

    fn lower_members(&mut self, members: &[serde_json::Value]) -> Result<(), String> {
        loop {
            let type_alias_count = self.type_aliases.len();
            for member in members {
                match string_field(member, "kind").as_deref() {
                    Some("TypeAlias") => self
                        .collect_type_alias(member)
                        .map_err(|error| with_source_span(member, error))?,
                    Some("TypeParameter") => self
                        .collect_type_parameter(member)
                        .map_err(|error| with_source_span(member, error))?,
                    _ => {}
                }
            }
            if self.type_aliases.len() == type_alias_count {
                break;
            }
        }
        for member in members {
            match string_field(member, "kind").as_deref() {
                Some("Subroutine") => self
                    .collect_subroutine(member)
                    .map_err(|error| with_source_span(member, error))?,
                Some("EnumType") => self
                    .collect_enum_type(member)
                    .map_err(|error| with_source_span(member, error))?,
                Some("StatementBlock") => self
                    .collect_procedure_locals(member)
                    .map_err(|error| with_source_span(member, error))?,
                Some("CovergroupType") => self
                    .collect_manual_covergroup(member)
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
                "ConcurrentAssertion" => self
                    .lower_concurrent_assertion(member)
                    .map_err(|error| with_source_span(member, error))?,
                "CovergroupType" => self
                    .lower_covergroup(member)
                    .map_err(|error| with_source_span(member, error))?,
                // slang inlines safe assertion instances into their use sites;
                // the declaration itself has no independent runtime behavior.
                "Sequence" | "Property" => {}
                "Checker" => {}
                "CheckerInstance" => self
                    .lower_checker_instance(member)
                    .map_err(|error| with_source_span(member, error))?,
                "Instance" => self
                    .lower_instance(member)
                    .map_err(|error| with_source_span(member, error))?,
                "InstanceArray" => self
                    .lower_instance_array(member)
                    .map_err(|error| with_source_span(member, error))?,
                "Subroutine" => {}
                "ClassType" => {}
                "TypeParameter" => {}
                // slang resolves defparam targets during elaboration and emits
                // the specialized child body/value; the residual member is not
                // a runtime statement.
                "DefParam" => {}
                "TypeAlias" => {}
                "ForwardingTypedef" => {}
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

fn definition_library_scopes(
    root: &serde_json::Value,
) -> std::collections::BTreeMap<String, String> {
    if let Some(libraries) = root
        .get("eda_definition_source_libraries")
        .and_then(serde_json::Value::as_object)
    {
        return libraries
            .iter()
            .map(|(name, library)| {
                let library = library.as_str().unwrap_or_default();
                let scope = if library.is_empty() {
                    name.clone()
                } else {
                    format!("{library}.{name}")
                };
                (name.clone(), scope)
            })
            .collect();
    }
    let empty = Vec::new();
    root.get("definitions")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty)
        .iter()
        .filter_map(|definition| {
            let name = string_field(definition, "name")?;
            let scope = match string_field(definition, "sourceLibrary") {
                Some(library) if !library.is_empty() => format!("{library}.{name}"),
                _ => name.clone(),
            };
            Some((name, scope))
        })
        .collect()
}

fn invalid_delay_decimal(decimal: &str) -> String {
    format!("invalid fractional delay constant '{decimal}'")
}
