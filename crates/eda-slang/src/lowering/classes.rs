//! Direct lowering for a small class-object subset with explicit dynamic
//! handle-alias metadata.

use super::{declaration_name, SlangAstLowerer};
use crate::ast_util::{bool_field, string_field};
use eda_core::ir::{BinOp, Expr, Stmt};
use eda_core::logic::LogicWord;
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone)]
struct ClassMethodCandidate {
    class_name: String,
    method: Value,
    owner: String,
}

impl SlangAstLowerer {
    pub(super) fn collect_class_static_members(&mut self) -> Result<(), String> {
        let definitions = self.class_definitions.clone();
        let mut initializers = Vec::new();
        let mut methods = Vec::new();
        for (class_name, class) in &definitions {
            let empty_members = Vec::new();
            let members = class
                .get("members")
                .and_then(Value::as_array)
                .unwrap_or(&empty_members);
            for member in members {
                match string_field(member, "kind").as_deref() {
                    Some("ClassProperty")
                        if string_field(member, "lifetime").as_deref() == Some("Static") =>
                    {
                        let property_name = declaration_name(member, "static ClassProperty")
                            .map_err(|error| super::with_source_span(member, error))?;
                        let symbol_key = member
                            .get("addr")
                            .and_then(Value::as_u64)
                            .map(|address| format!("{address} {property_name}"))
                            .ok_or_else(|| {
                                super::with_source_span(
                                    member,
                                    format!("static class property '{class_name}.{property_name}' has no address"),
                                )
                            })?;
                        if self.symbols.contains_key(&symbol_key) {
                            continue;
                        }
                        let property_type = string_field(member, "type").unwrap_or_default();
                        let (shape, signed, struct_fields) = self
                            .net_type(&property_type)
                            .map_err(|error| super::with_source_span(member, error))?;
                        let base = self.design.n_nets;
                        self.design.n_nets += shape.width;
                        self.design.net_decls.push(eda_core::ir::NetDecl {
                            name: format!("__class_static_{class_name}_{property_name}"),
                            width: shape.width,
                            is_reg: true,
                            is_signed: signed,
                            base,
                        });
                        let symbol = super::AstLoweredSymbol {
                            base,
                            width: shape.width,
                            signed,
                            bounds: shape.bounds,
                            dimensions: shape.dimensions,
                            memory: None,
                            struct_fields,
                        };
                        self.symbols.insert(symbol_key.clone(), symbol);
                        if let Some(initializer) = member.get("initializer") {
                            initializers.push((symbol_key, initializer.clone()));
                        }
                    }
                    Some("Subroutine") if method_is_static(member) => {
                        methods.push((class_name.clone(), member.clone()));
                    }
                    _ => {}
                }
            }
        }

        for (symbol_key, initializer) in initializers {
            let symbol = self.symbols[&symbol_key].clone();
            let expression = self
                .lower_expression(&initializer)
                .map_err(|error| super::with_source_span(&initializer, error))?;
            self.design.initials.push(Stmt::Bl {
                lv: class_lvalue(&symbol),
                e: expression,
            });
        }

        for (class_name, method) in methods {
            let method_name = declaration_name(&method, "static class Subroutine")
                .map_err(|error| super::with_source_span(&method, error))?;
            let address = method.get("addr").and_then(Value::as_u64).ok_or_else(|| {
                super::with_source_span(
                    &method,
                    format!("static class method '{class_name}.{method_name}' has no address"),
                )
            })?;
            let original_key = format!("{address} {method_name}");
            let synthetic_key = format!("eda_class_static_{address}_{method_name}");
            if let Some(existing) = self.subroutines.get(&synthetic_key) {
                if *existing != method {
                    return Err(super::with_source_span(
                        &method,
                        format!("duplicate static class method '{class_name}.{method_name}'"),
                    ));
                }
            } else {
                self.subroutines
                    .insert(synthetic_key.clone(), method.clone());
            }
            if method_return_type(&method) == "void" {
                let mut task_form = method.clone();
                if let Some(fields) = task_form.as_object_mut() {
                    fields.insert("subroutineKind".into(), Value::String("Task".into()));
                }
                let task_key = format!("{synthetic_key}_task");
                self.subroutines.insert(task_key, task_form);
            }
            self.class_static_methods
                .insert(original_key, synthetic_key);
        }
        Ok(())
    }

    pub(super) fn validate_dynamic_class_sources(&self) -> Result<(), String> {
        let mut reachable = self.constructed_class_objects.clone();
        let mut queue = reachable
            .iter()
            .cloned()
            .collect::<std::collections::VecDeque<_>>();
        while let Some(source) = queue.pop_front() {
            for (target, sources) in &self.dynamic_class_sources {
                if sources
                    .iter()
                    .any(|candidate| candidate.object_key == source)
                    && reachable.insert(target.clone())
                {
                    queue.push_back(target.clone());
                }
            }
        }
        for target in self.dynamic_class_sources.keys() {
            if !reachable.contains(target) {
                return Err(format!(
                    "class handle '{target}' can be assigned from a null object; null handle propagation is not supported"
                ));
            }
        }
        Ok(())
    }

