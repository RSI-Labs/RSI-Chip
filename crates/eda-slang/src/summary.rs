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
    procedural_assignments, ClassHandleTypes,
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
    class_name: String,
    methods: std::collections::BTreeMap<String, Value>,
    composition_methods: std::collections::BTreeMap<String, Value>,
    binary_property_methods: std::collections::BTreeMap<String, Value>,
    constant_parameter_methods: std::collections::BTreeMap<String, Value>,
    properties: std::collections::BTreeMap<String, String>,
    property_definitions: std::collections::BTreeMap<String, Value>,
    parameters: std::collections::BTreeMap<String, (String, String, String)>,
}

fn collect_simple_instance_methods(
    value: &Value,
    methods: &mut std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
    global_properties: &std::collections::BTreeMap<String, (String, Value)>,
) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("ClassType") {
            let mut index = SimpleInstanceMethodIndex {
                class_name: string_field(value, "name").unwrap_or_default(),
                ..SimpleInstanceMethodIndex::default()
            };
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
                if string_field(member, "kind").as_deref() != Some("Parameter") {
                    continue;
                }
                if let (Some(address), Some(name), Some(value)) = (
                    member.get("addr").and_then(serde_json::Value::as_u64),
                    string_field(member, "name"),
                    string_field(member, "value"),
                ) {
                    let mut parameter = member.clone();
                    normalize_detailed_types(&mut parameter);
                    index.parameters.insert(
                        format!("{address} {name}"),
                        (
                            name,
                            value,
                            string_field(&parameter, "type").unwrap_or_default(),
                        ),
                    );
                }
            }
            let mut referenced_symbols = Vec::new();
            for member in members {
                collect_referenced_property_symbols(member, &mut referenced_symbols);
            }
            for symbol in referenced_symbols {
                if let Some((name, definition)) = global_properties.get(&symbol) {
                    index.properties.insert(symbol.clone(), name.clone());
                    index
                        .property_definitions
                        .insert(symbol, definition.clone());
                }
            }
            for member in members {
                let is_static = string_field(member, "flags")
                    .is_some_and(|flags| flags.split(',').any(|flag| flag.trim() == "static"));
                let is_virtual = string_field(member, "flags")
                    .is_some_and(|flags| flags.split(',').any(|flag| flag.trim() == "virtual"));
                if is_static
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
                if !arguments.is_empty() || string_field(body, "kind").as_deref() != Some("Return")
                {
                    continue;
                }
                if let (Some(address), Some(name)) = (
                    member.get("addr").and_then(Value::as_u64),
                    string_field(member, "name"),
                ) {
                    let symbol = format!("{address} {name}");
                    if is_virtual {
                        if parameter_only_expression(expression, &index.parameters) {
                            index
                                .constant_parameter_methods
                                .insert(symbol, member.clone());
                        }
                        continue;
                    }
                    match string_field(expression, "kind").as_deref() {
                        Some("NamedValue") => {
                            let Some(property_symbol) = string_field(expression, "symbol") else {
                                continue;
                            };
                            if index.properties.contains_key(&property_symbol) {
                                index.methods.insert(symbol, member.clone());
                            }
                        }
                        Some("Call") => {
                            let empty_call_arguments = Vec::new();
                            let call_arguments = expression
                                .get("arguments")
                                .and_then(Value::as_array)
                                .unwrap_or(&empty_call_arguments);
                            let nested_receiver_is_property =
                                expression.get("thisClass").is_some_and(|nested_receiver| {
                                    matches!(
                                        string_field(nested_receiver, "kind").as_deref(),
                                        Some("NamedValue")
                                    ) && string_field(nested_receiver, "symbol").is_some_and(
                                        |symbol| index.properties.contains_key(&symbol),
                                    )
                                });
                            if call_arguments.is_empty()
                                && nested_receiver_is_property
                                && string_field(expression, "subroutine").is_some()
                            {
                                index.composition_methods.insert(symbol, member.clone());
                            }
                        }
                        Some("BinaryOp") => {
                            if binary_method_properties(expression, &index.properties).is_some() {
                                index.binary_property_methods.insert(symbol, member.clone());
                            }
                        }
                        _ => {}
                    }
                }
            }
            if !index.methods.is_empty()
                || !index.composition_methods.is_empty()
                || !index.binary_property_methods.is_empty()
                || !index.constant_parameter_methods.is_empty()
                || !index.property_definitions.is_empty()
            {
                let address = value
                    .get("addr")
                    .and_then(serde_json::Value::as_u64)
                    .map(|address| address.to_string())
                    .unwrap_or_default();
                let class_key = if address.is_empty() {
                    string_field(value, "name").unwrap_or_default()
                } else {
                    address
                };
                methods.insert(class_key, index);
            }
        }
        for nested in fields.values() {
            collect_simple_instance_methods(nested, methods, global_properties);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_simple_instance_methods(nested, methods, global_properties);
        }
    }
}

