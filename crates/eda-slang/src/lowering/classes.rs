//! Direct lowering for a small class-object subset with explicit dynamic
//! handle-alias metadata.

use super::{declaration_name, SlangAstLowerer};
use crate::ast_util::{bool_field, string_field};
use eda_core::ir::{BinOp, Expr, Stmt};
use eda_core::logic::LogicWord;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const MAX_CLASS_ARRAY_ELEMENTS: usize = 4096;

#[derive(Clone)]
struct ClassMethodCandidate {
    class_name: String,
    method: Value,
    owner: String,
}

struct DynamicClassArraySelection {
    base_key: String,
    selector: Expr,
    elements: Vec<(i64, String)>,
}

#[derive(Clone)]
struct ClassHandleCandidates {
    keys: Vec<String>,
    class_name: String,
}

impl SlangAstLowerer {
    fn class_child_lookup_key(&self, call: &Value) -> Result<String, String> {
        let receiver = call
            .get("thisClass")
            .ok_or_else(|| "class get_child_by_name call has no receiver".to_string())?;
        let parent_key = self
            .class_object_key_for_value(receiver)?
            .or_else(|| {
                if is_this_reference(receiver) {
                    self.active_class_objects.borrow().last().cloned()
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                "class get_child_by_name receiver is not a prepared class object".to_string()
            })?;
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 1 {
            return Err("class get_child_by_name requires one string argument".into());
        }
        let requested_name = string_field(
            conversion_operand(&arguments[0]).unwrap_or(&arguments[0]),
            "literal",
        )
        .ok_or_else(|| "class get_child_by_name requires a string literal".to_string())?;
        let children = self
            .class_object_children
            .borrow()
            .get(&parent_key)
            .cloned()
            .unwrap_or_default();
        children
            .iter()
            .find(|child_key| {
                self.class_object_declared_names
                    .borrow()
                    .get(*child_key)
                    .is_some_and(|name| *name == requested_name)
            })
            .cloned()
            .ok_or_else(|| {
                format!("class parent '{parent_key}' has no child named '{requested_name}'")
            })
    }

    fn dynamic_sources_for(&self, object_key: &str) -> Vec<super::DynamicClassSource> {
        let mut sources = self
            .dynamic_class_sources
            .get(object_key)
            .cloned()
            .unwrap_or_default();
        sources.extend(
            self.runtime_dynamic_class_sources
                .borrow()
                .get(object_key)
                .cloned()
                .unwrap_or_default(),
        );
        sources
    }

    pub(super) fn record_runtime_dynamic_source(
        &self,
        target_key: &str,
        source_key: &str,
        source_class: &str,
    ) -> Result<(), String> {
        let target_class = self
            .class_object_types
            .get(target_key)
            .ok_or_else(|| format!("unknown class handle '{target_key}'"))?;
        if !self
            .class_chain(source_class)?
            .iter()
            .any(|(class_name, _)| class_name == target_class)
        {
            return Ok(());
        }
        let mut sources = self.runtime_dynamic_class_sources.borrow_mut();
        let entries = sources.entry(target_key.to_string()).or_default();
        if !entries.iter().any(|source| source.object_key == source_key) {
            entries.push(super::DynamicClassSource {
                object_key: source_key.to_string(),
                class_name: source_class.to_string(),
            });
        }
        Ok(())
    }

    pub(super) fn class_type_key(&self, type_name: &str) -> String {
        let Some(base_name) = class_name(type_name) else {
            return type_name.to_string();
        };
        if let Some(address) = type_name
            .split_whitespace()
            .next()
            .and_then(|candidate| candidate.parse::<u64>().ok())
        {
            let specialized = format!("{base_name}@{address}");
            if self.class_definitions.contains_key(&specialized) {
                return specialized;
            }
        }
        base_name
    }

    pub(super) fn class_object_key_for_value(
        &self,
        value: &Value,
    ) -> Result<Option<String>, String> {
        match string_field(value, "kind").as_deref() {
            Some("NamedValue" | "HierarchicalValue") => {
                Ok(string_field(value, "symbol").filter(|symbol_key| {
                    self.class_objects.contains_key(symbol_key)
                        || self.class_object_arrays.contains_key(symbol_key)
                }))
            }
            Some("MemberAccess") => {
                let Some(receiver) = value.get("value") else {
                    return Ok(None);
                };
                if self.dynamic_class_array_selection(receiver)?.is_some() {
                    return Ok(None);
                }
                let Some(receiver_key) = self.class_object_key_for_value(receiver)? else {
                    return Ok(None);
                };
                let Some(member_name) = string_field(value, "member")
                    .and_then(|member| member.split_whitespace().next_back().map(str::to_string))
                else {
                    return Ok(None);
                };
                Ok(self
                    .class_composition_children
                    .get(&(receiver_key, member_name))
                    .cloned())
            }
            Some("ElementSelect") => {
                let Some(selected) = value.get("value") else {
                    return Ok(None);
                };
                let Some(base_key) = self.class_object_key_for_value(selected)? else {
                    return Ok(None);
                };
                if !self.class_object_arrays.contains_key(&base_key) {
                    return Ok(None);
                }
                let Some(selector) = value.get("selector") else {
                    return Err(format!("class array '{base_key}' element has no selector"));
                };
                let Some(index) = constant_selector(selector) else {
                    return Err(format!(
                        "dynamic class-array element selection through '{base_key}' is not supported"
                    ));
                };
                self.class_object_arrays
                    .get(&base_key)
                    .and_then(|elements| elements.get(&index))
                    .cloned()
                    .map(Ok)
                    .transpose()
            }
            _ => Ok(None),
        }
    }

    fn dynamic_class_array_selection(
        &self,
        value: &Value,
    ) -> Result<Option<DynamicClassArraySelection>, String> {
        if string_field(value, "kind").as_deref() != Some("ElementSelect") {
            return Ok(None);
        }
        let Some(selected) = value.get("value") else {
            return Ok(None);
        };
        let Some(base_key) = self.class_object_key_for_value(selected)? else {
            return Ok(None);
        };
        let Some(elements) = self.class_object_arrays.get(&base_key) else {
            return Ok(None);
        };
        let Some(selector) = value.get("selector") else {
            return Err(format!("class array '{base_key}' element has no selector"));
        };
        if constant_selector(selector).is_some() {
            return Ok(None);
        }
        let selector = self.lower_expression(selector)?;
        let elements = elements
            .iter()
            .map(|(index, key)| (*index, key.clone()))
            .collect();
        Ok(Some(DynamicClassArraySelection {
            base_key,
            selector,
            elements,
        }))
    }

    fn class_array_element_receiver(element_key: &str, value: &Value) -> Value {
        let mut receiver = value.clone();
        if let Some(fields) = receiver.as_object_mut() {
            fields.insert("kind".into(), Value::String("NamedValue".into()));
            fields.insert("symbol".into(), Value::String(element_key.to_string()));
            fields.remove("value");
            fields.remove("selector");
        }
        receiver
    }

    fn class_array_condition(selector: &Expr, index: i64) -> Expr {
        Expr::Bin {
            op: BinOp::Eq,
            a: Box::new(selector.clone()),
            b: Box::new(Expr::Const(LogicWord::from_u64(32, index as u64))),
        }
    }

    fn is_null_literal(value: &Value) -> bool {
        let value = conversion_operand(value).unwrap_or(value);
        string_field(value, "kind").as_deref() == Some("NullLiteral")
    }

    fn class_null_statement(&self, object_key: &str) -> Result<Stmt, String> {
        let Some(fields) = self.class_objects.get(object_key) else {
            return Ok(Stmt::Block(Vec::new()));
        };
        let mut statements = Vec::new();
        for symbol in [
            self.class_object_tags
                .get(object_key)
                .ok_or_else(|| format!("class object '{object_key}' has no runtime tag"))?,
            self.class_object_ids
                .get(object_key)
                .ok_or_else(|| format!("class object '{object_key}' has no object id"))?,
        ] {
            statements.push(Stmt::Bl {
                lv: class_lvalue(symbol),
                e: Expr::Const(LogicWord::zero(symbol.width)),
            });
        }
        for symbol in fields.values() {
            statements.push(Stmt::Bl {
                lv: class_lvalue(symbol),
                e: Expr::Const(LogicWord::xes(symbol.width)),
            });
        }
        Ok(Stmt::Block(statements))
    }

    pub(super) fn lower_class_null_assignment(
        &self,
        assignment: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(left) = assignment.get("left") else {
            return Ok(None);
        };
        let Some(right) = assignment.get("right") else {
            return Ok(None);
        };
        if !Self::is_null_literal(right) {
            return Ok(None);
        }
        if bool_field(assignment, "isNonBlocking") {
            return Err("non-blocking null class-handle assignment is not supported".into());
        }
        if let Some(selection) = self.dynamic_class_array_selection(left)? {
            let statements = selection
                .elements
                .iter()
                .map(|(index, object_key)| {
                    Ok(Stmt::If {
                        c: Self::class_array_condition(&selection.selector, *index),
                        t: Box::new(self.class_null_statement(object_key)?),
                        f: None,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            return Ok(Some(Stmt::Block(statements)));
        }
        let Some(object_key) = self.class_object_key_for_value(left)? else {
            return Ok(None);
        };
        self.class_null_statement(&object_key).map(Some)
    }

    fn class_handle_identity_expr(&self, value: &Value) -> Result<Option<Expr>, String> {
        if Self::is_null_literal(value) {
            return Ok(Some(Expr::Const(LogicWord::zero(64))));
        }
        if let Some(selection) = self.dynamic_class_array_selection(value)? {
            let mut result = Expr::Const(LogicWord::zero(64));
            for (index, object_key) in selection.elements.iter().rev() {
                let Some(id) = self.class_object_ids.get(object_key) else {
                    continue;
                };
                result = Expr::Cond {
                    c: Box::new(Self::class_array_condition(&selection.selector, *index)),
                    t: Box::new(class_expr(id)),
                    f: Box::new(result),
                };
            }
            return Ok(Some(result));
        }
        if is_class_child_lookup_call(value) {
            let child_key = self.class_child_lookup_key(value)?;
            if let Some(id) = self.class_object_ids.get(&child_key) {
                return Ok(Some(class_expr(id)));
            }
        }
        if string_field(value, "kind").as_deref() == Some("Call")
            && string_field(value, "subroutine")
                .and_then(|subroutine| {
                    subroutine
                        .split_whitespace()
                        .next_back()
                        .map(str::to_string)
                })
                .as_deref()
                == Some("get_parent")
        {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if arguments.is_empty() {
                if let Some(receiver) = value.get("thisClass") {
                    if let Some(object_key) = self.class_object_key_for_value(receiver)? {
                        if let Some(parent_id) = self.class_object_parent_ids.get(&object_key) {
                            return Ok(Some(class_expr(parent_id)));
                        }
                    }
                }
            }
        }
        if string_field(value, "kind").as_deref() == Some("NamedValue")
            && string_field(value, "symbol")
                .and_then(|symbol| symbol.split_whitespace().next_back().map(str::to_string))
                .as_deref()
                == Some("this")
        {
            if let Some(object_key) = self.active_class_objects.borrow().last().cloned() {
                if let Some(id) = self.class_object_ids.get(&object_key) {
                    return Ok(Some(class_expr(id)));
                }
            }
        }
        Ok(self
            .class_object_key_for_value(value)?
            .and_then(|object_key| self.class_object_ids.get(&object_key).cloned())
            .map(|symbol| class_expr(&symbol)))
    }

    pub(super) fn lower_class_handle_comparison(
        &self,
        value: &Value,
    ) -> Result<Option<Expr>, String> {
        if string_field(value, "kind").as_deref() != Some("BinaryOp") {
            return Ok(None);
        }
        let operator = match string_field(value, "op").as_deref() {
            Some("Equality") => BinOp::Eq,
            Some("Inequality") => BinOp::Ne,
            _ => return Ok(None),
        };
        let Some(left) = value.get("left") else {
            return Ok(None);
        };
        let Some(right) = value.get("right") else {
            return Ok(None);
        };
        let Some(left_id) = self.class_handle_identity_expr(left)? else {
            return Ok(None);
        };
        let Some(right_id) = self.class_handle_identity_expr(right)? else {
            return Ok(None);
        };
        Ok(Some(Expr::Bin {
            op: operator,
            a: Box::new(left_id),
            b: Box::new(right_id),
        }))
    }

    pub(super) fn collect_class_static_members(&mut self) -> Result<(), String> {
        let definitions = self
            .class_definitions
            .iter()
            .filter(|(key, _)| key.contains('@'))
            .map(|(key, class)| (key.clone(), class.clone()))
            .collect::<BTreeMap<_, _>>();
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
                        let property_address = member.get("addr").and_then(Value::as_u64).ok_or_else(|| {
                                super::with_source_span(
                                    member,
                                    format!("static class property '{class_name}.{property_name}' has no address"),
                                )
                            })?;
                        let symbol_key = format!("{property_address} {property_name}");
                        if self.symbols.contains_key(&symbol_key) {
                            continue;
                        }
                        let property_type = string_field(member, "type").unwrap_or_default();
                        let Ok((shape, signed, struct_fields)) = self.net_type(&property_type)
                        else {
                            continue;
                        };
                        let base = self.design.n_nets;
                        self.design.n_nets += shape.width;
                        let storage_name = format!(
                            "__class_static_{property_address}_{class_name}_{property_name}"
                        );
                        self.design.net_decls.push(eda_core::ir::NetDecl {
                            name: storage_name.clone(),
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
                        self.symbols.insert(symbol_key.clone(), symbol.clone());
                        self.class_static_properties.insert(storage_name, symbol);
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
            let Ok(expression) = self.lower_expression(&initializer) else {
                self.symbols.remove(&symbol_key);
                self.class_static_properties
                    .retain(|_, lowered| lowered.base != symbol.base);
                continue;
            };
            self.design.initials.push(Stmt::Bl {
                lv: class_lvalue(&symbol),
                e: expression,
            });
            self.class_static_initializer_count += 1;
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
            if string_field(value, "kind").as_deref() == Some("Call")
                && string_field(value, "subroutine").as_deref() == Some("$cast")
            {
                return self.record_class_cast_destination(value);
            }
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

    fn record_class_cast_destination(&mut self, call: &Value) -> Result<(), String> {
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 2 {
            return Err("$cast requires exactly two class-handle arguments".into());
        }
        let Some(target) = arguments[0].get("left") else {
            return Ok(());
        };
        let Some(target) = self.class_handle_candidates_from_ast(target)? else {
            return Ok(());
        };
        for target_key in target.keys {
            self.initialized_class_objects.insert(target_key);
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
        let Some(target) = self.class_handle_candidates_from_ast(left)? else {
            return Ok(());
        };
        if is_class_parent_call(right) {
            for target_key in target.keys {
                self.initialized_class_objects.insert(target_key);
            }
            return Ok(());
        }
        if string_field(right, "kind").as_deref() == Some("NewClass") {
            for target_key in target.keys {
                self.initialized_class_objects.insert(target_key.clone());
                self.constructed_class_objects.insert(target_key);
            }
            return Ok(());
        }
        let source = conversion_operand(right).unwrap_or(right);
        let Some(source) = self.class_handle_candidates_from_ast(source)? else {
            return Ok(());
        };
        let chain = self.class_chain(&source.class_name)?;
        if !chain
            .iter()
            .any(|(class_name, _)| *class_name == target.class_name)
        {
            return Ok(());
        }
        for target_key in target.keys {
            self.initialized_class_objects.insert(target_key.clone());
            let sources = self.dynamic_class_sources.entry(target_key).or_default();
            for source_key in &source.keys {
                if sources
                    .iter()
                    .any(|source| source.object_key == *source_key)
                {
                    continue;
                }
                sources.push(super::DynamicClassSource {
                    object_key: source_key.clone(),
                    class_name: source.class_name.clone(),
                });
            }
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
        let class_key = self.class_type_key(&type_name);
        if !self.class_definitions.contains_key(&class_key) {
            return Ok(false);
        }
        let Some(class) = self.class_definitions.get(&class_key).cloned() else {
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
        if !self.active_composition_classes.insert(class_key.clone()) {
            return Err(format!(
                "recursive class composition through '{class_name}' is not supported"
            ));
        }
        let chain = self.class_chain(&class_key)?;
        let official_uvm_derivation = chain
            .iter()
            .any(|(owner, _)| owner.split('@').next() == Some("uvm_test"));
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
        let base_object_key = format!("{handle_address} {name}");
        let indexes = class_array_indexes(&type_name).unwrap_or_else(|| vec![None]);
        if indexes.len() > MAX_CLASS_ARRAY_ELEMENTS {
            return Err(format!(
                "class object array '{name}' has {} elements; the direct path supports at most {MAX_CLASS_ARRAY_ELEMENTS}",
                indexes.len()
            ));
        }
        if indexes.iter().any(Option::is_some) {
            let empty_members = Vec::new();
            let constructor = chain.iter().find_map(|(_, class)| {
                class
                    .get("members")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty_members)
                    .iter()
                    .find(|member| {
                        string_field(member, "kind").as_deref() == Some("Subroutine")
                            && string_field(member, "name").as_deref() == Some("new")
                    })
                    .cloned()
            });
            if let Some(constructor) = constructor {
                let empty_arguments = Vec::new();
                let formals = constructor
                    .get("arguments")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty_arguments);
                let mut temps = Vec::new();
                for (position, formal) in formals.iter().enumerate() {
                    let address = formal
                        .get("addr")
                        .and_then(Value::as_u64)
                        .unwrap_or(position as u64);
                    let type_name = string_field(formal, "type").unwrap_or_default();
                    let (shape, signed, _) = self.net_type(&type_name)?;
                    let mut temporary = self.declare_class_internal_net(
                        &format!("__class_array_new_{handle_address}_{address}"),
                        shape.width,
                    );
                    temporary.signed = signed;
                    let symbol_name = format!("__class_array_new_value_{handle_address}_{address}");
                    self.symbols.insert(symbol_name.clone(), temporary.clone());
                    temps.push((symbol_name, temporary));
                }
                self.class_array_constructor_temps
                    .insert(base_object_key.clone(), temps);
            }
        }
        let mut array_elements = BTreeMap::new();
        let mut class_properties = Vec::new();
        for index in indexes {
            let element_name = index
                .map(|index| format!("{name}[{index}]"))
                .unwrap_or_else(|| name.to_string());
            let object_key = format!("{handle_address} {element_name}");
            let synthetic_prefix = index
                .map(|index| format!("{handle_address}_{index}"))
                .unwrap_or_else(|| handle_address.to_string());
            let mut fields: BTreeMap<String, super::AstLoweredSymbol> = BTreeMap::new();
            let mut property_initializers = Vec::new();
            let mut property_chains = vec![chain.clone()];
            if let Some(sources) = self.dynamic_class_sources.get(&object_key) {
                for source in sources {
                    property_chains.push(self.class_chain(&source.class_name)?);
                }
            }
            for property_chain in &property_chains {
                for (owner_name, class) in property_chain.iter().rev() {
                    let empty_members = Vec::new();
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
                        if official_uvm_derivation && owner_name != &class_key {
                            continue;
                        }
                        let property_name = declaration_name(property, "ClassProperty")?;
                        let initializer = property.get("initializer").cloned();
                        let property_address = property
                            .get("addr")
                            .and_then(Value::as_u64)
                            .ok_or_else(|| {
                                format!(
                                    "class property '{class_name}.{property_name}' has no address"
                                )
                            })?;
                        let property_type = string_field(property, "type").unwrap_or_default();
                        let property_class_key = self.class_type_key(&property_type);
                        if self.class_definitions.contains_key(&property_class_key) {
                            if owner_name != &class_key {
                                continue;
                            }
                            if !class_properties.iter().any(
                                |candidate: &(String, Value, String)| candidate.0 == property_name,
                            ) {
                                class_properties.push((
                                    property_name.clone(),
                                    property.clone(),
                                    property_class_key,
                                ));
                            }
                            continue;
                        }
                        let Ok((shape, signed, _)) = self.net_type(&property_type) else {
                            if owner_name != &class_key {
                                continue;
                            }
                            return Err(format!(
                                "class property '{class_name}.{property_name}' has an unsupported type"
                            ));
                        };
                        let synthetic_symbol =
                            format!("eda_class_{synthetic_prefix}_{property_address}");
                        if let Some(existing) = fields.get(&property_name) {
                            if existing.width != shape.width
                                || existing.signed != signed
                                || existing.bounds != shape.bounds
                            {
                                return Err(format!(
                                "dynamic class storage for '{element_name}.{property_name}' has conflicting layouts"
                            ));
                            }
                            self.symbols.insert(synthetic_symbol, existing.clone());
                            continue;
                        }
                        let field_name = format!("{element_name}.{property_name}");
                        let symbol = self.declare_net(property, &field_name, "Variable")?;
                        self.symbols.insert(synthetic_symbol, symbol.clone());
                        self.class_property_owners
                            .insert(symbol.base, (object_key.clone(), property_name.clone()));
                        let mut temporary = self.declare_class_internal_net(
                            &format!("__class_tmp_{synthetic_prefix}_{property_address}"),
                            shape.width,
                        );
                        temporary.signed = signed;
                        self.class_property_temps
                            .insert((object_key.clone(), property_name.clone()), temporary);
                        fields.insert(property_name.clone(), symbol);
                        if let Some(initializer) = initializer {
                            property_initializers.push((property_name.clone(), initializer));
                        }
                    }
                }
            }
            self.class_objects.insert(object_key.clone(), fields);
            self.class_property_initializers
                .insert(object_key.clone(), property_initializers);
            self.class_object_types
                .insert(object_key.clone(), class_key.clone());
            let tag =
                self.declare_class_internal_net(&format!("__class_tag_{synthetic_prefix}"), 32);
            let id = self
                .declare_class_internal_net(&format!("__class_object_id_{synthetic_prefix}"), 64);
            let object_name = self.declare_class_internal_net(
                &format!("__class_object_name_{synthetic_prefix}"),
                crate::types::STRING_WIDTH,
            );
            let object_full_name = self.declare_class_internal_net(
                &format!("__class_object_full_name_{synthetic_prefix}"),
                crate::types::STRING_WIDTH,
            );
            let parent_id = self
                .declare_class_internal_net(&format!("__class_parent_id_{synthetic_prefix}"), 64);
            self.design.initials.extend([
                Stmt::Bl {
                    lv: class_lvalue(&tag),
                    e: Expr::Const(LogicWord::zero(tag.width)),
                },
                Stmt::Bl {
                    lv: class_lvalue(&id),
                    e: Expr::Const(LogicWord::zero(id.width)),
                },
                Stmt::Bl {
                    lv: class_lvalue(&object_full_name),
                    e: Expr::Const(LogicWord::zero(object_full_name.width)),
                },
                Stmt::Bl {
                    lv: class_lvalue(&parent_id),
                    e: Expr::Const(LogicWord::zero(parent_id.width)),
                },
            ]);
            self.class_object_tags.insert(object_key.clone(), tag);
            self.class_object_ids.insert(object_key.clone(), id);
            self.class_object_names
                .insert(object_key.clone(), object_name);
            self.class_object_full_names
                .insert(object_key.clone(), object_full_name);
            self.class_object_parent_ids
                .insert(object_key.clone(), parent_id);
            if let Some(index) = index {
                array_elements.insert(index, object_key.clone());
            }
            for (property_name, property, _) in &class_properties {
                let synthetic_handle = self.next_synthetic_class_handle;
                self.next_synthetic_class_handle = self
                    .next_synthetic_class_handle
                    .checked_sub(1)
                    .ok_or_else(|| "synthetic class-handle address space exhausted".to_string())?;
                let mut child_member = property.clone();
                if let Some(fields) = child_member.as_object_mut() {
                    fields.insert("addr".into(), Value::from(synthetic_handle));
                }
                let child_name = format!("{element_name}.{property_name}");
                if !self.declare_class_object(&child_member, &child_name)? {
                    return Err(format!(
                        "class property '{object_key}.{property_name}' has an invalid class type"
                    ));
                }
                let child_key = format!("{synthetic_handle} {child_name}");
                self.class_composition_children.insert(
                    (object_key.clone(), property_name.clone()),
                    child_key.clone(),
                );
                self.class_composition_parents
                    .insert(child_key.clone(), object_key.clone());
                if let Some(elements) = self.class_object_arrays.get(&child_key) {
                    for element_key in elements.values() {
                        self.class_composition_parents
                            .insert(element_key.clone(), object_key.clone());
                    }
                }
            }
        }
        if !array_elements.is_empty() {
            self.class_object_arrays
                .insert(base_object_key, array_elements);
        }
        self.active_composition_classes.remove(&class_key);
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

    pub(super) fn declare_class_internal_net(
        &mut self,
        name: &str,
        width: usize,
    ) -> super::AstLoweredSymbol {
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
        if let Some(DynamicClassArraySelection {
            base_key,
            selector,
            elements,
        }) = self.dynamic_class_array_selection(left)?
        {
            let constructor_call = right
                .get("constructorCall")
                .ok_or_else(|| "class construction has no constructor call".to_string())?;
            let empty_arguments = Vec::new();
            let actuals = constructor_call
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            let temps = self
                .class_array_constructor_temps
                .get(&base_key)
                .cloned()
                .ok_or_else(|| {
                    format!("class object array '{base_key}' has no sampled constructor storage")
                })?;
            if actuals.len() != temps.len() {
                return Err(format!(
                    "class object array '{base_key}' constructor expects {} arguments, found {}",
                    temps.len(),
                    actuals.len()
                ));
            }

            let mut statements = Vec::new();
            let mut sampled_actuals = Vec::new();
            for ((symbol_name, temporary), actual) in temps.iter().zip(actuals) {
                let expression = self.lower_expression(actual)?;
                statements.push(Stmt::Bl {
                    lv: class_lvalue(temporary),
                    e: expression,
                });
                sampled_actuals.push(json!({
                    "kind": "NamedValue",
                    "type": string_field(actual, "type").unwrap_or_default(),
                    "symbol": symbol_name,
                }));
            }

            for (index, element_key) in elements {
                let mut specialized = assignment.clone();
                let Some(right) = specialized.get_mut("right") else {
                    continue;
                };
                let Some(constructor_call) = right.get_mut("constructorCall") else {
                    continue;
                };
                if let Some(arguments) = constructor_call
                    .get_mut("arguments")
                    .and_then(Value::as_array_mut)
                {
                    for (argument, sampled) in arguments.iter_mut().zip(sampled_actuals.iter()) {
                        *argument = sampled.clone();
                    }
                }
                if let Some(fields) = specialized.as_object_mut() {
                    fields.insert(
                        "left".into(),
                        Self::class_array_element_receiver(&element_key, left),
                    );
                }
                let Some(statement) = self.lower_class_constructor(&specialized)? else {
                    continue;
                };
                statements.push(Stmt::If {
                    c: Self::class_array_condition(&selector, index),
                    t: Box::new(statement),
                    f: None,
                });
            }
            return Ok(Some(Stmt::Block(statements)));
        }
        let Some(object_key) = self.class_object_key_for_value(left)? else {
            return Ok(None);
        };
        let constructor = right
            .get("constructorCall")
            .ok_or_else(|| "class construction has no constructor call".to_string())?;
        let target_class = self
            .class_object_types
            .get(&object_key)
            .cloned()
            .ok_or_else(|| format!("unknown class object '{object_key}'"))?;
        let runtime_class = string_field(right, "type")
            .map(|type_name| self.class_type_key(&type_name))
            .unwrap_or_else(|| target_class.clone());
        let target_base = target_class.split('@').next().unwrap_or(&target_class);
        let runtime_compatible = runtime_class != target_class
            && self.class_definitions.contains_key(&runtime_class)
            && self.class_chain(&runtime_class)?.iter().any(|(owner, _)| {
                owner == &target_class || owner.split('@').next() == Some(target_base)
            });
        if runtime_class != target_class && !runtime_compatible {
            return Err(format!(
                "class object '{object_key}' of type '{target_class}' cannot construct incompatible runtime class '{runtime_class}'"
            ));
        }
        let constructor_class = if runtime_compatible {
            runtime_class.clone()
        } else {
            target_class.clone()
        };
        let class_id = self.class_id(&constructor_class)?;
        let tag = self
            .class_object_tags
            .get(&object_key)
            .ok_or_else(|| format!("class object '{object_key}' has no runtime tag"))?
            .clone();
        let id = self
            .class_object_ids
            .get(&object_key)
            .ok_or_else(|| format!("class object '{object_key}' has no object id"))?
            .clone();
        let serial = self
            .class_object_serial
            .clone()
            .ok_or_else(|| "class object serial counter is missing".to_string())?;
        let object_name = self
            .class_object_names
            .get(&object_key)
            .cloned()
            .ok_or_else(|| format!("class object '{object_key}' has no name storage"))?;
        let object_full_name = self
            .class_object_full_names
            .get(&object_key)
            .cloned()
            .ok_or_else(|| format!("class object '{object_key}' has no full-name storage"))?;
        let empty_constructor_arguments = Vec::new();
        let constructor_arguments = constructor
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_constructor_arguments);
        let name_expression = constructor_arguments
            .first()
            .filter(|actual| {
                string_field(conversion_operand(actual).unwrap_or(actual), "kind").as_deref()
                    == Some("StringLiteral")
            })
            .map(|actual| {
                let expression = self.lower_expression(actual)?;
                Ok::<Expr, String>(expression)
            })
            .transpose()?;
        let declared_name = constructor_arguments.first().and_then(|actual| {
            string_field(conversion_operand(actual).unwrap_or(actual), "literal")
        });
        let parent_key = constructor_arguments
            .get(1)
            .and_then(|parent| {
                let parent = conversion_operand(parent).unwrap_or(parent);
                let is_this = string_field(parent, "kind").as_deref() == Some("NamedValue")
                    && string_field(parent, "symbol")
                        .and_then(|symbol| {
                            symbol.split_whitespace().next_back().map(str::to_string)
                        })
                        .as_deref()
                        == Some("this");
                is_this.then(|| {
                    self.active_class_objects
                        .borrow()
                        .last()
                        .cloned()
                        .ok_or_else(|| "class parent 'this' has no active receiver".to_string())
                })
            })
            .transpose()?;
        if let Some(parent_key) = &parent_key {
            self.class_object_parent_keys
                .borrow_mut()
                .insert(object_key.clone(), parent_key.clone());
        }
        if let (Some(parent_key), Some(declared_name)) = (&parent_key, declared_name) {
            self.class_object_declared_names
                .borrow_mut()
                .insert(object_key.clone(), declared_name.to_string());
            self.class_object_children
                .borrow_mut()
                .entry(parent_key.clone())
                .or_default()
                .retain(|existing| existing != &object_key);
            self.class_object_children
                .borrow_mut()
                .get_mut(parent_key)
                .ok_or_else(|| format!("class parent '{parent_key}' has no child registry"))?
                .push(object_key.clone());
        }
        let full_name_expression = match (&name_expression, &parent_key) {
            (Some(name_expression), Some(parent_key)) => {
                let parent_full_name = self
                    .class_object_full_names
                    .get(parent_key)
                    .cloned()
                    .ok_or_else(|| format!("class object '{parent_key}' has no full-name"))?;
                let separator = crate::types::string_literal_word(".")
                    .ok_or_else(|| "class-name separator is too long".to_string())?;
                Some(Expr::StringConcat {
                    left: Box::new(class_expr(&parent_full_name)),
                    right: Box::new(Expr::StringConcat {
                        left: Box::new(Expr::Const(separator)),
                        right: Box::new(name_expression.clone()),
                    }),
                })
            }
            (Some(name_expression), None) => Some(name_expression.clone()),
            (None, _) => None,
        };
        let parent_id_expression = parent_key.as_ref().and_then(|parent_key| {
            self.class_object_ids
                .get(parent_key)
                .cloned()
                .map(|parent_id| class_expr(&parent_id))
        });
        let object_parent_id = self
            .class_object_parent_ids
            .get(&object_key)
            .cloned()
            .ok_or_else(|| format!("class object '{object_key}' has no parent identity"))?;
        let initializer_statements = self.class_property_initializer_statements(&object_key)?;
        let body = if constructor_class != target_class {
            let Some((method, owner)) = self.resolve_class_method(&constructor_class, "new")?
            else {
                return Err(format!(
                    "class '{constructor_class}' has no constructor for object '{object_key}'"
                ));
            };
            let specialized =
                self.specialized_method_body(&object_key, &owner, &method, constructor)?;
            self.lower_statement(&specialized)?
        } else {
            self.lower_class_method(&object_key, constructor, true)?
        };
        let mut statements = vec![
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
        ];
        if let Some(expression) = name_expression {
            statements.push(Stmt::Bl {
                lv: class_lvalue(&object_name),
                e: expression,
            });
        }
        if let Some(expression) = full_name_expression {
            statements.push(Stmt::Bl {
                lv: class_lvalue(&object_full_name),
                e: expression,
            });
        }
        if let Some(expression) = parent_id_expression {
            statements.push(Stmt::Bl {
                lv: class_lvalue(&object_parent_id),
                e: expression,
            });
        }
        statements.extend(initializer_statements);
        statements.push(body);
        Ok(Some(Stmt::Block(statements)))
    }

    fn class_property_initializer_statements(&self, object_key: &str) -> Result<Vec<Stmt>, String> {
        let Some(initializers) = self.class_property_initializers.get(object_key) else {
            return Ok(Vec::new());
        };
        let Some(fields) = self.class_objects.get(object_key) else {
            return Ok(Vec::new());
        };
        let mut statements = Vec::new();
        for (field_name, initializer) in initializers {
            let Some(symbol) = fields.get(field_name) else {
                continue;
            };
            let expression = self.lower_expression(initializer)?;
            statements.push(Stmt::Bl {
                lv: class_lvalue(symbol),
                e: expression,
            });
        }
        Ok(statements)
    }

    pub(super) fn lower_class_task_call(
        &self,
        call: &Value,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        let Some(receiver) = call.get("thisClass") else {
            return Ok(None);
        };
        if let Some(statement) = self.lower_uvm_phase_objection_call(call)? {
            return Ok(Some(statement));
        }
        if let Some(DynamicClassArraySelection {
            selector, elements, ..
        }) = self.dynamic_class_array_selection(receiver)?
        {
            let mut statements = Vec::new();
            for (index, element_key) in elements {
                let mut specialized = call.clone();
                if let Some(fields) = specialized.as_object_mut() {
                    fields.insert(
                        "thisClass".into(),
                        Self::class_array_element_receiver(&element_key, receiver),
                    );
                }
                let Some(statement) = self.lower_class_task_call(&specialized)? else {
                    continue;
                };
                statements.push(Stmt::If {
                    c: Self::class_array_condition(&selector, index),
                    t: Box::new(statement),
                    f: None,
                });
            }
            return Ok(Some(Stmt::Block(statements)));
        }
        let Some(object_key) = self.class_object_key_for_value(receiver)? else {
            return Ok(None);
        };
        if !self.is_class_object_key(&object_key) {
            return Ok(None);
        }
        self.require_initialized_class_object(&object_key)?;
        if let Some(statement) = self.lower_dynamic_class_task(&object_key, call)? {
            return Ok(Some(statement));
        }
        self.lower_class_method(&object_key, call, false).map(Some)
    }

    pub(super) fn lower_class_cast_call(&self, call: &Value) -> Result<Option<Stmt>, String> {
        let Some((type_matches, copy)) = self.class_cast_plan(call)? else {
            return Ok(None);
        };
        Ok(Some(Stmt::If {
            c: type_matches,
            t: Box::new(Stmt::Block(copy)),
            f: None,
        }))
    }

    pub(super) fn lower_class_cast_conditional(
        &self,
        statement: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(condition) = statement
            .get("conditions")
            .and_then(Value::as_array)
            .and_then(|conditions| conditions.first())
            .and_then(|condition| condition.get("expr"))
        else {
            return Ok(None);
        };
        let (condition, negated) = if string_field(condition, "kind").as_deref() == Some("UnaryOp")
            && string_field(condition, "op").as_deref() == Some("LogicalNot")
            && condition.get("operand").is_some_and(is_class_cast_call)
        {
            (
                condition
                    .get("operand")
                    .ok_or_else(|| "logical-not expression has no operand".to_string())?,
                true,
            )
        } else if is_class_cast_call(condition) {
            (condition, false)
        } else {
            return Ok(None);
        };
        let Some((type_matches, copy)) = self.class_cast_plan(condition)? else {
            return Ok(None);
        };
        let if_true = statement
            .get("ifTrue")
            .ok_or_else(|| "conditional statement has no then branch".to_string())?;
        let if_false = statement.get("ifFalse");
        let original_then = self.lower_statement(if_true)?;
        let original_else = match if_false {
            Some(if_false) => Some(self.lower_statement(if_false)?),
            None => None,
        };
        let (lowered_then, lowered_else) = if negated {
            (
                original_else.map(|body| {
                    let mut statements = copy;
                    statements.push(body);
                    Stmt::Block(statements)
                }),
                Some(original_then),
            )
        } else {
            (
                Some({
                    let mut statements = copy;
                    statements.push(original_then);
                    Stmt::Block(statements)
                }),
                original_else,
            )
        };
        Ok(Some(Stmt::If {
            c: type_matches,
            t: Box::new(lowered_then.unwrap_or(Stmt::Block(Vec::new()))),
            f: lowered_else.map(Box::new),
        }))
    }

    fn class_cast_plan(&self, call: &Value) -> Result<Option<(Expr, Vec<Stmt>)>, String> {
        if !is_class_cast_call(call) {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 2 {
            return Err("$cast requires exactly two class-handle arguments".into());
        }
        let target = arguments[0]
            .get("left")
            .ok_or_else(|| "$cast destination must be an assignable class handle".to_string())?;
        let source = conversion_operand(&arguments[1]).unwrap_or(&arguments[1]);
        let Some(target_key) = self.class_object_key_for_value(target)? else {
            return Err("$cast destination is not a prepared class object".into());
        };
        let source_key = if is_class_child_lookup_call(source) {
            self.class_child_lookup_key(source)?
        } else if is_class_parent_call(source) {
            let Some(receiver) = source.get("thisClass") else {
                return Err("class get_parent call has no receiver".into());
            };
            let Some(child_key) = self.class_object_key_for_value(receiver)? else {
                return Err("class get_parent receiver is not a prepared class object".into());
            };
            self.require_initialized_class_object(&child_key)?;
            self.class_object_parent_keys
                .borrow()
                .get(&child_key)
                .cloned()
                .ok_or_else(|| format!("class object '{child_key}' has no prepared parent"))?
        } else {
            let Some(source_key) = self.class_object_key_for_value(source)? else {
                return Err("$cast source is not a prepared class object".into());
            };
            source_key
        };
        self.require_initialized_class_object(&source_key)?;
        if !self.class_objects.contains_key(&target_key)
            || !self.class_objects.contains_key(&source_key)
        {
            return Err("$cast source and destination must have prepared storage".into());
        }
        let target_fields = self.class_objects[&target_key].clone();
        let source_fields = self.class_objects[&source_key].clone();
        if target_fields
            .keys()
            .any(|field| !source_fields.contains_key(field))
        {
            return Err(
                "$cast destination has fields absent from the prepared source storage".into(),
            );
        }
        let target_type = self.class_object_types[&target_key].clone();
        let source_type = self.class_object_types[&source_key].clone();
        let mut possible_source_types = vec![source_type.clone()];
        possible_source_types.extend(
            self.dynamic_sources_for(&source_key)
                .into_iter()
                .map(|source| source.class_name),
        );
        if !possible_source_types.iter().any(|source_type| {
            self.class_chain(source_type)
                .is_ok_and(|chain| chain.iter().any(|(owner, _)| *owner == target_type))
        }) {
            return Err(format!(
                "$cast source '{source_key}' has no possible '{target_type}' object"
            ));
        }
        let acceptable_classes = self
            .class_definitions
            .keys()
            .filter(|candidate| {
                self.class_chain(candidate)
                    .is_ok_and(|chain| chain.iter().any(|(owner, _)| *owner == target_type))
            })
            .cloned()
            .collect::<Vec<_>>();
        if acceptable_classes.is_empty() {
            return Err(format!("$cast target class '{target_type}' is unknown"));
        }
        let source_tag = self
            .class_object_tags
            .get(&source_key)
            .cloned()
            .ok_or_else(|| format!("class object '{source_key}' has no runtime tag"))?;
        let mut type_matches = None;
        for class_name in &acceptable_classes {
            let class_id = self.class_id(class_name)?;
            let matches = Expr::Bin {
                op: BinOp::Eq,
                a: Box::new(class_expr(&source_tag)),
                b: Box::new(Expr::Const(LogicWord::from_u64(32, class_id))),
            };
            type_matches = Some(match type_matches {
                Some(previous) => Expr::Bin {
                    op: BinOp::LogOr,
                    a: Box::new(previous),
                    b: Box::new(matches),
                },
                None => matches,
            });
        }
        let type_matches = type_matches.expect("acceptable $cast class list is nonempty");
        self.record_runtime_dynamic_source(&target_key, &source_key, &source_type)?;
        let mut copy = Vec::new();
        for (target_symbol, source_symbol) in [
            (
                self.class_object_tags[&target_key].clone(),
                self.class_object_tags[&source_key].clone(),
            ),
            (
                self.class_object_ids[&target_key].clone(),
                self.class_object_ids[&source_key].clone(),
            ),
            (
                self.class_object_names[&target_key].clone(),
                self.class_object_names[&source_key].clone(),
            ),
            (
                self.class_object_full_names[&target_key].clone(),
                self.class_object_full_names[&source_key].clone(),
            ),
            (
                self.class_object_parent_ids[&target_key].clone(),
                self.class_object_parent_ids[&source_key].clone(),
            ),
        ] {
            copy.push(Stmt::Bl {
                lv: class_lvalue(&target_symbol),
                e: class_expr(&source_symbol),
            });
        }
        for (field_name, target_symbol) in target_fields {
            let Some(source_symbol) = source_fields.get(&field_name) else {
                continue;
            };
            copy.push(Stmt::Bl {
                lv: class_lvalue(&target_symbol),
                e: class_expr(source_symbol),
            });
        }
        Ok(Some((type_matches, copy)))
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
        if let Some(DynamicClassArraySelection {
            selector, elements, ..
        }) = self.dynamic_class_array_selection(receiver)?
        {
            let mut candidates = Vec::new();
            for (index, element_key) in elements {
                let mut specialized = call.clone();
                if let Some(fields) = specialized.as_object_mut() {
                    fields.insert(
                        "thisClass".into(),
                        Self::class_array_element_receiver(&element_key, receiver),
                    );
                }
                let Some(expression) = self.lower_class_function_call(&specialized)? else {
                    continue;
                };
                candidates.push((index, expression));
            }
            let Some((_, result)) = candidates.last() else {
                return Ok(None);
            };
            let result_width = result.width();
            let mut result = Expr::Const(LogicWord::xes(result_width));
            for (index, expression) in candidates.iter().rev() {
                result = Expr::Cond {
                    c: Box::new(Self::class_array_condition(&selector, *index)),
                    t: Box::new(expression.clone()),
                    f: Box::new(result),
                };
            }
            return Ok(Some(result));
        }
        let Some(object_key) = self.class_object_key_for_value(receiver)? else {
            return Ok(None);
        };
        if !self.is_class_object_key(&object_key) {
            return Ok(None);
        }
        self.require_initialized_class_object(&object_key)?;
        let method_name = string_field(call, "subroutine")
            .and_then(|symbol| symbol.split_whitespace().next_back().map(str::to_string))
            .unwrap_or_default();
        let empty_call_arguments = Vec::new();
        let call_arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_call_arguments);
        if matches!(method_name.as_str(), "get_name" | "get_full_name") && call_arguments.is_empty()
        {
            let name = if method_name == "get_full_name" {
                self.class_object_full_names
                    .get(&object_key)
                    .cloned()
                    .ok_or_else(|| {
                        format!("class object '{object_key}' has no full-name storage")
                    })?
            } else {
                self.class_object_names
                    .get(&object_key)
                    .cloned()
                    .ok_or_else(|| format!("class object '{object_key}' has no name storage"))?
            };
            return Ok(Some(class_expr(&name)));
        }
        let Some((method, owner)) = self.class_method(&object_key, call)? else {
            return Ok(None);
        };
        if method_return_type(&method) == "void" {
            return Ok(None);
        }
        if let Some(expression) = self.lower_dynamic_class_function(&object_key, call)? {
            return Ok(Some(expression));
        }
        let body = self.specialized_method_body(&object_key, &owner, &method, call)?;
        if string_field(&body, "kind").as_deref() != Some("Return") {
            return Err("class function must have a single direct return expression".into());
        }
        let expression = body
            .get("expr")
            .ok_or_else(|| "class function return has no expression".to_string())?;
        self.lower_expression(expression).map(Some)
    }

    fn lower_class_method_statement(&self, object_key: &str, body: &Value) -> Result<Stmt, String> {
        self.active_class_objects
            .borrow_mut()
            .push(object_key.to_string());
        let result = self.lower_statement(body);
        self.active_class_objects.borrow_mut().pop();
        result
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
        let source = conversion_operand(right).unwrap_or(right);
        let target_selection = self.dynamic_class_array_selection(left)?;
        let target_fixed_key = if target_selection.is_none() {
            self.class_object_key_for_value(left)?
        } else {
            None
        };
        if target_selection.is_none() && target_fixed_key.is_none() {
            return Ok(None);
        }
        let source_selection = self.dynamic_class_array_selection(source)?;
        let source_fixed_key = if source_selection.is_none() {
            self.class_object_key_for_value(source)?
        } else {
            None
        };
        if bool_field(assignment, "isNonBlocking") {
            return Err("non-blocking class handle assignment is not supported".into());
        }

        if let Some(target_selection) = target_selection {
            let target_statements = target_selection
                .elements
                .iter()
                .map(|(target_index, target_key)| {
                    let target_condition =
                        Self::class_array_condition(&target_selection.selector, *target_index);
                    let body = if let Some(source_selection) = &source_selection {
                        Stmt::Block(
                            source_selection
                                .elements
                                .iter()
                                .map(|(source_index, source_key)| {
                                    self.require_initialized_class_object(source_key)?;
                                    Ok(Stmt::If {
                                        c: Self::class_array_condition(
                                            &source_selection.selector,
                                            *source_index,
                                        ),
                                        t: Box::new(
                                            self.class_handle_copy_statement(
                                                target_key, source_key,
                                            )?,
                                        ),
                                        f: None,
                                    })
                                })
                                .collect::<Result<Vec<_>, String>>()?,
                        )
                    } else if let Some(source_key) = &source_fixed_key {
                        self.require_initialized_class_object(source_key)?;
                        self.class_handle_copy_statement(target_key, source_key)?
                    } else {
                        return Err(
                            "class handle assignment requires a constructed source handle".into(),
                        );
                    };
                    Ok(Stmt::If {
                        c: target_condition,
                        t: Box::new(body),
                        f: None,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            return Ok(Some(Stmt::Block(target_statements)));
        }

        let Some(target_key) = target_fixed_key else {
            return Ok(None);
        };
        if let Some(source_selection) = source_selection {
            let statements = source_selection
                .elements
                .iter()
                .map(|(source_index, source_key)| {
                    self.require_initialized_class_object(source_key)?;
                    Ok(Stmt::If {
                        c: Self::class_array_condition(&source_selection.selector, *source_index),
                        t: Box::new(self.class_handle_copy_statement(&target_key, source_key)?),
                        f: None,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            return Ok(Some(Stmt::Block(statements)));
        }

        let Some(source_key) = source_fixed_key else {
            return Err(
                "class handle assignment supports constructor and object-handle sources only"
                    .into(),
            );
        };
        self.require_initialized_class_object(&source_key)?;
        self.class_handle_copy_statement(&target_key, &source_key)
            .map(Some)
    }

    pub(super) fn lower_class_parent_assignment(
        &self,
        assignment: &Value,
        left: &Value,
        right: &Value,
    ) -> Result<Option<Stmt>, String> {
        if !is_class_parent_call(right) {
            return Ok(None);
        }
        let Some(receiver) = right.get("thisClass") else {
            return Err("class get_parent call has no receiver".into());
        };
        let Some(child_key) = self.class_object_key_for_value(receiver)? else {
            return Err("class get_parent receiver is not a prepared class object".into());
        };
        self.require_initialized_class_object(&child_key)?;
        let Some(parent_key) = self
            .class_object_parent_keys
            .borrow()
            .get(&child_key)
            .cloned()
        else {
            return Err(format!("class object '{child_key}' has no prepared parent"));
        };
        let Some(target_key) = self.class_object_key_for_value(left)? else {
            return Err("class parent assignment target is not a prepared class object".into());
        };
        self.class_parent_copy_statement(&target_key, &parent_key, assignment)
            .map(Some)
    }

    fn class_parent_copy_statement(
        &self,
        target_key: &str,
        source_key: &str,
        assignment: &Value,
    ) -> Result<Stmt, String> {
        if !self.class_objects.contains_key(target_key)
            || !self.class_objects.contains_key(source_key)
        {
            return Err(format!(
                "cannot copy unknown class parent '{target_key}' from '{source_key}'"
            ));
        }
        if bool_field(assignment, "isNonBlocking") {
            return Err("non-blocking class parent assignment is not supported".into());
        }
        let target_fields = self.class_objects[target_key].clone();
        let source_fields = self.class_objects[source_key].clone();
        let target_type = self.class_object_types[target_key].clone();
        let source_type = self.class_object_types[source_key].clone();
        let compatible = self.class_chain(&source_type)?.iter().any(|(owner, _)| {
            owner == &target_type
                || owner.split('@').next()
                    == Some(target_type.split('@').next().unwrap_or(&target_type))
        });
        if !compatible {
            return Err(format!(
                "class parent source '{source_type}' is not compatible with target '{target_type}'"
            ));
        }
        self.record_runtime_dynamic_source(target_key, source_key, &source_type)?;
        let mut statements = Vec::new();
        for (target_symbol, source_symbol) in [
            (
                self.class_object_tags[target_key].clone(),
                self.class_object_tags[source_key].clone(),
            ),
            (
                self.class_object_ids[target_key].clone(),
                self.class_object_ids[source_key].clone(),
            ),
            (
                self.class_object_names[target_key].clone(),
                self.class_object_names[source_key].clone(),
            ),
            (
                self.class_object_full_names[target_key].clone(),
                self.class_object_full_names[source_key].clone(),
            ),
            (
                self.class_object_parent_ids[target_key].clone(),
                self.class_object_parent_ids[source_key].clone(),
            ),
        ] {
            statements.push(Stmt::Bl {
                lv: class_lvalue(&target_symbol),
                e: class_expr(&source_symbol),
            });
        }
        for (field_name, target_symbol) in target_fields {
            let Some(source_symbol) = source_fields.get(&field_name) else {
                continue;
            };
            statements.push(Stmt::Bl {
                lv: class_lvalue(&target_symbol),
                e: class_expr(source_symbol),
            });
        }
        Ok(Stmt::Block(statements))
    }

    fn class_handle_copy_statement(
        &self,
        target_key: &str,
        source_key: &str,
    ) -> Result<Stmt, String> {
        if !self.class_objects.contains_key(target_key)
            || !self.class_objects.contains_key(source_key)
        {
            return Err(format!(
                "cannot copy unknown class handle '{target_key}' from '{source_key}'"
            ));
        }
        let target_fields = &self.class_objects[target_key];
        let source_fields = &self.class_objects[source_key];
        let target_type = &self.class_object_types[target_key];
        let source_type = &self.class_object_types[source_key];
        if target_type == source_type {
            self.record_runtime_dynamic_source(target_key, source_key, source_type)?;
        } else if !self
            .dynamic_class_sources
            .get(target_key)
            .is_some_and(|sources| sources.iter().any(|source| source.object_key == source_key))
        {
            return Err(format!(
                "class handle assignment from '{source_type}' to '{target_type}' requires pre-declared dynamic storage"
            ));
        }
        let mut statements = Vec::new();
        for (target_symbol, source_symbol) in [
            (
                &self.class_object_tags[target_key],
                &self.class_object_tags[source_key],
            ),
            (
                &self.class_object_ids[target_key],
                &self.class_object_ids[source_key],
            ),
            (
                &self.class_object_names[target_key],
                &self.class_object_names[source_key],
            ),
            (
                &self.class_object_full_names[target_key],
                &self.class_object_full_names[source_key],
            ),
            (
                &self.class_object_parent_ids[target_key],
                &self.class_object_parent_ids[source_key],
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
        Ok(Stmt::Block(statements))
    }

    pub(super) fn lower_class_property_assignment(
        &self,
        assignment: &Value,
        left: &Value,
        right: &Value,
    ) -> Result<Option<Stmt>, String> {
        if string_field(left, "kind").as_deref() == Some("MemberAccess") {
            if let Some(receiver) = left.get("value") {
                if let Some(DynamicClassArraySelection {
                    selector, elements, ..
                }) = self.dynamic_class_array_selection(receiver)?
                {
                    let field_name = string_field(left, "member")
                        .and_then(|member| {
                            member.split_whitespace().next_back().map(str::to_string)
                        })
                        .ok_or_else(|| "class property target has no member name".to_string())?;
                    let Some((_, first_key)) = elements.first() else {
                        return Ok(None);
                    };
                    self.require_initialized_class_object(first_key)?;
                    let expression = self.lower_expression(right)?;
                    let nonblocking = bool_field(assignment, "isNonBlocking");
                    let temporary = self
                        .class_property_temps
                        .get(&(first_key.clone(), field_name.clone()))
                        .ok_or_else(|| {
                            format!("class property '{first_key}.{field_name}' has no temporary")
                        })?
                        .clone();
                    let sampled_expression = class_expr(&temporary);
                    let mut statements = vec![Stmt::Bl {
                        lv: class_lvalue(&temporary),
                        e: expression,
                    }];

                    for (index, element_key) in &elements {
                        self.require_initialized_class_object(element_key)?;
                        let Some(fields) = self.class_objects.get(element_key) else {
                            continue;
                        };
                        let Some(symbol) = fields.get(&field_name) else {
                            continue;
                        };
                        let mut body = vec![class_field_assignment(
                            symbol,
                            &sampled_expression,
                            nonblocking,
                        )];
                        for related_key in self.related_class_handles(element_key) {
                            let Some(related_fields) = self.class_objects.get(&related_key) else {
                                continue;
                            };
                            let Some(related_symbol) = related_fields.get(&field_name) else {
                                continue;
                            };
                            body.push(Stmt::If {
                                c: self.class_alias_condition(element_key, &related_key),
                                t: Box::new(class_field_assignment(
                                    related_symbol,
                                    &sampled_expression,
                                    nonblocking,
                                )),
                                f: None,
                            });
                        }
                        statements.push(Stmt::If {
                            c: Self::class_array_condition(&selector, *index),
                            t: Box::new(Stmt::Block(body)),
                            f: None,
                        });
                    }
                    return Ok(Some(Stmt::Block(statements)));
                }
            }
        }
        let Some((object_key, field_name, symbol)) = self.class_property_target(left)? else {
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
            if !self.class_object_ids.contains_key(&related_key) {
                continue;
            }
            let condition = self.class_alias_condition(&object_key, &related_key);
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
    ) -> Result<Option<(String, String, super::AstLoweredSymbol)>, String> {
        if string_field(value, "kind").as_deref() == Some("NamedValue") {
            let Some(symbol_name) = string_field(value, "symbol") else {
                return Ok(None);
            };
            if !symbol_name.starts_with("eda_class_") {
                return Ok(None);
            }
            let Some(symbol) = self.symbols.get(&symbol_name) else {
                return Ok(None);
            };
            let Some((object_key, field_name)) =
                self.class_property_owners.get(&symbol.base).cloned()
            else {
                return Ok(None);
            };
            let symbol = symbol.clone();
            return Ok(Some((object_key, field_name, symbol)));
        }
        if string_field(value, "kind").as_deref() != Some("MemberAccess") {
            return Ok(None);
        }
        let Some(receiver) = value.get("value") else {
            return Ok(None);
        };
        let Some(object_key) = self.class_object_key_for_value(receiver)? else {
            return Ok(None);
        };
        self.require_class_object_key(&object_key).ok_or_else(|| {
            format!("class property target has unknown class object '{object_key}'")
        })?;
        let field_name = string_field(value, "member")
            .and_then(|member| member.split_whitespace().next_back().map(str::to_string))
            .ok_or_else(|| "class property target has no member name".to_string())?;
        let Some(fields) = self.class_objects.get(&object_key) else {
            return Ok(None);
        };
        let Some(symbol) = fields.get(&field_name) else {
            return Ok(None);
        };
        let symbol = symbol.clone();
        Ok(Some((object_key, field_name, symbol)))
    }

    fn related_class_handles(&self, object_key: &str) -> Vec<String> {
        let mut visited = std::collections::BTreeSet::from([object_key.to_string()]);
        let mut queue = std::collections::VecDeque::from([object_key.to_string()]);
        while let Some(current) = queue.pop_front() {
            let mut neighbors = Vec::new();
            neighbors.extend(
                self.dynamic_sources_for(&current)
                    .iter()
                    .map(|source| source.object_key.clone()),
            );
            let runtime_sources = self.runtime_dynamic_class_sources.borrow();
            for (candidate, sources) in runtime_sources.iter().chain(&self.dynamic_class_sources) {
                if candidate != &current
                    && sources.iter().any(|source| source.object_key == current)
                {
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

    fn class_alias_condition(&self, object_key: &str, related_key: &str) -> Expr {
        let object_id = &self.class_object_ids[object_key];
        let object_tag = &self.class_object_tags[object_key];
        let related_id = &self.class_object_ids[related_key];
        let related_tag = &self.class_object_tags[related_key];
        Expr::Bin {
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
        }
    }

    fn require_initialized_class_object(&self, object_key: &str) -> Result<(), String> {
        if self.initialized_class_objects.contains(object_key)
            || !self.dynamic_sources_for(object_key).is_empty()
        {
            Ok(())
        } else if let Some(parent_key) = self.class_composition_parents.get(object_key).cloned() {
            self.require_initialized_class_object(&parent_key)
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
        if let Some(DynamicClassArraySelection {
            selector, elements, ..
        }) = self.dynamic_class_array_selection(receiver)?
        {
            let mut candidates = Vec::new();
            for (index, element_key) in elements {
                let mut specialized = value.clone();
                if let Some(fields) = specialized.as_object_mut() {
                    fields.insert(
                        "value".into(),
                        Self::class_array_element_receiver(&element_key, receiver),
                    );
                }
                let Some(expression) = self.lower_class_property_access(&specialized)? else {
                    continue;
                };
                candidates.push((index, expression));
            }
            let Some((_, result)) = candidates.last() else {
                return Ok(None);
            };
            let result_width = result.width();
            let mut result = Expr::Const(LogicWord::xes(result_width));
            for (index, expression) in candidates.iter().rev() {
                result = Expr::Cond {
                    c: Box::new(Self::class_array_condition(&selector, *index)),
                    t: Box::new(expression.clone()),
                    f: Box::new(result),
                };
            }
            return Ok(Some(result));
        }
        let Some(object_key) = self.class_object_key_for_value(receiver)? else {
            return Ok(None);
        };
        if !self.is_class_object_key(&object_key) {
            return Ok(None);
        }
        self.require_initialized_class_object(&object_key)?;
        let Some(field_name) = string_field(value, "member")
            .and_then(|member| member.split_whitespace().next_back().map(str::to_string))
        else {
            return Ok(None);
        };
        let Some(fields) = self.class_objects.get(&object_key) else {
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
        self.lower_class_method_statement(object_key, &body)
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

    pub(super) fn resolve_class_method(
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
        let sources = self.dynamic_sources_for(object_key);
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
        self.lower_class_method_statement(object_key, &body)
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

    pub(super) fn class_chain(&self, start: &str) -> Result<Vec<(String, Value)>, String> {
        let mut chain = Vec::new();
        let mut current = start.to_string();
        loop {
            if chain.iter().any(|(name, _)| name == &current) {
                return Err(format!("class inheritance cycle at '{current}'"));
            }
            let Some(class) = self.class_definitions.get(&current).cloned() else {
                return Err(format!("unknown class '{current}'"));
            };
            let base_reference = class
                .get("baseClass")
                .and_then(Value::as_str)
                .map(str::to_string);
            let base = base_reference.as_deref().and_then(class_name);
            chain.push((current, class));
            match base {
                Some(_) => {
                    current = self.class_type_key(
                        base_reference
                            .as_deref()
                            .expect("class base reference was just checked"),
                    )
                }
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
        let synthetic_prefix = object_key
            .split_once('[')
            .and_then(|(_, suffix)| suffix.split(']').next())
            .map(|index| format!("{handle_address}_{index}"))
            .unwrap_or_else(|| handle_address.to_string());
        let empty_members = Vec::new();
        for (_, class) in self.class_chain(class_name)? {
            for property in class
                .get("members")
                .and_then(Value::as_array)
                .unwrap_or(&empty_members)
            {
                if string_field(property, "kind").as_deref() == Some("Parameter") {
                    if let Some(value) = string_field(property, "value") {
                        let parameter_name = declaration_name(property, "class Parameter")?;
                        let parameter_address = property
                            .get("addr")
                            .and_then(Value::as_u64)
                            .ok_or_else(|| {
                                format!("class parameter '{parameter_name}' has no address")
                            })?;
                        replacements.insert(
                            format!("{parameter_address} {parameter_name}"),
                            json!({
                                "kind": "IntegerLiteral",
                                "type": string_field(property, "type").unwrap_or_default(),
                                "value": value,
                            }),
                        );
                    }
                    continue;
                }
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
                if let Some(child_key) = self
                    .class_composition_children
                    .get(&(object_key.to_string(), property_name.clone()))
                {
                    replacements.insert(
                        format!("{property_address} {property_name}"),
                        json!({
                            "kind": "NamedValue",
                            "type": string_field(property, "type").unwrap_or_default(),
                            "symbol": child_key,
                        }),
                    );
                    continue;
                }
                let synthetic = format!("eda_class_{synthetic_prefix}_{property_address}");
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
                if class_name(&base).as_deref() == Some("uvm_test") {
                    return Ok(json!({"kind": "Empty"}));
                }
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

    fn class_handle_candidates_from_ast(
        &self,
        value: &Value,
    ) -> Result<Option<ClassHandleCandidates>, String> {
        match string_field(value, "kind").as_deref() {
            Some("NamedValue" | "HierarchicalValue") => {
                let Some(type_name) = string_field(value, "type") else {
                    return Ok(None);
                };
                let class_name = self.class_type_key(&type_name);
                if !self.class_definitions.contains_key(&class_name) {
                    return Ok(None);
                }
                let Some(key) = string_field(value, "symbol") else {
                    return Ok(None);
                };
                Ok(Some(ClassHandleCandidates {
                    keys: vec![key],
                    class_name,
                }))
            }
            Some("ElementSelect") => {
                let Some(selected) = value.get("value") else {
                    return Ok(None);
                };
                let Some(base_key) = string_field(selected, "symbol") else {
                    return Ok(None);
                };
                let Some(type_name) = string_field(selected, "type") else {
                    return Ok(None);
                };
                let class_name = self.class_type_key(&type_name);
                if !self.class_definitions.contains_key(&class_name) {
                    return Ok(None);
                }
                let indexes = class_array_indexes(&type_name).unwrap_or_default();
                let Some(selector) = value.get("selector") else {
                    return Ok(None);
                };
                let keys = if let Some(selected_index) = constant_selector(selector) {
                    indexes
                        .into_iter()
                        .flatten()
                        .filter(|index| *index == selected_index)
                        .map(|index| format!("{base_key}[{index}]"))
                        .collect()
                } else {
                    indexes
                        .into_iter()
                        .flatten()
                        .map(|index| format!("{base_key}[{index}]"))
                        .collect()
                };
                Ok(Some(ClassHandleCandidates { keys, class_name }))
            }
            _ => Ok(None),
        }
    }
}

fn class_name(type_name: &str) -> Option<String> {
    let raw_name = type_name.split_whitespace().next_back()?;
    let base_name = raw_name.split(['#', '$']).next()?.trim();
    (!base_name.is_empty()).then(|| base_name.to_string())
}

fn class_array_indexes(type_name: &str) -> Option<Vec<Option<i64>>> {
    let start = type_name.find("$[")? + 2;
    let end = type_name.rfind(']')?;
    if end <= start {
        return None;
    }
    let range = &type_name[start..end];
    let (left, right) = range.split_once(':')?;
    let left = left.trim().parse::<i64>().ok()?;
    let right = right.trim().parse::<i64>().ok()?;
    if left <= right {
        Some((left..=right).map(Some).collect())
    } else {
        Some((right..=left).rev().map(Some).collect())
    }
}

fn constant_selector(value: &Value) -> Option<i64> {
    string_field(value, "constant")
        .or_else(|| string_field(value, "value"))
        .and_then(|raw| raw.trim().parse::<i64>().ok())
}

pub(super) fn method_return_type(method: &Value) -> String {
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

fn conversion_operand(value: &Value) -> Option<&Value> {
    if string_field(value, "kind").as_deref() != Some("Conversion") {
        return None;
    }
    value.get("operand")
}

fn is_class_cast_call(value: &Value) -> bool {
    string_field(value, "kind").as_deref() == Some("Call")
        && string_field(value, "subroutine").as_deref() == Some("$cast")
}

fn is_class_parent_call(value: &Value) -> bool {
    string_field(value, "kind").as_deref() == Some("Call")
        && string_field(value, "subroutine")
            .and_then(|subroutine| {
                subroutine
                    .split_whitespace()
                    .next_back()
                    .map(str::to_string)
            })
            .as_deref()
            == Some("get_parent")
        && value
            .get("arguments")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
}

fn is_class_child_lookup_call(value: &Value) -> bool {
    string_field(value, "kind").as_deref() == Some("Call")
        && string_field(value, "subroutine")
            .and_then(|subroutine| {
                subroutine
                    .split_whitespace()
                    .next_back()
                    .map(str::to_string)
            })
            .as_deref()
            == Some("get_child_by_name")
        && value
            .get("arguments")
            .and_then(Value::as_array)
            .is_some_and(|arguments| arguments.len() == 1)
}

fn is_this_reference(value: &Value) -> bool {
    string_field(value, "kind").as_deref() == Some("NamedValue")
        && string_field(value, "symbol")
            .and_then(|symbol| symbol.split_whitespace().next_back().map(str::to_string))
            .as_deref()
            == Some("this")
}

pub(super) fn class_expr(symbol: &super::AstLoweredSymbol) -> Expr {
    Expr::Ref {
        net: symbol.base,
        decl_width: symbol.width,
        range: super::hierarchy::full_range(symbol.width),
        signed: symbol.signed,
    }
}

pub(super) fn class_lvalue(symbol: &super::AstLoweredSymbol) -> eda_core::ir::LValue {
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
