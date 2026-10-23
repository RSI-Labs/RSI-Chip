//! Typed summaries and structural validation for slang's elaborated AST.

use super::ast_util::{
    canonical_detailed_type, expression_symbol_id, expression_target_names, instance_parameters,
    literal_word, normalize_detailed_types, normalize_instance_bodies, parameter_summary,
    source_span, string_field,
};
use crate::types::{simple_type_signed, simple_type_width};
use eda_core::ir::Design;
use expressions::{
    collect_generate_summary, collect_kinds, empty_expression_summary, expression_summary,
    procedural_assignments,
};

mod expressions;
mod validation;
use serde_json::Value;

fn collect_class_handle_names(
    value: &Value,
    class_types: &std::collections::BTreeSet<String>,
    handles: &mut std::collections::BTreeSet<String>,
) {
    if let Some(fields) = value.as_object() {
        let type_name = string_field(value, "type").unwrap_or_default();
        let class_name = type_name
            .split_whitespace()
            .next_back()
            .and_then(|raw_name| raw_name.split(['#', '$']).next())
            .map(str::trim)
            .filter(|name| !name.is_empty());
        if class_type_is_selected(value, class_name, class_types) {
            if let Some(symbol) = string_field(value, "symbol").filter(|symbol| !symbol.is_empty())
            {
                handles.insert(symbol);
            } else if let Some(name) = string_field(value, "name").filter(|name| !name.is_empty()) {
                if let Some(address) = value.get("addr").and_then(Value::as_u64) {
                    handles.insert(format!("{address} {name}"));
                } else {
                    handles.insert(name);
                }
            }
        }
        for nested in fields.values() {
            collect_class_handle_names(nested, class_types, handles);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_class_handle_names(nested, class_types, handles);
        }
    }
}

fn collect_class_static_property_symbols(
    value: &Value,
    symbols: &mut std::collections::BTreeSet<String>,
) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("ClassType") {
            let empty_members = Vec::new();
            let members = value
                .get("members")
                .and_then(Value::as_array)
                .unwrap_or(&empty_members);
            for member in members {
                if string_field(member, "kind").as_deref() == Some("ClassProperty")
                    && string_field(member, "lifetime").as_deref() == Some("Static")
                {
                    let Some(name) = string_field(member, "name") else {
                        continue;
                    };
                    if let Some(address) = member.get("addr").and_then(Value::as_u64) {
                        symbols.insert(format!("{address} {name}"));
                    }
                }
            }
        }
        for nested in fields.values() {
            collect_class_static_property_symbols(nested, symbols);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_class_static_property_symbols(nested, symbols);
        }
    }
}

fn collect_class_static_property_names(
    value: &Value,
    names: &mut std::collections::BTreeMap<String, String>,
) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("ClassType") {
            if let Some(class_name) = string_field(value, "name") {
                let empty_members = Vec::new();
                let members = value
                    .get("members")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty_members);
                for member in members {
                    if string_field(member, "kind").as_deref() == Some("ClassProperty")
                        && string_field(member, "lifetime").as_deref() == Some("Static")
                    {
                        let Some(property_name) = string_field(member, "name") else {
                            continue;
                        };
                        if let Some(address) = member.get("addr").and_then(Value::as_u64) {
                            names.insert(
                                format!("{address} {property_name}"),
                                format!("{class_name}_{property_name}"),
                            );
                        }
                    }
                }
            }
        }
        for nested in fields.values() {
            collect_class_static_property_names(nested, names);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_class_static_property_names(nested, names);
        }
    }
}

fn collect_simple_static_function_returns(
    value: &Value,
    static_properties: &std::collections::BTreeMap<String, String>,
    returns: &mut std::collections::BTreeMap<String, Value>,
) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("ClassType") {
            let empty_members = Vec::new();
            let members = value
                .get("members")
                .and_then(Value::as_array)
                .unwrap_or(&empty_members);
            for member in members {
                if string_field(member, "kind").as_deref() != Some("Subroutine")
                    || !string_field(member, "flags")
                        .is_some_and(|flags| flags.split(',').any(|flag| flag.trim() == "static"))
                    || string_field(member, "subroutineKind").as_deref() != Some("Function")
                {
                    continue;
                }
                let Some(arguments) = member.get("arguments").and_then(Value::as_array) else {
                    continue;
                };
                if !arguments.is_empty() {
                    continue;
                }
                let Some(body) = member.get("body") else {
                    continue;
                };
                if string_field(body, "kind").as_deref() != Some("Return") {
                    continue;
                }
                let Some(expression) = body.get("expr") else {
                    continue;
                };
                let returns_static_property = string_field(expression, "kind").as_deref()
                    == Some("NamedValue")
                    && expression_symbol_id(expression)
                        .and_then(|symbol| static_properties.get(&symbol))
                        .is_some();
                if !returns_static_property {
                    continue;
                }
                let mut member = member.clone();
                normalize_detailed_types(&mut member);
                let mut expression = expression.clone();
                normalize_detailed_types(&mut expression);
                let return_type = string_field(&member, "returnType").unwrap_or_default();
                let expression_type = string_field(&expression, "type").unwrap_or_default();
                if return_type != expression_type {
                    continue;
                }
                let Some(address) = member.get("addr").and_then(Value::as_u64) else {
                    continue;
                };
                let Some(name) = string_field(&member, "name") else {
                    continue;
                };
                returns.insert(format!("{address} {name}"), expression);
            }
        }
        for nested in fields.values() {
            collect_simple_static_function_returns(nested, static_properties, returns);
        }
        return;
    }
    if let Some(values) = value.as_array() {
        for nested in values {
            collect_simple_static_function_returns(nested, static_properties, returns);
        }
    }
}

