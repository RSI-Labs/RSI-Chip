//! Direct lowering for a small non-virtual class-object subset.

use super::{declaration_name, SlangAstLowerer};
use crate::ast_util::{bool_field, string_field};
use serde_json::{json, Value};
use std::collections::BTreeMap;

impl SlangAstLowerer {
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
        let handle_address = member
            .get("addr")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("class object '{name}' has no symbol address"))?;
        let empty_members = Vec::new();
        let mut fields = BTreeMap::new();
        for (owner_name, class) in chain.iter().rev() {
            let members = class
                .get("members")
                .and_then(Value::as_array)
                .unwrap_or(&empty_members);
            for property in members {
                if string_field(property, "kind").as_deref() != Some("ClassProperty") {
                    continue;
                }
                let property_name = declaration_name(property, "ClassProperty")?;
                if fields.contains_key(&property_name) {
                    return Err(format!(
                    "class property shadowing '{class_name}.{property_name}' from '{owner_name}' is not supported"
                ));
                }
                if property.get("initializer").is_some() {
                    return Err(format!(
                    "class property '{class_name}.{property_name}' initializers are not supported; initialize in new()"
                ));
                }
                if string_field(property, "lifetime").as_deref() == Some("Static") {
                    return Err(format!(
                        "static class property '{class_name}.{property_name}' is not supported"
                    ));
                }
                let property_address =
                    property
                        .get("addr")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| {
                            format!("class property '{class_name}.{property_name}' has no address")
                        })?;
                let field_name = format!("{name}.{property_name}");
                let symbol = self.declare_net(property, &field_name, "Variable")?;
                let synthetic_symbol = format!("eda_class_{handle_address}_{property_address}");
                self.symbols
                    .insert(synthetic_symbol.clone(), symbol.clone());
                fields.insert(property_name, symbol);
            }
        }
        self.class_objects
            .insert(format!("{handle_address} {name}"), fields);
        self.class_object_types
            .insert(format!("{handle_address} {name}"), class_name);
        Ok(true)
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
        self.lower_class_method(object_key, constructor, true)
            .map(Some)
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
        self.lower_class_method(object_key, call, false).map(Some)
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
        let Some((method, owner)) = self.class_method(object_key, call)? else {
            return Ok(None);
        };
        if method_return_type(&method) == "void" {
            return Ok(None);
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

    fn receiver_class_name(&self, object_key: &str) -> Option<String> {
        self.class_object_types.get(object_key).cloned()
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
