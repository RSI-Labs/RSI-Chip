use super::*;

fn rule() -> LayoutLayerRule {
    LayoutLayerRule::new("M1", 10, 20, 100).unwrap()
}

#[test]
fn layout_drc_checks_exact_layer_density_rules() {
    let mut document = LayoutDocument::new();
    document
        .set_layer_rule(
            LayoutLayerRule::new("M1", 10, 20, 100)
                .unwrap()
                .with_density(Some(2_000), Some(8_000), 100)
                .unwrap(),
        )
        .unwrap();
    document
        .add_shape(LayoutShape {
            id: "a".into(),
            layer: "M1".into(),
            net: "a".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(0, 0, 50, 50).unwrap()),
        })
        .unwrap();
    assert!(document.check_layout_drc().is_clean());

    document.remove_shape("a").unwrap();
    document
        .add_shape(LayoutShape {
            id: "sparse".into(),
            layer: "M1".into(),
            net: "a".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(0, 0, 10, 10).unwrap()),
        })
        .unwrap();
    assert!(document
        .check_layout_drc()
        .violations
        .iter()
        .any(|violation| violation.code == "LAYOUT.MIN_DENSITY"));

    document.remove_shape("sparse").unwrap();
    document
        .add_shape(LayoutShape {
            id: "dense".into(),
            layer: "M1".into(),
            net: "a".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(0, 0, 90, 90).unwrap()),
        })
        .unwrap();
    assert!(document
        .check_layout_drc()
        .violations
        .iter()
        .any(|violation| violation.code == "LAYOUT.MAX_DENSITY"));

    document.remove_shape("dense").unwrap();
    for id in ["overlap1", "overlap2"] {
        document
            .add_shape(LayoutShape {
                id: id.into(),
                layer: "M1".into(),
                net: "a".into(),
                geometry: LayoutGeometry::Rect(LayoutRect::new(0, 0, 50, 50).unwrap()),
            })
            .unwrap();
    }
    assert!(document.check_layout_drc().is_clean());

    assert!(LayoutLayerRule::new("M1", 10, 20, 100)
        .unwrap()
        .with_density(Some(10_001), None, 100)
        .is_err());
    assert!(LayoutLayerRule::new("M1", 10, 20, 100)
        .unwrap()
        .with_density(Some(5_000), Some(4_000), 100)
        .is_err());
    assert!(LayoutLayerRule::new("M1", 10, 20, 100)
        .unwrap()
        .with_density(Some(1), None, 0)
        .is_err());
}

#[test]
fn layout_drc_checks_via_cut_spacing() {
    let mut document = LayoutDocument::new();
    document
        .set_via_rule(
            LayoutViaRule::new("VIA1", "V1", "M1", "M2", 20, 20, 0, 0)
                .unwrap()
                .with_cut_spacing(20)
                .unwrap(),
        )
        .unwrap();
    for (id, x1) in [("left", 0), ("near", 30), ("far", 70)] {
        document
            .add_via(LayoutVia {
                id: id.into(),
                rule: "VIA1".into(),
                net: "a".into(),
                cut_layer: "V1".into(),
                bottom_layer: "M1".into(),
                top_layer: "M2".into(),
                cut: LayoutRect::new(x1, 0, x1 + 20, 20).unwrap(),
            })
            .unwrap();
    }
    let result = document.check_layout_drc();
    let violations = result
        .violations
        .iter()
        .filter(|violation| violation.code == "LAYOUT.VIA_CUT_SPACING")
        .collect::<Vec<_>>();
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].object.as_deref(), Some("left|near"));
    assert!(violations[0].message.contains("below minimum 20"));

    assert!(LayoutViaRule::new("VIA1", "V1", "M1", "M2", 20, 20, 0, 0)
        .unwrap()
        .with_cut_spacing(-1)
        .is_err());
}