fn collect_global_class_properties(
    value: &Value,
    properties: &mut std::collections::BTreeMap<String, (String, Value)>,
) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("ClassProperty") {
            if let (Some(address), Some(name)) = (
                value.get("addr").and_then(serde_json::Value::as_u64),
                string_field(value, "name"),
            ) {
                let mut definition = value.clone();
                normalize_detailed_types(&mut definition);
                properties.insert(format!("{address} {name}"), (name, definition));
            }
        }
        for nested in fields.values() {
            collect_global_class_properties(nested, properties);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_global_class_properties(nested, properties);
        }
    }
}

fn collect_referenced_property_symbols(value: &Value, symbols: &mut Vec<String>) {
    if let Some(fields) = value.as_object() {
        if let Some(symbol) = string_field(value, "symbol") {
            symbols.push(symbol);
        }
        for nested in fields.values() {
            collect_referenced_property_symbols(nested, symbols);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_referenced_property_symbols(nested, symbols);
        }
    }
}

fn binary_method_properties(
    expression: &Value,
    properties: &std::collections::BTreeMap<String, String>,
) -> Option<(String, String)> {
    if string_field(expression, "kind").as_deref() != Some("BinaryOp")
        || string_field(expression, "op").as_deref() != Some("Add")
    {
        return None;
    }
    let left = conversion_property_symbol(expression.get("left")?)?;
    let right = conversion_property_symbol(expression.get("right")?)?;
    if properties.contains_key(&left) && properties.contains_key(&right) {
        Some((left, right))
    } else {
        None
    }
}

fn conversion_property_symbol(value: &Value) -> Option<String> {
    if string_field(value, "kind").as_deref() != Some("Conversion") {
        return None;
    }
    let operand = value.get("operand")?;
    if string_field(operand, "kind").as_deref() != Some("NamedValue") {
        return None;
    }
    string_field(operand, "symbol")
}

fn parameter_method_operands(
    expression: &Value,
    parameters: &std::collections::BTreeMap<String, (String, String, String)>,
) -> Option<(String, String)> {
    if string_field(expression, "kind").as_deref() != Some("BinaryOp")
        || string_field(expression, "op").as_deref() != Some("Add")
    {
        return None;
    }
    let left = parameter_symbol(expression.get("left")?)?;
    let right = parameter_symbol(expression.get("right")?)?;
    if parameters.contains_key(&left) && parameters.contains_key(&right) {
        Some((left, right))
    } else {
        None
    }
}

fn parameter_symbol(value: &Value) -> Option<String> {
    if string_field(value, "kind").as_deref() == Some("Conversion") {
        return parameter_symbol(value.get("operand")?);
    }
    if string_field(value, "kind").as_deref() != Some("NamedValue") {
        return None;
    }
    string_field(value, "symbol")
}

