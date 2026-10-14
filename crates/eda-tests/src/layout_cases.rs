//! Polygon- and via-level layout DRC regression cases.

use crate::CaseList;

fn spacing_document(separation: i64) -> Result<eda_api::LayoutDrcResult, String> {
    let mut layout = eda_api::LayoutDocument::new();
    layout.set_layer_rule(eda_api::LayoutLayerRule::new("M1", 10, 50, 100)?)?;
    layout.add_shape(eda_api::LayoutShape {
        id: "left".into(),
        layer: "M1".into(),
        net: "a".into(),
        geometry: eda_api::LayoutGeometry::Rect(eda_api::LayoutRect::new(0, 0, 50, 50)?),
    })?;
    layout.add_shape(eda_api::LayoutShape {
        id: "right".into(),
        layer: "M1".into(),
        net: "b".into(),
        geometry: eda_api::LayoutGeometry::Rect(eda_api::LayoutRect::new(
            50 + separation,
            0,
            100 + separation,
            50,
        )?),
    })?;
    Ok(layout.check_layout_drc())
}

fn via_enclosure_document(margin: i64) -> Result<eda_api::LayoutDrcResult, String> {
    let mut layout = eda_api::LayoutDocument::new();
    layout.set_layer_rule(eda_api::LayoutLayerRule::new("M1", 10, 50, 100)?)?;
    layout.set_layer_rule(eda_api::LayoutLayerRule::new("M2", 10, 50, 100)?)?;
    layout.set_via_rule(eda_api::LayoutViaRule::new(
        "VIA1", "V1", "M1", "M2", 20, 20, 25, 25,
    )?)?;
    for (id, layer) in [("bottom", "M1"), ("top", "M2")] {
        layout.add_shape(eda_api::LayoutShape {
            id: id.into(),
            layer: layer.into(),
            net: "vdd".into(),
            geometry: eda_api::LayoutGeometry::Rect(eda_api::LayoutRect::new(
                100 - margin,
                100 - margin,
                120 + margin,
                120 + margin,
            )?),
        })?;
    }
    layout.add_via(eda_api::LayoutVia {
        id: "via".into(),
        rule: "VIA1".into(),
        net: "vdd".into(),
        cut_layer: "V1".into(),
        bottom_layer: "M1".into(),
        top_layer: "M2".into(),
        cut: eda_api::LayoutRect::new(100, 100, 120, 120)?,
    })?;
    Ok(layout.check_layout_drc())
}