#[test]
fn layout_drc_checks_directional_via_extensions() {
    let mut document = LayoutDocument::new();
    document
        .set_layer_rule(LayoutLayerRule::new("M1", 10, 20, 100).unwrap())
        .unwrap();
    document
        .set_layer_rule(LayoutLayerRule::new("M2", 10, 20, 100).unwrap())
        .unwrap();
    document
        .set_via_rule(
            LayoutViaRule::new("VIA1", "V1", "M1", "M2", 20, 20, 0, 0)
                .unwrap()
                .with_extensions(10, 5, 5, 10)
                .unwrap(),
        )
        .unwrap();
    let add_rect = |document: &mut LayoutDocument, id: &str, layer: &str, rect: LayoutRect| {
        document
            .add_shape(LayoutShape {
                id: id.into(),
                layer: layer.into(),
                net: "a".into(),
                geometry: LayoutGeometry::Rect(rect),
            })
            .unwrap();
    };
    add_rect(
        &mut document,
        "bottom",
        "M1",
        LayoutRect::new(-10, -5, 30, 25).unwrap(),
    );
    add_rect(
        &mut document,
        "top",
        "M2",
        LayoutRect::new(-5, -10, 25, 30).unwrap(),
    );
    document
        .add_via(LayoutVia {
            id: "via".into(),
            rule: "VIA1".into(),
            net: "a".into(),
            cut_layer: "V1".into(),
            bottom_layer: "M1".into(),
            top_layer: "M2".into(),
            cut: LayoutRect::new(0, 0, 20, 20).unwrap(),
        })
        .unwrap();
    assert!(document.check_layout_drc().is_clean());

    document.remove_shape("bottom").unwrap();
    add_rect(
        &mut document,
        "bottom_short_x",
        "M1",
        LayoutRect::new(-5, -5, 30, 25).unwrap(),
    );
    let result = document.check_layout_drc();
    assert!(result
        .violations
        .iter()
        .any(|violation| violation.code == "LAYOUT.VIA_ENCLOSURE"
            && violation.message.contains("extension (0x10, 0x5)")));

    assert!(LayoutViaRule::new("VIA1", "V1", "M1", "M2", 20, 20, 0, 0)
        .unwrap()
        .with_extensions(-1, 0, 0, 0)
        .is_err());
}

#[test]
fn layout_drc_rejects_overlapping_via_cuts() {
    let mut document = LayoutDocument::new();
    document
        .set_via_rule(LayoutViaRule::new("VIA1", "V1", "M1", "M2", 20, 20, 0, 0).unwrap())
        .unwrap();
    for (id, x1) in [("left", 0), ("overlap", 10), ("touching", 30)] {
        document
            .add_via(LayoutVia {
                id: id.into(),
                rule: "VIA1".into(),
                net: "a".into(),
                cut_layer: "V1".into(),
                bottom_layer: "M1".into(),
                top_layer: "M2".into(),
                cut: LayoutRect::new(x1, 0, x1 + 20, 20).unwrap(),
            })
            .unwrap();
    }
    let result = document.check_layout_drc();
    let overlaps = result
        .violations
        .iter()
        .filter(|violation| violation.code == "LAYOUT.VIA_CUT_OVERLAP")
        .collect::<Vec<_>>();
    assert_eq!(overlaps.len(), 1);
    assert_eq!(overlaps[0].object.as_deref(), Some("left|overlap"));
    assert!(overlaps[0].message.contains("overlap on layer 'V1'"));
}

#[test]
fn layout_density_drc_checks_local_windows() {
    let mut document = LayoutDocument::new();
    document
        .set_layer_rule(
            LayoutLayerRule::new("M1", 10, 20, 100)
                .unwrap()
                .with_density(None, Some(8_000), 100)
                .unwrap(),
        )
        .unwrap();
    document
        .add_shape(LayoutShape {
            id: "local".into(),
            layer: "M1".into(),
            net: "a".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(0, 0, 90, 100).unwrap()),
        })
        .unwrap();
    document
        .add_shape(LayoutShape {
            id: "far".into(),
            layer: "M1".into(),
            net: "a".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(200, 0, 210, 100).unwrap()),
        })
        .unwrap();
    let result = document.check_layout_drc();
    assert!(result
        .violations
        .iter()
        .any(|violation| violation.code == "LAYOUT.MAX_DENSITY"
            && violation.message.contains("at (0,0)")
            && violation.message.contains("90.000%")));
}

