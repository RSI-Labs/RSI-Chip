//! Direct lowering from slang's post-elaboration AST JSON into the repository IR.

use crate::ast_util::{bool_field, string_field};
use eda_core::ir::Design;
use eda_core::logic::LogicWord;

mod declarations;
mod expressions;
mod hierarchy;
mod statements;

pub(super) fn parse_ast_root(json: &str) -> Result<serde_json::Value, String> {
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

#[derive(Clone)]
struct AstLoweredSymbol {
    base: usize,
    width: usize,
    signed: bool,
    /// Declared bounds for a simple packed vector and all packed dimensions
    /// used to map logical selects onto canonical physical bits.
    bounds: Option<(i64, i64)>,
    dimensions: Vec<crate::types::AstPackedDimension>,
    memory: Option<usize>,
}

pub(super) struct SlangAstLowerer {
    design: Design,
    symbols: std::collections::BTreeMap<String, AstLoweredSymbol>,
    constants: std::collections::BTreeMap<String, LogicWord>,
}

impl SlangAstLowerer {
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
            constants: std::collections::BTreeMap::new(),
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
                "Parameter" => self.lower_parameter(member)?,
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
}

fn declaration_name(member: &serde_json::Value, kind: &str) -> Result<String, String> {
    string_field(member, "name")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("slang AST {kind} has no name"))
}