    pub(super) fn collect_dynamic_class_sources(&mut self, value: &Value) -> Result<(), String> {
        if let Some(fields) = value.as_object() {
            if string_field(value, "kind").as_deref() == Some("Assignment") {
                self.record_dynamic_class_assignment(value)?;
            }
            for nested in fields.values() {
                self.collect_dynamic_class_sources(nested)?;
            }
        } else if let Some(values) = value.as_array() {
            for nested in values {
                self.collect_dynamic_class_sources(nested)?;
            }
        }
        Ok(())
    }

    fn record_dynamic_class_assignment(&mut self, assignment: &Value) -> Result<(), String> {
        let Some(left) = assignment.get("left") else {
            return Ok(());
        };
        let Some(right) = assignment.get("right") else {
            return Ok(());
        };
        let Some((target_key, target_class)) = named_class_handle(left) else {
            return Ok(());
        };
        if string_field(right, "kind").as_deref() == Some("NewClass") {
            self.initialized_class_objects.insert(target_key.clone());
            self.constructed_class_objects.insert(target_key);
            return Ok(());
        }
        let source = conversion_operand(right).unwrap_or(right);
        let Some((source_key, source_class)) = named_class_handle(source) else {
            return Ok(());
        };
        if !self.class_definitions.contains_key(&target_class)
            || !self.class_definitions.contains_key(&source_class)
        {
            return Ok(());
        }
        let chain = self.class_chain(&source_class)?;
        if !chain
            .iter()
            .any(|(class_name, _)| *class_name == target_class)
        {
            return Ok(());
        }
        self.initialized_class_objects.insert(target_key.clone());
        let sources = self.dynamic_class_sources.entry(target_key).or_default();
        if !sources.iter().any(|source| source.object_key == source_key) {
            sources.push(super::DynamicClassSource {
                object_key: source_key,
                class_name: source_class,
            });
        }
        Ok(())
    }