#[test]
fn layout_drc_checks_cross_layer_spacing() {
    let mut document = LayoutDocument::new();
    document
        .set_layer_rule(
            LayoutLayerRule::new("M1", 10, 20, 100)
                .unwrap()
                .with_spacing_to("M2", 30)
                .unwrap(),
        )
        .unwrap();
    document
        .set_layer_rule(
            LayoutLayerRule::new("M2", 10, 20, 100)
                .unwrap()
                .with_spacing_to("M1", 40)
                .unwrap(),
        )
        .unwrap();
    document
        .add_shape(LayoutShape {
            id: "m1".into(),
            layer: "M1".into(),
            net: "a".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(0, 0, 50, 10).unwrap()),
        })
        .unwrap();
    document
        .add_shape(LayoutShape {
            id: "m2".into(),
            layer: "M2".into(),
            net: "b".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(0, 45, 50, 55).unwrap()),
        })
        .unwrap();
    let result = document.check_layout_drc();
    assert!(result
        .violations
        .iter()
        .any(|violation| violation.code == "LAYOUT.SPACING"
            && violation.message.contains("below layer minimum 40")));

    document.remove_shape("m2").unwrap();
    document
        .add_shape(LayoutShape {
            id: "m2_same_net".into(),
            layer: "M2".into(),
            net: "a".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(0, 45, 50, 55).unwrap()),
        })
        .unwrap();
    assert!(document.check_layout_drc().is_clean());

    assert!(LayoutLayerRule::new("M1", 10, 20, 100)
        .unwrap()
        .with_spacing_to("M2", -1)
        .is_err());
}

#[test]
fn layout_drc_rejects_different_net_overlap() {
    let mut document = LayoutDocument::new();
    document
        .set_layer_rule(LayoutLayerRule::new("M1", 10, 0, 100).unwrap())
        .unwrap();
    document
        .add_shape(LayoutShape {
            id: "left".into(),
            layer: "M1".into(),
            net: "a".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(0, 0, 20, 20).unwrap()),
        })
        .unwrap();
    document
        .add_shape(LayoutShape {
            id: "right".into(),
            layer: "M1".into(),
            net: "b".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(10, 0, 30, 20).unwrap()),
        })
        .unwrap();
    let result = document.check_layout_drc();
    assert!(result
        .violations
        .iter()
        .any(|violation| violation.code == "LAYOUT.SHORT"
            && violation.object.as_deref() == Some("left|right")));

    document.remove_shape("right").unwrap();
    document
        .add_shape(LayoutShape {
            id: "touching".into(),
            layer: "M1".into(),
            net: "b".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(20, 0, 40, 20).unwrap()),
        })
        .unwrap();
    assert!(document.check_layout_drc().is_clean());
}

#[test]
fn layout_drc_checks_minimum_notch() {
    let notch_polygon = |opening: i64| {
        LayoutPolygon::new([
            LayoutPoint::new(0, 0),
            LayoutPoint::new(100, 0),
            LayoutPoint::new(100, 100),
            LayoutPoint::new(50 + opening / 2, 100),
            LayoutPoint::new(50 + opening / 2, 40),
            LayoutPoint::new(50 - opening / 2, 40),
            LayoutPoint::new(50 - opening / 2, 100),
            LayoutPoint::new(0, 100),
        ])
        .unwrap()
    };
    assert_eq!(notch_polygon(20).minimum_notch_width(), Some(20));
    assert_eq!(notch_polygon(30).minimum_notch_width(), Some(30));

    let mut document = LayoutDocument::new();
    document
        .set_layer_rule(
            LayoutLayerRule::new("M1", 10, 20, 100)
                .unwrap()
                .with_notch(25)
                .unwrap(),
        )
        .unwrap();
    document
        .add_shape(LayoutShape {
            id: "narrow".into(),
            layer: "M1".into(),
            net: "a".into(),
            geometry: LayoutGeometry::Polygon(notch_polygon(20)),
        })
        .unwrap();
    let result = document.check_layout_drc();
    assert!(result
        .violations
        .iter()
        .any(|violation| violation.code == "LAYOUT.MIN_NOTCH"
            && violation
                .message
                .contains("notch 20 is below layer minimum 25")));

    document.remove_shape("narrow").unwrap();
    document
        .add_shape(LayoutShape {
            id: "wide".into(),
            layer: "M1".into(),
            net: "a".into(),
            geometry: LayoutGeometry::Polygon(notch_polygon(30)),
        })
        .unwrap();
    assert!(document.check_layout_drc().is_clean());

    assert!(LayoutLayerRule::new("M1", 10, 20, 100)
        .unwrap()
        .with_notch(-1)
        .is_err());
}