pub fn register(c: &mut CaseList) {
    any_angle_polygon_structure(c);
    any_angle_polygon_spacing(c);
    any_angle_layout_document(c);
    any_angle_editor_sessions(c);

    for separation in 0_i64..100 {
        c.add(format!("layout_spacing_{separation:03}"), move || {
            let result = spacing_document(separation)?;
            let spacing_count = result
                .violations
                .iter()
                .filter(|violation| violation.code == "LAYOUT.SPACING")
                .count();
            let expected = usize::from(separation < 50);
            if spacing_count != expected || result.error_count() != expected {
                Err(format!(
                    "separation={} result={:?}",
                    separation, result.violations
                ))
            } else {
                Ok(())
            }
        });
    }

    for margin in 0_i64..100 {
        c.add(format!("layout_via_enclosure_{margin:03}"), move || {
            let result = via_enclosure_document(margin)?;
            let enclosure_count = result
                .violations
                .iter()
                .filter(|violation| violation.code == "LAYOUT.VIA_ENCLOSURE")
                .count();
            let expected = if margin < 25 { 2 } else { 0 };
            if enclosure_count != expected || result.error_count() != expected {
                Err(format!("margin={} result={:?}", margin, result.violations))
            } else {
                Ok(())
            }
        });
    }

    for offset in 0_i64..100 {
        c.add(format!("layout_boolean_rectangle_{offset:03}"), move || {
            let left = eda_api::LayoutGeometry::Rect(eda_api::LayoutRect::new(0, 0, 50, 50)?);
            let right = eda_api::LayoutGeometry::Rect(eda_api::LayoutRect::new(
                offset,
                0,
                offset + 50,
                50,
            )?);
            let overlap = (50 - offset).max(0) * 50;
            for (operation, expected) in [
                (
                    eda_api::ManhattanBooleanOp::Union,
                    if offset < 50 { 5_000 - overlap } else { 5_000 },
                ),
                (eda_api::ManhattanBooleanOp::Intersection, overlap),
                (eda_api::ManhattanBooleanOp::Difference, 2_500 - overlap),
                (
                    eda_api::ManhattanBooleanOp::SymmetricDifference,
                    5_000 - 2 * overlap,
                ),
            ] {
                let result = eda_api::manhattan_boolean(&left, &right, operation)?;
                if result.area != expected {
                    return Err(format!(
                        "offset={} op={:?} area={} expected={}",
                        offset, operation, result.area, expected
                    ));
                }
                if result
                    .rectangles
                    .iter()
                    .any(|rect| rect.x1 >= rect.x2 || rect.y1 >= rect.y2)
                {
                    return Err(format!("offset={} produced an empty rectangle", offset));
                }
            }
            Ok(())
        });
    }

    for index in 0_i64..100 {
        c.add(format!("layout_browser_rectangle_{index:03}"), move || {
            let mut document = eda_api::LayoutDocument::new();
            document.set_layer_rule(eda_api::LayoutLayerRule::new("M1", 10, 50, 100)?)?;
            let service = eda_api::LayoutBrowserService::new(document, "M1", "a", "")?;
            let start = index;
            let end = index + 60;
            let bodies = [
                r#"{"type":"setTool","tool":"rectangle"}"#.to_string(),
                format!("{{\"type\":\"pointerDown\",\"point\":{{\"x\":{start},\"y\":0}}}}"),
                format!("{{\"type\":\"pointerUp\",\"point\":{{\"x\":{end},\"y\":40}}}}"),
            ];
            for body in bodies {
                let request = format!(
                    "POST /event HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let response = service.handle_request(&request);
                if response.status != "200 OK" {
                    return Err(format!(
                        "layout browser event failed: {} {}",
                        response.status,
                        String::from_utf8_lossy(&response.body)
                    ));
                }
            }
            Ok(())
        });
    }

    for index in 0_i64..100 {
        c.add(format!("layout_browser_polygon_{index:03}"), move || {
            let mut document = eda_api::LayoutDocument::new();
            document.set_layer_rule(eda_api::LayoutLayerRule::new("M1", 10, 50, 100)?)?;
            let service = eda_api::LayoutBrowserService::new(document, "M1", "a", "")?;
            let height = 20 + index % 20;
            let bodies = [
                r#"{"type":"setTool","tool":"polygon"}"#.to_string(),
                r#"{"type":"pointerDown","point":{"x":0,"y":0}}"#.to_string(),
                r#"{"type":"pointerDown","point":{"x":60,"y":0}}"#.to_string(),
                format!("{{\"type\":\"pointerDown\",\"point\":{{\"x\":60,\"y\":{height}}}}}"),
                format!("{{\"type\":\"pointerDown\",\"point\":{{\"x\":0,\"y\":{height}}}}}"),
                r#"{"type":"finishPolygon"}"#.to_string(),
            ];
            for body in bodies {
                let request = format!(
                    "POST /event HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let response = service.handle_request(&request);
                if response.status != "200 OK" {
                    return Err(format!(
                        "polygon browser event failed: {} {}",
                        response.status,
                        String::from_utf8_lossy(&response.body)
                    ));
                }
            }
            Ok(())
        });
    }

    c.add("layout_browser_property_grid", || {
        let mut document = eda_api::LayoutDocument::new();
        document.set_layer_rule(eda_api::LayoutLayerRule::new("M1", 10, 50, 100)?)?;
        let service = eda_api::LayoutBrowserService::new(document, "M1", "net7", "")?;
        let bodies = [
            r#"{"type":"setTool","tool":"rectangle"}"#.to_string(),
            r#"{"type":"pointerDown","point":{"x":0,"y":0}}"#.to_string(),
            r#"{"type":"pointerUp","point":{"x":50,"y":40}}"#.to_string(),
            r#"{"type":"setTool","tool":"select"}"#.to_string(),
            r#"{"type":"pointerDown","point":{"x":25,"y":20}}"#.to_string(),
        ];
        for body in bodies {
            let request = format!(
                "POST /event HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let response = service.handle_request(&request);
            if response.status != "200 OK" {
                return Err(format!(
                    "{} {}",
                    response.status,
                    String::from_utf8_lossy(&response.body)
                ));
            }
        }
        let selected = service.handle_request("GET /properties HTTP/1.1\r\nHost: x\r\n\r\n");
        let text = String::from_utf8_lossy(&selected.body).to_string();
        if !text.contains("\"selection\":\"shape\"")
            || !text.contains("\"id\":\"shape1\"")
            || !text.contains("\"layer\":\"M1\"")
            || !text.contains("\"net\":\"net7\"")
            || !text.contains("\"area\":\"2000\"")
        {
            return Err(text);
        }
        Ok(())
    });
}

fn any_angle_polygon_structure(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("layout_any_angle_polygon_{index:03}"), move || {
            let variant = index % 5;
            let a = 1 + (index % 7) as i64;
            let b = 1 + ((index / 7) % 5) as i64;
            let height = 4 * (3 + ((index / 5) % 9) as i64);
            let width = 8 + 2 * (index % 9) as i64;
            let radius = 4 + (index % 11) as i64;
            let (polygon, expected_area2, inside, outside, boundary) = match variant {
                0 => {
                    let polygon = eda_api::AnyAnglePolygon::new([
                        eda_api::LayoutPoint::new(0, 0),
                        eda_api::LayoutPoint::new(a + b, 0),
                        eda_api::LayoutPoint::new(0, height),
                    ])?;
                    (
                        polygon,
                        i128::from((a + b) * height),
                        eda_api::LayoutPoint::new(a / 2, height / 4),
                        eda_api::LayoutPoint::new(a + b, height),
                        eda_api::LayoutPoint::new(0, 0),
                    )
                }
                1 => {
                    let polygon = eda_api::AnyAnglePolygon::new([
                        eda_api::LayoutPoint::new(0, 0),
                        eda_api::LayoutPoint::new(a + b, 0),
                        eda_api::LayoutPoint::new(a, height),
                        eda_api::LayoutPoint::new(0, height),
                    ])?;
                    (
                        polygon,
                        i128::from(height * (2 * a + b)),
                        eda_api::LayoutPoint::new(a, height / 2),
                        eda_api::LayoutPoint::new(a + b, height),
                        eda_api::LayoutPoint::new(a, height),
                    )
                }
                2 => {
                    let polygon = eda_api::AnyAnglePolygon::new([
                        eda_api::LayoutPoint::new(0, 0),
                        eda_api::LayoutPoint::new(width, 0),
                        eda_api::LayoutPoint::new(width, height),
                        eda_api::LayoutPoint::new(width / 2, height / 4),
                        eda_api::LayoutPoint::new(0, height),
                    ])?;
                    (
                        polygon,
                        i128::from(5 * width * height / 4),
                        eda_api::LayoutPoint::new(width / 2, height / 8),
                        eda_api::LayoutPoint::new(width / 2, height / 2),
                        eda_api::LayoutPoint::new(width / 2, height / 4),
                    )
                }
                3 => {
                    let polygon = eda_api::AnyAnglePolygon::new([
                        eda_api::LayoutPoint::new(radius, 0),
                        eda_api::LayoutPoint::new(2 * radius, radius),
                        eda_api::LayoutPoint::new(radius, 2 * radius),
                        eda_api::LayoutPoint::new(0, radius),
                    ])?;
                    (
                        polygon,
                        i128::from(4 * radius * radius),
                        eda_api::LayoutPoint::new(radius, radius),
                        eda_api::LayoutPoint::new(2 * radius, 2 * radius),
                        eda_api::LayoutPoint::new(radius, 0),
                    )
                }
                _ => {
                    let polygon = eda_api::AnyAnglePolygon::new([
                        eda_api::LayoutPoint::new(0, 0),
                        eda_api::LayoutPoint::new(0, height),
                        eda_api::LayoutPoint::new(a + b, 0),
                    ])?;
                    (
                        polygon,
                        -i128::from((a + b) * height),
                        eda_api::LayoutPoint::new(a / 2, height / 4),
                        eda_api::LayoutPoint::new(a + b, height),
                        eda_api::LayoutPoint::new(a + b, 0),
                    )
                }
            };

            if polygon.signed_area2()? != expected_area2 {
                return Err(format!(
                    "area2={:?}, expected {expected_area2}",
                    polygon.signed_area2()
                ));
            }
            if (polygon.area()? - expected_area2.unsigned_abs() as f64 / 2.0).abs() > 1e-12 {
                return Err(format!("float area={:?}", polygon.area()));
            }
            if !polygon.contains_point(inside) || polygon.contains_point(outside) {
                return Err(format!(
                    "containment inside={inside:?}, outside={outside:?}"
                ));
            }
            if !polygon.contains_point(boundary) {
                return Err(format!("boundary {boundary:?} was excluded"));
            }
            if polygon.is_clockwise()? != (expected_area2 < 0) {
                return Err("clockwise orientation mismatch".into());
            }

            let dx = 3 + (index % 5) as i64;
            let dy = 7 + ((index / 5) % 6) as i64;
            let translated = polygon.translated(dx, dy)?;
            if translated.signed_area2()? != expected_area2
                || !translated
                    .contains_point(eda_api::LayoutPoint::new(inside.x + dx, inside.y + dy))
            {
                return Err("translation changed area or containment".into());
            }
            let bbox = polygon.bbox();
            let translated_bbox = translated.bbox();
            if translated_bbox.x1 - bbox.x1 != dx
                || translated_bbox.y1 - bbox.y1 != dy
                || translated_bbox.x2 - bbox.x2 != dx
                || translated_bbox.y2 - bbox.y2 != dy
            {
                return Err("bounding-box translation mismatch".into());
            }
            if variant == 3
                && polygon
                    .edge_lengths_squared()
                    .into_iter()
                    .any(|length| length != 2 * radius as u128 * radius as u128)
            {
                return Err("diamond edge lengths are not equal".into());
            }

            let bowtie = eda_api::AnyAnglePolygon::new([
                eda_api::LayoutPoint::new(0, 0),
                eda_api::LayoutPoint::new(width, 0),
                eda_api::LayoutPoint::new(0, height),
                eda_api::LayoutPoint::new(width, height),
            ]);
            let repeated = eda_api::AnyAnglePolygon::new([
                eda_api::LayoutPoint::new(0, 0),
                eda_api::LayoutPoint::new(width, 0),
                eda_api::LayoutPoint::new(0, height),
                eda_api::LayoutPoint::new(0, 0),
            ]);
            if bowtie.is_ok() || repeated.is_ok() {
                return Err("invalid any-angle polygon was accepted".into());
            }
            Ok(())
        });
    }
}

fn any_angle_polygon_spacing(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("layout_any_angle_spacing_{index:03}"), move || {
            let variant = index % 5;
            let gap = 1 + (index % 19) as i64;
            let a = 1 + (index % 7) as i64;
            let b = 1 + ((index / 7) % 5) as i64;
            let height = 4 * (3 + ((index / 5) % 9) as i64);
            let width = 8 + 2 * (index % 9) as i64;
            let radius = 4 + (index % 11) as i64;
            let left = match variant {
                0 => eda_api::AnyAnglePolygon::new([
                    eda_api::LayoutPoint::new(0, 0),
                    eda_api::LayoutPoint::new(a + b, 0),
                    eda_api::LayoutPoint::new(0, height),
                ])?,
                1 => eda_api::AnyAnglePolygon::new([
                    eda_api::LayoutPoint::new(radius, 0),
                    eda_api::LayoutPoint::new(2 * radius, radius),
                    eda_api::LayoutPoint::new(radius, 2 * radius),
                    eda_api::LayoutPoint::new(0, radius),
                ])?,
                2 => eda_api::AnyAnglePolygon::new([
                    eda_api::LayoutPoint::new(0, 0),
                    eda_api::LayoutPoint::new(a + b, 0),
                    eda_api::LayoutPoint::new(a, height),
                    eda_api::LayoutPoint::new(0, height),
                ])?,
                3 => eda_api::AnyAnglePolygon::new([
                    eda_api::LayoutPoint::new(0, 0),
                    eda_api::LayoutPoint::new(width, 0),
                    eda_api::LayoutPoint::new(width, height),
                    eda_api::LayoutPoint::new(width / 2, height / 4),
                    eda_api::LayoutPoint::new(0, height),
                ])?,
                _ => eda_api::AnyAnglePolygon::new([
                    eda_api::LayoutPoint::new(0, 0),
                    eda_api::LayoutPoint::new(0, 2 * a),
                    eda_api::LayoutPoint::new(2 * a, 2 * a),
                ])?,
            };

            if variant == 4 {
                let right = eda_api::AnyAnglePolygon::new([
                    eda_api::LayoutPoint::new(2 * a + 1, 0),
                    eda_api::LayoutPoint::new(2 * a + 4, 0),
                    eda_api::LayoutPoint::new(2 * a + 4, 3),
                ])?;
                let expected = (2 * a + 1) as f64 / f64::sqrt(2.0);
                let result = left.check_spacing_to(&right, 1)?;
                if (result.actual_distance_dbu - expected).abs() > 1e-12 || !result.compliant {
                    return Err(format!(
                        "diagonal distance={:?}, expected={expected}",
                        result.actual_distance_dbu
                    ));
                }
                let far_requirement = left.check_spacing_to(&right, 1000)?;
                if far_requirement.compliant {
                    return Err("impossible arbitrary-angle spacing passed".into());
                }
                return Ok(());
            }

            let shape_width = match variant {
                0 | 2 => a + b,
                1 => 2 * radius,
                _ => width,
            };
            let right = left.translated(shape_width + gap, 0)?;
            let result = left.check_spacing_to(&right, gap)?;
            if !result.compliant || (result.actual_distance_dbu - gap as f64).abs() > 1e-12 {
                return Err(format!("variant={variant} gap={gap} result={result:?}"));
            }
            let violating = left.check_spacing_to(&right, gap + 1)?;
            if violating.compliant {
                return Err("spacing below requirement passed".into());
            }
            let touching = left.translated(shape_width, 0)?;
            let touch_result = left.check_spacing_to(&touching, 1)?;
            if touch_result.compliant || (touch_result.actual_distance_dbu - 0.0).abs() > 1e-12 {
                return Err(format!("touching polygons passed: {touch_result:?}"));
            }
            Ok(())
        });
    }
}

