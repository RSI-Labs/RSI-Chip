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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangExpressionSummary {
    pub kind: String,
    pub operator: String,
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
    pub operands: Vec<SlangExpressionSummary>,
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
        command.arg("--ast-json-source-info");
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
        let source_dir = files.first().and_then(|file| file.parent());
        let design = eda_hdl::compile_with_source_dir(&preprocessed, top, source_dir)?;
        authoritative.validate_against_design(&design)?;
        Ok(design)
    }

    /// Compiles through slang and lowers a deliberately small subset directly
    /// from slang's elaborated AST JSON.
    ///
    /// Unsupported constructs return explicit errors; this path never silently
    /// falls back to preprocessing and reparsing the source text.
    pub fn compile_files_via_ast(
        &self,
        files: &[PathBuf],
        top: Option<&str>,
        options: &PreprocessOptions,
    ) -> Result<Design, String> {
        self.check_files_with_options(files, top, options)
            .map_err(|error| error.to_string())?;
        let json = self
            .ast_json_with_options(files, top, &[], options)
            .map_err(|error| error.to_string())?;
        self.lower_ast_json(&json)
    }

    /// Directly lowers the current simple-module AST subset into `Design`.
    pub fn lower_ast_json(&self, json: &str) -> Result<Design, String> {
        let root = parse_ast_root(json)?;
        let mut design = SlangAstLowerer::lower(&root)?;
        design.infer_memory_kinds();
        let authoritative = SlangAstSummary::parse(json)?;
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

fn parse_ast_root(json: &str) -> Result<serde_json::Value, String> {
    let start = json
        .find('{')
        .ok_or_else(|| "slang AST output did not contain a JSON object".to_string())?;
    let root: serde_json::Value = serde_json::from_str(&json[start..])
        .map_err(|error| format!("invalid slang AST JSON: {error}"))?;
    let root = root.get("design").unwrap_or(&root);
    let root = if string_field(root, "kind").as_deref() == Some("Root") {
        root.get("members")
            .and_then(serde_json::Value::as_array)
            .and_then(|members| {
                members
                    .iter()
                    .find(|member| string_field(member, "kind").as_deref() == Some("Instance"))
            })
            .ok_or_else(|| "slang AST Root has no elaborated Instance".to_string())?
    } else {
        root
    };
    if string_field(root, "kind").as_deref() != Some("Instance") {
        return Err("slang AST root is not an elaborated Instance".into());
    }
    Ok(root.clone())
}

#[derive(Clone, Copy)]
struct AstLoweredSymbol {
    base: usize,
    width: usize,
    signed: bool,
    memory: Option<usize>,
}

struct AstMemoryType {
    element_width: usize,
    element_msb: i64,
    element_lsb: i64,
    dimensions: Vec<eda_core::ir::MemoryDimension>,
    flat_length: u64,
    signed: bool,
}

struct SlangAstLowerer {
    design: Design,
    symbols: std::collections::BTreeMap<String, AstLoweredSymbol>,
}

impl SlangAstLowerer {
    fn lower(root: &serde_json::Value) -> Result<Design, String> {
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
            let kind = string_field(member, "kind").unwrap_or_default();
            match kind.as_str() {
                "Port" => self.lower_port(member)?,
                "Net" | "Variable" => self.lower_declaration(member)?,
                "Parameter" => continue,
                "ContinuousAssign" => self.lower_continuous_assignment(member)?,
                "ProceduralBlock" => self.lower_procedural_block(member)?,
                "Instance" => self.lower_instance(member)?,
                generate_kind if generate_kind.starts_with("GenerateBlock") => {
                    if bool_field(member, "isUninstantiated") {
                        continue;
                    }
                    let members = member
                        .get("members")
                        .and_then(serde_json::Value::as_array)
                        .ok_or_else(|| "slang GenerateBlock has no members".to_string())?;
                    self.lower_members(members)?;
                }
                _ => {
                    return Err(format!(
                        "direct slang-AST lowering does not support member kind '{kind}'"
                    ))
                }
            }
        }
        Ok(())
    }

    fn lower_instance(&mut self, member: &serde_json::Value) -> Result<(), String> {
        let instance_name = declaration_name(member, "Instance")?;
        let body = member
            .get("body")
            .ok_or_else(|| "slang AST Instance has no body".to_string())?;
        let definition_name = string_field(body, "name").unwrap_or_default();
        let mut child = Self {
            design: Design::default(),
            symbols: std::collections::BTreeMap::new(),
        };
        child.lower_body(body)?;

        let offset = self.design.n_nets;
        let memory_offset = self.design.memories.len();
        let child_net_start = offset;
        let child_ports = child.design.ports.clone();
        let child_memories = child.design.memories.clone();
        for memory in &child.design.memories {
            let mut memory = memory.clone();
            memory.name = format!("{instance_name}.{}", memory.name);
            self.design.memories.push(memory);
        }
        for mut connection in child.design.memory_connections {
            connection.child += memory_offset;
            connection.parent += memory_offset;
            self.design.memory_connections.push(connection);
        }
        for declaration in &child.design.net_decls {
            self.design.net_decls.push(eda_core::ir::NetDecl {
                name: format!("{instance_name}.{}", declaration.name),
                width: declaration.width,
                is_reg: declaration.is_reg,
                is_signed: declaration.is_signed,
                base: declaration.base + offset,
            });
        }
        self.design.n_nets += child.design.n_nets;

        for mut assignment in child.design.assigns {
            remap_assignment(&mut assignment, offset);
            remap_expression_memory_ids(&mut assignment.e, memory_offset);
            self.design.assigns.push(assignment);
        }
        for mut statement in child.design.initials {
            remap_statement(&mut statement, offset);
            remap_statement_memory_ids(&mut statement, memory_offset);
            self.design.initials.push(statement);
        }
        for mut process in child.design.processes {
            remap_process(&mut process, offset);
            remap_statement_memory_ids(&mut process.body, memory_offset);
            self.design.processes.push(process);
        }

        let nested_hierarchy = child.design.hierarchy;

        let mut connections = std::collections::BTreeMap::new();
        let empty_connections = Vec::new();
        let memory_ports: std::collections::BTreeMap<String, usize> = body
            .get("members")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&empty_connections)
            .iter()
            .filter_map(|port| {
                let name = string_field(port, "name")?;
                let is_inout_memory = string_field(port, "direction").as_deref() == Some("InOut")
                    && memory_type(&string_field(port, "type").unwrap_or_default()).is_some();
                if !is_inout_memory {
                    return None;
                }
                let local_index = child_memories
                    .iter()
                    .position(|memory| memory.name == name)?;
                Some((name, local_index))
            })
            .collect();
        for connection in member
            .get("connections")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&empty_connections)
        {
            let port_name = connection
                .get("port")
                .and_then(|port| string_field(port, "name"))
                .ok_or_else(|| "instance connection has no port".to_string())?;
            let Some(port) = child_ports.iter().find(|port| port.name == port_name) else {
                return Err(format!(
                    "instance '{instance_name}' connects unknown child port '{port_name}'"
                ));
            };
            let expression = connection
                .get("expr")
                .ok_or_else(|| "instance connection has no expression".to_string())?;
            if let Some(local_memory) = memory_ports.get(&port_name).copied() {
                let parent_target = expression
                    .get("left")
                    .ok_or_else(|| "inout memory connection has no parent target".to_string())?;
                let parent_name = expression_symbol(parent_target).ok_or_else(|| {
                    format!("inout memory connection '{port_name}' has no parent symbol")
                })?;
                let parent_symbol = self
                    .symbols
                    .get(&parent_name)
                    .ok_or_else(|| format!("unknown inout memory parent '{parent_name}'"))?;
                let parent_memory = parent_symbol.memory.ok_or_else(|| {
                    format!("inout memory port '{port_name}' must connect to a sparse memory")
                })?;
                let child_memory = memory_offset + local_memory;
                let child = &self.design.memories[child_memory];
                let parent = &self.design.memories[parent_memory];
                if child.element_width != parent.element_width
                    || child.element_msb != parent.element_msb
                    || child.element_lsb != parent.element_lsb
                    || child.dimensions != parent.dimensions
                    || child.flat_length != parent.flat_length
                {
                    return Err(format!(
                        "instance '{instance_name}' inout memory port '{port_name}' shape does not match parent memory '{parent_name}'"
                    ));
                }
                self.design
                    .memory_connections
                    .push(eda_core::ir::MemoryConnection {
                        child: child_memory,
                        parent: parent_memory,
                    });
                continue;
            }
            if string_field(expression, "kind").as_deref() == Some("EmptyArgument") {
                continue;
            }
            match port.dir {
                eda_core::ir::Dir::In => {
                    let rhs = self.lower_expression(expression)?;
                    if let Some(parent_bits) = expression_parent_bits(&rhs) {
                        connections.insert(port_name.to_string(), parent_bits);
                    }
                    self.design.assigns.push(eda_core::ir::Assign {
                        lv: eda_core::ir::LValue {
                            net: port.base + offset,
                            decl_width: port.width,
                            range: full_range(port.width),
                        },
                        e: rhs,
                        delay: 0,
                    });
                }
                eda_core::ir::Dir::Out => {
                    if string_field(expression, "kind").as_deref() != Some("Assignment") {
                        return Err(format!(
                            "instance '{instance_name}' output port '{port_name}' has no parent assignment"
                        ));
                    }
                    let parent_target = expression
                        .get("left")
                        .ok_or_else(|| "output connection has no parent target".to_string())?;
                    let target = self.lower_lvalue(parent_target)?;
                    if let Some(parent_bits) = lvalue_parent_bits(&target) {
                        connections.insert(port_name.to_string(), parent_bits);
                    }
                    self.design.assigns.push(eda_core::ir::Assign {
                        lv: target,
                        e: eda_core::ir::Expr::Ref {
                            net: port.base + offset,
                            decl_width: port.width,
                            range: full_range(port.width),
                            signed: false,
                        },
                        delay: 0,
                    });
                }
            }
        }

        let ports = child_ports
            .iter()
            .map(|port| eda_core::ir::HierarchicalPort {
                name: port.name.clone(),
                direction: port.dir,
                base: port.base + offset,
                width: port.width,
                connections: connections
                    .get(&port.name)
                    .cloned()
                    .unwrap_or_else(|| vec![None; port.width]),
            })
            .collect::<Vec<_>>();
        self.design
            .hierarchy
            .push(eda_core::ir::HierarchicalInstance {
                path: format!("{}.{}", self.design.name, instance_name),
                module: definition_name,
                parent: Some(self.design.name.clone()),
                depth: 1,
                net_start: child_net_start,
                net_end: self.design.n_nets,
                ports,
            });
        let child_path = format!("{}.{}", self.design.name, instance_name);
        for mut nested in nested_hierarchy {
            nested.path = rewrite_hierarchy_path(&nested.path, &child_path);
            if let Some(parent) = nested.parent.as_deref() {
                nested.parent = Some(rewrite_hierarchy_path(parent, &child_path));
            }
            nested.depth += 1;
            nested.net_start += offset;
            nested.net_end += offset;
            for port in &mut nested.ports {
                port.base += offset;
                port.connections = port
                    .connections
                    .iter()
                    .map(|connection| connection.map(|net| net + offset))
                    .collect();
            }
            self.design.hierarchy.push(nested);
        }
        Ok(())
    }

    fn lower_port(&mut self, member: &serde_json::Value) -> Result<(), String> {
        let name = declaration_name(member, "Port")?;
        if memory_type(&string_field(member, "type").unwrap_or_default()).is_some() {
            return self.lower_memory_port(member, &name);
        }
        let direction = match string_field(member, "direction").as_deref() {
            Some("In") => eda_core::ir::Dir::In,
            Some("Out") => eda_core::ir::Dir::Out,
            other => return Err(format!("unsupported slang port direction {other:?}")),
        };
        let symbol = self.declare_net(member, &name, "Port")?;
        self.design.ports.push(eda_core::ir::PortDecl {
            name: name.clone(),
            dir: direction,
            base: symbol.base,
            width: symbol.width,
        });
        if member.get("initializer").is_some() {
            let rhs = self.lower_expression(
                member
                    .get("initializer")
                    .ok_or_else(|| format!("port '{name}' has invalid initializer"))?,
            )?;
            self.push_assignment(name, rhs);
        }
        Ok(())
    }

    fn lower_memory_port(&mut self, member: &serde_json::Value, name: &str) -> Result<(), String> {
        if string_field(member, "direction").as_deref() != Some("InOut") {
            return Err(format!(
                "direct slang-AST lowering supports only inout sparse-memory ports for '{name}'"
            ));
        }
        if !self.design.name.is_empty() {
            return Err(format!(
                "top-level inout sparse-memory port '{name}' is not supported"
            ));
        }
        self.lower_memory_declaration(member, name)?;
        let symbol = self.symbols[name];
        // Child PortDecl uses In only as internal connection metadata. The
        // actual storage contract is represented by MemoryConnection.
        self.design.ports.push(eda_core::ir::PortDecl {
            name: name.to_string(),
            dir: eda_core::ir::Dir::In,
            base: symbol.base,
            width: 1,
        });
        Ok(())
    }

    fn lower_declaration(&mut self, member: &serde_json::Value) -> Result<(), String> {
        let kind = string_field(member, "kind").unwrap_or_default();
        let name = declaration_name(member, &kind)?;
        if memory_type(&string_field(member, "type").unwrap_or_default()).is_some() {
            self.lower_memory_declaration(member, &name)?;
            return Ok(());
        }
        let symbol = self.declare_net(member, &name, &kind)?;
        if let Some(initializer) = member.get("initializer") {
            let rhs = self.lower_expression(initializer)?;
            let _ = symbol;
            self.push_assignment(name, rhs);
        }
        Ok(())
    }

    fn lower_memory_declaration(
        &mut self,
        member: &serde_json::Value,
        name: &str,
    ) -> Result<(), String> {
        let memory = memory_type(&string_field(member, "type").unwrap_or_default())
            .ok_or_else(|| format!("cannot parse sparse-memory type for '{name}'"))?;
        if let Some(existing) = self.symbols.get(name) {
            let Some(memory_index) = existing.memory else {
                return Err(format!("conflicting memory/net declaration '{name}'"));
            };
            let declared = &self.design.memories[memory_index];
            let matches = existing.width == memory.element_width
                && existing.signed == memory.signed
                && declared.element_width == memory.element_width
                && declared.element_msb == memory.element_msb
                && declared.element_lsb == memory.element_lsb
                && declared.dimensions == memory.dimensions
                && declared.flat_length == memory.flat_length;
            if !matches {
                return Err(format!(
                    "conflicting sparse-memory declarations for '{name}'"
                ));
            }
            return Ok(());
        }
        let initializer = member.get("initializer");
        let base = self.design.n_nets;
        self.design.n_nets += 1;
        self.design.net_decls.push(eda_core::ir::NetDecl {
            name: name.to_string(),
            width: 1,
            is_reg: true,
            is_signed: memory.signed,
            base,
        });
        let memory_index = self.design.memories.len();
        self.design.memories.push(eda_core::ir::MemoryDecl {
            name: name.to_string(),
            element_width: memory.element_width,
            element_msb: memory.element_msb,
            element_lsb: memory.element_lsb,
            dimensions: memory.dimensions,
            flat_length: memory.flat_length,
            kind: eda_core::ir::MemoryKind::Ram,
            template: eda_core::ir::MemoryTemplate::Unmapped,
            write_mask: None,
        });
        self.symbols.insert(
            name.to_string(),
            AstLoweredSymbol {
                base,
                width: memory.element_width,
                signed: memory.signed,
                memory: Some(memory_index),
            },
        );
        if let Some(initializer) = initializer {
            let dimensions = self.design.memories[memory_index].dimensions.clone();
            let statements = memory_initializer_statements(memory_index, &dimensions, initializer)?;
            self.design
                .initials
                .push(eda_core::ir::Stmt::Block(statements));
        }
        Ok(())
    }

    fn declare_net(
        &mut self,
        member: &serde_json::Value,
        name: &str,
        kind: &str,
    ) -> Result<AstLoweredSymbol, String> {
        if self.symbols.contains_key(name) {
            let existing = self.symbols[name];
            let type_name = string_field(member, "type").unwrap_or_default();
            let width = simple_type_width(&type_name)
                .ok_or_else(|| format!("direct lowering cannot resolve type '{type_name}'"))?;
            let signed = simple_type_signed(&type_name).ok_or_else(|| {
                format!("direct lowering cannot resolve signedness of '{type_name}'")
            })?;
            if existing.width != width || existing.signed != signed {
                return Err(format!("conflicting declarations for '{name}'"));
            }
            if kind == "Variable" {
                if let Some(declaration) = self
                    .design
                    .net_decls
                    .iter_mut()
                    .find(|declaration| declaration.name == name)
                {
                    declaration.is_reg = true;
                }
            }
            return Ok(existing);
        }
        let type_name = string_field(member, "type").unwrap_or_default();
        let width = simple_type_width(&type_name)
            .ok_or_else(|| format!("direct lowering cannot resolve type '{type_name}'"))?;
        let signed = simple_type_signed(&type_name)
            .ok_or_else(|| format!("direct lowering cannot resolve signedness of '{type_name}'"))?;
        let base = self.design.n_nets;
        let symbol = AstLoweredSymbol {
            base,
            width,
            signed,
            memory: None,
        };
        self.design.n_nets += width;
        self.design.net_decls.push(eda_core::ir::NetDecl {
            name: name.to_string(),
            width,
            is_reg: kind == "Port" || kind == "Variable",
            is_signed: signed,
            base,
        });
        self.symbols.insert(name.to_string(), symbol);
        Ok(symbol)
    }

    fn lower_continuous_assignment(&mut self, member: &serde_json::Value) -> Result<(), String> {
        let assignment = member
            .get("assignment")
            .ok_or_else(|| "ContinuousAssign has no assignment".to_string())?;
        let left = assignment
            .get("left")
            .ok_or_else(|| "continuous assignment has no left side".to_string())?;
        let right = assignment
            .get("right")
            .ok_or_else(|| "continuous assignment has no right side".to_string())?;
        if self.memory_element_select(left)?.is_some() || self.memory_range_select(left)?.is_some()
        {
            let body = self.lower_assignment(assignment)?;
            self.design.processes.push(eda_core::ir::Process {
                sens: eda_core::ir::Sens::Comb,
                body,
            });
            return Ok(());
        }
        let lv = self.lower_lvalue(left)?;
        let rhs = self.lower_expression(right)?;
        self.design.assigns.push(eda_core::ir::Assign {
            lv,
            e: rhs,
            delay: 0,
        });
        Ok(())
    }

    fn lower_procedural_block(&mut self, member: &serde_json::Value) -> Result<(), String> {
        let procedure_kind = string_field(member, "procedureKind").unwrap_or_default();
        let body = member
            .get("body")
            .ok_or_else(|| "slang ProceduralBlock has no body".to_string())?;
        match procedure_kind.as_str() {
            "Initial" => {
                let statement = self.lower_statement(body)?;
                self.design.initials.push(statement);
            }
            "AlwaysComb" => {
                let statement = self.lower_statement(body)?;
                self.design.processes.push(eda_core::ir::Process {
                    sens: eda_core::ir::Sens::Comb,
                    body: statement,
                });
            }
            "Always" | "AlwaysFF" | "AlwaysLatch" => {
                if string_field(body, "kind").as_deref() == Some("Timed") {
                    let timing = body
                        .get("timing")
                        .ok_or_else(|| "timed procedure has no timing control".to_string())?;
                    let statement = body
                        .get("stmt")
                        .ok_or_else(|| "timed procedure has no statement".to_string())?;
                    let sens = self.lower_sensitivity(timing)?;
                    let statement = self.lower_statement(statement)?;
                    self.design.processes.push(eda_core::ir::Process {
                        sens,
                        body: statement,
                    });
                } else {
                    let statement = self.lower_statement(body)?;
                    self.design.processes.push(eda_core::ir::Process {
                        sens: eda_core::ir::Sens::Comb,
                        body: statement,
                    });
                }
            }
            other => {
                return Err(format!(
                    "direct slang-AST lowering does not support procedure kind {other:?}"
                ))
            }
        }
        Ok(())
    }

    fn lower_sensitivity(&self, timing: &serde_json::Value) -> Result<eda_core::ir::Sens, String> {
        if string_field(timing, "kind").as_deref() == Some("ImplicitEvent") {
            return Ok(eda_core::ir::Sens::Comb);
        }
        if string_field(timing, "kind").as_deref() != Some("SignalEvent") {
            return Err(
                "direct slang-AST lowering supports only a single signal event sensitivity".into(),
            );
        }
        let expression = timing
            .get("expr")
            .ok_or_else(|| "signal event has no expression".to_string())?;
        let signal = self.lower_lvalue(expression)?;
        match string_field(timing, "edge").as_deref() {
            Some("PosEdge") => Ok(eda_core::ir::Sens::Posedge(signal.net)),
            Some("NegEdge") => Ok(eda_core::ir::Sens::Negedge(signal.net)),
            _ => Err("direct slang-AST lowering requires PosEdge or NegEdge sensitivity".into()),
        }
    }

    fn lower_statement(&self, statement: &serde_json::Value) -> Result<eda_core::ir::Stmt, String> {
        use eda_core::ir::Stmt;
        let kind = string_field(statement, "kind").unwrap_or_default();
        match kind.as_str() {
            "Block" => {
                if let Some(nested) = statement.get("body") {
                    if string_field(nested, "kind").as_deref() == Some("List") {
                        return self.lower_statement(nested);
                    }
                    let nested = self.lower_statement(nested)?;
                    return Ok(Stmt::Block(vec![nested]));
                }
                Ok(Stmt::Block(Vec::new()))
            }
            "List" => {
                let mut statements = Vec::new();
                if let Some(items) = statement.get("list").and_then(Value::as_array) {
                    for item in items {
                        statements.push(self.lower_statement(item)?);
                    }
                }
                Ok(Stmt::Block(statements))
            }
            "ExpressionStatement" => {
                let expression = statement
                    .get("expr")
                    .ok_or_else(|| "expression statement has no expression".to_string())?;
                if string_field(expression, "kind").as_deref() == Some("Call") {
                    return self.lower_memory_file_call(expression, statement);
                }
                if string_field(expression, "kind").as_deref() != Some("Assignment") {
                    return Err(
                        "direct slang-AST lowering supports only assignment expression statements"
                            .into(),
                    );
                }
                self.lower_assignment(expression)
            }
            "Assignment" => self.lower_assignment(statement),
            "Conditional" => {
                let conditions = statement
                    .get("conditions")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "conditional statement has no conditions".to_string())?;
                if conditions.len() != 1 {
                    return Err(
                        "direct slang-AST lowering supports one unqualified if condition".into(),
                    );
                }
                let condition = conditions
                    .first()
                    .and_then(|condition| condition.get("expr"))
                    .ok_or_else(|| {
                        "conditional statement has no condition expression".to_string()
                    })?;
                let true_statement = statement
                    .get("ifTrue")
                    .ok_or_else(|| "conditional statement has no true branch".to_string())?;
                let false_statement = match statement.get("ifFalse") {
                    Some(statement) => Some(Box::new(self.lower_statement(statement)?)),
                    None => None,
                };
                Ok(Stmt::If {
                    c: self.lower_expression(condition)?,
                    t: Box::new(self.lower_statement(true_statement)?),
                    f: false_statement,
                })
            }
            "Case" => {
                let expression = statement
                    .get("expr")
                    .ok_or_else(|| "case statement has no expression".to_string())?;
                let items = statement
                    .get("items")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "case statement has no items".to_string())?;
                let mut arms = Vec::new();
                for item in items {
                    let body = item
                        .get("stmt")
                        .ok_or_else(|| "case item has no statement".to_string())?;
                    let body = self.lower_statement(body)?;
                    let expressions = item
                        .get("expressions")
                        .and_then(Value::as_array)
                        .ok_or_else(|| "case item has no expressions".to_string())?;
                    for expression in expressions {
                        arms.push((self.lower_expression(expression)?, body.clone()));
                    }
                }
                let default = match statement.get("defaultCase") {
                    Some(statement) => Some(Box::new(self.lower_statement(statement)?)),
                    None => None,
                };
                Ok(Stmt::Case {
                    e: self.lower_expression(expression)?,
                    arms,
                    dflt: default,
                })
            }
            other => Err(format!(
                "direct slang-AST lowering does not support statement kind '{other}'"
            )),
        }
    }

    fn lower_memory_file_call(
        &self,
        call: &serde_json::Value,
        statement: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        let subroutine = string_field(call, "subroutine").unwrap_or_default();
        let is_write = matches!(subroutine.as_str(), "$writememh" | "$writememb");
        let radix = match subroutine.as_str() {
            "$readmemh" | "$writememh" => 16,
            "$readmemb" | "$writememb" => 2,
            other => {
                return Err(format!(
                    "direct slang-AST lowering does not support system call {other:?}"
                ))
            }
        };
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| "memory-file call has no arguments".to_string())?;
        if arguments.len() < 2 || arguments.len() > 4 {
            return Err(format!(
                "direct slang-AST lowering supports two to four arguments for memory-file tasks, got {}",
                arguments.len()
            ));
        }
        let raw_path = string_field(&arguments[0], "literal")
            .ok_or_else(|| "memory-file call requires a string literal path".to_string())?;
        let target = if is_write {
            &arguments[1]
        } else {
            arguments[1]
                .get("left")
                .ok_or_else(|| "memory-file call has no target".to_string())?
        };
        let name = expression_symbol(target)
            .ok_or_else(|| "memory-file call target has no symbol".to_string())?;
        let symbol = self
            .symbols
            .get(&name)
            .ok_or_else(|| format!("unknown memory-file target '{name}'"))?;
        let memory = symbol
            .memory
            .ok_or_else(|| format!("memory-file target '{name}' is not a sparse memory"))?;
        let start = arguments.get(2).map(slang_constant_u64).transpose()?;
        let finish = arguments.get(3).map(slang_constant_u64).transpose()?;
        let declaration = &self.design.memories[memory];
        let path = if is_write {
            resolve_memory_output_path(&raw_path, statement)
        } else {
            resolve_memory_file_path(&raw_path, statement)
        };
        if is_write {
            return Ok(eda_core::ir::Stmt::WriteMem {
                radix,
                path: path.to_string_lossy().into_owned(),
                memory,
                start: start.map(|start| {
                    eda_core::ir::Expr::Const(eda_core::logic::LogicWord::from_u64(32, start))
                }),
                finish: finish.map(|finish| {
                    eda_core::ir::Expr::Const(eda_core::logic::LogicWord::from_u64(32, finish))
                }),
            });
        }
        let values = eda_core::memory_file::read_memory_file(
            &path,
            radix,
            declaration.element_width,
            start,
        )?;
        let values = eda_core::memory_file::filter_memory_file_finish(values, start, finish)?;
        let mut statements = Vec::with_capacity(values.len());
        for physical_index in values.keys().copied().collect::<Vec<_>>() {
            let logical_indexes =
                eda_core::memory_file::physical_to_logical_indexes(declaration, physical_index)?;
            statements.push(eda_core::ir::Stmt::MemoryBl {
                memory,
                indexes: logical_indexes
                    .into_iter()
                    .map(|index| {
                        eda_core::ir::Expr::Const(eda_core::logic::LogicWord::from_u64(
                            32,
                            index as u64,
                        ))
                    })
                    .collect(),
                e: eda_core::ir::Expr::Const(values[&physical_index].clone()),
            });
        }
        Ok(eda_core::ir::Stmt::Block(statements))
    }

    fn lower_assignment(
        &self,
        assignment: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        use eda_core::ir::Stmt;
        let left = assignment
            .get("left")
            .ok_or_else(|| "assignment has no left side".to_string())?;
        let right = assignment
            .get("right")
            .ok_or_else(|| "assignment has no right side".to_string())?;
        if let Some((memory, indexes)) = self.memory_element_select(left)? {
            let expression = self.lower_expression(right)?;
            return Ok(if bool_field(assignment, "isNonBlocking") {
                Stmt::MemoryNBl {
                    memory,
                    indexes,
                    e: expression,
                }
            } else {
                Stmt::MemoryBl {
                    memory,
                    indexes,
                    e: expression,
                }
            });
        }
        if let Some((memory, indexes, start, width, ascending)) = self.memory_range_select(left)? {
            let expression = self.lower_expression(right)?;
            return Ok(if bool_field(assignment, "isNonBlocking") {
                Stmt::MemoryNBlPart {
                    memory,
                    indexes,
                    start: Box::new(start),
                    width,
                    ascending,
                    e: expression,
                }
            } else {
                Stmt::MemoryBlPart {
                    memory,
                    indexes,
                    start: Box::new(start),
                    width,
                    ascending,
                    e: expression,
                }
            });
        }
        let lv = self.lower_lvalue(left)?;
        let expression = self.lower_expression(right)?;
        if bool_field(assignment, "isNonBlocking") {
            Ok(Stmt::NBl { lv, e: expression })
        } else {
            Ok(Stmt::Bl { lv, e: expression })
        }
    }

    fn push_assignment(&mut self, name: String, rhs: eda_core::ir::Expr) {
        let symbol = self.symbols[&name];
        self.design.assigns.push(eda_core::ir::Assign {
            lv: eda_core::ir::LValue {
                net: symbol.base,
                decl_width: symbol.width,
                range: full_range(symbol.width),
            },
            e: rhs,
            delay: 0,
        });
    }

    fn lower_lvalue(&self, value: &serde_json::Value) -> Result<eda_core::ir::LValue, String> {
        match string_field(value, "kind").as_deref() {
            Some("NamedValue") => {
                let name = expression_symbol(value)
                    .ok_or_else(|| "assignment target has no symbol".to_string())?;
                let symbol = self
                    .symbols
                    .get(&name)
                    .ok_or_else(|| format!("unknown direct-lowering symbol '{name}'"))?;
                Ok(eda_core::ir::LValue {
                    net: symbol.base,
                    decl_width: symbol.width,
                    range: full_range(symbol.width),
                })
            }
            Some("Conversion") => {
                let operand = value
                    .get("operand")
                    .ok_or_else(|| "conversion lvalue has no operand".to_string())?;
                self.lower_lvalue(operand)
            }
            Some("ElementSelect") => {
                if self.memory_element_select(value)?.is_some() {
                    return Err(
                        "sparse-memory selects require the memory assignment lowering path".into(),
                    );
                }
                let (lv, index) = self.lower_constant_select_lvalue(value)?;
                Ok(eda_core::ir::LValue {
                    net: lv.net,
                    decl_width: lv.decl_width,
                    range: eda_core::ir::Range::Bit(index),
                })
            }
            Some("RangeSelect") => {
                if self.memory_range_select(value)?.is_some() {
                    return Err(
                        "sparse-memory part selects require the memory assignment lowering path"
                            .into(),
                    );
                }
                let (lv, msb, lsb) = self.lower_constant_range_lvalue(value)?;
                Ok(eda_core::ir::LValue {
                    net: lv.net,
                    decl_width: lv.decl_width,
                    range: eda_core::ir::Range::Span { msb, lsb },
                })
            }
            Some(kind) => Err(format!(
                "direct slang-AST lowering does not support lvalue kind '{kind}'"
            )),
            None => Err("slang lvalue has no kind".into()),
        }
    }

    fn lower_expression(&self, value: &serde_json::Value) -> Result<eda_core::ir::Expr, String> {
        use eda_core::ir::{Expr, UnOp};
        let kind = string_field(value, "kind").unwrap_or_default();
        match kind.as_str() {
            "NamedValue" => {
                let name = expression_symbol(value)
                    .ok_or_else(|| "expression has no symbol".to_string())?;
                if let Some(symbol) = self.symbols.get(&name) {
                    Ok(Expr::Ref {
                        net: symbol.base,
                        decl_width: symbol.width,
                        range: full_range(symbol.width),
                        signed: symbol.signed,
                    })
                } else {
                    named_constant_word(value)
                        .map(Expr::Const)
                        .ok_or_else(|| format!("unknown direct-lowering symbol '{name}'"))
                }
            }
            "IntegerLiteral" => {
                let word = integer_literal_word(value)
                    .ok_or_else(|| "invalid slang integer literal".to_string())?;
                Ok(Expr::Const(word))
            }
            "Conversion" => {
                let operand = value
                    .get("operand")
                    .ok_or_else(|| "conversion has no operand".to_string())?;
                let target_type = string_field(value, "type").unwrap_or_default();
                let operand_type = string_field(operand, "type").unwrap_or_default();
                let target = (
                    simple_type_width(&target_type),
                    simple_type_signed(&target_type),
                );
                let source = (
                    simple_type_width(&operand_type),
                    simple_type_signed(&operand_type),
                );
                if target != source {
                    return Err(format!(
                        "direct slang-AST lowering does not support conversion '{target_type}' from '{operand_type}'"
                    ));
                }
                self.lower_expression(operand)
            }
            "UnaryOp" => {
                let op = match string_field(value, "op").as_deref() {
                    Some("BitwiseNot") => UnOp::Not,
                    Some("LogicalNot") => UnOp::Not,
                    Some("BitwiseAnd") => UnOp::AndR,
                    Some("BitwiseOr") => UnOp::OrR,
                    Some("BitwiseXor") => UnOp::XorR,
                    Some(op) => {
                        return Err(format!(
                            "direct slang-AST lowering does not support unary operator '{op}'"
                        ))
                    }
                    None => return Err("unary expression has no operator".into()),
                };
                let operand = value
                    .get("operand")
                    .ok_or_else(|| "unary expression has no operand".to_string())?;
                Ok(Expr::Un {
                    op,
                    a: Box::new(self.lower_expression(operand)?),
                })
            }
            "BinaryOp" => {
                let op = binary_operator(string_field(value, "op").as_deref())?;
                let left = value
                    .get("left")
                    .ok_or_else(|| "binary expression has no left operand".to_string())?;
                let right = value
                    .get("right")
                    .ok_or_else(|| "binary expression has no right operand".to_string())?;
                Ok(Expr::Bin {
                    op,
                    a: Box::new(self.lower_expression(left)?),
                    b: Box::new(self.lower_expression(right)?),
                })
            }
            "Concatenation" => {
                let operands = value
                    .get("operands")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| "concatenation has no operands".to_string())?;
                let mut parts = Vec::new();
                for operand in operands {
                    parts.push(self.lower_expression(operand)?);
                }
                Ok(Expr::Concat(parts))
            }
            "ConditionalOp" => {
                let conditions = value
                    .get("conditions")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| "conditional expression has no conditions".to_string())?;
                if conditions.len() != 1 {
                    return Err(
                        "direct slang-AST lowering supports only one conditional condition".into(),
                    );
                }
                let condition = conditions
                    .first()
                    .and_then(|condition| condition.get("expr"))
                    .ok_or_else(|| "conditional condition has no expression".to_string())?;
                let left = value
                    .get("left")
                    .ok_or_else(|| "conditional expression has no true value".to_string())?;
                let right = value
                    .get("right")
                    .ok_or_else(|| "conditional expression has no false value".to_string())?;
                Ok(Expr::Cond {
                    c: Box::new(self.lower_expression(condition)?),
                    t: Box::new(self.lower_expression(left)?),
                    f: Box::new(self.lower_expression(right)?),
                })
            }
            "ElementSelect" => {
                if let Some((memory, indexes)) = self.memory_element_select(value)? {
                    let symbol = self.design.memories[memory].clone();
                    let signed = self
                        .design
                        .net_decls
                        .iter()
                        .find(|declaration| declaration.name == symbol.name)
                        .map(|declaration| declaration.is_signed)
                        .unwrap_or(false);
                    return Ok(Expr::MemoryRead {
                        memory,
                        indexes,
                        width: symbol.element_width,
                        signed,
                    });
                }
                let (base, index) = self.lower_constant_select(value)?;
                Ok(Expr::Index {
                    e: Box::new(base),
                    msb: index,
                    lsb: index,
                })
            }
            "RangeSelect" => {
                if let Some((memory, indexes, start, width, ascending)) =
                    self.memory_range_select(value)?
                {
                    return Ok(Expr::MemoryReadPart {
                        memory,
                        indexes,
                        start: Box::new(start),
                        width,
                        ascending,
                    });
                }
                let (base, msb, lsb) = self.lower_constant_range(value)?;
                Ok(Expr::Index {
                    e: Box::new(base),
                    msb,
                    lsb,
                })
            }
            _ => Err(format!(
                "direct slang-AST lowering does not support expression kind '{kind}'"
            )),
        }
    }

    fn lower_constant_select(
        &self,
        value: &serde_json::Value,
    ) -> Result<(eda_core::ir::Expr, usize), String> {
        let selected = value
            .get("value")
            .ok_or_else(|| "element select has no value".to_string())?;
        let selector = value
            .get("selector")
            .ok_or_else(|| "element select has no selector".to_string())?;
        let index = constant_index(selector)?;
        let base = self.lower_expression(selected)?;
        Ok((base, index))
    }

    fn memory_element_select(
        &self,
        value: &serde_json::Value,
    ) -> Result<Option<(usize, Vec<eda_core::ir::Expr>)>, String> {
        if string_field(value, "kind").as_deref() != Some("ElementSelect") {
            return Ok(None);
        }
        let selected = value
            .get("value")
            .ok_or_else(|| "element select has no value".to_string())?;
        let selector = value
            .get("selector")
            .ok_or_else(|| "memory element select has no selector".to_string())?;
        let selector = self.lower_expression(selector)?;
        match string_field(selected, "kind").as_deref() {
            Some("NamedValue") => {
                let name = expression_symbol(selected)
                    .ok_or_else(|| "memory element select has no symbol".to_string())?;
                let Some(symbol) = self.symbols.get(&name) else {
                    return Ok(None);
                };
                let Some(memory) = symbol.memory else {
                    return Ok(None);
                };
                Ok(Some((memory, vec![selector])))
            }
            Some("ElementSelect") => {
                let Some((memory, mut indexes)) = self.memory_element_select(selected)? else {
                    return Ok(None);
                };
                indexes.push(selector);
                Ok(Some((memory, indexes)))
            }
            _ => Ok(None),
        }
    }

    #[allow(clippy::type_complexity)]
    fn memory_range_select(
        &self,
        value: &serde_json::Value,
    ) -> Result<
        Option<(
            usize,
            Vec<eda_core::ir::Expr>,
            eda_core::ir::Expr,
            usize,
            bool,
        )>,
        String,
    > {
        if string_field(value, "kind").as_deref() != Some("RangeSelect") {
            return Ok(None);
        }
        let Some(selected) = value.get("value") else {
            return Ok(None);
        };
        let Some((memory, indexes)) = self.memory_element_select(selected)? else {
            return Ok(None);
        };
        let element_width = self.design.memories[memory].element_width;
        let selection_kind = string_field(value, "selectionKind").unwrap_or_default();
        let (start, width, ascending) = match selection_kind.as_str() {
            "Simple" => {
                let left = value
                    .get("left")
                    .ok_or_else(|| "memory range select has no left bound".to_string())?;
                let right = value
                    .get("right")
                    .ok_or_else(|| "memory range select has no right bound".to_string())?;
                let msb = constant_index(left)?;
                let lsb = constant_index(right)?;
                if msb < lsb {
                    return Err(
                        "direct slang-AST lowering does not yet support reversed simple ranges"
                            .into(),
                    );
                }
                (
                    eda_core::ir::Expr::Const(eda_core::logic::LogicWord::from_u64(32, lsb as u64)),
                    msb - lsb + 1,
                    true,
                )
            }
            "IndexedUp" | "IndexedDown" => {
                let left = value
                    .get("left")
                    .ok_or_else(|| "indexed memory select has no start".to_string())?;
                let right = value
                    .get("right")
                    .ok_or_else(|| "indexed memory select has no width".to_string())?;
                let width = constant_index(right)?;
                (
                    self.lower_expression(left)?,
                    width,
                    selection_kind == "IndexedUp",
                )
            }
            other => {
                return Err(format!(
                    "direct slang-AST lowering does not support memory selection kind '{other}'"
                ))
            }
        };
        if width == 0 || width > element_width {
            return Err(format!(
                "memory part-select width {width} exceeds element width {element_width}"
            ));
        }
        Ok(Some((memory, indexes, start, width, ascending)))
    }

    fn lower_constant_select_lvalue(
        &self,
        value: &serde_json::Value,
    ) -> Result<(eda_core::ir::LValue, usize), String> {
        let selected = value
            .get("value")
            .ok_or_else(|| "element select has no value".to_string())?;
        let selector = value
            .get("selector")
            .ok_or_else(|| "element select has no selector".to_string())?;
        Ok((self.lower_lvalue(selected)?, constant_index(selector)?))
    }

    fn lower_constant_range(
        &self,
        value: &serde_json::Value,
    ) -> Result<(eda_core::ir::Expr, usize, usize), String> {
        let selected = value
            .get("value")
            .ok_or_else(|| "range select has no value".to_string())?;
        let left = value
            .get("left")
            .ok_or_else(|| "range select has no left bound".to_string())?;
        let right = value
            .get("right")
            .ok_or_else(|| "range select has no right bound".to_string())?;
        let msb = constant_index(left)?;
        let lsb = constant_index(right)?;
        if msb < lsb {
            return Err("descending direct range selects are not yet lowered".into());
        }
        let base = self.lower_expression(selected)?;
        Ok((base, msb, lsb))
    }

    fn lower_constant_range_lvalue(
        &self,
        value: &serde_json::Value,
    ) -> Result<(eda_core::ir::LValue, usize, usize), String> {
        let selected = value
            .get("value")
            .ok_or_else(|| "range select has no value".to_string())?;
        let left = value
            .get("left")
            .ok_or_else(|| "range select has no left bound".to_string())?;
        let right = value
            .get("right")
            .ok_or_else(|| "range select has no right bound".to_string())?;
        let msb = constant_index(left)?;
        let lsb = constant_index(right)?;
        if msb < lsb {
            return Err("descending direct range-select lvalues are not yet lowered".into());
        }
        Ok((self.lower_lvalue(selected)?, msb, lsb))
    }
}