fn collect_simple_function_definitions(
    value: &Value,
    in_class: bool,
    functions: &mut std::collections::BTreeMap<String, Value>,
) {
    if let Some(fields) = value.as_object() {
        let in_class = in_class || string_field(value, "kind").as_deref() == Some("ClassType");
        if !in_class
            && string_field(value, "kind").as_deref() == Some("Subroutine")
            && string_field(value, "subroutineKind").as_deref() == Some("Function")
        {
            if let (Some(address), Some(name)) = (
                value.get("addr").and_then(Value::as_u64),
                string_field(value, "name"),
            ) {
                let mut definition = value.clone();
                normalize_detailed_types(&mut definition);
                if simple_function_return_expression(
                    &definition,
                    &std::collections::BTreeMap::new(),
                )
                .is_some()
                {
                    functions.insert(format!("{address} {name}"), definition);
                }
            }
        }
        for nested in fields.values() {
            collect_simple_function_definitions(nested, in_class, functions);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_simple_function_definitions(nested, in_class, functions);
        }
    }
}

#[derive(Default)]
struct SimpleInstanceMethodIndex {
    methods: std::collections::BTreeMap<String, Value>,
    properties: std::collections::BTreeMap<String, String>,
    property_definitions: std::collections::BTreeMap<String, Value>,
}

fn collect_simple_instance_methods(
    value: &Value,
    methods: &mut std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("ClassType") {
            let mut index = SimpleInstanceMethodIndex::default();
            let empty_members = Vec::new();
            let members = value
                .get("members")
                .and_then(Value::as_array)
                .unwrap_or(&empty_members);
            for member in members {
                if string_field(member, "kind").as_deref() == Some("ClassProperty") {
                    if let (Some(address), Some(name)) = (
                        member.get("addr").and_then(Value::as_u64),
                        string_field(member, "name"),
                    ) {
                        let symbol = format!("{address} {name}");
                        index.properties.insert(symbol.clone(), name);
                        let mut definition = member.clone();
                        normalize_detailed_types(&mut definition);
                        index.property_definitions.insert(symbol, definition);
                    }
                }
            }
            for member in members {
                let static_or_virtual = string_field(member, "flags").is_some_and(|flags| {
                    flags
                        .split(',')
                        .any(|flag| matches!(flag.trim(), "static" | "virtual"))
                });
                if static_or_virtual
                    || string_field(member, "kind").as_deref() != Some("Subroutine")
                    || string_field(member, "subroutineKind").as_deref() != Some("Function")
                {
                    continue;
                }
                let empty_arguments = Vec::new();
                let arguments = member
                    .get("arguments")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty_arguments);
                let Some(body) = member.get("body") else {
                    continue;
                };
                let Some(expression) = body.get("expr") else {
                    continue;
                };
                if !arguments.is_empty()
                    || string_field(body, "kind").as_deref() != Some("Return")
                    || string_field(expression, "kind").as_deref() != Some("NamedValue")
                {
                    continue;
                }
                let Some(property_symbol) = string_field(expression, "symbol") else {
                    continue;
                };
                if !index.properties.contains_key(&property_symbol) {
                    continue;
                }
                if let (Some(address), Some(name)) = (
                    member.get("addr").and_then(Value::as_u64),
                    string_field(member, "name"),
                ) {
                    index
                        .methods
                        .insert(format!("{address} {name}"), member.clone());
                }
            }
            if !index.methods.is_empty() || !index.property_definitions.is_empty() {
                methods.insert(string_field(value, "name").unwrap_or_default(), index);
            }
        }
        for nested in fields.values() {
            collect_simple_instance_methods(nested, methods);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_simple_instance_methods(nested, methods);
        }
    }
}