#[test]
fn layout_rule_deck_round_trips_all_typed_rules() {
    let mut document = LayoutDocument::new();
    document.set_antenna_ratio(20.0).unwrap();
    document
        .set_layer_rule(
            LayoutLayerRule::new("M1", 10, 20, 100)
                .unwrap()
                .with_density(Some(2_000), Some(8_000), 100)
                .unwrap()
                .with_notch(25)
                .unwrap()
                .with_spacing_to("M2", 30)
                .unwrap(),
        )
        .unwrap();
    document
        .set_layer_rule(LayoutLayerRule::new("M2", 10, 20, 100).unwrap())
        .unwrap();
    document
        .set_via_rule(
            LayoutViaRule::new("VIA1", "V1", "M1", "M2", 20, 20, 1, 2)
                .unwrap()
                .with_extensions(10, 5, 6, 11)
                .unwrap()
                .with_cut_spacing(20)
                .unwrap(),
        )
        .unwrap();
    let written = write_layout_rule_deck(&document).unwrap();
    let restored = read_layout_rule_deck(&written).unwrap();
    assert_eq!(restored.layer_rule("M1"), document.layer_rule("M1"));
    assert_eq!(restored.layer_rule("M2"), document.layer_rule("M2"));
    assert_eq!(restored.via_rule("VIA1"), document.via_rule("VIA1"));
    assert_eq!(restored.antenna_ratio(), Some(20.0));
    assert_eq!(write_layout_rule_deck(&restored).unwrap(), written);

    assert!(read_layout_rule_deck("").is_err());
    assert!(read_layout_rule_deck("version 2\n").is_err());
    assert!(read_layout_rule_deck("version 1\nlayer M1\n").is_err());
    assert!(read_layout_rule_deck("version 1\nspacing M1 M2 10\n").is_err());
    assert!(read_layout_rule_deck("version 1\nvia V1 V1 M1 M2\n").is_err());
    assert!(read_layout_rule_deck("version 1\nantenna_ratio 0\n").is_err());
    assert!(read_layout_rule_deck("version 1\nantenna_ratio 1\nantenna_ratio 2\n").is_err());
}

#[test]
fn layout_drc_waivers_use_exact_auditable_selectors() {
    let violation = |code: &str, object: &str| LayoutDrcViolation {
        code: code.into(),
        severity: DrcSeverity::Error,
        message: format!("{code} {object}"),
        layer: Some("M1".into()),
        net: Some("a".into()),
        object: Some(object.into()),
    };
    let result = LayoutDrcResult {
        violations: vec![
            violation("LAYOUT.MIN_WIDTH", "narrow"),
            violation("LAYOUT.MIN_WIDTH", "other"),
            violation("LAYOUT.SPACING", "narrow|other"),
        ],
    };
    let waiver = LayoutDrcWaiver::new(
        "w1",
        "LAYOUT.MIN_WIDTH",
        Some("M1".into()),
        Some("a".into()),
        Some("narrow".into()),
        "physical-owner",
        "approved duplicate metal fill",
    )
    .unwrap();
    let waived = apply_layout_drc_waivers(&result, &[waiver]).unwrap();
    assert_eq!(waived.active.violations.len(), 2);
    assert_eq!(waived.waived.len(), 1);
    assert_eq!(waived.waived[0].waiver_id, "w1");
    assert_eq!(waived.waived[0].owner, "physical-owner");
    assert_eq!(waived.used_waiver_ids, vec!["w1".to_string()]);
    let report = layout_drc_waiver_report(&waived);
    assert!(report.contains("2 active, 1 waived, 1 waiver(s) used"));
    assert!(report.contains("waived LAYOUT.MIN_WIDTH [narrow] by 'w1'"));
    assert!(report.contains("owner='physical-owner'"));

    let wrong_object = LayoutDrcWaiver::new(
        "w2",
        "LAYOUT.MIN_WIDTH",
        Some("M1".into()),
        Some("a".into()),
        Some("missing".into()),
        "physical-owner",
        "does not match",
    )
    .unwrap();
    let unused = apply_layout_drc_waivers(&result, &[wrong_object]).unwrap();
    assert_eq!(unused.active.violations.len(), 3);
    assert!(unused.waived.is_empty());

    assert!(
        LayoutDrcWaiver::new("", "LAYOUT.MIN_WIDTH", None, None, None, "owner", "reason").is_err()
    );
    let first = LayoutDrcWaiver::new(
        "same",
        "LAYOUT.MIN_WIDTH",
        None,
        None,
        None,
        "owner",
        "reason",
    )
    .unwrap();
    assert!(apply_layout_drc_waivers(&result, &[first.clone(), first]).is_err());
}