fn declaration_name(member: &serde_json::Value, kind: &str) -> Result<String, String> {
    string_field(member, "name")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("slang AST {kind} has no name"))
}

fn simple_type_signed(type_name: &str) -> Option<bool> {
    simple_type_width(type_name)
        .map(|_| type_name.split_whitespace().any(|token| token == "signed"))
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

fn memory_type(type_name: &str) -> Option<AstMemoryType> {
    let separator = type_name.find('$')?;
    let element_type = type_name[..separator].trim();
    let dimension_type = type_name[separator + 1..].trim();
    let element_width = simple_type_width(element_type)?;
    let (element_msb, element_lsb) =
        parse_type_range(element_type).unwrap_or((element_width as i64 - 1, 0));
    if (element_msb - element_lsb).unsigned_abs() as usize + 1 != element_width {
        return None;
    }
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
        dimensions.push(eda_core::ir::MemoryDimension { base, step, length });
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
        signed: simple_type_signed(element_type)?,
    })
}

fn full_range(width: usize) -> eda_core::ir::Range {
    if width == 1 {
        eda_core::ir::Range::Bit(0)
    } else {
        eda_core::ir::Range::Span {
            msb: width.saturating_sub(1),
            lsb: 0,
        }
    }
}

fn range_parent_bits(net: usize, range: &eda_core::ir::Range) -> Option<Vec<Option<usize>>> {
    match range {
        eda_core::ir::Range::Bit(index) => Some(vec![Some(net + index)]),
        eda_core::ir::Range::Span { msb, lsb } => {
            if msb < lsb {
                return None;
            }
            Some((*lsb..=*msb).map(|index| Some(net + index)).collect())
        }
    }
}

