//! Planning and annotation of slang interface-array port connections.

use crate::ast_util::string_field;
use serde_json::Value;
use std::collections::BTreeMap;

const MAX_INTERFACE_ARRAY_PORT_ELEMENTS: usize = 4096;

#[derive(Debug)]
struct InterfaceArrayPortLeaf {
    address: u64,
    scope_suffix: String,
    flat_index: usize,
}

#[derive(Debug)]
struct InterfaceArrayPortTarget {
    instance_path: String,
    container_path: String,
    port_name: String,
    parent_name: String,
    modport: String,
    leaves: Vec<InterfaceArrayPortLeaf>,
    original_symbols: Vec<(String, String)>,
}

#[derive(Debug)]
struct PlannedInterfaceArrayMember {
    scope: String,
    parent_scope: String,
    target_index: usize,
    leaf_index: usize,
    member_name: String,
    original_symbol_id: String,
}

#[derive(Debug, Default)]
pub(crate) struct InterfaceArrayPortPlan {
    targets: Vec<InterfaceArrayPortTarget>,
    members: Vec<PlannedInterfaceArrayMember>,
}

fn instance_array_indexes(member: &Value) -> Result<Vec<i64>, String> {
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
    if left <= right {
        Ok((left..=right).collect())
    } else {
        Ok((right..=left).rev().collect())
    }
}

fn collect_interface_array_leaves(
    member: &Value,
    suffix: &str,
    leaves: &mut Vec<InterfaceArrayPortLeaf>,
) -> Result<(), String> {
    let indexes = instance_array_indexes(member)?;
    let elements = member
        .get("members")
        .and_then(Value::as_array)
        .ok_or_else(|| "slang InstanceArray has no members".to_string())?;
    if indexes.len() != elements.len() {
        return Err(format!(
            "interface instance array has {} indexes but {} elaborated elements",
            indexes.len(),
            elements.len()
        ));
    }
    for (index, element) in indexes.into_iter().zip(elements) {
        let element_suffix = format!("{suffix}[{index}]");
        match string_field(element, "kind").as_deref() {
            Some("Instance") => {
                let address = element
                    .get("addr")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| "interface instance has no address".to_string())?;
                leaves.push(InterfaceArrayPortLeaf {
                    address,
                    scope_suffix: element_suffix,
                    flat_index: leaves.len(),
                });
            }
            Some("InstanceArray") => {
                collect_interface_array_leaves(element, &element_suffix, leaves)?;
            }
            other => {
                return Err(format!(
                    "interface instance array contains unsupported member kind {other:?}"
                ))
            }
        }
    }
    Ok(())
}

fn first_interface_body(array: &Value) -> Result<Value, String> {
    let empty_members = Vec::new();
    let elements = array
        .get("members")
        .and_then(Value::as_array)
        .unwrap_or(&empty_members);
    let Some(first) = elements.first() else {
        return Err("interface instance array is empty".into());
    };
    match string_field(first, "kind").as_deref() {
        Some("Instance") => first
            .get("body")
            .cloned()
            .ok_or_else(|| "first interface instance has no body".to_string()),
        Some("InstanceArray") => first_interface_body(first),
        other => Err(format!(
            "interface instance array contains unsupported member kind {other:?}"
        )),
    }
}

fn interface_array_modport_symbols(
    array: &Value,
    modport_name: &str,
) -> Result<Vec<(String, String)>, String> {
    let body = first_interface_body(array)?;
    if string_field(&body, "kind").as_deref() != Some("InstanceBody") {
        return Err("first interface instance body is not an InstanceBody".into());
    }
    let empty_members = Vec::new();
    let members = body
        .get("members")
        .and_then(Value::as_array)
        .unwrap_or(&empty_members);
    let modport = members
        .iter()
        .find(|member| {
            string_field(member, "kind").as_deref() == Some("Modport")
                && string_field(member, "name").as_deref() == Some(modport_name)
        })
        .ok_or_else(|| format!("interface array port uses unknown modport '{modport_name}'"))?;
    let empty_ports = Vec::new();
    let ports = modport
        .get("members")
        .and_then(Value::as_array)
        .unwrap_or(&empty_ports);
    let mut symbols = Vec::new();
    for port in ports {
        if string_field(port, "kind").as_deref() != Some("ModportPort") {
            continue;
        }
        let name = string_field(port, "name").unwrap_or_default();
        let address = port
            .get("addr")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("modport port '{name}' has no address"))?;
        symbols.push((format!("{address} {name}"), name));
    }
    Ok(symbols)
}