#[test]
fn rectilinear_polygon_geometry_is_exact() {
    let polygon = LayoutPolygon::new([
        LayoutPoint::new(0, 0),
        LayoutPoint::new(30, 0),
        LayoutPoint::new(30, 10),
        LayoutPoint::new(10, 10),
        LayoutPoint::new(10, 30),
        LayoutPoint::new(0, 30),
    ])
    .unwrap();
    assert_eq!(polygon.area(), 500);
    assert_eq!(polygon.bbox(), LayoutRect::new(0, 0, 30, 30).unwrap());
    assert!(polygon.contains_point(LayoutPoint::new(5, 5)));
    assert!(!polygon.contains_point(LayoutPoint::new(20, 20)));
    assert!(LayoutPolygon::new([
        LayoutPoint::new(0, 0),
        LayoutPoint::new(20, 0),
        LayoutPoint::new(20, 20),
        LayoutPoint::new(0, 20),
        LayoutPoint::new(0, 0),
    ])
    .is_err());
}

#[test]
fn layout_drc_reports_width_area_spacing_and_via_enclosure() {
    let mut document = LayoutDocument::new();
    document.set_layer_rule(rule()).unwrap();
    document
        .set_via_rule(LayoutViaRule::new("VIA1", "V1", "M1", "M2", 20, 20, 5, 5).unwrap())
        .unwrap();
    document
        .add_shape(LayoutShape {
            id: "narrow".into(),
            layer: "M1".into(),
            net: "a".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(0, 0, 5, 40).unwrap()),
        })
        .unwrap();
    document
        .add_shape(LayoutShape {
            id: "near".into(),
            layer: "M1".into(),
            net: "b".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(20, 0, 40, 40).unwrap()),
        })
        .unwrap();
    document
        .add_via(LayoutVia {
            id: "v1".into(),
            rule: "VIA1".into(),
            net: "a".into(),
            cut_layer: "V1".into(),
            bottom_layer: "M1".into(),
            top_layer: "M2".into(),
            cut: LayoutRect::new(100, 100, 120, 120).unwrap(),
        })
        .unwrap();
    let result = document.check_layout_drc();
    let codes: Vec<_> = result.violations.iter().map(|v| v.code.as_str()).collect();
    assert!(codes.contains(&"LAYOUT.MIN_WIDTH"));
    assert!(codes.contains(&"LAYOUT.SPACING"));
    assert!(!codes.contains(&"LAYOUT.UNKNOWN_LAYER"));
    assert!(codes.contains(&"LAYOUT.VIA_ENCLOSURE"));
}

#[test]
fn layout_document_accepts_and_checks_arbitrary_angle_polygons() {
    let mut document = LayoutDocument::new();
    document
        .set_layer_rule(LayoutLayerRule::new("M1", 5, 10, 20).unwrap())
        .unwrap();
    let left = AnyAnglePolygon::new([
        LayoutPoint::new(0, 0),
        LayoutPoint::new(10, 0),
        LayoutPoint::new(0, 10),
    ])
    .unwrap();
    let right = left.translated(20, 0).unwrap();
    document
        .add_shape(LayoutShape {
            id: "left".into(),
            layer: "M1".into(),
            net: "a".into(),
            geometry: LayoutGeometry::AnyAngle(left.clone()),
        })
        .unwrap();
    document
        .add_shape(LayoutShape {
            id: "right".into(),
            layer: "M1".into(),
            net: "b".into(),
            geometry: LayoutGeometry::AnyAngle(right),
        })
        .unwrap();

    assert!(document.check_layout_drc().is_clean());
    assert!(document
        .hit_test(LayoutPoint::new(2, 2))
        .is_some_and(|shape| shape.id == "left"));
    assert!(document.write_svg().unwrap().contains("<polygon points=\""));
    assert_eq!(LayoutGeometry::AnyAngle(left).area2().unwrap(), 100,);

    document.move_shape("right", -5, 0).unwrap();
    let result = document.check_layout_drc();
    assert!(result
        .violations
        .iter()
        .any(|violation| violation.code == "LAYOUT.SPACING"));

    let geometry = LayoutGeometry::AnyAngle(
        AnyAnglePolygon::new([
            LayoutPoint::new(0, 0),
            LayoutPoint::new(1, 0),
            LayoutPoint::new(0, 1),
        ])
        .unwrap(),
    );
    assert!(manhattan_boolean(
        &geometry,
        &LayoutGeometry::Rect(LayoutRect::new(0, 0, 1, 1).unwrap()),
        ManhattanBooleanOp::Union
    )
    .is_err());
}

