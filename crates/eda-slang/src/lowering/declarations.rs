//! Module member, port, net, memory, and parameter lowering.

use super::{
    declaration_name, AstEnumType, AstLoweredSymbol, AstStructField, AstStructType, SlangAstLowerer,
};
use crate::ast_util::canonical_detailed_type;
use crate::ast_util::{
    bool_field, literal_word, memory_initializer_statements, string_field, string_literal,
    usize_field, with_source_span,
};
use crate::types::{
    associative_memory_type, dynamic_memory_type, memory_type, memory_type_with_element,
    queue_memory_type, simple_type_shape, simple_type_signed, simple_type_width,
    string_literal_word, AstMemoryType, AstPackedDimension, AstTypeShape, STRING_WIDTH,
};

impl SlangAstLowerer {
    pub(super) fn prepare_plusargs(&mut self, body: &serde_json::Value) -> Result<(), String> {
        let mut calls = Vec::new();
        collect_plusarg_test_calls(body, &mut calls);
        let mut value_calls = Vec::new();
        collect_plusarg_value_calls(body, &mut value_calls);
        for call in calls {
            let empty_arguments = Vec::new();
            let arguments = call
                .get("arguments")
                .and_then(serde_json::Value::as_array)
                .unwrap_or(&empty_arguments);
            if arguments.len() != 1 {
                return Err(with_source_span(
                    call,
                    "$test$plusargs requires one string literal argument".into(),
                ));
            }
            let name = string_literal(&arguments[0])
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    with_source_span(
                        call,
                        "$test$plusargs requires a non-empty string literal".into(),
                    )
                })?;
            self.declare_plusarg_query(&name);
        }
        for call in value_calls {
            let plan = value_plusarg_plan(call).map_err(|error| with_source_span(call, error))?;
            self.declare_plusarg_query(&plan.query);
            self.declare_plusarg_value(&plan.query);
        }
        Ok(())
    }

    pub(super) fn declare_plusarg_query(&mut self, name: &str) {
        if self.plusargs.contains_key(name) {
            return;
        }
        let net_name = eda_core::ir::plusarg_net_name(name);
        let base = self.design.n_nets;
        self.design.n_nets += 1;
        self.design.net_decls.push(eda_core::ir::NetDecl {
            name: net_name,
            width: 1,
            is_reg: false,
            is_signed: false,
            base,
        });
        let symbol = AstLoweredSymbol {
            base,
            width: 1,
            signed: false,
            bounds: None,
            dimensions: Vec::new(),
            memory: None,
            struct_fields: std::collections::BTreeMap::new(),
        };
        self.plusargs.insert(name.to_string(), symbol);
        self.design.plusargs.insert(name.to_string(), base);
    }

    pub(super) fn declare_plusarg_value(&mut self, name: &str) {
        if self.plusarg_values.contains_key(name) {
            return;
        }
        let net_name = eda_core::ir::plusarg_value_net_name(name);
        let base = self.design.n_nets;
        self.design.n_nets += STRING_WIDTH;
        self.design.net_decls.push(eda_core::ir::NetDecl {
            name: net_name,
            width: STRING_WIDTH,
            is_reg: false,
            is_signed: false,
            base,
        });
        let symbol = AstLoweredSymbol {
            base,
            width: STRING_WIDTH,
            signed: false,
            bounds: None,
            dimensions: Vec::new(),
            memory: None,
            struct_fields: std::collections::BTreeMap::new(),
        };
        self.plusarg_values.insert(name.to_string(), symbol);
        self.design.plusarg_values.insert(name.to_string(), base);
    }

    fn insert_type_alias(
        &mut self,
        name: String,
        member: &serde_json::Value,
        definition: AstStructType,
        canonical_type_name: Option<String>,
    ) {
        let address = member.get("addr").and_then(serde_json::Value::as_u64);
        let scoped_name = if let Some(package) =
            string_field(member, "eda_package_scope").filter(|package| !package.is_empty())
        {
            address.map_or_else(
                || format!("{package}::{name}"),
                |address| format!("{address} {package}::{name}"),
            )
        } else {
            address.map_or_else(
                || format!("{}.{name}", self.design.name),
                |address| format!("{address} {}.{}", self.design.name, name),
            )
        };
        if string_field(member, "eda_package_scope").is_some() {
            self.package_type_aliases
                .insert(name.clone(), definition.clone());
            if let Some(canonical_type_name) = canonical_type_name.clone() {
                self.package_type_alias_type_names
                    .insert(name.clone(), canonical_type_name);
            }
        }
        self.type_aliases
            .insert(scoped_name.clone(), definition.clone());
        if let Some(canonical_type_name) = canonical_type_name {
            self.type_alias_type_names
                .insert(scoped_name, canonical_type_name.clone());
            self.type_alias_type_names
                .insert(name.clone(), canonical_type_name);
        }
        self.type_aliases.insert(name, definition);
    }

    fn canonical_type_alias_target(target: &serde_json::Value) -> Option<String> {
        if target.is_string() {
            return target.as_str().map(str::to_string);
        }
        let type_name = canonical_detailed_type(target)?;
        simple_type_shape(&type_name).map(|_| type_name)
    }

    pub(super) fn type_alias_definition(&self, type_name: &str) -> Option<&AstStructType> {
        let type_reference = type_name.split('$').next()?.trim();
        if !type_reference.is_empty() {
            if let Some(definition) = self.type_aliases.get(type_reference) {
                return Some(definition);
            }
        }
        let scoped_alias_name = type_name.split_whitespace().next_back()?;
        if let Some(definition) = self.type_aliases.get(scoped_alias_name) {
            return Some(definition);
        }
        let alias_name = scoped_alias_name.rsplit(['.', '#', ':']).next()?;
        self.type_aliases.get(alias_name)
    }

    pub(super) fn canonical_type_alias_name(&self, type_name: &str) -> Option<String> {
        let type_reference = type_name.split('$').next()?.trim();
        let mut current = if !type_reference.is_empty() {
            self.type_alias_type_names
                .get(type_reference)
                .cloned()
                .unwrap_or_else(|| type_reference.to_string())
        } else {
            type_name.to_string()
        };
        for _ in 0..64 {
            if simple_type_shape(&current).is_some() {
                return Some(current);
            }
            let scoped_alias_name = current.split_whitespace().next_back()?.to_string();
            let next = self
                .type_alias_type_names
                .get(&scoped_alias_name)
                .or_else(|| {
                    let alias_name = scoped_alias_name.rsplit(['.', '#', ':']).next()?;
                    self.type_alias_type_names.get(alias_name)
                })?
                .clone();
            if next == current {
                return None;
            }
            current = next;
        }
        None
    }

    pub(super) fn collect_procedure_locals(
        &mut self,
        member: &serde_json::Value,
    ) -> Result<(), String> {
        let Some(block_address) = usize_field(member, "addr") else {
            return Err("procedure-local block has no symbol address".into());
        };
        if let Some(block_name) = string_field(member, "name").filter(|name| !name.is_empty()) {
            self.known_assertion_scopes
                .insert(format!("{block_address} {block_name}"));
        }
        let empty_members = Vec::new();
        let variables = member
            .get("members")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&empty_members);
        let mut symbol_ids = Vec::new();
        for variable in variables {
            let member_kind = string_field(variable, "kind");
            if !matches!(member_kind.as_deref(), Some("Variable") | Some("Parameter")) {
                continue;
            }
            let name = declaration_name(variable, "local Variable")?;
            let lifetime = string_field(variable, "lifetime");
            if member_kind.as_deref() == Some("Variable")
                && !matches!(lifetime.as_deref(), Some("Automatic") | Some("Static"))
            {
                return Err(format!(
                    "procedure-local variable '{name}' must be automatic"
                ));
            }
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
                .insert(symbol_id.clone(), variable.get("initializer").cloned());
            if member_kind.as_deref() == Some("Variable") && lifetime.as_deref() == Some("Static") {
                self.local_static_symbols.insert(symbol_id.clone());
            }
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
                .rsplit(['.', '#', ':'])
                .next()
                .unwrap_or_default();
            if self.type_enums.contains_key(target_name) {
                return Ok(());
            }
            let target_type = target.as_str().unwrap_or_default();
            if let Some(shape) = simple_type_shape(target_type) {
                let signed = simple_type_signed(target_type).ok_or_else(|| {
                    format!("direct lowering cannot resolve signedness of type alias '{name}'")
                })?;
                self.insert_type_alias(
                    name,
                    member,
                    AstStructType {
                        width: shape.width,
                        signed,
                        fields: std::collections::BTreeMap::new(),
                        is_union: false,
                        is_unpacked: false,
                        flat_pattern: true,
                    },
                    Some(target_type.to_string()),
                );
                return Ok(());
            }
            return Ok(());
        }
        let target_kind = string_field(target, "kind").unwrap_or_default();
        let is_union = matches!(
            target_kind.as_str(),
            "PackedUnionType" | "UnpackedUnionType"
        );
        let is_unpacked = matches!(
            target_kind.as_str(),
            "UnpackedStructType" | "UnpackedUnionType"
        );
        if matches!(
            target_kind.as_str(),
            "ScalarType" | "PackedArrayType" | "PredefinedIntegerType" | "StringType"
        ) {
            let target_type = if target_kind == "PackedArrayType" {
                String::new()
            } else {
                string_field(target, "name").unwrap_or_default()
            };
            let shape = if target_kind == "PackedArrayType" {
                None
            } else {
                simple_type_shape(&target_type)
            };
            let (width, signed) = match shape {
                Some(shape) => {
                    let signed = target
                        .get("isSigned")
                        .and_then(serde_json::Value::as_bool)
                        .map_or_else(|| simple_type_signed(&target_type), Some)
                        .ok_or_else(|| format!("type alias '{name}' has unknown signedness"))?;
                    (shape.width, signed)
                }
                None => (detailed_type_width(target)?, detailed_type_signed(target)),
            };
            self.insert_type_alias(
                name,
                member,
                AstStructType {
                    width,
                    signed,
                    fields: std::collections::BTreeMap::new(),
                    is_union: false,
                    is_unpacked: false,
                    flat_pattern: true,
                },
                Self::canonical_type_alias_target(target),
            );
            return Ok(());
        }
        if !matches!(
            target_kind.as_str(),
            "PackedStructType" | "PackedUnionType" | "UnpackedStructType" | "UnpackedUnionType"
        ) {
            return Ok(());
        }
        if bool_field(target, "isTagged") {
            return Err(format!(
                "direct slang-AST lowering does not support tagged union '{name}'"
            ));
        }
        let fields = target
            .get("members")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("packed aggregate type '{name}' has no fields"))?;
        let mut struct_fields = std::collections::BTreeMap::new();
        let mut width = 0_usize;
        let mut flat_pattern = true;
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
                let Ok(field_width) = detailed_type_width(field_type) else {
                    return Ok(());
                };
                field_width
            } else {
                let type_name = string_field(field, "type").unwrap_or_default();
                let Ok(shape) = self.net_type(&type_name) else {
                    return Ok(());
                };
                shape.0.width
            };
            let field_is_scalar_integral = if field_type.is_object() {
                matches!(
                    string_field(field_type, "kind").as_deref(),
                    Some("ScalarType" | "PredefinedIntegerType" | "PackedArrayType")
                ) && detailed_type_width(field_type).is_ok_and(|width| width <= 64)
            } else {
                let type_name = string_field(field, "type").unwrap_or_default();
                self.net_type(&type_name)
                    .is_ok_and(|_| simple_type_width(&type_name).is_some())
            };
            let nested_type = self.nested_struct_pattern_type(field_type);
            flat_pattern &= field_is_scalar_integral || nested_type.is_some();
            let field_signed = if field_type.is_object() {
                detailed_type_signed(field_type)
            } else {
                let type_name = string_field(field, "type").unwrap_or_default();
                self.net_type(&type_name)
                    .map(|(_, signed, _)| signed)
                    .unwrap_or_default()
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
                    signed: field_signed,
                    nested_type,
                },
            );
        }
        if is_union && !is_unpacked && struct_fields.values().any(|field| field.width != width) {
            return Err(format!(
                "packed union '{name}' members must all have the same width"
            ));
        }
        let definition = AstStructType {
            width,
            signed: bool_field(target, "isSigned"),
            fields: struct_fields,
            is_union,
            is_unpacked,
            flat_pattern,
        };
        let canonical_type_name = Self::canonical_type_alias_target(target);
        self.insert_type_alias(name, member, definition, canonical_type_name);
        Ok(())
    }

    pub(super) fn collect_type_parameter(
        &mut self,
        member: &serde_json::Value,
    ) -> Result<(), String> {
        let name = declaration_name(member, "TypeParameter")?;
        let target = member
            .get("type")
            .ok_or_else(|| format!("type parameter '{name}' has no resolved type"))?;
        let alias_member = serde_json::json!({
            "kind": "TypeAlias",
            "name": name,
            "addr": member.get("addr").cloned().unwrap_or(serde_json::Value::Null),
            "target": target,
        });
        self.collect_type_alias(&alias_member)
    }

    fn inline_struct_pattern_type(&self, target: &serde_json::Value) -> Option<AstStructType> {
        let target_kind = string_field(target, "kind")?;
        if !matches!(
            target_kind.as_str(),
            "PackedStructType" | "UnpackedStructType" | "PackedUnionType" | "UnpackedUnionType"
        ) || bool_field(target, "isTagged")
        {
            return None;
        }
        let fields = target.get("members")?.as_array()?;
        let mut struct_fields = std::collections::BTreeMap::new();
        let mut width = 0_usize;
        let mut flat_pattern = true;
        for field in fields {
            let field_name = string_field(field, "name")?;
            let offset = field
                .get("bitOffset")
                .and_then(serde_json::Value::as_u64)
                .and_then(|offset| usize::try_from(offset).ok())?;
            let field_index = field
                .get("fieldIndex")
                .and_then(serde_json::Value::as_u64)
                .and_then(|index| usize::try_from(index).ok())?;
            let field_type = field.get("type")?;
            let field_width = if field_type.is_object() {
                detailed_type_width(field_type).ok()?
            } else {
                let type_name = field_type.as_str()?;
                self.net_type(type_name).ok()?.0.width
            };
            let field_is_scalar_integral = if field_type.is_object() {
                matches!(
                    string_field(field_type, "kind").as_deref(),
                    Some("ScalarType" | "PredefinedIntegerType" | "PackedArrayType")
                ) && detailed_type_width(field_type).is_ok_and(|width| width <= 64)
            } else {
                let type_name = field_type.as_str()?;
                self.net_type(type_name)
                    .is_ok_and(|_| simple_type_width(type_name).is_some())
            };
            let nested_type = self.nested_struct_pattern_type(field_type);
            flat_pattern &= field_is_scalar_integral || nested_type.is_some();
            let field_signed = if field_type.is_object() {
                detailed_type_signed(field_type)
            } else {
                let type_name = field_type.as_str()?;
                self.net_type(type_name)
                    .map(|(_, signed, _)| signed)
                    .unwrap_or_default()
            };
            width = width.max(offset.saturating_add(field_width));
            struct_fields.insert(
                field_name.to_string(),
                AstStructField {
                    offset,
                    width: field_width,
                    field_index,
                    signed: field_signed,
                    nested_type,
                },
            );
        }
        let is_union = target_kind.ends_with("UnionType");
        if is_union && !target_kind.starts_with("Unpacked") {
            let widths_match = struct_fields.values().all(|field| field.width == width);
            if !widths_match {
                return None;
            }
        }
        Some(AstStructType {
            width,
            signed: bool_field(target, "isSigned"),
            fields: struct_fields,
            is_union,
            is_unpacked: target_kind == "UnpackedStructType",
            flat_pattern,
        })
    }

    fn nested_struct_pattern_type(
        &self,
        field_type: &serde_json::Value,
    ) -> Option<crate::lowering::AstNestedStruct> {
        if field_type.is_object() {
            let inline = self.inline_struct_pattern_type(field_type)?;
            return Some(crate::lowering::AstNestedStruct::Inline(Box::new(inline)));
        }
        let type_name = field_type.as_str()?;
        self.type_alias_definition(type_name)
            .filter(|definition| !definition.fields.is_empty() && definition.flat_pattern)
            .map(|_| crate::lowering::AstNestedStruct::Alias(type_name.to_string()))
    }

    pub(super) fn collect_enum_type(&mut self, member: &serde_json::Value) -> Result<(), String> {
        let name = declaration_name(member, "EnumType")?;
        let base = member
            .get("baseType")
            .ok_or_else(|| format!("enum type '{name}' has no base type"))?;
        let (width, signed) = if base.is_object() {
            (detailed_type_width(base)?, detailed_type_signed(base))
        } else {
            let base_type = base.as_str().unwrap_or_default();
            let shape = self
                .net_type(base_type)
                .map_err(|error| format!("enum type '{name}' base type: {error}"))?;
            (shape.0.width, shape.1)
        };
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
        if let Some(definition) = self.type_alias_definition(type_name) {
            return Ok((
                AstTypeShape {
                    width: definition.width,
                    bounds: None,
                    dimensions: Vec::<AstPackedDimension>::new(),
                },
                definition.signed,
                definition.fields.clone(),
            ));
        }
        let alias_name = type_name
            .split_whitespace()
            .next_back()
            .unwrap_or_default()
            .rsplit(['.', '#', ':'])
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
        Err(format!("direct lowering cannot resolve type '{type_name}'"))
    }

    fn aggregate_container_memory_type(&self, type_name: &str) -> Option<AstMemoryType> {
        let separator = type_name.find('$')?;
        let element_type = type_name[..separator].trim();
        let (shape, signed, _) = self.net_type(element_type).ok()?;
        let element_msb = i64::try_from(shape.width.saturating_sub(1)).ok()?;
        let bounds = shape.bounds.unwrap_or((element_msb, 0));
        let dimension = type_name[separator + 1..].trim();

        let (capacity, queue, associative, index_width, index_signed) = if dimension == "[]" {
            (256_usize, false, false, 0_usize, false)
        } else if dimension == "queue[$]" {
            (256_usize, true, false, 0_usize, false)
        } else if let Some(bound) = dimension.strip_prefix("queue[$:") {
            let bound = bound.strip_suffix(']')?.parse::<usize>().ok()?;
            (bound.checked_add(1)?, true, false, 0_usize, false)
        } else if let Some(index_type) = dimension.strip_prefix("assoc[") {
            let index_type = index_type.strip_suffix(']')?.trim();
            let index_width = simple_type_width(index_type)?;
            let index_signed = simple_type_signed(index_type)?;
            (256_usize, false, true, index_width, index_signed)
        } else {
            return None;
        };

        let capacity_msb = i64::try_from(capacity.checked_sub(1)?).ok()?;
        let fixed_type_name = format!("{element_type}$[0:{capacity_msb}]");
        let mut memory = memory_type_with_element(&fixed_type_name, shape.width, bounds, signed)?;
        memory.dynamic = true;
        memory.queue = queue;
        memory.queue_bound = if queue && dimension != "queue[$]" {
            capacity.checked_sub(1)
        } else {
            None
        };
        memory.associative = associative;
        memory.index_width = index_width;
        memory.index_signed = index_signed;
        Some(memory)
    }

    pub(super) fn declared_memory_type(&self, type_name: &str) -> Option<AstMemoryType> {
        if let Some(memory) = associative_memory_type(type_name) {
            return Some(memory);
        }
        if let Some(memory) = queue_memory_type(type_name) {
            return Some(memory);
        }
        if let Some(memory) = dynamic_memory_type(type_name) {
            return Some(memory);
        }
        if let Some(memory) = memory_type(type_name) {
            return Some(memory);
        }
        if let Some(memory) = self.aggregate_container_memory_type(type_name) {
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
        if string_field(member, "kind").as_deref() == Some("InterfacePort") {
            return Ok(());
        }
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
        if self
            .declared_memory_type(&string_field(member, "type").unwrap_or_default())
            .is_some_and(|memory| memory.dynamic)
        {
            return Err(format!(
                "dynamic-array ports are not supported for '{name}'"
            ));
        }
        let memory_direction = match string_field(member, "direction").as_deref() {
            Some("In") => eda_core::ir::MemoryPortDirection::In,
            Some("Out") => eda_core::ir::MemoryPortDirection::Out,
            Some("InOut") => eda_core::ir::MemoryPortDirection::InOut,
            other => {
                return Err(format!(
                    "sparse-memory port '{name}' has unsupported direction {other:?}"
                ))
            }
        };
        self.lower_memory_declaration(member, name)?;
        let symbol = self.symbols[name].clone();
        self.design.memory_ports.push(eda_core::ir::MemoryPortDecl {
            name: name.to_string(),
            direction: memory_direction,
            memory: symbol.memory.ok_or_else(|| {
                format!("sparse-memory port '{name}' does not reference a memory object")
            })?,
        });
        // The placeholder PortDecl keeps scalar port inventory symmetric; the
        // first-class storage contract is represented by MemoryPortDecl.
        let direction = match memory_direction {
            eda_core::ir::MemoryPortDirection::In | eda_core::ir::MemoryPortDirection::InOut => {
                eda_core::ir::Dir::In
            }
            eda_core::ir::MemoryPortDirection::Out => eda_core::ir::Dir::Out,
        };
        self.design.ports.push(eda_core::ir::PortDecl {
            name: name.to_string(),
            dir: direction,
            base: symbol.base,
            width: 1,
        });
        Ok(())
    }

    pub(super) fn lower_declaration(&mut self, member: &serde_json::Value) -> Result<(), String> {
        let kind = string_field(member, "kind").unwrap_or_default();
        let name = declaration_name(member, &kind)?;
        let type_name = string_field(member, "type").unwrap_or_default();
        if kind == "Variable" && self.manual_covergroups.contains_key(&type_name) {
            if member.get("initializer").is_some() {
                return Err(format!(
                    "covergroup instance '{name}' does not support an initializer"
                ));
            }
            let address = member
                .get("addr")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| format!("covergroup instance '{name}' has no symbol address"))?;
            self.covergroup_instances
                .insert(format!("{address} {name}"), type_name);
            return Ok(());
        }
        if type_is_process_handle(&type_name) && kind == "Variable" {
            let mut integral = member.clone();
            if let Some(fields) = integral.as_object_mut() {
                fields.insert(
                    "type".into(),
                    serde_json::Value::String("logic[63:0]".into()),
                );
            }
            self.declare_net(&integral, &name, &kind)?;
            if let Some(initializer) = member.get("initializer") {
                let rhs = self.lower_real_assignment_expression(member, initializer)?;
                self.push_assignment(name, rhs);
            }
            return Ok(());
        }
        if type_name.contains("uvm_sequencer") {
            self.declare_uvm_sequencer(member, &name)?;
            return Ok(());
        }
        if type_name.contains("uvm_tlm_fifo") {
            self.declare_uvm_tlm_fifo(member, &name)?;
            return Ok(());
        }
        if type_name.contains("uvm_put_port")
            || type_name.contains("uvm_get_peek_port")
            || type_name.contains("uvm_put_export")
            || type_name.contains("uvm_get_peek_export")
        {
            self.declare_uvm_tlm_port(member, &name)?;
            return Ok(());
        }
        if type_name.contains("uvm_analysis_port") || type_name.contains("uvm_analysis_export") {
            let handle_address = member
                .get("addr")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| format!("UVM analysis port '{name}' has no symbol address"))?;
            let object_key = format!("{handle_address} {name}");
            let endpoint_class = if type_name.contains("uvm_analysis_export") {
                "uvm_analysis_export"
            } else {
                "uvm_analysis_port"
            };
            self.class_objects
                .insert(object_key.clone(), std::collections::BTreeMap::new());
            self.class_object_types
                .insert(object_key, endpoint_class.to_string());
            return Ok(());
        }
        if self.declare_class_object(member, &name)? {
            return Ok(());
        }
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
            let rhs = self.lower_real_assignment_expression(member, initializer)?;
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
            dynamic: memory.dynamic,
            queue: memory.queue,
            queue_bound: memory.queue_bound,
            associative: memory.associative,
            index_width: memory.index_width,
            index_signed: memory.index_signed,
            declaration_initialized: initializer.is_some(),
            storage: eda_core::ir::MemoryStorage::Ordinary,
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
            } else if kind == "Net" {
                // Slang elaborates an ANSI port such as `input a` into a Port
                // symbol followed by its internal Net symbol. The companion Net
                // proves that storage is net storage, even when the data type
                // is implicit `logic`; do not leave the provisional port mark
                // as procedural storage metadata.
                if let Some(declaration) = self
                    .design
                    .net_decls
                    .iter_mut()
                    .find(|declaration| declaration.name == name)
                {
                    declaration.is_reg = false;
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
        let shape = match simple_type_shape(&type_name) {
            Some(shape) => shape,
            None => self.net_type(&type_name)?.0,
        };
        let value = string_field(member, "value")
            .ok_or_else(|| format!("parameter '{name}' has no elaborated constant value"))?;
        let value_type = if simple_type_width(&type_name).is_some() {
            type_name.clone()
        } else {
            format!("logic[{}:0]", shape.width.saturating_sub(1))
        };
        if value == "$" {
            self.unbounded_parameters.insert(name);
            return Ok(());
        }
        let word = if type_name == "string" {
            let literal = value.trim().trim_matches('"');
            string_literal_word(literal)
        } else {
            literal_word(&value_type, &value)
        }
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

fn type_is_process_handle(type_name: &str) -> bool {
    type_name.split_whitespace().next_back() == Some("process")
}

fn detailed_type_width(type_value: &serde_json::Value) -> Result<usize, String> {
    match string_field(type_value, "kind").as_deref() {
        Some("ScalarType") => Ok(1),
        Some("StringType") => Ok(STRING_WIDTH),
        Some("FloatingType") => {
            let type_name = string_field(type_value, "name").unwrap_or_default();
            match type_name.trim() {
                "real" | "realtime" => Ok(64),
                "shortreal" => Ok(32),
                other => Err(format!("unsupported floating type '{other}'")),
            }
        }
        Some("PredefinedIntegerType") => {
            let type_name = string_field(type_value, "name").unwrap_or_default();
            simple_type_width(&type_name)
                .ok_or_else(|| format!("unsupported predefined integer type '{type_name}'"))
        }
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
    if let Some(is_signed) = type_value
        .get("isSigned")
        .and_then(serde_json::Value::as_bool)
    {
        return is_signed;
    }
    match string_field(type_value, "kind").as_deref() {
        Some("PackedArrayType") => type_value
            .get("elementType")
            .map(detailed_type_signed)
            .unwrap_or(false),
        Some("PredefinedIntegerType") => !matches!(
            string_field(type_value, "name").as_deref(),
            Some("byte unsigned" | "shortint unsigned" | "int unsigned" | "longint unsigned")
        ),
        Some("FloatingType") => true,
        _ => bool_field(type_value, "isSigned"),
    }
}

fn collect_plusarg_test_calls<'value>(
    value: &'value serde_json::Value,
    calls: &mut Vec<&'value serde_json::Value>,
) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("Call")
            && string_field(value, "subroutine").as_deref() == Some("$test$plusargs")
        {
            calls.push(value);
        }
        for nested in fields.values() {
            collect_plusarg_test_calls(nested, calls);
        }
        return;
    }
    if let Some(values) = value.as_array() {
        for nested in values {
            collect_plusarg_test_calls(nested, calls);
        }
    }
}