fn canonicalize_simple_instance_method_calls(
    value: &mut Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
    dynamic_receivers: &std::collections::BTreeSet<String>,
) -> Option<()> {
    if string_field(value, "kind").as_deref() == Some("Call") {
        if let (Some(subroutine), Some(receiver)) =
            (string_field(value, "subroutine"), value.get("thisClass"))
        {
            if let Some((receiver_path, class_name)) = class_method_receiver_path(receiver, classes)
            {
                let index = classes.get(&class_name)?;
                if let Some(method) = index.constant_parameter_methods.get(&subroutine) {
                    if !dynamic_receivers.contains(receiver_path.as_str()) {
                        let expression = method.get("body")?.get("expr")?;
                        if string_field(expression, "kind").as_deref() == Some("IntegerLiteral") {
                            *value = expression.clone();
                            return Some(());
                        }
                        let (left_symbol, right_symbol) =
                            parameter_method_operands(expression, &index.parameters)?;
                        let (_, left_value, left_type) = index.parameters.get(&left_symbol)?;
                        let (_, right_value, right_type) = index.parameters.get(&right_symbol)?;
                        *value = serde_json::json!({
                            "kind": "BinaryOp",
                            "type": expression.get("type"),
                            "op": "Add",
                            "left": {
                                "kind": "IntegerLiteral",
                                "type": left_type,
                                "value": left_value,
                            },
                            "right": {
                                "kind": "IntegerLiteral",
                                "type": right_type,
                                "value": right_value,
                            },
                        });
                        return Some(());
                    }
                }
                if let Some(method) = index.methods.get(&subroutine) {
                    let expression = method.get("body")?.get("expr")?;
                    let property_symbol = string_field(expression, "symbol")?;
                    let property_name = index.properties.get(&property_symbol)?;
                    *value = serde_json::json!({
                        "kind": "NamedValue",
                        "type": expression.get("type"),
                        "symbol": format!("{receiver_path}.{property_name}"),
                    });
                    return Some(());
                }
                if let Some(method) = index.composition_methods.get(&subroutine) {
                    let nested_call = method.get("body")?.get("expr")?;
                    let nested_subroutine = string_field(nested_call, "subroutine")?;
                    let nested_receiver = nested_call.get("thisClass")?;
                    let property_symbol = string_field(nested_receiver, "symbol")?;
                    let property_name = index.properties.get(&property_symbol)?;
                    let property = index.property_definitions.get(&property_symbol)?;
                    let property_type = string_field(property, "type")?;
                    let nested_class_name = class_type_key(&property_type);
                    let nested_index = classes.get(&nested_class_name)?;
                    let nested_method = nested_index.methods.get(&nested_subroutine)?;
                    let nested_expression = nested_method.get("body")?.get("expr")?;
                    let nested_property_symbol = string_field(nested_expression, "symbol")?;
                    let nested_property_name =
                        nested_index.properties.get(&nested_property_symbol)?;
                    *value = serde_json::json!({
                        "kind": "NamedValue",
                        "type": nested_expression.get("type"),
                        "symbol": format!(
                            "{receiver_path}.{property_name}.{nested_property_name}"
                        ),
                    });
                    return Some(());
                }
                if let Some(method) = index.binary_property_methods.get(&subroutine) {
                    let expression = method.get("body")?.get("expr")?;
                    let (left_symbol, right_symbol) =
                        binary_method_properties(expression, &index.properties)?;
                    let left_name = index.properties.get(&left_symbol)?;
                    let right_name = index.properties.get(&right_symbol)?;
                    let left_conversion = expression.get("left")?;
                    let right_conversion = expression.get("right")?;
                    let left_operand = left_conversion.get("operand")?;
                    let right_operand = right_conversion.get("operand")?;
                    *value = serde_json::json!({
                        "kind": "BinaryOp",
                        "type": expression.get("type"),
                        "op": "Add",
                        "left": {
                            "kind": "Conversion",
                            "type": left_conversion.get("type"),
                            "operand": {
                                "kind": "NamedValue",
                                "type": left_operand.get("type"),
                                "symbol": format!("{receiver_path}.{left_name}"),
                            },
                        },
                        "right": {
                            "kind": "Conversion",
                            "type": right_conversion.get("type"),
                            "operand": {
                                "kind": "NamedValue",
                                "type": right_operand.get("type"),
                                "symbol": format!("{receiver_path}.{right_name}"),
                            },
                        },
                    });
                    return Some(());
                }
            }
        }
    }
    if let Value::Array(values) = value {
        for nested in values {
            let _ = canonicalize_simple_instance_method_calls(nested, classes, dynamic_receivers);
        }
        return Some(());
    }
    let Value::Object(fields) = value else {
        return Some(());
    };
    for nested in fields.values_mut() {
        let _ = canonicalize_simple_instance_method_calls(nested, classes, dynamic_receivers);
    }
    Some(())
}