fn rewrite_hierarchy_path(path: &str, prefix: &str) -> String {
    if path.is_empty() {
        return prefix.to_string();
    }
    if path.starts_with('.') {
        format!("{prefix}{path}")
    } else {
        format!("{prefix}.{path}")
    }
}

fn expression_parent_bits(expression: &eda_core::ir::Expr) -> Option<Vec<Option<usize>>> {
    match expression {
        eda_core::ir::Expr::Ref { net, range, .. } => range_parent_bits(*net, range),
        _ => None,
    }
}

fn lvalue_parent_bits(lvalue: &eda_core::ir::LValue) -> Option<Vec<Option<usize>>> {
    range_parent_bits(lvalue.net, &lvalue.range)
}

fn remap_assignment(assignment: &mut eda_core::ir::Assign, offset: usize) {
    assignment.lv.net += offset;
    remap_expression(&mut assignment.e, offset);
}

fn remap_expression(expression: &mut eda_core::ir::Expr, offset: usize) {
    use eda_core::ir::Expr;
    match expression {
        Expr::Const(_) => {}
        Expr::Ref { net, .. } => *net += offset,
        Expr::Un { a, .. } => remap_expression(a, offset),
        Expr::Bin { a, b, .. } => {
            remap_expression(a, offset);
            remap_expression(b, offset);
        }
        Expr::Cond { c, t, f } => {
            remap_expression(c, offset);
            remap_expression(t, offset);
            remap_expression(f, offset);
        }
        Expr::Concat(parts) => parts
            .iter_mut()
            .for_each(|part| remap_expression(part, offset)),
        Expr::Repeat { e, .. } | Expr::Signed { e, .. } => remap_expression(e, offset),
        Expr::MemoryRead { indexes, .. } => {
            indexes
                .iter_mut()
                .for_each(|index| remap_expression(index, offset));
        }
        Expr::MemoryReadPart { indexes, start, .. } => {
            indexes
                .iter_mut()
                .for_each(|index| remap_expression(index, offset));
            remap_expression(start, offset);
        }
        Expr::Index { e, .. } => remap_expression(e, offset),
    }
}