fn canonicalize_simple_instance_method_calls(
    value: &mut Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
) {
    if string_field(value, "kind").as_deref() == Some("Call") {
        if let (Some(subroutine), Some(receiver), Some(receiver_type)) = (
            string_field(value, "subroutine"),
            value.get("thisClass"),
            value
                .get("thisClass")
                .and_then(|receiver| string_field(receiver, "type")),
        ) {
            let class_name = receiver_type
                .split_whitespace()
                .next_back()
                .unwrap_or_default()
                .rsplit(['.', '#', ':'])
                .next()
                .unwrap_or_default()
                .to_string();
            if let Some(index) = classes.get(&class_name) {
                if let Some(method) = index.methods.get(&subroutine) {
                    let body = method.get("body");
                    let expression = body.and_then(|body| body.get("expr"));
                    if let Some(expression) = expression {
                        if let Some(property_symbol) = string_field(expression, "symbol") {
                            if let Some(property_name) = index.properties.get(&property_symbol) {
                                if let Some(receiver_name) =
                                    receiver.get("symbol").and_then(Value::as_str)
                                {
                                    let receiver_name = receiver_name
                                        .split_whitespace()
                                        .next_back()
                                        .unwrap_or_default();
                                    *value = serde_json::json!({
                                        "kind": "NamedValue",
                                        "type": expression.get("type"),
                                        "symbol": format!("{receiver_name}.{property_name}"),
                                    });
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if let Value::Array(values) = value {
        for nested in values {
            canonicalize_simple_instance_method_calls(nested, classes);
        }
        return;
    }
    let Value::Object(fields) = value else {
        return;
    };
    for nested in fields.values_mut() {
        canonicalize_simple_instance_method_calls(nested, classes);
    }
}

fn canonicalize_simple_class_property_reads(
    value: &mut Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
) {
    if string_field(value, "kind").as_deref() == Some("MemberAccess") {
        if let Some((property_type, symbol)) = simple_class_property_path(value, classes) {
            *value = serde_json::json!({
                "kind": "NamedValue",
                "type": property_type,
                "symbol": symbol,
            });
            return;
        }
    }
    if let Value::Array(values) = value {
        for nested in values {
            canonicalize_simple_class_property_reads(nested, classes);
        }
        return;
    }
    let Value::Object(fields) = value else {
        return;
    };
    for nested in fields.values_mut() {
        canonicalize_simple_class_property_reads(nested, classes);
    }
}

fn simple_class_property_path(
    value: &Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
) -> Option<(String, String)> {
    let mut members = Vec::new();
    let mut receiver = value;
    while string_field(receiver, "kind").as_deref() == Some("MemberAccess") {
        members.push(string_field(receiver, "member")?);
        receiver = receiver.get("value")?;
    }
    if !matches!(
        string_field(receiver, "kind").as_deref(),
        Some("NamedValue" | "HierarchicalValue")
    ) {
        return None;
    }
    let receiver_type = string_field(receiver, "type")?;
    let mut current_class = class_type_name(&receiver_type);
    let receiver_symbol = string_field(receiver, "symbol")?;
    let receiver_name = receiver_symbol
        .split_whitespace()
        .next_back()
        .unwrap_or_default()
        .to_string();
    members.reverse();

    for (level, member) in members.iter().enumerate() {
        let index = classes.get(&current_class)?;
        let property = index.property_definitions.get(member)?;
        if string_field(property, "flags")
            .is_some_and(|flags| flags.split(',').any(|flag| flag.trim() == "static"))
        {
            return None;
        }
        let property_type = string_field(property, "type")?;
        if level + 1 == members.len() {
            simple_type_width(&property_type)?;
            let mut symbol = receiver_name;
            for member in members {
                symbol.push('.');
                symbol.push_str(member.split_whitespace().next_back()?);
            }
            return Some((property_type, symbol));
        }
        current_class = class_type_name(&property_type);
        if !classes.contains_key(&current_class) {
            return None;
        }
    }
    None
}

fn class_type_name(type_name: &str) -> String {
    type_name
        .split_whitespace()
        .next_back()
        .unwrap_or_default()
        .rsplit(['.', '#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

#[derive(Default)]
struct SlangPackageImports {
    wildcard: std::collections::BTreeSet<String>,
    explicit: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
}

fn collect_package_parameters(
    value: &Value,
    current_package: Option<&str>,
    imports: &SlangPackageImports,
    parameters: &mut Vec<SlangParameterSummary>,
) {
    if let Some(fields) = value.as_object() {
        let mut current_package = current_package.map(str::to_string);
        if string_field(value, "kind").as_deref() == Some("Package") {
            if let (Some(address), Some(name)) = (
                value.get("addr").and_then(Value::as_u64),
                string_field(value, "name"),
            ) {
                current_package = Some(format!("{address} {name}"));
            }
        }
        if let Some(package) = current_package.as_deref() {
            let wildcard = imports.wildcard.contains(package);
            let explicitly_imported = string_field(value, "name").is_some_and(|name| {
                imports
                    .explicit
                    .get(package)
                    .is_some_and(|names| names.contains(&name))
            });
            if (wildcard || explicitly_imported)
                && string_field(value, "kind").as_deref() == Some("Parameter")
            {
                let mut parameter = value.clone();
                normalize_detailed_types(&mut parameter);
                if let Ok(parameter) = parameter_summary(&parameter) {
                    parameters.push(parameter);
                }
            }
        }
        let current_package = current_package.as_deref();
        for nested in fields.values() {
            collect_package_parameters(nested, current_package, imports, parameters);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_package_parameters(nested, current_package, imports, parameters);
        }
    }
}

fn collect_visible_package_imports(value: &Value, imports: &mut SlangPackageImports) {
    if let Some(fields) = value.as_object() {
        let kind = string_field(value, "kind");
        if kind.as_deref() == Some("WildcardImport") {
            if let Some(package) = string_field(value, "package") {
                imports.wildcard.insert(package);
            }
        } else if kind.as_deref() == Some("ExplicitImport") {
            if let (Some(package), Some(import), Some(name)) = (
                string_field(value, "package"),
                string_field(value, "import"),
                string_field(value, "name"),
            ) {
                if import
                    .rsplit_once(' ')
                    .is_some_and(|(_, imported)| imported == name)
                {
                    imports.explicit.entry(package).or_default().insert(name);
                }
            }
        }
        for nested in fields.values() {
            collect_visible_package_imports(nested, imports);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_visible_package_imports(nested, imports);
        }
    }
}

fn canonicalize_simple_function_calls(
    value: &mut Value,
    functions: &std::collections::BTreeMap<String, Value>,
) {
    if string_field(value, "kind").as_deref() == Some("Call") {
        if let Some(subroutine) = string_field(value, "subroutine") {
            if let Some(definition) = functions.get(&subroutine) {
                let empty_arguments = Vec::new();
                let arguments = value
                    .get("arguments")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty_arguments);
                let empty_formals = Vec::new();
                let formals = definition
                    .get("arguments")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty_formals);
                if arguments.len() == formals.len() {
                    let mut environment = std::collections::BTreeMap::new();
                    let mut exact = true;
                    for (formal, actual) in formals.iter().zip(arguments) {
                        let Some(name) = formal.get("name").and_then(Value::as_str) else {
                            exact = false;
                            break;
                        };
                        let Some(address) = formal.get("addr").and_then(Value::as_u64) else {
                            exact = false;
                            break;
                        };
                        let Some(type_name) = string_field(formal, "type") else {
                            exact = false;
                            break;
                        };
                        if !matches!(
                            string_field(formal, "direction").as_deref(),
                            None | Some("In")
                        ) {
                            exact = false;
                            break;
                        }
                        environment.insert(
                            format!("{address} {name}"),
                            serde_json::json!({
                                "kind": "Conversion",
                                "type": type_name,
                                "operand": actual,
                            }),
                        );
                    }
                    if exact {
                        if let Some(expression) =
                            simple_function_return_expression(definition, &environment)
                        {
                            *value = expression;
                            canonicalize_simple_function_calls(value, functions);
                            return;
                        }
                    }
                }
            }
        }
    }
    if let Value::Array(values) = value {
        for nested in values {
            canonicalize_simple_function_calls(nested, functions);
        }
        return;
    }
    let Value::Object(fields) = value else {
        return;
    };
    for nested in fields.values_mut() {
        canonicalize_simple_function_calls(nested, functions);
    }
}

fn simple_function_return_expression(
    definition: &Value,
    environment: &std::collections::BTreeMap<String, Value>,
) -> Option<Value> {
    let body = definition.get("body")?;
    if string_field(body, "kind").as_deref() != Some("ExpressionStatement") {
        return None;
    }
    let assignment = body.get("expr")?;
    if string_field(assignment, "kind").as_deref() != Some("Assignment") {
        return None;
    }
    let left = assignment.get("left")?;
    let function_name = string_field(definition, "name")?;
    let return_name = string_field(left, "symbol")
        .or_else(|| string_field(left, "name"))?
        .split_whitespace()
        .next_back()?
        .to_string();
    if return_name != function_name {
        return None;
    }
    let expression = assignment.get("right")?.clone();
    let expression = substitute_simple_function_values(&expression, environment)?;
    let return_type = string_field(definition, "returnType")?;
    Some(serde_json::json!({
        "kind": "Conversion",
        "type": return_type,
        "operand": expression,
    }))
}

fn substitute_simple_function_values(
    value: &Value,
    environment: &std::collections::BTreeMap<String, Value>,
) -> Option<Value> {
    if let Some(symbol) = string_field(value, "symbol") {
        if let Some(replacement) = environment.get(&symbol) {
            return Some(replacement.clone());
        }
    }
    match value {
        Value::Object(fields) => {
            let mut result = serde_json::Map::new();
            for (key, nested) in fields {
                result.insert(
                    key.clone(),
                    substitute_simple_function_values(nested, environment)?,
                );
            }
            Some(Value::Object(result))
        }
        Value::Array(values) => Some(Value::Array(
            values
                .iter()
                .map(|nested| substitute_simple_function_values(nested, environment))
                .collect::<Option<_>>()?,
        )),
        scalar => Some(scalar.clone()),
    }
}

fn canonicalize_class_static_symbols(
    value: &mut Value,
    names: &std::collections::BTreeMap<String, String>,
    simple_returns: &std::collections::BTreeMap<String, Value>,
) {
    match value {
        Value::Object(fields) => {
            let is_call = fields
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind == "Call");
            let subroutine = fields
                .get("subroutine")
                .and_then(Value::as_str)
                .map(str::to_string);
            if is_call {
                if let Some(subroutine) = subroutine {
                    if let Some(expression) = simple_returns.get(&subroutine) {
                        *value = expression.clone();
                        canonicalize_class_static_symbols(value, names, simple_returns);
                        return;
                    }
                }
            }
            for (key, nested) in fields.iter_mut() {
                if key == "symbol" {
                    if let Some(symbol) = nested.as_str() {
                        if let Some(canonical) = names.get(symbol) {
                            *nested = Value::String(canonical.clone());
                        }
                    }
                } else {
                    canonicalize_class_static_symbols(nested, names, simple_returns);
                }
            }
        }
        Value::Array(values) => {
            for nested in values {
                canonicalize_class_static_symbols(nested, names, simple_returns);
            }
        }
        _ => {}
    }
}

fn collect_class_type_names(value: &Value, class_types: &mut std::collections::BTreeSet<String>) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("ClassType") {
            if let Some(name) = string_field(value, "name") {
                class_types.insert(name);
            }
        }
        for nested in fields.values() {
            collect_class_type_names(nested, class_types);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_class_type_names(nested, class_types);
        }
    }
}

fn class_type_is_selected(
    value: &Value,
    class_name: Option<&str>,
    class_types: &std::collections::BTreeSet<String>,
) -> bool {
    let Some(class_name) = class_name else {
        return false;
    };
    matches!(
        string_field(value, "kind").as_deref(),
        Some("Net" | "Variable" | "Port" | "NamedValue" | "HierarchicalValue")
    ) && class_types.contains(class_name)
}

fn collect_instance_array_summary(
    member: &Value,
    prefix: &str,
    instances: &mut Vec<SlangInstanceSummary>,
) -> Result<(), String> {
    let range = string_field(member, "range").unwrap_or_default();
    let range = range.trim().trim_start_matches('[').trim_end_matches(']');
    let Some((left, right)) = range.split_once(':') else {
        return Err(format!("instance array has invalid range '{range}'"));
    };
    let left = left
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("instance array has invalid left bound '{left}'"))?;
    let right = right
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("instance array has invalid right bound '{right}'"))?;
    let indexes = if left <= right {
        (left..=right).collect::<Vec<_>>()
    } else {
        (right..=left).rev().collect::<Vec<_>>()
    };
    let elements = member
        .get("members")
        .and_then(Value::as_array)
        .ok_or_else(|| "slang AST InstanceArray has no members".to_string())?;
    if indexes.len() != elements.len() {
        return Err(format!(
            "instance array range has {} indexes but {} elaborated elements",
            indexes.len(),
            elements.len()
        ));
    }
    for (index, element) in indexes.into_iter().zip(elements) {
        let instance_name = format!("{prefix}[{index}]");
        match string_field(element, "kind").as_deref() {
            Some("Instance") => {
                let definition_name = element
                    .get("body")
                    .and_then(|body| string_field(body, "name"))
                    .unwrap_or_default();
                let parameters = element
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
            Some("InstanceArray") => {
                collect_instance_array_summary(element, &instance_name, instances)?;
            }
            other => {
                return Err(format!(
                    "instance array contains unsupported member kind {other:?}"
                ))
            }
        }
    }
    Ok(())
}

fn type_layout_summary(
    member: &Value,
    existing: &std::collections::BTreeMap<String, SlangTypeLayoutSummary>,
) -> Result<Option<(String, SlangTypeLayoutSummary)>, String> {
    if string_field(member, "kind").as_deref() != Some("TypeAlias") {
        return Ok(None);
    }
    let name = string_field(member, "name")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "slang TypeAlias has no name".to_string())?;
    let mut target = member
        .get("target")
        .cloned()
        .ok_or_else(|| format!("slang TypeAlias '{name}' has no target"))?;
    normalize_detailed_types(&mut target);
    if !matches!(
        string_field(&target, "kind").as_deref(),
        Some("PackedStructType" | "PackedUnionType")
    ) {
        return Ok(None);
    }
    let members = target
        .get("members")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("packed type '{name}' has no field layouts"))?;
    let mut fields = Vec::new();
    for field in members {
        let field_name = string_field(field, "name")
            .ok_or_else(|| format!("packed type '{name}' has a field without a name"))?;
        let offset = field
            .get("bitOffset")
            .and_then(Value::as_u64)
            .and_then(|offset| usize::try_from(offset).ok())
            .ok_or_else(|| format!("packed type field '{name}.{field_name}' has no bit offset"))?;
        let type_name = string_field(field, "type").unwrap_or_default();
        let alias = type_name
            .split_whitespace()
            .next_back()
            .unwrap_or_default()
            .rsplit(['.', '#'])
            .next()
            .unwrap_or_default();
        let Some(width) = crate::types::simple_type_width(&type_name)
            .or_else(|| existing.get(alias).map(|layout| layout.element_width))
        else {
            return Ok(None);
        };
        fields.push(SlangFieldLayoutSummary {
            name: field_name,
            offset,
            width,
            type_name,
        });
    }
    let Some(element_width) = fields.iter().map(|field| field.offset + field.width).max() else {
        return Ok(None);
    };
    Ok(Some((
        name,
        SlangTypeLayoutSummary {
            element_width,
            fields,
        },
    )))
}

#[derive(Default)]
struct SlangTypeAliasIndex {
    exact: std::collections::BTreeMap<String, Value>,
    by_name: std::collections::BTreeMap<String, Vec<Value>>,
}

fn collect_type_alias_targets(value: &Value, aliases: &mut SlangTypeAliasIndex) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("TypeAlias") {
            if let (Some(address), Some(name), Some(target)) = (
                value.get("addr").and_then(Value::as_u64),
                string_field(value, "name"),
                value.get("target"),
            ) {
                aliases
                    .exact
                    .insert(format!("{address} {name}"), target.clone());
                aliases
                    .by_name
                    .entry(name)
                    .or_default()
                    .push(target.clone());
            }
        }
        for nested in fields.values() {
            collect_type_alias_targets(nested, aliases);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_type_alias_targets(nested, aliases);
        }
    }
}

fn collect_enum_type_summaries(
    value: &Value,
    aliases: &SlangTypeAliasIndex,
    enums: &mut std::collections::BTreeMap<String, Vec<SlangEnumTypeSummary>>,
) -> Result<(), String> {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("EnumType") {
            if string_field(value, "name").is_none_or(|name| name.is_empty()) {
                for nested in fields.values() {
                    collect_enum_type_summaries(nested, aliases, enums)?;
                }
                return Ok(());
            }
            let definition = enum_type_summary(value, aliases)?;
            enums.entry(definition.0).or_default().push(definition.1);
        }
        for nested in fields.values() {
            collect_enum_type_summaries(nested, aliases, enums)?;
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_enum_type_summaries(nested, aliases, enums)?;
        }
    }
    Ok(())
}