    pub(super) fn declare_class_object(
        &mut self,
        member: &Value,
        name: &str,
    ) -> Result<bool, String> {
        let type_name = string_field(member, "type").unwrap_or_default();
        let Some(class_name) = class_name(&type_name) else {
            return Ok(false);
        };
        if !self.class_definitions.contains_key(&class_name) {
            return Ok(false);
        }
        let Some(class) = self.class_definitions.get(&class_name).cloned() else {
            return Ok(false);
        };
        if bool_field(&class, "isAbstract")
            || bool_field(&class, "isInterface")
            || bool_field(&class, "isFinal")
        {
            return Err(format!("class '{class_name}' qualifiers are not supported"));
        }
        if class
            .get("implements")
            .and_then(Value::as_array)
            .is_some_and(|interfaces| !interfaces.is_empty())
        {
            return Err(format!("class '{class_name}' interfaces are not supported"));
        }
        let chain = self.class_chain(&class_name)?;
        let empty_members = Vec::new();
        let mut declared_properties = BTreeMap::new();
        for (owner_name, class) in &chain {
            let members = class
                .get("members")
                .and_then(Value::as_array)
                .unwrap_or(&empty_members);
            for property in members {
                if string_field(property, "kind").as_deref() != Some("ClassProperty") {
                    continue;
                }
                let property_name = declaration_name(property, "ClassProperty")?;
                if let Some(previous_owner) =
                    declared_properties.insert(property_name.clone(), owner_name.clone())
                {
                    return Err(format!(
                        "class property shadowing '{class_name}.{property_name}' from '{previous_owner}' is not supported"
                    ));
                }
            }
        }
        let handle_address = member
            .get("addr")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("class object '{name}' has no symbol address"))?;
        self.ensure_class_object_serial()?;
        let empty_members = Vec::new();
        let mut fields: BTreeMap<String, super::AstLoweredSymbol> = BTreeMap::new();
        let mut property_chains = vec![chain.clone()];
        let object_key = format!("{handle_address} {name}");
        if let Some(sources) = self.dynamic_class_sources.get(&object_key) {
            for source in sources {
                property_chains.push(self.class_chain(&source.class_name)?);
            }
        }
        for property_chain in &property_chains {
            for (_, class) in property_chain.iter().rev() {
                let members = class
                    .get("members")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty_members);
                for property in members {
                    if string_field(property, "kind").as_deref() != Some("ClassProperty") {
                        continue;
                    }
                    if string_field(property, "lifetime").as_deref() == Some("Static") {
                        continue;
                    }
                    let property_name = declaration_name(property, "ClassProperty")?;
                    if property.get("initializer").is_some() {
                        return Err(format!(
                            "class property '{class_name}.{property_name}' initializers are not supported; initialize in new()"
                        ));
                    }
                    let property_address = property
                        .get("addr")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| {
                            format!("class property '{class_name}.{property_name}' has no address")
                        })?;
                    let property_type = string_field(property, "type").unwrap_or_default();
                    let (shape, signed, _) = self.net_type(&property_type)?;
                    let synthetic_symbol = format!("eda_class_{handle_address}_{property_address}");
                    if let Some(existing) = fields.get(&property_name) {
                        if existing.width != shape.width
                            || existing.signed != signed
                            || existing.bounds != shape.bounds
                        {
                            return Err(format!(
                                "dynamic class storage for '{name}.{property_name}' has conflicting layouts"
                            ));
                        }
                        self.symbols.insert(synthetic_symbol, existing.clone());
                        continue;
                    }
                    let field_name = format!("{name}.{property_name}");
                    let symbol = self.declare_net(property, &field_name, "Variable")?;
                    self.symbols.insert(synthetic_symbol, symbol.clone());
                    self.class_property_owners
                        .insert(symbol.base, (object_key.clone(), property_name.clone()));
                    let mut temporary = self.declare_class_internal_net(
                        &format!("__class_tmp_{handle_address}_{property_address}"),
                        shape.width,
                    );
                    temporary.signed = signed;
                    self.class_property_temps
                        .insert((object_key.clone(), property_name.clone()), temporary);
                    fields.insert(property_name, symbol);
                }
            }
        }
        self.class_objects.insert(object_key.clone(), fields);
        self.class_object_types
            .insert(object_key.clone(), class_name);
        let tag =
            self.declare_class_internal_net(&format!("__class_tag_{handle_address}_{name}"), 32);
        let id = self
            .declare_class_internal_net(&format!("__class_object_id_{handle_address}_{name}"), 64);
        self.class_object_tags.insert(object_key.clone(), tag);
        self.class_object_ids.insert(object_key, id);
        Ok(true)
    }

    fn ensure_class_object_serial(&mut self) -> Result<(), String> {
        if self.class_object_serial.is_some() {
            return Ok(());
        }
        let symbol = self.declare_class_internal_net("__class_object_serial", 64);
        self.design.initials.push(Stmt::Bl {
            lv: class_lvalue(&symbol),
            e: Expr::Const(LogicWord::from_u64(64, 1)),
        });
        self.class_object_serial = Some(symbol);
        Ok(())
    }

    fn declare_class_internal_net(&mut self, name: &str, width: usize) -> super::AstLoweredSymbol {
        let base = self.design.n_nets;
        self.design.n_nets += width;
        self.design.net_decls.push(eda_core::ir::NetDecl {
            name: name.to_string(),
            width,
            is_reg: true,
            is_signed: false,
            base,
        });
        super::AstLoweredSymbol {
            base,
            width,
            signed: false,
            bounds: None,
            dimensions: Vec::new(),
            memory: None,
            struct_fields: BTreeMap::new(),
        }
    }

    pub(super) fn lower_class_constructor(
        &self,
        assignment: &Value,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        let Some(right) = assignment.get("right") else {
            return Ok(None);
        };
        if string_field(right, "kind").as_deref() != Some("NewClass") {
            return Ok(None);
        }
        let Some(left) = assignment.get("left") else {
            return Ok(None);
        };
        let Some(object_key) = left.get("symbol").and_then(Value::as_str) else {
            return Ok(None);
        };
        let constructor = right
            .get("constructorCall")
            .ok_or_else(|| "class construction has no constructor call".to_string())?;
        let class_name = self
            .class_object_types
            .get(object_key)
            .ok_or_else(|| format!("unknown class object '{object_key}'"))?;
        let class_id = self.class_id(class_name)?;
        let tag = self
            .class_object_tags
            .get(object_key)
            .ok_or_else(|| format!("class object '{object_key}' has no runtime tag"))?
            .clone();
        let id = self
            .class_object_ids
            .get(object_key)
            .ok_or_else(|| format!("class object '{object_key}' has no object id"))?
            .clone();
        let serial = self
            .class_object_serial
            .clone()
            .ok_or_else(|| "class object serial counter is missing".to_string())?;
        let body = self.lower_class_method(object_key, constructor, true)?;
        Ok(Some(Stmt::Block(vec![
            Stmt::Bl {
                lv: class_lvalue(&tag),
                e: Expr::Const(LogicWord::from_u64(32, class_id)),
            },
            Stmt::Bl {
                lv: class_lvalue(&id),
                e: class_expr(&serial),
            },
            Stmt::Bl {
                lv: class_lvalue(&serial),
                e: Expr::Bin {
                    op: BinOp::Add,
                    a: Box::new(class_expr(&serial)),
                    b: Box::new(Expr::Const(LogicWord::from_u64(64, 1))),
                },
            },
            body,
        ])))
    }

    pub(super) fn lower_class_task_call(
        &self,
        call: &Value,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        let Some(receiver) = call.get("thisClass") else {
            return Ok(None);
        };
        let Some(object_key) = receiver.get("symbol").and_then(Value::as_str) else {
            return Ok(None);
        };
        if !self.is_class_object_key(object_key) {
            return Ok(None);
        }
        self.require_initialized_class_object(object_key)?;
        if let Some(statement) = self.lower_dynamic_class_task(object_key, call)? {
            return Ok(Some(statement));
        }
        self.lower_class_method(object_key, call, false).map(Some)
    }

    pub(super) fn lower_static_class_function_call(
        &self,
        value: &Value,
    ) -> Result<Option<eda_core::ir::Expr>, String> {
        let Some(subroutine) = string_field(value, "subroutine") else {
            return Ok(None);
        };
        let Some(synthetic) = self.class_static_methods.get(&subroutine) else {
            return Ok(None);
        };
        let mut rewritten = value.clone();
        if let Some(fields) = rewritten.as_object_mut() {
            fields.insert("subroutine".into(), Value::String(synthetic.clone()));
        }
        self.lower_function_call(&rewritten).map(Some)
    }

    pub(super) fn lower_static_class_task_call(
        &self,
        value: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(subroutine) = string_field(value, "subroutine") else {
            return Ok(None);
        };
        let Some(function_key) = self.class_static_methods.get(&subroutine) else {
            return Ok(None);
        };
        let synthetic = if self
            .subroutines
            .get(function_key)
            .and_then(|method| string_field(method, "subroutineKind"))
            .as_deref()
            == Some("Task")
        {
            function_key.clone()
        } else {
            format!("{function_key}_task")
        };
        let mut rewritten = value.clone();
        if let Some(fields) = rewritten.as_object_mut() {
            fields.insert("subroutine".into(), Value::String(synthetic));
        }
        self.lower_task_call(&rewritten)
    }

    pub(super) fn lower_class_function_call(
        &self,
        call: &Value,
    ) -> Result<Option<eda_core::ir::Expr>, String> {
        let Some(receiver) = call.get("thisClass") else {
            return Ok(None);
        };
        let Some(object_key) = receiver.get("symbol").and_then(Value::as_str) else {
            return Ok(None);
        };
        if !self.is_class_object_key(object_key) {
            return Ok(None);
        }
        self.require_initialized_class_object(object_key)?;
        let Some((method, owner)) = self.class_method(object_key, call)? else {
            return Ok(None);
        };
        if method_return_type(&method) == "void" {
            return Ok(None);
        }
        if let Some(expression) = self.lower_dynamic_class_function(object_key, call)? {
            return Ok(Some(expression));
        }
        let body = self.specialized_method_body(object_key, &owner, &method, call)?;
        if string_field(&body, "kind").as_deref() != Some("Return") {
            return Err("class function must have a single direct return expression".into());
        }
        let expression = body
            .get("expr")
            .ok_or_else(|| "class function return has no expression".to_string())?;
        self.lower_expression(expression).map(Some)
    }

    pub(super) fn lower_class_handle_assignment(
        &self,
        assignment: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(left) = assignment.get("left") else {
            return Ok(None);
        };
        let Some(right) = assignment.get("right") else {
            return Ok(None);
        };
        let Some((target_key, target_class)) = named_class_handle(left) else {
            return Ok(None);
        };
        if !self.class_definitions.contains_key(&target_class) {
            return Ok(None);
        }
        let source = conversion_operand(right).unwrap_or(right);
        let Some((source_key, _)) = named_class_handle(source) else {
            return Err(
                "class handle assignment supports constructor and object-handle sources only"
                    .into(),
            );
        };
        if !self.class_objects.contains_key(&target_key)
            || !self.class_objects.contains_key(&source_key)
        {
            return Ok(None);
        }
        if bool_field(assignment, "isNonBlocking") {
            return Err("non-blocking class handle assignment is not supported".into());
        }
        let target_fields = &self.class_objects[&target_key];
        let source_fields = &self.class_objects[&source_key];
        let mut statements = Vec::new();
        for (target_symbol, source_symbol) in [
            (
                &self.class_object_tags[&target_key],
                &self.class_object_tags[&source_key],
            ),
            (
                &self.class_object_ids[&target_key],
                &self.class_object_ids[&source_key],
            ),
        ] {
            statements.push(Stmt::Bl {
                lv: class_lvalue(target_symbol),
                e: class_expr(source_symbol),
            });
        }
        for (field_name, target_symbol) in target_fields {
            let Some(source_symbol) = source_fields.get(field_name) else {
                continue;
            };
            statements.push(Stmt::Bl {
                lv: class_lvalue(target_symbol),
                e: class_expr(source_symbol),
            });
        }
        Ok(Some(Stmt::Block(statements)))
    }

    pub(super) fn lower_class_property_assignment(
        &self,
        assignment: &Value,
        left: &Value,
        right: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some((object_key, field_name, symbol)) = self.class_property_target(left) else {
            return Ok(None);
        };
        self.require_initialized_class_object(&object_key)?;
        let expression = self.lower_expression(right)?;
        let nonblocking = bool_field(assignment, "isNonBlocking");
        let temporary = self
            .class_property_temps
            .get(&(object_key.clone(), field_name.clone()))
            .ok_or_else(|| format!("class property '{object_key}.{field_name}' has no temporary"))?
            .clone();
        let sampled_expression = class_expr(&temporary);
        let mut statements = vec![Stmt::Bl {
            lv: class_lvalue(&temporary),
            e: expression,
        }];
        statements.push(class_field_assignment(
            &symbol,
            &sampled_expression,
            nonblocking,
        ));
        for related_key in self.related_class_handles(&object_key) {
            let Some(related_fields) = self.class_objects.get(&related_key) else {
                continue;
            };
            let Some(related_symbol) = related_fields.get(&field_name) else {
                continue;
            };
            let Some(related_id) = self.class_object_ids.get(&related_key) else {
                continue;
            };
            let object_id = &self.class_object_ids[&object_key];
            let object_tag = &self.class_object_tags[&object_key];
            let related_tag = &self.class_object_tags[&related_key];
            let condition = Expr::Bin {
                op: BinOp::LogAnd,
                a: Box::new(Expr::Bin {
                    op: BinOp::LogAnd,
                    a: Box::new(Expr::Bin {
                        op: BinOp::Ne,
                        a: Box::new(class_expr(object_id)),
                        b: Box::new(Expr::Const(LogicWord::zero(object_id.width))),
                    }),
                    b: Box::new(Expr::Bin {
                        op: BinOp::Eq,
                        a: Box::new(class_expr(object_id)),
                        b: Box::new(class_expr(related_id)),
                    }),
                }),
                b: Box::new(Expr::Bin {
                    op: BinOp::Eq,
                    a: Box::new(class_expr(object_tag)),
                    b: Box::new(class_expr(related_tag)),
                }),
            };
            statements.push(Stmt::If {
                c: condition,
                t: Box::new(class_field_assignment(
                    related_symbol,
                    &sampled_expression,
                    nonblocking,
                )),
                f: None,
            });
        }
        Ok(Some(Stmt::Block(statements)))
    }

    fn class_property_target(
        &self,
        value: &Value,
    ) -> Option<(String, String, super::AstLoweredSymbol)> {
        if string_field(value, "kind").as_deref() == Some("NamedValue") {
            let symbol_name = string_field(value, "symbol")?;
            if !symbol_name.starts_with("eda_class_") {
                return None;
            }
            let symbol = self.symbols.get(&symbol_name)?.clone();
            let (object_key, field_name) = self.class_property_owners.get(&symbol.base)?.clone();
            return Some((object_key, field_name, symbol));
        }
        if string_field(value, "kind").as_deref() != Some("MemberAccess") {
            return None;
        }
        let receiver = value.get("value")?;
        let object_key = receiver.get("symbol").and_then(Value::as_str)?;
        self.require_class_object_key(object_key)?;
        let field_name = string_field(value, "member")
            .and_then(|member| member.split_whitespace().next_back().map(str::to_string))?;
        let symbol = self
            .class_objects
            .get(object_key)?
            .get(&field_name)?
            .clone();
        Some((object_key.to_string(), field_name, symbol))
    }

    fn related_class_handles(&self, object_key: &str) -> Vec<String> {
        let mut visited = std::collections::BTreeSet::from([object_key.to_string()]);
        let mut queue = std::collections::VecDeque::from([object_key.to_string()]);
        while let Some(current) = queue.pop_front() {
            let mut neighbors = Vec::new();
            if let Some(sources) = self.dynamic_class_sources.get(&current) {
                neighbors.extend(sources.iter().map(|source| source.object_key.clone()));
            }
            for (candidate, sources) in &self.dynamic_class_sources {
                if sources.iter().any(|source| source.object_key == current) {
                    neighbors.push(candidate.clone());
                }
            }
            for neighbor in neighbors {
                if !visited.contains(&neighbor) {
                    visited.insert(neighbor.clone());
                    queue.push_back(neighbor);
                }
            }
        }
        visited.remove(object_key);
        visited.into_iter().collect()
    }

    fn require_initialized_class_object(&self, object_key: &str) -> Result<(), String> {
        if self.initialized_class_objects.contains(object_key) {
            Ok(())
        } else {
            Err(format!(
                "class object '{object_key}' may be null; null object access is not supported"
            ))
        }
    }

    fn require_class_object_key(&self, object_key: &str) -> Option<()> {
        let class_name = self.class_object_types.get(object_key)?;
        self.class_definitions
            .contains_key(class_name)
            .then_some(())
    }

    fn is_class_object_key(&self, object_key: &str) -> bool {
        self.require_class_object_key(object_key).is_some()
    }

    pub(super) fn lower_class_property_access(
        &self,
        value: &Value,
    ) -> Result<Option<eda_core::ir::Expr>, String> {
        if string_field(value, "kind").as_deref() != Some("MemberAccess") {
            return Ok(None);
        }
        let Some(receiver) = value.get("value") else {
            return Ok(None);
        };
        let Some(object_key) = receiver.get("symbol").and_then(Value::as_str) else {
            return Ok(None);
        };
        if !self.is_class_object_key(object_key) {
            return Ok(None);
        }
        self.require_initialized_class_object(object_key)?;
        let Some(field_name) = string_field(value, "member")
            .and_then(|member| member.split_whitespace().next_back().map(str::to_string))
        else {
            return Ok(None);
        };
        let Some(fields) = self.class_objects.get(object_key) else {
            return Ok(None);
        };
        let Some(symbol) = fields.get(&field_name) else {
            return Ok(None);
        };
        Ok(Some(eda_core::ir::Expr::Ref {
            net: symbol.base,
            decl_width: symbol.width,
            range: super::hierarchy::full_range(symbol.width),
            signed: symbol.signed,
        }))
    }

    fn lower_class_method(
        &self,
        object_key: &str,
        call: &Value,
        is_constructor: bool,
    ) -> Result<eda_core::ir::Stmt, String> {
        let Some((method, owner)) = self.class_method(object_key, call)? else {
            return Err("class method is missing".into());
        };
        if !is_constructor && method_return_type(&method) != "void" {
            return Err("non-void class methods must be used as expressions".into());
        }
        let body = self.specialized_method_body(object_key, &owner, &method, call)?;
        self.lower_statement(&body)
    }

    fn class_method(
        &self,
        object_key: &str,
        call: &Value,
    ) -> Result<Option<(Value, String)>, String> {
        let Some(receiver_type) = self
            .class_objects
            .get(object_key)
            .and_then(|_| self.receiver_class_name(object_key))
        else {
            return Ok(None);
        };
        let method_name = string_field(call, "subroutine")
            .and_then(|symbol| symbol.split_whitespace().next_back().map(str::to_string))
            .ok_or_else(|| "class method call has no method name".to_string())?;
        for (owner, class) in self.class_chain(&receiver_type)? {
            let empty_members = Vec::new();
            let members = class
                .get("members")
                .and_then(Value::as_array)
                .unwrap_or(&empty_members);
            if let Some(method) = members.iter().find(|member| {
                string_field(member, "kind").as_deref() == Some("Subroutine")
                    && string_field(member, "name").as_deref() == Some(&method_name)
            }) {
                return Ok(Some((method.clone(), owner)));
            }
        }
        Ok(None)
    }

    fn resolve_class_method(
        &self,
        class_name: &str,
        method_name: &str,
    ) -> Result<Option<(Value, String)>, String> {
        for (owner, class) in self.class_chain(class_name)? {
            let empty_members = Vec::new();
            let members = class
                .get("members")
                .and_then(Value::as_array)
                .unwrap_or(&empty_members);
            if let Some(method) = members.iter().find(|member| {
                string_field(member, "kind").as_deref() == Some("Subroutine")
                    && string_field(member, "name").as_deref() == Some(method_name)
            }) {
                return Ok(Some((method.clone(), owner)));
            }
        }
        Ok(None)
    }

    fn dynamic_class_method_candidates(
        &self,
        object_key: &str,
        call: &Value,
    ) -> Result<Option<Vec<ClassMethodCandidate>>, String> {
        let Some(receiver_type) = self.receiver_class_name(object_key) else {
            return Ok(None);
        };
        let Some(sources) = self.dynamic_class_sources.get(object_key) else {
            return Ok(None);
        };
        if sources.is_empty() {
            return Ok(None);
        }
        let method_name = string_field(call, "subroutine")
            .and_then(|symbol| symbol.split_whitespace().next_back().map(str::to_string))
            .ok_or_else(|| "class method call has no method name".to_string())?;
        let mut class_names = sources
            .iter()
            .map(|source| source.class_name.clone())
            .collect::<Vec<_>>();
        class_names.push(receiver_type);
        let mut candidates = Vec::new();
        for class_name in class_names {
            let Some((method, owner)) = self.resolve_class_method(&class_name, &method_name)?
            else {
                continue;
            };
            if !method_is_virtual(&method) {
                continue;
            }
            let address = method.get("addr").and_then(Value::as_u64);
            if candidates.iter().any(|candidate: &ClassMethodCandidate| {
                candidate.method.get("addr").and_then(Value::as_u64) == address
            }) {
                continue;
            }
            candidates.push(ClassMethodCandidate {
                class_name,
                method,
                owner,
            });
        }
        if candidates.len() < 2 {
            return Ok(None);
        }
        Ok(Some(candidates))
    }

    fn class_method_return_expression(
        &self,
        object_key: &str,
        candidate: &ClassMethodCandidate,
        call: &Value,
    ) -> Result<Expr, String> {
        let body =
            self.specialized_method_body(object_key, &candidate.owner, &candidate.method, call)?;
        if string_field(&body, "kind").as_deref() != Some("Return") {
            return Err("class function must have a single direct return expression".into());
        }
        let expression = body
            .get("expr")
            .ok_or_else(|| "class function return has no expression".to_string())?;
        self.lower_expression(expression)
    }

    fn lower_dynamic_class_function(
        &self,
        object_key: &str,
        call: &Value,
    ) -> Result<Option<Expr>, String> {
        let Some(mut candidates) = self.dynamic_class_method_candidates(object_key, call)? else {
            return Ok(None);
        };
        let tag = self
            .class_object_tags
            .get(object_key)
            .ok_or_else(|| format!("class object '{object_key}' has no runtime tag"))?;
        let fallback = candidates
            .pop()
            .ok_or_else(|| "dynamic class dispatch has no fallback method".to_string())?;
        let mut result = self.class_method_return_expression(object_key, &fallback, call)?;
        for candidate in candidates.iter().rev() {
            let class_id = self.class_id(&candidate.class_name)?;
            result = Expr::Cond {
                c: Box::new(Expr::Bin {
                    op: BinOp::Eq,
                    a: Box::new(class_expr(tag)),
                    b: Box::new(Expr::Const(LogicWord::from_u64(32, class_id))),
                }),
                t: Box::new(self.class_method_return_expression(object_key, candidate, call)?),
                f: Box::new(result),
            };
        }
        Ok(Some(result))
    }

    fn lower_dynamic_class_task(
        &self,
        object_key: &str,
        call: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(mut candidates) = self.dynamic_class_method_candidates(object_key, call)? else {
            return Ok(None);
        };
        let tag = self
            .class_object_tags
            .get(object_key)
            .ok_or_else(|| format!("class object '{object_key}' has no runtime tag"))?;
        let fallback = candidates
            .pop()
            .ok_or_else(|| "dynamic class dispatch has no fallback method".to_string())?;
        let mut result =
            self.lower_specialized_class_task(object_key, &fallback.owner, &fallback.method, call)?;
        for candidate in candidates.iter().rev() {
            let class_id = self.class_id(&candidate.class_name)?;
            result = Stmt::If {
                c: Expr::Bin {
                    op: BinOp::Eq,
                    a: Box::new(class_expr(tag)),
                    b: Box::new(Expr::Const(LogicWord::from_u64(32, class_id))),
                },
                t: Box::new(self.lower_specialized_class_task(
                    object_key,
                    &candidate.owner,
                    &candidate.method,
                    call,
                )?),
                f: Some(Box::new(result)),
            };
        }
        Ok(Some(result))
    }

    fn lower_specialized_class_task(
        &self,
        object_key: &str,
        owner_class: &str,
        method: &Value,
        call: &Value,
    ) -> Result<Stmt, String> {
        let body = self.specialized_method_body(object_key, owner_class, method, call)?;
        self.lower_statement(&body)
    }

    fn receiver_class_name(&self, object_key: &str) -> Option<String> {
        self.class_object_types.get(object_key).cloned()
    }

    fn class_id(&self, class_name: &str) -> Result<u64, String> {
        self.class_definitions
            .keys()
            .position(|candidate| candidate == class_name)
            .map(|index| index as u64 + 1)
            .ok_or_else(|| format!("unknown class '{class_name}'"))
    }

    fn class_chain(&self, start: &str) -> Result<Vec<(String, Value)>, String> {
        let mut chain = Vec::new();
        let mut current = start.to_string();
        loop {
            if chain.iter().any(|(name, _)| name == &current) {
                return Err(format!("class inheritance cycle at '{current}'"));
            }
            let Some(class) = self.class_definitions.get(&current).cloned() else {
                return Err(format!("unknown class '{current}'"));
            };
            let base = class
                .get("baseClass")
                .and_then(Value::as_str)
                .and_then(class_name);
            chain.push((current, class));
            match base {
                Some(next) => current = next,
                None => break,
            }
        }
        Ok(chain)
    }

    fn property_replacements(
        &self,
        object_key: &str,
        class_name: &str,
    ) -> Result<BTreeMap<String, Value>, String> {
        let mut replacements = BTreeMap::new();
        let handle_address = object_key
            .split_whitespace()
            .next()
            .and_then(|address| address.parse::<u64>().ok())
            .ok_or_else(|| format!("invalid class object '{object_key}'"))?;
        let empty_members = Vec::new();
        for (_, class) in self.class_chain(class_name)? {
            for property in class
                .get("members")
                .and_then(Value::as_array)
                .unwrap_or(&empty_members)
            {
                if string_field(property, "kind").as_deref() != Some("ClassProperty") {
                    continue;
                }
                if string_field(property, "lifetime").as_deref() == Some("Static") {
                    continue;
                }
                let property_name = declaration_name(property, "ClassProperty")?;
                let property_address = property
                    .get("addr")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| format!("class property '{property_name}' has no address"))?;
                let synthetic = format!("eda_class_{handle_address}_{property_address}");
                replacements.insert(
                    format!("{property_address} {property_name}"),
                    json!({
                        "kind": "NamedValue",
                        "type": string_field(property, "type").unwrap_or_default(),
                        "symbol": synthetic,
                    }),
                );
            }
        }
        Ok(replacements)
    }

    fn specialized_method_body(
        &self,
        object_key: &str,
        owner_class: &str,
        method: &Value,
        call: &Value,
    ) -> Result<Value, String> {
        let receiver_type = self
            .receiver_class_name(object_key)
            .ok_or_else(|| format!("unknown class object '{object_key}'"))?;
        let _ = receiver_type;
        let mut replacements = self.property_replacements(object_key, owner_class)?;

        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        let formals = method
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| "class method has no formal argument list".to_string())?;
        if arguments.len() != formals.len() {
            return Err(format!(
                "class method expects {} arguments, found {}",
                formals.len(),
                arguments.len()
            ));
        }
        for (formal, actual) in formals.iter().zip(arguments) {
            let formal_name = formal
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "class method has an unnamed argument".to_string())?;
            let formal_address = formal
                .get("addr")
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("class argument '{formal_name}' has no address"))?;
            replacements.insert(
                format!("{formal_address} {formal_name}"),
                json!({
                    "kind": "Conversion",
                    "type": string_field(formal, "type").unwrap_or_default(),
                    "operand": actual,
                }),
            );
        }

        let body = method
            .get("body")
            .cloned()
            .ok_or_else(|| "class method has no body".to_string())?;
        let specialized = substitute_class_symbols(&body, &replacements);
        self.expand_super_constructors(object_key, owner_class, &specialized)
    }

    fn expand_super_constructors(
        &self,
        object_key: &str,
        owner_class: &str,
        value: &Value,
    ) -> Result<Value, String> {
        let Some(fields) = value.as_object() else {
            if let Some(values) = value.as_array() {
                let mut rewritten = Vec::with_capacity(values.len());
                for nested in values {
                    rewritten.push(self.expand_super_constructors(
                        object_key,
                        owner_class,
                        nested,
                    )?);
                }
                return Ok(Value::Array(rewritten));
            }
            return Ok(value.clone());
        };
        if string_field(value, "kind").as_deref() == Some("ExpressionStatement") {
            let Some(expression) = value.get("expr") else {
                return Ok(value.clone());
            };
            if string_field(expression, "kind").as_deref() == Some("NewClass")
                && bool_field(expression, "isSuperClass")
            {
                let constructor = expression
                    .get("constructorCall")
                    .ok_or_else(|| "super constructor call has no constructor".to_string())?;
                let base = self
                    .class_definitions
                    .get(owner_class)
                    .and_then(|class| {
                        class
                            .get("baseClass")
                            .and_then(Value::as_str)
                            .and_then(class_name)
                    })
                    .ok_or_else(|| format!("class '{owner_class}' has no base constructor"))?;
                let empty_members = Vec::new();
                let mut base_constructor = None;
                for (class_name, class) in self.class_chain(&base)? {
                    let members = class
                        .get("members")
                        .and_then(Value::as_array)
                        .unwrap_or(&empty_members);
                    if let Some(method) = members.iter().find(|member| {
                        string_field(member, "kind").as_deref() == Some("Subroutine")
                            && string_field(member, "name").as_deref() == Some("new")
                    }) {
                        base_constructor = Some((method.clone(), class_name));
                        break;
                    }
                }
                let (method, base_owner) =
                    base_constructor.ok_or_else(|| format!("class '{base}' has no constructor"))?;
                let mut replacements = self.property_replacements(object_key, &base_owner)?;
                let empty_arguments = Vec::new();
                let arguments = constructor
                    .get("arguments")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty_arguments);
                let formals = method
                    .get("arguments")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "class constructor has no arguments".to_string())?;
                if arguments.len() != formals.len() {
                    return Err(format!(
                        "super constructor expects {} arguments, found {}",
                        formals.len(),
                        arguments.len()
                    ));
                }
                for (formal, actual) in formals.iter().zip(arguments) {
                    let formal_name = formal
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "super constructor argument is unnamed".to_string())?;
                    let formal_address =
                        formal.get("addr").and_then(Value::as_u64).ok_or_else(|| {
                            format!("super constructor argument '{formal_name}' has no address")
                        })?;
                    replacements.insert(
                        format!("{formal_address} {formal_name}"),
                        json!({
                            "kind": "Conversion",
                            "type": string_field(formal, "type").unwrap_or_default(),
                            "operand": actual,
                        }),
                    );
                }
                let body = method
                    .get("body")
                    .cloned()
                    .ok_or_else(|| "super constructor has no body".to_string())?;
                let specialized = substitute_class_symbols(&body, &replacements);
                return self.expand_super_constructors(object_key, &base_owner, &specialized);
            }
        }

        let mut rewritten = serde_json::Map::new();
        for (key, nested) in fields {
            rewritten.insert(
                key.clone(),
                self.expand_super_constructors(object_key, owner_class, nested)?,
            );
        }
        Ok(Value::Object(rewritten))
    }
}

