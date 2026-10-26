//! Typed summaries and structural validation for slang's elaborated AST.

use super::ast_util::{
    canonical_detailed_type, constant_json_word, expression_symbol_id, expression_target_names,
    instance_parameters, integer_literal_word, literal_word, normalize_detailed_types,
    normalize_instance_bodies, parameter_summary, source_span, string_field,
};
use crate::types::{simple_type_signed, simple_type_width};
use eda_core::ir::Design;
use expressions::{
    collect_generate_summary, collect_kinds, empty_expression_summary, expression_summary,
    procedural_assignments, ClassHandleTypes,
};

mod classes;
mod expressions;
mod memory_files;
mod patterns;
mod tasks;
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
    if string_field(body, "kind").as_deref() == Some("Return") {
        let mut environment = environment.clone();
        initialize_simple_function_locals(definition, &mut environment)?;
        let expression = simple_function_statement_expression(
            body,
            &mut environment,
            &string_field(definition, "returnType").unwrap_or_default(),
        )?;
        return Some(serde_json::json!({
            "kind": "Conversion",
            "type": string_field(definition, "returnType")?,
            "operand": expression,
        }));
    }
    if string_field(body, "kind").as_deref() == Some("List") {
        let mut environment = environment.clone();
        initialize_simple_function_locals(definition, &mut environment)?;
        if let Some(expression) = simple_function_statement_expression(
            body,
            &mut environment,
            &string_field(definition, "returnType").unwrap_or_default(),
        ) {
            return Some(serde_json::json!({
                "kind": "Conversion",
                "type": string_field(definition, "returnType")?,
                "operand": expression,
            }));
        }
    }
    let mut environment = environment.clone();
    initialize_simple_function_locals(definition, &mut environment)?;
    if let Some(expression) = simple_function_statement_expression(
        body,
        &mut environment,
        &string_field(definition, "returnType").unwrap_or_default(),
    ) {
        return Some(serde_json::json!({
            "kind": "Conversion",
            "type": string_field(definition, "returnType")?,
            "operand": expression,
        }));
    }
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
    let expression = substitute_simple_function_values(&expression, &environment)?;
    let return_type = string_field(definition, "returnType")?;
    Some(serde_json::json!({
        "kind": "Conversion",
        "type": return_type,
        "operand": expression,
    }))
}

fn initialize_simple_function_locals(
    definition: &Value,
    environment: &mut std::collections::BTreeMap<String, Value>,
) -> Option<()> {
    fn collect(value: &Value, environment: &mut std::collections::BTreeMap<String, Value>) {
        if let Some(fields) = value.as_object() {
            if let Some(initializer) = value.get("initializer") {
                if let (Some(address), Some(name)) = (
                    value.get("addr").and_then(Value::as_u64),
                    string_field(value, "name"),
                ) {
                    if let Some(initializer) =
                        substitute_simple_function_values(initializer, environment)
                    {
                        environment.insert(format!("{address} {name}"), initializer);
                    }
                }
            }
            for nested in fields.values() {
                collect(nested, environment);
            }
        } else if let Some(values) = value.as_array() {
            for nested in values {
                collect(nested, environment);
            }
        }
    }
    collect(definition, environment);
    Some(())
}

