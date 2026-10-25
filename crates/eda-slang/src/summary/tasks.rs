//! Canonicalization for the bounded direct task-inlining subset.

use super::simple_function_constant_i64;
use crate::ast_util::{bool_field, expression_symbol, normalize_detailed_types, string_field};
use crate::types::simple_type_width;
use serde_json::Value;

pub(super) fn canonicalize_simple_task_calls(document: &Value, root: &mut Value) {
    let mut tasks = std::collections::BTreeMap::new();
    collect_simple_task_definitions(document, false, &mut tasks);
    canonicalize_root_simple_task_calls(root, &tasks);
}

#[derive(Clone)]
struct SimpleTaskLocal {
    type_name: String,
    initializer: Option<Value>,
}

fn collect_simple_task_definitions(
    value: &Value,
    in_class: bool,
    tasks: &mut std::collections::BTreeMap<String, Value>,
) {
    if let Some(fields) = value.as_object() {
        let in_class = in_class || string_field(value, "kind").as_deref() == Some("ClassType");
        if !in_class
            && string_field(value, "kind").as_deref() == Some("Subroutine")
            && string_field(value, "subroutineKind").as_deref() == Some("Task")
        {
            if let (Some(address), Some(name)) = (
                value.get("addr").and_then(Value::as_u64),
                string_field(value, "name"),
            ) {
                let mut definition = value.clone();
                normalize_detailed_types(&mut definition);
                tasks.insert(format!("{address} {name}"), definition);
            }
        }
        for nested in fields.values() {
            collect_simple_task_definitions(nested, in_class, tasks);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_simple_task_definitions(nested, in_class, tasks);
        }
    }
}

fn canonicalize_root_simple_task_calls(
    value: &mut Value,
    tasks: &std::collections::BTreeMap<String, Value>,
) {
    if string_field(value, "kind").as_deref() == Some("Subroutine") {
        return;
    }
    if string_field(value, "kind").as_deref() == Some("ExpressionStatement") {
        if let Some(call) = value.get("expr") {
            if string_field(call, "kind").as_deref() == Some("Call") {
                if let Some(subroutine) = string_field(call, "subroutine") {
                    if let Some(definition) = tasks.get(&subroutine) {
                        if let Some(inlined) = inline_simple_task_call(call, definition) {
                            *value = inlined;
                            canonicalize_root_simple_task_calls(value, tasks);
                            return;
                        }
                    }
                }
            }
        }
    }
    if let Value::Array(values) = value {
        for nested in values {
            canonicalize_root_simple_task_calls(nested, tasks);
        }
        return;
    }
    let Value::Object(fields) = value else {
        return;
    };
    for nested in fields.values_mut() {
        canonicalize_root_simple_task_calls(nested, tasks);
    }
}

fn inline_simple_task_call(call: &Value, definition: &Value) -> Option<Value> {
    let arguments = call.get("arguments")?.as_array()?;
    let formals = definition.get("arguments")?.as_array()?;
    if arguments.len() != formals.len() {
        return None;
    }
    let mut environment = std::collections::BTreeMap::new();
    for (formal, actual) in formals.iter().zip(arguments) {
        let name = string_field(formal, "name")?;
        match string_field(formal, "direction").as_deref() {
            Some("In") => {
                environment.insert(
                    name,
                    serde_json::json!({
                        "kind": "Conversion",
                        "type": string_field(formal, "type")?,
                        "operand": actual,
                    }),
                );
            }
            Some("Out" | "InOut" | "Ref") => {
                if string_field(actual, "kind").as_deref() != Some("Assignment") {
                    return None;
                }
                let target = actual.get("left")?;
                if string_field(formal, "type")? != string_field(target, "type")? {
                    return None;
                }
                environment.insert(name, target.clone());
            }
            _ => return None,
        }
    }

    let locals = simple_task_locals(definition)?;
    initialize_simple_task_locals(&locals, &mut environment)?;
    let body = definition.get("body")?;
    inline_simple_task_statement(body, &mut environment, &locals)
}

fn simple_task_locals(
    definition: &Value,
) -> Option<std::collections::BTreeMap<String, SimpleTaskLocal>> {
    let mut locals = std::collections::BTreeMap::new();
    fn collect(
        value: &Value,
        locals: &mut std::collections::BTreeMap<String, SimpleTaskLocal>,
    ) -> Option<()> {
        if let Some(fields) = value.as_object() {
            if string_field(value, "kind").as_deref() == Some("Variable")
                && !bool_field(value, "flags")
            {
                let name = string_field(value, "name")?;
                locals.insert(
                    name,
                    SimpleTaskLocal {
                        type_name: string_field(value, "type")?,
                        initializer: value.get("initializer").cloned(),
                    },
                );
            }
            for nested in fields.values() {
                collect(nested, locals)?;
            }
        } else if let Some(values) = value.as_array() {
            for nested in values {
                collect(nested, locals)?;
            }
        }
        Some(())
    }
    collect(definition, &mut locals)?;
    Some(locals)
}