fn remap_statement(statement: &mut eda_core::ir::Stmt, offset: usize) {
    use eda_core::ir::Stmt;
    match statement {
        Stmt::Block(statements) => statements
            .iter_mut()
            .for_each(|statement| remap_statement(statement, offset)),
        Stmt::Forever(body) => remap_statement(body, offset),
        Stmt::NBl { lv, e } | Stmt::Bl { lv, e } => {
            lv.net += offset;
            remap_expression(e, offset);
        }
        Stmt::MemoryBl { indexes, e, .. } | Stmt::MemoryNBl { indexes, e, .. } => {
            indexes
                .iter_mut()
                .for_each(|index| remap_expression(index, offset));
            remap_expression(e, offset);
        }
        Stmt::MemoryBlPart {
            indexes, start, e, ..
        }
        | Stmt::MemoryNBlPart {
            indexes, start, e, ..
        } => {
            indexes
                .iter_mut()
                .for_each(|index| remap_expression(index, offset));
            remap_expression(start, offset);
            remap_expression(e, offset);
        }
        Stmt::BlIndexed { net, start, e, .. } | Stmt::NBlIndexed { net, start, e, .. } => {
            *net += offset;
            remap_expression(start, offset);
            remap_expression(e, offset);
        }
        Stmt::If { c, t, f } => {
            remap_expression(c, offset);
            remap_statement(t, offset);
            if let Some(f) = f {
                remap_statement(f, offset);
            }
        }
        Stmt::Case { e, arms, dflt } => {
            remap_expression(e, offset);
            for (expression, _) in arms {
                remap_expression(expression, offset);
            }
            if let Some(statement) = dflt {
                remap_statement(statement, offset);
            }
        }
        Stmt::Delay { body, .. } => remap_statement(body, offset),
        Stmt::Display { args, .. } => {
            args.iter_mut()
                .for_each(|argument| remap_expression(argument, offset));
        }
        Stmt::Assert { cond, .. } | Stmt::Assume { cond, .. } | Stmt::Cover { cond, .. } => {
            remap_expression(cond, offset)
        }
        Stmt::WriteMem { start, finish, .. } => {
            if let Some(start) = start {
                remap_expression(start, offset);
            }
            if let Some(finish) = finish {
                remap_expression(finish, offset);
            }
        }
        Stmt::Finish => {}
    }
}