fn canonicalize_dynamic_parameter_virtual_calls(
    value: &mut Value,
    definitions: &std::collections::BTreeMap<String, Value>,
    dynamic_sources: &std::collections::BTreeMap<String, String>,
) {
    if string_field(value, "kind").as_deref() == Some("Call") {
        if let (Some(subroutine), Some(receiver), Some(receiver_type)) = (
            string_field(value, "subroutine"),
            value.get("thisClass"),
            value
                .get("thisClass")
                .and_then(|receiver| string_field(receiver, "type")),
        ) {
            let method_name = subroutine
                .split_whitespace()
                .next_back()
                .unwrap_or_default();
            let receiver_name = receiver
                .get("symbol")
                .and_then(Value::as_str)
                .and_then(|symbol| symbol.split_whitespace().next_back())
                .unwrap_or_default();
            let receiver_key = definition_class_key(&receiver_type, definitions);
            if let Some(source_key) = dynamic_sources.get(receiver_name) {
                if let (Some(source_method), Some(fallback_method)) = (
                    virtual_parameter_method(source_key, method_name, definitions),
                    virtual_parameter_method(&receiver_key, method_name, definitions),
                ) {
                    let source_parameters =
                        class_parameter_environment(source_key, definitions).unwrap_or_default();
                    let fallback_parameters =
                        class_parameter_environment(&receiver_key, definitions).unwrap_or_default();
                    let source_expression = source_method
                        .get("body")
                        .and_then(|body| body.get("expr"))
                        .cloned();
                    let fallback_expression = fallback_method
                        .get("body")
                        .and_then(|body| body.get("expr"))
                        .cloned();
                    if let (Some(source_expression), Some(fallback_expression)) =
                        (source_expression, fallback_expression)
                    {
                        let source_expression =
                            substitute_class_parameters(&source_expression, &source_parameters);
                        let fallback_expression =
                            substitute_class_parameters(&fallback_expression, &fallback_parameters);
                        let Some(source_id) = definitions
                            .keys()
                            .position(|class_key| class_key == source_key)
                            .map(|index| index as u64 + 1)
                        else {
                            return;
                        };
                        let condition = serde_json::json!({
                            "kind": "BinaryOp",
                            "type": "bit",
                            "op": "Equality",
                            "left": {
                                "kind": "NamedValue",
                                "type": "bit[31:0]",
                                "symbol": format!("__class_tag_{receiver_name}"),
                            },
                            "right": {
                                "kind": "IntegerLiteral",
                                "type": "int",
                                "value": source_id.to_string(),
                            },
                        });
                        *value = serde_json::json!({
                            "kind": "ConditionalOp",
                            "type": value.get("type"),
                            "conditions": [{"expr": condition}],
                            "left": source_expression,
                            "right": fallback_expression,
                        });
                        return;
                    }
                }
            }
        }
    }
    if let Value::Array(values) = value {
        for nested in values {
            canonicalize_dynamic_parameter_virtual_calls(nested, definitions, dynamic_sources);
        }
        return;
    }
    let Value::Object(fields) = value else {
        return;
    };
    for nested in fields.values_mut() {
        canonicalize_dynamic_parameter_virtual_calls(nested, definitions, dynamic_sources);
    }
}

fn virtual_parameter_method(
    class_key: &str,
    method_name: &str,
    definitions: &std::collections::BTreeMap<String, Value>,
) -> Option<Value> {
    let parameters = class_parameter_environment(class_key, definitions)?;
    for (_, class) in class_definition_chain(class_key, definitions)? {
        let empty_members = Vec::new();
        let members = class
            .get("members")
            .and_then(Value::as_array)
            .unwrap_or(&empty_members);
        let Some(method) = members.iter().find(|member| {
            string_field(member, "kind").as_deref() == Some("Subroutine")
                && string_field(member, "name").as_deref() == Some(method_name)
        }) else {
            continue;
        };
        let is_virtual = string_field(method, "flags")
            .is_some_and(|flags| flags.split(',').any(|flag| flag.trim() == "virtual"));
        let has_parameter_expression = method
            .get("body")
            .and_then(|body| body.get("expr"))
            .is_some_and(|expression| parameter_only_expression(expression, &parameters));
        if has_parameter_expression && is_virtual {
            return Some(method.clone());
        }
        return None;
    }
    None
}

fn parameter_only_expression(
    value: &Value,
    parameters: &std::collections::BTreeMap<String, (String, String, String)>,
) -> bool {
    match string_field(value, "kind").as_deref() {
        Some("IntegerLiteral") => true,
        Some("NamedValue") => string_field(value, "symbol")
            .and_then(|symbol| class_parameter_value(&symbol, parameters))
            .is_some(),
        Some("BinaryOp") => {
            string_field(value, "op").as_deref() == Some("Add")
                && value
                    .get("left")
                    .is_some_and(|left| parameter_only_expression(left, parameters))
                && value
                    .get("right")
                    .is_some_and(|right| parameter_only_expression(right, parameters))
        }
        _ => false,
    }
}

fn substitute_class_parameters(
    value: &Value,
    parameters: &std::collections::BTreeMap<String, (String, String, String)>,
) -> Value {
    if let Some(symbol) = string_field(value, "symbol") {
        if let Some((parameter_value, parameter_type)) = class_parameter_value(&symbol, parameters)
        {
            return serde_json::json!({
                "kind": "IntegerLiteral",
                "type": parameter_type,
                "value": parameter_value,
            });
        }
    }
    match value {
        Value::Object(fields) => {
            let mut result = serde_json::Map::new();
            for (key, nested) in fields {
                result.insert(key.clone(), substitute_class_parameters(nested, parameters));
            }
            Value::Object(result)
        }
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|nested| substitute_class_parameters(nested, parameters))
                .collect(),
        ),
        scalar => scalar.clone(),
    }
}

