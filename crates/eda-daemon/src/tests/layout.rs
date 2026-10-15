use super::*;

#[test]
fn rpc_layout_drc_parses_polygons_and_vias() {
    let request = RpcRequest::new(9, "layoutDrc").with_params(Json::object([
        (
            "layerRules",
            Json::Array(vec![Json::object([
                ("layer", Json::string("M1")),
                ("minWidth", Json::Number(10.0)),
                ("minSpacing", Json::Number(20.0)),
                ("minArea", Json::Number(100.0)),
            ])]),
        ),
        (
            "shapes",
            Json::Array(vec![
                Json::object([
                    ("id", Json::string("a")),
                    ("layer", Json::string("M1")),
                    ("net", Json::string("a")),
                    (
                        "rect",
                        Json::Array(vec![
                            Json::Array(vec![Json::Number(0.0), Json::Number(0.0)]),
                            Json::Array(vec![Json::Number(50.0), Json::Number(50.0)]),
                        ]),
                    ),
                ]),
                Json::object([
                    ("id", Json::string("b")),
                    ("layer", Json::string("M1")),
                    ("net", Json::string("b")),
                    (
                        "rect",
                        Json::Array(vec![
                            Json::Array(vec![Json::Number(60.0), Json::Number(0.0)]),
                            Json::Array(vec![Json::Number(110.0), Json::Number(50.0)]),
                        ]),
                    ),
                ]),
            ]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["clean"], Json::Bool(false));
    assert_eq!(fields["errorCount"], Json::from(1_u64));
    let Json::Array(violations) = &fields["violations"] else {
        panic!("layout DRC response has no violation array");
    };
    assert_eq!(violations.len(), 1);
    let Json::Object(violation) = &violations[0] else {
        panic!("layout violation is not an object");
    };
    assert_eq!(violation["code"], Json::string("LAYOUT.SPACING"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}

#[test]
fn rpc_layout_drc_checks_layer_density() {
    let request = RpcRequest::new(100, "layoutDrc").with_params(Json::object([
        (
            "layerRules",
            Json::Array(vec![Json::object([
                ("layer", Json::string("M1")),
                ("minWidth", Json::from(10_i64)),
                ("minSpacing", Json::from(20_i64)),
                ("minArea", Json::from(100_i64)),
                ("minDensityBp", Json::from(2_000_i64)),
                ("maxDensityBp", Json::from(8_000_i64)),
                ("densityWindow", Json::from(100_i64)),
            ])]),
        ),
        (
            "shapes",
            Json::Array(vec![
                Json::object([
                    ("id", Json::string("overlap1")),
                    ("layer", Json::string("M1")),
                    ("net", Json::string("a")),
                    (
                        "rect",
                        Json::Array(vec![
                            Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                            Json::Array(vec![Json::from(50_i64), Json::from(50_i64)]),
                        ]),
                    ),
                ]),
                Json::object([
                    ("id", Json::string("overlap2")),
                    ("layer", Json::string("M1")),
                    ("net", Json::string("a")),
                    (
                        "rect",
                        Json::Array(vec![
                            Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                            Json::Array(vec![Json::from(50_i64), Json::from(50_i64)]),
                        ]),
                    ),
                ]),
            ]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["clean"], Json::Bool(true));
    assert_eq!(fields["violationCount"], Json::from(0_u64));

    let mut sparse = request.params.clone();
    let Json::Object(sparse_fields) = &mut sparse else {
        panic!("params should be an object");
    };
    sparse_fields.insert(
        "shapes".into(),
        Json::Array(vec![Json::object([
            ("id", Json::string("sparse")),
            ("layer", Json::string("M1")),
            ("net", Json::string("a")),
            (
                "rect",
                Json::Array(vec![
                    Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                    Json::Array(vec![Json::from(10_i64), Json::from(10_i64)]),
                ]),
            ),
        ])]),
    );
    let sparse_response = dispatch(&RpcRequest::new(101, "layoutDrc").with_params(sparse))
        .result
        .unwrap();
    let sparse_result = sparse_response.as_object().unwrap();
    assert_eq!(sparse_result["clean"], Json::Bool(false));
    let Json::Array(violations) = &sparse_result["violations"] else {
        panic!("density DRC response should contain violations");
    };
    let violation = violations[0].as_object().unwrap();
    assert_eq!(violation["code"], Json::string("LAYOUT.MIN_DENSITY"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}

#[test]
fn rpc_layout_drc_checks_cross_layer_spacing() {
    let request = RpcRequest::new(106, "layoutDrc").with_params(Json::object([
        (
            "layerRules",
            Json::Array(vec![
                Json::object([
                    ("layer", Json::string("M1")),
                    ("minWidth", Json::from(10_i64)),
                    ("minSpacing", Json::from(20_i64)),
                    ("minArea", Json::from(100_i64)),
                    ("spacingTo", Json::object([("M2", Json::from(30_i64))])),
                ]),
                Json::object([
                    ("layer", Json::string("M2")),
                    ("minWidth", Json::from(10_i64)),
                    ("minSpacing", Json::from(20_i64)),
                    ("minArea", Json::from(100_i64)),
                    ("spacingTo", Json::object([("M1", Json::from(40_i64))])),
                ]),
            ]),
        ),
        (
            "shapes",
            Json::Array(vec![
                Json::object([
                    ("id", Json::string("m1")),
                    ("layer", Json::string("M1")),
                    ("net", Json::string("a")),
                    (
                        "rect",
                        Json::Array(vec![
                            Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                            Json::Array(vec![Json::from(50_i64), Json::from(10_i64)]),
                        ]),
                    ),
                ]),
                Json::object([
                    ("id", Json::string("m2")),
                    ("layer", Json::string("M2")),
                    ("net", Json::string("b")),
                    (
                        "rect",
                        Json::Array(vec![
                            Json::Array(vec![Json::from(0_i64), Json::from(45_i64)]),
                            Json::Array(vec![Json::from(50_i64), Json::from(55_i64)]),
                        ]),
                    ),
                ]),
            ]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["clean"], Json::Bool(false));
    let Json::Array(violations) = &fields["violations"] else {
        panic!("cross-layer DRC response should contain violations");
    };
    assert!(violations.iter().any(|violation| {
        let violation = violation.as_object().unwrap();
        violation["code"] == Json::string("LAYOUT.SPACING")
            && violation["message"]
                .as_str()
                .unwrap()
                .contains("below layer minimum 40")
    }));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}

#[test]
fn rpc_layout_drc_rejects_different_net_overlap() {
    let request = RpcRequest::new(107, "layoutDrc").with_params(Json::object([
        (
            "layerRules",
            Json::Array(vec![Json::object([
                ("layer", Json::string("M1")),
                ("minWidth", Json::from(10_i64)),
                ("minSpacing", Json::from(0_i64)),
                ("minArea", Json::from(100_i64)),
            ])]),
        ),
        (
            "shapes",
            Json::Array(vec![
                Json::object([
                    ("id", Json::string("left")),
                    ("layer", Json::string("M1")),
                    ("net", Json::string("a")),
                    (
                        "rect",
                        Json::Array(vec![
                            Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                            Json::Array(vec![Json::from(20_i64), Json::from(20_i64)]),
                        ]),
                    ),
                ]),
                Json::object([
                    ("id", Json::string("right")),
                    ("layer", Json::string("M1")),
                    ("net", Json::string("b")),
                    (
                        "rect",
                        Json::Array(vec![
                            Json::Array(vec![Json::from(10_i64), Json::from(0_i64)]),
                            Json::Array(vec![Json::from(30_i64), Json::from(20_i64)]),
                        ]),
                    ),
                ]),
            ]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["clean"], Json::Bool(false));
    let Json::Array(violations) = &fields["violations"] else {
        panic!("short DRC response should contain violations");
    };
    assert!(violations.iter().any(|violation| {
        let violation = violation.as_object().unwrap();
        violation["code"] == Json::string("LAYOUT.SHORT")
            && violation["object"] == Json::string("left|right")
    }));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}

#[test]
fn rpc_layout_rule_deck_normalizes_typed_rules() {
    let deck = r#"
version 1
layer M1 min_width 10 min_spacing 20 min_area 100 min_density_bp 2000 max_density_bp 8000 density_window 100 min_notch 25
layer M2 min_width 10 min_spacing 20 min_area 100
spacing M1 M2 30
via VIA1 cut_layer V1 bottom_layer M1 top_layer M2 cut_width 20 cut_height 20 bottom_enclosure 1 top_enclosure 2 bottom_extension_x 10 bottom_extension_y 5 top_extension_x 6 top_extension_y 11 cut_spacing 20
# typed rule deck comments are allowed
"#;
    let request = RpcRequest::new(108, "layoutRuleDeck")
        .with_params(Json::object([("deck", Json::string(deck))]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["valid"], Json::Bool(true));
    assert_eq!(fields["layerCount"], Json::from(2_u64));
    assert_eq!(fields["crossLayerSpacingCount"], Json::from(1_u64));
    assert_eq!(fields["viaRuleCount"], Json::from(1_u64));
    let normalized = fields["normalized"].as_str().unwrap();
    assert!(normalized.contains("min_density_bp 2000"));
    assert!(normalized.contains("spacing M1 M2 30"));
    assert!(normalized.contains("cut_spacing 20"));

    let invalid = RpcRequest::new(109, "layoutRuleDeck")
        .with_params(Json::object([("deck", Json::string("version 2\n"))]));
    assert!(dispatch(&invalid).result.unwrap_err().contains("version"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}

#[test]
fn rpc_layout_drc_applies_auditable_waivers() {
    let request = RpcRequest::new(110, "layoutDrc").with_params(Json::object([
        (
            "layerRules",
            Json::Array(vec![Json::object([
                ("layer", Json::string("M1")),
                ("minWidth", Json::from(10_i64)),
                ("minSpacing", Json::from(20_i64)),
                ("minArea", Json::from(100_i64)),
            ])]),
        ),
        (
            "shapes",
            Json::Array(vec![Json::object([
                ("id", Json::string("narrow")),
                ("layer", Json::string("M1")),
                ("net", Json::string("a")),
                (
                    "rect",
                    Json::Array(vec![
                        Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                        Json::Array(vec![Json::from(5_i64), Json::from(40_i64)]),
                    ]),
                ),
            ])]),
        ),
        (
            "waivers",
            Json::Array(vec![Json::object([
                ("id", Json::string("w1")),
                ("code", Json::string("LAYOUT.MIN_WIDTH")),
                ("layer", Json::string("M1")),
                ("net", Json::string("a")),
                ("object", Json::string("narrow")),
                ("owner", Json::string("physical-owner")),
                ("reason", Json::string("approved review exception")),
            ])]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["clean"], Json::Bool(true));
    assert_eq!(fields["violationCount"], Json::from(0_u64));
    assert_eq!(fields["preWaiverViolationCount"], Json::from(1_u64));
    assert_eq!(fields["preWaiverErrorCount"], Json::from(1_u64));
    assert_eq!(fields["waiverCount"], Json::from(1_u64));
    assert_eq!(fields["waivedViolationCount"], Json::from(1_u64));
    assert_eq!(
        fields["usedWaiverIds"],
        Json::Array(vec![Json::string("w1")])
    );
    let Json::Array(waived) = &fields["waivedViolations"] else {
        panic!("waived violations should be an array");
    };
    let waived = waived[0].as_object().unwrap();
    assert_eq!(waived["waiverId"], Json::string("w1"));
    assert_eq!(waived["owner"], Json::string("physical-owner"));
    assert_eq!(waived["reason"], Json::string("approved review exception"));
    let waived_violation = waived["violation"].as_object().unwrap();
    assert_eq!(waived_violation["code"], Json::string("LAYOUT.MIN_WIDTH"));
    assert!(fields["waiverReport"]
        .as_str()
        .unwrap()
        .contains("waived LAYOUT.MIN_WIDTH [narrow] by 'w1'"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}

#[test]
fn rpc_service_persists_layout_drc_waiver_registry() {
    let service = RpcService::new();
    let empty = service
        .dispatch(&RpcRequest::new(111, "layoutDrcWaiverRegistryList"))
        .result
        .unwrap();
    assert_eq!(empty.as_object().unwrap()["count"], Json::from(0_u64));

    let create = RpcRequest::new(112, "layoutDrcWaiverRegistryCreate").with_params(Json::object([
        ("registryId", Json::string("phys")),
        ("actor", Json::string("physical-owner")),
        (
            "waivers",
            Json::Array(vec![Json::object([
                ("id", Json::string("w1")),
                ("code", Json::string("LAYOUT.MIN_WIDTH")),
                ("layer", Json::string("M1")),
                ("net", Json::string("a")),
                ("object", Json::string("narrow")),
                ("owner", Json::string("physical-owner")),
                ("reason", Json::string("approved review exception")),
            ])]),
        ),
    ]));
    let created = service.dispatch(&create).result.unwrap();
    let created_fields = created.as_object().unwrap();
    assert_eq!(created_fields["activeWaiverCount"], Json::from(1_u64));
    assert_eq!(created_fields["auditEventCount"], Json::from(1_u64));
    assert!(created_fields["report"]
        .as_str()
        .unwrap()
        .contains("#1 added 'w1' by physical-owner"));

    let drc = RpcRequest::new(113, "layoutDrc").with_params(Json::object([
        (
            "layerRules",
            Json::Array(vec![Json::object([
                ("layer", Json::string("M1")),
                ("minWidth", Json::from(10_i64)),
                ("minSpacing", Json::from(20_i64)),
                ("minArea", Json::from(100_i64)),
            ])]),
        ),
        (
            "shapes",
            Json::Array(vec![Json::object([
                ("id", Json::string("narrow")),
                ("layer", Json::string("M1")),
                ("net", Json::string("a")),
                (
                    "rect",
                    Json::Array(vec![
                        Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                        Json::Array(vec![Json::from(5_i64), Json::from(40_i64)]),
                    ]),
                ),
            ])]),
        ),
        ("waiverRegistryId", Json::string("phys")),
    ]));
    let waived = service.dispatch(&drc).result.unwrap();
    let waived_fields = waived.as_object().unwrap();
    assert_eq!(waived_fields["clean"], Json::Bool(true));
    assert_eq!(waived_fields["waivedViolationCount"], Json::from(1_u64));

    let nested_drc = parse_json(&drc.wire()).unwrap();
    let submitted = service
        .dispatch(
            &RpcRequest::new(116, "jobSubmit").with_params(Json::object([("request", nested_drc)])),
        )
        .result
        .unwrap();
    let job_id = submitted.as_object().unwrap()["jobId"].clone();
    let waited = service
        .dispatch(&RpcRequest::new(117, "jobWait").with_params(Json::object([
            ("jobId", job_id),
            ("timeoutMs", Json::Number(2_000.0)),
        ])))
        .result
        .unwrap();
    let waited_fields = waited.as_object().unwrap();
    assert_eq!(waited_fields["state"], Json::string("completed"));
    assert_eq!(
        waited_fields["result"].as_object().unwrap()["clean"],
        Json::Bool(true)
    );

    let remove =
        RpcRequest::new(114, "layoutDrcWaiverRegistryRemoveWaiver").with_params(Json::object([
            ("registryId", Json::string("phys")),
            ("waiverId", Json::string("w1")),
            ("actor", Json::string("reviewer")),
        ]));
    let removed = service.dispatch(&remove).result.unwrap();
    let removed_fields = removed.as_object().unwrap();
    assert_eq!(removed_fields["activeWaiverCount"], Json::from(0_u64));
    assert_eq!(removed_fields["auditEventCount"], Json::from(2_u64));
    assert!(removed_fields["report"]
        .as_str()
        .unwrap()
        .contains("#2 removed 'w1' by reviewer"));

    let restored = service.dispatch(&drc).result.unwrap();
    assert_eq!(restored.as_object().unwrap()["clean"], Json::Bool(false));

    let delete = RpcRequest::new(115, "layoutDrcWaiverRegistryDelete")
        .with_params(Json::object([("registryId", Json::string("phys"))]));
    let deleted = service.dispatch(&delete).result.unwrap();
    assert_eq!(deleted.as_object().unwrap()["remaining"], Json::from(0_u64));
}

#[test]
fn rpc_layout_drc_checks_via_cut_spacing() {
    let via_json = |x: i64| {
        Json::object([
            ("id", Json::string(if x == 0 { "left" } else { "near" })),
            ("rule", Json::string("VIA1")),
            ("net", Json::string("a")),
            ("cutLayer", Json::string("V1")),
            ("bottomLayer", Json::string("M1")),
            ("topLayer", Json::string("M2")),
            (
                "cut",
                Json::Array(vec![
                    Json::Array(vec![Json::from(x), Json::from(0_i64)]),
                    Json::Array(vec![Json::from(x + 20), Json::from(20_i64)]),
                ]),
            ),
        ])
    };
    let request = RpcRequest::new(102, "layoutDrc").with_params(Json::object([
        (
            "viaRules",
            Json::Array(vec![Json::object([
                ("name", Json::string("VIA1")),
                ("cutLayer", Json::string("V1")),
                ("bottomLayer", Json::string("M1")),
                ("topLayer", Json::string("M2")),
                ("cutWidth", Json::from(20_i64)),
                ("cutHeight", Json::from(20_i64)),
                ("minBottomEnclosure", Json::from(0_i64)),
                ("minTopEnclosure", Json::from(0_i64)),
                ("minCutSpacing", Json::from(20_i64)),
            ])]),
        ),
        ("vias", Json::Array(vec![via_json(0), via_json(30)])),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["clean"], Json::Bool(false));
    let Json::Array(violations) = &fields["violations"] else {
        panic!("via DRC response should contain violations");
    };
    let cut_spacing = violations
        .iter()
        .map(|violation| violation.as_object().unwrap())
        .find(|violation| violation["code"] == Json::string("LAYOUT.VIA_CUT_SPACING"))
        .expect("via cut spacing violation");
    assert_eq!(cut_spacing["object"], Json::string("left|near"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}

#[test]
fn rpc_layout_drc_rejects_overlapping_via_cuts() {
    let via_json = |id: &str, x1: i64| {
        Json::object([
            ("id", Json::string(id)),
            ("rule", Json::string("VIA1")),
            ("net", Json::string("a")),
            ("cutLayer", Json::string("V1")),
            ("bottomLayer", Json::string("M1")),
            ("topLayer", Json::string("M2")),
            (
                "cut",
                Json::Array(vec![
                    Json::Array(vec![Json::from(x1), Json::from(0_i64)]),
                    Json::Array(vec![Json::from(x1 + 20), Json::from(20_i64)]),
                ]),
            ),
        ])
    };
    let request = RpcRequest::new(105, "layoutDrc").with_params(Json::object([
        (
            "viaRules",
            Json::Array(vec![Json::object([
                ("name", Json::string("VIA1")),
                ("cutLayer", Json::string("V1")),
                ("bottomLayer", Json::string("M1")),
                ("topLayer", Json::string("M2")),
                ("cutWidth", Json::from(20_i64)),
                ("cutHeight", Json::from(20_i64)),
                ("minBottomEnclosure", Json::from(0_i64)),
                ("minTopEnclosure", Json::from(0_i64)),
            ])]),
        ),
        (
            "vias",
            Json::Array(vec![via_json("left", 0), via_json("right", 10)]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["clean"], Json::Bool(false));
    let Json::Array(violations) = &fields["violations"] else {
        panic!("via overlap DRC response should contain violations");
    };
    assert!(violations.iter().any(|violation| {
        let violation = violation.as_object().unwrap();
        violation["code"] == Json::string("LAYOUT.VIA_CUT_OVERLAP")
            && violation["object"] == Json::string("left|right")
    }));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}

#[test]
fn rpc_layout_drc_checks_minimum_notch() {
    let request = RpcRequest::new(103, "layoutDrc").with_params(Json::object([
        (
            "layerRules",
            Json::Array(vec![Json::object([
                ("layer", Json::string("M1")),
                ("minWidth", Json::from(10_i64)),
                ("minSpacing", Json::from(20_i64)),
                ("minArea", Json::from(100_i64)),
                ("minNotch", Json::from(25_i64)),
            ])]),
        ),
        (
            "shapes",
            Json::Array(vec![Json::object([
                ("id", Json::string("narrow")),
                ("layer", Json::string("M1")),
                ("net", Json::string("a")),
                (
                    "polygon",
                    Json::Array(vec![
                        Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                        Json::Array(vec![Json::from(100_i64), Json::from(0_i64)]),
                        Json::Array(vec![Json::from(100_i64), Json::from(100_i64)]),
                        Json::Array(vec![Json::from(60_i64), Json::from(100_i64)]),
                        Json::Array(vec![Json::from(60_i64), Json::from(40_i64)]),
                        Json::Array(vec![Json::from(40_i64), Json::from(40_i64)]),
                        Json::Array(vec![Json::from(40_i64), Json::from(100_i64)]),
                        Json::Array(vec![Json::from(0_i64), Json::from(100_i64)]),
                    ]),
                ),
            ])]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["clean"], Json::Bool(false));
    let Json::Array(violations) = &fields["violations"] else {
        panic!("notch DRC response should contain violations");
    };
    let violation = violations[0].as_object().unwrap();
    assert_eq!(violation["code"], Json::string("LAYOUT.MIN_NOTCH"));
    assert!(violation["message"]
        .as_str()
        .unwrap()
        .contains("notch 20 is below layer minimum 25"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}

#[test]
fn rpc_layout_drc_checks_directional_via_extensions() {
    let request_for = |bottom_x1: i64| {
        RpcRequest::new(104, "layoutDrc").with_params(Json::object([
            (
                "layerRules",
                Json::Array(vec![
                    Json::object([
                        ("layer", Json::string("M1")),
                        ("minWidth", Json::from(10_i64)),
                        ("minSpacing", Json::from(20_i64)),
                        ("minArea", Json::from(100_i64)),
                    ]),
                    Json::object([
                        ("layer", Json::string("M2")),
                        ("minWidth", Json::from(10_i64)),
                        ("minSpacing", Json::from(20_i64)),
                        ("minArea", Json::from(100_i64)),
                    ]),
                ]),
            ),
            (
                "viaRules",
                Json::Array(vec![Json::object([
                    ("name", Json::string("VIA1")),
                    ("cutLayer", Json::string("V1")),
                    ("bottomLayer", Json::string("M1")),
                    ("topLayer", Json::string("M2")),
                    ("cutWidth", Json::from(20_i64)),
                    ("cutHeight", Json::from(20_i64)),
                    ("minBottomEnclosure", Json::from(0_i64)),
                    ("minTopEnclosure", Json::from(0_i64)),
                    ("minBottomExtensionX", Json::from(10_i64)),
                    ("minBottomExtensionY", Json::from(5_i64)),
                    ("minTopExtensionX", Json::from(5_i64)),
                    ("minTopExtensionY", Json::from(10_i64)),
                ])]),
            ),
            (
                "shapes",
                Json::Array(vec![
                    Json::object([
                        ("id", Json::string("bottom")),
                        ("layer", Json::string("M1")),
                        ("net", Json::string("a")),
                        (
                            "rect",
                            Json::Array(vec![
                                Json::Array(vec![Json::from(bottom_x1), Json::from(-5_i64)]),
                                Json::Array(vec![Json::from(30_i64), Json::from(25_i64)]),
                            ]),
                        ),
                    ]),
                    Json::object([
                        ("id", Json::string("top")),
                        ("layer", Json::string("M2")),
                        ("net", Json::string("a")),
                        (
                            "rect",
                            Json::Array(vec![
                                Json::Array(vec![Json::from(-5_i64), Json::from(-10_i64)]),
                                Json::Array(vec![Json::from(25_i64), Json::from(30_i64)]),
                            ]),
                        ),
                    ]),
                ]),
            ),
            (
                "vias",
                Json::Array(vec![Json::object([
                    ("id", Json::string("via")),
                    ("rule", Json::string("VIA1")),
                    ("net", Json::string("a")),
                    ("cutLayer", Json::string("V1")),
                    ("bottomLayer", Json::string("M1")),
                    ("topLayer", Json::string("M2")),
                    (
                        "cut",
                        Json::Array(vec![
                            Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                            Json::Array(vec![Json::from(20_i64), Json::from(20_i64)]),
                        ]),
                    ),
                ])]),
            ),
        ]))
    };
    let clean = dispatch(&request_for(-10)).result.unwrap();
    assert_eq!(clean.as_object().unwrap()["clean"], Json::Bool(true));

    let short = dispatch(&request_for(-5)).result.unwrap();
    let fields = short.as_object().unwrap();
    assert_eq!(fields["clean"], Json::Bool(false));
    let Json::Array(violations) = &fields["violations"] else {
        panic!("extension DRC response should contain violations");
    };
    assert!(violations.iter().any(|violation| {
        let violation = violation.as_object().unwrap();
        violation["code"] == Json::string("LAYOUT.VIA_ENCLOSURE")
            && violation["message"]
                .as_str()
                .unwrap()
                .contains("extension (0x10, 0x5)")
    }));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request_for(-10), &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}

#[test]
fn rpc_layout_any_angle_route_returns_exact_segments() {
    let request = RpcRequest::new(93, "layoutAnyAngleRoute").with_params(Json::object([
        (
            "requests",
            Json::Array(vec![
                Json::object([
                    ("net", Json::from(7_u64)),
                    (
                        "start",
                        Json::Array(vec![Json::from(0_i64), Json::from(0_i64)]),
                    ),
                    (
                        "end",
                        Json::Array(vec![Json::from(3_i64), Json::from(5_i64)]),
                    ),
                ]),
                Json::object([
                    ("net", Json::from(8_u64)),
                    (
                        "start",
                        Json::Array(vec![Json::from(10_i64), Json::from(0_i64)]),
                    ),
                    (
                        "end",
                        Json::Array(vec![Json::from(13_i64), Json::from(5_i64)]),
                    ),
                ]),
            ]),
        ),
        ("blockages", Json::Array(Vec::new())),
        ("options", Json::object([("minSpacing", Json::from(0_i64))])),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["routeCount"], Json::from(2_u64));
    assert_eq!(fields["blockageCount"], Json::from(0_u64));
    assert_eq!(fields["minSpacing"], Json::from(0_u64));
    let Json::Array(routes) = &fields["routes"] else {
        panic!("any-angle routes should be an array");
    };
    let route = routes[0].as_object().unwrap();
    assert_eq!(route["net"], Json::from(7_u64));
    assert_eq!(route["lengthSquaredNumerator"], Json::string("34"));
    assert_eq!(route["lengthSquaredDenominator"], Json::string("1"));
    assert!((route["length"].as_number().unwrap() - f64::sqrt(34.0)).abs() < 1e-12);
    assert!((fields["totalLength"].as_number().unwrap() - 2.0 * f64::sqrt(34.0)).abs() < 1e-12);

    let mut blocked = request.params.clone();
    let blocked_fields = match &mut blocked {
        Json::Object(fields) => fields,
        _ => panic!("params should be an object"),
    };
    blocked_fields.insert(
        "blockages".into(),
        Json::Array(vec![Json::object([(
            "points",
            Json::Array(vec![
                Json::Array(vec![Json::from(-2_i64), Json::from(-2_i64)]),
                Json::Array(vec![Json::from(2_i64), Json::from(-2_i64)]),
                Json::Array(vec![Json::from(0_i64), Json::from(3_i64)]),
            ]),
        )])]),
    );
    blocked_fields.insert(
        "requests".into(),
        Json::Array(vec![Json::object([
            ("net", Json::from(0_u64)),
            (
                "start",
                Json::Array(vec![Json::from(-4_i64), Json::from(0_i64)]),
            ),
            (
                "end",
                Json::Array(vec![Json::from(4_i64), Json::from(0_i64)]),
            ),
        ])]),
    );
    let error = dispatch_result_cancellable(
        &RpcRequest::new(94, "layoutAnyAngleRoute").with_params(blocked),
        &FlowCancellation::new(),
    )
    .unwrap_err();
    assert_eq!(error, "any-angle route for net 0 crosses a layout blockage");

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}

#[test]
fn rpc_layout_any_angle_maze_route_detours_around_blockage() {
    let request = RpcRequest::new(98, "layoutAnyAngleMazeRoute").with_params(Json::object([
        (
            "requests",
            Json::Array(vec![Json::object([
                ("net", Json::from(4_u64)),
                (
                    "start",
                    Json::Array(vec![Json::from(-10_i64), Json::from(0_i64)]),
                ),
                (
                    "end",
                    Json::Array(vec![Json::from(10_i64), Json::from(0_i64)]),
                ),
            ])]),
        ),
        (
            "blockages",
            Json::Array(vec![Json::object([(
                "points",
                Json::Array(vec![
                    Json::Array(vec![Json::from(-2_i64), Json::from(-2_i64)]),
                    Json::Array(vec![Json::from(2_i64), Json::from(-2_i64)]),
                    Json::Array(vec![Json::from(0_i64), Json::from(3_i64)]),
                ]),
            )])]),
        ),
        (
            "options",
            Json::object([
                ("minSpacing", Json::from(1_i64)),
                ("maxSearchNodes", Json::from(128_u64)),
            ]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["routeCount"], Json::from(1_u64));
    assert_eq!(fields["blockageCount"], Json::from(1_u64));
    assert_eq!(fields["minSpacing"], Json::from(1_u64));
    assert_eq!(fields["maxSearchNodes"], Json::from(128_u64));
    let Json::Array(routes) = &fields["routes"] else {
        panic!("maze routes should be an array");
    };
    let route = routes[0].as_object().unwrap();
    let segment_count = route["segmentCount"]
        .as_number()
        .copied()
        .unwrap_or_default();
    assert!(segment_count > 1.0, "{route:?}");
    let Json::Array(points) = &route["points"] else {
        panic!("maze route points should be an array");
    };
    assert!(points.len() > 2);
    assert!(route["length"].as_number().unwrap() > &20.0);
    assert!(fields["totalLength"].as_number().unwrap() > &20.0);

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}

#[test]
fn rpc_layout_any_angle_maze_route_ripgup_resolves_crossing_nets() {
    let request = RpcRequest::new(99, "layoutAnyAngleMazeRoute").with_params(Json::object([
        (
            "requests",
            Json::Array(vec![
                Json::object([
                    ("net", Json::from(0_u64)),
                    (
                        "start",
                        Json::Array(vec![Json::from(-10_i64), Json::from(0_i64)]),
                    ),
                    (
                        "end",
                        Json::Array(vec![Json::from(10_i64), Json::from(0_i64)]),
                    ),
                ]),
                Json::object([
                    ("net", Json::from(1_u64)),
                    (
                        "start",
                        Json::Array(vec![Json::from(0_i64), Json::from(-10_i64)]),
                    ),
                    (
                        "end",
                        Json::Array(vec![Json::from(0_i64), Json::from(10_i64)]),
                    ),
                ]),
            ]),
        ),
        ("blockages", Json::Array(Vec::new())),
        (
            "options",
            Json::object([
                ("minSpacing", Json::from(0_i64)),
                ("maxSearchNodes", Json::from(96_u64)),
            ]),
        ),
        ("ripgup", Json::Bool(true)),
        ("maxPasses", Json::from(8_u64)),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["ripgup"], Json::Bool(true));
    assert_eq!(fields["routeCount"], Json::from(2_u64));
    assert!(fields["passes"].as_number().unwrap() <= &8.0);
    let Json::Array(initial_conflicts) = &fields["initialConflicts"] else {
        panic!("initial conflicts should be an array");
    };
    assert!(!initial_conflicts.is_empty());
    let Json::Array(routes) = &fields["routes"] else {
        panic!("rip-up routes should be an array");
    };
    assert!(routes
        .iter()
        .any(|route| route.as_object().unwrap()["segmentCount"]
            .as_number()
            .unwrap()
            > &1.0));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}

#[test]
fn rpc_layout_any_angle_boolean_returns_rational_polygons() {
    let polygon = |points: &[i64]| {
        Json::object([(
            "points",
            Json::Array(
                points
                    .chunks(2)
                    .map(|point| Json::Array(vec![Json::from(point[0]), Json::from(point[1])]))
                    .collect(),
            ),
        )])
    };
    let request = RpcRequest::new(95, "layoutAnyAngleBoolean").with_params(Json::object([
        ("left", polygon(&[0, 0, 6, 0, 6, 6, 0, 6])),
        ("right", polygon(&[3, 3, 9, 3, 9, 9, 3, 9])),
        ("operation", Json::string("union")),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["operation"], Json::string("union"));
    assert_eq!(fields["polygonCount"], Json::from(1_u64));
    assert!((fields["totalArea"].as_number().unwrap() - 63.0).abs() < 1e-12);

    let rational = RpcRequest::new(96, "layoutAnyAngleBoolean").with_params(Json::object([
        ("left", polygon(&[0, 0, 4, 0, 0, 4])),
        ("right", polygon(&[1, 1, 5, 2, 2, 5])),
        ("operation", Json::string("intersection")),
    ]));
    let rational_response = dispatch(&rational).result.unwrap();
    let rational_fields = rational_response.as_object().unwrap();
    assert!((rational_fields["totalArea"].as_number().unwrap() - 1.2).abs() < 1e-12);
    let Json::Array(result_polygons) = &rational_fields["polygons"] else {
        panic!("Boolean result polygons should be an array");
    };
    let result_polygon = result_polygons[0].as_object().unwrap();
    let Json::Array(points) = &result_polygon["points"] else {
        panic!("Boolean result points should be an array");
    };
    assert!(points.iter().any(|point| {
        let point = point.as_object().unwrap();
        point["xDenominator"] == Json::string("5") && point["yDenominator"] == Json::string("5")
    }));

    let mut invalid = rational.params.clone();
    if let Json::Object(fields) = &mut invalid {
        fields.insert("operation".into(), Json::string("xor"));
    }
    assert_eq!(
        dispatch_result_cancellable(
            &RpcRequest::new(97, "layoutAnyAngleBoolean").with_params(invalid),
            &FlowCancellation::new(),
        )
        .unwrap_err(),
        "any-angle Boolean operation must be union, intersection, or difference, found 'xor'"
    );

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        dispatch_result_cancellable(&request, &cancellation).unwrap_err(),
        "RPC job cancelled"
    );
}