fn initialize_simple_task_locals(
    locals: &std::collections::BTreeMap<String, SimpleTaskLocal>,
    environment: &mut std::collections::BTreeMap<String, Value>,
) -> Option<()> {
    for (name, local) in locals {
        let width = simple_type_width(&local.type_name)?.max(1);
        let initializer = match local.initializer.as_ref() {
            Some(initializer) => substitute_simple_task_values(initializer, environment)?,
            None => {
                let two_state = ["bit", "byte", "shortint", "int", "longint"]
                    .iter()
                    .any(|type_name| local.type_name.contains(type_name));
                let value = if two_state {
                    format!("{width}'d0")
                } else {
                    format!("{width}'b{}", "x".repeat(width))
                };
                serde_json::json!({
                    "kind": "IntegerLiteral",
                    "type": local.type_name,
                    "value": value,
                })
            }
        };
        environment.insert(
            name.clone(),
            serde_json::json!({
                "kind": "Conversion",
                "type": local.type_name,
                "operand": initializer,
            }),
        );
    }
    Some(())
}

fn inline_simple_task_statement(
    statement: &Value,
    environment: &mut std::collections::BTreeMap<String, Value>,
    locals: &std::collections::BTreeMap<String, SimpleTaskLocal>,
) -> Option<Value> {
    match string_field(statement, "kind")?.as_str() {
        "List" | "Block" => {
            if let Some(body) = statement.get("body") {
                return inline_simple_task_statement(body, environment, locals);
            }
            let mut statements = Vec::new();
            if let Some(items) = statement.get("list").and_then(Value::as_array) {
                for item in items {
                    if let Some(statement) = inline_simple_task_statement(item, environment, locals)
                    {
                        statements.push(statement);
                    }
                }
            }
            Some(serde_json::json!({
                "kind": "List",
                "list": statements,
            }))
        }
        "VariableDeclaration" => None,
        "ExpressionStatement" => {
            let expression = statement.get("expr")?;
            if string_field(expression, "kind").as_deref() == Some("Assignment") {
                return inline_simple_task_assignment(expression, environment, locals);
            }
            let expression = substitute_simple_task_values(expression, environment)?;
            Some(serde_json::json!({
                "kind": "ExpressionStatement",
                "expr": expression,
            }))
        }
        "Assignment" => inline_simple_task_assignment(statement, environment, locals),
        "Conditional" => {
            let condition = statement
                .get("conditions")?
                .as_array()?
                .first()?
                .get("expr")?;
            let condition = substitute_simple_task_values(condition, environment)?;
            let if_true = statement.get("ifTrue")?;
            let if_false = statement.get("ifFalse");
            let mut true_environment = environment.clone();
            let true_body = inline_simple_task_statement(if_true, &mut true_environment, locals);
            let mut false_environment = environment.clone();
            let false_body = match if_false {
                Some(statement) => {
                    inline_simple_task_statement(statement, &mut false_environment, locals)
                }
                None => None,
            };
            merge_simple_task_environment(
                environment,
                true_environment,
                false_environment,
                condition.clone(),
                locals,
            )?;
            if true_body.is_none() && false_body.is_none() {
                return None;
            }
            let empty_block = serde_json::json!({
                "kind": "Block",
                "blockKind": "Sequential",
            });
            Some(serde_json::json!({
                "kind": "Conditional",
                "conditions": [{"expr": condition}],
                "check": "None",
                "ifTrue": true_body.unwrap_or_else(|| empty_block.clone()),
                "ifFalse": false_body.unwrap_or(empty_block),
            }))
        }
        "ForLoop" => {
            const MAX_INLINE_TASK_LOOP_STEPS: usize = 256;
            let mut statements = Vec::new();
            if let Some(initializers) = statement.get("initializers").and_then(Value::as_array) {
                for initializer in initializers {
                    if let Some(statement) =
                        inline_simple_task_statement(initializer, environment, locals)
                    {
                        statements.push(statement);
                    }
                }
            }
            let stop = statement.get("stopExpr")?;
            let body = statement.get("body")?;
            let steps = statement.get("steps")?.as_array()?;
            for _ in 0..=MAX_INLINE_TASK_LOOP_STEPS {
                let stop = substitute_simple_task_values(stop, environment)?;
                if simple_function_constant_i64(&stop)? == 0 {
                    return Some(serde_json::json!({
                        "kind": "List",
                        "list": statements,
                    }));
                }
                if let Some(statement) = inline_simple_task_statement(body, environment, locals) {
                    statements.push(statement);
                }
                for step in steps {
                    if let Some(statement) = inline_simple_task_statement(step, environment, locals)
                    {
                        statements.push(statement);
                    }
                }
            }
            None
        }
        "WhileLoop" => {
            const MAX_INLINE_TASK_LOOP_STEPS: usize = 256;
            let condition = statement.get("cond")?;
            let body = statement.get("body")?;
            let mut statements = Vec::new();
            for _ in 0..=MAX_INLINE_TASK_LOOP_STEPS {
                let condition = substitute_simple_task_values(condition, environment)?;
                if simple_function_constant_i64(&condition)? == 0 {
                    return Some(serde_json::json!({
                        "kind": "List",
                        "list": statements,
                    }));
                }
                if let Some(statement) = inline_simple_task_statement(body, environment, locals) {
                    statements.push(statement);
                }
            }
            None
        }
        "RepeatLoop" => {
            const MAX_INLINE_TASK_LOOP_STEPS: usize = 256;
            let count = substitute_simple_task_values(statement.get("count")?, environment)?;
            let count = usize::try_from(simple_function_constant_i64(&count)?).ok()?;
            if count > MAX_INLINE_TASK_LOOP_STEPS {
                return None;
            }
            let body = statement.get("body")?;
            let mut statements = Vec::new();
            for _ in 0..count {
                if let Some(statement) = inline_simple_task_statement(body, environment, locals) {
                    statements.push(statement);
                }
            }
            Some(serde_json::json!({
                "kind": "List",
                "list": statements,
            }))
        }
        "Return" => substitute_simple_task_values(statement, environment),
        _ => {
            if simple_task_contains_local_assignment(statement, locals) {
                return None;
            }
            substitute_simple_task_values(statement, environment)
        }
    }
}