fn remap_process(process: &mut eda_core::ir::Process, offset: usize) {
    use eda_core::ir::Sens;
    fn remap_sensitivity(sensitivity: &mut Sens, offset: usize) {
        match sensitivity {
            Sens::Comb => {}
            Sens::Posedge(net) | Sens::Negedge(net) => *net += offset,
            Sens::List(list) => list
                .iter_mut()
                .for_each(|item| remap_sensitivity(item, offset)),
        }
    }
    remap_sensitivity(&mut process.sens, offset);
    remap_statement(&mut process.body, offset);
}

fn remap_expression_memory_ids(expression: &mut eda_core::ir::Expr, offset: usize) {
    use eda_core::ir::Expr;
    match expression {
        Expr::Const(_) => {}
        Expr::Ref { .. } => {}
        Expr::Un { a, .. } => remap_expression_memory_ids(a, offset),
        Expr::Bin { a, b, .. } => {
            remap_expression_memory_ids(a, offset);
            remap_expression_memory_ids(b, offset);
        }
        Expr::Cond { c, t, f } => {
            remap_expression_memory_ids(c, offset);
            remap_expression_memory_ids(t, offset);
            remap_expression_memory_ids(f, offset);
        }
        Expr::Concat(parts) => parts
            .iter_mut()
            .for_each(|part| remap_expression_memory_ids(part, offset)),
        Expr::Repeat { e, .. } | Expr::Signed { e, .. } => remap_expression_memory_ids(e, offset),
        Expr::MemoryRead {
            memory, indexes, ..
        } => {
            *memory += offset;
            indexes
                .iter_mut()
                .for_each(|index| remap_expression_memory_ids(index, offset));
        }
        Expr::MemoryReadPart {
            memory,
            indexes,
            start,
            ..
        } => {
            *memory += offset;
            indexes
                .iter_mut()
                .for_each(|index| remap_expression_memory_ids(index, offset));
            remap_expression_memory_ids(start, offset);
        }
        Expr::Index { e, .. } => remap_expression_memory_ids(e, offset),
    }
}