fn class_name(type_name: &str) -> Option<String> {
    type_name.split_whitespace().next_back().map(str::to_string)
}

fn method_return_type(method: &Value) -> String {
    string_field(method, "returnType").unwrap_or_else(|| {
        method
            .get("returnType")
            .and_then(|return_type| string_field(return_type, "name"))
            .unwrap_or_default()
    })
}

fn method_is_virtual(method: &Value) -> bool {
    string_field(method, "flags")
        .map(|flags| flags.split(',').any(|flag| flag.trim() == "virtual"))
        .unwrap_or(false)
}

fn method_is_static(method: &Value) -> bool {
    string_field(method, "flags")
        .map(|flags| flags.split(',').any(|flag| flag.trim() == "static"))
        .unwrap_or(false)
}

fn named_class_handle(value: &Value) -> Option<(String, String)> {
    if string_field(value, "kind").as_deref() != Some("NamedValue") {
        return None;
    }
    let object_key = string_field(value, "symbol")?;
    let type_name = string_field(value, "type")?;
    let class_name = class_name(&type_name)?;
    Some((object_key, class_name))
}

fn conversion_operand(value: &Value) -> Option<&Value> {
    if string_field(value, "kind").as_deref() != Some("Conversion") {
        return None;
    }
    value.get("operand")
}