fn simple_function_statement_expression(
    statement: &Value,
    environment: &mut std::collections::BTreeMap<String, Value>,
    return_type: &str,
) -> Option<Value> {
    match string_field(statement, "kind")?.as_str() {
        "List" | "Block" => {
            if let Some(body) = statement.get("body") {
                return simple_function_statement_expression(body, environment, return_type);
            }
            if let Some(statements) = statement.get("list").and_then(Value::as_array) {
                for nested in statements {
                    if let Some(expression) =
                        simple_function_statement_expression(nested, environment, return_type)
                    {
                        return Some(expression);
                    }
                }
            }
            None
        }
        "Return" => substitute_simple_function_values(statement.get("expr")?, environment),
        "ExpressionStatement" => {
            apply_simple_function_assignment(statement.get("expr")?, environment);
            None
        }
        "Assignment" => {
            apply_simple_function_assignment(statement, environment);
            None
        }
        "ForLoop" => {
            if let Some(initializers) = statement.get("initializers").and_then(Value::as_array) {
                for initializer in initializers {
                    if let Some(expression) =
                        simple_function_statement_expression(initializer, environment, return_type)
                    {
                        return Some(expression);
                    }
                }
            }
            let stop = statement.get("stopExpr")?;
            let body = statement.get("body")?;
            let steps = statement.get("steps")?.as_array()?;
            for _ in 0..=256 {
                let stop = substitute_simple_function_values(stop, environment)?;
                if simple_function_constant_i64(&stop)? == 0 {
                    return None;
                }
                if let Some(expression) =
                    simple_function_statement_expression(body, environment, return_type)
                {
                    return Some(expression);
                }
                for step in steps {
                    if let Some(expression) =
                        simple_function_statement_expression(step, environment, return_type)
                    {
                        return Some(expression);
                    }
                }
            }
            None
        }
        "WhileLoop" => {
            let condition = statement.get("cond")?;
            let body = statement.get("body")?;
            for _ in 0..=256 {
                let condition = substitute_simple_function_values(condition, environment)?;
                if simple_function_constant_i64(&condition)? == 0 {
                    return None;
                }
                if let Some(expression) =
                    simple_function_statement_expression(body, environment, return_type)
                {
                    return Some(expression);
                }
            }
            None
        }
        "RepeatLoop" => {
            let count = substitute_simple_function_values(statement.get("count")?, environment)?;
            let count = usize::try_from(simple_function_constant_i64(&count)?).ok()?;
            let body = statement.get("body")?;
            for _ in 0..count {
                if let Some(expression) =
                    simple_function_statement_expression(body, environment, return_type)
                {
                    return Some(expression);
                }
            }
            None
        }
        "Conditional" => {
            let condition = statement
                .get("conditions")?
                .as_array()?
                .first()?
                .get("expr")?;
            let condition = substitute_simple_function_values(condition, environment)?;
            let if_true = statement.get("ifTrue")?;
            let if_false = statement.get("ifFalse");
            let mut true_environment = environment.clone();
            let true_return =
                simple_function_statement_expression(if_true, &mut true_environment, return_type);
            let mut false_environment = environment.clone();
            let false_return = if_false.and_then(|statement| {
                simple_function_statement_expression(statement, &mut false_environment, return_type)
            });
            if let (Some(left), Some(right)) = (true_return, false_return) {
                return Some(serde_json::json!({
                    "kind": "ConditionalOp",
                    "type": return_type,
                    "conditions": [{"expr": condition}],
                    "left": left,
                    "right": right,
                }));
            }

            let true_target = conditional_assignment_target(if_true)?;
            let true_value = true_environment.get(&true_target)?.clone();
            let false_value = if let Some(if_false) = if_false {
                let false_target = conditional_assignment_target(if_false)?;
                false_environment.get(&false_target)?.clone()
            } else {
                environment.get(&true_target)?.clone()
            };
            let value = serde_json::json!({
                "kind": "ConditionalOp",
                "type": return_type,
                "conditions": [{"expr": condition}],
                "left": true_value,
                "right": false_value,
            });
            environment.insert(true_target, value);
            None
        }
        _ => None,
    }
}

fn apply_simple_function_assignment(
    assignment: &Value,
    environment: &mut std::collections::BTreeMap<String, Value>,
) -> Option<()> {
    if string_field(assignment, "kind").as_deref() != Some("Assignment") {
        return None;
    }
    let target = string_field(assignment.get("left")?, "symbol")?;
    let value = substitute_simple_function_values(assignment.get("right")?, environment)?;
    environment.insert(target, value);
    Some(())
}