fn enum_type_summary(
    member: &Value,
    aliases: &SlangTypeAliasIndex,
) -> Result<(String, SlangEnumTypeSummary), String> {
    let name = string_field(member, "name")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "slang EnumType has no name".to_string())?;
    let raw_base_type = member
        .get("baseType")
        .ok_or_else(|| format!("enum type '{name}' has no base type"))?;
    let base_type_name = enum_base_type_name(raw_base_type, aliases, 0)
        .filter(|type_name| !type_name.is_empty())
        .ok_or_else(|| format!("enum type '{name}' has an unresolved base type"))?;
    let width = simple_type_width(&base_type_name)
        .ok_or_else(|| format!("enum type '{name}' has an unresolved base width"))?;
    let signed = simple_type_signed(&base_type_name)
        .ok_or_else(|| format!("enum type '{name}' has unresolved base signedness"))?;
    let members = member
        .get("members")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("enum type '{name}' has no values"))?;
    let mut summaries = Vec::with_capacity(members.len());
    for value in members {
        let value_name = string_field(value, "name")
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("enum type '{name}' has an unnamed value"))?;
        let raw = string_field(value, "value")
            .ok_or_else(|| format!("enum value '{name}.{value_name}' has no value"))?;
        let word = literal_word(&base_type_name, &raw)
            .ok_or_else(|| format!("enum value '{name}.{value_name}' is invalid"))?;
        summaries.push(SlangEnumMemberSummary {
            name: value_name,
            literal: format!("{}:{}", word.width(), word.to_bin_str()),
        });
    }
    Ok((
        name,
        SlangEnumTypeSummary {
            base_type_name,
            width,
            signed,
            members: summaries,
        },
    ))
}

