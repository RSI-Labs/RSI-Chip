//! Optional [slang](https://github.com/MikePopoloski/slang) front-end adapter.
//!
//! slang owns the commercial-strength lexer, preprocessor, parser, type
//! checker, and elaborator.  This crate first asks slang to validate the
//! source files, then asks it for a single preprocessed compilation unit and
//! lowers that unit through the repository HDL elaborator.  Keeping the
//! validated/preprocessed boundary explicit avoids silently accepting source
//! that the downstream bounded elaborator cannot represent.

use eda_core::ir::Design;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
pub struct SlangFrontend {
    program: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct PreprocessOptions {
    pub include_directories: Vec<PathBuf>,
    pub defines: Vec<String>,
    pub undefines: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangFailure {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// A shallow, typed view over slang's elaborated AST JSON.
///
/// This is not a full AST lowering layer yet. It is the first validation
/// boundary: after the bounded repository elaborator runs, its top-module,
/// ports, simple declarations, and direct-instance inventory are checked
/// against slang's authoritative elaboration so silent drops cannot masquerade
/// as successful compilation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangAstSummary {
    pub top_name: String,
    pub ports: Vec<SlangPortSummary>,
    pub declarations: Vec<SlangDeclarationSummary>,
    pub parameters: Vec<SlangParameterSummary>,
    pub continuous_targets: Vec<SlangContinuousTarget>,
    pub procedural_blocks: Vec<SlangProceduralBlockSummary>,
    pub instances: Vec<SlangInstanceSummary>,
    pub kinds: std::collections::BTreeSet<String>,
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangProceduralBlockSummary {
    pub procedure_kind: String,
    pub assignment_targets: Vec<String>,
}

impl std::fmt::Display for SlangFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "slang failed with status {:?}", self.status)?;
        if !self.stdout.trim().is_empty() {
            write!(f, "\nstdout:\n{}", self.stdout)?;
        }
        if !self.stderr.trim().is_empty() {
            write!(f, "\nstderr:\n{}", self.stderr)?;
        }
        Ok(())
    }
}

impl std::error::Error for SlangFailure {}

impl SlangFrontend {
    /// Finds slang through `SLANG_BIN`, `SLANG`, or `PATH`.
    pub fn discover() -> Result<Self, String> {
        if let Some(path) = std::env::var_os("SLANG_BIN") {
            let path = PathBuf::from(path);
            if path.is_file() {
                return Ok(Self { program: path });
            }
            return Err(format!(
                "SLANG_BIN does not point to slang: {}",
                path.display()
            ));
        }
        if let Some(path) = std::env::var_os("SLANG") {
            let path = PathBuf::from(path);
            if path.is_file() {
                return Ok(Self { program: path });
            }
            return Err(format!("SLANG does not point to slang: {}", path.display()));
        }
        which("slang").map(|program| Self { program })
    }