fn simple_function_constant_i64(value: &Value) -> Option<i64> {
    if let Some(word) = integer_literal_word(value).or_else(|| constant_json_word(value)) {
        return simple_function_word_i64(&word, &string_field(value, "type").unwrap_or_default());
    }
    match string_field(value, "kind").as_deref() {
        Some("Conversion") => simple_function_constant_i64(value.get("operand")?),
        Some("UnaryOp") => {
            let operand = simple_function_constant_i64(value.get("operand")?)?;
            match string_field(value, "op").as_deref() {
                Some("Plus") => Some(operand),
                Some("Minus") => operand.checked_neg(),
                _ => None,
            }
        }
        Some("BinaryOp") => {
            let left = simple_function_constant_i64(value.get("left")?)?;
            let right = simple_function_constant_i64(value.get("right")?)?;
            match string_field(value, "op").as_deref() {
                Some("Add") => left.checked_add(right),
                Some("Subtract") => left.checked_sub(right),
                Some("Multiply") => left.checked_mul(right),
                Some("LessThan") => Some((left < right) as i64),
                Some("LessThanEqual") => Some((left <= right) as i64),
                Some("GreaterThan") => Some((left > right) as i64),
                Some("GreaterThanEqual") => Some((left >= right) as i64),
                Some("Equality") => Some((left == right) as i64),
                Some("Inequality") => Some((left != right) as i64),
                _ => None,
            }
        }
        Some("ConditionalOp") => {
            let condition = value.get("conditions")?.as_array()?.first()?.get("expr")?;
            let selected = if simple_function_constant_i64(condition)? != 0 {
                "left"
            } else {
                "right"
            };
            simple_function_constant_i64(value.get(selected)?)
        }
        _ => None,
    }
}

fn simple_function_word_i64(word: &eda_core::logic::LogicWord, type_name: &str) -> Option<i64> {
    if word.width() == 0 || word.width() > u64::BITS as usize || !word.is_fully_known() {
        return None;
    }
    let mut magnitude = 0_i128;
    for (bit, value) in word.bits.iter().enumerate() {
        if value.as_bool() == Some(true) {
            magnitude |= 1_i128 << bit;
        }
    }
    let signed = simple_type_signed(type_name).unwrap_or(false);
    let sign_bit = 1_i128 << (word.width() - 1);
    let signed_value = if signed && magnitude & sign_bit != 0 {
        magnitude - (1_i128 << word.width())
    } else {
        magnitude
    };
    i64::try_from(signed_value).ok()
}

