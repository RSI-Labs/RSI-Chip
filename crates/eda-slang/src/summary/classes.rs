//! Class-index construction and direct class-expression canonicalization.

use crate::ast_util::{integer_literal_word, normalize_detailed_types, string_field};
use crate::types::simple_type_width;
use serde_json::Value;

pub(super) fn collect_simple_instance_classes(
    document: &Value,
    global_properties: &std::collections::BTreeMap<String, (String, Value)>,
) -> std::collections::BTreeMap<String, SimpleInstanceMethodIndex> {
    let mut methods = std::collections::BTreeMap::new();
    collect_simple_instance_methods(document, &mut methods, global_properties);
    inherit_simple_instance_members(&mut methods);
    classify_class_property_methods(&mut methods);
    methods
}

#[derive(Default, Clone)]
pub(super) struct SimpleInstanceMethodIndex {
    pub(super) class_name: String,
    base_class: String,
    methods: std::collections::BTreeMap<String, Value>,
    candidate_methods: std::collections::BTreeMap<String, Value>,
    composition_methods: std::collections::BTreeMap<String, Value>,
    binary_property_methods: std::collections::BTreeMap<String, Value>,
    constant_parameter_methods: std::collections::BTreeMap<String, Value>,
    properties: std::collections::BTreeMap<String, String>,
    pub(super) property_definitions: std::collections::BTreeMap<String, Value>,
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
                base_class: string_field(value, "baseClass")
                    .and_then(|base| base.split_whitespace().next().map(str::to_string))
                    .unwrap_or_default(),
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
                    index
                        .candidate_methods
                        .insert(symbol.clone(), member.clone());
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

fn inherit_simple_instance_members(
    methods: &mut std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
) {
    let keys = methods.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        inherit_simple_instance_members_for_class(methods, &key);
    }
}

fn inherit_simple_instance_members_for_class(
    methods: &mut std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
    key: &str,
) {
    let Some(index) = methods.get(key) else {
        return;
    };
    let base_class = index.base_class.clone();
    if base_class.is_empty() || base_class == key {
        return;
    }
    inherit_simple_instance_members_for_class(methods, &base_class);
    let Some(base) = methods.get(&base_class).cloned() else {
        return;
    };
    let Some(index) = methods.get_mut(key) else {
        return;
    };
    index.properties.extend(base.properties);
    index.property_definitions.extend(base.property_definitions);
    index.parameters.extend(base.parameters);
    index.methods.extend(base.methods);
    index.composition_methods.extend(base.composition_methods);
    index
        .binary_property_methods
        .extend(base.binary_property_methods);
    index
        .constant_parameter_methods
        .extend(base.constant_parameter_methods);
    index.candidate_methods.extend(base.candidate_methods);
}

fn classify_class_property_methods(
    methods: &mut std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
) {
    let keys = methods.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        let Some(mut index) = methods.get(&key).cloned() else {
            continue;
        };
        for (symbol, method) in &index.candidate_methods {
            let empty_arguments = Vec::new();
            let arguments = method
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            let Some(body) = method.get("body") else {
                continue;
            };
            if !arguments.is_empty() || string_field(body, "kind").as_deref() != Some("Return") {
                continue;
            }
            let Some(expression) = body.get("expr") else {
                continue;
            };
            if class_property_expression(expression, methods, &key).is_some() {
                index
                    .binary_property_methods
                    .insert(symbol.clone(), method.clone());
            }
        }
        methods.insert(key, index);
    }
}