fn collect_targets_in_instance(
    instance: &Value,
    instance_path: &str,
    container_path: &str,
    container_body: &Value,
    targets: &mut Vec<InterfaceArrayPortTarget>,
) -> Result<(), String> {
    let empty_connections = Vec::new();
    for connection in instance
        .get("connections")
        .and_then(Value::as_array)
        .unwrap_or(&empty_connections)
    {
        let Some(port) = connection.get("port") else {
            continue;
        };
        if string_field(port, "kind").as_deref() != Some("InterfacePort") {
            continue;
        }
        let Some(interface_instance) = connection.get("ifaceInstance").and_then(Value::as_str)
        else {
            continue;
        };
        let parent_name = interface_instance
            .split_whitespace()
            .next_back()
            .unwrap_or_default();
        let empty_members = Vec::new();
        let Some(array) = container_body
            .get("members")
            .and_then(Value::as_array)
            .unwrap_or(&empty_members)
            .iter()
            .find(|member| {
                string_field(member, "kind").as_deref() == Some("InstanceArray")
                    && string_field(member, "name").as_deref() == Some(parent_name)
            })
        else {
            continue;
        };

        let port_name = string_field(port, "name").unwrap_or_default();
        let modport = connection
            .get("modport")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("interface array port '{port_name}' has no modport"))?;
        let modport_name = modport
            .split_whitespace()
            .next_back()
            .ok_or_else(|| format!("interface array port '{port_name}' has invalid modport"))?;
        let mut leaves = Vec::new();
        collect_interface_array_leaves(array, "", &mut leaves)?;
        if leaves.len() > MAX_INTERFACE_ARRAY_PORT_ELEMENTS {
            return Err(format!(
                "interface array port '{port_name}' has more than {MAX_INTERFACE_ARRAY_PORT_ELEMENTS} elements"
            ));
        }
        let original_symbols = interface_array_modport_symbols(array, modport_name)?;
        targets.push(InterfaceArrayPortTarget {
            instance_path: instance_path.to_string(),
            container_path: container_path.to_string(),
            port_name,
            parent_name: parent_name.to_string(),
            modport: modport.to_string(),
            leaves,
            original_symbols,
        });
    }
    Ok(())
}

fn visit_instance(
    instance: &Value,
    instance_path: &str,
    container_path: &str,
    container_body: &Value,
    targets: &mut Vec<InterfaceArrayPortTarget>,
) -> Result<(), String> {
    collect_targets_in_instance(
        instance,
        instance_path,
        container_path,
        container_body,
        targets,
    )?;
    let body = instance
        .get("body")
        .ok_or_else(|| format!("instance '{instance_path}' has no body"))?;
    visit_scope(body, instance_path, targets)
}

fn visit_instance_array(
    member: &Value,
    path: &str,
    container_path: &str,
    container_body: &Value,
    targets: &mut Vec<InterfaceArrayPortTarget>,
) -> Result<(), String> {
    let name = string_field(member, "name").unwrap_or_default();
    let indexes = instance_array_indexes(member)?;
    let elements = member
        .get("members")
        .and_then(Value::as_array)
        .ok_or_else(|| "slang InstanceArray has no members".to_string())?;
    if indexes.len() != elements.len() {
        return Err("instance array range does not match elaborated elements".into());
    }
    for (index, element) in indexes.iter().zip(elements) {
        let child_path = format!("{path}.{name}[{index}]");
        match string_field(element, "kind").as_deref() {
            Some("Instance") => visit_instance(
                element,
                &child_path,
                container_path,
                container_body,
                targets,
            )?,
            Some("InstanceArray") => visit_instance_array(
                element,
                &child_path,
                container_path,
                container_body,
                targets,
            )?,
            other => {
                return Err(format!(
                    "instance array '{child_path}' contains unsupported member kind {other:?}"
                ))
            }
        }
    }
    Ok(())
}