fn enum_base_type_name(
    value: &Value,
    aliases: &SlangTypeAliasIndex,
    depth: usize,
) -> Option<String> {
    if depth > 16 {
        return None;
    }
    if let Some(reference) = value.as_str() {
        let target = aliases.exact.get(reference).or_else(|| {
            let name = reference
                .split_whitespace()
                .next_back()?
                .rsplit(['.', '#', ':'])
                .next()?;
            let candidates = aliases.by_name.get(name)?;
            (candidates.len() == 1).then(|| &candidates[0])
        })?;
        return enum_base_type_name(target, aliases, depth + 1);
    }
    canonical_detailed_type(value)
}

fn annotate_enum_name_calls(
    expression: &mut super::SlangExpressionSummary,
    enums: &std::collections::BTreeMap<String, Vec<SlangEnumTypeSummary>>,
) {
    if expression.kind == "Call" {
        let method = expression
            .operator
            .split_whitespace()
            .next_back()
            .unwrap_or_default();
        if method == "name" {
            if let Some(receiver) = expression.operands.first() {
                let enum_name = receiver
                    .type_name
                    .split_whitespace()
                    .next_back()
                    .unwrap_or_default()
                    .rsplit(['.', '#', ':'])
                    .next()
                    .unwrap_or_default()
                    .to_string();
                if let Some(definitions) = enums.get(&enum_name) {
                    if definitions.len() == 1 {
                        expression.enum_definition = Some(definitions[0].clone());
                    }
                }
            }
        }
    }
    for operand in &mut expression.operands {
        annotate_enum_name_calls(operand, enums);
    }
}

