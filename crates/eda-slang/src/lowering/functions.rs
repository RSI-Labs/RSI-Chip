//! Straight-line function inlining from slang's elaborated AST.

use super::{declaration_name, SlangAstLowerer};
use crate::ast_util::{expression_symbol, string_field};
use serde_json::{json, Value};

impl SlangAstLowerer {
    pub(super) fn collect_subroutine(&mut self, member: &Value) -> Result<(), String> {
        let name = declaration_name(member, "Subroutine")?;
        if self.subroutines.contains_key(&name) {
            return Err(format!("duplicate slang subroutine '{name}'"));
        }
        self.subroutines.insert(name, member.clone());
        Ok(())
    }

    pub(super) fn lower_function_call(&self, value: &Value) -> Result<eda_core::ir::Expr, String> {
        let Some(subroutine) = string_field(value, "subroutine") else {
            return Err("function call has no subroutine symbol".into());
        };
        let subroutine_name = expression_symbol(value)
            .or_else(|| {
                subroutine
                    .split_whitespace()
                    .next_back()
                    .map(str::to_string)
            })
            .ok_or_else(|| "function call has no subroutine name".to_string())?;
        let definition = self
            .subroutines
            .get(&subroutine_name)
            .ok_or_else(|| format!("unknown direct-lowering function '{subroutine_name}'"))?;
        if string_field(definition, "subroutineKind").as_deref() != Some("Function") {
            return Err(format!(
                "direct slang-AST lowering does not support task call '{subroutine_name}'"
            ));
        }

        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("function call '{subroutine_name}' has no arguments"))?;
        let formals = definition
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("function '{subroutine_name}' has no formal arguments"))?;
        if arguments.len() != formals.len() {
            return Err(format!(
                "function '{subroutine_name}' expects {} arguments, found {}",
                formals.len(),
                arguments.len()
            ));
        }

        let mut environment = std::collections::BTreeMap::new();
        for (formal, actual) in formals.iter().zip(arguments) {
            let formal_name = formal
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("function '{subroutine_name}' has an unnamed argument"))?;
            let formal_type = formal.get("type").and_then(Value::as_str).ok_or_else(|| {
                format!("function argument '{formal_name}' has no elaborated type")
            })?;
            environment.insert(
                formal_name.to_string(),
                json!({
                    "kind": "Conversion",
                    "type": formal_type,
                    "operand": actual,
                }),
            );
        }

        let function_name = declaration_name(definition, "Subroutine")?;
        {
            let mut stack = self.inline_stack.borrow_mut();
            if stack.iter().any(|name| name == &function_name) {
                return Err(format!(
                    "recursive function '{function_name}' is not supported"
                ));
            }
            stack.push(function_name.clone());
        }

        let body = definition
            .get("body")
            .ok_or_else(|| format!("function '{function_name}' has no body"))?;
        let return_type = string_field(definition, "returnType")
            .ok_or_else(|| format!("function '{function_name}' has no return type"))?;
        let result = match self.inline_function_body(body, &mut environment, &function_name) {
            Ok(return_expression) => {
                let converted = json!({
                    "kind": "Conversion",
                    "type": return_type,
                    "operand": return_expression,
                });
                self.lower_expression(&converted)
            }
            Err(error) => Err(error),
        };
        self.inline_stack.borrow_mut().pop();
        result
    }

    fn inline_function_body(
        &self,
        value: &Value,
        environment: &mut std::collections::BTreeMap<String, Value>,
        function_name: &str,
    ) -> Result<Value, String> {
        if let Some(result) = self.inline_function_statement(value, environment)? {
            return Ok(result);
        }
        environment.get(function_name).cloned().ok_or_else(|| {
            format!("function '{function_name}' has no return value on a control path")
        })
    }

    fn inline_function_statement(
        &self,
        value: &Value,
        environment: &mut std::collections::BTreeMap<String, Value>,
    ) -> Result<Option<Value>, String> {
        match string_field(value, "kind").as_deref() {
            Some("List") => {
                let Some(items) = value.get("list").and_then(Value::as_array) else {
                    return Err("function statement list has no items".into());
                };
                for item in items {
                    if let Some(result) = self.inline_function_statement(item, environment)? {
                        return Ok(Some(result));
                    }
                }
                Ok(None)
            }
            Some("Block") => {
                let Some(body) = value.get("body") else {
                    return Err("function block has no body".into());
                };
                self.inline_function_statement(body, environment)
            }
            Some("VariableDeclaration") => Ok(None),
            Some("ExpressionStatement") => {
                let Some(expression) = value.get("expr") else {
                    return Err("function expression statement has no expression".into());
                };
                self.inline_function_statement(expression, environment)
            }
            Some("Assignment") => {
                let target = value
                    .get("left")
                    .and_then(expression_symbol)
                    .ok_or_else(|| "function assignment has no named target".to_string())?;
                let rhs = value
                    .get("right")
                    .ok_or_else(|| "function assignment has no right side".to_string())?;
                let rhs = substitute_named_values(rhs, environment);
                environment.insert(target, rhs);
                Ok(None)
            }
            Some("Return") => {
                let expression = value
                    .get("expr")
                    .ok_or_else(|| "function return has no expression".to_string())?;
                Ok(Some(substitute_named_values(expression, environment)))
            }
            Some(kind) => Err(format!(
                "direct function inlining does not support statement kind '{kind}'"
            )),
            None => Err("function statement has no kind".into()),
        }
    }
}

fn substitute_named_values(
    value: &Value,
    environment: &mut std::collections::BTreeMap<String, Value>,
) -> Value {
    match value {
        Value::Object(fields) => {
            if string_field(value, "kind").as_deref() == Some("NamedValue") {
                if let Some(name) = expression_symbol(value) {
                    if let Some(replacement) = environment.get(&name) {
                        return replacement.clone();
                    }
                }
            }
            fields
                .iter()
                .map(|(key, nested)| (key.clone(), substitute_named_values(nested, environment)))
                .collect::<serde_json::Map<String, Value>>()
                .into()
        }
        Value::Array(values) => values
            .iter()
            .map(|nested| substitute_named_values(nested, environment))
            .collect::<Vec<_>>()
            .into(),
        scalar => scalar.clone(),
    }
}