fn visit_scope(
    body: &Value,
    path: &str,
    targets: &mut Vec<InterfaceArrayPortTarget>,
) -> Result<(), String> {
    let empty_members = Vec::new();
    let members = body
        .get("members")
        .and_then(Value::as_array)
        .unwrap_or(&empty_members);
    for member in members {
        match string_field(member, "kind").as_deref() {
            Some("Instance") => {
                let name = string_field(member, "name").unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                visit_instance(member, &format!("{path}.{name}"), path, body, targets)?;
            }
            Some("InstanceArray") => {
                visit_instance_array(member, path, path, body, targets)?;
            }
            Some("GenerateBlock") => {
                let name = string_field(member, "name").unwrap_or_default();
                let child_path = if name.is_empty() {
                    path.to_string()
                } else {
                    format!("{path}.{name}")
                };
                let empty_nested = Vec::new();
                for nested in member
                    .get("members")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty_nested)
                {
                    if string_field(nested, "kind").as_deref() == Some("InstanceBody") {
                        visit_scope(nested, &child_path, targets)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn plan_interface_array_port_scopes(
    root: &Value,
) -> Result<InterfaceArrayPortPlan, String> {
    let top_name = string_field(root, "name").unwrap_or_default();
    let body = root
        .get("body")
        .ok_or_else(|| "slang AST Instance has no body".to_string())?;
    let mut targets = Vec::new();
    visit_scope(body, &top_name, &mut targets)?;
    let mut members = Vec::new();
    for (target_index, target) in targets.iter().enumerate() {
        for (leaf_index, leaf) in target.leaves.iter().enumerate() {
            for (original_symbol_id, member_name) in &target.original_symbols {
                members.push(PlannedInterfaceArrayMember {
                    scope: format!(
                        "{}.{}{}.{}",
                        target.instance_path, target.port_name, leaf.scope_suffix, member_name
                    ),
                    parent_scope: format!(
                        "{}.{}{}.{}",
                        target.container_path, target.parent_name, leaf.scope_suffix, member_name
                    ),
                    target_index,
                    leaf_index,
                    member_name: member_name.clone(),
                    original_symbol_id: original_symbol_id.clone(),
                });
            }
        }
    }
    Ok(InterfaceArrayPortPlan { targets, members })
}

pub(crate) fn interface_array_port_scope_paths(plan: &InterfaceArrayPortPlan) -> Vec<String> {
    let mut scopes = Vec::new();
    for member in &plan.members {
        scopes.push(member.scope.clone());
        scopes.push(member.parent_scope.clone());
    }
    scopes
}

pub(crate) fn interface_array_port_plan_has_members(plan: &InterfaceArrayPortPlan) -> bool {
    !plan.members.is_empty()
}

pub(crate) fn interface_array_port_member_count(plan: &InterfaceArrayPortPlan) -> usize {
    plan.members.len()
}

pub(crate) fn annotate_interface_array_port_connections(
    root: &mut Value,
    plan: &InterfaceArrayPortPlan,
    scope_addresses: &[u64],
) -> Result<(), String> {
    if plan.members.len() * 2 != scope_addresses.len() {
        return Err(format!(
            "slang emitted {} interface-array port member scopes but {} were expected",
            scope_addresses.len(),
            plan.members.len() * 2
        ));
    }
    let specialized_addresses = plan
        .members
        .iter()
        .enumerate()
        .map(|(member_index, _)| scope_addresses[member_index * 2])
        .collect::<Vec<_>>();
    let parent_addresses = plan
        .members
        .iter()
        .enumerate()
        .map(|(member_index, _)| scope_addresses[member_index * 2 + 1])
        .collect::<Vec<_>>();
    let specialized = plan
        .members
        .iter()
        .zip(specialized_addresses)
        .map(|(member, address)| format!("{address} {}", member.member_name))
        .collect::<Vec<_>>();
    let top_name = string_field(root, "name").unwrap_or_default();
    let body = root
        .get_mut("body")
        .ok_or_else(|| "slang AST Instance has no body".to_string())?;
    annotate_scope(body, &top_name, plan, &specialized)?;
    let body = root
        .get_mut("body")
        .ok_or_else(|| "slang AST Instance has no body".to_string())?;
    annotate_parent_arrays(body, &top_name, plan, &parent_addresses)
}

fn parent_symbol_annotations(
    plan: &InterfaceArrayPortPlan,
    parent_addresses: &[u64],
) -> BTreeMap<(usize, usize), Value> {
    let mut annotations = BTreeMap::new();
    for (member_index, member) in plan.members.iter().enumerate() {
        let key = (member.target_index, member.leaf_index);
        let symbol_id = format!("{} {}", parent_addresses[member_index], member.member_name);
        let entry = annotations
            .entry(key)
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Some(fields) = entry.as_object_mut() {
            fields.insert(symbol_id, Value::String(member.member_name.clone()));
        }
    }
    annotations
}

fn annotate_parent_arrays(
    body: &mut Value,
    path: &str,
    plan: &InterfaceArrayPortPlan,
    parent_addresses: &[u64],
) -> Result<(), String> {
    let annotations = parent_symbol_annotations(plan, parent_addresses);
    let Some(members) = body.get_mut("members").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    for member in members {
        match string_field(member, "kind").as_deref() {
            Some("InstanceArray") => {
                let name = string_field(member, "name").unwrap_or_default();
                let target_indices = plan
                    .targets
                    .iter()
                    .enumerate()
                    .filter(|(_, target)| {
                        target.container_path == path && target.parent_name == name
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                if !target_indices.is_empty() {
                    let mut leaf_index = 0;
                    annotate_parent_array_leaves(
                        member,
                        &mut leaf_index,
                        &target_indices,
                        &annotations,
                    )?;
                }
            }
            Some("Instance") => {
                let name = string_field(member, "name").unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                if let Some(child_body) = member.get_mut("body") {
                    annotate_parent_arrays(
                        child_body,
                        &format!("{path}.{name}"),
                        plan,
                        parent_addresses,
                    )?;
                }
            }
            Some("GenerateBlock") => {
                let name = string_field(member, "name").unwrap_or_default();
                let child_path = if name.is_empty() {
                    path.to_string()
                } else {
                    format!("{path}.{name}")
                };
                if let Some(nested) = member.get_mut("members").and_then(Value::as_array_mut) {
                    for scope in nested {
                        if string_field(scope, "kind").as_deref() == Some("InstanceBody") {
                            annotate_parent_arrays(scope, &child_path, plan, parent_addresses)?;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn annotate_parent_array_leaves(
    member: &mut Value,
    leaf_index: &mut usize,
    target_indices: &[usize],
    annotations: &BTreeMap<(usize, usize), Value>,
) -> Result<(), String> {
    let indexes = instance_array_indexes(member)?;
    let elements = member
        .get_mut("members")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "slang InstanceArray has no members".to_string())?;
    if indexes.len() != elements.len() {
        return Err("instance array range does not match elaborated elements".into());
    }
    for element in elements {
        match string_field(element, "kind").as_deref() {
            Some("Instance") => {
                let current_leaf = *leaf_index;
                if let Some(fields) = element.as_object_mut() {
                    let mut merged = fields
                        .get("eda_hierarchical_symbols")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default();
                    for target_index in target_indices {
                        if let Some(annotation) = annotations.get(&(*target_index, current_leaf)) {
                            if let Some(annotation_fields) = annotation.as_object() {
                                merged.extend(annotation_fields.clone());
                            }
                        }
                    }
                    fields.insert("eda_hierarchical_symbols".into(), Value::Object(merged));
                }
                *leaf_index += 1;
            }
            Some("InstanceArray") => {
                annotate_parent_array_leaves(element, leaf_index, target_indices, annotations)?
            }
            other => {
                return Err(format!(
                    "interface instance array contains unsupported member kind {other:?}"
                ))
            }
        }
    }
    Ok(())
}

fn target_annotation(
    target: &InterfaceArrayPortTarget,
    target_index: usize,
    plan: &InterfaceArrayPortPlan,
    specialized: &[String],
) -> Value {
    let mut elements = Vec::new();
    for (leaf_index, leaf) in target.leaves.iter().enumerate() {
        let mut external_symbols = serde_json::Map::new();
        for (member_index, member) in plan.members.iter().enumerate() {
            if member.target_index == target_index && member.leaf_index == leaf_index {
                external_symbols.insert(
                    member.original_symbol_id.clone(),
                    Value::String(specialized[member_index].clone()),
                );
            }
        }
        elements.push(serde_json::json!({
            "ifaceInstance": format!("{} {}", leaf.address, target.parent_name),
            "modport": target.modport,
            "eda_proxy_suffix": format!("{}_{}", target.port_name, leaf.flat_index),
            "eda_external_symbols": external_symbols,
        }));
    }
    Value::Array(elements)
}

fn annotate_instance(
    instance: &mut Value,
    instance_path: &str,
    plan: &InterfaceArrayPortPlan,
    specialized: &[String],
) -> Result<(), String> {
    for (target_index, target) in plan.targets.iter().enumerate() {
        if target.instance_path != instance_path {
            continue;
        }
        let Some(connections) = instance
            .get_mut("connections")
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        for connection in connections {
            let parent_matches = connection
                .get("ifaceInstance")
                .and_then(Value::as_str)
                .and_then(|reference| reference.split_whitespace().next_back())
                .is_some_and(|name| name == target.parent_name);
            let port_matches = connection
                .get("port")
                .map(|port| string_field(port, "name"))
                .is_some_and(|name| name.as_deref() == Some(target.port_name.as_str()));
            if parent_matches && port_matches {
                if let Some(fields) = connection.as_object_mut() {
                    fields.insert(
                        "eda_interface_array_connections".into(),
                        target_annotation(target, target_index, plan, specialized),
                    );
                }
            }
        }
    }
    let body = instance
        .get_mut("body")
        .ok_or_else(|| format!("instance '{instance_path}' has no body"))?;
    annotate_scope(body, instance_path, plan, specialized)
}

fn annotate_instance_array(
    member: &mut Value,
    path: &str,
    plan: &InterfaceArrayPortPlan,
    specialized: &[String],
) -> Result<(), String> {
    let name = string_field(member, "name").unwrap_or_default();
    let indexes = instance_array_indexes(member)?;
    let elements = member
        .get_mut("members")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "slang InstanceArray has no members".to_string())?;
    if indexes.len() != elements.len() {
        return Err("instance array range does not match elaborated elements".into());
    }
    for (index, element) in indexes.iter().zip(elements) {
        let child_path = format!("{path}.{name}[{index}]");
        match string_field(element, "kind").as_deref() {
            Some("Instance") => annotate_instance(element, &child_path, plan, specialized)?,
            Some("InstanceArray") => {
                annotate_instance_array(element, &child_path, plan, specialized)?
            }
            other => {
                return Err(format!(
                    "instance array '{child_path}' contains unsupported member kind {other:?}"
                ))
            }
        }
    }
    Ok(())
}

fn annotate_scope(
    body: &mut Value,
    path: &str,
    plan: &InterfaceArrayPortPlan,
    specialized: &[String],
) -> Result<(), String> {
    let Some(members) = body.get_mut("members").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    for member in members {
        match string_field(member, "kind").as_deref() {
            Some("Instance") => {
                let name = string_field(member, "name").unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                annotate_instance(member, &format!("{path}.{name}"), plan, specialized)?;
            }
            Some("InstanceArray") => {
                annotate_instance_array(member, path, plan, specialized)?;
            }
            Some("GenerateBlock") => {
                let name = string_field(member, "name").unwrap_or_default();
                let child_path = if name.is_empty() {
                    path.to_string()
                } else {
                    format!("{path}.{name}")
                };
                if let Some(nested) = member.get_mut("members").and_then(Value::as_array_mut) {
                    for scope in nested {
                        if string_field(scope, "kind").as_deref() == Some("InstanceBody") {
                            annotate_scope(scope, &child_path, plan, specialized)?;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}