fn remap_statement_memory_ids(statement: &mut eda_core::ir::Stmt, offset: usize) {
    use eda_core::ir::Stmt;
    match statement {
        Stmt::Block(statements) => statements
            .iter_mut()
            .for_each(|statement| remap_statement_memory_ids(statement, offset)),
        Stmt::Forever(body) => remap_statement_memory_ids(body, offset),
        Stmt::NBl { e, .. } | Stmt::Bl { e, .. } => remap_expression_memory_ids(e, offset),
        Stmt::MemoryBl {
            memory, indexes, e, ..
        }
        | Stmt::MemoryNBl {
            memory, indexes, e, ..
        } => {
            *memory += offset;
            indexes
                .iter_mut()
                .for_each(|index| remap_expression_memory_ids(index, offset));
            remap_expression_memory_ids(e, offset);
        }
        Stmt::MemoryBlPart {
            memory,
            indexes,
            start,
            e,
            ..
        }
        | Stmt::MemoryNBlPart {
            memory,
            indexes,
            start,
            e,
            ..
        } => {
            *memory += offset;
            indexes
                .iter_mut()
                .for_each(|index| remap_expression_memory_ids(index, offset));
            remap_expression_memory_ids(start, offset);
            remap_expression_memory_ids(e, offset);
        }
        Stmt::BlIndexed { start, e, .. } | Stmt::NBlIndexed { start, e, .. } => {
            remap_expression_memory_ids(start, offset);
            remap_expression_memory_ids(e, offset);
        }
        Stmt::If { c, t, f } => {
            remap_expression_memory_ids(c, offset);
            remap_statement_memory_ids(t, offset);
            if let Some(f) = f {
                remap_statement_memory_ids(f, offset);
            }
        }
        Stmt::Case { e, arms, dflt } => {
            remap_expression_memory_ids(e, offset);
            for (expression, _) in arms {
                remap_expression_memory_ids(expression, offset);
            }
            if let Some(statement) = dflt {
                remap_statement_memory_ids(statement, offset);
            }
        }
        Stmt::Delay { body, .. } => remap_statement_memory_ids(body, offset),
        Stmt::Display { args, .. } => {
            args.iter_mut()
                .for_each(|argument| remap_expression_memory_ids(argument, offset));
        }
        Stmt::Assert { cond, .. } | Stmt::Assume { cond, .. } | Stmt::Cover { cond, .. } => {
            remap_expression_memory_ids(cond, offset)
        }
        Stmt::WriteMem {
            memory,
            start,
            finish,
            ..
        } => {
            *memory += offset;
            if let Some(start) = start {
                remap_expression_memory_ids(start, offset);
            }
            if let Some(finish) = finish {
                remap_expression_memory_ids(finish, offset);
            }
        }
        Stmt::Finish => {}
    }
}

fn binary_operator(operator: Option<&str>) -> Result<eda_core::ir::BinOp, String> {
    match operator {
        Some("Add") => Ok(eda_core::ir::BinOp::Add),
        Some("Subtract") => Ok(eda_core::ir::BinOp::Sub),
        Some("Multiply") => Ok(eda_core::ir::BinOp::Mul),
        Some("Divide") => Ok(eda_core::ir::BinOp::Div),
        Some("Modulus") => Ok(eda_core::ir::BinOp::Mod),
        Some("BinaryAnd") => Ok(eda_core::ir::BinOp::And),
        Some("BinaryOr") => Ok(eda_core::ir::BinOp::Or),
        Some("BitwiseXor") => Ok(eda_core::ir::BinOp::Xor),
        Some("LogicalShiftLeft") => Ok(eda_core::ir::BinOp::Shl),
        Some("LogicalShiftRight") => Ok(eda_core::ir::BinOp::Shr),
        Some("LogicalAnd") => Ok(eda_core::ir::BinOp::LogAnd),
        Some("LogicalOr") => Ok(eda_core::ir::BinOp::LogOr),
        Some("Equality") => Ok(eda_core::ir::BinOp::Eq),
        Some("Inequality") => Ok(eda_core::ir::BinOp::Ne),
        Some("LessThan") => Ok(eda_core::ir::BinOp::Lt),
        Some("LessThanEqual") => Ok(eda_core::ir::BinOp::Le),
        Some("GreaterThan") => Ok(eda_core::ir::BinOp::Gt),
        Some("GreaterThanEqual") => Ok(eda_core::ir::BinOp::Ge),
        Some(other) => Err(format!(
            "direct slang-AST lowering does not support binary operator {other:?}"
        )),
        None => Err("binary expression has no operator".into()),
    }
}

fn constant_index(value: &serde_json::Value) -> Result<usize, String> {
    integer_literal_word(value)
        .and_then(|word| word.to_u64())
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| "direct lowering requires an integer constant index".to_string())
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
        let mut source_spans = std::collections::BTreeMap::new();
        if let Some(span) = source_span(root) {
            source_spans.insert("top".to_string(), span);
        }
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
                    let assignments = procedural_assignments(member);
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

    pub fn validate_against_design(&self, design: &Design) -> Result<(), String> {
        if design.name != self.top_name {
            return Err(format!(
                "bounded elaborator top '{}' does not match slang top '{}'",
                design.name, self.top_name
            ) + &self.span_note("top"));
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
            ) + &self.span_note("top"));
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
                    return Err(
                        format!("bounded elaborator dropped slang declaration '{name}'")
                            + &self.span_note(&format!("declaration:{name}")),
                    );
                }
                Some(actual_width) => {
                    if let Some(slang_width) = slang_width {
                        if *actual_width != slang_width {
                            return Err(format!(
                                "bounded elaborator declaration '{name}' width {actual_width} does not match slang width {slang_width}"
                            ) + &self.span_note(&format!("declaration:{name}")));
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

        let mut expected_continuous_targets: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for target in &self.continuous_targets {
            expected_continuous_targets
                .entry(target.name.clone())
                .or_default()
                .push(slang_expression_fingerprint(&target.rhs).unwrap_or_else(|| "?".to_string()));
        }
        let memory_names: std::collections::BTreeSet<String> = design
            .memories
            .iter()
            .map(|memory| memory.name.clone())
            .collect();
        let mut actual_continuous_targets: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for assignment in &design.assigns {
            let Some(name) = design_lvalue_name(design, &assignment.lv) else {
                continue;
            };
            if name.starts_with("__") || name.contains('.') {
                continue;
            }
            actual_continuous_targets
                .entry(name)
                .or_default()
                .push(design_expr_fingerprint(design, &assignment.e));
        }
        for (name, expected_shapes) in expected_continuous_targets {
            let represented_as_sparse_memory = memory_names.contains(&name);
            if represented_as_sparse_memory {
                continue;
            }
            let actual_shapes = actual_continuous_targets
                .get(&name)
                .cloned()
                .unwrap_or_default();
            if !contains_expected_shapes(&actual_shapes, &expected_shapes) {
                return Err(format!(
                    "bounded elaborator continuous assignment shape for '{name}' does not match slang: internal {actual_shapes:?}, slang {expected_shapes:?}"
                ) + &self.span_note(&format!("continuous:{name}:")));
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
            ) + &self.span_note("procedural:"));
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
            ) + &self.span_note("procedural:"));
        }

        let declaration_names: std::collections::BTreeSet<String> = self
            .declarations
            .iter()
            .map(|declaration| declaration.name.clone())
            .collect();
        let mut expected_procedural_targets: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        let legacy_array_targets: std::collections::BTreeSet<String> = self
            .declarations
            .iter()
            .filter(|declaration| {
                declaration.type_name.contains('$')
                    && !design
                        .memories
                        .iter()
                        .any(|memory| memory.name == declaration.name)
            })
            .map(|declaration| declaration.name.clone())
            .collect();
        for block in &self.procedural_blocks {
            for assignment in &block.assignments {
                for target in &assignment.target_names {
                    if declaration_names.contains(target) {
                        let shape = if legacy_array_targets.contains(target) {
                            // Small arrays still use the legacy bounded
                            // per-element expansion, whose expression tree is
                            // intentionally not structurally equivalent to the
                            // slang source. Preserve assignment-count checking
                            // without guessing an equivalence.
                            "?".to_string()
                        } else {
                            slang_expression_fingerprint(&assignment.rhs)
                                .unwrap_or_else(|| "?".to_string())
                        };
                        expected_procedural_targets
                            .entry(target.clone())
                            .or_default()
                            .push(shape);
                    }
                }
            }
        }
        let mut actual_procedural_targets: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for initial in &design.initials {
            for (name, shape) in design_stmt_assignments(design, initial) {
                if !name.starts_with("__") && !name.contains('.') {
                    actual_procedural_targets
                        .entry(name)
                        .or_default()
                        .push(shape);
                }
            }
        }
        for process in &design.processes {
            for (name, shape) in design_stmt_assignments(design, &process.body) {
                if !name.starts_with("__") && !name.contains('.') {
                    actual_procedural_targets
                        .entry(name)
                        .or_default()
                        .push(shape);
                }
            }
        }
        for (name, expected_shapes) in expected_procedural_targets {
            let actual_shapes = actual_procedural_targets
                .get(&name)
                .cloned()
                .unwrap_or_default();
            if !contains_expected_shapes(&actual_shapes, &expected_shapes) {
                return Err(format!(
                    "bounded elaborator procedural assignment shape for '{name}' does not match slang: internal {actual_shapes:?}, slang {expected_shapes:?}"
                ) + &self.span_note("procedural:"));
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
            ) + &self.span_note("instance:"));
        }
        Ok(())
    }

    fn span_note(&self, key_prefix: &str) -> String {
        self.source_spans
            .iter()
            .find(|(key, _)| key.as_str() == key_prefix || key.starts_with(key_prefix))
            .map(|(key, span)| {
                format!(
                    " [slang {key} at {}:{}:{}-{}:{}]",
                    span.file, span.start_line, span.start_column, span.end_line, span.end_column
                )
            })
            .unwrap_or_default()
    }
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_string)
}

fn bool_field(value: &Value, field: &str) -> bool {
    value.get(field).and_then(Value::as_bool).unwrap_or(false)
}

fn usize_field(value: &Value, field: &str) -> Option<usize> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn source_span(value: &Value) -> Option<SlangSourceSpan> {
    let (file, start_line, start_column, end_line, end_column) =
        if let (Some(file), Some(start_line), Some(start_column)) = (
            string_field(value, "source_file_start"),
            usize_field(value, "source_line_start"),
            usize_field(value, "source_column_start"),
        ) {
            (
                file,
                start_line,
                start_column,
                usize_field(value, "source_line_end").unwrap_or(start_line),
                usize_field(value, "source_column_end").unwrap_or(start_column),
            )
        } else {
            let file = string_field(value, "source_file")?;
            let start_line = usize_field(value, "source_line")?;
            let start_column = usize_field(value, "source_column")?;
            (file, start_line, start_column, start_line, start_column)
        };
    Some(SlangSourceSpan {
        file,
        start_line,
        start_column,
        end_line,
        end_column,
    })
}

fn parameter_summary(member: &Value) -> Result<SlangParameterSummary, String> {
    let name = string_field(member, "name")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "slang AST Parameter has no name".to_string())?;
    Ok(SlangParameterSummary {
        name,
        type_name: string_field(member, "type").unwrap_or_default(),
        value: string_field(member, "value").unwrap_or_default(),
        is_local: bool_field(member, "isLocal"),
        is_port: bool_field(member, "isPort"),
    })
}

