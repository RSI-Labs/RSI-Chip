//! Layout DRC, waivers, rule decks, and arbitrary-angle geometry RPCs.

use crate::{
    optional_json_array, required_object_i64, required_object_string, required_object_value,
    required_string, required_value,
    service::{
        LayoutDrcWaiverAuditAction, LayoutDrcWaiverAuditEvent, LayoutDrcWaiverRegistryState,
        SharedLayoutDrcWaiverRegistries, LAYOUT_DRC_WAIVER_REGISTRY_CAPACITY,
    },
    Json,
};
use eda_api::{
    any_angle_boolean, read_layout_rule_deck, route_any_angle_direct,
    route_any_angle_maze_cancellable, route_any_angle_maze_ripgup_cancellable,
    AnyAngleBooleanOperation, AnyAngleMazeRoutingOptions, FlowCancellation,
};
use std::collections::BTreeMap;

pub(crate) fn layout_rule_deck_rpc(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout rule deck validation cancelled".into());
    }
    let source = required_string(params, "deck")?;
    let document = read_layout_rule_deck(&source)?;
    let normalized = eda_api::write_layout_rule_deck(&document)?;
    if cancellation.is_cancelled() {
        return Err("layout rule deck validation cancelled".into());
    }
    let layer_count = normalized
        .lines()
        .filter(|line| line.starts_with("layer "))
        .count();
    let spacing_count = normalized
        .lines()
        .filter(|line| line.starts_with("spacing "))
        .count();
    let via_count = normalized
        .lines()
        .filter(|line| line.starts_with("via "))
        .count();
    Ok(Json::object([
        ("valid", Json::Bool(true)),
        ("layerCount", Json::from(layer_count as u64)),
        ("crossLayerSpacingCount", Json::from(spacing_count as u64)),
        ("viaRuleCount", Json::from(via_count as u64)),
        ("normalized", Json::string(normalized)),
    ]))
}

pub(crate) fn parse_layout_drc_waiver(value: &Json) -> Result<eda_api::LayoutDrcWaiver, String> {
    let waiver = value
        .as_object()
        .ok_or_else(|| "each layout DRC waiver must be an object".to_string())?;
    eda_api::LayoutDrcWaiver::new(
        required_object_string(waiver, "id")?,
        required_object_string(waiver, "code")?,
        optional_object_string(waiver, "layer")?,
        optional_object_string(waiver, "net")?,
        optional_object_string(waiver, "object")?,
        required_object_string(waiver, "owner")?,
        required_object_string(waiver, "reason")?,
    )
}

pub(crate) fn layout_drc_waiver_json(waiver: &eda_api::LayoutDrcWaiver) -> Json {
    Json::object([
        ("id", Json::string(waiver.id.clone())),
        ("code", Json::string(waiver.code.clone())),
        ("layer", optional_string_json(&waiver.layer)),
        ("net", optional_string_json(&waiver.net)),
        ("object", optional_string_json(&waiver.object)),
        ("owner", Json::string(waiver.owner.clone())),
        ("reason", Json::string(waiver.reason.clone())),
    ])
}