fn class_expr(symbol: &super::AstLoweredSymbol) -> Expr {
    Expr::Ref {
        net: symbol.base,
        decl_width: symbol.width,
        range: super::hierarchy::full_range(symbol.width),
        signed: symbol.signed,
    }
}

fn class_lvalue(symbol: &super::AstLoweredSymbol) -> eda_core::ir::LValue {
    eda_core::ir::LValue {
        net: symbol.base,
        decl_width: symbol.width,
        range: super::hierarchy::full_range(symbol.width),
    }
}

fn class_field_assignment(
    symbol: &super::AstLoweredSymbol,
    expression: &Expr,
    nonblocking: bool,
) -> Stmt {
    if nonblocking {
        Stmt::NBl {
            lv: class_lvalue(symbol),
            e: expression.clone(),
        }
    } else {
        Stmt::Bl {
            lv: class_lvalue(symbol),
            e: expression.clone(),
        }
    }
}

fn substitute_class_symbols(value: &Value, replacements: &BTreeMap<String, Value>) -> Value {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("NamedValue") {
            if let Some(symbol) = string_field(value, "symbol") {
                if let Some(replacement) = replacements.get(&symbol) {
                    return replacement.clone();
                }
            }
        }
        fields
            .iter()
            .map(|(key, nested)| (key.clone(), substitute_class_symbols(nested, replacements)))
            .collect::<serde_json::Map<String, Value>>()
            .into()
    } else if let Some(values) = value.as_array() {
        values
            .iter()
            .map(|nested| substitute_class_symbols(nested, replacements))
            .collect::<Vec<_>>()
            .into()
    } else {
        value.clone()
    }
}