fn conditional_assignment_target(statement: &Value) -> Option<String> {
    if matches!(
        string_field(statement, "kind").as_deref(),
        Some("List" | "Block")
    ) {
        if let Some(body) = statement.get("body") {
            return conditional_assignment_target(body);
        }
        let statements = statement
            .get("list")
            .or_else(|| statement.get("body"))
            .and_then(Value::as_array)?;
        let assignment = statements
            .iter()
            .find(|nested| string_field(nested, "kind").as_deref() == Some("ExpressionStatement"))?
            .get("expr")?;
        return string_field(assignment.get("left")?, "symbol");
    }
    let assignment = statement.get("expr")?;
    string_field(assignment.get("left")?, "symbol")
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
        Some("PackedStructType" | "PackedUnionType" | "UnpackedStructType" | "UnpackedUnionType")
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
        let Some(width) = crate::types::simple_type_width(&type_name).or_else(|| {
            lookup_type_layout(&type_name, existing).map(|layout| layout.element_width)
        }) else {
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

fn collect_type_alias_members(value: &Value, aliases: &mut Vec<Value>) {
    if let Some(fields) = value.as_object() {
        if fields.get("kind").and_then(Value::as_str) == Some("TypeAlias") {
            aliases.push(value.clone());
        }
        for nested in fields.values() {
            collect_type_alias_members(nested, aliases);
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_type_alias_members(nested, aliases);
        }
    }
}

#[derive(Default)]
struct SimpleTypeAliasWidths {
    by_address: std::collections::BTreeMap<u64, usize>,
    by_name: std::collections::BTreeMap<String, Vec<usize>>,
}

fn collect_simple_type_alias_widths(aliases: &[Value], widths: &mut SimpleTypeAliasWidths) {
    let mut pending = aliases.to_vec();
    let mut changed = true;
    while changed {
        changed = false;
        pending.retain(|alias| {
            let Some(name) = string_field(alias, "name") else {
                return false;
            };
            let Some(address) = alias.get("addr").and_then(Value::as_u64) else {
                return false;
            };
            let Some(target) = alias.get("target") else {
                return false;
            };
            let target_name = if target.is_string() {
                target.as_str().unwrap_or_default().to_string()
            } else {
                match canonical_detailed_type(target) {
                    Some(target_name) => target_name,
                    None => return true,
                }
            };
            let width = simple_type_width(&target_name)
                .or_else(|| lookup_simple_type_alias_width(&target_name, widths));
            let Some(width) = width else {
                return true;
            };
            if widths.by_address.insert(address, width).is_none() {
                widths.by_name.entry(name).or_default().push(width);
                changed = true;
            }
            false
        });
    }
}

fn lookup_simple_type_alias_width(
    type_name: &str,
    widths: &SimpleTypeAliasWidths,
) -> Option<usize> {
    let (address, name) = type_name.split_once(' ')?;
    if let Some(width) = address
        .parse::<u64>()
        .ok()
        .and_then(|address| widths.by_address.get(&address))
    {
        return Some(*width);
    }
    let name = name.rsplit(['.', '#', ':']).next()?;
    let matches = widths.by_name.get(name)?;
    (matches.len() == 1).then(|| matches[0])
}

fn annotate_simple_type_alias_widths(
    expression: &mut SlangExpressionSummary,
    widths: &SimpleTypeAliasWidths,
) {
    if expression.type_width.is_none() {
        let base_type = expression.type_name.split('$').next().unwrap_or_default();
        if let Some(width) = lookup_simple_type_alias_width(base_type, widths) {
            expression.type_width = Some(width);
        }
    }
    for operand in &mut expression.operands {
        annotate_simple_type_alias_widths(operand, widths);
    }
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
                if let Some(layout) = lookup_type_layout(&memory.type_name, type_layouts) {
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

fn lookup_type_layout<'a>(
    type_name: &str,
    type_layouts: &'a std::collections::BTreeMap<String, SlangTypeLayoutSummary>,
) -> Option<&'a SlangTypeLayoutSummary> {
    let alias = type_layout_alias_key(type_name)?;
    if let Some(layout) = type_layouts.get(alias) {
        return Some(layout);
    }
    let alias_address = alias.split_whitespace().next()?.parse::<u64>().ok()?;
    let address_matches = type_layouts
        .keys()
        .filter(|key| {
            key.split_whitespace()
                .next()
                .and_then(|address| address.parse::<u64>().ok())
                .is_some_and(|address| address == alias_address)
        })
        .collect::<Vec<_>>();
    if address_matches.len() == 1 {
        return Some(&type_layouts[address_matches[0]]);
    }
    let alias_name = alias
        .split_whitespace()
        .next_back()?
        .rsplit(['.', '#', ':'])
        .next()?;
    let matches = type_layouts
        .keys()
        .filter(|key| {
            key.split_whitespace()
                .next_back()
                .and_then(|name| name.rsplit(['.', '#', ':']).next())
                .is_some_and(|name| name == alias_name)
        })
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| &type_layouts[matches[0]])
}

fn annotate_packed_member_shapes(
    expression: &mut SlangExpressionSummary,
    type_layouts: &std::collections::BTreeMap<String, SlangTypeLayoutSummary>,
) -> Option<()> {
    if matches!(expression.kind.as_str(), "NamedValue" | "HierarchicalValue") {
        let layout = lookup_type_layout(&expression.type_name, type_layouts)?;
        expression.type_width = Some(layout.element_width);
        expression.type_signed.get_or_insert(
            crate::types::simple_type_signed(&expression.type_name).unwrap_or(false),
        );
    }
    if expression.kind == "MemberAccess" {
        let selected = expression.operands.first_mut()?;
        let member = expression.member_path.first()?.clone();
        let layout = lookup_type_layout(&selected.type_name, type_layouts)?;
        let field = layout
            .fields
            .iter()
            .find(|candidate| candidate.name == member)?;
        let msb = field.offset.checked_add(field.width)?.checked_sub(1)?;
        expression.member_range = Some((msb, field.offset));
        expression.type_width = Some(field.width);
        expression
            .type_signed
            .get_or_insert(crate::types::simple_type_signed(&field.type_name).unwrap_or(false));
        selected.type_width = Some(layout.element_width);
        selected
            .type_signed
            .get_or_insert(crate::types::simple_type_signed(&selected.type_name).unwrap_or(false));
    }
    for operand in &mut expression.operands {
        let _ = annotate_packed_member_shapes(operand, type_layouts);
    }
    Some(())
}

fn collect_instance_member_paths(
    value: &Value,
    root: &Value,
    paths: &mut std::collections::BTreeMap<String, String>,
) {
    if let Some(fields) = value.as_object() {
        let mut recurse = true;
        match fields.get("kind").and_then(Value::as_str) {
            Some("Instance") => {
                let instance_name = fields
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !instance_name.is_empty() {
                    if let Some(members) = fields
                        .get("body")
                        .and_then(|body| body.get("members"))
                        .and_then(Value::as_array)
                    {
                        insert_instance_member_paths(
                            members,
                            0,
                            &format!("{instance_name}."),
                            paths,
                        );
                    }
                }
            }
            Some("InstanceArray") => {
                if let Some(array_name) = fields.get("name").and_then(Value::as_str) {
                    collect_instance_array_member_paths(value, array_name, root, paths);
                }
                recurse = false;
            }
            _ => {}
        }
        if recurse {
            for nested in fields.values() {
                collect_instance_member_paths(nested, root, paths);
            }
        }
    } else if let Some(values) = value.as_array() {
        for nested in values {
            collect_instance_member_paths(nested, root, paths);
        }
    }
}

fn instance_array_indexes(member: &Value) -> Option<Vec<i64>> {
    let range = string_field(member, "range")?;
    let range = range.trim().trim_start_matches('[').trim_end_matches(']');
    let (left, right) = range.split_once(':')?;
    let left = left.trim().parse::<i64>().ok()?;
    let right = right.trim().parse::<i64>().ok()?;
    Some(if left <= right {
        (left..=right).collect()
    } else {
        (right..=left).rev().collect()
    })
}

fn collect_instance_array_leaves<'value>(
    member: &'value Value,
    indexes: &mut Vec<i64>,
    leaves: &mut Vec<(&'value Value, Vec<i64>)>,
) -> Option<()> {
    let element_indexes = instance_array_indexes(member)?;
    let elements = member.get("members")?.as_array()?;
    if element_indexes.len() != elements.len() {
        return None;
    }
    for (index, element) in element_indexes.into_iter().zip(elements) {
        indexes.push(index);
        match element.get("kind").and_then(Value::as_str) {
            Some("Instance") => leaves.push((element, indexes.clone())),
            Some("InstanceArray") => {
                collect_instance_array_leaves(element, indexes, leaves)?;
            }
            _ => return None,
        }
        indexes.pop();
    }
    Some(())
}

fn collect_instance_array_member_paths(
    member: &Value,
    array_name: &str,
    root: &Value,
    paths: &mut std::collections::BTreeMap<String, String>,
) -> Option<()> {
    let mut leaves = Vec::new();
    let mut indexes = Vec::new();
    collect_instance_array_leaves(member, &mut indexes, &mut leaves)?;
    let first = leaves.first()?.0;
    let first_address = first.get("addr")?.as_u64()?;
    let first_body = first.get("body")?;
    let first_members = first_body.get("members")?.as_array()?;

    let mut direct_symbols = std::collections::BTreeMap::<String, Vec<String>>::new();
    let top_members = root
        .get("body")
        .and_then(|body| body.get("members"))
        .and_then(Value::as_array)?;
    for member in top_members {
        if string_field(member, "kind").as_deref() == Some("ContinuousAssign") {
            if let Some(right) = member
                .get("assignment")
                .and_then(|assignment| assignment.get("right"))
            {
                collect_direct_hierarchical_symbols(right, &mut direct_symbols);
            }
        }
    }

    for (element, element_indexes) in &leaves {
        let element_address = element.get("addr")?.as_u64()?;
        let delta = i128::from(element_address) - i128::from(first_address);
        let mut path = array_name.to_string();
        for index in element_indexes {
            path.push_str(&format!("[{index}]"));
        }
        path.push('.');
        insert_instance_member_paths(first_members, delta, &path, paths);
    }

    for first_member in first_members {
        let member_name = first_member.get("name").and_then(Value::as_str)?;
        if member_name.is_empty() {
            continue;
        }
        let expected = leaves
            .iter()
            .map(|(_, indexes)| {
                let mut path = array_name.to_string();
                for index in indexes {
                    path.push_str(&format!("[{index}]"));
                }
                path.push('.');
                path.push_str(member_name);
                path
            })
            .collect::<Vec<_>>();
        if let Some(symbols) = direct_symbols.get(member_name) {
            if symbols.len() == expected.len() {
                for (symbol, path) in symbols.iter().zip(expected) {
                    paths.insert(symbol.clone(), path);
                }
            }
        }
    }
    Some(())
}

fn collect_direct_hierarchical_symbols(
    value: &Value,
    symbols: &mut std::collections::BTreeMap<String, Vec<String>>,
) {
    let Some(kind) = string_field(value, "kind") else {
        return;
    };
    if kind == "HierarchicalValue" {
        if let Some(symbol) = string_field(value, "symbol") {
            let name = symbol.split_whitespace().next_back().unwrap_or_default();
            if !name.is_empty() {
                let entries = symbols.entry(name.to_string()).or_default();
                if !entries.contains(&symbol) {
                    entries.push(symbol);
                }
            }
        }
        return;
    }
    if kind == "Instance" || kind == "InstanceArray" {
        return;
    }
    match value {
        Value::Object(fields) => {
            for nested in fields.values() {
                collect_direct_hierarchical_symbols(nested, symbols);
            }
        }
        Value::Array(values) => {
            for nested in values {
                collect_direct_hierarchical_symbols(nested, symbols);
            }
        }
        _ => {}
    }
}

fn insert_instance_member_paths(
    members: &[Value],
    address_delta: i128,
    path_prefix: &str,
    paths: &mut std::collections::BTreeMap<String, String>,
) {
    for member in members {
        if let (Some(address), Some(member_name)) = (
            member.get("addr").and_then(Value::as_u64),
            member.get("name").and_then(Value::as_str),
        ) {
            if member_name.is_empty() {
                continue;
            }
            if let Some(address) = i128::from(address)
                .checked_add(address_delta)
                .and_then(|address| u64::try_from(address).ok())
            {
                paths.insert(
                    format!("{address} {member_name}"),
                    format!("{path_prefix}{member_name}"),
                );
            }
        }
    }
}

fn annotate_interface_member_paths(
    expression: &mut SlangExpressionSummary,
    interface_members: &std::collections::BTreeMap<String, String>,
    type_layouts: &std::collections::BTreeMap<String, SlangTypeLayoutSummary>,
) -> Option<()> {
    if expression.kind == "HierarchicalValue" {
        if let Some(path) = expression
            .symbol_id
            .as_deref()
            .and_then(|symbol| interface_members.get(symbol))
        {
            expression.interface_path = Some(path.clone());
            if expression.type_width.is_none() {
                let layout = lookup_type_layout(&expression.type_name, type_layouts)?;
                expression.type_width = Some(layout.element_width);
            }
            expression.type_signed.get_or_insert(false);
        }
    }
    for operand in &mut expression.operands {
        let _ = annotate_interface_member_paths(operand, interface_members, type_layouts);
    }
    Some(())
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
    /// Authoritative slang symbol ID, including address and name.
    pub symbol_id: Option<String>,
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
    /// Source member path for a packed struct/union field access.
    pub member_path: Vec<String>,
    /// Physical inclusive bit range selected by a packed member path.
    pub member_range: Option<(usize, usize)>,
    /// Canonical hierarchical path for a direct interface member reference.
    pub interface_path: Option<String>,
    /// Slice size for a SystemVerilog streaming concatenation.
    pub streaming_slice_size: Option<usize>,
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
        tasks::canonicalize_simple_task_calls(&document, &mut root);
        memory_files::canonicalize_memory_file_reads(&mut root)?;
        let mut global_class_properties = std::collections::BTreeMap::new();
        classes::collect_global_class_properties(&document, &mut global_class_properties);
        let simple_instance_methods =
            classes::collect_simple_instance_classes(&document, &global_class_properties);
        let mut class_types = std::collections::BTreeSet::new();
        collect_class_type_names(&document, &mut class_types);
        let mut class_definitions = std::collections::BTreeMap::new();
        classes::collect_class_definitions(&document, &mut class_definitions);
        let mut dynamic_class_sources = std::collections::BTreeMap::new();
        classes::collect_dynamic_class_sources(
            &root,
            &class_definitions,
            &mut dynamic_class_sources,
        );
        let mut dynamic_class_receivers = std::collections::BTreeSet::new();
        classes::collect_dynamic_class_receivers(&root, &class_types, &mut dynamic_class_receivers);
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
        let _ = classes::canonicalize_simple_instance_method_calls(
            &mut root,
            &simple_instance_methods,
            &dynamic_class_receivers,
        );
        classes::canonicalize_dynamic_class_array_reads(&mut root, &simple_instance_methods);
        classes::canonicalize_dynamic_parameter_virtual_calls(
            &mut root,
            &class_definitions,
            &dynamic_class_sources,
        );
        classes::canonicalize_class_handle_comparisons(&mut root, &simple_instance_methods);
        classes::canonicalize_simple_class_property_reads(&mut root, &simple_instance_methods);
        normalize_detailed_types(&mut root);
        let root_ref = &root;
        if string_field(root_ref, "kind").as_deref() != Some("Instance") {
            return Err("slang AST root is not an elaborated Instance".into());
        }
        let body = root_ref
            .get("body")
            .ok_or_else(|| "slang AST Instance has no body".to_string())?;
        if string_field(body, "kind").as_deref() != Some("InstanceBody") {
            return Err("slang AST Instance body has unexpected kind".into());
        }
        let top_name = string_field(root_ref, "name")
            .or_else(|| string_field(body, "name"))
            .ok_or_else(|| "slang AST Instance has no name".to_string())?;
        let time_unit_exponent = summary_time_unit_exponent(&document, body);
        let mut source_spans = std::collections::BTreeMap::new();
        if let Some(span) = source_span(root_ref) {
            source_spans.insert("top".to_string(), span);
        }
        let members = body
            .get("members")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut class_handles = std::collections::BTreeSet::new();
        collect_class_handle_names(root_ref, &class_types, &mut class_handles);
        collect_class_static_property_symbols(&document, &mut class_handles);
        let mut type_aliases = SlangTypeAliasIndex::default();
        collect_type_alias_targets(root_ref, &mut type_aliases);
        let mut enum_types = std::collections::BTreeMap::<String, Vec<SlangEnumTypeSummary>>::new();
        collect_enum_type_summaries(root_ref, &type_aliases, &mut enum_types)?;
        let mut ports = Vec::new();
        let mut declarations = Vec::new();
        let mut parameters = Vec::new();
        let mut continuous_targets = Vec::new();
        let mut procedural_blocks = Vec::new();
        let mut instances = Vec::new();
        let mut type_layouts = std::collections::BTreeMap::new();
        let mut package_imports = SlangPackageImports::default();
        collect_visible_package_imports(root_ref, &mut package_imports);
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
        collect_type_alias_members(root_ref, &mut type_alias_members);
        let mut simple_type_alias_widths = SimpleTypeAliasWidths::default();
        collect_simple_type_alias_widths(&type_alias_members, &mut simple_type_alias_widths);
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
        patterns::canonicalize_packed_struct_assignment_patterns(&mut root, &type_layouts);
        patterns::canonicalize_unpacked_assignment_patterns(&mut root);
        let root = &root;
        let body = root
            .get("body")
            .ok_or_else(|| "slang AST Instance has no body".to_string())?;
        let members = body
            .get("members")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
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
        let mut interface_members = std::collections::BTreeMap::new();
        collect_instance_member_paths(root, root, &mut interface_members);
        for target in &mut continuous_targets {
            annotate_simple_type_alias_widths(&mut target.rhs, &simple_type_alias_widths);
            annotate_simulation_time_units(&mut target.rhs, time_unit_exponent);
            annotate_enum_name_calls(&mut target.rhs, &enum_types);
            annotate_memory_element_shapes(&mut target.rhs, &type_layouts);
            let _ = annotate_packed_member_shapes(&mut target.rhs, &type_layouts);
            let _ =
                annotate_interface_member_paths(&mut target.rhs, &interface_members, &type_layouts);
        }
        for block in &mut procedural_blocks {
            for assignment in &mut block.assignments {
                annotate_simple_type_alias_widths(&mut assignment.rhs, &simple_type_alias_widths);
                annotate_simulation_time_units(&mut assignment.rhs, time_unit_exponent);
                annotate_enum_name_calls(&mut assignment.rhs, &enum_types);
                annotate_memory_element_shapes(&mut assignment.rhs, &type_layouts);
                let _ = annotate_packed_member_shapes(&mut assignment.rhs, &type_layouts);
                let _ = annotate_interface_member_paths(
                    &mut assignment.rhs,
                    &interface_members,
                    &type_layouts,
                );
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