fn annotate_memory_element_shapes(
    expression: &mut SlangExpressionSummary,
    type_layouts: &std::collections::BTreeMap<String, SlangTypeLayoutSummary>,
) {
    if expression.kind == "ElementSelect" {
        let mut selected = expression.operands.first();
        while let Some(node) = selected {
            if node.kind == "ElementSelect" {
                selected = node.operands.first();
            } else {
                break;
            }
        }
        if let Some(memory) = selected {
            if memory.type_name.contains('$') {
                let element_type = memory.type_name.split('$').next().unwrap_or_default();
                let alias = element_type
                    .split_whitespace()
                    .next_back()
                    .unwrap_or_default()
                    .rsplit(['.', '#', ':'])
                    .next()
                    .unwrap_or_default();
                if let Some(layout) = type_layouts.get(alias) {
                    expression.type_width.get_or_insert(layout.element_width);
                    expression.type_signed.get_or_insert(false);
                }
            }
        }
    }
    for operand in &mut expression.operands {
        annotate_memory_element_shapes(operand, type_layouts);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangAstSummary {
    pub top_name: String,
    pub time_unit_exponent: i8,
    pub source_spans: std::collections::BTreeMap<String, SlangSourceSpan>,
    pub ports: Vec<SlangPortSummary>,
    pub declarations: Vec<SlangDeclarationSummary>,
    pub parameters: Vec<SlangParameterSummary>,
    pub continuous_targets: Vec<SlangContinuousTarget>,
    pub procedural_blocks: Vec<SlangProceduralBlockSummary>,
    pub instances: Vec<SlangInstanceSummary>,
    pub type_layouts: std::collections::BTreeMap<String, SlangTypeLayoutSummary>,
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
pub struct SlangTypeLayoutSummary {
    pub element_width: usize,
    pub fields: Vec<SlangFieldLayoutSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangEnumTypeSummary {
    pub base_type_name: String,
    pub width: usize,
    pub signed: bool,
    pub members: Vec<SlangEnumMemberSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangEnumMemberSummary {
    pub name: String,
    /// Canonical literal bits (`width:bits`, MSB first).
    pub literal: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangFieldLayoutSummary {
    pub name: String,
    pub offset: usize,
    pub width: usize,
    pub type_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangMemoryMemberLvalueSummary {
    pub indexes: Vec<SlangExpressionSummary>,
    pub members: Vec<String>,
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
    /// Selector of a dynamic packed lvalue. Direct lowering canonicalizes the
    /// update into a whole-vector concatenation of per-bit conditionals.
    pub dynamic_packed_lvalue: bool,
    pub dynamic_packed_lvalue_selector: Option<SlangExpressionSummary>,
    /// Member writes through a sparse-memory element are canonicalized into a
    /// whole-memory-element concat.
    pub memory_member_lvalue: Option<SlangMemoryMemberLvalueSummary>,
    /// Class static storage uses an internal IR name, so exact RHS symbol
    /// equivalence is deferred while assignment inventory remains checked.
    pub uses_class_static: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangExpressionSummary {
    pub kind: String,
    pub type_name: String,
    pub operator: String,
    /// `Simple`, `IndexedUp`, or `IndexedDown` for slang range selects.
    pub selection_kind: String,
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
    /// Elaborated replication count for slang `Replication` nodes.
    pub repeat_count: Option<usize>,
    /// Normalized decimal exponent for elaborated `$time`/`$stime` calls.
    pub simulation_time_unit: Option<i8>,
    /// Definition used to construct the conditional tree emitted by direct
    /// lowering for an enum `.name()` call.
    pub enum_definition: Option<SlangEnumTypeSummary>,
    pub operands: Vec<SlangExpressionSummary>,
}

impl SlangAstSummary {
    pub fn parse(json: &str) -> Result<Self, String> {
        let start = json
            .find('{')
            .ok_or_else(|| "slang AST output did not contain a JSON object".to_string())?;
        let document: Value = serde_json::from_str(&json[start..])
            .map_err(|error| format!("invalid slang AST JSON: {error}"))?;
        let mut class_static_names = std::collections::BTreeMap::new();
        collect_class_static_property_names(&document, &mut class_static_names);
        let mut simple_static_function_returns = std::collections::BTreeMap::new();
        collect_simple_static_function_returns(
            &document,
            &class_static_names,
            &mut simple_static_function_returns,
        );
        let root = document.get("design").unwrap_or(&document);
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
        let mut root = root.clone();
        canonicalize_class_static_symbols(
            &mut root,
            &class_static_names,
            &simple_static_function_returns,
        );
        normalize_instance_bodies(&mut root);
        normalize_detailed_types(&mut root);
        let mut simple_functions = std::collections::BTreeMap::new();
        collect_simple_function_definitions(&document, false, &mut simple_functions);
        canonicalize_simple_function_calls(&mut root, &simple_functions);
        let mut simple_instance_methods = std::collections::BTreeMap::new();
        collect_simple_instance_methods(&document, &mut simple_instance_methods);
        canonicalize_simple_instance_method_calls(&mut root, &simple_instance_methods);
        canonicalize_simple_class_property_reads(&mut root, &simple_instance_methods);
        normalize_detailed_types(&mut root);
        let root = &root;
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
        let time_unit_exponent = summary_time_unit_exponent(&document, body);
        let mut source_spans = std::collections::BTreeMap::new();
        if let Some(span) = source_span(root) {
            source_spans.insert("top".to_string(), span);
        }
        let members = body
            .get("members")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut class_types = std::collections::BTreeSet::new();
        collect_class_type_names(&document, &mut class_types);
        let mut class_handles = std::collections::BTreeSet::new();
        collect_class_handle_names(root, &class_types, &mut class_handles);
        collect_class_static_property_symbols(&document, &mut class_handles);
        let mut type_aliases = SlangTypeAliasIndex::default();
        collect_type_alias_targets(root, &mut type_aliases);
        let mut enum_types = std::collections::BTreeMap::<String, Vec<SlangEnumTypeSummary>>::new();
        collect_enum_type_summaries(root, &type_aliases, &mut enum_types)?;
        let mut ports = Vec::new();
        let mut declarations = Vec::new();
        let mut parameters = Vec::new();
        let mut continuous_targets = Vec::new();
        let mut procedural_blocks = Vec::new();
        let mut instances = Vec::new();
        let mut type_layouts = std::collections::BTreeMap::new();
        let type_alias_members = members
            .iter()
            .filter(|member| string_field(member, "kind").as_deref() == Some("TypeAlias"))
            .cloned()
            .collect::<Vec<_>>();
        loop {
            let mut changed = false;
            for member in &type_alias_members {
                let Some((name, layout)) = type_layout_summary(member, &type_layouts)? else {
                    continue;
                };
                if type_layouts.insert(name, layout).is_none() {
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
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
                    let assignments = procedural_assignments(member, &class_handles);
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
                "InstanceArray" => {
                    let array_name = string_field(member, "name")
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| "slang AST InstanceArray has no name".to_string())?;
                    collect_instance_array_summary(member, &array_name, &mut instances)?;
                }
                generate_kind if generate_kind.starts_with("GenerateBlock") => {
                    collect_generate_summary(
                        member,
                        &[],
                        &mut instances,
                        &mut continuous_targets,
                        &mut procedural_blocks,
                        &class_handles,
                    )?;
                }
                _ => {}
            }
        }
        if let Some(members) = root
            .get("eda_compilation_unit_members")
            .and_then(serde_json::Value::as_array)
        {
            for member in members {
                if string_field(member, "kind").as_deref() == Some("Parameter") {
                    parameters.push(parameter_summary(member)?);
                }
            }
        }
        let mut package_parameters = Vec::new();
        let mut package_imports = SlangPackageImports::default();
        collect_visible_package_imports(root, &mut package_imports);
        collect_package_parameters(&document, None, &package_imports, &mut package_parameters);
        let visible_parameter_names = parameters
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect::<std::collections::BTreeSet<_>>();
        parameters.extend(
            package_parameters
                .into_iter()
                .filter(|parameter| !visible_parameter_names.contains(&parameter.name)),
        );
        let mut kinds = std::collections::BTreeSet::new();
        collect_kinds(root, &mut kinds);
        for target in &mut continuous_targets {
            annotate_simulation_time_units(&mut target.rhs, time_unit_exponent);
            annotate_enum_name_calls(&mut target.rhs, &enum_types);
            annotate_memory_element_shapes(&mut target.rhs, &type_layouts);
        }
        for block in &mut procedural_blocks {
            for assignment in &mut block.assignments {
                annotate_simulation_time_units(&mut assignment.rhs, time_unit_exponent);
                annotate_enum_name_calls(&mut assignment.rhs, &enum_types);
                annotate_memory_element_shapes(&mut assignment.rhs, &type_layouts);
                if let Some(selector) = assignment.dynamic_packed_lvalue_selector.as_mut() {
                    annotate_memory_element_shapes(selector, &type_layouts);
                }
                if let Some(member) = assignment.memory_member_lvalue.as_mut() {
                    for index in &mut member.indexes {
                        annotate_memory_element_shapes(index, &type_layouts);
                    }
                }
            }
        }
        Ok(Self {
            top_name,
            time_unit_exponent,
            source_spans,
            ports,
            declarations,
            parameters,
            continuous_targets,
            procedural_blocks,
            instances,
            type_layouts,
            kinds,
        })
    }
}

fn annotate_simulation_time_units(expression: &mut SlangExpressionSummary, exponent: i8) {
    if expression.kind == "Call" && matches!(expression.operator.as_str(), "$time" | "$stime") {
        expression.simulation_time_unit = Some(exponent);
    }
    for operand in &mut expression.operands {
        annotate_simulation_time_units(operand, exponent);
    }
}

fn summary_time_unit_exponent(document: &Value, body: &Value) -> i8 {
    if let (Some(magnitude), Some(exponent)) = (
        body.get("eda_time_unit_magnitude").and_then(Value::as_u64),
        body.get("eda_time_unit_exponent")
            .and_then(Value::as_i64)
            .and_then(|value| i8::try_from(value).ok()),
    ) {
        let trailing_zeros = match magnitude {
            1 => 0,
            10 => 1,
            100 => 2,
            _ => 0,
        };
        return exponent.saturating_add(trailing_zeros);
    }
    let definition = body
        .get("definition")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let scale = document
        .get("definitions")
        .and_then(Value::as_array)
        .and_then(|definitions| {
            definitions.iter().find(|candidate| {
                let address = candidate.get("addr").and_then(Value::as_u64);
                let name = string_field(candidate, "name").unwrap_or_default();
                address
                    .map(|address| format!("{address} {name}"))
                    .is_some_and(|reference| reference == definition)
            })
        })
        .and_then(|candidate| string_field(candidate, "timeScale"))
        .and_then(|scale| scale.split('/').next().map(|unit| unit.trim().to_string()));
    let Some(scale) = scale else {
        return -9;
    };
    let digits_end = scale
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(scale.len());
    let Ok(magnitude) = scale[..digits_end].parse::<u64>() else {
        return -9;
    };
    let exponent: i8 = match scale[digits_end..].trim() {
        "s" => 0,
        "ms" => -3,
        "us" => -6,
        "ns" => -9,
        "ps" => -12,
        "fs" => -15,
        _ => -9,
    };
    let trailing_zeros = match magnitude {
        1 => 0,
        10 => 1,
        100 => 2,
        _ => 0,
    };
    exponent.saturating_add(trailing_zeros)
}