fn any_angle_layout_document(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("layout_any_angle_document_{index:03}"), move || {
            let variant = index % 4;
            let a = 1 + (index % 7) as i64;
            let b = 1 + ((index / 7) % 5) as i64;
            let height = 4 * (3 + ((index / 5) % 9) as i64);
            let width = 8 + 2 * (index % 9) as i64;
            let radius = 4 + (index % 11) as i64;
            let (left, inside, shape_width, expected_area2) = match variant {
                0 => {
                    let polygon = eda_api::AnyAnglePolygon::new([
                        eda_api::LayoutPoint::new(0, 0),
                        eda_api::LayoutPoint::new(a + b, 0),
                        eda_api::LayoutPoint::new(0, height),
                    ])?;
                    (
                        polygon,
                        eda_api::LayoutPoint::new(a / 2, height / 4),
                        a + b,
                        i128::from((a + b) * height),
                    )
                }
                1 => {
                    let polygon = eda_api::AnyAnglePolygon::new([
                        eda_api::LayoutPoint::new(radius, 0),
                        eda_api::LayoutPoint::new(2 * radius, radius),
                        eda_api::LayoutPoint::new(radius, 2 * radius),
                        eda_api::LayoutPoint::new(0, radius),
                    ])?;
                    (
                        polygon,
                        eda_api::LayoutPoint::new(radius, radius),
                        2 * radius,
                        i128::from(4 * radius * radius),
                    )
                }
                2 => {
                    let polygon = eda_api::AnyAnglePolygon::new([
                        eda_api::LayoutPoint::new(0, 0),
                        eda_api::LayoutPoint::new(a + b, 0),
                        eda_api::LayoutPoint::new(a, height),
                        eda_api::LayoutPoint::new(0, height),
                    ])?;
                    (
                        polygon,
                        eda_api::LayoutPoint::new(a, height / 2),
                        a + b,
                        i128::from(height * (2 * a + b)),
                    )
                }
                _ => {
                    let polygon = eda_api::AnyAnglePolygon::new([
                        eda_api::LayoutPoint::new(0, 0),
                        eda_api::LayoutPoint::new(width, 0),
                        eda_api::LayoutPoint::new(width, height),
                        eda_api::LayoutPoint::new(width / 2, height / 4),
                        eda_api::LayoutPoint::new(0, height),
                    ])?;
                    (
                        polygon,
                        eda_api::LayoutPoint::new(width / 2, height / 8),
                        width,
                        i128::from(5 * width * height / 4),
                    )
                }
            };
            let spacing = 1 + (index % 19) as i64;
            let right = left.translated(shape_width + spacing, 0)?;
            let mut document = eda_api::LayoutDocument::new();
            document.set_layer_rule(eda_api::LayoutLayerRule::new("M1", 1, spacing, 1)?)?;
            document.add_shape(eda_api::LayoutShape {
                id: "left".into(),
                layer: "M1".into(),
                net: "a".into(),
                geometry: eda_api::LayoutGeometry::AnyAngle(left),
            })?;
            document.add_shape(eda_api::LayoutShape {
                id: "right".into(),
                layer: "M1".into(),
                net: "b".into(),
                geometry: eda_api::LayoutGeometry::AnyAngle(right),
            })?;

            let clean = document.check_layout_drc();
            if !clean.is_clean() {
                return Err(format!(
                    "clean arbitrary-angle DRC failed: {:?}",
                    clean.violations
                ));
            }
            match document.hit_test(inside) {
                Some(shape) if shape.id == "left" => {}
                _ => return Err("arbitrary-angle layout hit test failed".into()),
            }
            if !document.write_svg()?.contains("<polygon points=\"") {
                return Err("arbitrary-angle SVG omitted polygon geometry".into());
            }
            if let Some(shape) = document.shapes().first() {
                if shape.geometry.area2()? != expected_area2 {
                    return Err("arbitrary-angle document area mismatch".into());
                }
            }

            document.move_shape("right", -1, 0)?;
            let violating = document.check_layout_drc();
            if !violating
                .violations
                .iter()
                .any(|violation| violation.code == "LAYOUT.SPACING")
            {
                return Err(format!(
                    "arbitrary-angle spacing violation was omitted: {:?}",
                    violating.violations
                ));
            }
            let first_geometry = document
                .shapes()
                .first()
                .map(|shape| shape.geometry.clone())
                .ok_or("missing arbitrary-angle shape")?;
            let eda_api::LayoutGeometry::AnyAngle(_) = first_geometry else {
                return Err("first arbitrary-angle geometry changed kind".into());
            };
            if eda_api::manhattan_boolean(
                &first_geometry,
                &eda_api::LayoutGeometry::Rect(eda_api::LayoutRect::new(0, 0, 1, 1)?),
                eda_api::ManhattanBooleanOp::Union,
            )
            .is_ok()
            {
                return Err("Manhattan Boolean accepted arbitrary-angle geometry".into());
            }
            Ok(())
        });
    }
}