fn instance_parameters(body: &Value) -> Result<Vec<SlangParameterSummary>, String> {
    let mut parameters = Vec::new();
    if let Some(members) = body.get("members").and_then(Value::as_array) {
        for member in members {
            if string_field(member, "kind").as_deref() == Some("Parameter") {
                parameters.push(parameter_summary(member)?);
            }
        }
    }
    Ok(parameters)
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
    } else if matches!(
        string_field(value, "kind").as_deref(),
        Some("ElementSelect" | "RangeSelect")
    ) {
        value
            .get("value")
            .map(expression_target_names)
            .unwrap_or_default()
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

fn expression_summary(value: &Value) -> SlangExpressionSummary {
    let kind = string_field(value, "kind").unwrap_or_default();
    let literal = expression_literal(value);
    // The bounded IR folds parameter constants to unsigned 32-bit Const nodes.
    // Keep those leaves comparable without inventing a signed Constant type.
    let type_signed = if kind == "NamedValue" && literal.is_some() {
        Some(false)
    } else {
        string_field(value, "type").and_then(|type_name| simple_type_signed(&type_name))
    };
    SlangExpressionSummary {
        kind,
        operator: string_field(value, "op").unwrap_or_default(),
        type_width: string_field(value, "type").and_then(|type_name| simple_type_width(&type_name)),
        type_signed,
        symbol: expression_symbol(value),
        literal,
        operands: expression_operands(value),
    }
}

fn empty_expression_summary() -> SlangExpressionSummary {
    SlangExpressionSummary {
        kind: String::new(),
        operator: String::new(),
        type_width: None,
        type_signed: None,
        symbol: None,
        literal: None,
        operands: Vec::new(),
    }
}

fn expression_literal(value: &Value) -> Option<String> {
    let kind = string_field(value, "kind")?;
    let raw = match kind.as_str() {
        "IntegerLiteral" => string_field(value, "value")?,
        "NamedValue" => string_field(value, "constant")?,
        _ => return None,
    };
    let type_name = string_field(value, "type")?;
    let word = literal_word(&type_name, &raw)?;
    let width = word.width();
    Some(format!("{width}:{}", word.to_bin_str()))
}

fn literal_word(type_name: &str, raw: &str) -> Option<eda_core::logic::LogicWord> {
    let width = simple_type_width(type_name)?;
    if let Some(separator) = raw.find('\'') {
        let reported_width = raw[..separator].parse::<usize>().ok()?;
        if reported_width != width {
            return None;
        }
        return eda_core::logic::LogicWord::parse(width, &format!("'{}", &raw[separator + 1..]));
    }
    let digits = raw.replace('_', "");
    let signed_value = digits.parse::<i128>().ok()?;
    let value = u64::try_from(signed_value).unwrap_or(signed_value as u128 as u64);
    Some(eda_core::logic::LogicWord::from_u64(width, value))
}

fn constant_json_word(value: &serde_json::Value) -> Option<eda_core::logic::LogicWord> {
    let type_name = string_field(value, "type")?;
    let raw = string_field(value, "constant")?;
    literal_word(&type_name, &raw)
}

fn slang_constant_u64(value: &serde_json::Value) -> Result<u64, String> {
    integer_literal_word(value)
        .or_else(|| named_constant_word(value))
        .or_else(|| constant_json_word(value))
        .and_then(|word| word.to_u64())
        .ok_or_else(|| "memory-file start/finish bounds must be integer constants".to_string())
}

fn memory_initializer_statements(
    memory: usize,
    dimensions: &[eda_core::ir::MemoryDimension],
    pattern: &serde_json::Value,
) -> Result<Vec<eda_core::ir::Stmt>, String> {
    fn visit(
        memory: usize,
        dimensions: &[eda_core::ir::MemoryDimension],
        pattern: &serde_json::Value,
        indexes: &mut Vec<i64>,
        statements: &mut Vec<eda_core::ir::Stmt>,
    ) -> Result<(), String> {
        if indexes.len() == dimensions.len() {
            let word = constant_json_word(pattern).ok_or_else(|| {
                "sparse-memory initializer elements must be integer constants".to_string()
            })?;
            statements.push(eda_core::ir::Stmt::MemoryBl {
                memory,
                indexes: indexes
                    .iter()
                    .map(|index| {
                        eda_core::ir::Expr::Const(eda_core::logic::LogicWord::from_u64(
                            32,
                            *index as u64,
                        ))
                    })
                    .collect(),
                e: eda_core::ir::Expr::Const(word),
            });
            return Ok(());
        }
        if string_field(pattern, "kind").as_deref() != Some("SimpleAssignmentPattern") {
            return Err(
                "direct slang-AST lowering supports only simple sparse-memory initializers".into(),
            );
        }
        let elements = pattern
            .get("elements")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "sparse-memory initializer has no elements".to_string())?;
        let dimension = dimensions[indexes.len()];
        if elements.len() != dimension.length {
            return Err(format!(
                "sparse-memory initializer dimension has {} elements, expected {}",
                elements.len(),
                dimension.length
            ));
        }
        for (position, element) in elements.iter().enumerate() {
            let logical_index = dimension.base + dimension.step * position as i64;
            if logical_index < 0 {
                return Err(
                    "negative sparse-memory initializer indexes are not yet lowered".into(),
                );
            }
            indexes.push(logical_index);
            visit(memory, dimensions, element, indexes, statements)?;
            indexes.pop();
        }
        Ok(())
    }

    let mut statements = Vec::new();
    let mut indexes = Vec::new();
    visit(memory, dimensions, pattern, &mut indexes, &mut statements)?;
    Ok(statements)
}

fn resolve_memory_file_path(raw_path: &str, statement: &serde_json::Value) -> std::path::PathBuf {
    let raw_path = std::path::PathBuf::from(raw_path);
    if raw_path.is_absolute() {
        return raw_path;
    }
    let source_file = source_span(statement)
        .map(|span| std::path::PathBuf::from(span.file))
        .unwrap_or_default();
    let source_relative = source_file
        .parent()
        .map(|parent| parent.join(&raw_path))
        .unwrap_or_else(|| raw_path.clone());
    if source_relative.is_file() {
        source_relative
    } else {
        raw_path
    }
}

fn resolve_memory_output_path(raw_path: &str, statement: &serde_json::Value) -> std::path::PathBuf {
    let raw_path = std::path::PathBuf::from(raw_path);
    let source_file = source_span(statement)
        .map(|span| std::path::PathBuf::from(span.file))
        .unwrap_or_default();
    let resolved = if raw_path.is_absolute() {
        raw_path
    } else {
        source_file
            .parent()
            .map(|parent| parent.join(&raw_path))
            .unwrap_or(raw_path)
    };
    let Some(parent) = resolved.parent() else {
        return resolved;
    };
    let Ok(canonical_parent) = parent.canonicalize() else {
        return resolved;
    };
    match resolved.file_name() {
        Some(file_name) => canonical_parent.join(file_name),
        None => resolved,
    }
}

fn integer_literal_word(value: &serde_json::Value) -> Option<eda_core::logic::LogicWord> {
    if string_field(value, "kind").as_deref() != Some("IntegerLiteral") {
        return None;
    }
    literal_word(
        &string_field(value, "type")?,
        &string_field(value, "value")?,
    )
}

fn named_constant_word(value: &serde_json::Value) -> Option<eda_core::logic::LogicWord> {
    if string_field(value, "kind").as_deref() != Some("NamedValue") {
        return None;
    }
    literal_word(
        &string_field(value, "type")?,
        &string_field(value, "constant")?,
    )
}

fn expression_symbol(value: &Value) -> Option<String> {
    string_field(value, "symbol")
        .as_deref()
        .and_then(|symbol| symbol.split_whitespace().next_back())
        .filter(|symbol| !symbol.is_empty())
        .map(str::to_string)
}

fn expression_operands(value: &Value) -> Vec<SlangExpressionSummary> {
    let kind = string_field(value, "kind").unwrap_or_default();
    let mut operands = Vec::new();
    match kind.as_str() {
        "UnaryOp" => {
            if let Some(operand) = value.get("operand") {
                operands.push(expression_summary(operand));
            }
        }
        "BinaryOp" => {
            for field in ["left", "right"] {
                if let Some(operand) = value.get(field) {
                    operands.push(expression_summary(operand));
                }
            }
        }
        "Conversion" => {
            if let Some(operand) = value.get("operand") {
                operands.push(expression_summary(operand));
            }
        }
        "Concatenation" => {
            if let Some(values) = value.get("operands").and_then(Value::as_array) {
                operands.extend(values.iter().map(expression_summary));
            }
        }
        "ConditionalOp" => {
            if let Some(conditions) = value.get("conditions").and_then(Value::as_array) {
                operands.extend(
                    conditions
                        .iter()
                        .filter_map(|condition| condition.get("expr"))
                        .map(expression_summary),
                );
            }
            for field in ["left", "right"] {
                if let Some(operand) = value.get(field) {
                    operands.push(expression_summary(operand));
                }
            }
        }
        _ => {}
    }
    operands
}

fn procedural_assignments(value: &Value) -> Vec<SlangAssignmentSummary> {
    let mut assignments = Vec::new();
    if string_field(value, "kind").as_deref() == Some("Assignment") {
        if let (Some(left), Some(right)) = (value.get("left"), value.get("right")) {
            assignments.push(SlangAssignmentSummary {
                target_names: expression_target_names(left),
                rhs: expression_summary(right),
                nonblocking: bool_field(value, "isNonBlocking"),
            });
        }
    }
    match value {
        Value::Array(values) => values
            .iter()
            .for_each(|nested| assignments.extend(procedural_assignments(nested))),
        Value::Object(fields) => {
            let is_assignment = string_field(value, "kind").as_deref() == Some("Assignment");
            fields
                .iter()
                .filter(|(field, _)| {
                    !(is_assignment && (field.as_str() == "left" || field.as_str() == "right"))
                })
                .for_each(|(_, nested)| assignments.extend(procedural_assignments(nested)));
        }
        _ => {}
    }
    assignments
}