fn class_parameter_value(
    symbol: &str,
    parameters: &std::collections::BTreeMap<String, (String, String, String)>,
) -> Option<(String, String)> {
    if let Some(value) = parameters.get(symbol) {
        return Some((value.1.clone(), value.2.clone()));
    }
    let name = symbol.split_whitespace().next_back()?.to_string();
    parameters
        .iter()
        .find(|(parameter_symbol, _)| {
            parameter_symbol
                .split_whitespace()
                .next_back()
                .is_some_and(|parameter_name| *parameter_name == name)
        })
        .map(|(_, value)| (value.1.clone(), value.2.clone()))
}

fn class_method_receiver_path(
    value: &Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
) -> Option<(String, String)> {
    if matches!(
        string_field(value, "kind").as_deref(),
        Some("NamedValue" | "HierarchicalValue")
    ) {
        let symbol = string_field(value, "symbol")?;
        let receiver_name = symbol.split_whitespace().next_back()?.to_string();
        let class_name = class_type_key(&string_field(value, "type")?);
        return classes
            .contains_key(&class_name)
            .then_some((receiver_name, class_name));
    }
    if string_field(value, "kind").as_deref() != Some("MemberAccess") {
        return None;
    }

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
    let receiver_symbol = string_field(receiver, "symbol")?;
    let mut symbol = receiver_symbol.split_whitespace().next_back()?.to_string();
    let mut current_class = class_type_key(&string_field(receiver, "type")?);
    members.reverse();
    for member in members {
        let index = classes.get(&current_class)?;
        let property = index.property_definitions.get(&member)?;
        if string_field(property, "flags")
            .is_some_and(|flags| flags.split(',').any(|flag| flag.trim() == "static"))
        {
            return None;
        }
        let property_type = string_field(property, "type")?;
        current_class = class_type_key(&property_type);
        if !classes.contains_key(&current_class) {
            return None;
        }
        symbol.push('.');
        symbol.push_str(member.split_whitespace().next_back()?);
    }
    Some((symbol, current_class))
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
    let mut current_class = class_type_key(&receiver_type);
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
        current_class = class_type_key(&property_type);
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
        .split(['.', '#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

fn class_type_key(type_name: &str) -> String {
    type_name
        .split_whitespace()
        .next()
        .filter(|token| token.chars().all(|character| character.is_ascii_digit()))
        .map(str::to_string)
        .unwrap_or_else(|| class_type_name(type_name))
}

fn collect_dynamic_class_receivers(
    value: &Value,
    class_types: &std::collections::BTreeSet<String>,
    receivers: &mut std::collections::BTreeSet<String>,
) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("Assignment") {
            if let (Some(left), Some(right)) = (value.get("left"), value.get("right")) {
                let left = conversion_operand_json(left);
                let right = conversion_operand_json(right);
                if let (Some((left_name, left_class)), Some((_, right_class))) = (
                    named_class_value(left, class_types),
                    named_class_value(right, class_types),
                ) {
                    if left_class != right_class {
                        receivers.insert(left_name);
                    }
                }
            }
        }
        for nested in fields.values() {
            collect_dynamic_class_receivers(nested, class_types, receivers);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_dynamic_class_receivers(nested, class_types, receivers);
        }
    }
}

fn collect_class_definitions(
    value: &Value,
    definitions: &mut std::collections::BTreeMap<String, Value>,
) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("ClassType") {
            if let (Some(address), Some(name)) = (
                value.get("addr").and_then(serde_json::Value::as_u64),
                string_field(value, "name"),
            ) {
                definitions.insert(format!("{name}@{address}"), value.clone());
                definitions.entry(name).or_insert_with(|| value.clone());
            }
        }
        for nested in fields.values() {
            collect_class_definitions(nested, definitions);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_class_definitions(nested, definitions);
        }
    }
}

fn definition_class_key(
    type_name: &str,
    definitions: &std::collections::BTreeMap<String, Value>,
) -> String {
    let base_name = class_type_name(type_name);
    if let Some(address) = type_name
        .split_whitespace()
        .next()
        .and_then(|token| token.parse::<u64>().ok())
    {
        let specialized = format!("{base_name}@{address}");
        if definitions.contains_key(&specialized) {
            return specialized;
        }
    }
    base_name
}

fn class_definition_chain(
    class_key: &str,
    definitions: &std::collections::BTreeMap<String, Value>,
) -> Option<Vec<(String, Value)>> {
    let mut chain = Vec::new();
    let mut current = class_key.to_string();
    loop {
        if chain.iter().any(|(name, _)| name == &current) {
            return None;
        }
        let class = definitions.get(&current)?.clone();
        if let Some(base_type) = string_field(&class, "baseClass") {
            let base = definition_class_key(&base_type, definitions);
            chain.push((current, class));
            current = base;
        } else {
            chain.push((current, class));
            return Some(chain);
        }
    }
}