#[test]
fn manhattan_boolean_operations_are_exact() {
    let l_shape = LayoutGeometry::Polygon(
        LayoutPolygon::new([
            LayoutPoint::new(0, 0),
            LayoutPoint::new(30, 0),
            LayoutPoint::new(30, 10),
            LayoutPoint::new(10, 10),
            LayoutPoint::new(10, 30),
            LayoutPoint::new(0, 30),
        ])
        .unwrap(),
    );
    let rectangle = LayoutGeometry::Rect(LayoutRect::new(20, 0, 40, 20).unwrap());
    for (operation, expected_area) in [
        (ManhattanBooleanOp::Union, 800),
        (ManhattanBooleanOp::Intersection, 100),
        (ManhattanBooleanOp::Difference, 400),
        (ManhattanBooleanOp::SymmetricDifference, 700),
    ] {
        let result = manhattan_boolean(&l_shape, &rectangle, operation).unwrap();
        assert_eq!(result.area, expected_area, "{operation:?}");
        for rect in &result.rectangles {
            assert!(rect.width() > 0 && rect.height() > 0);
        }
    }
    let union = manhattan_union(&[
        l_shape,
        LayoutGeometry::Rect(LayoutRect::new(100, 0, 120, 10).unwrap()),
    ])
    .unwrap();
    assert_eq!(union.area, 700);
    assert_eq!(union.rectangles.len(), 3);
}

#[test]
fn layout_fix_suggestions_are_applicable() {
    let mut document = LayoutDocument::new();
    document.set_layer_rule(rule()).unwrap();
    document
        .add_shape(LayoutShape {
            id: "narrow".into(),
            layer: "M1".into(),
            net: "a".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(0, 0, 5, 40).unwrap()),
        })
        .unwrap();
    let suggestion = document
        .suggest_layout_fixes()
        .into_iter()
        .next()
        .expect("minimum geometry suggestion");
    assert_eq!(suggestion.code, "LAYOUT.GROW_SHAPE");
    let mut editor = LayoutEditor::new(document).unwrap();
    editor.apply(suggestion.edit).unwrap();
    assert!(editor.document().check_layout_drc().is_clean());

    let mut document = LayoutDocument::new();
    document.set_layer_rule(rule()).unwrap();
    document
        .add_shape(LayoutShape {
            id: "a".into(),
            layer: "M1".into(),
            net: "a".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(0, 0, 50, 50).unwrap()),
        })
        .unwrap();
    document
        .add_shape(LayoutShape {
            id: "b".into(),
            layer: "M1".into(),
            net: "b".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(60, 0, 110, 50).unwrap()),
        })
        .unwrap();
    let suggestion = document
        .suggest_layout_fixes()
        .into_iter()
        .next()
        .expect("spacing suggestion");
    assert_eq!(suggestion.code, "LAYOUT.MOVE_FOR_SPACING");
    let mut editor = LayoutEditor::new(document).unwrap();
    editor.apply(suggestion.edit).unwrap();
    assert!(editor.document().check_layout_drc().is_clean());

    let mut document = LayoutDocument::new();
    document.set_layer_rule(rule()).unwrap();
    document
        .set_layer_rule(LayoutLayerRule::new("M2", 10, 20, 100).unwrap())
        .unwrap();
    document
        .set_via_rule(LayoutViaRule::new("VIA1", "V1", "M1", "M2", 20, 20, 5, 5).unwrap())
        .unwrap();
    document
        .add_via(LayoutVia {
            id: "via".into(),
            rule: "VIA1".into(),
            net: "vdd".into(),
            cut_layer: "V1".into(),
            bottom_layer: "M1".into(),
            top_layer: "M2".into(),
            cut: LayoutRect::new(100, 100, 120, 120).unwrap(),
        })
        .unwrap();
    let suggestions = document.suggest_layout_fixes();
    assert_eq!(suggestions.len(), 2);
    let mut editor = LayoutEditor::new(document).unwrap();
    for suggestion in suggestions {
        editor.apply(suggestion.edit).unwrap();
    }
    assert!(editor.document().check_layout_drc().is_clean());
}