fn class_property_expression(
    value: &Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
    class_name: &str,
) -> Option<Value> {
    match string_field(value, "kind").as_deref()? {
        "Call" => {
            let subroutine = string_field(value, "subroutine")?;
            let receiver = value.get("thisClass")?;
            let (receiver_path, class_name) = class_method_receiver_path(receiver, classes)?;
            let index = classes.get(&class_name)?;
            let method = index.methods.get(&subroutine)?;
            let expression = method.get("body")?.get("expr")?;
            let property_symbol = string_field(expression, "symbol")?;
            let property_name = index.properties.get(&property_symbol)?;
            Some(serde_json::json!({
                "kind": "NamedValue",
                "type": expression.get("type"),
                "symbol": format!("{receiver_path}.{property_name}"),
            }))
        }
        "NamedValue" => {
            let property_symbol = string_field(value, "symbol")?;
            let index = classes.get(class_name)?;
            let property_name = index.properties.get(&property_symbol)?;
            Some(serde_json::json!({
                "kind": "NamedValue",
                "type": value.get("type"),
                "symbol": property_name,
            }))
        }
        "BinaryOp" if string_field(value, "op").as_deref() == Some("Add") => {
            let left = class_property_expression(value.get("left")?, classes, class_name)?;
            let right = class_property_expression(value.get("right")?, classes, class_name)?;
            Some(serde_json::json!({
                "kind": "BinaryOp",
                "type": value.get("type"),
                "op": "Add",
                "left": left,
                "right": right,
            }))
        }
        _ => None,
    }
}

