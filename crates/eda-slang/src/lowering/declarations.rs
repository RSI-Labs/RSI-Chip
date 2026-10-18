//! Module member, port, net, memory, and parameter lowering.

use super::{
    declaration_name, AstEnumType, AstLoweredSymbol, AstStructField, AstStructType, SlangAstLowerer,
};
use crate::ast_util::{
    bool_field, literal_word, memory_initializer_statements, string_field, usize_field,
};
use crate::types::{
    memory_type, memory_type_with_element, simple_type_shape, simple_type_signed, AstMemoryType,
    AstPackedDimension, AstTypeShape,
};

impl SlangAstLowerer {
    pub(super) fn collect_procedure_locals(
        &mut self,
        member: &serde_json::Value,
    ) -> Result<(), String> {
        let Some(block_address) = usize_field(member, "addr") else {
            return Err("procedure-local block has no symbol address".into());
        };
        let variables = member
            .get("members")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "procedure-local block has no members".to_string())?;
        let mut symbol_ids = Vec::new();
        for variable in variables {
            if string_field(variable, "kind").as_deref() != Some("Variable") {
                continue;
            }
            let name = declaration_name(variable, "local Variable")?;
            if string_field(variable, "lifetime").as_deref() != Some("Automatic") {
                return Err(format!(
                    "procedure-local variable '{name}' must be automatic"
                ));
            }
            let Some(initializer) = variable.get("initializer") else {
                return Err(format!(
                    "procedure-local variable '{name}' must have an initializer in this subset"
                ));
            };
            let Some(address) = usize_field(variable, "addr") else {
                return Err(format!(
                    "procedure-local variable '{name}' has no symbol address"
                ));
            };
            let symbol_id = format!("{address} {name}");
            let type_name = string_field(variable, "type").unwrap_or_default();
            if self.declared_memory_type(&type_name).is_some() {
                return Err(format!(
                    "procedure-local sparse memories are not supported for '{name}'"
                ));
            }
            let (shape, signed, struct_fields) = self.net_type(&type_name)?;
            let internal_name = format!("__local_{address}_{name}");
            let base = self.design.n_nets;
            self.design.n_nets += shape.width;
            self.design.net_decls.push(eda_core::ir::NetDecl {
                name: internal_name,
                width: shape.width,
                is_reg: true,
                is_signed: signed,
                base,
            });
            self.local_symbols.insert(
                symbol_id.clone(),
                AstLoweredSymbol {
                    base,
                    width: shape.width,
                    signed,
                    bounds: shape.bounds,
                    dimensions: shape.dimensions,
                    memory: None,
                    struct_fields,
                },
            );
            self.local_initializers
                .insert(symbol_id.clone(), initializer.clone());
            symbol_ids.push(symbol_id);
        }
        self.local_blocks
            .insert(block_address.to_string(), symbol_ids);
        Ok(())
    }

    pub(super) fn collect_type_alias(&mut self, member: &serde_json::Value) -> Result<(), String> {
        let name = declaration_name(member, "TypeAlias")?;
        let target = member
            .get("target")
            .ok_or_else(|| format!("type alias '{name}' has no detailed target type"))?;
        if target.is_string() {
            let target_name = target
                .as_str()
                .unwrap_or_default()
                .split_whitespace()
                .next_back()
                .unwrap_or_default()
                .rsplit(['.', '#'])
                .next()
                .unwrap_or_default();
            if self.type_enums.contains_key(target_name) {
                return Ok(());
            }
        }
        let target_kind = string_field(target, "kind").unwrap_or_default();
        let is_union = target_kind == "PackedUnionType";
        if !matches!(target_kind.as_str(), "PackedStructType" | "PackedUnionType") {
            return Err(format!(
                "direct slang-AST lowering does not support type alias '{name}' target"
            ));
        }
        let fields = target
            .get("members")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("packed aggregate type '{name}' has no fields"))?;
        let mut struct_fields = std::collections::BTreeMap::new();
        let mut width = 0_usize;
        for field in fields {
            let field_name = field
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("packed struct type '{name}' has an unnamed field"))?;
            let offset = field
                .get("bitOffset")
                .and_then(serde_json::Value::as_u64)
                .and_then(|offset| usize::try_from(offset).ok())
                .ok_or_else(|| {
                    format!("packed struct field '{name}.{field_name}' has no bit offset")
                })?;
            let field_index = field
                .get("fieldIndex")
                .and_then(serde_json::Value::as_u64)
                .and_then(|index| usize::try_from(index).ok())
                .ok_or_else(|| {
                    format!("packed struct field '{name}.{field_name}' has no field index")
                })?;
            let field_type = field
                .get("type")
                .ok_or_else(|| format!("packed struct field '{name}.{field_name}' has no type"))?;
            let field_width = if field_type.is_object() {
                detailed_type_width(field_type)?
            } else {
                let type_name = string_field(field, "type").unwrap_or_default();
                self.net_type(&type_name)?.0.width
            };
            width = width.max(offset.saturating_add(field_width));
            if is_union && offset != 0 {
                return Err(format!(
                    "packed union '{name}' member '{field_name}' does not start at bit zero"
                ));
            }
            struct_fields.insert(
                field_name.to_string(),
                AstStructField {
                    offset,
                    width: field_width,
                    field_index,
                },
            );
        }
        if is_union && struct_fields.values().any(|field| field.width != width) {
            return Err(format!(
                "packed union '{name}' members must all have the same width"
            ));
        }
        let definition = AstStructType {
            width,
            signed: bool_field(target, "isSigned"),
            fields: struct_fields,
            is_union,
        };
        self.type_aliases.insert(name, definition);
        Ok(())
    }

    pub(super) fn collect_enum_type(&mut self, member: &serde_json::Value) -> Result<(), String> {
        let name = declaration_name(member, "EnumType")?;
        let base = member
            .get("baseType")
            .ok_or_else(|| format!("enum type '{name}' has no base type"))?;
        let width = detailed_type_width(base)?;
        let signed = detailed_type_signed(base);
        let value_type = if width == 1 {
            "logic".to_string()
        } else {
            format!("logic[{}:0]", width - 1)
        };
        let values = member
            .get("members")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("enum type '{name}' has no values"))?;
        let mut ordered_values = Vec::with_capacity(values.len());
        let mut ordered_names = Vec::with_capacity(values.len());
        for value in values {
            let value_name = value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("enum type '{name}' has an unnamed value"))?;
            let raw = string_field(value, "value")
                .ok_or_else(|| format!("enum value '{name}.{value_name}' has no value"))?;
            let word = literal_word(&value_type, &raw)
                .ok_or_else(|| format!("enum value '{name}.{value_name}' is invalid"))?;
            ordered_values.push(word.clone());
            ordered_names.push(value_name.to_string());
            self.enum_constants.insert(value_name.to_string(), word);
        }
        self.type_enums.insert(
            name,
            AstEnumType {
                width,
                signed,
                values: ordered_values,
                names: ordered_names,
            },
        );
        Ok(())
    }

    pub(super) fn net_type(
        &self,
        type_name: &str,
    ) -> Result<
        (
            AstTypeShape,
            bool,
            std::collections::BTreeMap<String, AstStructField>,
        ),
        String,
    > {
        if let Some(shape) = simple_type_shape(type_name) {
            let signed = simple_type_signed(type_name).ok_or_else(|| {
                format!("direct lowering cannot resolve signedness of '{type_name}'")
            })?;
            return Ok((shape, signed, std::collections::BTreeMap::new()));
        }
        let alias_name = type_name
            .split_whitespace()
            .next_back()
            .unwrap_or_default()
            .rsplit(['.', '#'])
            .next()
            .unwrap_or_default();
        if let Some(definition) = self.type_enums.get(alias_name) {
            return Ok((
                AstTypeShape {
                    width: definition.width,
                    bounds: None,
                    dimensions: Vec::<AstPackedDimension>::new(),
                },
                definition.signed,
                std::collections::BTreeMap::new(),
            ));
        }
        let definition = self
            .type_aliases
            .get(alias_name)
            .ok_or_else(|| format!("direct lowering cannot resolve type '{type_name}'"))?;
        Ok((
            AstTypeShape {
                width: definition.width,
                bounds: None,
                dimensions: Vec::<AstPackedDimension>::new(),
            },
            definition.signed,
            definition.fields.clone(),
        ))
    }

    pub(super) fn declared_memory_type(&self, type_name: &str) -> Option<AstMemoryType> {
        if let Some(memory) = memory_type(type_name) {
            return Some(memory);
        }
        let separator = type_name.find('$')?;
        let element_type = type_name[..separator].trim();
        let (shape, signed, _) = self.net_type(element_type).ok()?;
        let element_msb = i64::try_from(shape.width.saturating_sub(1)).ok()?;
        let bounds = shape.bounds.unwrap_or((element_msb, 0));
        memory_type_with_element(type_name, shape.width, bounds, signed)
    }

    pub(super) fn lower_port(&mut self, member: &serde_json::Value) -> Result<(), String> {
        let name = declaration_name(member, "Port")?;
        let type_name = string_field(member, "type").unwrap_or_default();
        if type_name == "event" {
            return Err(format!("event ports are not supported for '{name}'"));
        }
        if self.declared_memory_type(&type_name).is_some() {
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
        let type_name = string_field(member, "type").unwrap_or_default();
        if type_name.starts_with("event$") {
            self.declare_net(member, &name, &kind)?;
            if member.get("initializer").is_some() {
                return Err(format!("event array '{name}' cannot have an initializer"));
            }
            return Ok(());
        }
        if self.declared_memory_type(&type_name).is_some() {
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
        let type_name = string_field(member, "type").unwrap_or_default();
        let memory = self
            .declared_memory_type(&type_name)
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
                struct_fields: std::collections::BTreeMap::new(),
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
            let (shape, signed, _) = self.net_type(&type_name)?;
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
        let (shape, signed, struct_fields) = self.net_type(&type_name)?;
        let base = self.design.n_nets;
        let symbol = AstLoweredSymbol {
            base,
            width: shape.width,
            signed,
            bounds: shape.bounds,
            dimensions: shape.dimensions,
            memory: None,
            struct_fields,
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

fn detailed_type_width(type_value: &serde_json::Value) -> Result<usize, String> {
    match string_field(type_value, "kind").as_deref() {
        Some("ScalarType") => Ok(1),
        Some("PackedArrayType") => {
            let element = type_value
                .get("elementType")
                .ok_or_else(|| "packed array type has no element type".to_string())?;
            let element_width = detailed_type_width(element)?;
            let range = string_field(type_value, "range").unwrap_or_default();
            let Some((left, right)) = range.trim_matches(|c| c == '[' || c == ']').split_once(':')
            else {
                return Err(format!("packed array type has invalid range '{range}'"));
            };
            let left = left
                .trim()
                .parse::<i128>()
                .map_err(|_| format!("packed array type has invalid range bound '{left}'"))?;
            let right = right
                .trim()
                .parse::<i128>()
                .map_err(|_| format!("packed array type has invalid range bound '{right}'"))?;
            let length = (left - right).unsigned_abs() + 1;
            usize::try_from(length)
                .ok()
                .and_then(|length| element_width.checked_mul(length))
                .ok_or_else(|| "packed array type width overflows".to_string())
        }
        Some("PackedStructType") => {
            let mut width = 0_usize;
            let members = type_value
                .get("members")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| "packed struct type has no members".to_string())?;
            for member in members {
                let offset = member
                    .get("bitOffset")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|offset| usize::try_from(offset).ok())
                    .ok_or_else(|| "packed struct member has no bit offset".to_string())?;
                let field_type = member
                    .get("type")
                    .ok_or_else(|| "packed struct member has no type".to_string())?;
                width = width.max(offset.saturating_add(detailed_type_width(field_type)?));
            }
            Ok(width)
        }
        Some(kind) => Err(format!(
            "direct lowering does not support detailed type kind '{kind}'"
        )),
        None => Err("detailed type has no kind".into()),
    }
}

fn detailed_type_signed(type_value: &serde_json::Value) -> bool {
    match string_field(type_value, "kind").as_deref() {
        Some("PackedArrayType") => type_value
            .get("elementType")
            .map(detailed_type_signed)
            .unwrap_or(false),
        Some("PredefinedIntegerType") => !matches!(
            string_field(type_value, "name").as_deref(),
            Some("byte unsigned" | "shortint unsigned" | "int unsigned" | "longint unsigned")
        ),
        _ => bool_field(type_value, "isSigned"),
    }
}