fn any_angle_editor_sessions(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(format!("layout_any_angle_editor_{index:03}"), move || {
            let variant = index % 4;
            let a = 4 + (index % 7) as i64;
            let b = 4 + ((index / 7) % 5) as i64;
            let height = 12 + 2 * ((index / 5) % 11) as i64;
            let width = 16 + 2 * (index % 9) as i64;
            let points = match variant {
                0 => vec![
                    eda_api::LayoutPoint::new(0, 0),
                    eda_api::LayoutPoint::new(a + b, 3 + (index % 5) as i64),
                    eda_api::LayoutPoint::new(2 + (index % 3) as i64, height),
                ],
                1 => vec![
                    eda_api::LayoutPoint::new(a, 0),
                    eda_api::LayoutPoint::new(2 * a, a + 2),
                    eda_api::LayoutPoint::new(a, 2 * a + 4),
                    eda_api::LayoutPoint::new(0, a + 2),
                ],
                2 => vec![
                    eda_api::LayoutPoint::new(0, 0),
                    eda_api::LayoutPoint::new(a + b, 2 + (index % 4) as i64),
                    eda_api::LayoutPoint::new(a, height),
                    eda_api::LayoutPoint::new(0, height),
                ],
                _ => vec![
                    eda_api::LayoutPoint::new(0, 0),
                    eda_api::LayoutPoint::new(width, 2 + (index % 3) as i64),
                    eda_api::LayoutPoint::new(width, height),
                    eda_api::LayoutPoint::new(width / 2, height / 4 + 1),
                    eda_api::LayoutPoint::new(0, height),
                ],
            };
            let mut document = eda_api::LayoutDocument::new();
            document.set_layer_rule(eda_api::LayoutLayerRule::new("M1", 1, 20, 1)?)?;
            let mut session = eda_api::LayoutSession::new(document, "M1", "net", "")?;
            session.process(eda_api::LayoutEvent::SetTool {
                tool: eda_api::LayoutTool::AnyAnglePolygon,
            })?;
            for (vertex, point) in points.iter().enumerate() {
                let outcome =
                    session.process(eda_api::LayoutEvent::PointerDown { point: *point })?;
                match outcome {
                    eda_api::LayoutEventOutcome::PolygonPreviewChanged { points } => {
                        if points.len() != vertex + 1 || points[vertex] != *point {
                            return Err("any-angle preview sequence mismatch".into());
                        }
                    }
                    other => return Err(format!("unexpected preview outcome: {other:?}")),
                }
            }
            let added = session.process(eda_api::LayoutEvent::FinishPolygon)?;
            match added {
                eda_api::LayoutEventOutcome::ShapeAdded { id } if id == "shape1" => {}
                other => return Err(format!("unexpected finish outcome: {other:?}")),
            }
            let properties = session.properties();
            if properties.get("geometry") != Some(&"any-angle polygon".to_string())
                || properties.get("tool") != Some(&"any-angle polygon".to_string())
            {
                return Err(format!("any-angle properties mismatch: {properties:?}"));
            }
            let geometry = session
                .document()
                .shapes()
                .first()
                .map(|shape| shape.geometry.clone())
                .ok_or("any-angle editor shape is missing")?;
            let eda_api::LayoutGeometry::AnyAngle(polygon) = geometry else {
                return Err("any-angle editor created the wrong geometry".into());
            };
            if polygon.points() != points.as_slice() {
                return Err("any-angle editor changed the clicked point sequence".into());
            }

            let selected_vertex = 1 + index % (points.len() - 1);
            session.process(eda_api::LayoutEvent::SetTool {
                tool: eda_api::LayoutTool::Select,
            })?;
            session.process(eda_api::LayoutEvent::PointerDown {
                point: points[selected_vertex],
            })?;
            let target = eda_api::LayoutPoint::new(
                points[selected_vertex].x + 3,
                points[selected_vertex].y + 4,
            );
            let moved = session.process(eda_api::LayoutEvent::PointerUp { point: target })?;
            match moved {
                eda_api::LayoutEventOutcome::ShapeVertexMoved { vertex, point, .. }
                    if vertex == selected_vertex && point == target => {}
                other => return Err(format!("unexpected vertex move outcome: {other:?}")),
            }
            let geometry = session
                .document()
                .shapes()
                .first()
                .map(|shape| shape.geometry.clone())
                .ok_or("dragged any-angle shape disappeared")?;
            let eda_api::LayoutGeometry::AnyAngle(polygon) = geometry else {
                return Err("drag changed arbitrary-angle geometry kind".into());
            };
            let expected_points = {
                let mut updated = points.clone();
                updated[selected_vertex] = target;
                updated
            };
            if polygon.points() != expected_points.as_slice() {
                return Err(format!(
                    "dragged points={:?}, expected={expected_points:?}",
                    polygon.points()
                ));
            }

            if index % 4 == 0 {
                session.process(eda_api::LayoutEvent::Undo)?;
                let geometry = session
                    .document()
                    .shapes()
                    .first()
                    .map(|shape| shape.geometry.clone())
                    .ok_or("undo dropped any-angle shape")?;
                let eda_api::LayoutGeometry::AnyAngle(restored) = geometry else {
                    return Err("undo changed any-angle geometry kind".into());
                };
                if restored.points() != points.as_slice() {
                    return Err("undo did not restore any-angle vertex".into());
                }
                session.process(eda_api::LayoutEvent::Redo)?;
                let geometry = session
                    .document()
                    .shapes()
                    .first()
                    .map(|shape| shape.geometry.clone())
                    .ok_or("redo dropped any-angle shape")?;
                let eda_api::LayoutGeometry::AnyAngle(redone) = geometry else {
                    return Err("redo changed any-angle geometry kind".into());
                };
                if redone.points() != expected_points.as_slice() {
                    return Err("redo did not restore dragged vertex".into());
                }
            }
            Ok(())
        });
    }
}
