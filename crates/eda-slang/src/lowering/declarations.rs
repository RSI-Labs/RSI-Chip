//! Module member, port, net, memory, and parameter lowering.

use super::{declaration_name, AstLoweredSymbol, SlangAstLowerer};
use crate::ast_util::{literal_word, memory_initializer_statements, string_field};
use crate::types::{memory_type, simple_type_shape, simple_type_signed};

impl SlangAstLowerer {
    pub(super) fn lower_port(&mut self, member: &serde_json::Value) -> Result<(), String> {
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

    pub(super) fn lower_memory_port(
        &mut self,
        member: &serde_json::Value,
        name: &str,
    ) -> Result<(), String> {
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
        let symbol = self.symbols[name].clone();
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

    pub(super) fn lower_declaration(&mut self, member: &serde_json::Value) -> Result<(), String> {
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

    pub(super) fn lower_memory_declaration(
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
            byte_enable: None,
            banking: eda_core::ir::MemoryBanking::default(),
        });
        self.symbols.insert(
            name.to_string(),
            AstLoweredSymbol {
                base,
                width: memory.element_width,
                signed: memory.signed,
                bounds: Some((memory.element_msb, memory.element_lsb)),
                dimensions: Vec::new(),
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

    pub(super) fn declare_net(
        &mut self,
        member: &serde_json::Value,
        name: &str,
        kind: &str,
    ) -> Result<AstLoweredSymbol, String> {
        if self.symbols.contains_key(name) {
            let existing = self.symbols[name].clone();
            let type_name = string_field(member, "type").unwrap_or_default();
            let shape = simple_type_shape(&type_name)
                .ok_or_else(|| format!("direct lowering cannot resolve type '{type_name}'"))?;
            let signed = simple_type_signed(&type_name).ok_or_else(|| {
                format!("direct lowering cannot resolve signedness of '{type_name}'")
            })?;
            if existing.width != shape.width
                || existing.bounds != shape.bounds
                || existing.signed != signed
            {
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
        let shape = simple_type_shape(&type_name)
            .ok_or_else(|| format!("direct lowering cannot resolve type '{type_name}'"))?;
        let signed = simple_type_signed(&type_name)
            .ok_or_else(|| format!("direct lowering cannot resolve signedness of '{type_name}'"))?;
        let base = self.design.n_nets;
        let symbol = AstLoweredSymbol {
            base,
            width: shape.width,
            signed,
            bounds: shape.bounds,
            dimensions: shape.dimensions,
            memory: None,
        };
        self.design.n_nets += shape.width;
        self.design.net_decls.push(eda_core::ir::NetDecl {
            name: name.to_string(),
            width: shape.width,
            is_reg: kind == "Port" || kind == "Variable",
            is_signed: signed,
            base,
        });
        self.symbols.insert(name.to_string(), symbol.clone());
        Ok(symbol)
    }

    pub(super) fn lower_parameter(&mut self, member: &serde_json::Value) -> Result<(), String> {
        let name = declaration_name(member, "Parameter")?;
        let type_name = string_field(member, "type").unwrap_or_default();
        let shape = simple_type_shape(&type_name)
            .ok_or_else(|| format!("parameter '{name}' has unsupported type '{type_name}'"))?;
        let value = string_field(member, "value")
            .ok_or_else(|| format!("parameter '{name}' has no elaborated constant value"))?;
        let word = literal_word(&type_name, &value)
            .ok_or_else(|| format!("parameter '{name}' has unsupported value '{value}'"))?;
        if word.width() != shape.width {
            return Err(format!(
                "parameter '{name}' value width {} does not match type width {}",
                word.width(),
                shape.width
            ));
        }
        self.constants.insert(name, word);
        Ok(())
    }
}