fn slang_expression_shape(expression: &SlangExpressionSummary) -> Option<&'static str> {
    match (expression.kind.as_str(), expression.operator.as_str()) {
        ("NamedValue", _) if expression.literal.is_some() => Some("Const"),
        ("NamedValue", _) => Some("Ref"),
        ("IntegerLiteral" | "RealLiteral" | "StringLiteral", _) => Some("Const"),
        // Selects are canonicalized differently by the bounded frontend:
        // constant selects may become direct references while dynamic selects
        // become Index nodes. Defer exact select-shape equivalence to typed
        // AST lowering rather than guessing here.
        ("ElementSelect" | "RangeSelect", _) => None,
        ("Concat" | "Concatenation", _) => Some("Concat"),
        ("Replication", _) => Some("Repeat"),
        ("ConditionalOp", _) => Some("Cond"),
        ("UnaryOp", "BitwiseNot") => Some("Not"),
        ("UnaryOp", "LogicalNot") => Some("Not"),
        ("BinaryOp", "Add") => Some("Add"),
        ("BinaryOp", "Subtract") => Some("Sub"),
        ("BinaryOp", "Multiply") => Some("Mul"),
        ("BinaryOp", "Divide") => Some("Div"),
        ("BinaryOp", "Modulus") => Some("Mod"),
        ("BinaryOp", "BinaryAnd") => Some("And"),
        ("BinaryOp", "BinaryOr") => Some("Or"),
        ("BinaryOp", "BitwiseXor") => Some("Xor"),
        ("BinaryOp", "LogicalAnd") => Some("LogAnd"),
        ("BinaryOp", "LogicalOr") => Some("LogOr"),
        ("BinaryOp", "Equality") => Some("Eq"),
        ("BinaryOp", "Inequality") => Some("Ne"),
        ("BinaryOp", "LessThan") => Some("Lt"),
        ("BinaryOp", "LessThanEqual") => Some("Le"),
        ("BinaryOp", "GreaterThan") => Some("Gt"),
        ("BinaryOp", "GreaterThanEqual") => Some("Ge"),
        _ => None,
    }
}

fn contains_expected_shapes(actual: &[String], expected: &[String]) -> bool {
    let mut remaining = actual.to_vec();
    for shape in expected {
        let index = if shape == "?" {
            if remaining.is_empty() {
                return false;
            }
            0
        } else {
            let Some(index) = remaining.iter().position(|candidate| candidate == shape) else {
                return false;
            };
            index
        };
        remaining.swap_remove(index);
    }
    true
}

fn slang_expression_fingerprint(expression: &SlangExpressionSummary) -> Option<String> {
    if expression.kind == "Conversion" {
        let operand = expression.operands.first()?;
        if operand.kind != "IntegerLiteral"
            || expression.type_width.is_none()
            || expression.type_width != operand.type_width
            || expression.type_signed != operand.type_signed
        {
            return None;
        }
        return slang_expression_fingerprint(operand);
    }
    let root = slang_expression_shape(expression)?;
    let width = expression.type_width?;
    let signed = expression.type_signed?;
    let signed_label = if signed { "s" } else { "u" };
    if root == "Ref" {
        let symbol = expression.symbol.as_deref()?;
        return Some(format!("Ref:{width}:{signed_label}[{symbol}]"));
    }
    if root == "Const" {
        let literal = expression.literal.as_deref()?;
        return Some(format!("Const:{width}:{signed_label}[{literal}]"));
    }
    let operands = expression
        .operands
        .iter()
        .map(slang_expression_fingerprint)
        .collect::<Option<Vec<_>>>()?;
    Some(format!(
        "{}:{width}:{signed_label}[{}]",
        root,
        operands
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(",")
    ))
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
                let parameters = member
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
                    continuous_targets.push(SlangContinuousTarget {
                        name,
                        source: "ContinuousAssign".into(),
                        rhs: rhs.clone(),
                    });
                }
            }
            "ProceduralBlock" => {
                let procedure_kind = string_field(member, "procedureKind").unwrap_or_default();
                let assignments = procedural_assignments(member);
                let assignment_targets = assignments
                    .iter()
                    .flat_map(|assignment| assignment.target_names.iter().cloned())
                    .collect();
                procedural_blocks.push(SlangProceduralBlockSummary {
                    procedure_kind,
                    assignment_targets,
                    assignments,
                });
            }
            nested if nested.starts_with("GenerateBlock") => {
                collect_generate_summary(member, instances, continuous_targets, procedural_blocks)?;
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

fn design_expr_shape(expression: &eda_core::ir::Expr) -> &'static str {
    use eda_core::ir::{BinOp, Expr, UnOp};
    match expression {
        Expr::Const(_) => "Const",
        Expr::Ref { .. } | Expr::MemoryRead { .. } | Expr::MemoryReadPart { .. } => "Ref",
        Expr::Un { op, .. } => match op {
            UnOp::Not => "Not",
            UnOp::AndR | UnOp::OrR | UnOp::XorR => "Reduction",
        },
        Expr::Bin { op, .. } => match op {
            BinOp::Add => "Add",
            BinOp::Sub => "Sub",
            BinOp::Mul => "Mul",
            BinOp::Div => "Div",
            BinOp::Mod => "Mod",
            BinOp::And => "And",
            BinOp::Or => "Or",
            BinOp::Xor => "Xor",
            BinOp::Shl => "Shl",
            BinOp::Shr => "Shr",
            BinOp::Eq => "Eq",
            BinOp::Ne => "Ne",
            BinOp::Lt => "Lt",
            BinOp::Le => "Le",
            BinOp::Gt => "Gt",
            BinOp::Ge => "Ge",
            BinOp::LogAnd => "LogAnd",
            BinOp::LogOr => "LogOr",
            BinOp::CaseEq => "CaseEq",
            BinOp::CaseNe => "CaseNe",
        },
        Expr::Cond { .. } => "Cond",
        Expr::Concat(_) => "Concat",
        Expr::Repeat { .. } => "Repeat",
        Expr::Signed { .. } => "Signed",
        Expr::Index { .. } => "Index",
    }
}

fn design_expr_fingerprint(design: &Design, expression: &eda_core::ir::Expr) -> String {
    use eda_core::ir::Expr;
    let root = design_expr_shape(expression);
    if root == "Ref" {
        if let Some(name) = design_expr_symbol(design, expression) {
            return format!(
                "Ref:{}:{}[{name}]",
                expression.width(),
                if expression.is_signed() { "s" } else { "u" }
            );
        }
    }
    if let Expr::Const(word) = expression {
        return format!(
            "Const:{}:u[{}:{}]",
            word.width(),
            word.width(),
            word.to_bin_str()
        );
    }
    let operands: Vec<String> = match expression {
        Expr::Const(_) | Expr::Ref { .. } | Expr::MemoryRead { .. } => Vec::new(),
        Expr::MemoryReadPart { indexes, start, .. } => indexes
            .iter()
            .chain(std::iter::once(start.as_ref()))
            .map(|operand| design_expr_fingerprint(design, operand))
            .collect(),
        Expr::Un { a, .. } => vec![design_expr_fingerprint(design, a)],
        Expr::Bin { a, b, .. } => {
            vec![
                design_expr_fingerprint(design, a),
                design_expr_fingerprint(design, b),
            ]
        }
        Expr::Cond { c, t, f } => vec![
            design_expr_fingerprint(design, c),
            design_expr_fingerprint(design, t),
            design_expr_fingerprint(design, f),
        ],
        Expr::Concat(parts) => parts
            .iter()
            .map(|operand| design_expr_fingerprint(design, operand))
            .collect(),
        Expr::Repeat { e, .. } => vec![design_expr_fingerprint(design, e)],
        Expr::Signed { e, .. } => vec![design_expr_fingerprint(design, e)],
        Expr::Index { e, .. } => vec![design_expr_fingerprint(design, e)],
    };
    format!(
        "{}:{}:{}[{}]",
        root,
        expression.width(),
        if expression.is_signed() { "s" } else { "u" },
        operands
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn design_expr_symbol(design: &Design, expression: &eda_core::ir::Expr) -> Option<String> {
    use eda_core::ir::Expr;
    match expression {
        Expr::Ref { net, .. } => design
            .net_decls
            .iter()
            .find(|declaration| {
                *net >= declaration.base && *net < declaration.base + declaration.width
            })
            .map(|declaration| declaration.name.clone()),
        Expr::MemoryRead { memory, .. } | Expr::MemoryReadPart { memory, .. } => design
            .memories
            .get(*memory)
            .map(|memory| memory.name.clone()),
        _ => None,
    }
}

fn design_stmt_assignments(
    design: &Design,
    statement: &eda_core::ir::Stmt,
) -> Vec<(String, String)> {
    use eda_core::ir::Stmt;
    let mut assignments = Vec::new();
    match statement {
        Stmt::Block(statements) => statements
            .iter()
            .for_each(|nested| assignments.extend(design_stmt_assignments(design, nested))),
        Stmt::Forever(body) | Stmt::Delay { body, .. } => {
            assignments.extend(design_stmt_assignments(design, body));
        }
        Stmt::NBl { lv, e } | Stmt::Bl { lv, e } => {
            if let Some(name) = design_lvalue_name(design, lv) {
                assignments.push((name, design_expr_fingerprint(design, e)));
            }
        }
        Stmt::MemoryBl { memory, e, .. }
        | Stmt::MemoryNBl { memory, e, .. }
        | Stmt::MemoryBlPart { memory, e, .. }
        | Stmt::MemoryNBlPart { memory, e, .. } => {
            assignments.extend(
                design
                    .memories
                    .get(*memory)
                    .map(|memory| (memory.name.clone(), design_expr_fingerprint(design, e))),
            );
        }
        Stmt::BlIndexed { net, e, .. } | Stmt::NBlIndexed { net, e, .. } => {
            assignments.extend(design.net_decls.iter().find_map(|declaration| {
                (*net >= declaration.base && *net < declaration.base + declaration.width)
                    .then(|| (declaration.name.clone(), design_expr_fingerprint(design, e)))
            }));
        }
        Stmt::If { t, f, .. } => {
            assignments.extend(design_stmt_assignments(design, t));
            if let Some(f) = f {
                assignments.extend(design_stmt_assignments(design, f));
            }
        }
        Stmt::Case { arms, dflt, .. } => {
            for (_, body) in arms {
                assignments.extend(design_stmt_assignments(design, body));
            }
            if let Some(body) = dflt {
                assignments.extend(design_stmt_assignments(design, body));
            }
        }
        Stmt::Display { .. }
        | Stmt::Assert { .. }
        | Stmt::Assume { .. }
        | Stmt::Cover { .. }
        | Stmt::WriteMem { .. }
        | Stmt::Finish => {}
    }
    assignments
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
    static NEXT_UNIQUE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let unique = format!(
        "eda-slang-{}-{}-{}.sv",
        std::process::id(),
        NEXT_UNIQUE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
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