    /// Used by tests so machines without slang can still run the workspace.
    pub fn discover_optional() -> Option<Self> {
        Self::discover().ok()
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Runs slang's full parse/type/elaboration action.
    pub fn check_files(&self, files: &[PathBuf], top: Option<&str>) -> Result<(), SlangFailure> {
        self.check_files_with_options(files, top, &PreprocessOptions::default())
    }

    /// Runs slang's full action with the same include/define environment used
    /// for preprocessing.
    pub fn check_files_with_options(
        &self,
        files: &[PathBuf],
        top: Option<&str>,
        options: &PreprocessOptions,
    ) -> Result<(), SlangFailure> {
        let mut command = self.command();
        if let Some(top) = top {
            command.arg("--top").arg(top);
        }
        add_preprocessor_options(&mut command, options);
        command.args(files);
        self.run(command)
    }

    pub fn preprocess_files(
        &self,
        files: &[PathBuf],
        options: &PreprocessOptions,
    ) -> Result<String, SlangFailure> {
        let mut command = self.command();
        command.arg("--preprocess").arg("--single-unit");
        add_preprocessor_options(&mut command, options);
        command.args(files);
        let output = self.run_capturing(command)?;
        String::from_utf8(output.stdout).map_err(|error| SlangFailure {
            status: None,
            stdout: String::from_utf8_lossy(&error.into_bytes()).into_owned(),
            stderr: "slang preprocessed output was not UTF-8".into(),
        })
    }

    /// Returns slang's post-elaboration AST JSON.
    pub fn ast_json(
        &self,
        files: &[PathBuf],
        top: Option<&str>,
        scopes: &[String],
    ) -> Result<String, SlangFailure> {
        self.ast_json_with_options(files, top, scopes, &PreprocessOptions::default())
    }

    /// Returns slang's post-elaboration AST JSON using the same include and
    /// macro environment as compilation.
    pub fn ast_json_with_options(
        &self,
        files: &[PathBuf],
        top: Option<&str>,
        scopes: &[String],
        options: &PreprocessOptions,
    ) -> Result<String, SlangFailure> {
        let mut command = self.command();
        if let Some(top) = top {
            command.arg("--top").arg(top);
        }
        command.arg("--ast-json").arg("-");
        for scope in scopes {
            command.arg("--ast-json-scope").arg(scope);
        }
        add_preprocessor_options(&mut command, options);
        command.args(files);
        let output = self.run_capturing(command)?;
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let start = text.find('{').unwrap_or(0);
        Ok(text[start..].to_string())
    }

    /// Parses slang's AST JSON into the shallow authoritative summary used by
    /// post-lowering structural validation.
    pub fn summarize_ast_json(&self, json: &str) -> Result<SlangAstSummary, String> {
        SlangAstSummary::parse(json)
    }

    /// Checks source with slang, preprocesses it, and lowers the result.
    pub fn compile_files(
        &self,
        files: &[PathBuf],
        top: Option<&str>,
        options: &PreprocessOptions,
    ) -> Result<Design, String> {
        self.check_files_with_options(files, top, options)
            .map_err(|error| error.to_string())?;
        let authoritative_ast = self
            .ast_json_with_options(files, top, &[], options)
            .map_err(|error| error.to_string())?;
        let authoritative = self.summarize_ast_json(&authoritative_ast)?;
        let preprocessed = self
            .preprocess_files(files, options)
            .map_err(|error| error.to_string())?;
        let design = eda_hdl::compile(&preprocessed, top)?;
        authoritative.validate_against_design(&design)?;
        Ok(design)
    }

    /// Convenience wrapper for an in-memory source text.
    pub fn compile_source(
        &self,
        source: &str,
        top: Option<&str>,
        options: &PreprocessOptions,
    ) -> Result<Design, String> {
        let path = write_temporary_source(source)?;
        let result = self.compile_files(std::slice::from_ref(&path), top, options);
        let _ = std::fs::remove_file(&path);
        result
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.arg("--quiet");
        command
    }

    fn run(&self, command: Command) -> Result<(), SlangFailure> {
        self.run_capturing(command).map(|_| ())
    }

    fn run_capturing(&self, mut command: Command) -> Result<std::process::Output, SlangFailure> {
        command
            .output()
            .map_err(|error| SlangFailure {
                status: None,
                stdout: String::new(),
                stderr: format!("cannot execute {}: {error}", self.program.display()),
            })
            .and_then(|output| {
                if output.status.success() {
                    Ok(output)
                } else {
                    Err(SlangFailure {
                        status: output.status.code(),
                        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    })
                }
            })
    }
}

impl SlangAstSummary {
    pub fn parse(json: &str) -> Result<Self, String> {
        let start = json
            .find('{')
            .ok_or_else(|| "slang AST output did not contain a JSON object".to_string())?;
        let root: Value = serde_json::from_str(&json[start..])
            .map_err(|error| format!("invalid slang AST JSON: {error}"))?;
        let root = root.get("design").unwrap_or(&root);
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
        let members = body
            .get("members")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
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
                        continuous_targets.push(SlangContinuousTarget {
                            name: name.clone(),
                            source: "Initializer".into(),
                        });
                    }
                }
                "Parameter" => {
                    let name = string_field(member, "name")
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| "slang AST Parameter has no name".to_string())?;
                    parameters.push(SlangParameterSummary {
                        name,
                        type_name: string_field(member, "type").unwrap_or_default(),
                        value: string_field(member, "value").unwrap_or_default(),
                        is_local: bool_field(member, "isLocal"),
                        is_port: bool_field(member, "isPort"),
                    });
                }
                "ContinuousAssign" => {
                    let Some(assignment) = member.get("assignment") else {
                        continue;
                    };
                    let Some(left) = assignment.get("left") else {
                        continue;
                    };
                    for name in expression_target_names(left) {
                        continuous_targets.push(SlangContinuousTarget {
                            name,
                            source: "ContinuousAssign".into(),
                        });
                    }
                }
                "ProceduralBlock" => {
                    let procedure_kind = string_field(member, "procedureKind").unwrap_or_default();
                    let assignment_targets = procedural_target_names(member);
                    procedural_blocks.push(SlangProceduralBlockSummary {
                        procedure_kind,
                        assignment_targets,
                    });
                }
                "Instance" => {
                    let instance_name = string_field(member, "name")
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| "slang AST Instance has no name".to_string())?;
                    let definition_name = member
                        .get("body")
                        .and_then(|body| string_field(body, "name"))
                        .unwrap_or_default();
                    instances.push(SlangInstanceSummary {
                        instance_name,
                        definition_name,
                    });
                }
                generate_kind if generate_kind.starts_with("GenerateBlock") => {
                    collect_generate_summary(
                        member,
                        &mut instances,
                        &mut continuous_targets,
                        &mut procedural_blocks,
                    )?;
                }
                _ => {}
            }
        }
        let mut kinds = std::collections::BTreeSet::new();
        collect_kinds(root, &mut kinds);
        Ok(Self {
            top_name,
            ports,
            declarations,
            parameters,
            continuous_targets,
            procedural_blocks,
            instances,
            kinds,
        })
    }

    pub fn validate_against_design(&self, design: &Design) -> Result<(), String> {
        if design.name != self.top_name {
            return Err(format!(
                "bounded elaborator top '{}' does not match slang top '{}'",
                design.name, self.top_name
            ));
        }

        let actual_ports: Vec<(String, &str)> = design
            .ports
            .iter()
            .map(|port| {
                let direction = match port.dir {
                    eda_core::ir::Dir::In => "In",
                    eda_core::ir::Dir::Out => "Out",
                };
                (port.name.clone(), direction)
            })
            .collect();
        let expected_ports: Vec<(String, &str)> = self
            .ports
            .iter()
            .map(|port| (port.name.clone(), port.direction.as_str()))
            .collect();
        if actual_ports != expected_ports {
            return Err(format!(
                "bounded elaborator ports for '{}' do not match slang: internal {:?}, slang {:?}",
                self.top_name, actual_ports, expected_ports
            ));
        }

        let mut expected_declarations: std::collections::BTreeMap<String, Option<usize>> =
            std::collections::BTreeMap::new();
        for declaration in &self.declarations {
            match expected_declarations.get(&declaration.name) {
                Some(existing_width) if *existing_width != declaration.width => {
                    return Err(format!(
                        "slang declares '{}' with inconsistent widths {:?} and {:?}",
                        declaration.name, existing_width, declaration.width
                    ));
                }
                _ => {
                    expected_declarations.insert(declaration.name.clone(), declaration.width);
                }
            }
        }
        let visible_design_declarations: std::collections::BTreeMap<String, usize> = design
            .net_decls
            .iter()
            .filter(|declaration| {
                !declaration.name.starts_with("__") && !declaration.name.contains('.')
            })
            .map(|declaration| (declaration.name.clone(), declaration.width))
            .collect();
        for (name, slang_width) in expected_declarations {
            match visible_design_declarations.get(&name) {
                None => {
                    return Err(format!(
                        "bounded elaborator dropped slang declaration '{name}'"
                    ));
                }
                Some(actual_width) => {
                    if let Some(slang_width) = slang_width {
                        if *actual_width != slang_width {
                            return Err(format!(
                                "bounded elaborator declaration '{name}' width {actual_width} does not match slang width {slang_width}"
                            ));
                        }
                    }
                }
            }
        }

        let mut parameter_names = std::collections::BTreeSet::new();
        for parameter in &self.parameters {
            if !parameter_names.insert(parameter.name.clone()) {
                return Err(format!(
                    "slang declares parameter '{}' more than once",
                    parameter.name
                ));
            }
        }

        let mut expected_continuous_targets: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for target in &self.continuous_targets {
            *expected_continuous_targets
                .entry(target.name.clone())
                .or_insert(0) += 1;
        }
        let memory_names: std::collections::BTreeSet<String> = design
            .memories
            .iter()
            .map(|memory| memory.name.clone())
            .collect();
        let mut actual_continuous_targets: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for assignment in &design.assigns {
            let Some(name) = design_lvalue_name(design, &assignment.lv) else {
                continue;
            };
            if name.starts_with("__") || name.contains('.') {
                continue;
            }
            *actual_continuous_targets.entry(name).or_insert(0) += 1;
        }
        for (name, expected_count) in expected_continuous_targets {
            let represented_as_sparse_memory = memory_names.contains(&name);
            let actual_count = actual_continuous_targets.get(&name).copied().unwrap_or(0);
            if !represented_as_sparse_memory && actual_count < expected_count {
                return Err(format!(
                    "bounded elaborator dropped continuous assignments for '{name}': slang expects {expected_count}, internal has {actual_count}"
                ));
            }
        }

        let expected_initial_count = self
            .procedural_blocks
            .iter()
            .filter(|block| block.procedure_kind == "Initial")
            .count();
        if design.initials.len() < expected_initial_count {
            return Err(format!(
                "bounded elaborator dropped initial blocks: slang expects {expected_initial_count}, internal has {}",
                design.initials.len()
            ));
        }
        let expected_process_count = self
            .procedural_blocks
            .iter()
            .filter(|block| block.procedure_kind != "Initial")
            .count();
        if design.processes.len() < expected_process_count {
            return Err(format!(
                "bounded elaborator dropped procedural blocks: slang expects {expected_process_count}, internal has {}",
                design.processes.len()
            ));
        }

        let declaration_names: std::collections::BTreeSet<String> = self
            .declarations
            .iter()
            .map(|declaration| declaration.name.clone())
            .collect();
        let mut expected_procedural_targets: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for block in &self.procedural_blocks {
            for target in &block.assignment_targets {
                if declaration_names.contains(target) {
                    *expected_procedural_targets.entry(target.clone()).or_insert(0) += 1;
                }
            }
        }
        let mut actual_procedural_targets: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for initial in &design.initials {
            for name in design_stmt_target_names(design, initial) {
                if !name.starts_with("__") && !name.contains('.') {
                    *actual_procedural_targets.entry(name).or_insert(0) += 1;
                }
            }
        }
        for process in &design.processes {
            for name in design_stmt_target_names(design, &process.body) {
                if !name.starts_with("__") && !name.contains('.') {
                    *actual_procedural_targets.entry(name).or_insert(0) += 1;
                }
            }
        }
        for (name, expected_count) in expected_procedural_targets {
            let actual_count = actual_procedural_targets
                .get(&name)
                .copied()
                .unwrap_or(0);
            if actual_count < expected_count {
                return Err(format!(
                    "bounded elaborator dropped procedural assignments for '{name}': slang expects {expected_count}, internal has {actual_count}"
                ));
            }
        }

        let actual_instances: std::collections::BTreeSet<(String, String)> = design
            .hierarchy
            .iter()
            .filter(|instance| instance.depth == 1)
            .map(|instance| {
                (
                    instance
                        .path
                        .strip_prefix(&format!("{}.", self.top_name))
                        .unwrap_or(&instance.path)
                        .to_string(),
                    instance.module.clone(),
                )
            })
            .collect();
        let expected_instances: std::collections::BTreeSet<(String, String)> = self
            .instances
            .iter()
            .map(|instance| {
                (
                    instance.instance_name.clone(),
                    instance.definition_name.clone(),
                )
            })
            .collect();
        if actual_instances != expected_instances {
            return Err(format!(
                "bounded elaborator direct instances for '{}' do not match slang: internal {:?}, slang {:?}",
                self.top_name, actual_instances, expected_instances
            ));
        }
        Ok(())
    }
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_string)
}