fn class_parameter_environment(
    class_key: &str,
    definitions: &std::collections::BTreeMap<String, Value>,
) -> Option<std::collections::BTreeMap<String, (String, String, String)>> {
    let mut parameters = std::collections::BTreeMap::new();
    for (_, class) in class_definition_chain(class_key, definitions)? {
        let empty_members = Vec::new();
        let members = class
            .get("members")
            .and_then(Value::as_array)
            .unwrap_or(&empty_members);
        for member in members {
            if string_field(member, "kind").as_deref() != Some("Parameter") {
                continue;
            }
            if let (Some(address), Some(name), Some(value)) = (
                member.get("addr").and_then(serde_json::Value::as_u64),
                string_field(member, "name"),
                string_field(member, "value"),
            ) {
                let mut normalized = member.clone();
                normalize_detailed_types(&mut normalized);
                parameters.insert(
                    format!("{address} {name}"),
                    (
                        name,
                        value,
                        string_field(&normalized, "type").unwrap_or_default(),
                    ),
                );
            }
        }
    }
    Some(parameters)
}

fn conversion_operand_json(value: &'_ Value) -> &'_ Value {
    if string_field(value, "kind").as_deref() == Some("Conversion") {
        value.get("operand").unwrap_or(value)
    } else {
        value
    }
}

fn named_class_value(
    value: &Value,
    class_types: &std::collections::BTreeSet<String>,
) -> Option<(String, String)> {
    if !matches!(
        string_field(value, "kind").as_deref(),
        Some("NamedValue" | "HierarchicalValue")
    ) {
        return None;
    }
    let symbol = string_field(value, "symbol")?;
    let name = symbol.split_whitespace().next_back()?.to_string();
    let class_name = class_type_name(&string_field(value, "type")?);
    class_types
        .contains(&class_name)
        .then_some((name, class_name))
}

fn collect_dynamic_class_sources(
    value: &Value,
    definitions: &std::collections::BTreeMap<String, Value>,
    sources: &mut std::collections::BTreeMap<String, String>,
) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("Call")
            && string_field(value, "subroutine").as_deref() == Some("$cast")
        {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if arguments.len() == 2 {
                let target = arguments[0].get("left");
                let source = &arguments[1];
                if let (Some(target_name), Some(target_type), Some(source_type)) = (
                    target.and_then(named_class_name),
                    target.and_then(named_class_type),
                    named_class_type(source),
                ) {
                    let target_key = definition_class_key(&target_type, definitions);
                    let source_key = definition_class_key(&source_type, definitions);
                    let related =
                        class_definition_chain(&source_key, definitions).is_some_and(|chain| {
                            chain.iter().any(|(class_key, _)| *class_key == target_key)
                        });
                    if target_key != source_key && related {
                        sources.insert(target_name, source_key);
                    }
                }
            }
        }
        if string_field(value, "kind").as_deref() == Some("Assignment") {
            if let (Some(left), Some(right)) = (value.get("left"), value.get("right")) {
                let left = conversion_operand_json(left);
                let right = conversion_operand_json(right);
                if let (Some(target_name), Some(target_type), Some(source_type)) = (
                    named_class_name(left),
                    named_class_type(left),
                    named_class_type(right),
                ) {
                    let target_key = definition_class_key(&target_type, definitions);
                    let source_key = definition_class_key(&source_type, definitions);
                    if target_key != source_key
                        && class_definition_chain(&source_key, definitions).is_some_and(|chain| {
                            chain.iter().any(|(class_key, _)| *class_key == target_key)
                        })
                    {
                        sources.insert(target_name, source_key);
                    }
                }
            }
        }
        for nested in fields.values() {
            collect_dynamic_class_sources(nested, definitions, sources);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_dynamic_class_sources(nested, definitions, sources);
        }
    }
}

fn named_class_name(value: &Value) -> Option<String> {
    string_field(value, "symbol")?
        .split_whitespace()
        .next_back()
        .map(str::to_string)
}