#[test]
fn clean_via_enclosure_and_layout_undo_redo_work() {
    let mut document = LayoutDocument::new();
    document.set_layer_rule(rule()).unwrap();
    document
        .set_layer_rule(LayoutLayerRule::new("M2", 10, 20, 100).unwrap())
        .unwrap();
    document
        .set_via_rule(LayoutViaRule::new("VIA1", "V1", "M1", "M2", 20, 20, 5, 5).unwrap())
        .unwrap();
    document
        .add_shape(LayoutShape {
            id: "bottom".into(),
            layer: "M1".into(),
            net: "vdd".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(95, 95, 125, 125).unwrap()),
        })
        .unwrap();
    document
        .add_shape(LayoutShape {
            id: "top".into(),
            layer: "M2".into(),
            net: "vdd".into(),
            geometry: LayoutGeometry::Rect(LayoutRect::new(95, 95, 125, 125).unwrap()),
        })
        .unwrap();
    let mut editor = LayoutEditor::new(document).unwrap();
    editor
        .apply(LayoutEdit::AddVia(LayoutVia {
            id: "via".into(),
            rule: "VIA1".into(),
            net: "vdd".into(),
            cut_layer: "V1".into(),
            bottom_layer: "M1".into(),
            top_layer: "M2".into(),
            cut: LayoutRect::new(100, 100, 120, 120).unwrap(),
        }))
        .unwrap();
    assert!(editor.document().check_layout_drc().is_clean());
    assert!(editor.undo().unwrap());
    assert!(editor.document().vias().is_empty());
    assert!(editor.redo().unwrap());
    assert_eq!(editor.document().vias().len(), 1);
}

#[test]
fn layout_session_draws_drags_checks_and_renders() {
    let mut document = LayoutDocument::new();
    document.set_layer_rule(rule()).unwrap();
    let mut session = LayoutSession::new(document, "M1", "a", "").unwrap();
    session
        .process(LayoutEvent::SetTool {
            tool: LayoutTool::Rectangle,
        })
        .unwrap();
    assert_eq!(
        session
            .process(LayoutEvent::PointerDown {
                point: LayoutPoint::new(0, 0)
            })
            .unwrap(),
        LayoutEventOutcome::RectanglePreviewChanged {
            start: LayoutPoint::new(0, 0),
            end: LayoutPoint::new(0, 0)
        }
    );
    assert_eq!(
        session
            .process(LayoutEvent::PointerUp {
                point: LayoutPoint::new(30, 20)
            })
            .unwrap(),
        LayoutEventOutcome::ShapeAdded {
            id: "shape1".into()
        }
    );
    assert_eq!(
        session.process(LayoutEvent::CheckDrc).unwrap(),
        LayoutEventOutcome::DrcChecked {
            clean: true,
            error_count: 0
        }
    );
    session
        .process(LayoutEvent::SetTool {
            tool: LayoutTool::Select,
        })
        .unwrap();
    session
        .process(LayoutEvent::PointerDown {
            point: LayoutPoint::new(10, 10),
        })
        .unwrap();
    session
        .process(LayoutEvent::PointerMove {
            point: LayoutPoint::new(15, 12),
        })
        .unwrap();
    assert_eq!(
        session
            .process(LayoutEvent::PointerUp {
                point: LayoutPoint::new(15, 12)
            })
            .unwrap(),
        LayoutEventOutcome::ShapeMoved {
            id: "shape1".into()
        }
    );
    assert_eq!(session.document().shapes()[0].geometry.bbox().x1, 5);
    assert!(session.render_svg().unwrap().contains("<svg"));
    assert_eq!(
        session.process(LayoutEvent::Delete).unwrap(),
        LayoutEventOutcome::Deleted {
            selection: LayoutSelection::Shape {
                id: "shape1".into()
            }
        }
    );
    assert!(session.document().shapes().is_empty());
}

#[test]
fn layout_session_places_rule_sized_via() {
    let mut document = LayoutDocument::new();
    document.set_layer_rule(rule()).unwrap();
    document
        .set_layer_rule(LayoutLayerRule::new("M2", 10, 20, 100).unwrap())
        .unwrap();
    document
        .set_via_rule(LayoutViaRule::new("VIA1", "V1", "M1", "M2", 20, 20, 5, 5).unwrap())
        .unwrap();
    let mut session = LayoutSession::new(document, "M1", "vdd", "VIA1").unwrap();
    session
        .process(LayoutEvent::SetTool {
            tool: LayoutTool::Via,
        })
        .unwrap();
    assert_eq!(
        session
            .process(LayoutEvent::PointerUp {
                point: LayoutPoint::new(100, 100)
            })
            .unwrap(),
        LayoutEventOutcome::ViaAdded { id: "via1".into() }
    );
    assert_eq!(
        session.document().vias()[0].cut.bbox(),
        LayoutRect::new(90, 90, 110, 110).unwrap()
    );
    let result = session.document().check_layout_drc();
    assert_eq!(result.error_count(), 2);
}