fn collect_plusarg_value_calls<'value>(
    value: &'value serde_json::Value,
    calls: &mut Vec<&'value serde_json::Value>,
) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("Call")
            && string_field(value, "subroutine").as_deref() == Some("$value$plusargs")
        {
            calls.push(value);
        }
        for nested in fields.values() {
            collect_plusarg_value_calls(nested, calls);
        }
        return;
    }
    if let Some(values) = value.as_array() {
        for nested in values {
            collect_plusarg_value_calls(nested, calls);
        }
    }
}

pub(super) struct PlusargValue<'a> {
    pub(super) query: String,
    pub(super) specifier: char,
    pub(super) target: &'a serde_json::Value,
}

pub(super) fn value_plusarg_plan(call: &serde_json::Value) -> Result<PlusargValue<'_>, String> {
    let arguments = call
        .get("arguments")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "$value$plusargs call has no arguments".to_string())?;
    if arguments.len() != 2 {
        return Err("$value$plusargs requires format and output arguments".into());
    }
    let format = string_literal(&arguments[0])
        .ok_or_else(|| "$value$plusargs format must be a string literal".to_string())?;
    let Some(percent) = format.find('%') else {
        return Err("$value$plusargs format requires one conversion".into());
    };
    let mut characters = format[percent + 1..].chars();
    let Some(specifier) = characters.next() else {
        return Err("$value$plusargs format has a trailing '%'".into());
    };
    if characters.next().is_some() {
        return Err("$value$plusargs supports exactly one conversion".into());
    }
    if !matches!(specifier, 'd' | 'h' | 'o' | 'b' | 's') {
        return Err(format!(
            "$value$plusargs does not support conversion '%{specifier}'"
        ));
    }
    let prefix = &format[..percent];
    let Some(query) = prefix.strip_suffix('=') else {
        return Err("$value$plusargs format must be NAME=<conversion>".into());
    };
    if query.is_empty() {
        return Err("$value$plusargs query name cannot be empty".into());
    }
    let target = arguments[1]
        .get("left")
        .ok_or_else(|| "$value$plusargs output argument has no target".to_string())?;
    match arguments[1].get("right") {
        Some(right) if string_field(right, "kind").as_deref() == Some("EmptyArgument") => {}
        _ => {
            return Err(
                "$value$plusargs output argument must be an empty assignment target".into(),
            );
        }
    }
    Ok(PlusargValue {
        query: query.to_string(),
        specifier,
        target,
    })
}