fn inline_simple_task_assignment(
    assignment: &Value,
    environment: &mut std::collections::BTreeMap<String, Value>,
    locals: &std::collections::BTreeMap<String, SimpleTaskLocal>,
) -> Option<Value> {
    let left = assignment.get("left")?;
    let target = expression_symbol(left)?;
    let rhs = substitute_simple_task_values(assignment.get("right")?, environment)?;
    if let Some(local) = locals.get(&target) {
        environment.insert(
            target,
            serde_json::json!({
                "kind": "Conversion",
                "type": local.type_name,
                "operand": rhs,
            }),
        );
        return None;
    }
    let mut assignment = assignment.clone();
    if let Some(actual_target) = environment.get(&target) {
        if let Some(fields) = assignment.as_object_mut() {
            fields.insert("left".into(), actual_target.clone());
        }
    }
    if let Some(fields) = assignment.as_object_mut() {
        fields.insert("right".into(), rhs);
    }
    Some(assignment)
}

fn merge_simple_task_environment(
    environment: &mut std::collections::BTreeMap<String, Value>,
    true_environment: std::collections::BTreeMap<String, Value>,
    false_environment: std::collections::BTreeMap<String, Value>,
    condition: Value,
    locals: &std::collections::BTreeMap<String, SimpleTaskLocal>,
) -> Option<()> {
    for name in locals.keys() {
        let (true_value, false_value) =
            match (true_environment.get(name), false_environment.get(name)) {
                (Some(true_value), Some(false_value)) => (true_value, false_value),
                _ => return None,
            };
        if true_value == false_value {
            environment.insert(name.clone(), true_value.clone());
        } else {
            environment.insert(
                name.clone(),
                serde_json::json!({
                    "kind": "ConditionalOp",
                    "type": string_field(true_value, "type").unwrap_or_default(),
                    "conditions": [{"expr": condition.clone()}],
                    "left": true_value,
                    "right": false_value,
                }),
            );
        }
    }
    Some(())
}

fn simple_task_contains_local_assignment(
    value: &Value,
    locals: &std::collections::BTreeMap<String, SimpleTaskLocal>,
) -> bool {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("Assignment") {
            if let Some(target) = value.get("left").and_then(expression_symbol) {
                if locals.contains_key(&target) {
                    return true;
                }
            }
        }
        fields
            .values()
            .any(|nested| simple_task_contains_local_assignment(nested, locals))
    } else if let Some(values) = value.as_array() {
        values
            .iter()
            .any(|nested| simple_task_contains_local_assignment(nested, locals))
    } else {
        false
    }
}

fn substitute_simple_task_values(
    value: &Value,
    environment: &std::collections::BTreeMap<String, Value>,
) -> Option<Value> {
    if matches!(
        string_field(value, "kind").as_deref(),
        Some("NamedValue" | "HierarchicalValue")
    ) {
        if let Some(name) = expression_symbol(value) {
            if let Some(replacement) = environment.get(&name) {
                return Some(replacement.clone());
            }
        }
    }
    match value {
        Value::Object(fields) => {
            let mut result = serde_json::Map::new();
            for (key, nested) in fields {
                result.insert(
                    key.clone(),
                    substitute_simple_task_values(nested, environment)?,
                );
            }
            Some(Value::Object(result))
        }
        Value::Array(values) => Some(Value::Array(
            values
                .iter()
                .map(|nested| substitute_simple_task_values(nested, environment))
                .collect::<Option<_>>()?,
        )),
        scalar => Some(scalar.clone()),
    }
}