pub(crate) fn layout_drc_rpc(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout DRC cancelled".into());
    }
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let mut layout = eda_api::LayoutDocument::new();
    let optional_nonnegative_i64 =
        |fields: &BTreeMap<String, Json>, name: &'static str| -> Result<Option<i64>, String> {
            match fields.get(name) {
                None | Some(Json::Null) => Ok(None),
                Some(Json::Number(value)) => {
                    if *value < 0.0 || value.fract() != 0.0 || *value > i64::MAX as f64 {
                        return Err(format!("'{name}' must be a nonnegative i64"));
                    }
                    Ok(Some(*value as i64))
                }
                Some(_) => Err(format!("'{name}' must be a nonnegative i64")),
            }
        };
    for value in optional_json_array(fields, "layerRules")? {
        let rule = value
            .as_object()
            .ok_or_else(|| "each layer rule must be an object".to_string())?;
        let layer_rule = eda_api::LayoutLayerRule::new(
            required_object_string(rule, "layer")?,
            required_object_i64(rule, "minWidth")?,
            required_object_i64(rule, "minSpacing")?,
            required_object_i64(rule, "minArea")?,
        )?;
        let density_fields = [
            ("minDensityBp", rule.get("minDensityBp")),
            ("maxDensityBp", rule.get("maxDensityBp")),
        ];
        let density_present = density_fields
            .iter()
            .any(|(_, value)| matches!(value, Some(Json::Number(_))));
        let layer_rule = if density_present {
            let parse_density = |name: &str, value: Option<&Json>| -> Result<Option<u32>, String> {
                match value {
                    None | Some(Json::Null) => Ok(None),
                    Some(Json::Number(value)) => {
                        if *value < 0.0 || value.fract() != 0.0 || *value > u32::MAX as f64 {
                            return Err(format!("'{name}' must be a u32 basis-point value"));
                        }
                        Ok(Some(*value as u32))
                    }
                    Some(_) => Err(format!("'{name}' must be a u32 basis-point value")),
                }
            };
            layer_rule.with_density(
                parse_density("minDensityBp", rule.get("minDensityBp"))?,
                parse_density("maxDensityBp", rule.get("maxDensityBp"))?,
                required_object_i64(rule, "densityWindow")?,
            )?
        } else {
            layer_rule
        };
        let layer_rule = match rule.get("minNotch") {
            None | Some(Json::Null) => layer_rule,
            Some(Json::Number(value)) => {
                if *value < 0.0 || value.fract() != 0.0 || *value > i64::MAX as f64 {
                    return Err("'minNotch' must be a nonnegative i64".into());
                }
                layer_rule.with_notch(*value as i64)?
            }
            Some(_) => return Err("'minNotch' must be a nonnegative i64".into()),
        };
        let layer_rule = match rule.get("spacingTo") {
            None | Some(Json::Null) => layer_rule,
            Some(Json::Object(spacing_rules)) => {
                let mut layered_rule = layer_rule;
                for (other_layer, spacing) in spacing_rules {
                    let Json::Number(spacing) = spacing else {
                        return Err(format!(
                            "cross-layer spacing for '{other_layer}' must be a nonnegative i64"
                        ));
                    };
                    if *spacing < 0.0 || spacing.fract() != 0.0 || *spacing > i64::MAX as f64 {
                        return Err(format!(
                            "cross-layer spacing for '{other_layer}' must be a nonnegative i64"
                        ));
                    }
                    layered_rule =
                        layered_rule.with_spacing_to(other_layer.clone(), *spacing as i64)?;
                }
                layered_rule
            }
            Some(_) => return Err("'spacingTo' must be an object".into()),
        };
        layout.set_layer_rule(layer_rule)?;
    }
    for value in optional_json_array(fields, "viaRules")? {
        let rule = value
            .as_object()
            .ok_or_else(|| "each via rule must be an object".to_string())?;
        let via_rule = eda_api::LayoutViaRule::new(
            required_object_string(rule, "name")?,
            required_object_string(rule, "cutLayer")?,
            required_object_string(rule, "bottomLayer")?,
            required_object_string(rule, "topLayer")?,
            required_object_i64(rule, "cutWidth")?,
            required_object_i64(rule, "cutHeight")?,
            required_object_i64(rule, "minBottomEnclosure")?,
            required_object_i64(rule, "minTopEnclosure")?,
        )?;
        let via_rule = match rule.get("minCutSpacing") {
            None | Some(Json::Null) => via_rule,
            Some(Json::Number(value)) => {
                if *value < 0.0 || value.fract() != 0.0 || *value > i64::MAX as f64 {
                    return Err("'minCutSpacing' must be a nonnegative i64".into());
                }
                via_rule.with_cut_spacing(*value as i64)?
            }
            Some(_) => return Err("'minCutSpacing' must be a nonnegative i64".into()),
        };
        let extension_present = [
            "minBottomExtensionX",
            "minBottomExtensionY",
            "minTopExtensionX",
            "minTopExtensionY",
        ]
        .into_iter()
        .any(|name| rule.contains_key(name));
        let via_rule = if extension_present {
            via_rule.with_extensions(
                optional_nonnegative_i64(rule, "minBottomExtensionX")?.unwrap_or(0),
                optional_nonnegative_i64(rule, "minBottomExtensionY")?.unwrap_or(0),
                optional_nonnegative_i64(rule, "minTopExtensionX")?.unwrap_or(0),
                optional_nonnegative_i64(rule, "minTopExtensionY")?.unwrap_or(0),
            )?
        } else {
            via_rule
        };
        layout.set_via_rule(via_rule)?;
    }
    for value in optional_json_array(fields, "shapes")? {
        let shape = value
            .as_object()
            .ok_or_else(|| "each layout shape must be an object".to_string())?;
        let geometry = if let Some(rect) = shape.get("rect") {
            eda_api::LayoutGeometry::Rect(parse_rect(rect)?)
        } else if let Some(polygon) = shape.get("polygon") {
            eda_api::LayoutGeometry::Polygon(eda_api::LayoutPolygon::new(parse_points(polygon)?)?)
        } else if let Some(polygon) = shape.get("anyAnglePolygon") {
            eda_api::LayoutGeometry::AnyAngle(eda_api::AnyAnglePolygon::new(parse_points(
                polygon,
            )?)?)
        } else {
            return Err("each layout shape needs 'rect', 'polygon', or 'anyAnglePolygon'".into());
        };
        layout.add_shape(eda_api::LayoutShape {
            id: required_object_string(shape, "id")?,
            layer: required_object_string(shape, "layer")?,
            net: required_object_string(shape, "net")?,
            geometry,
        })?;
    }
    for value in optional_json_array(fields, "vias")? {
        let via = value
            .as_object()
            .ok_or_else(|| "each layout via must be an object".to_string())?;
        layout.add_via(eda_api::LayoutVia {
            id: required_object_string(via, "id")?,
            rule: required_object_string(via, "rule")?,
            net: required_object_string(via, "net")?,
            cut_layer: required_object_string(via, "cutLayer")?,
            bottom_layer: required_object_string(via, "bottomLayer")?,
            top_layer: required_object_string(via, "topLayer")?,
            cut: parse_rect(&required_object_value(via, "cut")?)?,
        })?;
    }
    let waivers = optional_json_array(fields, "waivers")?
        .into_iter()
        .map(parse_layout_drc_waiver)
        .collect::<Result<Vec<_>, String>>()?;
    if cancellation.is_cancelled() {
        return Err("layout DRC cancelled".into());
    }
    let raw_result = layout.check_layout_drc();
    let raw_violation_count = raw_result.violations.len();
    let raw_error_count = raw_result.error_count();
    let waiver_result = if waivers.is_empty() {
        None
    } else {
        Some(eda_api::apply_layout_drc_waivers(&raw_result, &waivers)?)
    };
    let result = waiver_result
        .as_ref()
        .map(|waived| waived.active.clone())
        .unwrap_or(raw_result);
    let waived = waiver_result
        .as_ref()
        .map(|result| result.waived.clone())
        .unwrap_or_default();
    let used_waiver_ids = waiver_result
        .as_ref()
        .map(|result| result.used_waiver_ids.clone())
        .unwrap_or_default();
    let waiver_report = waiver_result
        .as_ref()
        .map(eda_api::layout_drc_waiver_report)
        .unwrap_or_default();
    let violations = result
        .violations
        .iter()
        .map(|violation| {
            Ok(Json::object([
                ("code", Json::string(violation.code.clone())),
                ("severity", Json::string(drc_severity(&violation.severity))),
                ("message", Json::string(violation.message.clone())),
                ("layer", optional_string_json(&violation.layer)),
                ("net", optional_string_json(&violation.net)),
                ("object", optional_string_json(&violation.object)),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("clean", Json::Bool(result.is_clean())),
        ("errorCount", Json::from(result.error_count() as u64)),
        ("violationCount", Json::from(result.violations.len() as u64)),
        ("violations", Json::Array(violations)),
        (
            "preWaiverViolationCount",
            Json::from(raw_violation_count as u64),
        ),
        ("preWaiverErrorCount", Json::from(raw_error_count as u64)),
        ("waiverCount", Json::from(waivers.len() as u64)),
        ("waivedViolationCount", Json::from(waived.len() as u64)),
        (
            "waivedViolations",
            Json::Array(
                waived
                    .iter()
                    .map(|waived| {
                        Ok(Json::object([
                            ("waiverId", Json::string(waived.waiver_id.clone())),
                            ("owner", Json::string(waived.owner.clone())),
                            ("reason", Json::string(waived.reason.clone())),
                            (
                                "violation",
                                Json::object([
                                    ("code", Json::string(waived.violation.code.clone())),
                                    ("message", Json::string(waived.violation.message.clone())),
                                    ("layer", optional_string_json(&waived.violation.layer)),
                                    ("net", optional_string_json(&waived.violation.net)),
                                    ("object", optional_string_json(&waived.violation.object)),
                                ]),
                            ),
                        ]))
                    })
                    .collect::<Result<Vec<_>, String>>()?,
            ),
        ),
        (
            "usedWaiverIds",
            Json::Array(
                used_waiver_ids
                    .iter()
                    .map(|id| Json::string(id.clone()))
                    .collect(),
            ),
        ),
        ("waiverReport", Json::string(waiver_report)),
    ]))
}

pub(crate) fn layout_drc_waiver_audit_report(
    registry_id: &str,
    state: &LayoutDrcWaiverRegistryState,
) -> String {
    let mut out = format!(
        "Layout DRC waiver registry '{}': {} active, {} event(s)\n",
        registry_id,
        state.waivers.len(),
        state.audit.len()
    );
    for event in &state.audit {
        out.push_str(&format!(
            "  #{} {} '{}' by {}\n",
            event.sequence,
            event.action.label(),
            event.waiver_id,
            event.actor.as_deref().unwrap_or("<unknown>")
        ));
    }
    out
}

pub(crate) fn layout_drc_waiver_registry_response(
    registry_id: &str,
    state: &LayoutDrcWaiverRegistryState,
) -> Result<Json, String> {
    let audit = state
        .audit
        .iter()
        .map(|event| {
            Ok(Json::object([
                ("sequence", Json::from(event.sequence as u64)),
                ("action", Json::string(event.action.label().to_string())),
                ("waiverId", Json::string(event.waiver_id.clone())),
                (
                    "actor",
                    event.actor.clone().map_or(Json::Null, Json::string),
                ),
                (
                    "waiver",
                    event
                        .waiver
                        .as_ref()
                        .map_or(Json::Null, layout_drc_waiver_json),
                ),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let waivers = state
        .waivers
        .values()
        .map(layout_drc_waiver_json)
        .collect::<Vec<_>>();
    Ok(Json::object([
        ("registryId", Json::string(registry_id)),
        ("activeWaiverCount", Json::from(waivers.len() as u64)),
        ("waivers", Json::Array(waivers)),
        ("auditEventCount", Json::from(state.audit.len() as u64)),
        ("audit", Json::Array(audit)),
        (
            "report",
            Json::string(layout_drc_waiver_audit_report(registry_id, state)),
        ),
    ]))
}

pub(crate) fn layout_drc_waiver_registry_create(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedLayoutDrcWaiverRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout DRC waiver registry creation cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    if registry_id.trim().is_empty() {
        return Err("layout DRC waiver registry ID cannot be empty".into());
    }
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let waivers = optional_json_array(fields, "waivers")?
        .into_iter()
        .map(parse_layout_drc_waiver)
        .collect::<Result<Vec<_>, String>>()?;
    let actor = optional_object_string(fields, "actor")?;
    let mut state = LayoutDrcWaiverRegistryState::default();
    for waiver in waivers {
        if state
            .waivers
            .insert(waiver.id.clone(), waiver.clone())
            .is_some()
        {
            return Err(format!(
                "duplicate initial layout DRC waiver '{}'",
                waiver.id
            ));
        }
        state.audit.push(LayoutDrcWaiverAuditEvent {
            sequence: state.audit.len() + 1,
            action: LayoutDrcWaiverAuditAction::Added,
            waiver_id: waiver.id.clone(),
            actor: actor.clone(),
            waiver: Some(waiver),
        });
    }
    let mut entries = registries
        .lock()
        .map_err(|_| "layout DRC waiver registry mutex poisoned".to_string())?;
    if entries.contains_key(&registry_id) {
        return Err(format!(
            "layout DRC waiver registry '{registry_id}' already exists"
        ));
    }
    if entries.len() >= LAYOUT_DRC_WAIVER_REGISTRY_CAPACITY {
        return Err(format!(
            "layout DRC waiver registry store is full ({LAYOUT_DRC_WAIVER_REGISTRY_CAPACITY} entries)"
        ));
    }
    entries.insert(registry_id.clone(), state);
    let state = entries
        .get(&registry_id)
        .ok_or_else(|| "internal error: layout DRC waiver registry vanished".to_string())?;
    layout_drc_waiver_registry_response(&registry_id, state)
}

pub(crate) fn layout_drc_waiver_registry_get(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedLayoutDrcWaiverRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout DRC waiver registry lookup cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let entries = registries
        .lock()
        .map_err(|_| "layout DRC waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get(&registry_id)
        .ok_or_else(|| format!("layout DRC waiver registry '{registry_id}' does not exist"))?;
    layout_drc_waiver_registry_response(&registry_id, state)
}

pub(crate) fn layout_drc_waiver_registry_list(
    cancellation: &FlowCancellation,
    registries: &SharedLayoutDrcWaiverRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout DRC waiver registry listing cancelled".into());
    }
    let entries = registries
        .lock()
        .map_err(|_| "layout DRC waiver registry mutex poisoned".to_string())?;
    let summaries = entries
        .iter()
        .map(|(id, state)| {
            Json::object([
                ("registryId", Json::string(id.clone())),
                ("activeWaiverCount", Json::from(state.waivers.len() as u64)),
                ("auditEventCount", Json::from(state.audit.len() as u64)),
            ])
        })
        .collect::<Vec<_>>();
    Ok(Json::object([
        ("count", Json::from(entries.len() as u64)),
        (
            "capacity",
            Json::from(LAYOUT_DRC_WAIVER_REGISTRY_CAPACITY as u64),
        ),
        ("registries", Json::Array(summaries)),
    ]))
}

pub(crate) fn layout_drc_waiver_registry_add_waiver(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedLayoutDrcWaiverRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout DRC waiver registry update cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let waiver = parse_layout_drc_waiver(&required_value(params, "waiver")?)?;
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let actor = optional_object_string(fields, "actor")?;
    let mut entries = registries
        .lock()
        .map_err(|_| "layout DRC waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get_mut(&registry_id)
        .ok_or_else(|| format!("layout DRC waiver registry '{registry_id}' does not exist"))?;
    if state.waivers.contains_key(&waiver.id) {
        return Err(format!("layout DRC waiver '{}' already exists", waiver.id));
    }
    state.audit.push(LayoutDrcWaiverAuditEvent {
        sequence: state.audit.len() + 1,
        action: LayoutDrcWaiverAuditAction::Added,
        waiver_id: waiver.id.clone(),
        actor,
        waiver: Some(waiver.clone()),
    });
    let waiver_id = waiver.id.clone();
    state.waivers.insert(waiver_id, waiver);
    let state = entries
        .get(&registry_id)
        .ok_or_else(|| "internal error: layout DRC waiver registry vanished".to_string())?;
    layout_drc_waiver_registry_response(&registry_id, state)
}

pub(crate) fn layout_drc_waiver_registry_remove_waiver(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedLayoutDrcWaiverRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout DRC waiver registry update cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let waiver_id = required_string(params, "waiverId")?;
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let actor = optional_object_string(fields, "actor")?;
    let mut entries = registries
        .lock()
        .map_err(|_| "layout DRC waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get_mut(&registry_id)
        .ok_or_else(|| format!("layout DRC waiver registry '{registry_id}' does not exist"))?;
    let Some(waiver) = state.waivers.remove(&waiver_id) else {
        return Err(format!("layout DRC waiver '{waiver_id}' does not exist"));
    };
    state.audit.push(LayoutDrcWaiverAuditEvent {
        sequence: state.audit.len() + 1,
        action: LayoutDrcWaiverAuditAction::Removed,
        waiver_id,
        actor,
        waiver: Some(waiver),
    });
    let state = entries
        .get(&registry_id)
        .ok_or_else(|| "internal error: layout DRC waiver registry vanished".to_string())?;
    layout_drc_waiver_registry_response(&registry_id, state)
}

pub(crate) fn layout_drc_waiver_registry_delete(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedLayoutDrcWaiverRegistries,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("layout DRC waiver registry deletion cancelled".into());
    }
    let registry_id = required_string(params, "registryId")?;
    let mut entries = registries
        .lock()
        .map_err(|_| "layout DRC waiver registry mutex poisoned".to_string())?;
    entries
        .remove(&registry_id)
        .ok_or_else(|| format!("layout DRC waiver registry '{registry_id}' does not exist"))?;
    Ok(Json::object([
        ("registryId", Json::string(registry_id)),
        ("deleted", Json::Bool(true)),
        ("remaining", Json::from(entries.len() as u64)),
    ]))
}

pub(crate) fn layout_drc_rpc_with_registry(
    params: &Json,
    cancellation: &FlowCancellation,
    registries: &SharedLayoutDrcWaiverRegistries,
) -> Result<Json, String> {
    let Some(fields) = params.as_object() else {
        return layout_drc_rpc(params, cancellation);
    };
    let Some(registry_id) = fields.get("waiverRegistryId") else {
        return layout_drc_rpc(params, cancellation);
    };
    let Json::String(registry_id) = registry_id else {
        return Err("'waiverRegistryId' must be a string".into());
    };
    if fields.contains_key("waivers") {
        return Err("'waivers' and 'waiverRegistryId' are mutually exclusive".into());
    }
    let entries = registries
        .lock()
        .map_err(|_| "layout DRC waiver registry mutex poisoned".to_string())?;
    let state = entries
        .get(registry_id)
        .ok_or_else(|| format!("layout DRC waiver registry '{registry_id}' does not exist"))?;
    let waivers = Json::Array(state.waivers.values().map(layout_drc_waiver_json).collect());
    drop(entries);
    let mut params = params.clone();
    let Json::Object(fields) = &mut params else {
        return Err("params must be an object".into());
    };
    fields.insert("waivers".into(), waivers);
    layout_drc_rpc(&params, cancellation)
}

pub(crate) fn parse_rect(value: &Json) -> Result<eda_api::LayoutRect, String> {
    let points = parse_points(value)?;
    if points.len() != 2 {
        return Err("layout rectangle must contain two [x,y] points".into());
    }
    eda_api::LayoutRect::new(points[0].x, points[0].y, points[1].x, points[1].y)
}

pub(crate) fn parse_points(value: &Json) -> Result<Vec<eda_api::LayoutPoint>, String> {
    let Json::Array(values) = value else {
        return Err("layout geometry must be an array".into());
    };
    values
        .iter()
        .map(|point| {
            let Json::Array(coordinates) = point else {
                return Err("layout point must be [x,y]".into());
            };
            if coordinates.len() != 2 {
                return Err("layout point must contain exactly x and y".into());
            }
            Ok(eda_api::LayoutPoint::new(
                required_json_i64(&coordinates[0])?,
                required_json_i64(&coordinates[1])?,
            ))
        })
        .collect()
}

pub(crate) fn required_json_i64(value: &Json) -> Result<i64, String> {
    let number = value
        .as_number()
        .ok_or_else(|| "layout coordinate must be an integer".to_string())?;
    if number.fract() != 0.0 || !(i64::MIN as f64..=i64::MAX as f64).contains(number) {
        return Err("layout coordinate is outside i64 range".into());
    }
    Ok(*number as i64)
}

pub(crate) fn optional_object_string(
    fields: &BTreeMap<String, Json>,
    name: &str,
) -> Result<Option<String>, String> {
    match fields.get(name) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("layout field '{name}' must be a string or null")),
    }
}

pub(crate) fn parse_any_angle_polygon(value: &Json) -> Result<eda_api::AnyAnglePolygon, String> {
    let fields = value
        .as_object()
        .ok_or_else(|| "any-angle blockage must be an object".to_string())?;
    let points = required_object_value(fields, "points")?;
    let points = parse_points(&points)?;
    eda_api::AnyAnglePolygon::new(points)
}

pub(crate) fn parse_any_angle_route_request(
    value: &Json,
) -> Result<eda_api::AnyAngleRouteRequest, String> {
    let fields = value
        .as_object()
        .ok_or_else(|| "any-angle route request must be an object".to_string())?;
    let net = required_object_i64(fields, "net")?;
    if net < 0 {
        return Err("any-angle route net must be a nonnegative usize".into());
    }
    let start_value = required_object_value(fields, "start")?;
    let end_value = required_object_value(fields, "end")?;
    let Json::Array(start) = start_value else {
        return Err("any-angle route start must be [x,y]".into());
    };
    let Json::Array(end) = end_value else {
        return Err("any-angle route end must be [x,y]".into());
    };
    if start.len() != 2 || end.len() != 2 {
        return Err("any-angle route endpoints must each contain exactly x and y".into());
    }
    eda_api::AnyAngleRouteRequest::new(
        net as usize,
        eda_api::LayoutPoint::new(required_json_i64(&start[0])?, required_json_i64(&start[1])?),
        eda_api::LayoutPoint::new(required_json_i64(&end[0])?, required_json_i64(&end[1])?),
    )
}

pub(crate) fn parse_any_angle_routing_options(
    params: &Json,
) -> Result<eda_api::AnyAngleRoutingOptions, String> {
    let Some(value) = params.as_object().and_then(|fields| fields.get("options")) else {
        return Ok(eda_api::AnyAngleRoutingOptions::default());
    };
    let fields = value
        .as_object()
        .ok_or_else(|| "any-angle routing options must be an object".to_string())?;
    let min_spacing = required_object_i64(fields, "minSpacing")?;
    if min_spacing < 0 {
        return Err("any-angle routing spacing cannot be negative".into());
    }
    Ok(eda_api::AnyAngleRoutingOptions { min_spacing })
}

pub(crate) fn parse_any_angle_maze_routing_options(
    params: &Json,
) -> Result<AnyAngleMazeRoutingOptions, String> {
    let Some(value) = params.as_object().and_then(|fields| fields.get("options")) else {
        return Ok(AnyAngleMazeRoutingOptions::default());
    };
    let fields = value
        .as_object()
        .ok_or_else(|| "any-angle maze routing options must be an object".to_string())?;
    let min_spacing = required_object_i64(fields, "minSpacing")?;
    if min_spacing < 0 {
        return Err("any-angle maze routing spacing cannot be negative".into());
    }
    let max_search_nodes = required_object_i64(fields, "maxSearchNodes")?;
    if max_search_nodes < 0 || u128::from(max_search_nodes.unsigned_abs()) > usize::MAX as u128 {
        return Err("'maxSearchNodes' must be a nonnegative usize".into());
    }
    Ok(AnyAngleMazeRoutingOptions {
        min_spacing,
        max_search_nodes: max_search_nodes as usize,
    })
}

pub(crate) fn layout_any_angle_route_rpc(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("any-angle routing cancelled".into());
    }
    let requests_value = required_value(params, "requests")?;
    let Json::Array(request_values) = requests_value else {
        return Err("'requests' must be an array".into());
    };
    if request_values.is_empty() {
        return Err("any-angle routing requires at least one request".into());
    }
    let requests = request_values
        .iter()
        .map(parse_any_angle_route_request)
        .collect::<Result<Vec<_>, String>>()?;
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let blockage_values = optional_json_array(fields, "blockages")?;
    let blockages = blockage_values
        .into_iter()
        .map(parse_any_angle_polygon)
        .collect::<Result<Vec<_>, String>>()?;
    let options = parse_any_angle_routing_options(params)?;
    if cancellation.is_cancelled() {
        return Err("any-angle routing cancelled".into());
    }
    let routing = route_any_angle_direct(&requests, &blockages, &options)?;
    if cancellation.is_cancelled() {
        return Err("any-angle routing cancelled".into());
    }

    let point_json = |point: eda_api::LayoutPoint| {
        Json::object([("x", Json::from(point.x)), ("y", Json::from(point.y))])
    };
    let routes = routing
        .routes
        .iter()
        .map(|route| {
            Ok(Json::object([
                ("net", Json::from(route.net as u64)),
                ("start", point_json(route.start)),
                ("end", point_json(route.end)),
                (
                    "lengthSquaredNumerator",
                    Json::string(route.length_squared.numerator.to_string()),
                ),
                (
                    "lengthSquaredDenominator",
                    Json::string(route.length_squared.denominator.to_string()),
                ),
                ("length", Json::number(route.length())?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("routeCount", Json::from(routing.routes.len() as u64)),
        ("blockageCount", Json::from(blockages.len() as u64)),
        ("minSpacing", Json::from(options.min_spacing as u64)),
        ("routes", Json::Array(routes)),
        ("totalLength", Json::number(routing.total_length())?),
    ]))
}

pub(crate) fn layout_any_angle_maze_route_rpc(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("any-angle maze routing cancelled".into());
    }
    let requests_value = required_value(params, "requests")?;
    let Json::Array(request_values) = requests_value else {
        return Err("'requests' must be an array".into());
    };
    if request_values.is_empty() {
        return Err("any-angle maze routing requires at least one request".into());
    }
    let requests = request_values
        .iter()
        .map(parse_any_angle_route_request)
        .collect::<Result<Vec<_>, String>>()?;
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let blockage_values = optional_json_array(fields, "blockages")?;
    let blockages = blockage_values
        .into_iter()
        .map(parse_any_angle_polygon)
        .collect::<Result<Vec<_>, String>>()?;
    let options = parse_any_angle_maze_routing_options(params)?;
    let ripgup = match fields.get("ripgup") {
        None | Some(Json::Null) => false,
        Some(Json::Bool(value)) => *value,
        Some(_) => return Err("'ripgup' must be a boolean".into()),
    };
    let max_passes = if ripgup {
        let value = required_object_i64(fields, "maxPasses")?;
        if value < 0 || u128::from(value.unsigned_abs()) > usize::MAX as u128 {
            return Err("'maxPasses' must be a nonnegative usize".into());
        }
        value as usize
    } else {
        0
    };
    if cancellation.is_cancelled() {
        return Err("any-angle maze routing cancelled".into());
    }
    let (routing, passes, initial_conflicts) = if ripgup {
        let result = route_any_angle_maze_ripgup_cancellable(
            &requests,
            &blockages,
            &options,
            max_passes,
            cancellation,
        )?;
        let conflicts = result
            .initial_conflicts
            .iter()
            .map(|conflict| {
                Ok(Json::object([
                    ("leftNet", Json::from(conflict.left_net as u64)),
                    ("rightNet", Json::from(conflict.right_net as u64)),
                    ("leftSegment", Json::from(conflict.left_segment as u64)),
                    ("rightSegment", Json::from(conflict.right_segment as u64)),
                ]))
            })
            .collect::<Result<Vec<_>, String>>()?;
        (result.routing, result.passes, Some(conflicts))
    } else {
        (
            route_any_angle_maze_cancellable(&requests, &blockages, &options, cancellation)?,
            0,
            None,
        )
    };
    if cancellation.is_cancelled() {
        return Err("any-angle maze routing cancelled".into());
    }

    let point_json = |point: eda_api::LayoutPoint| {
        Json::object([("x", Json::from(point.x)), ("y", Json::from(point.y))])
    };
    let routes = routing
        .routes
        .iter()
        .map(|route| {
            let lengths = route
                .segment_lengths_squared
                .iter()
                .map(|length| {
                    Ok(Json::object([
                        ("numerator", Json::string(length.numerator.to_string())),
                        ("denominator", Json::string(length.denominator.to_string())),
                        ("value", Json::number(length.sqrt())?),
                    ]))
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(Json::object([
                ("net", Json::from(route.net as u64)),
                (
                    "points",
                    Json::Array(
                        route
                            .points
                            .iter()
                            .map(|point| point_json(*point))
                            .collect(),
                    ),
                ),
                ("segmentCount", Json::from(route.segment_count() as u64)),
                ("segmentLengthsSquared", Json::Array(lengths)),
                ("length", Json::number(route.total_length())?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut response = vec![
        ("routeCount", Json::from(routing.routes.len() as u64)),
        ("blockageCount", Json::from(blockages.len() as u64)),
        ("minSpacing", Json::from(options.min_spacing as u64)),
        (
            "maxSearchNodes",
            Json::from(options.max_search_nodes as u64),
        ),
        ("routes", Json::Array(routes)),
        ("totalLength", Json::number(routing.total_length())?),
    ];
    if ripgup {
        response.push(("ripgup", Json::Bool(true)));
        response.push(("passes", Json::from(passes as u64)));
        response.push((
            "initialConflicts",
            Json::Array(initial_conflicts.unwrap_or_default()),
        ));
    }
    Ok(Json::Object(
        response
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    ))
}

pub(crate) fn any_angle_rational_point_json(
    point: eda_api::AnyAngleRationalPoint,
) -> Result<Json, String> {
    Ok(Json::object([
        ("xNumerator", Json::string(point.x_numerator.to_string())),
        (
            "xDenominator",
            Json::string(point.x_denominator.to_string()),
        ),
        ("yNumerator", Json::string(point.y_numerator.to_string())),
        (
            "yDenominator",
            Json::string(point.y_denominator.to_string()),
        ),
        ("x", Json::number(point.x())?),
        ("y", Json::number(point.y())?),
    ]))
}

pub(crate) fn layout_any_angle_boolean_rpc(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("any-angle Boolean operation cancelled".into());
    }
    let left_value = required_value(params, "left")?;
    let right_value = required_value(params, "right")?;
    let left = parse_any_angle_polygon(&left_value)?;
    let right = parse_any_angle_polygon(&right_value)?;
    let operation = match required_string(params, "operation")?.as_str() {
        "union" => AnyAngleBooleanOperation::Union,
        "intersection" => AnyAngleBooleanOperation::Intersection,
        "difference" => AnyAngleBooleanOperation::Difference,
        other => {
            return Err(format!(
                "any-angle Boolean operation must be union, intersection, or difference, found '{other}'"
            ))
        }
    };
    if cancellation.is_cancelled() {
        return Err("any-angle Boolean operation cancelled".into());
    }
    let result = any_angle_boolean(&left, &right, operation)?;
    if cancellation.is_cancelled() {
        return Err("any-angle Boolean operation cancelled".into());
    }
    let polygons = result
        .polygons
        .iter()
        .map(|polygon| {
            Ok(Json::object([
                (
                    "points",
                    Json::Array(
                        polygon
                            .points()
                            .iter()
                            .map(|point| any_angle_rational_point_json(*point))
                            .collect::<Result<Vec<_>, String>>()?,
                    ),
                ),
                ("area", Json::number(polygon.area())?),
                ("signedArea", Json::number(polygon.signed_area())?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        (
            "operation",
            Json::string(match operation {
                AnyAngleBooleanOperation::Union => "union",
                AnyAngleBooleanOperation::Intersection => "intersection",
                AnyAngleBooleanOperation::Difference => "difference",
            }),
        ),
        ("polygonCount", Json::from(result.polygons.len() as u64)),
        ("polygons", Json::Array(polygons)),
        ("totalArea", Json::number(result.total_area)?),
    ]))
}

pub(crate) fn optional_string_json(value: &Option<String>) -> Json {
    match value {
        Some(value) => Json::string(value.clone()),
        None => Json::Null,
    }
}

pub(crate) fn drc_severity(severity: &eda_api::DrcSeverity) -> &'static str {
    match severity {
        eda_api::DrcSeverity::Warning => "warning",
        eda_api::DrcSeverity::Error => "error",
    }
}