fn named_class_type(value: &Value) -> Option<String> {
    if !matches!(
        string_field(value, "kind").as_deref(),
        Some("NamedValue" | "HierarchicalValue")
    ) {
        return None;
    }
    string_field(value, "type")
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

fn collect_imported_package_type_aliases_from_document(
    document: &Value,
    imported_packages: &std::collections::BTreeSet<String>,
    aliases: &mut std::collections::BTreeMap<String, Value>,
    unqualified_owners: &mut std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    let design = document.get("design").unwrap_or(document);
    let empty_members = Vec::new();
    let members = design
        .get("members")
        .and_then(Value::as_array)
        .unwrap_or(&empty_members);
    for member in members {
        if string_field(member, "kind").as_deref() != Some("CompilationUnit") {
            continue;
        }
        let empty_packages = Vec::new();
        let packages = member
            .get("members")
            .and_then(Value::as_array)
            .unwrap_or(&empty_packages);
        for package in packages {
            if string_field(package, "kind").as_deref() != Some("Package") {
                continue;
            }
            let (Some(address), Some(name)) = (
                package.get("addr").and_then(Value::as_u64),
                string_field(package, "name"),
            ) else {
                return Err("slang AST Package has no address or name".into());
            };
            if imported_packages.contains(&format!("{address} {name}")) {
                collect_imported_package_type_aliases(
                    package,
                    None,
                    imported_packages,
                    aliases,
                    unqualified_owners,
                )?;
            }
        }
    }
    Ok(())
}

fn collect_imported_package_type_aliases(
    value: &Value,
    current_package: Option<(&str, &str)>,
    imported_packages: &std::collections::BTreeSet<String>,
    aliases: &mut std::collections::BTreeMap<String, Value>,
    unqualified_owners: &mut std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    let Value::Object(fields) = value else {
        if let Value::Array(values) = value {
            for nested in values {
                collect_imported_package_type_aliases(
                    nested,
                    current_package,
                    imported_packages,
                    aliases,
                    unqualified_owners,
                )?;
            }
        }
        return Ok(());
    };

    let mut current_package =
        current_package.map(|(name, key)| (name.to_string(), key.to_string()));
    if string_field(value, "kind").as_deref() == Some("Package") {
        let (Some(address), Some(name)) = (
            value.get("addr").and_then(Value::as_u64),
            string_field(value, "name"),
        ) else {
            return Err("slang AST Package has no address or name".into());
        };
        current_package = Some((name.clone(), format!("{address} {name}")));
    }

    if let Some((package_name, package_key)) = current_package
        .as_ref()
        .map(|(name, key)| (name.as_str(), key.as_str()))
    {
        if imported_packages.contains(package_key)
            && string_field(value, "kind").as_deref() == Some("TypeAlias")
        {
            let Some(alias_name) = string_field(value, "name") else {
                return Err("slang AST TypeAlias has no name".into());
            };
            let qualified_name = format!("{package_name}::{alias_name}");
            match unqualified_owners.get(&alias_name) {
                Some(existing) if existing != &qualified_name => {
                    return Err(format!(
                        "imported package type alias collision for '{alias_name}'"
                    ));
                }
                None => {
                    unqualified_owners.insert(alias_name, qualified_name.clone());
                }
                _ => {}
            }
            if aliases.contains_key(&qualified_name) {
                return Ok(());
            }

            let mut alias = value.clone();
            normalize_detailed_types(&mut alias);
            if let Some(alias_fields) = alias.as_object_mut() {
                alias_fields.insert("name".into(), Value::String(qualified_name.clone()));
            }
            aliases.insert(qualified_name, alias);
        }
    }

    let current_package = current_package
        .as_ref()
        .map(|(name, key)| (name.as_str(), key.as_str()));
    for nested in fields.values() {
        collect_imported_package_type_aliases(
            nested,
            current_package,
            imported_packages,
            aliases,
            unqualified_owners,
        )?;
    }
    Ok(())
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
    let key = member
        .get("addr")
        .and_then(Value::as_u64)
        .map(|address| format!("{address} {name}"))
        .unwrap_or_else(|| name.clone());
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
        let alias = type_layout_alias_key(&type_name).unwrap_or_default();
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
        key,
        SlangTypeLayoutSummary {
            element_width,
            fields,
        },
    )))
}