#[test]
fn layout_session_draws_and_edits_polygon_vertices() {
    let mut document = LayoutDocument::new();
    document.set_layer_rule(rule()).unwrap();
    let mut session = LayoutSession::new(document, "M1", "a", "").unwrap();
    session
        .process(LayoutEvent::SetTool {
            tool: LayoutTool::Polygon,
        })
        .unwrap();
    let clicks = [
        LayoutPoint::new(0, 0),
        LayoutPoint::new(30, 0),
        LayoutPoint::new(30, 10),
        LayoutPoint::new(10, 10),
        LayoutPoint::new(10, 30),
        LayoutPoint::new(0, 30),
    ];
    for click in clicks {
        session
            .process(LayoutEvent::PointerDown { point: click })
            .unwrap();
    }
    assert_eq!(
        session.process(LayoutEvent::FinishPolygon).unwrap(),
        LayoutEventOutcome::ShapeAdded {
            id: "shape1".into()
        }
    );
    session
        .process(LayoutEvent::SetTool {
            tool: LayoutTool::Select,
        })
        .unwrap();
    session
        .process(LayoutEvent::PointerDown {
            point: LayoutPoint::new(30, 10),
        })
        .unwrap();
    assert_eq!(
        session.selection(),
        &LayoutSelection::ShapeVertex {
            id: "shape1".into(),
            vertex: 2
        }
    );
    assert_eq!(
        session
            .process(LayoutEvent::PointerUp {
                point: LayoutPoint::new(35, 12)
            })
            .unwrap(),
        LayoutEventOutcome::ShapeVertexMoved {
            id: "shape1".into(),
            vertex: 2,
            point: LayoutPoint::new(30, 12)
        }
    );
    let LayoutGeometry::Polygon(polygon) = &session.document().shapes()[0].geometry else {
        panic!("polygon tool did not create polygon");
    };
    assert_eq!(polygon.points[2], LayoutPoint::new(30, 12));
    assert_eq!(polygon.points[3], LayoutPoint::new(10, 12));
    assert!(session.render_svg().unwrap().contains("<polygon"));
}

#[test]
fn layout_session_draws_and_edits_arbitrary_angle_vertices() {
    let mut document = LayoutDocument::new();
    document.set_layer_rule(rule()).unwrap();
    let mut session = LayoutSession::new(document, "M1", "a", "").unwrap();
    session
        .process(LayoutEvent::SetTool {
            tool: LayoutTool::AnyAnglePolygon,
        })
        .unwrap();
    for point in [
        LayoutPoint::new(0, 0),
        LayoutPoint::new(20, 5),
        LayoutPoint::new(5, 20),
    ] {
        session.process(LayoutEvent::PointerDown { point }).unwrap();
    }
    assert_eq!(
        session.process(LayoutEvent::FinishPolygon).unwrap(),
        LayoutEventOutcome::ShapeAdded {
            id: "shape1".into()
        }
    );
    let LayoutGeometry::AnyAngle(polygon) = &session.document().shapes()[0].geometry else {
        panic!("any-angle tool did not create AnyAngle geometry");
    };
    assert_eq!(polygon.signed_area2().unwrap(), 375);
    assert!(session.render_svg().unwrap().contains("<polygon"));

    session
        .process(LayoutEvent::SetTool {
            tool: LayoutTool::Select,
        })
        .unwrap();
    session
        .process(LayoutEvent::PointerDown {
            point: LayoutPoint::new(20, 5),
        })
        .unwrap();
    assert_eq!(
        session.selection(),
        &LayoutSelection::ShapeVertex {
            id: "shape1".into(),
            vertex: 1
        }
    );
    assert_eq!(
        session
            .process(LayoutEvent::PointerUp {
                point: LayoutPoint::new(24, 9),
            })
            .unwrap(),
        LayoutEventOutcome::ShapeVertexMoved {
            id: "shape1".into(),
            vertex: 1,
            point: LayoutPoint::new(24, 9)
        }
    );
    let LayoutGeometry::AnyAngle(polygon) = &session.document().shapes()[0].geometry else {
        panic!("arbitrary-angle geometry changed kind");
    };
    assert_eq!(polygon.points()[1], LayoutPoint::new(24, 9));
}