pub(super) fn collect_global_class_properties(
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
    if string_field(value, "kind").as_deref() == Some("NamedValue") {
        return string_field(value, "symbol");
    }
    if string_field(value, "kind").as_deref() != Some("Conversion") {
        return None;
    }
    conversion_property_symbol(value.get("operand")?)
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

pub(super) fn canonicalize_simple_instance_method_calls(
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
                    if dynamic_receivers.contains(receiver_path.as_str()) {
                        return None;
                    }
                    let expression = method.get("body")?.get("expr")?;
                    if binary_method_properties(expression, &index.properties).is_none() {
                        let mut aggregate =
                            class_property_expression(expression, classes, &class_name)?;
                        prefix_class_property_expression(&mut aggregate, &receiver_path);
                        *value = aggregate;
                        return Some(());
                    }
                    let (left_symbol, right_symbol) =
                        binary_method_properties(expression, &index.properties)?;
                    let left_name = index.properties.get(&left_symbol)?;
                    let right_name = index.properties.get(&right_symbol)?;
                    let left_source = expression.get("left")?;
                    let right_source = expression.get("right")?;
                    let mut left_operand = left_source;
                    let mut right_operand = right_source;
                    if string_field(left_operand, "kind").as_deref() == Some("Conversion") {
                        left_operand = left_operand.get("operand")?;
                    }
                    if string_field(right_operand, "kind").as_deref() == Some("Conversion") {
                        right_operand = right_operand.get("operand")?;
                    }
                    let left_value =
                        if string_field(left_source, "kind").as_deref() == Some("Conversion") {
                            serde_json::json!({
                                "kind": "Conversion",
                                "type": left_source.get("type"),
                                "operand": {
                                    "kind": "NamedValue",
                                    "type": left_operand.get("type"),
                                    "symbol": format!("{receiver_path}.{left_name}"),
                                },
                            })
                        } else {
                            serde_json::json!({
                                "kind": "NamedValue",
                                "type": left_operand.get("type"),
                                "symbol": format!("{receiver_path}.{left_name}"),
                            })
                        };
                    let right_value =
                        if string_field(right_source, "kind").as_deref() == Some("Conversion") {
                            serde_json::json!({
                                "kind": "Conversion",
                                "type": right_source.get("type"),
                                "operand": {
                                    "kind": "NamedValue",
                                    "type": right_operand.get("type"),
                                    "symbol": format!("{receiver_path}.{right_name}"),
                                },
                            })
                        } else {
                            serde_json::json!({
                                "kind": "NamedValue",
                                "type": right_operand.get("type"),
                                "symbol": format!("{receiver_path}.{right_name}"),
                            })
                        };
                    *value = serde_json::json!({
                        "kind": "BinaryOp",
                        "type": expression.get("type"),
                        "op": "Add",
                        "left": left_value,
                        "right": right_value,
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

fn prefix_class_property_expression(value: &mut Value, receiver_path: &str) -> Option<()> {
    if string_field(value, "kind").as_deref() == Some("NamedValue") {
        if let Some(fields) = value.as_object_mut() {
            let symbol = fields.get("symbol")?.as_str()?.to_string();
            fields.insert(
                "symbol".into(),
                Value::String(format!("{receiver_path}.{symbol}")),
            );
        }
        return Some(());
    }
    match value {
        Value::Object(fields) => {
            for nested in fields.values_mut() {
                prefix_class_property_expression(nested, receiver_path)?;
            }
            Some(())
        }
        Value::Array(values) => {
            for nested in values {
                prefix_class_property_expression(nested, receiver_path)?;
            }
            Some(())
        }
        _ => Some(()),
    }
}

pub(super) fn canonicalize_dynamic_parameter_virtual_calls(
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
                if class_parameter_environment(source_key, definitions)
                    .is_some_and(|parameters| parameters.is_empty())
                    && class_parameter_environment(&receiver_key, definitions)
                        .is_some_and(|parameters| parameters.is_empty())
                {
                    if let Some(conditional) = dynamic_simple_virtual_call(
                        value,
                        receiver_name,
                        &receiver_key,
                        source_key,
                        definitions,
                    ) {
                        *value = conditional;
                        return;
                    }
                }
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

fn dynamic_simple_virtual_call(
    value: &Value,
    receiver_name: &str,
    receiver_key: &str,
    source_key: &str,
    definitions: &std::collections::BTreeMap<String, Value>,
) -> Option<Value> {
    let method_name = string_field(value, "subroutine")?
        .split_whitespace()
        .next_back()?
        .to_string();
    let source_method = virtual_simple_method(source_key, &method_name, definitions)?;
    let fallback_method = virtual_simple_method(receiver_key, &method_name, definitions)?;
    let source_expression = source_method.get("body")?.get("expr")?.clone();
    let fallback_expression = fallback_method.get("body")?.get("expr")?.clone();
    let source_expression = substitute_class_properties(&source_expression, receiver_name);
    let fallback_expression = substitute_class_properties(&fallback_expression, receiver_name);
    let source_id = definitions
        .keys()
        .position(|class_key| class_key == source_key)
        .map(|index| index as u64 + 1)?;
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
    Some(serde_json::json!({
        "kind": "ConditionalOp",
        "type": value.get("type"),
        "conditions": [{"expr": condition}],
        "left": source_expression,
        "right": fallback_expression,
    }))
}

fn virtual_simple_method(
    class_key: &str,
    method_name: &str,
    definitions: &std::collections::BTreeMap<String, Value>,
) -> Option<Value> {
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
        let empty_arguments = Vec::new();
        let arguments = method
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if is_virtual
            && arguments.is_empty()
            && string_field(method.get("body")?, "kind").as_deref() == Some("Return")
        {
            return Some(method.clone());
        }
        return None;
    }
    None
}

fn substitute_class_properties(value: &Value, receiver_name: &str) -> Value {
    if matches!(
        string_field(value, "kind").as_deref(),
        Some("NamedValue" | "HierarchicalValue")
    ) {
        if let Some(symbol) = string_field(value, "symbol") {
            if symbol.split_whitespace().next_back() != Some("this") {
                let property_name = symbol.split_whitespace().next_back().unwrap_or_default();
                return serde_json::json!({
                    "kind": "NamedValue",
                    "type": value.get("type"),
                    "symbol": format!("{receiver_name}.{property_name}"),
                });
            }
        }
    }
    match value {
        Value::Object(fields) => {
            let mut result = serde_json::Map::new();
            for (key, nested) in fields {
                result.insert(
                    key.clone(),
                    substitute_class_properties(nested, receiver_name),
                );
            }
            Value::Object(result)
        }
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|nested| substitute_class_properties(nested, receiver_name))
                .collect(),
        ),
        scalar => scalar.clone(),
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
    resolve_class_method_receiver_path(value, classes)
}

pub(super) fn canonicalize_simple_class_property_reads(
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

pub(super) fn canonicalize_class_handle_comparisons(
    value: &mut Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
) {
    if string_field(value, "kind").as_deref() == Some("BinaryOp")
        && matches!(
            string_field(value, "op").as_deref(),
            Some("Equality" | "Inequality")
        )
    {
        if let (Some(left), Some(right)) = (value.get("left"), value.get("right")) {
            if dynamic_class_array_selection(left, classes).is_some()
                || dynamic_class_array_selection(right, classes).is_some()
            {
                if let Some(comparison) =
                    dynamic_class_handle_comparison(value, left, right, classes)
                {
                    *value = comparison;
                    return;
                }
            }
            if let (Some(left_id), Some(right_id)) = (
                class_handle_identity(left, classes),
                class_handle_identity(right, classes),
            ) {
                let operator = if string_field(value, "op").as_deref() == Some("Equality") {
                    "__eda_class_handle_eq"
                } else {
                    "__eda_class_handle_ne"
                };
                *value = serde_json::json!({
                    "kind": "Call",
                    "type": "int",
                    "subroutine": operator,
                    "arguments": [left_id, right_id],
                });
                return;
            }
        }
    }
    if let Value::Array(values) = value {
        for nested in values {
            canonicalize_class_handle_comparisons(nested, classes);
        }
        return;
    }
    let kind = string_field(value, "kind");
    let Value::Object(fields) = value else {
        return;
    };
    for nested in fields.values_mut() {
        canonicalize_class_handle_comparisons(nested, classes);
    }
    if kind.as_deref() == Some("Conversion")
        && fields
            .get("operand")
            .and_then(|operand| string_field(operand, "subroutine"))
            .is_some_and(|subroutine| {
                matches!(
                    subroutine.as_str(),
                    "__eda_class_handle_eq" | "__eda_class_handle_ne"
                )
            })
    {
        if let Some(marker) = fields.get("operand").cloned() {
            *value = marker;
        }
    }
}

fn dynamic_class_handle_comparison(
    value: &Value,
    left: &Value,
    right: &Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
) -> Option<Value> {
    let (left_id, right_id) = if dynamic_class_array_selection(left, classes).is_some() {
        (
            dynamic_class_array_identity(left, classes)?,
            class_handle_identity(right, classes)?,
        )
    } else {
        (
            class_handle_identity(left, classes)?,
            dynamic_class_array_identity(right, classes)?,
        )
    };
    let operator = if string_field(value, "op").as_deref() == Some("Equality") {
        "__eda_class_handle_eq"
    } else {
        "__eda_class_handle_ne"
    };
    Some(serde_json::json!({
        "kind": "Call",
        "type": "int",
        "subroutine": operator,
        "arguments": [left_id, right_id],
    }))
}

fn dynamic_class_array_identity(
    value: &Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
) -> Option<Value> {
    let (selector, lower, upper) = dynamic_class_array_selection(value, classes)?;
    let indexes = if lower <= upper {
        (lower..=upper).collect::<Vec<_>>()
    } else {
        (upper..=lower).rev().collect::<Vec<_>>()
    };
    let mut result = serde_json::json!({
        "kind": "IntegerLiteral",
        "type": "logic[63:0]",
        "value": "0",
    });
    for index in indexes.iter().rev() {
        let mut candidate = value.clone();
        if !replace_first_dynamic_class_array_selection(&mut candidate, classes, *index) {
            return None;
        }
        let identity = class_handle_identity(&candidate, classes)?;
        let selector_type = selector.get("type")?.clone();
        let condition = serde_json::json!({
            "kind": "BinaryOp",
            "type": "bit",
            "op": "Equality",
            "left": selector,
            "right": {
                "kind": "IntegerLiteral",
                "type": selector_type,
                "value": index.to_string(),
            },
        });
        result = serde_json::json!({
            "kind": "ConditionalOp",
            "type": "logic[63:0]",
            "conditions": [{"expr": condition}],
            "left": identity,
            "right": result,
        });
    }
    Some(result)
}

fn class_handle_identity(
    value: &Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
) -> Option<Value> {
    if string_field(value, "kind").as_deref() == Some("NullLiteral") {
        return Some(serde_json::json!({
            "kind": "IntegerLiteral",
            "type": "logic[63:0]",
            "value": "0",
        }));
    }
    let (path, class_name) = if matches!(
        string_field(value, "kind").as_deref(),
        Some("NamedValue" | "HierarchicalValue")
    ) {
        let symbol = string_field(value, "symbol")?;
        let name = symbol.split_whitespace().next_back()?.to_string();
        let class_name = class_type_key(&string_field(value, "type")?);
        (name, class_name)
    } else {
        class_method_receiver_path(value, classes)?
    };
    classes.contains_key(&class_name).then(|| {
        serde_json::json!({
            "kind": "NamedValue",
            "type": "logic[63:0]",
            "symbol": format!("__class_object_id_{path}"),
        })
    })
}

pub(super) fn canonicalize_dynamic_class_array_reads(
    value: &mut Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
) {
    let is_call = string_field(value, "kind").as_deref() == Some("Call")
        && value
            .get("thisClass")
            .is_some_and(|receiver| dynamic_class_array_selection(receiver, classes).is_some());
    let is_property = string_field(value, "kind").as_deref() == Some("MemberAccess")
        && value
            .get("value")
            .is_some_and(|receiver| dynamic_class_array_selection(receiver, classes).is_some());
    if is_call || is_property {
        if let Some(conditional) = dynamic_class_array_read_conditional(value, classes, is_call) {
            *value = conditional;
            return;
        }
    }
    if let Value::Array(values) = value {
        for nested in values {
            canonicalize_dynamic_class_array_reads(nested, classes);
        }
        return;
    }
    let Value::Object(fields) = value else {
        return;
    };
    for nested in fields.values_mut() {
        canonicalize_dynamic_class_array_reads(nested, classes);
    }
}

fn dynamic_class_array_read_conditional(
    value: &Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
    is_call: bool,
) -> Option<Value> {
    let receiver = if is_call {
        value.get("thisClass")?
    } else {
        value.get("value")?
    };
    let (selector, lower, upper) = dynamic_class_array_selection(receiver, classes)?;
    let indexes = if lower <= upper {
        (lower..=upper).collect::<Vec<_>>()
    } else {
        (upper..=lower).rev().collect::<Vec<_>>()
    };
    let expression_type = value.get("type")?.clone();
    let width = crate::types::simple_type_width(&string_field(value, "type")?)?;
    let mut result = serde_json::json!({
        "kind": "IntegerLiteral",
        "type": expression_type,
        "value": format!("{width}'b{}", "x".repeat(width)),
    });

    for index in indexes.iter().rev() {
        let mut candidate = value.clone();
        if !replace_first_dynamic_class_array_selection(&mut candidate, classes, *index) {
            return None;
        }
        if is_call {
            let _ = canonicalize_simple_instance_method_calls(
                &mut candidate,
                classes,
                &std::collections::BTreeSet::new(),
            );
            if string_field(&candidate, "kind").as_deref() == Some("Call") {
                return None;
            }
        } else {
            canonicalize_simple_class_property_reads(&mut candidate, classes);
            if string_field(&candidate, "kind").as_deref() != Some("NamedValue") {
                return None;
            }
        }
        let selector_type = selector.get("type")?.clone();
        let condition = serde_json::json!({
            "kind": "BinaryOp",
            "type": {
                "name": "bit",
                "kind": "ScalarType",
                "isSigned": false,
            },
            "op": "Equality",
            "left": selector,
            "right": {
                "kind": "IntegerLiteral",
                "type": selector_type,
                "value": index.to_string(),
            },
        });
        result = serde_json::json!({
            "kind": "Call",
            "type": expression_type,
            "subroutine": "__eda_class_array_read",
            "arguments": [condition, candidate, result],
        });
    }
    Some(result)
}

fn dynamic_class_array_selection(
    value: &Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
) -> Option<(Value, i64, i64)> {
    if string_field(value, "kind").as_deref() == Some("ElementSelect") {
        let selected = value.get("value")?;
        let selected_type = string_field(selected, "type")?;
        if let Some(element_class) = class_array_element_key(&selected_type) {
            let selector = value.get("selector")?;
            let selector_is_constant = integer_literal_word(selector).is_some()
                || string_field(selector, "constant").is_some();
            if !selector_is_constant && classes.contains_key(&element_class) {
                let (lower, upper) = fixed_class_array_bounds(&selected_type)?;
                return Some((selector.clone(), lower, upper));
            }
        }
    }
    match value {
        Value::Object(fields) => fields
            .values()
            .find_map(|nested| dynamic_class_array_selection(nested, classes)),
        Value::Array(values) => values
            .iter()
            .find_map(|nested| dynamic_class_array_selection(nested, classes)),
        _ => None,
    }
}

fn fixed_class_array_bounds(type_name: &str) -> Option<(i64, i64)> {
    let start = type_name.find("$[")? + 2;
    let end = type_name.rfind(']')?;
    if end <= start {
        return None;
    }
    let (left, right) = type_name[start..end].split_once(':')?;
    Some((
        left.trim().parse::<i64>().ok()?,
        right.trim().parse::<i64>().ok()?,
    ))
}

fn replace_first_dynamic_class_array_selection(
    value: &mut Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
    index: i64,
) -> bool {
    if string_field(value, "kind").as_deref() == Some("ElementSelect") {
        let selected_type = value
            .get("value")
            .and_then(|selected| string_field(selected, "type"))
            .unwrap_or_default();
        if let Some(element_class) = class_array_element_key(&selected_type) {
            let selector_is_constant = value.get("selector").is_some_and(|selector| {
                integer_literal_word(selector).is_some()
                    || string_field(selector, "constant").is_some()
            });
            if !selector_is_constant && classes.contains_key(&element_class) {
                let selector_type = value
                    .get("selector")
                    .and_then(|selector| selector.get("type"))
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({"name": "int"}));
                if let Some(fields) = value.as_object_mut() {
                    fields.insert(
                        "selector".into(),
                        serde_json::json!({
                            "kind": "IntegerLiteral",
                            "type": selector_type,
                            "value": index.to_string(),
                        }),
                    );
                }
                return true;
            }
        }
    }
    match value {
        Value::Object(fields) => fields
            .values_mut()
            .any(|nested| replace_first_dynamic_class_array_selection(nested, classes, index)),
        Value::Array(values) => values
            .iter_mut()
            .any(|nested| replace_first_dynamic_class_array_selection(nested, classes, index)),
        _ => false,
    }
}

fn simple_class_property_path(
    value: &Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
) -> Option<(String, String)> {
    resolve_class_property_path(value, classes)
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

fn class_array_element_key(type_name: &str) -> Option<String> {
    let separator = type_name.find('$')?;
    let element = type_name[..separator].trim();
    (!element.is_empty()).then(|| class_type_key(element))
}

#[derive(Clone)]
enum ClassPathEvent {
    Member(String),
    Selector(String),
}

fn collect_class_path_events(value: &Value) -> Option<(String, String, Vec<ClassPathEvent>)> {
    let mut events = Vec::new();
    let mut receiver = value;
    loop {
        match string_field(receiver, "kind").as_deref() {
            Some("MemberAccess") => {
                events.push(ClassPathEvent::Member(string_field(receiver, "member")?));
                receiver = receiver.get("value")?;
            }
            Some("ElementSelect") => {
                let selector = receiver.get("selector")?;
                let selector = string_field(selector, "constant")
                    .or_else(|| string_field(selector, "value"))?;
                events.push(ClassPathEvent::Selector(selector));
                receiver = receiver.get("value")?;
            }
            _ => break,
        }
    }
    if !matches!(
        string_field(receiver, "kind").as_deref(),
        Some("NamedValue" | "HierarchicalValue")
    ) {
        return None;
    }
    let symbol = string_field(receiver, "symbol")?;
    let receiver_name = symbol.split_whitespace().next_back()?.to_string();
    let receiver_type = string_field(receiver, "type")?;
    events.reverse();
    Some((receiver_name, receiver_type, events))
}

fn resolve_class_property_path(
    value: &Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
) -> Option<(String, String)> {
    let (symbol, current_type, events) = collect_class_path_events(value)?;
    let mut symbol = symbol;
    let mut current_type = current_type;
    let mut current_class = if current_type.contains('$') {
        None
    } else {
        Some(class_type_key(&current_type))
    };
    let mut ends_with_property = false;

    for event in events {
        match event {
            ClassPathEvent::Member(member) => {
                let index = classes.get(current_class.as_deref()?)?;
                let property = index.property_definitions.get(&member)?;
                if string_field(property, "flags")
                    .is_some_and(|flags| flags.split(',').any(|flag| flag.trim() == "static"))
                {
                    return None;
                }
                let property_name = member.split_whitespace().next_back()?.to_string();
                symbol.push('.');
                symbol.push_str(&property_name);
                current_type = string_field(property, "type")?;
                if current_type.contains('$') {
                    current_class = None;
                } else {
                    current_class = Some(class_type_key(&current_type));
                }
                ends_with_property = true;
            }
            ClassPathEvent::Selector(selector) => {
                let element_class = class_array_element_key(&current_type)?;
                symbol.push('[');
                symbol.push_str(&selector);
                symbol.push(']');
                current_class = Some(element_class);
                current_type = current_class.clone().unwrap_or_default();
                ends_with_property = false;
            }
        }
    }
    if !ends_with_property || current_type.contains('$') {
        return None;
    }
    simple_type_width(&current_type)?;
    Some((current_type, symbol))
}

fn resolve_class_method_receiver_path(
    value: &Value,
    classes: &std::collections::BTreeMap<String, SimpleInstanceMethodIndex>,
) -> Option<(String, String)> {
    let (symbol, receiver_type, events) = collect_class_path_events(value)?;
    let mut symbol = symbol;
    let mut current_type = receiver_type;
    let mut current_class = if current_type.contains('$') {
        None
    } else {
        Some(class_type_key(&current_type))
    };

    for event in events {
        match event {
            ClassPathEvent::Member(member) => {
                let index = classes.get(current_class.as_deref()?)?;
                let property = index.property_definitions.get(&member)?;
                let property_name = member.split_whitespace().next_back()?.to_string();
                symbol.push('.');
                symbol.push_str(&property_name);
                current_type = string_field(property, "type")?;
                if current_type.contains('$') {
                    current_class = None;
                } else {
                    current_class = Some(class_type_key(&current_type));
                }
            }
            ClassPathEvent::Selector(selector) => {
                let element_class = class_array_element_key(&current_type)?;
                symbol.push('[');
                symbol.push_str(&selector);
                symbol.push(']');
                current_class = Some(element_class);
                current_type = current_class.clone().unwrap_or_default();
            }
        }
    }
    let current_class = current_class?;
    classes
        .contains_key(&current_class)
        .then_some((symbol, current_class))
}

pub(super) fn collect_dynamic_class_receivers(
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

pub(super) fn collect_class_definitions(
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

pub(super) fn collect_dynamic_class_sources(
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