fn bool_field(value: &Value, field: &str) -> bool {
    value.get(field).and_then(Value::as_bool).unwrap_or(false)
}

fn expression_target_names(value: &Value) -> Vec<String> {
    if string_field(value, "kind").as_deref() == Some("NamedValue") {
        let symbol = string_field(value, "symbol")
            .or_else(|| string_field(value, "name"))
            .unwrap_or_default();
        let name = symbol.split_whitespace().next_back().unwrap_or_default();
        if name.is_empty() {
            Vec::new()
        } else {
            vec![name.to_string()]
        }
    } else {
        value
            .as_object()
            .map(|fields| {
                fields
                    .values()
                    .flat_map(expression_target_names)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }
}

fn procedural_target_names(value: &Value) -> Vec<String> {
    let mut targets = Vec::new();
    if string_field(value, "kind").as_deref() == Some("Assignment") {
        if let Some(left) = value.get("left") {
            targets.extend(expression_target_names(left));
        }
    }
    match value {
        Value::Array(values) => values
            .iter()
            .for_each(|nested| targets.extend(procedural_target_names(nested))),
        Value::Object(fields) => {
            let is_assignment = string_field(value, "kind").as_deref() == Some("Assignment");
            fields
                .iter()
                .filter(|(field, _)| !(is_assignment && field.as_str() == "left"))
                .for_each(|(_, nested)| targets.extend(procedural_target_names(nested)));
        }
        _ => {}
    }
    targets
}

fn simple_type_width(type_name: &str) -> Option<usize> {
    let type_name = type_name.trim();
    if type_name.contains('$')
        || type_name.contains('{')
        || type_name.contains("class")
        || type_name.contains("iterator")
    {
        return None;
    }

    let (base, range) = if let Some(start) = type_name.rfind('[') {
        let range = type_name.get(start + 1..type_name.len().saturating_sub(1))?;
        (type_name[..start].trim(), Some(range))
    } else {
        (type_name, None)
    };
    let element_width = if base.contains("logic") || base.contains("reg") || base.contains("bit") {
        1
    } else {
        match base {
            "byte" => 8,
            "shortint" => 16,
            "int" | "integer" => 32,
            "longint" => 64,
            _ => return None,
        }
    };
    match range {
        Some(range) => {
            let (left, right) = range.split_once(':')?;
            let left = left.trim().parse::<i64>().ok()?;
            let right = right.trim().parse::<i64>().ok()?;
            if left == right {
                return None;
            }
            Some(((left as i128 - right as i128).unsigned_abs()) as usize + 1)
        }
        None => Some(element_width),
    }
}

fn collect_kinds(value: &Value, kinds: &mut std::collections::BTreeSet<String>) {
    if let Some(kind) = value.get("kind").and_then(Value::as_str) {
        kinds.insert(kind.to_string());
    }
    match value {
        Value::Array(values) => values.iter().for_each(|value| collect_kinds(value, kinds)),
        Value::Object(fields) => fields
            .values()
            .for_each(|value| collect_kinds(value, kinds)),
        _ => {}
    }
}

fn collect_generate_summary(
    value: &Value,
    instances: &mut Vec<SlangInstanceSummary>,
    continuous_targets: &mut Vec<SlangContinuousTarget>,
    procedural_blocks: &mut Vec<SlangProceduralBlockSummary>,
) -> Result<(), String> {
    let Some(members) = value.get("members").and_then(Value::as_array) else {
        return Ok(());
    };
    for member in members {
        let Some(kind) = string_field(member, "kind") else {
            continue;
        };
        match kind.as_str() {
            "Instance" => {
                let instance_name = string_field(member, "name")
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| "slang AST Instance has no name".to_string())?;
                let definition_name = member
                    .get("body")
                    .and_then(|body| string_field(body, "name"))
                    .unwrap_or_default();
                instances.push(SlangInstanceSummary {
                    instance_name,
                    definition_name,
                });
            }
            "ContinuousAssign" => {
                let Some(assignment) = member.get("assignment") else {
                    continue;
                };
                let Some(left) = assignment.get("left") else {
                    continue;
                };
                for name in expression_target_names(left) {
                    continuous_targets.push(SlangContinuousTarget {
                        name,
                        source: "ContinuousAssign".into(),
                    });
                }
            }
            "ProceduralBlock" => {
                let procedure_kind = string_field(member, "procedureKind").unwrap_or_default();
                let assignment_targets = procedural_target_names(member);
                procedural_blocks.push(SlangProceduralBlockSummary {
                    procedure_kind,
                    assignment_targets,
                });
            }
            nested if nested.starts_with("GenerateBlock") => {
                collect_generate_summary(
                    member,
                    instances,
                    continuous_targets,
                    procedural_blocks,
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn design_lvalue_name(design: &Design, lvalue: &eda_core::ir::LValue) -> Option<String> {
    design
        .net_decls
        .iter()
        .find(|declaration| {
            lvalue.net >= declaration.base && lvalue.net < declaration.base + declaration.width
        })
        .map(|declaration| declaration.name.clone())
}

fn add_preprocessor_options(command: &mut Command, options: &PreprocessOptions) {
    for directory in &options.include_directories {
        command.arg("-I").arg(directory);
    }
    for define in &options.defines {
        command.arg("-D").arg(define);
    }
    for undefine in &options.undefines {
        command.arg("-U").arg(undefine);
    }
}

fn which(program: &str) -> Result<PathBuf, String> {
    let path = std::env::var_os("PATH").ok_or("PATH is not set")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(program);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!("could not find '{program}' on PATH; set SLANG_BIN"))
}

fn write_temporary_source(source: &str) -> Result<PathBuf, String> {
    let unique = format!(
        "eda-slang-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_nanos())
            .unwrap_or(0)
    );
    let path = std::env::temp_dir().join(unique);
    std::fs::write(&path, source).map_err(|error| {
        format!(
            "cannot create temporary slang source {}: {error}",
            path.display()
        )
    })?;
    Ok(path)
}