fn type_layout_alias_key(type_name: &str) -> Option<&str> {
    let alias = type_name.split('$').next()?.trim();
    (!alias.is_empty()).then_some(alias)
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
        if matches!(method, "name" | "next" | "prev" | "first" | "last" | "num") {
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
    if expression.kind == "Conversion" {
        let mut enum_name = expression
            .type_name
            .split_whitespace()
            .next_back()
            .unwrap_or_default()
            .rsplit(['.', '#', ':'])
            .next()
            .unwrap_or_default()
            .to_string();
        if !enums.contains_key(&enum_name) {
            enum_name = expression
                .operands
                .first()
                .map(|operand| operand.type_name.as_str())
                .unwrap_or_default()
                .split_whitespace()
                .next_back()
                .unwrap_or_default()
                .rsplit(['.', '#', ':'])
                .next()
                .unwrap_or_default()
                .to_string();
        }
        if let Some(definitions) = enums.get(&enum_name) {
            if definitions.len() == 1 {
                expression.enum_definition = Some(definitions[0].clone());
            }
        }
    }
    if matches!(expression.kind.as_str(), "NamedValue" | "HierarchicalValue") {
        let enum_name = expression
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
    if expression.enum_definition.is_none() {
        let enum_name = expression
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
                let alias = type_layout_alias_key(&memory.type_name).unwrap_or_default();
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
pub struct SlangArrayPartLvalueSummary {
    pub indexes: Vec<SlangExpressionSummary>,
    pub start: SlangExpressionSummary,
    pub width: usize,
    pub selection_kind: String,
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
    /// Constant-index unpacked-array lvalues can be compared by their RHS
    /// fingerprint even when the bounded path flattens the array storage.
    pub constant_array_element_lvalue: bool,
    /// Selectors of a one-dimensional dynamic unpacked-array element lvalue.
    pub dynamic_array_lvalue_indexes: Vec<SlangExpressionSummary>,
    /// Constant-index array element with a part-select lvalue.
    pub array_part_lvalue: Option<SlangArrayPartLvalueSummary>,
    /// Canonical source object path for a whole class-handle assignment.
    pub class_handle_source: Option<String>,
    /// Authoritative slang symbol ID for a field-less whole-handle target.
    pub class_handle_target_key: Option<String>,
    /// Authoritative slang symbol ID for a field-less whole-handle source.
    pub class_handle_source_key: Option<String>,
    /// All element sources selected by a dynamic class-array read.
    pub class_handle_dynamic_sources: Vec<String>,
    /// All element targets selected by a dynamic class-array write.
    pub class_handle_dynamic_targets: Vec<String>,
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
        let mut global_class_properties = std::collections::BTreeMap::new();
        collect_global_class_properties(&document, &mut global_class_properties);
        let mut simple_instance_methods = std::collections::BTreeMap::new();
        collect_simple_instance_methods(
            &document,
            &mut simple_instance_methods,
            &global_class_properties,
        );
        let mut class_types = std::collections::BTreeSet::new();
        collect_class_type_names(&document, &mut class_types);
        let mut class_definitions = std::collections::BTreeMap::new();
        collect_class_definitions(&document, &mut class_definitions);
        let mut dynamic_class_sources = std::collections::BTreeMap::new();
        collect_dynamic_class_sources(&root, &class_definitions, &mut dynamic_class_sources);
        let mut dynamic_class_receivers = std::collections::BTreeSet::new();
        collect_dynamic_class_receivers(&root, &class_types, &mut dynamic_class_receivers);
        let class_handle_source_types = simple_instance_methods
            .iter()
            .filter(|(_, index)| {
                index.property_definitions.values().any(|property| {
                    string_field(property, "type")
                        .and_then(|type_name| simple_type_width(&type_name))
                        .is_some()
                })
            })
            .flat_map(|(name, index)| [name.clone(), index.class_name.clone()])
            .collect::<std::collections::BTreeSet<_>>();
        let mut all_class_handle_types = class_types.clone();
        all_class_handle_types.extend(simple_instance_methods.keys().cloned());
        let class_handle_types = ClassHandleTypes {
            field_bearing: &class_handle_source_types,
            all: &all_class_handle_types,
        };
        let _ = canonicalize_simple_instance_method_calls(
            &mut root,
            &simple_instance_methods,
            &dynamic_class_receivers,
        );
        canonicalize_dynamic_parameter_virtual_calls(
            &mut root,
            &class_definitions,
            &dynamic_class_sources,
        );
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
        let mut package_imports = SlangPackageImports::default();
        collect_visible_package_imports(root, &mut package_imports);
        let mut imported_packages = package_imports.wildcard.clone();
        imported_packages.extend(package_imports.explicit.keys().cloned());
        let mut imported_type_aliases = std::collections::BTreeMap::new();
        let mut imported_type_alias_owners = std::collections::BTreeMap::new();
        collect_imported_package_type_aliases_from_document(
            &document,
            &imported_packages,
            &mut imported_type_aliases,
            &mut imported_type_alias_owners,
        )?;
        let mut type_alias_members = members
            .iter()
            .filter(|member| string_field(member, "kind").as_deref() == Some("TypeAlias"))
            .map(|member| {
                let mut scoped = member.clone();
                let name = string_field(&scoped, "name").unwrap_or_default();
                if let Some(fields) = scoped.as_object_mut() {
                    fields.insert("name".into(), Value::String(format!("{top_name}.{name}")));
                }
                scoped
            })
            .collect::<Vec<_>>();
        type_alias_members.extend(imported_type_aliases.into_values());
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
                    let assignments =
                        procedural_assignments(member, &class_handles, class_handle_types);
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
                        &[],
                        &mut instances,
                        &mut continuous_targets,
                        &mut procedural_blocks,
                        &class_handles,
                        class_handle_types,
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
